use std::collections::HashSet;
use std::path::Path;
use std::time::{Duration, SystemTime};
use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64;

/// Saves a base64-encoded attachment into `<base_data_dir>/attachments/<uuid>.<ext>`.
/// Enforces directory traversal protection and UUID filename generation.
pub fn save_attachment_file(
    base_data_dir: &Path,
    file_name: &str,
    data_base64: &str,
) -> Result<String, String> {
    let attachments_dir = base_data_dir.join("attachments");
    if let Err(e) = std::fs::create_dir_all(&attachments_dir) {
        return Err(format!("Failed to create attachments dir: {}", e));
    }

    // Determine extension safely, strictly allowing image & document types
    let ext = Path::new(file_name)
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_lowercase())
        .unwrap_or_else(|| "png".to_string());

    let safe_ext = match ext.as_str() {
        "png" | "jpg" | "jpeg" | "gif" | "webp" | "svg" | "pdf" | "txt" => ext,
        _ => "png".to_string(),
    };

    let new_filename = format!("{}.{}", uuid::Uuid::new_v4(), safe_ext);
    let target_path = attachments_dir.join(&new_filename);

    // Verify canonical path does not escape attachments_dir
    // Strip data URL prefix if present (e.g. data:image/png;base64,...)
    let clean_base64 = if let Some(idx) = data_base64.find(";base64,") {
        &data_base64[idx + 8..]
    } else if let Some(stripped) = data_base64.strip_prefix("data:") {
        stripped
    } else {
        data_base64
    };

    let bytes = BASE64
        .decode(clean_base64.trim())
        .map_err(|e| format!("Invalid base64 attachment data: {}", e))?;

    std::fs::write(&target_path, bytes)
        .map_err(|e| format!("Failed to write attachment file: {}", e))?;

    // Return the relative path: attachments/<uuid>.<ext>
    Ok(format!("attachments/{}", new_filename))
}

/// Files younger than this are never treated as orphans: an image may be uploaded
/// before the document that references it has been autosaved.
pub const ORPHAN_GRACE: Duration = Duration::from_secs(60 * 60);

/// Result of [`cleanup_orphan_attachments`].
#[derive(Debug, Default, serde::Serialize)]
pub struct OrphanCleanup {
    /// Relative paths (`attachments/<file>`) that were deleted.
    pub removed: Vec<String>,
    /// Number of files still referenced by a document (live or in the trash).
    pub kept: usize,
}

/// Collects every `attachments/<file>` name referenced from document content.
/// Documents in the trash count too, so restoring one never loses its images.
fn referenced_attachment_names(conn: &rusqlite::Connection) -> rusqlite::Result<HashSet<String>> {
    const MARKER: &str = "attachments/";
    let mut names = HashSet::new();
    let mut stmt = conn.prepare("SELECT content FROM documents WHERE content LIKE '%attachments/%'")?;
    let rows = stmt.query_map([], |row| row.get::<_, String>(0))?;
    for content in rows {
        let content = content?;
        let mut rest = content.as_str();
        while let Some(pos) = rest.find(MARKER) {
            rest = &rest[pos + MARKER.len()..];
            let end = rest
                .find(|c: char| !(c.is_ascii_alphanumeric() || c == '-' || c == '.' || c == '_'))
                .unwrap_or(rest.len());
            if end > 0 {
                names.insert(rest[..end].to_string());
            }
            rest = &rest[end..];
        }
    }
    Ok(names)
}

/// Deletes files in `<base_data_dir>/attachments/` that no document references
/// and that are older than `min_age`. Only plain files directly in that folder
/// are considered; symlinks and subdirectories are left alone.
pub fn cleanup_orphan_attachments(
    conn: &rusqlite::Connection,
    base_data_dir: &Path,
    min_age: Duration,
) -> Result<OrphanCleanup, String> {
    let dir = base_data_dir.join("attachments");
    let mut report = OrphanCleanup::default();
    let entries = match std::fs::read_dir(&dir) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(report),
        Err(e) => return Err(format!("Failed to read attachments dir: {e}")),
    };
    let referenced = referenced_attachment_names(conn).map_err(|e| e.to_string())?;
    let now = SystemTime::now();

    for entry in entries.flatten() {
        let Ok(meta) = entry.metadata() else { continue };
        let Ok(file_type) = entry.file_type() else { continue };
        if !file_type.is_file() {
            continue;
        }
        let name = entry.file_name().to_string_lossy().into_owned();
        if referenced.contains(&name) {
            report.kept += 1;
            continue;
        }
        let age = meta
            .modified()
            .ok()
            .and_then(|m| now.duration_since(m).ok())
            .unwrap_or(Duration::ZERO);
        if age < min_age {
            continue;
        }
        if std::fs::remove_file(entry.path()).is_ok() {
            report.removed.push(format!("attachments/{name}"));
        }
    }
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_save_attachment_file_and_traversal_prevention() {
        let temp_dir = std::env::temp_dir().join(format!("sy_notes_attach_{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&temp_dir).unwrap();

        // 1. Normal save with data URL prefix
        let raw = b"hello image bytes";
        let b64 = format!("data:image/png;base64,{}", BASE64.encode(raw));
        let rel_path = save_attachment_file(&temp_dir, "my_photo.png", &b64).expect("save success");

        assert!(rel_path.starts_with("attachments/"));
        assert!(rel_path.ends_with(".png"));

        let full_path = temp_dir.join(&rel_path);
        assert!(full_path.exists());
        let read_bytes = std::fs::read(&full_path).unwrap();
        assert_eq!(read_bytes, raw);

        // 2. Directory traversal attempt in file_name (e.g. "../../../etc/passwd")
        let evil_name = "../../../evil.exe";
        let evil_res = save_attachment_file(&temp_dir, evil_name, &b64).expect("save safely sanitized");
        assert!(evil_res.starts_with("attachments/"));
        assert!(evil_res.ends_with(".png"), "evil extension should default to safe ext");

        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    fn docs_db(contents: &[(&str, &str, bool)]) -> rusqlite::Connection {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        crate::db::init_database(&conn).unwrap();
        for (id, content, trashed) in contents {
            conn.execute(
                "INSERT INTO documents (id, title, content, content_text, doc_type, created_at, updated_at, deleted_at)
                 VALUES (?1, ?1, ?2, '', 'memo', 1, 1, ?3)",
                rusqlite::params![id, content, if *trashed { Some(1i64) } else { None }],
            )
            .unwrap();
        }
        conn
    }

    fn write_old(path: &Path) {
        std::fs::write(path, b"x").unwrap();
        let old = SystemTime::now() - Duration::from_secs(2 * 60 * 60);
        std::fs::File::options().write(true).open(path).unwrap().set_modified(old).unwrap();
    }

    #[test]
    fn test_cleanup_orphan_attachments() {
        let base = std::env::temp_dir().join(format!("sy_notes_orphan_{}", uuid::Uuid::new_v4()));
        let dir = base.join("attachments");
        std::fs::create_dir_all(dir.join("subdir")).unwrap();
        for name in ["live.png", "trashed.png", "orphan.png"] {
            write_old(&dir.join(name));
        }
        std::fs::write(dir.join("fresh.png"), b"x").unwrap(); // just uploaded, not saved yet

        let conn = docs_db(&[
            ("a", r#"[{"type":"image","props":{"url":"attachments/live.png"}}]"#, false),
            ("b", r#"[{"type":"image","props":{"url":"attachments/trashed.png"}}]"#, true),
        ]);

        let report = cleanup_orphan_attachments(&conn, &base, ORPHAN_GRACE).unwrap();
        assert_eq!(report.removed, vec!["attachments/orphan.png".to_string()]);
        assert_eq!(report.kept, 2);
        assert!(dir.join("live.png").exists());
        assert!(dir.join("trashed.png").exists(), "trash can still be restored");
        assert!(dir.join("fresh.png").exists(), "grace period protects new uploads");
        assert!(dir.join("subdir").exists());

        // Once the trashed doc is purged, its image becomes an orphan.
        conn.execute("DELETE FROM documents WHERE id = 'b'", []).unwrap();
        let report = cleanup_orphan_attachments(&conn, &base, ORPHAN_GRACE).unwrap();
        assert_eq!(report.removed, vec!["attachments/trashed.png".to_string()]);

        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn test_cleanup_orphan_attachments_missing_dir() {
        let base = std::env::temp_dir().join(format!("sy_notes_orphan_{}", uuid::Uuid::new_v4()));
        let conn = docs_db(&[]);
        let report = cleanup_orphan_attachments(&conn, &base, ORPHAN_GRACE).unwrap();
        assert!(report.removed.is_empty());
    }
}
