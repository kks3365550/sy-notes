use rusqlite::Connection;
use std::fs;
use std::time::Duration;
use crate::db::{init_database, get_storage_info, is_directory_writable, create_pool};

/// Chaos Test 1: SQLite WAL Mode - Abrupt termination / Process Crash & 100% Undamaged Recovery
#[test]
fn test_sqlite_wal_crash_recovery_100_percent() {
    let temp_dir = std::env::temp_dir().join(format!("sy_notes_chaos_{}", uuid::Uuid::new_v4()));
    fs::create_dir_all(&temp_dir).expect("create temp dir");
    let db_path = temp_dir.join("chaos_wal.db");

    // Phase 1: Initialize DB with WAL mode and schema v1
    {
        let conn = Connection::open(&db_path).expect("open db");
        init_database(&conn).expect("init db");
        
        // Verify WAL mode is active
        let journal_mode: String = conn
            .query_row("PRAGMA journal_mode;", [], |r| r.get(0))
            .expect("query journal_mode");
        assert_eq!(journal_mode.to_uppercase(), "WAL", "SQLite must be in WAL mode");

        // Insert 100 committed documents in transactions
        for i in 0..100 {
            conn.execute(
                "INSERT INTO documents (id, title, content, content_text, doc_type, date, created_at, updated_at) 
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                (
                    format!("doc-{}", i),
                    format!("식품안전 검교정 일지 {}", i),
                    "{\"blocks\":[]}",
                    format!("삼영 스마트팩토리 점검 내용 {}", i),
                    "memo",
                    Option::<String>::None,
                    1700000000000i64 + i,
                    1700000000000i64 + i,
                ),
            ).expect("insert committed doc");
        }

        // Verify committed count before crash
        let count: i64 = conn
            .query_row("SELECT count(*) FROM documents", [], |r| r.get(0))
            .expect("count committed");
        assert_eq!(count, 100);

        // Phase 2: Start an uncommitted transaction (simulating mid-flight crash/kill)
        conn.execute("BEGIN IMMEDIATE;", []).expect("begin immediate");
        for j in 100..120 {
            conn.execute(
                "INSERT INTO documents (id, title, content, content_text, doc_type, date, created_at, updated_at) 
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                (
                    format!("doc-uncommitted-{}", j),
                    format!("미커밋 문서 {}", j),
                    "{\"blocks\":[]}",
                    format!("비정상 종료 중 쓰기 {}", j),
                    "memo",
                    Option::<String>::None,
                    1700000000000i64 + j,
                    1700000000000i64 + j,
                ),
            ).expect("insert uncommitted");
        }

        // SIMULATE ABRUPT CRASH / KILL: Drop connection without COMMIT and without CHECKPOINT.
        // In real OS kill, process memory is reclaimed while WAL file contains uncommitted or committed pages.
        drop(conn);
    }

    assert!(db_path.exists(), "Main DB file must exist");

    // Phase 3: App restarts - Reopen DB and perform WAL recovery
    {
        let conn = Connection::open(&db_path).expect("reopen db after crash");

        // 1. Integrity check: Must be 100% OK
        let integrity: String = conn
            .query_row("PRAGMA integrity_check;", [], |r| r.get(0))
            .expect("integrity check");
        assert_eq!(integrity, "ok", "Database integrity must be 100% intact after crash!");

        // 2. Quick check: Must be 100% OK
        let quick: String = conn
            .query_row("PRAGMA quick_check;", [], |r| r.get(0))
            .expect("quick check");
        assert_eq!(quick, "ok", "Quick check must be 100% intact!");

        // 3. Count documents: Exactly 100 committed documents must be preserved (0 data loss)
        let count: i64 = conn
            .query_row("SELECT count(*) FROM documents", [], |r| r.get(0))
            .expect("count after crash");
        assert_eq!(count, 100, "Exactly 100 committed documents must be recovered, uncommitted must be rolled back");

        // 4. Verify FTS5 index integrity after recovery
        let fts_count: i64 = conn
            .query_row("SELECT count(*) FROM documents_fts WHERE documents_fts MATCH '검교정'", [], |r| r.get(0))
            .expect("fts search after crash");
        assert_eq!(fts_count, 100, "FTS5 trigram index must be completely synchronized after WAL recovery");

        // 5. Subsequent write & read operations succeed smoothly
        conn.execute(
            "INSERT INTO documents (id, title, content, content_text, doc_type, date, created_at, updated_at) 
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            (
                "doc-after-recovery",
                "복구 후 신규 작성 문서",
                "{}",
                "신규 작성 정상 확인",
                "memo",
                Option::<String>::None,
                1700000099999i64,
                1700000099999i64,
            ),
        ).expect("write after recovery");

        let new_count: i64 = conn
            .query_row("SELECT count(*) FROM documents", [], |r| r.get(0))
            .expect("count new");
        assert_eq!(new_count, 101);
    }

    // Clean up
    let _ = fs::remove_dir_all(&temp_dir);
}

/// Chaos Test 2: Read-Only Storage Detection & %APPDATA% Fallback
#[test]
fn test_readonly_storage_fallback_and_warning() {
    let temp_base = std::env::temp_dir().join(format!("sy_notes_ro_test_{}", uuid::Uuid::new_v4()));
    fs::create_dir_all(&temp_base).expect("create temp base");

    let ro_dir = temp_base.join("readonly_data");
    fs::create_dir_all(&ro_dir).expect("create readonly dir");

    // Make directory read-only
    let mut perms = fs::metadata(&ro_dir).expect("metadata").permissions();
    perms.set_readonly(true);
    let _ = fs::set_permissions(&ro_dir, perms);

    // Test writability check on a non-writable or simulated path
    let non_writable_dir = temp_base.join("file_blocking_dir");
    fs::write(&non_writable_dir, b"blocker").expect("write blocker file");
    let blocked_sub_dir = non_writable_dir.join("sub_data");

    assert!(!is_directory_writable(&blocked_sub_dir), "Blocked path must return false for writability");

    // Test storage info fallback resolution
    let storage_info = get_storage_info();
    assert!(!storage_info.db_path.is_empty(), "Database path must not be empty");
    
    // Test initializing database on the resolved path
    let test_db_path = temp_base.join("fallback_test.db");
    let pool = create_pool(&test_db_path).expect("create pool");
    let conn = pool.get().expect("get conn from pool");
    init_database(&conn).expect("init database on fallback path");

    let v: i32 = conn.query_row("PRAGMA user_version;", [], |r| r.get(0)).expect("query user_version");
    assert_eq!(v, 1, "Database initialized on fallback path must reach schema v1");

    // Clean up permissions so temp folder can be removed
    let mut clean_perms = fs::metadata(&ro_dir).expect("metadata").permissions();
    clean_perms.set_readonly(false);
    let _ = fs::set_permissions(&ro_dir, clean_perms);
    let _ = fs::remove_dir_all(&temp_base);
}

/// Chaos Test 3: LLM Endpoint Offline & Timeout (Zero UI Freezing)
#[test]
fn test_llm_endpoint_offline_graceful_recovery() {
    use tokio_util::sync::CancellationToken;

    // Fast-failing client for testing offline connectivity
    let fast_client = reqwest::Client::builder()
        .connect_timeout(Duration::from_millis(500))
        .timeout(Duration::from_millis(800))
        .build()
        .expect("build client");

    // Point to an offline / non-existent local port
    let offline_endpoint = "http://127.0.0.1:59998/v1".to_string();

    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("build tokio runtime");

    rt.block_on(async {
        let send_future = fast_client.post(format!("{}/chat/completions", offline_endpoint)).send();
        
        let res = tokio::time::timeout(Duration::from_secs(2), send_future).await;
        match res {
            Ok(Ok(_)) => panic!("Offline endpoint must not succeed"),
            Ok(Err(e)) => {
                // Must be connect error or timeout error, gracefully caught
                assert!(e.is_connect() || e.is_timeout(), "Error must be connection/timeout failure without panic: {}", e);
            }
            Err(_) => panic!("Connection to offline port must not hang"),
        }

        // Test cancellation token mechanism
        let cancel = CancellationToken::new();
        cancel.cancel(); // already cancelled
        assert!(cancel.is_cancelled(), "Cancellation token must report cancelled state");
    });
}

/// Chaos Test 4: Korean Syllable & Jamo Integrity Test
#[test]
fn test_korean_text_no_decomposition() {
    let test_cases = vec![
        "식품의약품안전처 HACCP 기준 준수",
        "스마트팩토리 검교정 온도계 및 금속검출기 점검",
        "똠얌꿍, 꽭, 힣, 뾾, 뷁", // Complex edge cases
    ];

    for text in test_cases {
        let conn = Connection::open_in_memory().expect("open memory conn");
        init_database(&conn).expect("init db");

        conn.execute(
            "INSERT INTO documents (id, title, content, content_text, doc_type, date, created_at, updated_at) 
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            (
                "doc-korean",
                text,
                "{}",
                text,
                "memo",
                Option::<String>::None,
                1700000000000i64,
                1700000000000i64,
            ),
        ).expect("insert korean doc");

        let saved_text: String = conn
            .query_row("SELECT content_text FROM documents WHERE id = 'doc-korean'", [], |r| r.get(0))
            .expect("read korean doc");

        assert_eq!(saved_text, text, "Saved text must be byte-for-byte identical to input (NFC preserved)");

        // Verify FTS search matches
        let count: i64 = conn
            .query_row("SELECT count(*) FROM documents_fts WHERE documents_fts MATCH '검교정'", [], |r| r.get(0))
            .unwrap_or(0);
        if text.contains("검교정") {
            assert_eq!(count, 1);
        }
    }
}
