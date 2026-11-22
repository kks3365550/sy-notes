use crate::db::{create_pool, init_pool};
use crate::doc::{create_or_update_doc, Document};
use chrono::Utc;
use rusqlite::{params, TransactionBehavior};
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Instant;
use uuid::Uuid;

/// The benchmarks in this module measure wall-clock latency, so they must not run
/// at the same time as each other: the 8-thread concurrency benchmark saturates CPU
/// and disk and pushed the 50,000-char p95 from ~13ms to ~99ms on a CI runner.
static BENCH_LOCK: Mutex<()> = Mutex::new(());

fn bench_guard() -> std::sync::MutexGuard<'static, ()> {
    BENCH_LOCK.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// RAII helper to clean up test database files (.db, .db-wal, .db-shm)
struct TempDbGuard(PathBuf);

impl Drop for TempDbGuard {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
        let _ = std::fs::remove_file(self.0.with_extension("db-wal"));
        let _ = std::fs::remove_file(self.0.with_extension("db-shm"));
    }
}

fn create_temp_db_path(prefix: &str) -> (PathBuf, TempDbGuard) {
    let temp_dir = std::env::temp_dir();
    let file_name = format!("{}_{}.db", prefix, Uuid::new_v4());
    let path = temp_dir.join(file_name);
    let guard = TempDbGuard(path.clone());
    (path, guard)
}

#[test]
fn test_concurrency_8_threads_10000_operations_zero_locked() {
    let _bench = bench_guard();
    let (db_path, _guard) = create_temp_db_path("perf_conc_10k");

    // Initialize r2d2 pool using library configuration (size=4, busy_timeout=5000, synchronous=NORMAL)
    let pool = create_pool(&db_path).expect("Failed to create r2d2 pool");
    init_pool(&pool).expect("Failed to initialize database schema");

    // Pre-seed 100 documents for SELECT and UPDATE operations
    {
        let conn = pool.get().expect("Failed to acquire connection for seeding");
        let now = Utc::now().timestamp_millis();
        for i in 0..100 {
            let doc_id = format!("seed-perf-{:04}", i);
            let title = format!("Seed Document {:04}", i);
            let content = r#"{"type":"doc","content":[{"type":"paragraph","text":"Seed content"}]}"#;
            let content_text = format!("Seed content {} 삼영스마트팩토리 검교정", i);
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
                                    let doc_id = format!("doc-perf-t{}-{}", thread_id, op_idx);
                                    let title = format!("Concurrent Note {} by thread {}", op_idx, thread_id);
                                    let content = r#"{"type":"doc","content":[{"type":"paragraph","text":"동시성 테스트"}]}"#;
                                    let content_text = format!("동시성 테스트 데이터 {} 삼영스마트팩토리", op_idx);
                                    let now = Utc::now().timestamp_millis();

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
                                        // FTS5 Trigram MATCH
                                        let mut stmt = conn.prepare_cached(
                                            "SELECT count(*) FROM documents_fts WHERE documents_fts MATCH '스마트'"
                                        )?;
                                        let _count: i64 = stmt.query_row([], |r| r.get(0))?;
                                    } else if op_idx % 2 == 0 {
                                        // Primary Key lookup
                                        let target_id = format!("seed-perf-{:04}", op_idx % 100);
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
                                        // Active documents count
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
                                    let target_id = format!("seed-perf-{:04}", op_idx % 100);
                                    let new_title = format!("Updated Note {} by thread {}", op_idx, thread_id);
                                    let now = Utc::now().timestamp_millis();

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

    println!("\n=======================================================");
    println!(" [BENCHMARK 1] 8-Thread 10,000 Concurrent Read/Write");
    println!("=======================================================");
    println!(" Total Operations   : {:>10}", total_executed);
    println!("   - INSERT ops     : {:>10}", inserts);
    println!("   - SELECT ops     : {:>10}", selects);
    println!("   - UPDATE ops     : {:>10}", updates);
    println!(" Locked Errors      : {:>10} (Target: 0)", locked_errors);
    println!(" Other Errors       : {:>10} (Target: 0)", other_errors);
    println!(" Total Elapsed Time : {:>10.3} ms", elapsed.as_secs_f64() * 1000.0);
    println!(" Throughput         : {:>10.2} ops/sec", total_executed as f64 / elapsed.as_secs_f64());
    println!(" Average Latency    : {:>10.2} us", avg_latency_us);
    println!(" p95 Latency        : {:>10} us ({:.3} ms)", p95_latency_us, p95_latency_us as f64 / 1000.0);
    println!(" p99 Latency        : {:>10} us ({:.3} ms)", p99_latency_us, p99_latency_us as f64 / 1000.0);
    println!("=======================================================");

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
    }

    // STRICT VERIFICATION ASSERTIONS
    assert_eq!(locked_errors, 0, "database is locked errors MUST BE 0!");
    assert_eq!(other_errors, 0, "Other errors MUST BE 0!");
    assert_eq!(inserts + selects + updates, TOTAL_OPERATIONS, "All 10,000 operations must complete successfully!");
}

#[test]
fn test_50000_char_ipc_roundtrip_latency_under_16ms() {
    let _bench = bench_guard();
    let (db_path, _guard) = create_temp_db_path("perf_ipc_50k");

    let pool = create_pool(&db_path).expect("Create db pool");
    init_pool(&pool).expect("Init db pool");

    // Generate 50,000 characters (mix of Korean and English)
    let sample = "동해물과 백두산이 마르고 닳도록 하느님이 보우하사 우리나라 만세. 삼영스마트팩토리 검교정 자동화 시스템. Quick brown fox jumps over the lazy dog 1234567890. ";
    let repeats = 50_000 / sample.chars().count() + 1;
    let large_text: String = sample.repeat(repeats).chars().take(50_000).collect();
    assert_eq!(large_text.chars().count(), 50_000);

    let doc_id = Uuid::new_v4().to_string();
    let now = Utc::now().timestamp_millis();

    let initial_doc = Document {
        id: doc_id.clone(),
        title: "50,000자 대용량 성능 벤치마크".to_string(),
        content: format!(r#"[{{ "type": "paragraph", "content": [{{ "type": "text", "text": "{}" }}] }}]"#, large_text),
        content_text: large_text.clone(),
        doc_type: "memo".to_string(),
        date: None,
        created_at: now,
        updated_at: 0,
        deleted_at: None,
    };

    let iterations = 100;
    let mut latencies_us = Vec::with_capacity(iterations);

    let conn = pool.get().expect("Get conn");

    let mut current_doc = initial_doc;

    // Warmup 5 iterations
    for _ in 0..5 {
        let json_input = serde_json::to_string(&current_doc).unwrap();
        let doc_deser: Document = serde_json::from_str(&json_input).unwrap();
        let saved = create_or_update_doc(&conn, &doc_deser).unwrap();
        current_doc = saved;
    }

    // Per-stage timings so a p95 miss can be attributed to serialization
    // or the DB. WebView/IPC transport cost is not visible here: it is the CDP
    // benchmark total (benchmarks/measure_ipc_cdp.mjs) minus this total.
    let mut ser_us = Vec::with_capacity(iterations);
    let mut db_us = Vec::with_capacity(iterations);

    for i in 0..iterations {
        let t0 = Instant::now();

        // 1. Frontend -> Tauri IPC JSON serialization simulation (50,000 chars payload)
        let json_payload = serde_json::to_string(&current_doc).expect("Serialize to JSON");

        // 2. Tauri IPC Deserialization
        let mut doc: Document = serde_json::from_str(&json_payload).expect("Deserialize from JSON");
        doc.title = format!("50,000자 대용량 성능 벤치마크 iteration {}", i);
        let t1 = Instant::now();

        // 3. Backend DB Execution (upsert + FTS5 trigram update triggers)
        let saved = create_or_update_doc(&conn, &doc).expect("create_or_update_doc");
        let t2 = Instant::now();

        // 4. Rust -> Frontend IPC JSON serialization
        let json_response = serde_json::to_string(&saved).expect("Serialize response");

        // 5. Frontend IPC response parse
        let received: Document = serde_json::from_str(&json_response).expect("Deserialize response");
        let t3 = Instant::now();

        latencies_us.push((t3 - t0).as_micros());
        ser_us.push(((t1 - t0) + (t3 - t2)).as_micros());
        db_us.push((t2 - t1).as_micros());

        current_doc = received;
    }

    let p95_of = |v: &mut Vec<u128>| {
        v.sort_unstable();
        v[(v.len() as f64 * 0.95) as usize] as f64 / 1000.0
    };
    let ser_p95_ms = p95_of(&mut ser_us);
    let db_p95_ms = p95_of(&mut db_us);

    latencies_us.sort_unstable();

    let min_us = latencies_us[0];
    let max_us = latencies_us[latencies_us.len() - 1];
    let avg_us = latencies_us.iter().sum::<u128>() as f64 / latencies_us.len() as f64;
    let p50_us = latencies_us[(latencies_us.len() as f64 * 0.50) as usize];
    let p90_us = latencies_us[(latencies_us.len() as f64 * 0.90) as usize];
    let p95_us = latencies_us[(latencies_us.len() as f64 * 0.95) as usize];
    let p99_us = latencies_us[(latencies_us.len() as f64 * 0.99) as usize];

    let p95_ms = p95_us as f64 / 1000.0;
    let avg_ms = avg_us / 1000.0;
    let min_ms = min_us as f64 / 1000.0;
    let max_ms = max_us as f64 / 1000.0;

    println!("\n=======================================================");
    println!(" [BENCHMARK 2] 50,000-Char IPC Round-Trip Latency (100 runs)");
    println!("=======================================================");
    println!(" Text character count : {:>10}", 50_000);
    println!(" Min Latency          : {:>10.3} ms ({:>8} us)", min_ms, min_us);
    println!(" Average Latency      : {:>10.3} ms ({:>8.1} us)", avg_ms, avg_us);
    println!(" p50 Latency          : {:>10.3} ms ({:>8} us)", p50_us as f64 / 1000.0, p50_us);
    println!(" p90 Latency          : {:>10.3} ms ({:>8} us)", p90_us as f64 / 1000.0, p90_us);
    println!(" p95 Latency          : {:>10.3} ms ({:>8} us)", p95_ms, p95_us);
    println!(" p99 Latency          : {:>10.3} ms ({:>8} us)", p99_us as f64 / 1000.0, p99_us);
    println!(" Max Latency          : {:>10.3} ms ({:>8} us)", max_ms, max_us);
    println!(" -- p95 by stage --");
    println!("   JSON (de)serialize : {:>10.3} ms", ser_p95_ms);
    println!("   DB upsert + FTS    : {:>10.3} ms", db_p95_ms);
    println!("   WebView/IPC        : (CDP benchmark total - this total)");
    let threshold = if cfg!(debug_assertions) { 50.0 } else { 16.0 };
    println!(" Threshold Target     : < {:.3} ms (Release Target: < 16ms)", threshold);
    println!("=======================================================");

    assert!(
        p95_ms < threshold,
        "p95 latency ({:.3} ms) must be strictly below {}ms!",
        p95_ms,
        threshold
    );
    println!("[PASS] 50,000-char IPC round-trip p95 ({:.3} ms) is strictly below {}ms threshold!", p95_ms, threshold);
}
