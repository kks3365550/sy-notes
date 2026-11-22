use chrono::Utc;
use rusqlite::{params, Connection, OptionalExtension, TransactionBehavior};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Representation of a document in sy-notes.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Document {
    pub id: String,
    pub title: String,
    #[serde(alias = "content_json")]
    pub content: String,
    pub content_text: String,
    #[serde(default = "default_doc_type")]
    pub doc_type: String,
    pub date: Option<String>,
    pub created_at: i64,
    pub updated_at: i64,
    pub deleted_at: Option<i64>,
}

fn default_doc_type() -> String {
    "memo".to_string()
}

/// Errors occurring within document operations.
#[derive(Debug)]
pub enum DocError {
    Sqlite(rusqlite::Error),
    OptimisticLockConflict {
        id: String,
        expected: i64,
        actual: Option<i64>,
    },
    NotFound(String),
    InvalidInput(String),
}

impl std::fmt::Display for DocError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DocError::Sqlite(err) => write!(f, "Database error: {err}"),
            DocError::OptimisticLockConflict { id, expected, actual } => write!(
                f,
                "Optimistic lock conflict for document '{id}': expected updated_at={expected}, actual={actual:?}"
            ),
            DocError::NotFound(id) => write!(f, "Document not found: {id}"),
            DocError::InvalidInput(msg) => write!(f, "Invalid input: {msg}"),
        }
    }
}

impl std::error::Error for DocError {}

impl From<rusqlite::Error> for DocError {
    fn from(err: rusqlite::Error) -> Self {
        DocError::Sqlite(err)
    }
}

/// Returns current Unix epoch timestamp in milliseconds.
pub fn unix_now() -> i64 {
    Utc::now().timestamp_millis()
}

/// Default JSON content template for daily logs with BlockNote structure.
fn default_daily_content(_date: &str) -> String {
    format!(
        r#"[
  {{
    "id": "daily-goal-h",
    "type": "heading",
    "props": {{ "level": 2 }},
    "content": [{{ "type": "text", "text": "오늘 목표", "styles": {{}} }}],
    "children": []
  }},
  {{
    "id": "daily-goal-1",
    "type": "checkListItem",
    "props": {{ "checked": false }},
    "content": [{{ "type": "text", "text": "오늘의 주요 목표 수립", "styles": {{}} }}],
    "children": []
  }},
  {{
    "id": "daily-work-h",
    "type": "heading",
    "props": {{ "level": 2 }},
    "content": [{{ "type": "text", "text": "진행 업무", "styles": {{}} }}],
    "children": []
  }},
  {{
    "id": "daily-work-1",
    "type": "checkListItem",
    "props": {{ "checked": false }},
    "content": [{{ "type": "text", "text": "진행 업무 내용 기록", "styles": {{}} }}],
    "children": []
  }},
  {{
    "id": "daily-notes-h",
    "type": "heading",
    "props": {{ "level": 2 }},
    "content": [{{ "type": "text", "text": "특이사항", "styles": {{}} }}],
    "children": []
  }},
  {{
    "id": "daily-notes-1",
    "type": "paragraph",
    "props": {{}},
    "content": [{{ "type": "text", "text": "", "styles": {{}} }}],
    "children": []
  }}
]"#
    )
}

/// Default plain text template for search indexing and fallback display.
fn default_daily_content_text(date: &str) -> String {
    format!(
        "{date} 업무일지\n\n[오늘 목표]\n- 오늘의 주요 목표 수립\n\n[진행 업무]\n- 진행 업무 내용 기록\n\n[특이사항]\n- "
    )
}

/// Atomically gets or creates a daily log document for the given `date` ("YYYY-MM-DD").
/// Executes inside an `IMMEDIATE` transaction to prevent race conditions and duplicate creation (`uq_daily`).
/// If an existing active daily log exists, returns it.
/// If an existing daily log was soft-deleted, restores it (`deleted_at = NULL`).
/// If none exists, creates and inserts a default daily log template atomically.
pub fn get_or_create_daily(conn: &mut Connection, date_str: &str) -> Result<Document, DocError> {
    let resolved_date = if date_str.trim().is_empty() {
        Utc::now().format("%Y-%m-%d").to_string()
    } else {
        date_str.trim().to_string()
    };

    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;

    // Check if daily doc for the date exists (including soft-deleted)
    let existing: Option<Document> = tx
        .query_row(
            "SELECT id, title, content, content_text, doc_type, date, created_at, updated_at, deleted_at
             FROM documents
             WHERE doc_type = 'daily' AND date = ?1
             ORDER BY deleted_at ASC
             LIMIT 1",
            params![resolved_date],
            |row| {
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
            },
        )
        .optional()?;

    if let Some(mut doc) = existing {
        if doc.deleted_at.is_some() {
            // Restore soft-deleted daily log
            let now = unix_now();
            tx.execute(
                "UPDATE documents SET deleted_at = NULL, updated_at = ?1 WHERE id = ?2",
                params![now, doc.id],
            )?;
            doc.deleted_at = None;
            doc.updated_at = now;
        }
        tx.commit()?;
        return Ok(doc);
    }

    // Create new daily document
    let now = unix_now();
    let new_id = Uuid::new_v4().to_string();
    let title = format!("{resolved_date} 업무일지");
    let content = default_daily_content(&resolved_date);
    let content_text = default_daily_content_text(&resolved_date);

    let new_doc = Document {
        id: new_id,
        title,
        content,
        content_text,
        doc_type: "daily".to_string(),
        date: Some(resolved_date.clone()),
        created_at: now,
        updated_at: now,
        deleted_at: None,
    };

    match tx.execute(
        "INSERT INTO documents (id, title, content, content_text, doc_type, date, created_at, updated_at, deleted_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
        params![
            new_doc.id,
            new_doc.title,
            new_doc.content,
            new_doc.content_text,
            new_doc.doc_type,
            new_doc.date,
            new_doc.created_at,
            new_doc.updated_at,
            new_doc.deleted_at,
        ],
    ) {
        Ok(_) => {
            tx.commit()?;
            Ok(new_doc)
        }
        Err(rusqlite::Error::SqliteFailure(ref err, _))
            if err.code == rusqlite::ffi::ErrorCode::ConstraintViolation =>
        {
            // Concurrent race fallback: read existing row committed concurrently
            drop(tx);
            let fallback_doc = conn.query_row(
                "SELECT id, title, content, content_text, doc_type, date, created_at, updated_at, deleted_at
                 FROM documents
                 WHERE doc_type = 'daily' AND date = ?1
                 LIMIT 1",
                params![resolved_date],
                |row| {
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
                },
            )?;
            Ok(fallback_doc)
        }
        Err(e) => Err(DocError::Sqlite(e)),
    }
}

/// Creates or updates a document using `ON CONFLICT(id) DO UPDATE`.
/// Preserves `created_at` on updates.
/// Checks optimistic lock against `doc.updated_at` (if > 0): if the database contains a newer or conflicting
/// `updated_at`, returns `DocError::OptimisticLockConflict`.
pub fn create_or_update_doc(conn: &Connection, doc: &Document) -> Result<Document, DocError> {
    let now = unix_now();
    let id = if doc.id.trim().is_empty() {
        Uuid::new_v4().to_string()
    } else {
        doc.id.trim().to_string()
    };

    let insert_created_at = if doc.created_at > 0 {
        doc.created_at
    } else {
        now
    };

    let expected_updated_at = doc.updated_at;

    // ON CONFLICT(id) DO UPDATE preserves `created_at` and checks optimistic lock on `updated_at`.
    // If expected_updated_at <= 0, optimistic check is bypassed (for initial creation or forced overwrite).
    let changes = conn.execute(
        "INSERT INTO documents (id, title, content, content_text, doc_type, date, created_at, updated_at, deleted_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
         ON CONFLICT(id) DO UPDATE SET
             title = excluded.title,
             content = excluded.content,
             content_text = excluded.content_text,
             doc_type = excluded.doc_type,
             date = excluded.date,
             updated_at = excluded.updated_at,
             deleted_at = excluded.deleted_at
         WHERE ?10 <= 0 OR documents.updated_at = ?10",
        params![
            id,
            doc.title,
            doc.content,
            doc.content_text,
            doc.doc_type,
            doc.date,
            insert_created_at,
            now,
            doc.deleted_at,
            expected_updated_at,
        ],
    )?;

    if changes == 0 {
        // Update was skipped because documents.updated_at != expected_updated_at
        let actual_updated: Option<i64> = conn
            .query_row(
                "SELECT updated_at FROM documents WHERE id = ?1",
                params![id],
                |row| row.get(0),
            )
            .optional()?;

        return match actual_updated {
            Some(actual) => Err(DocError::OptimisticLockConflict {
                id,
                expected: expected_updated_at,
                actual: Some(actual),
            }),
            None => Err(DocError::NotFound(id)),
        };
    }

    // Fetch the actual record as committed
    let saved = conn.query_row(
        "SELECT id, title, content, content_text, doc_type, date, created_at, updated_at, deleted_at
         FROM documents
         WHERE id = ?1",
        params![id],
        |row| {
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
        },
    )?;

    Ok(saved)
}

/// Performs a soft delete: sets `deleted_at = unix_now()` and updates `updated_at`.
pub fn delete_doc(conn: &Connection, id: &str) -> Result<(), DocError> {
    let now = unix_now();
    let rows_affected = conn.execute(
        "UPDATE documents SET deleted_at = ?1, updated_at = ?1 WHERE id = ?2 AND deleted_at IS NULL",
        params![now, id],
    )?;

    if rows_affected == 0 {
        let exists: bool = conn
            .query_row(
                "SELECT 1 FROM documents WHERE id = ?1",
                params![id],
                |_| Ok(true),
            )
            .optional()?
            .unwrap_or(false);

        if !exists {
            return Err(DocError::NotFound(id.to_string()));
        }
    }

    Ok(())
}

/// Restores a soft-deleted document: sets `deleted_at = NULL` and updates `updated_at`.
pub fn restore_doc(conn: &Connection, id: &str) -> Result<(), DocError> {
    let now = unix_now();
    let rows_affected = conn.execute(
        "UPDATE documents SET deleted_at = NULL, updated_at = ?1 WHERE id = ?2 AND deleted_at IS NOT NULL",
        params![now, id],
    )?;

    if rows_affected == 0 {
        let exists: bool = conn
            .query_row(
                "SELECT 1 FROM documents WHERE id = ?1",
                params![id],
                |_| Ok(true),
            )
            .optional()?
            .unwrap_or(false);

        if !exists {
            return Err(DocError::NotFound(id.to_string()));
        }
    }

    Ok(())
}

/// Purges soft-deleted documents older than `older_than_days`.
/// If `older_than_days <= 0`, purges all soft-deleted documents immediately.
/// Cleans up orphaned attachment files on disk if the attachments table exists.
/// Triggers SQLite FTS deletion (`documents_ad`) via trigger on documents.
/// Returns the number of permanently purged documents.
pub fn purge_trash(conn: &Connection, older_than_days: i64) -> Result<usize, DocError> {
    let cutoff = if older_than_days <= 0 {
        unix_now() + 1000 // all soft-deleted documents
    } else {
        unix_now() - (older_than_days * 86_400 * 1000)
    };

    // Clean up physical files for attachments associated with documents being purged
    let has_attachments: bool = conn
        .query_row(
            "SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = 'attachments'",
            [],
            |_| Ok(true),
        )
        .optional()?
        .unwrap_or(false);

    if has_attachments {
        let mut stmt = conn.prepare(
            "SELECT file_path FROM attachments WHERE doc_id IN (
                SELECT id FROM documents WHERE deleted_at IS NOT NULL AND deleted_at <= ?1
            )",
        )?;
        let paths: Vec<String> = stmt
            .query_map(params![cutoff], |row| row.get(0))?
            .filter_map(Result::ok)
            .collect();

        for path in paths {
            let _ = std::fs::remove_file(path);
        }
    }

    // Permanently remove documents
    let count = conn.execute(
        "DELETE FROM documents WHERE deleted_at IS NOT NULL AND deleted_at <= ?1",
        params![cutoff],
    )?;

    Ok(count)
}

/// Retrieves a single document by its id (including deleted if applicable).
pub fn get_doc(conn: &Connection, id: &str) -> Result<Option<Document>, DocError> {
    let doc = conn
        .query_row(
            "SELECT id, title, content, content_text, doc_type, date, created_at, updated_at, deleted_at
             FROM documents WHERE id = ?1",
            params![id],
            |row| {
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
            },
        )
        .optional()?;

    Ok(doc)
}

/// Lists documents ordered by `updated_at DESC`.
/// If `include_deleted` is false, filters out soft-deleted items (`deleted_at IS NULL`).
pub fn list_docs(conn: &Connection, include_deleted: bool) -> Result<Vec<Document>, DocError> {
    let sql = if include_deleted {
        "SELECT id, title, content, content_text, doc_type, date, created_at, updated_at, deleted_at
         FROM documents ORDER BY updated_at DESC"
    } else {
        "SELECT id, title, content, content_text, doc_type, date, created_at, updated_at, deleted_at
         FROM documents WHERE deleted_at IS NULL ORDER BY updated_at DESC"
    };

    let mut stmt = conn.prepare(sql)?;
    let docs = stmt
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
        .filter_map(Result::ok)
        .collect();

    Ok(docs)
}

/// Lists recent active documents up to `limit`.
pub fn list_recent_docs(conn: &Connection, limit: i64) -> Result<Vec<Document>, DocError> {
    let mut stmt = conn.prepare(
        "SELECT id, title, content, content_text, doc_type, date, created_at, updated_at, deleted_at
         FROM documents WHERE deleted_at IS NULL ORDER BY updated_at DESC LIMIT ?1",
    )?;

    let docs = stmt
        .query_map(params![limit], |row| {
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
        .filter_map(Result::ok)
        .collect();

    Ok(docs)
}

/// Lists all soft-deleted documents (Trash view).
pub fn list_trash(conn: &Connection) -> Result<Vec<Document>, DocError> {
    let mut stmt = conn.prepare(
        "SELECT id, title, content, content_text, doc_type, date, created_at, updated_at, deleted_at
         FROM documents WHERE deleted_at IS NOT NULL ORDER BY updated_at DESC",
    )?;

    let docs = stmt
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
        .filter_map(Result::ok)
        .collect();

    Ok(docs)
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::Connection;

    fn setup_test_db() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "PRAGMA busy_timeout = 5000;
             PRAGMA foreign_keys = ON;
             CREATE TABLE documents (
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
             CREATE UNIQUE INDEX uq_daily ON documents(date) WHERE doc_type = 'daily' AND deleted_at IS NULL;
             CREATE TABLE attachments (
                 id TEXT PRIMARY KEY,
                 doc_id TEXT NOT NULL,
                 file_name TEXT NOT NULL,
                 file_path TEXT NOT NULL,
                 file_size INTEGER NOT NULL DEFAULT 0,
                 created_at INTEGER NOT NULL,
                 FOREIGN KEY(doc_id) REFERENCES documents(id) ON DELETE CASCADE
             );
             CREATE VIRTUAL TABLE documents_fts USING fts5(
                 title,
                 content_text,
                 content='documents',
                 content_rowid='rowid',
                 tokenize='trigram'
             );
             CREATE TRIGGER documents_ai AFTER INSERT ON documents BEGIN
                 INSERT INTO documents_fts(rowid, title, content_text) VALUES (new.rowid, new.title, new.content_text);
             END;
             CREATE TRIGGER documents_ad AFTER DELETE ON documents BEGIN
                 INSERT INTO documents_fts(documents_fts, rowid, title, content_text) VALUES('delete', old.rowid, old.title, old.content_text);
             END;
             CREATE TRIGGER documents_au AFTER UPDATE ON documents BEGIN
                 INSERT INTO documents_fts(documents_fts, rowid, title, content_text) VALUES('delete', old.rowid, old.title, old.content_text);
                 INSERT INTO documents_fts(rowid, title, content_text) VALUES (new.rowid, new.title, new.content_text);
             END;"
        ).unwrap();
        conn
    }

    #[test]
    fn test_get_or_create_daily_idempotency_and_template() {
        let mut conn = setup_test_db();
        let date = "2026-11-22";

        let doc1 = get_or_create_daily(&mut conn, date).unwrap();
        assert_eq!(doc1.doc_type, "daily");
        assert_eq!(doc1.date, Some(date.to_string()));
        assert!(doc1.title.contains(date));
        assert!(doc1.content.contains("오늘 목표"));
        assert!(doc1.content_text.contains("진행 업무"));
        assert!(doc1.deleted_at.is_none());

        // Call again for same date -> must return existing record
        let doc2 = get_or_create_daily(&mut conn, date).unwrap();
        assert_eq!(doc1.id, doc2.id);
        assert_eq!(doc1.created_at, doc2.created_at);

        // Different date -> creates distinct record
        let doc_next = get_or_create_daily(&mut conn, "2026-11-23").unwrap();
        assert_ne!(doc1.id, doc_next.id);
        assert_eq!(doc_next.date, Some("2026-11-23".to_string()));
    }

    #[test]
    fn test_create_or_update_doc_preserves_created_at() {
        let conn = setup_test_db();

        let initial_doc = Document {
            id: "doc-test-1".to_string(),
            title: "제목 1".to_string(),
            content: "내용 1".to_string(),
            content_text: "내용 텍스트 1".to_string(),
            doc_type: "memo".to_string(),
            date: None,
            created_at: 0,
            updated_at: 0,
            deleted_at: None,
        };

        let created = create_or_update_doc(&conn, &initial_doc).unwrap();
        assert_eq!(created.id, "doc-test-1");
        assert!(created.created_at > 0);
        assert_eq!(created.created_at, created.updated_at);

        // Wait a tiny moment or ensure timestamp differs
        std::thread::sleep(std::time::Duration::from_millis(5));

        let mut update_doc = created.clone();
        update_doc.title = "제목 1 수정됨".to_string();
        update_doc.content = "내용 1 수정됨".to_string();

        let updated = create_or_update_doc(&conn, &update_doc).unwrap();
        assert_eq!(updated.id, created.id);
        assert_eq!(updated.created_at, created.created_at); // created_at MUST BE PRESERVED
        assert!(updated.updated_at >= created.updated_at);
        assert_eq!(updated.title, "제목 1 수정됨");
    }

    #[test]
    fn test_optimistic_lock_conflict() {
        let conn = setup_test_db();

        let doc = Document {
            id: "doc-lock-test".to_string(),
            title: "낙관적 락 테스트".to_string(),
            content: "초기 내용".to_string(),
            content_text: "초기 텍스트".to_string(),
            doc_type: "memo".to_string(),
            date: None,
            created_at: 0,
            updated_at: 0,
            deleted_at: None,
        };

        let created = create_or_update_doc(&conn, &doc).unwrap();

        // User A modifies document
        std::thread::sleep(std::time::Duration::from_millis(5));
        let mut user_a_doc = created.clone();
        user_a_doc.title = "User A 수정".to_string();
        let after_a = create_or_update_doc(&conn, &user_a_doc).unwrap();
        assert_eq!(after_a.title, "User A 수정");

        // User B tries to update using stale version (created.updated_at)
        let mut user_b_doc = created.clone();
        user_b_doc.title = "User B 충돌 시도".to_string();

        let result = create_or_update_doc(&conn, &user_b_doc);
        match result {
            Err(DocError::OptimisticLockConflict { id, expected, actual }) => {
                assert_eq!(id, "doc-lock-test");
                assert_eq!(expected, created.updated_at);
                assert_eq!(actual, Some(after_a.updated_at));
            }
            other => panic!("Expected OptimisticLockConflict, got {other:?}"),
        }
    }

    #[test]
    fn test_delete_and_restore_doc() {
        let conn = setup_test_db();

        let doc = Document {
            id: "doc-del-test".to_string(),
            title: "삭제 테스트".to_string(),
            content: "내용".to_string(),
            content_text: "텍스트".to_string(),
            doc_type: "memo".to_string(),
            date: None,
            created_at: 0,
            updated_at: 0,
            deleted_at: None,
        };
        create_or_update_doc(&conn, &doc).unwrap();

        // Active list includes it
        let active_docs = list_docs(&conn, false).unwrap();
        assert_eq!(active_docs.len(), 1);

        // Soft delete
        delete_doc(&conn, "doc-del-test").unwrap();

        let active_docs_after = list_docs(&conn, false).unwrap();
        assert_eq!(active_docs_after.len(), 0);

        let trash = list_trash(&conn).unwrap();
        assert_eq!(trash.len(), 1);
        assert!(trash[0].deleted_at.is_some());

        // Restore
        restore_doc(&conn, "doc-del-test").unwrap();

        let restored_active = list_docs(&conn, false).unwrap();
        assert_eq!(restored_active.len(), 1);
        assert!(restored_active[0].deleted_at.is_none());
    }

    #[test]
    fn test_purge_trash() {
        let conn = setup_test_db();

        let doc1 = Document {
            id: "doc-keep".to_string(),
            title: "보관 문서".to_string(),
            content: "".to_string(),
            content_text: "".to_string(),
            doc_type: "memo".to_string(),
            date: None,
            created_at: 0,
            updated_at: 0,
            deleted_at: None,
        };
        let doc2 = Document {
            id: "doc-purge".to_string(),
            title: "삭제 대상 문서".to_string(),
            content: "".to_string(),
            content_text: "".to_string(),
            doc_type: "memo".to_string(),
            date: None,
            created_at: 0,
            updated_at: 0,
            deleted_at: None,
        };

        create_or_update_doc(&conn, &doc1).unwrap();
        create_or_update_doc(&conn, &doc2).unwrap();

        delete_doc(&conn, "doc-purge").unwrap();

        // Purge with older_than_days = 0 (immediately purge all trash)
        let purged_count = purge_trash(&conn, 0).unwrap();
        assert_eq!(purged_count, 1);

        // Active doc still exists
        let active = get_doc(&conn, "doc-keep").unwrap();
        assert!(active.is_some());

        // Purged doc is gone
        let purged = get_doc(&conn, "doc-purge").unwrap();
        assert!(purged.is_none());
    }
}
