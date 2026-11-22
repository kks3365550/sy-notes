use rusqlite::Connection;
use sy_notes_lib::db::init_database;
use sy_notes_lib::doc::{create_or_update_doc, delete_doc, restore_doc, unix_now, Document};
use sy_notes_lib::search::{sanitize_fts_query, search_docs};

/// Helper to initialize an in-memory SQLite database with schema v1.
fn setup_test_db() -> Connection {
    let conn = Connection::open_in_memory().expect("failed to open in-memory database");
    init_database(&conn).expect("failed to initialize database schema v1");
    conn
}

/// Helper to create and insert a test Document.
fn create_test_doc(
    id: &str,
    title: &str,
    content_text: &str,
    doc_type: &str,
    updated_at: i64,
    deleted_at: Option<i64>,
) -> Document {
    Document {
        id: id.to_string(),
        title: title.to_string(),
        content: format!(r#"{{"type":"doc","text":"{}"}}"#, content_text),
        content_text: content_text.to_string(),
        doc_type: doc_type.to_string(),
        date: if doc_type == "daily" {
            Some("2026-11-22".to_string())
        } else {
            None
        },
        created_at: 1700000000000,
        updated_at,
        deleted_at,
    }
}

// =========================================================================
// 1. 한국어 3자 검색(`검교정`) FTS5 적중 검증
// =========================================================================
#[test]
fn test_korean_3chars_fts5_trigram_match() {
    let conn = setup_test_db();

    // Insert multiple documents: some with "검교정", some without
    let doc1 = create_test_doc(
        "doc-cal-1",
        "온도계 검교정 표준작업서",
        "삼영 스마트팩토리 가열 공정 온도계 정기 검교정 실시 기록",
        "memo",
        1000,
        None,
    );
    let doc2 = create_test_doc(
        "doc-cal-2",
        "2026년 하반기 계측기 목록",
        "압력계 및 유량계 검교정 성적서 첨부 완료",
        "memo",
        2000,
        None,
    );
    let doc3 = create_test_doc(
        "doc-other",
        "생산 라인 점검 일지",
        "삼영 포장라인 점검 및 일반 청소 작업",
        "memo",
        3000,
        None,
    );

    create_or_update_doc(&conn, &doc1).expect("insert doc1");
    create_or_update_doc(&conn, &doc2).expect("insert doc2");
    create_or_update_doc(&conn, &doc3).expect("insert doc3");

    // 1-1. 3-character Korean search: "검교정" -> triggers FTS5 MATCH
    let results = search_docs(&conn, "검교정", 10).expect("FTS search for '검교정'");
    assert_eq!(
        results.len(),
        2,
        "Expected exactly 2 documents matching '검교정', got {}",
        results.len()
    );

    let ids: Vec<&str> = results.iter().map(|d| d.id.as_str()).collect();
    assert!(ids.contains(&"doc-cal-1"));
    assert!(ids.contains(&"doc-cal-2"));
    assert!(!ids.contains(&"doc-other"));

    // 1-2. FTS match in title only: "표준작업서" (5 chars)
    let results_title = search_docs(&conn, "표준작업서", 10).expect("FTS search for title");
    assert_eq!(results_title.len(), 1);
    assert_eq!(results_title[0].id, "doc-cal-1");

    // 1-3. FTS match in content_text only: "스마트팩토리" (6 chars)
    let results_content = search_docs(&conn, "스마트팩토리", 10).expect("FTS search for content");
    assert_eq!(results_content.len(), 1);
    assert_eq!(results_content[0].id, "doc-cal-1");

    // 1-4. Non-matching 3-character query
    let results_none = search_docs(&conn, "자율주행", 10).expect("FTS search for non-matching");
    assert_eq!(results_none.len(), 0);
}

// =========================================================================
// 2. 1~2자 검색(`검`, `노트`) LIKE 폴백 적중 검증
// =========================================================================
#[test]
fn test_like_fallback_1_and_2_chars() {
    let conn = setup_test_db();

    // doc-a has "검교정" (contains '검'), updated_at = 1000
    let doc_a = create_test_doc(
        "doc-a",
        "검교정 일지",
        "온도계 100도 점검",
        "memo",
        1000,
        None,
    );
    // doc-b has "안전 점검 노트" (contains '검' and '노트'), updated_at = 2000
    let doc_b = create_test_doc(
        "doc-b",
        "안전 점검 노트",
        "특이사항 없음",
        "memo",
        2000,
        None,
    );
    // doc-c has "회의 메모" (no '검', no '노트'), updated_at = 3000
    let doc_c = create_test_doc(
        "doc-c",
        "회의 메모",
        "생산량 논의",
        "memo",
        3000,
        None,
    );

    create_or_update_doc(&conn, &doc_a).expect("insert doc_a");
    create_or_update_doc(&conn, &doc_b).expect("insert doc_b");
    create_or_update_doc(&conn, &doc_c).expect("insert doc_c");

    // Explicitly set updated_at timestamps to test ORDER BY updated_at DESC
    conn.execute("UPDATE documents SET updated_at = 1000 WHERE id = 'doc-a'", []).expect("set doc_a updated_at");
    conn.execute("UPDATE documents SET updated_at = 2000 WHERE id = 'doc-b'", []).expect("set doc_b updated_at");
    conn.execute("UPDATE documents SET updated_at = 3000 WHERE id = 'doc-c'", []).expect("set doc_c updated_at");

    // 2-1. 1-character search: "검" (char_count = 1 < 3 -> LIKE fallback)
    // Matches doc_a (검교정, 점검) and doc_b (점검)
    let results_1char = search_docs(&conn, "검", 10).expect("search 1 char '검'");
    assert_eq!(
        results_1char.len(),
        2,
        "Expected 2 documents matching 1-char '검'"
    );

    // Verify ordering: updated_at DESC (doc-b=2000 first, doc-a=1000 second)
    assert_eq!(results_1char[0].id, "doc-b");
    assert_eq!(results_1char[1].id, "doc-a");

    // 2-2. 2-character search: "노트" (char_count = 2 < 3 -> LIKE fallback)
    // Matches doc_b ("안전 점검 노트")
    let results_2char = search_docs(&conn, "노트", 10).expect("search 2 char '노트'");
    assert_eq!(
        results_2char.len(),
        1,
        "Expected 1 document matching 2-char '노트'"
    );
    assert_eq!(results_2char[0].id, "doc-b");
    assert_eq!(results_2char[0].title, "안전 점검 노트");

    // 2-3. Limit restriction on LIKE fallback
    let results_limit = search_docs(&conn, "검", 1).expect("search with limit 1");
    assert_eq!(
        results_limit.len(),
        1,
        "Expected exactly 1 document with limit 1"
    );
    assert_eq!(results_limit[0].id, "doc-b", "Expected newest document first");
}

// =========================================================================
// 3. 소프트 삭제(`deleted_at IS NOT NULL`) 문서가 FTS 검색에서 0건으로 정확히 필터링되는지 검증
// =========================================================================
#[test]
fn test_soft_deleted_documents_excluded_from_fts_and_like() {
    let conn = setup_test_db();

    // 1) Active document with "검교정" and "노트"
    let doc_active = create_test_doc(
        "doc-active",
        "활성 검교정 업무일지",
        "정상 가동 중인 검교정 노트 내용",
        "memo",
        1000,
        None,
    );

    // 2) Soft-deleted document with "검교정" and "노트" (deleted_at is set)
    let doc_deleted = create_test_doc(
        "doc-deleted",
        "삭제된 검교정 폐기일지",
        "휴지통으로 이동된 검교정 노트 내용",
        "memo",
        2000,
        Some(1700000100000),
    );

    create_or_update_doc(&conn, &doc_active).expect("insert doc_active");
    create_or_update_doc(&conn, &doc_deleted).expect("insert doc_deleted");

    // 3-1. FTS5 3-character search: "검교정"
    // Must return ONLY doc-active (1 match). doc-deleted MUST NOT appear (0 count).
    let fts_results = search_docs(&conn, "검교정", 10).expect("FTS search");
    assert_eq!(
        fts_results.len(),
        1,
        "Soft-deleted document must be excluded from FTS search"
    );
    assert_eq!(fts_results[0].id, "doc-active");

    // 3-2. LIKE 1-character search: "검"
    // Must return ONLY doc-active.
    let like_1char = search_docs(&conn, "검", 10).expect("LIKE 1-char search");
    assert_eq!(
        like_1char.len(),
        1,
        "Soft-deleted document must be excluded from LIKE 1-char search"
    );
    assert_eq!(like_1char[0].id, "doc-active");

    // 3-3. LIKE 2-character search: "노트"
    // Must return ONLY doc-active.
    let like_2char = search_docs(&conn, "노트", 10).expect("LIKE 2-char search");
    assert_eq!(
        like_2char.len(),
        1,
        "Soft-deleted document must be excluded from LIKE 2-char search"
    );
    assert_eq!(like_2char[0].id, "doc-active");

    // 3-4. When active document is also soft-deleted, search returns 0 results
    delete_doc(&conn, "doc-active").expect("soft delete active doc");

    let fts_after_del = search_docs(&conn, "검교정", 10).expect("FTS search after deletion");
    assert_eq!(
        fts_after_del.len(),
        0,
        "Expected 0 results when all matching docs are soft-deleted"
    );

    let like_after_del = search_docs(&conn, "검", 10).expect("LIKE search after deletion");
    assert_eq!(
        like_after_del.len(),
        0,
        "Expected 0 results for LIKE when all matching docs are soft-deleted"
    );
}

// =========================================================================
// 4. 전체 수명주기(생성 -> 수정 -> 소프트 삭제 -> 복구) 검색 반영 검증
// =========================================================================
#[test]
fn test_search_lifecycle_integration() {
    let conn = setup_test_db();

    // 1) Create doc with "검교정"
    let mut doc = create_test_doc(
        "lifecycle-doc",
        "검교정 초기 등록",
        "초기 온도계 검교정 데이터",
        "memo",
        0,
        None,
    );
    let created = create_or_update_doc(&conn, &doc).expect("create doc");

    // Verified: FTS matches "검교정"
    let res1 = search_docs(&conn, "검교정", 10).unwrap();
    assert_eq!(res1.len(), 1);

    // 2) Update doc: replace "검교정" with "금속검출기" (pass valid updated_at for optimistic lock)
    doc.title = "금속검출기 관리대장".to_string();
    doc.content_text = "금속검출기 센서 감도 점검".to_string();
    doc.updated_at = created.updated_at;
    let _updated = create_or_update_doc(&conn, &doc).expect("update doc");

    // Old term "검교정" must NOT match
    let res_old = search_docs(&conn, "검교정", 10).unwrap();
    assert_eq!(res_old.len(), 0, "Old term must not match after update");

    // New term "금속검" (3 chars) must match via FTS
    let res_new_fts = search_docs(&conn, "금속검", 10).unwrap();
    assert_eq!(res_new_fts.len(), 1);
    assert_eq!(res_new_fts[0].title, "금속검출기 관리대장");

    // 3) Soft-delete doc
    delete_doc(&conn, "lifecycle-doc").expect("soft delete");
    assert_eq!(search_docs(&conn, "금속검", 10).unwrap().len(), 0);
    assert_eq!(search_docs(&conn, "금속", 10).unwrap().len(), 0);

    // 4) Restore doc
    restore_doc(&conn, "lifecycle-doc").expect("restore doc");
    let res_restored_fts = search_docs(&conn, "금속검", 10).unwrap();
    assert_eq!(
        res_restored_fts.len(),
        1,
        "Restored doc must reappear in FTS search"
    );

    let res_restored_like = search_docs(&conn, "금속", 10).unwrap();
    assert_eq!(
        res_restored_like.len(),
        1,
        "Restored doc must reappear in LIKE search"
    );
}

// =========================================================================
// 5. 보안: SQL Injection 및 FTS5 특수문자 방어 검증
// =========================================================================
#[test]
fn test_security_sql_injection_and_fts_syntax_defense() {
    let conn = setup_test_db();

    let doc = create_test_doc(
        "sec-doc",
        "정상 검교정 데이터",
        "보안 테스트를 위한 정상 레코드",
        "memo",
        1000,
        None,
    );
    create_or_update_doc(&conn, &doc).expect("insert sec-doc");

    // Test a wide variety of SQL Injection and FTS syntax error payloads
    let attack_payloads = vec![
        "' OR 1=1 --",
        "\"; DROP TABLE documents; --",
        "' UNION SELECT * FROM documents --",
        "admin'--",
        "\" OR \"\"=\"",
        "NEAR(foo, bar)",
        "AND NOT OR",
        "col:val",
        "***",
        "^prefix*",
        "((()))",
        "'{[(<#$%&~|`>)]}'",
    ];

    for payload in attack_payloads {
        // Query must never panic or return Err due to injection or syntax error
        let search_result = search_docs(&conn, payload, 10);
        assert!(
            search_result.is_ok(),
            "Search failed on malicious payload: {}",
            payload
        );
    }

    // Verify database integrity: documents table must still exist and contain 1 record
    let count: i64 = conn
        .query_row("SELECT count(*) FROM documents", [], |r| r.get(0))
        .expect("table count check");
    assert_eq!(count, 1, "Documents table must remain intact");

    // Verify sanitize_fts_query helper directly
    assert_eq!(sanitize_fts_query("hello"), "\"hello\"");
    assert_eq!(sanitize_fts_query("foo\"bar"), "\"foo\"\"bar\"");
    assert_eq!(sanitize_fts_query("   "), "");
}

// =========================================================================
// 6. 경계 조건 및 엣지 케이스 검증
// =========================================================================
#[test]
fn test_search_edge_cases() {
    let conn = setup_test_db();

    let doc = create_test_doc(
        "edge-doc",
        "삼영 스마트팩토리 CCP-2B 검교정 100°C",
        "영문(English), 한글(Korean), 숫자(12345), 특수문자(!@#$%) 혼합",
        "memo",
        unix_now(),
        None,
    );
    create_or_update_doc(&conn, &doc).expect("insert edge-doc");

    // 6-1. Empty query -> empty result
    assert_eq!(search_docs(&conn, "", 10).unwrap().len(), 0);
    assert_eq!(search_docs(&conn, "   ", 10).unwrap().len(), 0);

    // 6-2. Zero or negative limit -> empty result
    assert_eq!(search_docs(&conn, "검교정", 0).unwrap().len(), 0);
    assert_eq!(search_docs(&conn, "검교정", -1).unwrap().len(), 0);

    // 6-3. Mixed alphanumeric & symbols search
    let res_mixed = search_docs(&conn, "CCP-2B", 10).expect("search mixed");
    assert_eq!(res_mixed.len(), 1);

    // 6-4. English 3+ characters FTS search
    let res_eng = search_docs(&conn, "English", 10).expect("search english");
    assert_eq!(res_eng.len(), 1);

    // 6-5. English 1~2 characters LIKE fallback
    let res_eng_short = search_docs(&conn, "En", 10).expect("search english short");
    assert_eq!(res_eng_short.len(), 1);
}
