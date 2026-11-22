#[cfg(test)]
mod tests {
    use rusqlite::Connection;
    use crate::db::init_database;
    use crate::doc::{
        create_or_update_doc, delete_doc, get_doc, get_or_create_daily, purge_trash, restore_doc,
        Document,
    };
    use crate::search::search_docs;

    fn setup_db() -> Connection {
        let conn = Connection::open_in_memory().expect("open in-memory db");
        init_database(&conn).expect("initialize schema v1");
        conn
    }

    // =========================================================================
    // 1. SQLite 마이그레이션 v0 -> v1 무결성 검증
    // =========================================================================
    #[test]
    fn test_logic_item1_migration_v0_to_v1_integrity() {
        let conn = Connection::open_in_memory().expect("open memory connection");

        // 1-1. Initial state must be user_version 0
        let v0: i32 = conn
            .query_row("PRAGMA user_version;", [], |r| r.get(0))
            .expect("query initial user_version");
        assert_eq!(v0, 0, "Initial database user_version must be 0");

        // 1-2. Execute migration v1
        init_database(&conn).expect("migration v0 -> v1 must succeed");

        // 1-3. Verify user_version is updated to 1
        let v1: i32 = conn
            .query_row("PRAGMA user_version;", [], |r| r.get(0))
            .expect("query migrated user_version");
        assert_eq!(v1, 1, "Database user_version after migration must be 1");

        // 1-4. Verify all required tables exist
        let tables: Vec<String> = conn
            .prepare("SELECT name FROM sqlite_master WHERE type IN ('table', 'virtual') ORDER BY name")
            .expect("query tables")
            .query_map([], |r| r.get(0))
            .expect("map table names")
            .collect::<Result<Vec<_>, _>>()
            .expect("collect table names");

        assert!(tables.contains(&"documents".to_string()), "documents table must exist");
        assert!(tables.contains(&"documents_fts".to_string()), "documents_fts virtual table must exist");
        assert!(tables.contains(&"attachments".to_string()), "attachments table must exist");
        assert!(tables.contains(&"settings".to_string()), "settings table must exist");

        // 1-5. Verify FTS5 triggers exist (documents_ai, documents_ad, documents_au)
        let triggers: Vec<String> = conn
            .prepare("SELECT name FROM sqlite_master WHERE type = 'trigger' ORDER BY name")
            .expect("query triggers")
            .query_map([], |r| r.get(0))
            .expect("map trigger names")
            .collect::<Result<Vec<_>, _>>()
            .expect("collect trigger names");

        assert!(triggers.contains(&"documents_ai".to_string()), "documents_ai trigger must exist");
        assert!(triggers.contains(&"documents_ad".to_string()), "documents_ad trigger must exist");
        assert!(triggers.contains(&"documents_au".to_string()), "documents_au trigger must exist");

        // 1-6. Verify unique index uq_daily exists
        let indexes: Vec<String> = conn
            .prepare("SELECT name FROM sqlite_master WHERE type = 'index' ORDER BY name")
            .expect("query indexes")
            .query_map([], |r| r.get(0))
            .expect("map index names")
            .collect::<Result<Vec<_>, _>>()
            .expect("collect index names");

        assert!(indexes.contains(&"uq_daily".to_string()), "uq_daily index must exist");

        // 1-7. Idempotency check: Running init_database a second time must succeed without error or version change
        init_database(&conn).expect("re-running init_database must be idempotent");
        let v1_recheck: i32 = conn
            .query_row("PRAGMA user_version;", [], |r| r.get(0))
            .expect("recheck user_version");
        assert_eq!(v1_recheck, 1, "user_version must remain 1 after repeated init");
    }

    // =========================================================================
    // 2. get_or_create_daily 자정 롤오버 및 동일 날짜 중복 생성 차단 (uq_daily)
    // =========================================================================
    #[test]
    fn test_logic_item2_get_or_create_daily_rollover_and_uq_daily() {
        let mut conn = setup_db();
        let date_today = "2026-11-22";
        let date_tomorrow = "2026-11-23";

        // 2-1. Initial creation of today's daily log
        let daily1 = get_or_create_daily(&mut conn, date_today).expect("create today daily");
        assert_eq!(daily1.doc_type, "daily");
        assert_eq!(daily1.date.as_deref(), Some(date_today));
        assert!(daily1.title.contains(date_today));
        assert!(daily1.content.contains("오늘 목표"));
        assert!(daily1.deleted_at.is_none());

        // 2-2. Calling get_or_create_daily again on the same date returns the existing record (idempotency)
        let daily2 = get_or_create_daily(&mut conn, date_today).expect("get today daily again");
        assert_eq!(daily1.id, daily2.id, "Same date must return identical document ID");
        assert_eq!(daily1.created_at, daily2.created_at, "created_at must be preserved");

        // 2-3. Verify uq_daily constraint: direct raw insert of duplicate active daily doc fails
        let duplicate_insert = conn.execute(
            "INSERT INTO documents (id, title, content, content_text, doc_type, date, created_at, updated_at)
             VALUES ('dup-id', '중복 일지', '{}', '', 'daily', ?1, 2000, 2000)",
            rusqlite::params![date_today],
        );
        assert!(
            duplicate_insert.is_err(),
            "Direct insert of duplicate daily log for the same date must fail with uq_daily violation"
        );

        // 2-4. Midnight rollover check: Requesting tomorrow's date creates a distinct new document
        let daily_tomorrow = get_or_create_daily(&mut conn, date_tomorrow).expect("create tomorrow daily");
        assert_ne!(daily1.id, daily_tomorrow.id, "Rollover date must create a distinct document");
        assert_eq!(daily_tomorrow.date.as_deref(), Some(date_tomorrow));
        assert!(daily_tomorrow.title.contains(date_tomorrow));

        // 2-5. Soft-deleted daily log resurrection:
        // If today's daily log was soft-deleted, get_or_create_daily must restore it rather than creating a duplicate
        delete_doc(&conn, &daily1.id).expect("soft delete today daily");
        let deleted_check = get_doc(&conn, &daily1.id).expect("fetch deleted").expect("doc exists");
        assert!(deleted_check.deleted_at.is_some(), "Document must be soft deleted");

        let restored_daily = get_or_create_daily(&mut conn, date_today).expect("resurrect daily");
        assert_eq!(restored_daily.id, daily1.id, "Restored daily must retain original ID");
        assert!(restored_daily.deleted_at.is_none(), "Restored daily must have deleted_at = NULL");
    }

    // =========================================================================
    // 3. FTS5 한글 3자 검색 및 1~2자 LIKE 폴백 정확도
    // =========================================================================
    #[test]
    fn test_logic_item3_fts5_korean_3chars_and_like_fallback() {
        let conn = setup_db();

        // Prepare test documents
        let doc_a = Document {
            id: "doc-calib".to_string(),
            title: "식품안전 검교정 일지".to_string(),
            content: "{\"blocks\":[]}".to_string(),
            content_text: "삼영 스마트팩토리 온도계 및 센서 검교정 점검 기록표".to_string(),
            doc_type: "daily".to_string(),
            date: Some("2026-11-22".to_string()),
            created_at: 1000,
            updated_at: 1000,
            deleted_at: None,
        };
        let doc_b = Document {
            id: "doc-motor".to_string(),
            title: "생산설비 모터 수리".to_string(),
            content: "{\"blocks\":[]}".to_string(),
            content_text: "공정 2라인 구동 모터 진동 측정 및 윤활유 보충".to_string(),
            doc_type: "memo".to_string(),
            date: None,
            created_at: 2000,
            updated_at: 2000,
            deleted_at: None,
        };
        let doc_c = Document {
            id: "doc-metal".to_string(),
            title: "금속검출기 감도 테스트".to_string(),
            content: "{\"blocks\":[]}".to_string(),
            content_text: "CCP-2B 금속 검출 한계치 Fe 1.0mm, SUS 1.5mm 검증 완료".to_string(),
            doc_type: "memo".to_string(),
            date: None,
            created_at: 3000,
            updated_at: 3000,
            deleted_at: None,
        };

        create_or_update_doc(&conn, &doc_a).expect("insert doc A");
        create_or_update_doc(&conn, &doc_b).expect("insert doc B");
        create_or_update_doc(&conn, &doc_c).expect("insert doc C");

        // 3-1. 3-character Korean trigram FTS5 search: "검교정"
        let res_calib = search_docs(&conn, "검교정", 10).expect("search 검교정");
        assert_eq!(res_calib.len(), 1, "3-char '검교정' must return exactly 1 document");
        assert_eq!(res_calib[0].id, "doc-calib");

        // 3-2. 3-character Korean trigram FTS5 search: "금속검"
        let res_metal = search_docs(&conn, "금속검", 10).expect("search 금속검");
        assert_eq!(res_metal.len(), 1, "3-char '금속검' must return exactly 1 document");
        assert_eq!(res_metal[0].id, "doc-metal");

        // 3-3. 1-character LIKE fallback: "검" (matches "검교정" in doc-calib, and "금속검출기/검출" in doc-metal)
        let res_single = search_docs(&conn, "검", 10).expect("search 1-char 검");
        assert_eq!(res_single.len(), 2, "1-char '검' must return 2 matching documents via LIKE fallback");
        let found_ids: Vec<String> = res_single.into_iter().map(|d| d.id).collect();
        assert!(found_ids.contains(&"doc-calib".to_string()));
        assert!(found_ids.contains(&"doc-metal".to_string()));

        // 3-4. 2-character LIKE fallback: "모터" (matches doc-motor)
        let res_motor = search_docs(&conn, "모터", 10).expect("search 2-char 모터");
        assert_eq!(res_motor.len(), 1, "2-char '모터' must return doc-motor via LIKE fallback");
        assert_eq!(res_motor[0].id, "doc-motor");

        // 3-5. Query sanitization & safety: quotes and special characters must not crash FTS5
        let res_special = search_docs(&conn, "\"검교정\" *AND* OR NOT", 10);
        assert!(res_special.is_ok(), "Sanitized query must safely execute without FTS syntax error");

        // 3-6. Empty / whitespace search returns empty list without error
        let res_empty = search_docs(&conn, "   ", 10).expect("empty query");
        assert!(res_empty.is_empty(), "Empty query must return empty result");
    }

    // =========================================================================
    // 4. 소프트 삭제 문서 검색 배제 (deleted_at IS NULL 조인) 확인
    // =========================================================================
    #[test]
    fn test_logic_item4_soft_deleted_documents_excluded_from_search() {
        let conn = setup_db();

        // Insert active document
        let doc = Document {
            id: "doc-target".to_string(),
            title: "CCP 검교정 기준서".to_string(),
            content: "{\"blocks\":[]}".to_string(),
            content_text: "온도계 및 압력계 주기적 교정 기준 검교정 매뉴얼".to_string(),
            doc_type: "memo".to_string(),
            date: None,
            created_at: 1000,
            updated_at: 1000,
            deleted_at: None,
        };
        create_or_update_doc(&conn, &doc).expect("insert target doc");

        // Pre-condition: FTS5 (3-char) and LIKE (1-char) both find the active document
        let before_fts = search_docs(&conn, "검교정", 10).expect("search fts before delete");
        assert_eq!(before_fts.len(), 1, "Active document must be found by FTS5");

        let before_like = search_docs(&conn, "검", 10).expect("search like before delete");
        assert_eq!(before_like.len(), 1, "Active document must be found by LIKE");

        // 4-1. Perform soft delete
        delete_doc(&conn, &doc.id).expect("soft delete doc");

        // Verify it is indeed soft deleted in database
        let fetched = get_doc(&conn, &doc.id).expect("query doc").expect("exists");
        assert!(fetched.deleted_at.is_some(), "Document must have deleted_at timestamp set");

        // 4-2. FTS5 search (3-character query): MUST return 0 results
        let after_fts = search_docs(&conn, "검교정", 10).expect("search fts after delete");
        assert_eq!(
            after_fts.len(),
            0,
            "Soft-deleted document must be strictly excluded from FTS5 search (deleted_at IS NULL)"
        );

        // 4-3. LIKE fallback search (1-2 character query): MUST return 0 results
        let after_like = search_docs(&conn, "검", 10).expect("search like after delete");
        assert_eq!(
            after_like.len(),
            0,
            "Soft-deleted document must be strictly excluded from LIKE fallback search (deleted_at IS NULL)"
        );

        // 4-4. Restore document: MUST immediately reappear in both searches
        restore_doc(&conn, &doc.id).expect("restore doc");

        let restored_fts = search_docs(&conn, "검교정", 10).expect("search fts after restore");
        assert_eq!(restored_fts.len(), 1, "Restored document must reappear in FTS5 search");

        let restored_like = search_docs(&conn, "검", 10).expect("search like after restore");
        assert_eq!(restored_like.len(), 1, "Restored document must reappear in LIKE search");

        // 4-5. Purge trash: Permanent deletion cleans up database and triggers FTS index cleanup
        delete_doc(&conn, &doc.id).expect("soft delete again");
        let purged_count = purge_trash(&conn, None).expect("purge all trash immediately");
        assert_eq!(purged_count, 1, "Purged 1 trash document");

        let final_fts = search_docs(&conn, "검교정", 10).expect("search fts after purge");
        assert_eq!(final_fts.len(), 0, "Purged document must remain 0 in FTS5 search");
    }
}
