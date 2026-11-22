#[cfg(test)]
mod tests {
    use rusqlite::Connection;
    use crate::ai::{AiState, DEFAULT_OLLAMA_ENDPOINT};
    use crate::attachment::save_attachment_file;
    use crate::backup::sanitize_filename;
    use crate::db::{create_in_memory_pool, init_database};
    use crate::doc::{create_or_update_doc, get_or_create_daily, Document};
    use crate::search::search_docs;

    fn setup_test_db() -> Connection {
        let conn = Connection::open_in_memory().expect("open memory db");
        init_database(&conn).expect("init db");
        conn
    }

    // ========================================================================
    // 1. SQLite 쿼리 인젝션 (FTS5 및 LIKE 파라미터 바인딩 검증)
    // ========================================================================
    #[test]
    fn test_security_fts5_and_like_injection_resistance() {
        let conn = setup_test_db();

        // Legitimate test documents
        conn.execute(
            "INSERT INTO documents (id, title, content, content_text, doc_type, date, created_at, updated_at, deleted_at)
             VALUES ('doc-sec-1', '기밀 제조공정 표준서', '{\"ops\":[]}', '삼영 스마트팩토리 비공개 제조 레시피 규격', 'memo', NULL, 1000, 1000, NULL)",
            [],
        ).unwrap();

        conn.execute(
            "INSERT INTO documents (id, title, content, content_text, doc_type, date, created_at, updated_at, deleted_at)
             VALUES ('doc-sec-2', '공개 공지사항', '{\"ops\":[]}', '사내 식당 메뉴 안내 및 일반 게시판', 'memo', NULL, 1000, 1000, NULL)",
            [],
        ).unwrap();

        let injection_payloads = vec![
            // Classic SQL injection attacks
            "' OR '1'='1",
            "'; DROP TABLE documents; --",
            "'; DROP TABLE documents_fts; --",
            "' UNION SELECT id, title, content, content_text, doc_type, date, created_at, updated_at, deleted_at FROM documents --",
            "1' OR 1=1 #",
            "admin' --",
            // FTS5 syntax corruption attacks
            "\" OR \"\"=\"",
            "\"\"\"\"\"\"\"\"\"",
            "NEAR(manufacturing, secret, 10)",
            "NOT OR AND",
            "*",
            "title: *",
            "{title content_text} : *",
            "^recipe*",
            "column:value",
            "((( unbalanced parenthesis",
            "unicode null \0 inject",
        ];

        for payload in injection_payloads {
            let search_result = search_docs(&conn, payload, 50);
            assert!(
                search_result.is_ok(),
                "Search failed or panicked on payload: {}",
                payload
            );

            let docs = search_result.unwrap();
            if payload.contains("OR '1'='1") || payload.contains("1=1") {
                assert!(
                    docs.is_empty(),
                    "SQL injection payload '{}' matched unexpected documents!",
                    payload
                );
            }
        }

        // Verify tables intact
        let table_count: i64 = conn
            .query_row(
                "SELECT count(*) FROM sqlite_master WHERE type='table' AND name IN ('documents', 'documents_fts')",
                [],
                |r| r.get(0),
            )
            .expect("verify tables exist");
        assert_eq!(table_count, 2, "Tables were dropped or corrupted!");

        let doc_count: i64 = conn
            .query_row("SELECT count(*) FROM documents", [], |r| r.get(0))
            .expect("verify documents remain intact");
        assert_eq!(doc_count, 2, "Documents were deleted or corrupted!");
    }

    #[test]
    fn test_security_doc_crud_parameter_binding() {
        let mut conn = setup_test_db();

        let evil_title = "Title'; DROP TABLE documents; --";
        let evil_content = "Content' OR '1'='1";
        let evil_date = "2026-11-22'; DELETE FROM documents; --";

        let evil_doc = Document {
            id: "evil-doc-1".to_string(),
            title: evil_title.to_string(),
            content: evil_content.to_string(),
            content_text: "Clean text".to_string(),
            doc_type: "memo".to_string(),
            date: Some(evil_date.to_string()),
            created_at: 1000,
            updated_at: 0,
            deleted_at: None,
        };

        let created = create_or_update_doc(&conn, &evil_doc).expect("insert evil doc safely");
        assert_eq!(created.title, evil_title);

        let count: i64 = conn
            .query_row("SELECT count(*) FROM documents", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 1);

        let daily_res = get_or_create_daily(&mut conn, "2026-11-22' OR '1'='1");
        assert!(daily_res.is_ok(), "get_or_create_daily handled injection safely");
    }

    // ========================================================================
    // 2. 첨부파일 저장 경로 조작 (Directory Traversal 방어)
    // ========================================================================
    #[test]
    fn test_security_attachment_directory_traversal_defense() {
        let temp_root = std::env::temp_dir().join(format!("sy_sec_mod_{}", uuid::Uuid::new_v4()));
        let base_data_dir = temp_root.join("data");
        std::fs::create_dir_all(&base_data_dir).unwrap();

        let dummy_base64 = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNk+M9QDwADhgGAWjR9awAAAABJRU5ErkJggg==";

        let malicious_filenames = vec![
            "../../../../evil.exe",
            "..\\..\\..\\windows\\system32\\calc.exe",
            "/etc/passwd",
            "C:\\Windows\\System32\\cmd.exe",
            "....//....//shell.bat",
            "payload.vbs",
            "normal_name.png",
        ];

        for malicious_name in malicious_filenames {
            let result = save_attachment_file(&base_data_dir, malicious_name, dummy_base64);
            assert!(result.is_ok(), "save_attachment_file failed on: {}", malicious_name);

            let saved_rel_path = result.unwrap();
            assert!(
                saved_rel_path.starts_with("attachments/"),
                "Relative path '{}' escaped attachments dir!",
                saved_rel_path
            );

            let full_path = base_data_dir.join(&saved_rel_path);
            assert!(full_path.exists(), "Target file was not written");

            let canonical_target = full_path.canonicalize().unwrap();
            let canonical_attachments = base_data_dir.join("attachments").canonicalize().unwrap();
            assert!(
                canonical_target.starts_with(&canonical_attachments),
                "Directory traversal vulnerability! Target {:?} escaped attachments dir {:?}",
                canonical_target,
                canonical_attachments
            );

            let ext = full_path.extension().and_then(|e| e.to_str()).unwrap_or("");
            assert!(
                ext == "png" || ext == "jpg" || ext == "jpeg" || ext == "gif" || ext == "webp" || ext == "svg" || ext == "pdf" || ext == "txt",
                "Dangerous executable extension allowed: {}",
                ext
            );
        }

        assert_eq!(sanitize_filename("../../etc/shadow", "fallback"), "etc_shadow");
        assert_eq!(sanitize_filename("..\\..\\Windows\\System32", "fallback"), "Windows_System32");
        assert_eq!(sanitize_filename("...///...", "fallback"), "fallback");

        let _ = std::fs::remove_dir_all(&temp_root);
    }

    // ========================================================================
    // 3. 외부 통신 데이터 유출 방지 및 로컬 우선 원칙 검증
    // ========================================================================
    #[test]
    fn test_security_local_first_and_network_leak_prevention() {
        assert_eq!(DEFAULT_OLLAMA_ENDPOINT, "http://127.0.0.1:11434/v1");

        let ai_state = AiState::new();
        let current_endpoint = ai_state.get_endpoint();
        assert!(
            current_endpoint.contains("127.0.0.1") || current_endpoint.contains("localhost"),
            "Default endpoint is not local loopback: {}",
            current_endpoint
        );

        assert_eq!(ai_state.get_model(), "qwen2.5:latest");

        let pool = create_in_memory_pool().unwrap();
        let conn = pool.get().unwrap();
        let journal_mode: String = conn
            .query_row("PRAGMA journal_mode;", [], |r| r.get(0))
            .unwrap();
        assert!(!journal_mode.is_empty());
    }
}
