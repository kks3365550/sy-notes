use chrono::{Datelike, Local, Utc};
use rusqlite::backup::Backup;
use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use std::fs::{self, File};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;
use zip::write::SimpleFileOptions;
use zip::ZipWriter;

use crate::doc::Document;

/// Errors occurring within backup and export operations.
#[derive(Debug)]
pub enum BackupError {
    Sqlite(rusqlite::Error),
    Io(io::Error),
    Zip(zip::result::ZipError),
    Serde(serde_json::Error),
    InvalidPath(String),
    LockTimeout(String),
}

impl std::fmt::Display for BackupError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            BackupError::Sqlite(err) => write!(f, "SQLite backup error: {err}"),
            BackupError::Io(err) => write!(f, "I/O error during backup: {err}"),
            BackupError::Zip(err) => write!(f, "ZIP compression error: {err}"),
            BackupError::Serde(err) => write!(f, "Serialization error: {err}"),
            BackupError::InvalidPath(msg) => write!(f, "Invalid backup path: {msg}"),
            BackupError::LockTimeout(msg) => write!(f, "Lock timeout during backup: {msg}"),
        }
    }
}

impl std::error::Error for BackupError {}

impl From<rusqlite::Error> for BackupError {
    fn from(err: rusqlite::Error) -> Self {
        BackupError::Sqlite(err)
    }
}

impl From<io::Error> for BackupError {
    fn from(err: io::Error) -> Self {
        BackupError::Io(err)
    }
}

impl From<zip::result::ZipError> for BackupError {
    fn from(err: zip::result::ZipError) -> Self {
        BackupError::Zip(err)
    }
}

impl From<serde_json::Error> for BackupError {
    fn from(err: serde_json::Error) -> Self {
        BackupError::Serde(err)
    }
}

/// Result metadata for backup rotation.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct RotationResult {
    pub deleted_daily: Vec<PathBuf>,
    pub deleted_weekly: Vec<PathBuf>,
    pub remaining_daily_count: usize,
    pub remaining_weekly_count: usize,
}

/// Result metadata returned after a database backup.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BackupResult {
    pub daily_path: PathBuf,
    pub weekly_path: Option<PathBuf>,
    pub bytes_copied: u64,
    pub rotation: RotationResult,
    pub timestamp: String,
}

/// Result metadata returned after ZIP export.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ExportReport {
    pub export_path: PathBuf,
    pub total_exported: usize,
    pub active_count: usize,
    pub trash_count: usize,
    pub attachments_count: usize,
    pub zip_bytes: u64,
}

/// Performs an SQLite Online Backup from `src_conn` to `dst_path` without locking readers/writers.
/// Uses atomic write pattern (.tmp file -> atomic rename) to prevent corrupted snapshot files.
pub fn online_backup(src_conn: &Connection, dst_path: &Path) -> Result<u64, BackupError> {
    if let Some(parent) = dst_path.parent() {
        fs::create_dir_all(parent)?;
    }

    let tmp_path = dst_path.with_extension("tmp");
    if tmp_path.exists() {
        let _ = fs::remove_file(&tmp_path);
    }

    let mut dst_conn = Connection::open(&tmp_path)?;
    dst_conn.busy_timeout(Duration::from_millis(5000))?;

    // SQLite Online Backup API: step in 100 pages chunks with 10ms yield
    let backup = Backup::new(src_conn, &mut dst_conn)?;
    backup.run_to_completion(100, Duration::from_millis(10), None)?;

    drop(backup);
    drop(dst_conn);

    fs::rename(&tmp_path, dst_path)?;
    let size = fs::metadata(dst_path)?.len();
    Ok(size)
}

/// Rotates backup files keeping at most `max_daily` daily snapshots and `max_weekly` weekly snapshots.
/// Files are sorted lexicographically by timestamp in filename.
pub fn rotate_backups<P: AsRef<Path>>(
    backup_dir: P,
    max_daily: usize,
    max_weekly: usize,
) -> Result<RotationResult, BackupError> {
    let dir = backup_dir.as_ref();
    if !dir.exists() {
        return Ok(RotationResult::default());
    }

    let mut daily_files: Vec<PathBuf> = Vec::new();
    let mut weekly_files: Vec<PathBuf> = Vec::new();

    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        if !path.is_file() {
            continue;
        }

        let file_name = match path.file_name().and_then(|n| n.to_str()) {
            Some(name) => name,
            None => continue,
        };

        // Skip incomplete temporary files
        if file_name.ends_with(".tmp") {
            continue;
        }

        if (file_name.starts_with("daily_") || file_name.starts_with("backup_daily_"))
            && file_name.ends_with(".db")
        {
            daily_files.push(path);
        } else if (file_name.starts_with("weekly_") || file_name.starts_with("backup_weekly_"))
            && file_name.ends_with(".db")
        {
            weekly_files.push(path);
        }
    }

    daily_files.sort();
    weekly_files.sort();

    let mut deleted_daily = Vec::new();
    if daily_files.len() > max_daily {
        let excess = daily_files.len() - max_daily;
        for path in daily_files.drain(..excess) {
            if fs::remove_file(&path).is_ok() {
                deleted_daily.push(path);
            }
        }
    }

    let mut deleted_weekly = Vec::new();
    if weekly_files.len() > max_weekly {
        let excess = weekly_files.len() - max_weekly;
        for path in weekly_files.drain(..excess) {
            if fs::remove_file(&path).is_ok() {
                deleted_weekly.push(path);
            }
        }
    }

    Ok(RotationResult {
        deleted_daily,
        deleted_weekly,
        remaining_daily_count: daily_files.len(),
        remaining_weekly_count: weekly_files.len(),
    })
}

/// Backup database with explicit weekly option (for testing or manual trigger).
pub fn backup_db_with_options<P: AsRef<Path>>(
    conn: &Connection,
    backup_dir: P,
    force_weekly: bool,
) -> Result<BackupResult, BackupError> {
    let dir = backup_dir.as_ref();
    fs::create_dir_all(dir)?;

    let now = Local::now();
    let timestamp_str = now.format("%Y%m%d_%H%M%S").to_string();
    let daily_filename = format!("daily_{}.db", timestamp_str);
    let daily_path = dir.join(&daily_filename);

    let bytes_copied = online_backup(conn, &daily_path)?;

    // Check if today is Sunday (or forced weekly snapshot)
    let is_sunday = force_weekly || (now.weekday() == chrono::Weekday::Sun);
    let weekly_path = if is_sunday {
        let weekly_filename = format!("weekly_{}.db", timestamp_str);
        let wpath = dir.join(&weekly_filename);
        // Bit-identical atomic copy of the newly created snapshot
        fs::copy(&daily_path, &wpath)?;
        Some(wpath)
    } else {
        None
    };

    // Rotation: keep up to 7 daily, 4 weekly
    let rotation = rotate_backups(dir, 7, 4)?;

    Ok(BackupResult {
        daily_path,
        weekly_path,
        bytes_copied,
        rotation,
        timestamp: timestamp_str,
    })
}

/// Backs up the SQLite database to `backup_dir` using SQLite Online Backup API.
/// Performs 7-day daily rolling + Sunday weekly snapshot (max 4 weeks) rotation.
pub fn backup_db<P: AsRef<Path>>(
    conn: &Connection,
    backup_dir: P,
) -> Result<BackupResult, BackupError> {
    backup_db_with_options(conn, backup_dir, false)
}

/// Restores a SQLite database from `backup_file` into `target_db` using SQLite Online Backup API.
pub fn restore_backup<P: AsRef<Path>, Q: AsRef<Path>>(
    backup_file: P,
    target_db: Q,
) -> Result<u64, BackupError> {
    let src_conn = Connection::open(backup_file.as_ref())?;
    let mut dst_conn = Connection::open(target_db.as_ref())?;
    dst_conn.busy_timeout(Duration::from_millis(5000))?;

    let backup = Backup::new(&src_conn, &mut dst_conn)?;
    backup.run_to_completion(100, Duration::from_millis(10), None)?;

    drop(backup);
    drop(dst_conn);

    let size = fs::metadata(target_db.as_ref())?.len();
    Ok(size)
}

/// Sanitizes a title for use in file paths, strictly preventing Directory Traversal
/// and replacing reserved Windows / filesystem characters.
pub fn sanitize_filename(title: &str, fallback_id: &str) -> String {
    let mut sanitized = String::new();
    for c in title.chars() {
        match c {
            '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' => {
                sanitized.push('_');
            }
            c if (c as u32) < 32 => {
                sanitized.push('_');
            }
            _ => sanitized.push(c),
        }
    }

    // Collapse multiple underscores and trim dots/spaces
    let mut cleaned = String::new();
    let mut last_was_underscore = false;
    for c in sanitized.trim().chars() {
        if c == '_' {
            if !last_was_underscore {
                cleaned.push('_');
                last_was_underscore = true;
            }
        } else {
            cleaned.push(c);
            last_was_underscore = false;
        }
    }

    let trimmed = cleaned.trim_matches(|c: char| c == '.' || c == ' ' || c == '_');
    if trimmed.is_empty() {
        fallback_id.to_string()
    } else {
        // Limit length to 80 chars
        let max_len = 80.min(trimmed.len());
        trimmed[..max_len].to_string()
    }
}

/// Converts BlockNote JSON block hierarchy into formatted Markdown.
/// If JSON parsing fails or format differs, falls back gracefully to `fallback_text`.
pub fn blocknote_to_markdown(content_json: &str, fallback_text: &str) -> String {
    let parsed: serde_json::Value = match serde_json::from_str(content_json) {
        Ok(v) => v,
        Err(_) => return fallback_text.to_string(),
    };

    let blocks = match parsed.as_array() {
        Some(arr) if !arr.is_empty() => arr,
        _ => return fallback_text.to_string(),
    };

    let mut md = String::new();

    for block in blocks {
        let block_type = block.get("type").and_then(|t| t.as_str()).unwrap_or("paragraph");
        let mut inline_text = String::new();

        if let Some(content_arr) = block.get("content").and_then(|c| c.as_array()) {
            for item in content_arr {
                let text = item.get("text").and_then(|t| t.as_str()).unwrap_or("");
                if text.is_empty() {
                    continue;
                }

                let mut styled = text.to_string();
                if let Some(styles) = item.get("styles").and_then(|s| s.as_object()) {
                    if styles.get("bold").and_then(|v| v.as_bool()).unwrap_or(false) {
                        styled = format!("**{styled}**");
                    }
                    if styles.get("italic").and_then(|v| v.as_bool()).unwrap_or(false) {
                        styled = format!("*{styled}*");
                    }
                    if styles.get("code").and_then(|v| v.as_bool()).unwrap_or(false) {
                        styled = format!("`{styled}`");
                    }
                    if styles.get("strike").and_then(|v| v.as_bool()).unwrap_or(false) {
                        styled = format!("~~{styled}~~");
                    }
                }
                inline_text.push_str(&styled);
            }
        }

        match block_type {
            "heading" => {
                let level = block
                    .get("props")
                    .and_then(|p| p.get("level"))
                    .and_then(|l| l.as_u64())
                    .unwrap_or(1)
                    .clamp(1, 6);
                let hashes = "#".repeat(level as usize);
                md.push_str(&format!("{hashes} {inline_text}\n\n"));
            }
            "bulletListItem" => {
                md.push_str(&format!("- {inline_text}\n"));
            }
            "numberedListItem" => {
                md.push_str(&format!("1. {inline_text}\n"));
            }
            "checkListItem" => {
                let checked = block
                    .get("props")
                    .and_then(|p| p.get("checked"))
                    .and_then(|c| c.as_bool())
                    .unwrap_or(false);
                let mark = if checked { "x" } else { " " };
                md.push_str(&format!("- [{mark}] {inline_text}\n"));
            }
            "codeBlock" => {
                let lang = block
                    .get("props")
                    .and_then(|p| p.get("language"))
                    .and_then(|l| l.as_str())
                    .unwrap_or("");
                md.push_str(&format!("```{lang}\n{inline_text}\n```\n\n"));
            }
            "blockquote" => {
                md.push_str(&format!("> {inline_text}\n\n"));
            }
            _ => {
                // Default paragraph
                if !inline_text.is_empty() {
                    md.push_str(&format!("{inline_text}\n\n"));
                }
            }
        }
    }

    let trimmed = md.trim();
    if trimmed.is_empty() {
        fallback_text.to_string()
    } else {
        trimmed.to_string()
    }
}

/// Builds markdown representation with YAML frontmatter for a document.
pub fn document_to_markdown(doc: &Document) -> String {
    let body_md = blocknote_to_markdown(&doc.content, &doc.content_text);
    let date_str = doc.date.as_deref().unwrap_or("");
    let deleted_str = match doc.deleted_at {
        Some(ts) => ts.to_string(),
        None => "null".to_string(),
    };

    format!(
        "---\nid: \"{}\"\ntitle: \"{}\"\ndoc_type: \"{}\"\ndate: \"{}\"\ncreated_at: {}\nupdated_at: {}\ndeleted_at: {}\n---\n\n# {}\n\n{}\n",
        doc.id,
        doc.title.replace('"', "\\\""),
        doc.doc_type,
        date_str,
        doc.created_at,
        doc.updated_at,
        deleted_str,
        doc.title,
        body_md
    )
}

/// Exports all documents into a ZIP archive at `export_path`.
/// The archive contains:
/// - `manifest.json`: Summary metadata of exported records.
/// - `all_documents.json`: Full array of documents for complete JSON restore.
/// - `markdown/`: Individual Markdown files with frontmatter.
/// - `json/`: Individual JSON document files.
/// - `trash/`: Soft-deleted documents in Markdown and JSON (if any).
/// - `attachments/`: Associated binary attachments found on disk.
pub fn export_all<P: AsRef<Path>>(
    conn: &Connection,
    export_path: P,
) -> Result<ExportReport, BackupError> {
    let dest = export_path.as_ref();
    if let Some(parent) = dest.parent() {
        fs::create_dir_all(parent)?;
    }

    let file = File::create(dest)?;
    let mut zip = ZipWriter::new(file);
    let zip_options = SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated);

    // Query all documents
    let mut stmt = conn.prepare(
        "SELECT id, title, content, content_text, doc_type, date, created_at, updated_at, deleted_at
         FROM documents
         ORDER BY updated_at DESC",
    )?;

    let docs: Vec<Document> = stmt
        .query_map([], |row| {
            Ok(Document {
                id: row.get(0)?,
                title: row.get(1)?,
                content: row.get(2)?,
                content_text: row.get(3)?,
                doc_type: row.get(4)?,
                date: row.get(5)?,
                created_at: row.get(6)?,
                updated_at: row.get(7)?,
                deleted_at: row.get(8)?,
            })
        })?
        .filter_map(|r| r.ok())
        .collect();

    let mut active_count = 0;
    let mut trash_count = 0;

    for doc in &docs {
        let is_deleted = doc.deleted_at.is_some();
        if is_deleted {
            trash_count += 1;
        } else {
            active_count += 1;
        }

        let prefix = if is_deleted { "trash/" } else { "" };
        let short_id = if doc.id.len() >= 8 {
            &doc.id[..8]
        } else {
            &doc.id
        };
        let safe_title = sanitize_filename(&doc.title, short_id);
        let base_name = if let Some(ref d) = doc.date {
            format!("[{}]_{}_{}", d, safe_title, short_id)
        } else {
            format!("{}_{}", safe_title, short_id)
        };

        // Write Markdown file
        let md_path = format!("{}markdown/{}.md", prefix, base_name);
        let md_content = document_to_markdown(doc);
        zip.start_file(&md_path, zip_options)?;
        zip.write_all(md_content.as_bytes())?;

        // Write JSON file
        let json_path = format!("{}json/{}.json", prefix, base_name);
        let json_content = serde_json::to_string_pretty(doc)?;
        zip.start_file(&json_path, zip_options)?;
        zip.write_all(json_content.as_bytes())?;
    }

    // Write all_documents.json at root
    let all_docs_json = serde_json::to_string_pretty(&docs)?;
    zip.start_file("all_documents.json", zip_options)?;
    zip.write_all(all_docs_json.as_bytes())?;

    // Try exporting attachments if table exists
    let mut attachments_count = 0;
    let mut has_attachments_table = false;
    if let Ok(mut chk) = conn.prepare("SELECT count(*) FROM sqlite_master WHERE type='table' AND name='attachments'") {
        if let Ok(count) = chk.query_row([], |r| r.get::<_, i64>(0)) {
            has_attachments_table = count > 0;
        }
    }

    if has_attachments_table {
        if let Ok(mut att_stmt) = conn.prepare("SELECT file_path, file_name FROM attachments") {
            if let Ok(att_rows) = att_stmt.query_map([], |r| {
                Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
            }) {
                for att in att_rows.flatten() {
                    let (stored_path, file_name) = att;
                    let p = Path::new(&stored_path);
                    if p.exists() && p.is_file() {
                        if let Ok(bytes) = fs::read(p) {
                            let safe_name = sanitize_filename(&file_name, "asset");
                            let ext = p.extension().and_then(|e| e.to_str()).unwrap_or("bin");
                            let zip_att_name = format!("attachments/{}_{}.{}", safe_name, attachments_count, ext);
                            if zip.start_file(&zip_att_name, zip_options).is_ok() {
                                let _ = zip.write_all(&bytes);
                                attachments_count += 1;
                            }
                        }
                    }
                }
            }
        }
    }

    // Write manifest.json
    let manifest = serde_json::json!({
        "version": "1.0",
        "exported_at": Utc::now().to_rfc3339(),
        "total_documents": docs.len(),
        "active_documents": active_count,
        "trash_documents": trash_count,
        "attachments_count": attachments_count,
    });
    zip.start_file("manifest.json", zip_options)?;
    zip.write_all(serde_json::to_string_pretty(&manifest)?.as_bytes())?;

    zip.finish()?;

    let zip_bytes = fs::metadata(dest)?.len();

    Ok(ExportReport {
        export_path: dest.to_path_buf(),
        total_exported: docs.len(),
        active_count,
        trash_count,
        attachments_count,
        zip_bytes,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::Connection;

    struct TempTestDir {
        path: PathBuf,
    }

    impl TempTestDir {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!("sy_test_{}", uuid::Uuid::new_v4()));
            let _ = fs::create_dir_all(&path);
            Self { path }
        }

        fn path(&self) -> &Path {
            &self.path
        }
    }

    impl Drop for TempTestDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.path);
        }
    }

    fn setup_test_db() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE documents (
                id TEXT PRIMARY KEY,
                title TEXT NOT NULL,
                content TEXT NOT NULL DEFAULT '',
                content_text TEXT NOT NULL DEFAULT '',
                doc_type TEXT NOT NULL DEFAULT 'memo',
                date TEXT,
                created_at INTEGER NOT NULL,
                updated_at INTEGER NOT NULL,
                deleted_at INTEGER
            );
            CREATE TABLE IF NOT EXISTS attachments (
                id TEXT PRIMARY KEY,
                doc_id TEXT NOT NULL,
                file_name TEXT NOT NULL,
                file_path TEXT NOT NULL,
                file_size INTEGER NOT NULL DEFAULT 0,
                created_at INTEGER NOT NULL
            );",
        )
        .unwrap();

        conn.execute(
            "INSERT INTO documents (id, title, content, content_text, doc_type, date, created_at, updated_at, deleted_at)
             VALUES ('doc-1', '테스트 업무일지', '[{\"type\":\"paragraph\",\"content\":[{\"text\":\"업무 완료\"}]}]', '업무 완료', 'daily', '2026-11-22', 1700000000, 1700000000, NULL)",
            [],
        ).unwrap();

        conn.execute(
            "INSERT INTO documents (id, title, content, content_text, doc_type, date, created_at, updated_at, deleted_at)
             VALUES ('doc-2', '삭제된 메모', '내용', '내용', 'memo', NULL, 1700000000, 1700000000, 1700000100)",
            [],
        ).unwrap();

        conn
    }

    #[test]
    fn test_online_backup_and_restore() {
        let dir = TempTestDir::new();
        let backup_dir = dir.path().join("backups");
        let conn = setup_test_db();

        let result = backup_db_with_options(&conn, &backup_dir, true).unwrap();
        assert!(result.daily_path.exists());
        assert!(result.weekly_path.is_some());
        assert!(result.weekly_path.unwrap().exists());
        assert!(result.bytes_copied > 0);

        // Test restore
        let restore_path = dir.path().join("restored.db");
        let restored_bytes = restore_backup(&result.daily_path, &restore_path).unwrap();
        assert!(restored_bytes > 0);

        let restored_conn = Connection::open(&restore_path).unwrap();
        let count: i64 = restored_conn
            .query_row("SELECT count(*) FROM documents", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 2);
    }

    #[test]
    fn test_rotation_7_daily_4_weekly() {
        let dir = TempTestDir::new();
        let bdir = dir.path();

        // Create 10 fake daily backups
        for i in 0..10 {
            let path = bdir.join(format!("daily_202611{:02}_120000.db", i + 1));
            fs::write(&path, b"snapshot").unwrap();
        }

        // Create 6 fake weekly backups
        for i in 0..6 {
            let path = bdir.join(format!("weekly_202610{:02}_120000.db", i + 1));
            fs::write(&path, b"snapshot").unwrap();
        }

        let rot = rotate_backups(bdir, 7, 4).unwrap();
        assert_eq!(rot.deleted_daily.len(), 3); // 10 - 7 = 3
        assert_eq!(rot.deleted_weekly.len(), 2); // 6 - 4 = 2
        assert_eq!(rot.remaining_daily_count, 7);
        assert_eq!(rot.remaining_weekly_count, 4);
    }

    #[test]
    fn test_export_all_zip() {
        let dir = TempTestDir::new();
        let conn = setup_test_db();
        let export_zip = dir.path().join("export.zip");

        let report = export_all(&conn, &export_zip).unwrap();
        assert_eq!(report.total_exported, 2);
        assert_eq!(report.active_count, 1);
        assert_eq!(report.trash_count, 1);
        assert!(export_zip.exists());
        assert!(report.zip_bytes > 0);

        // Read and verify ZIP content entries
        let file = File::open(&export_zip).unwrap();
        let mut archive = zip::ZipArchive::new(file).unwrap();
        let names: Vec<String> = (0..archive.len())
            .map(|i| {
                let entry = archive.by_index(i).unwrap();
                entry.name().unwrap().to_string()
            })
            .collect();

        assert!(names.contains(&"manifest.json".to_string()));
        assert!(names.contains(&"all_documents.json".to_string()));
        assert!(names.iter().any(|n| n.starts_with("markdown/") && n.ends_with(".md")));
        assert!(names.iter().any(|n| n.starts_with("json/") && n.ends_with(".json")));
        assert!(names.iter().any(|n| n.starts_with("trash/markdown/")));
    }

    #[test]
    fn test_sanitize_filename_traversal_protection() {
        assert_eq!(sanitize_filename("../../etc/passwd", "fallback"), "etc_passwd");
        assert_eq!(sanitize_filename("C:\\Windows\\System32", "fallback"), "C_Windows_System32");
        assert_eq!(sanitize_filename("특이사항: CCP-2B 점검", "fallback"), "특이사항_ CCP-2B 점검");
        assert_eq!(sanitize_filename("...///...", "fallback"), "fallback");
    }
}
