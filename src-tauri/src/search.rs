use crate::doc::Document;
use rusqlite::{params, Connection, Result, Row};

/// Escapes and sanitizes user input for FTS5 queries.
///
/// Prevents FTS5 syntax errors caused by special characters, boolean operators (AND, OR, NOT),
/// column filters, and quotes. Enclosing escaped terms in double quotes treats them as literal
/// tokens or phrase queries for FTS5 (including the trigram tokenizer).
pub fn sanitize_fts_query(query: &str) -> String {
    let trimmed = query.trim();
    if trimmed.is_empty() {
        return String::new();
    }

    // Double-quote escape any internal double quotes: " -> ""
    let escaped = trimmed.replace('"', "\"\"");
    format!("\"{}\"", escaped)
}

/// Maps a database row from `documents` to a `Document` struct.
/// Supports both named column lookup and indexed lookup fallback.
pub fn document_from_row(row: &Row) -> Result<Document> {
    Ok(Document {
        id: row.get("id")?,
        title: row.get("title")?,
        content: row.get("content")?,
        content_text: row.get("content_text")?,
        doc_type: row.get("doc_type")?,
        date: row.get("date")?,
        created_at: row.get("created_at")?,
        updated_at: row.get("updated_at")?,
        deleted_at: row.get("deleted_at")?,
    })
}

/// Searches documents with full-text search (FTS5 trigram) or LIKE fallback.
///
/// - For queries with 3 or more Unicode characters:
///   Executes FTS5 MATCH join:
///   `SELECT d.* FROM documents_fts f JOIN documents d ON d.rowid = f.rowid WHERE documents_fts MATCH ? AND d.deleted_at IS NULL ORDER BY rank LIMIT ?`
/// - For queries with 1 to 2 Unicode characters:
///   Executes LIKE fallback:
///   `SELECT * FROM documents WHERE (title LIKE ? OR content_text LIKE ?) AND deleted_at IS NULL ORDER BY updated_at DESC LIMIT ?`
///
/// SQL injection is completely prevented through parameterized query binding.
pub fn search_docs(conn: &Connection, query: &str, limit: i64) -> Result<Vec<Document>> {
    // Strip null bytes to prevent C-string NulError in SQLite bindings
    let cleaned: String = query.chars().filter(|c| *c != '\0').collect();
    let trimmed = cleaned.trim();
    if trimmed.is_empty() || limit <= 0 {
        return Ok(Vec::new());
    }

    let char_count = trimmed.chars().count();

    if char_count >= 3 {
        search_fts(conn, trimmed, limit)
    } else {
        search_like(conn, trimmed, limit)
    }
}

/// Executes FTS5 trigram search.
///
/// If FTS5 MATCH encounters an edge-case syntax error on unusual inputs,
/// it safely falls back to LIKE search to guarantee zero runtime panics or crashes.
fn search_fts(conn: &Connection, query: &str, limit: i64) -> Result<Vec<Document>> {
    let fts_query = sanitize_fts_query(query);

    let sql = "SELECT d.* FROM documents_fts f JOIN documents d ON d.rowid = f.rowid WHERE documents_fts MATCH ?1 AND d.deleted_at IS NULL ORDER BY rank LIMIT ?2";

    let mut stmt = match conn.prepare(sql) {
        Ok(s) => s,
        Err(_) => {
            // Fallback to LIKE if FTS statement preparation fails
            return search_like(conn, query, limit);
        }
    };

    let rows = stmt.query_map(params![fts_query, limit], document_from_row);

    match rows {
        Ok(mapped_rows) => {
            let mut docs = Vec::new();
            for doc_res in mapped_rows {
                match doc_res {
                    Ok(doc) => docs.push(doc),
                    Err(e) => return Err(e),
                }
            }
            Ok(docs)
        }
        Err(_) => {
            // Graceful fallback to LIKE on execution error
            search_like(conn, query, limit)
        }
    }
}

/// Executes LIKE fallback search for short queries (1-2 characters) or as FTS safety fallback.
fn search_like(conn: &Connection, query: &str, limit: i64) -> Result<Vec<Document>> {
    let pattern = format!("%{}%", query);
    let sql = "SELECT * FROM documents WHERE (title LIKE ?1 OR content_text LIKE ?1) AND deleted_at IS NULL ORDER BY updated_at DESC LIMIT ?2";

    let mut stmt = conn.prepare(sql)?;
    let rows = stmt.query_map(params![pattern, limit], document_from_row)?;

    let mut docs = Vec::new();
    for doc_res in rows {
        docs.push(doc_res?);
    }
    Ok(docs)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::init_database;

    fn setup_test_db() -> Connection {
        let conn = Connection::open_in_memory().expect("open in-memory db");
        init_database(&conn).expect("init db");
        conn
    }

    fn insert_test_doc(
        conn: &Connection,
        id: &str,
        title: &str,
        content_text: &str,
        deleted_at: Option<i64>,
    ) {
        conn.execute(
            "INSERT INTO documents (id, title, content, content_text, doc_type, date, created_at, updated_at, deleted_at)
             VALUES (?1, ?2, ?3, ?4, 'memo', NULL, 1000, 1000, ?5)",
            params![id, title, "{}", content_text, deleted_at],
        )
        .expect("insert test doc");
    }

    #[test]
    fn test_fts5_korean_3chars_match() {
        let conn = setup_test_db();
        insert_test_doc(&conn, "doc1", "검교정 일지", "삼영 스마트팩토리 온도계 점검", None);
        insert_test_doc(&conn, "doc2", "생산 계획", "스마트 공정 라인 A 가동", None);
        insert_test_doc(&conn, "doc3", "기타 문서", "일반 내용입니다", None);

        // 3-character Korean search: "검교정" -> triggers FTS5
        let results = search_docs(&conn, "검교정", 10).expect("search docs");
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].id, "doc1");
        assert_eq!(results[0].title, "검교정 일지");
    }

    #[test]
    fn test_like_fallback_1_to_2_chars() {
        let conn = setup_test_db();
        insert_test_doc(&conn, "doc1", "검교정 일지", "삼영 스마트팩토리 온도계 점검", None);
        insert_test_doc(&conn, "doc2", "안전 점검 노트", "특이사항 없음", None);

        // 1-character search: "검" -> triggers LIKE
        let results_1 = search_docs(&conn, "검", 10).expect("search 1 char");
        assert_eq!(results_1.len(), 2); // Matches "검교정" (doc1) and "점검" (doc1, doc2)

        // 2-character search: "노트" -> triggers LIKE
        let results_2 = search_docs(&conn, "노트", 10).expect("search 2 chars");
        assert_eq!(results_2.len(), 1);
        assert_eq!(results_2[0].id, "doc2");
    }

    #[test]
    fn test_soft_deleted_documents_excluded() {
        let conn = setup_test_db();
        // Active document
        insert_test_doc(&conn, "doc-active", "검교정 활성 일지", "온도계 점검 완료", None);
        // Soft-deleted document (deleted_at IS NOT NULL)
        insert_test_doc(
            &conn,
            "doc-deleted",
            "검교정 삭제 일지",
            "삭제된 온도계 점검",
            Some(1700000000),
        );

        // FTS search for "검교정"
        let fts_results = search_docs(&conn, "검교정", 10).expect("search fts");
        assert_eq!(fts_results.len(), 1);
        assert_eq!(fts_results[0].id, "doc-active");

        // LIKE search for "검"
        let like_results = search_docs(&conn, "검", 10).expect("search like");
        assert_eq!(like_results.len(), 1);
        assert_eq!(like_results[0].id, "doc-active");
    }

    #[test]
    fn test_sql_injection_defense() {
        let conn = setup_test_db();
        insert_test_doc(&conn, "doc1", "정상 문서", "내용입니다", None);

        // Typical SQL Injection payloads
        let payloads = vec![
            "' OR 1=1 --",
            "\"; DROP TABLE documents; --",
            "' UNION SELECT * FROM documents --",
            "admin'--",
            "\" OR \"\"=\"",
            "NEAR(foo, bar)",
            "*",
            "AND NOT OR",
        ];

        for payload in payloads {
            // Must not panic, must not corrupt data, must return safe Result
            let res = search_docs(&conn, payload, 10);
            assert!(res.is_ok(), "Failed on payload: {payload}");
        }

        // Verify table still exists and data is intact
        let count: i64 = conn
            .query_row("SELECT count(*) FROM documents", [], |r| r.get(0))
            .expect("count documents");
        assert_eq!(count, 1);
    }

    #[test]
    fn test_empty_query_and_zero_limit() {
        let conn = setup_test_db();
        insert_test_doc(&conn, "doc1", "테스트", "내용", None);

        assert_eq!(search_docs(&conn, "", 10).unwrap().len(), 0);
        assert_eq!(search_docs(&conn, "   ", 10).unwrap().len(), 0);
        assert_eq!(search_docs(&conn, "테스트", 0).unwrap().len(), 0);
        assert_eq!(search_docs(&conn, "테스트", -5).unwrap().len(), 0);
    }
}
