use r2d2::Pool;
use r2d2_sqlite::SqliteConnectionManager;
use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

pub type DbPool = Pool<SqliteConnectionManager>;

pub const SCHEMA_V1: &str = r#"
-- Core documents table
CREATE TABLE IF NOT EXISTS documents (
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

-- Unique index for daily documents (one active daily document per date)
CREATE UNIQUE INDEX IF NOT EXISTS uq_daily 
    ON documents(date) 
    WHERE doc_type = 'daily' AND deleted_at IS NULL;

-- Fast index for listings sorted by update time
CREATE INDEX IF NOT EXISTS idx_documents_type_updated 
    ON documents(doc_type, updated_at DESC);

-- Fast index for soft-delete filtering
CREATE INDEX IF NOT EXISTS idx_documents_deleted_at 
    ON documents(deleted_at);

-- Attachments table for tracking associated assets
CREATE TABLE IF NOT EXISTS attachments (
    id TEXT PRIMARY KEY,
    doc_id TEXT NOT NULL,
    file_name TEXT NOT NULL,
    file_path TEXT NOT NULL,
    file_size INTEGER NOT NULL DEFAULT 0,
    created_at INTEGER NOT NULL,
    FOREIGN KEY(doc_id) REFERENCES documents(id) ON DELETE CASCADE
);

CREATE INDEX IF NOT EXISTS idx_attachments_doc_id 
    ON attachments(doc_id);

-- Key-value settings table
CREATE TABLE IF NOT EXISTS settings (
    key TEXT PRIMARY KEY,
    value TEXT NOT NULL,
    updated_at INTEGER NOT NULL
);

-- FTS5 full-text search table with trigram tokenizer
CREATE VIRTUAL TABLE IF NOT EXISTS documents_fts USING fts5(
    title,
    content_text,
    content='documents',
    content_rowid='rowid',
    tokenize='trigram'
);

-- FTS5 synchronization triggers (Insert, Delete, Update)
CREATE TRIGGER IF NOT EXISTS documents_ai AFTER INSERT ON documents BEGIN
    INSERT INTO documents_fts(rowid, title, content_text) 
    VALUES (new.rowid, new.title, new.content_text);
END;

CREATE TRIGGER IF NOT EXISTS documents_ad AFTER DELETE ON documents BEGIN
    INSERT INTO documents_fts(documents_fts, rowid, title, content_text) 
    VALUES('delete', old.rowid, old.title, old.content_text);
END;

CREATE TRIGGER IF NOT EXISTS documents_au AFTER UPDATE ON documents BEGIN
    INSERT INTO documents_fts(documents_fts, rowid, title, content_text) 
    VALUES('delete', old.rowid, old.title, old.content_text);
    INSERT INTO documents_fts(rowid, title, content_text) 
    VALUES (new.rowid, new.title, new.content_text);
END;
"#;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StorageInfo {
    pub db_path: String,
    pub is_fallback: bool,
    pub is_portable: bool,
}

/// Creates an r2d2 SQLite connection pool with a fixed pool size of 4.
/// Enforces PRAGMA busy_timeout = 5000 and PRAGMA synchronous = NORMAL on every connection.
pub fn create_pool<P: AsRef<Path>>(path: P) -> Result<DbPool, r2d2::Error> {
    let manager = SqliteConnectionManager::file(path)
        .with_init(|conn| {
            conn.execute_batch(
                "PRAGMA busy_timeout = 5000;
                 PRAGMA synchronous = NORMAL;
                 PRAGMA foreign_keys = ON;"
            )
        });

    Pool::builder()
        .max_size(4)
        .build(manager)
}

/// Creates an in-memory r2d2 SQLite connection pool for testing.
pub fn create_in_memory_pool() -> Result<DbPool, r2d2::Error> {
    let manager = SqliteConnectionManager::memory()
        .with_init(|conn| {
            conn.execute_batch(
                "PRAGMA busy_timeout = 5000;
                 PRAGMA synchronous = NORMAL;
                 PRAGMA foreign_keys = ON;"
            )
        });

    Pool::builder()
        .max_size(4)
        .build(manager)
}

/// Checks if a directory is writable by writing and deleting a temporary probe file.
pub fn is_directory_writable(dir: &Path) -> bool {
    if std::fs::create_dir_all(dir).is_err() {
        return false;
    }
    let probe_file = dir.join(format!(".write_test_{}", uuid::Uuid::new_v4()));
    if std::fs::write(&probe_file, b"ok").is_ok() {
        let _ = std::fs::remove_file(&probe_file);
        true
    } else {
        false
    }
}

/// Resolves storage information and database path, supporting portable mode with AppData fallback.
pub fn get_storage_info() -> StorageInfo {
    let portable_data = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|d| d.join("data")));

    let (dir, is_fallback, is_portable) = match portable_data {
        Some(ref d) if is_directory_writable(d) => (d.clone(), false, true),
        _ => (fallback_data_dir(), true, false),
    };

    let db_path = dir.join("sy_notes.db");
    StorageInfo {
        db_path: db_path.to_string_lossy().to_string(),
        is_fallback,
        is_portable,
    }
}

/// Resolves the default SQLite database path.
pub fn get_default_db_path() -> PathBuf {
    PathBuf::from(get_storage_info().db_path)
}

fn fallback_data_dir() -> PathBuf {
    if let Ok(appdata) = std::env::var("APPDATA") {
        let dir = PathBuf::from(appdata).join("sy-notes").join("data");
        let _ = std::fs::create_dir_all(&dir);
        dir
    } else {
        let dir = PathBuf::from("data");
        let _ = std::fs::create_dir_all(&dir);
        dir
    }
}

/// Initializes database PRAGMAs (WAL mode) and executes schema v1 migration if user_version < 1.
pub fn init_database(conn: &Connection) -> Result<(), rusqlite::Error> {
    // 1. Force WAL journal mode (ignored on :memory: DBs by SQLite)
    let _ = conn.execute_batch("PRAGMA journal_mode = WAL;");

    // 2. Inspect user_version
    let user_version: i32 = conn.query_row("PRAGMA user_version;", [], |row| row.get(0))?;

    if user_version < 1 {
        // Execute migration v1 inside a batch
        conn.execute_batch(SCHEMA_V1)?;
        conn.execute_batch("PRAGMA user_version = 1;")?;
    }

    Ok(())
}

/// Helper to initialize database via connection pool
pub fn init_pool(pool: &DbPool) -> Result<(), Box<dyn std::error::Error>> {
    let conn = pool.get()?;
    init_database(&conn)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_pool_connection_and_pragmas() {
        let pool = create_in_memory_pool().expect("Failed to create in-memory pool");
        assert_eq!(pool.max_size(), 4);

        let conn = pool.get().expect("Failed to get connection from pool");

        // Verify busy_timeout
        let timeout: i64 = conn
            .query_row("PRAGMA busy_timeout;", [], |r| r.get(0))
            .expect("query busy_timeout");
        assert_eq!(timeout, 5000);

        // Verify synchronous (1 = NORMAL)
        let sync_mode: i32 = conn
            .query_row("PRAGMA synchronous;", [], |r| r.get(0))
            .expect("query synchronous");
        assert_eq!(sync_mode, 1);
    }

    #[test]
    fn test_init_database_and_migration_v1() {
        let conn = Connection::open_in_memory().expect("Failed to open in-memory connection");

        // Check initial version is 0
        let v0: i32 = conn
            .query_row("PRAGMA user_version;", [], |r| r.get(0))
            .expect("query v0");
        assert_eq!(v0, 0);

        // Run migration
        init_database(&conn).expect("init_database failed");

        // Check new version is 1
        let v1: i32 = conn
            .query_row("PRAGMA user_version;", [], |r| r.get(0))
            .expect("query v1");
        assert_eq!(v1, 1);

        // Idempotency: re-running should succeed without error
        init_database(&conn).expect("re-running init_database failed");
    }

    #[test]
    fn test_fts5_trigram_triggers() {
        let conn = Connection::open_in_memory().expect("open memory conn");
        init_database(&conn).expect("init db");

        // 1. Insert into documents -> triggers documents_ai -> inserts into documents_fts
        conn.execute(
            "INSERT INTO documents (id, title, content, content_text, doc_type, date, created_at, updated_at) 
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            (
                "doc-1",
                "식품안전 검교정 일지",
                "{\"ops\":[]}",
                "삼영 스마트팩토리 검교정 온도계 점검 기록",
                "daily",
                "2026-11-22",
                1700000000000i64,
                1700000000000i64,
            ),
        )
        .expect("insert doc");

        // Search with 3-char Korean trigram "검교정"
        let count: i64 = conn
            .query_row(
                "SELECT count(*) FROM documents_fts WHERE documents_fts MATCH '검교정'",
                [],
                |r| r.get(0),
            )
            .expect("match query");
        assert_eq!(count, 1);

        // 2. Update -> triggers documents_au
        conn.execute(
            "UPDATE documents SET content_text = '금속검출기 감도 테스트 기록' WHERE id = 'doc-1'",
            [],
        )
        .expect("update doc");

        // Search with new content "금속검"
        let count_new: i64 = conn
            .query_row(
                "SELECT count(*) FROM documents_fts WHERE documents_fts MATCH '금속검'",
                [],
                |r| r.get(0),
            )
            .expect("match new");
        assert_eq!(count_new, 1);

        // Old content should no longer match
        let count_old: i64 = conn
            .query_row(
                "SELECT count(*) FROM documents_fts WHERE documents_fts MATCH '온도계'",
                [],
                |r| r.get(0),
            )
            .expect("match old");
        assert_eq!(count_old, 0);

        // 3. Delete -> triggers documents_ad
        conn.execute("DELETE FROM documents WHERE id = 'doc-1'", [])
            .expect("delete doc");

        let count_deleted: i64 = conn
            .query_row(
                "SELECT count(*) FROM documents_fts WHERE documents_fts MATCH '금속검'",
                [],
                |r| r.get(0),
            )
            .expect("match after delete");
        assert_eq!(count_deleted, 0);
    }

    #[test]
    fn test_uq_daily_constraint() {
        let conn = Connection::open_in_memory().expect("open memory conn");
        init_database(&conn).expect("init db");

        conn.execute(
            "INSERT INTO documents (id, title, content, content_text, doc_type, date, created_at, updated_at) 
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            (
                "daily-1",
                "일지 2026-11-22",
                "{}",
                "",
                "daily",
                "2026-11-22",
                1000i64,
                1000i64,
            ),
        )
        .expect("insert first daily");

        // Inserting another active daily note with the same date must fail due to uq_daily
        let duplicate_res = conn.execute(
            "INSERT INTO documents (id, title, content, content_text, doc_type, date, created_at, updated_at) 
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            (
                "daily-2",
                "일지 2026-11-22 중복",
                "{}",
                "",
                "daily",
                "2026-11-22",
                2000i64,
                2000i64,
            ),
        );
        assert!(duplicate_res.is_err(), "Duplicate active daily document on the same date should fail");

        // Regular memo with same date or NULL date should succeed
        let memo_res = conn.execute(
            "INSERT INTO documents (id, title, content, content_text, doc_type, date, created_at, updated_at) 
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            (
                "memo-1",
                "일반 메모",
                "{}",
                "",
                "memo",
                "2026-11-22",
                3000i64,
                3000i64,
            ),
        );
        assert!(memo_res.is_ok(), "Non-daily documents should not conflict on date");
    }

    #[test]
    fn test_disk_database_wal_mode() {
        let temp_dir = std::env::temp_dir().join(format!("sy_notes_test_{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&temp_dir).unwrap();
        let db_path = temp_dir.join("test.db");

        let pool = create_pool(&db_path).expect("create file pool");
        init_pool(&pool).expect("init pool");

        let conn = pool.get().expect("get conn");
        let journal_mode: String = conn
            .query_row("PRAGMA journal_mode;", [], |r| r.get(0))
            .expect("query journal_mode");
        assert_eq!(journal_mode.to_lowercase(), "wal");

        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn test_storage_info() {
        let info = get_storage_info();
        assert!(!info.db_path.is_empty());
    }
}
