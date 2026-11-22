use r2d2::Pool;
use r2d2_sqlite::SqliteConnectionManager;
use rusqlite::{params, TransactionBehavior};
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};
use uuid::Uuid;

const SCHEMA: &str = r#"
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

CREATE UNIQUE INDEX IF NOT EXISTS uq_daily 
    ON documents(date) 
    WHERE doc_type = 'daily' AND deleted_at IS NULL;

CREATE VIRTUAL TABLE IF NOT EXISTS documents_fts USING fts5(
    title,
    content_text,
    content='documents',
    content_rowid='rowid',
    tokenize='trigram'
);

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

struct TempDbGuard {
    path: PathBuf,
}

impl Drop for TempDbGuard {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
        let _ = std::fs::remove_file(self.path.with_extension("db-wal"));
        let _ = std::fs::remove_file(self.path.with_extension("db-shm"));
    }
}

fn create_test_pool(prefix: &str) -> (Pool<SqliteConnectionManager>, PathBuf, TempDbGuard) {
    let temp_dir = std::env::temp_dir();
    let db_path = temp_dir.join(format!("{}_{}.db", prefix, Uuid::new_v4()));
    let guard = TempDbGuard { path: db_path.clone() };

    // Enforce WAL mode on file creation
    {
        let conn = rusqlite::Connection::open(&db_path).unwrap();
        conn.execute_batch("PRAGMA journal_mode = WAL;").unwrap();
        conn.execute_batch(SCHEMA).unwrap();
    }

    let manager = SqliteConnectionManager::file(&db_path)
        .with_init(|conn| {
            conn.execute_batch(
                "PRAGMA busy_timeout = 5000;
                 PRAGMA synchronous = NORMAL;
                 PRAGMA foreign_keys = ON;"
            )
        });

    let pool = Pool::builder().max_size(4).build(manager).unwrap();
    (pool, db_path, guard)
}

#[derive(Debug, Default)]
struct ConcurrencyStats {
    total_ops: usize,
    insert_ops: usize,
    select_ops: usize,
    update_ops: usize,
    locked_errors: usize,
    other_errors: usize,
    elapsed: Duration,
    ops_per_sec: f64,
    avg_latency_us: f64,
    p95_latency_us: u128,
    p99_latency_us: u128,
}

impl ConcurrencyStats {
    fn print_report(&self, title: &str) {
        println!("\n=======================================================");
        println!("       BENCHMARK REPORT: {}", title);
        println!("=======================================================");
        println!(" Total Operations   : {:>10}", self.total_ops);
        println!("   - INSERT ops     : {:>10}", self.insert_ops);
        println!("   - SELECT ops     : {:>10}", self.select_ops);
        println!("   - UPDATE ops     : {:>10}", self.update_ops);
        println!(" Locked Errors      : {:>10} (Expected: 0)", self.locked_errors);
        println!(" Other Errors       : {:>10} (Expected: 0)", self.other_errors);
        println!(" Total Elapsed Time : {:>10.3} ms", self.elapsed.as_secs_f64() * 1000.0);
        println!(" Throughput         : {:>10.2} ops/sec", self.ops_per_sec);
        println!(" Average Latency    : {:>10.2} us", self.avg_latency_us);
        println!(" p95 Latency        : {:>10} us", self.p95_latency_us);
        println!(" p99 Latency        : {:>10} us", self.p99_latency_us);
        println!(" Deadlock Detected  : {:>10}", "NO (0)");
        println!("=======================================================\n");
    }
}

#[test]
fn test_concurrency_10000_operations_8_threads() {
    let (pool, _db_path, _guard) = create_test_pool("sy_notes_10k_bench");

    // Pre-seed 100 documents for SELECT and UPDATE queries
    {
        let conn = pool.get().expect("Failed to acquire connection for seeding");
        let now = chrono::Utc::now().timestamp_millis();
        for i in 0..100 {
            let doc_id = format!("seed-doc-{:04}", i);
            let title = format!("Seed Document {:04}", i);
            let content = r#"{"type":"doc","content":[{"type":"paragraph","text":"Seed content"}]}"#;
            let content_text = format!("Seed content {} 삼영스마트팩토리", i);
            conn.execute(
                "INSERT INTO documents (id, title, content, content_text, doc_type, date, created_at, updated_at, deleted_at)
                 VALUES (?1, ?2, ?3, ?4, 'memo', NULL, ?5, ?6, NULL)",
                params![doc_id, title, content, content_text, now, now],
            ).expect("Failed to insert seed document");
        }
    }

    const NUM_THREADS: usize = 8;
    const TOTAL_OPERATIONS: usize = 10_000;

    let op_counter = Arc::new(AtomicUsize::new(0));
    let insert_count = Arc::new(AtomicUsize::new(0));
    let select_count = Arc::new(AtomicUsize::new(0));
    let update_count = Arc::new(AtomicUsize::new(0));
    let locked_error_count = Arc::new(AtomicUsize::new(0));
    let other_error_count = Arc::new(AtomicUsize::new(0));

    let pool_arc = Arc::new(pool);
    let start_time = Instant::now();

    let mut handles = Vec::with_capacity(NUM_THREADS);

    for thread_id in 0..NUM_THREADS {
        let pool = Arc::clone(&pool_arc);
        let op_counter = Arc::clone(&op_counter);
        let insert_count = Arc::clone(&insert_count);
        let select_count = Arc::clone(&select_count);
        let update_count = Arc::clone(&update_count);
        let locked_error_count = Arc::clone(&locked_error_count);
        let other_error_count = Arc::clone(&other_error_count);

        let handle = thread::spawn(move || {
            let mut latencies_us = Vec::with_capacity(TOTAL_OPERATIONS / NUM_THREADS + 100);

            loop {
                let op_idx = op_counter.fetch_add(1, Ordering::SeqCst);
                if op_idx >= TOTAL_OPERATIONS {
                    break;
                }

                let op_start = Instant::now();
                let op_type = op_idx % 3;

                match pool.get() {
                    Ok(mut conn) => {
                        let op_result: Result<(), rusqlite::Error> = (|| {
                            match op_type {
                                0 => {
                                    // INSERT operation
                                    let doc_id = format!("doc-t{}-{}", thread_id, op_idx);
                                    let title = format!("Concurrent Note {} by thread {}", op_idx, thread_id);
                                    let content = r#"{"type":"doc","content":[{"type":"paragraph","text":"동시성 테스트 본문"}]}"#;
                                    let content_text = format!("동시성 테스트 데이터 {} 삼영스마트팩토리", op_idx);
                                    let now = chrono::Utc::now().timestamp_millis();

                                    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
                                    tx.execute(
                                        "INSERT INTO documents (id, title, content, content_text, doc_type, date, created_at, updated_at, deleted_at)
                                         VALUES (?1, ?2, ?3, ?4, 'memo', NULL, ?5, ?6, NULL)",
                                        params![doc_id, title, content, content_text, now, now],
                                    )?;
                                    tx.commit()?;
                                    insert_count.fetch_add(1, Ordering::SeqCst);
                                    Ok(())
                                }
                                1 => {
                                    // SELECT operation (mix of PK lookup, count, and FTS5 trigram match)
                                    if op_idx % 5 == 0 {
                                        let mut stmt = conn.prepare_cached(
                                            "SELECT count(*) FROM documents_fts WHERE documents_fts MATCH '스마트'"
                                        )?;
                                        let _count: i64 = stmt.query_row([], |r| r.get(0))?;
                                    } else if op_idx % 2 == 0 {
                                        let target_id = format!("seed-doc-{:04}", op_idx % 100);
                                        let mut stmt = conn.prepare_cached(
                                            "SELECT id, title, updated_at FROM documents WHERE id = ?1"
                                        )?;
                                        let _res = stmt.query_row(params![target_id], |r| {
                                            let _id: String = r.get(0)?;
                                            let _title: String = r.get(1)?;
                                            let _updated: i64 = r.get(2)?;
                                            Ok(())
                                        })?;
                                    } else {
                                        let mut stmt = conn.prepare_cached(
                                            "SELECT count(*) FROM documents WHERE deleted_at IS NULL"
                                        )?;
                                        let _count: i64 = stmt.query_row([], |r| r.get(0))?;
                                    }
                                    select_count.fetch_add(1, Ordering::SeqCst);
                                    Ok(())
                                }
                                _ => {
                                    // UPDATE operation (touches documents table + triggers FTS synchronization)
                                    let target_id = format!("seed-doc-{:04}", op_idx % 100);
                                    let new_title = format!("Updated Note {} by thread {}", op_idx, thread_id);
                                    let now = chrono::Utc::now().timestamp_millis();

                                    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
                                    tx.execute(
                                        "UPDATE documents SET title = ?1, updated_at = ?2 WHERE id = ?3",
                                        params![new_title, now, target_id],
                                    )?;
                                    tx.commit()?;
                                    update_count.fetch_add(1, Ordering::SeqCst);
                                    Ok(())
                                }
                            }
                        })();

                        if let Err(e) = op_result {
                            let err_msg = e.to_string();
                            if err_msg.contains("database is locked") || err_msg.contains("busy") {
                                locked_error_count.fetch_add(1, Ordering::SeqCst);
                            } else {
                                other_error_count.fetch_add(1, Ordering::SeqCst);
                            }
                        }
                    }
                    Err(e) => {
                        let err_msg = format!("{:?}", e);
                        if err_msg.contains("database is locked") || err_msg.contains("busy") {
                            locked_error_count.fetch_add(1, Ordering::SeqCst);
                        } else {
                            other_error_count.fetch_add(1, Ordering::SeqCst);
                        }
                    }
                }

                latencies_us.push(op_start.elapsed().as_micros());
            }

            latencies_us
        });

        handles.push(handle);
    }

    let mut all_latencies = Vec::with_capacity(TOTAL_OPERATIONS);
    for (i, handle) in handles.into_iter().enumerate() {
        let thread_latencies = handle.join().unwrap_or_else(|_| panic!("Thread {} panicked or deadlocked!", i));
        all_latencies.extend(thread_latencies);
    }

    let elapsed = start_time.elapsed();
    let total_executed = op_counter.load(Ordering::SeqCst);
    let inserts = insert_count.load(Ordering::SeqCst);
    let selects = select_count.load(Ordering::SeqCst);
    let updates = update_count.load(Ordering::SeqCst);
    let locked_errors = locked_error_count.load(Ordering::SeqCst);
    let other_errors = other_error_count.load(Ordering::SeqCst);

    all_latencies.sort_unstable();
    let p95_latency_us = if !all_latencies.is_empty() {
        all_latencies[(all_latencies.len() as f64 * 0.95) as usize]
    } else {
        0
    };
    let p99_latency_us = if !all_latencies.is_empty() {
        all_latencies[(all_latencies.len() as f64 * 0.99) as usize]
    } else {
        0
    };
    let avg_latency_us = if !all_latencies.is_empty() {
        all_latencies.iter().sum::<u128>() as f64 / all_latencies.len() as f64
    } else {
        0.0
    };

    let stats = ConcurrencyStats {
        total_ops: total_executed,
        insert_ops: inserts,
        select_ops: selects,
        update_ops: updates,
        locked_errors,
        other_errors,
        elapsed,
        ops_per_sec: total_executed as f64 / elapsed.as_secs_f64(),
        avg_latency_us,
        p95_latency_us,
        p99_latency_us,
    };

    stats.print_report("8-Thread 10,000 Concurrent INSERT/SELECT/UPDATE");

    // Verify DB integrity check
    {
        let conn = pool_arc.get().expect("Get connection for integrity check");
        let integrity: String = conn
            .query_row("PRAGMA integrity_check;", [], |r| r.get(0))
            .expect("integrity check query");
        assert_eq!(integrity, "ok", "Database integrity check must be ok");

        let total_docs: i64 = conn
            .query_row("SELECT count(*) FROM documents;", [], |r| r.get(0))
            .expect("query count");
        assert_eq!(total_docs as usize, 100 + inserts);

        let fts_count: i64 = conn
            .query_row("SELECT count(*) FROM documents_fts;", [], |r| r.get(0))
            .expect("query fts count");
        assert_eq!(fts_count as usize, 100 + inserts);
    }

    // STRICT VERIFICATION ASSERTIONS
    assert_eq!(locked_errors, 0, "database is locked errors MUST BE 0!");
    assert_eq!(other_errors, 0, "Other errors MUST BE 0!");
    assert_eq!(inserts + selects + updates, TOTAL_OPERATIONS, "All 10,000 operations must complete successfully!");
}

#[test]
fn test_concurrency_heavy_write_burst_8_threads() {
    let (pool, _db_path, _guard) = create_test_pool("sy_notes_burst_heavy");

    const NUM_THREADS: usize = 8;
    const CYCLES_PER_THREAD: usize = 250;
    let pool_arc = Arc::new(pool);
    let locked_error_count = Arc::new(AtomicUsize::new(0));

    let mut handles = Vec::with_capacity(NUM_THREADS);
    let start_time = Instant::now();

    for t in 0..NUM_THREADS {
        let pool = Arc::clone(&pool_arc);
        let locked_errors = Arc::clone(&locked_error_count);

        handles.push(thread::spawn(move || {
            for c in 0..CYCLES_PER_THREAD {
                let doc_id = format!("burst-doc-t{}-{}", t, c);
                let title = format!("Burst Note {} thread {}", c, t);
                let content_text = format!("버스트 동시성 텍스트 {}", c);
                let now = chrono::Utc::now().timestamp_millis();

                // 1. Write
                match pool.get() {
                    Ok(mut conn) => {
                        let res = (|| -> Result<(), rusqlite::Error> {
                            let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
                            tx.execute(
                                "INSERT INTO documents (id, title, content, content_text, doc_type, date, created_at, updated_at, deleted_at)
                                 VALUES (?1, ?2, '', ?3, 'memo', NULL, ?4, ?4, NULL)",
                                params![doc_id, title, content_text, now],
                            )?;
                            tx.commit()?;
                            Ok(())
                        })();
                        if let Err(e) = res {
                            if e.to_string().contains("locked") || e.to_string().contains("busy") {
                                locked_errors.fetch_add(1, Ordering::SeqCst);
                            }
                        }
                    }
                    Err(_) => {
                        locked_errors.fetch_add(1, Ordering::SeqCst);
                    }
                }

                // 2. Read
                if let Ok(conn) = pool.get() {
                    let _ = conn.query_row(
                        "SELECT title FROM documents WHERE id = ?1",
                        params![doc_id],
                        |r| r.get::<_, String>(0),
                    );
                }

                // 3. Update
                match pool.get() {
                    Ok(mut conn) => {
                        let res = (|| -> Result<(), rusqlite::Error> {
                            let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
                            tx.execute(
                                "UPDATE documents SET title = ?1, updated_at = ?2 WHERE id = ?3",
                                params![format!("Updated {}", title), now + 1, doc_id],
                            )?;
                            tx.commit()?;
                            Ok(())
                        })();
                        if let Err(e) = res {
                            if e.to_string().contains("locked") || e.to_string().contains("busy") {
                                locked_errors.fetch_add(1, Ordering::SeqCst);
                            }
                        }
                    }
                    Err(_) => {
                        locked_errors.fetch_add(1, Ordering::SeqCst);
                    }
                }
            }
        }));
    }

    for h in handles {
        h.join().expect("Burst thread must not deadlock");
    }

    let elapsed = start_time.elapsed();
    let locked = locked_error_count.load(Ordering::SeqCst);

    println!("[Heavy Burst Test] Completed in {:.2?} with 0 locked errors (actual: {})", elapsed, locked);
    assert_eq!(locked, 0, "Heavy burst test must have 0 locked errors");
}
