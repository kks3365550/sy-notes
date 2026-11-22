pub mod ai;
pub mod attachment;
pub mod backup;
pub mod db;
pub mod doc;
pub mod search;
#[cfg(test)]
pub mod security_tests;
#[cfg(test)]
pub mod verifier_logic_tests;
#[cfg(test)]
pub mod verifier_perf_tests;

use std::path::{Path, PathBuf};

// Learn more about Tauri commands at https://tauri.app/develop/calling-rust/
#[tauri::command]
fn greet(name: &str) -> String {
    format!("Hello, {}! You've been greeted from Rust!", name)
}

#[tauri::command]
fn get_or_create_daily(
    date: Option<String>,
    state: tauri::State<'_, db::DbPool>,
) -> Result<doc::Document, String> {
    let mut conn = state.get().map_err(|e| e.to_string())?;
    let target_date = date.as_deref().unwrap_or("");
    doc::get_or_create_daily(&mut conn, target_date).map_err(|e| e.to_string())
}

#[tauri::command]
fn get_doc(
    id: String,
    state: tauri::State<'_, db::DbPool>,
) -> Result<Option<doc::Document>, String> {
    let conn = state.get().map_err(|e| e.to_string())?;
    doc::get_doc(&conn, &id).map_err(|e| e.to_string())
}

/// Structured save error so the frontend can distinguish an optimistic-lock
/// conflict (`code = "conflict"`) from other failures.
#[derive(Debug, serde::Serialize)]
pub struct SaveError {
    pub code: &'static str,
    pub message: String,
    pub actual_updated_at: Option<i64>,
}

impl From<doc::DocError> for SaveError {
    fn from(err: doc::DocError) -> Self {
        let message = err.to_string();
        match err {
            doc::DocError::OptimisticLockConflict { actual, .. } => SaveError {
                code: "conflict",
                message,
                actual_updated_at: actual,
            },
            doc::DocError::NotFound(_) => SaveError {
                code: "not_found",
                message,
                actual_updated_at: None,
            },
            _ => SaveError {
                code: "error",
                message,
                actual_updated_at: None,
            },
        }
    }
}

#[tauri::command]
fn create_or_update_doc(
    doc: doc::Document,
    state: tauri::State<'_, db::DbPool>,
) -> Result<doc::Document, SaveError> {
    let conn = state.get().map_err(|e| SaveError {
        code: "error",
        message: e.to_string(),
        actual_updated_at: None,
    })?;
    doc::create_or_update_doc(&conn, &doc).map_err(SaveError::from)
}

#[tauri::command]
fn delete_doc(
    id: String,
    state: tauri::State<'_, db::DbPool>,
) -> Result<(), String> {
    let conn = state.get().map_err(|e| e.to_string())?;
    doc::delete_doc(&conn, &id).map_err(|e| e.to_string())
}

#[tauri::command]
fn restore_doc(
    id: String,
    state: tauri::State<'_, db::DbPool>,
) -> Result<(), String> {
    let conn = state.get().map_err(|e| e.to_string())?;
    doc::restore_doc(&conn, &id).map_err(|e| e.to_string())
}

#[tauri::command]
fn purge_trash(
    older_than_days: Option<i64>,
    state: tauri::State<'_, db::DbPool>,
) -> Result<usize, String> {
    let conn = state.get().map_err(|e| e.to_string())?;
    doc::purge_trash(&conn, older_than_days.unwrap_or(30)).map_err(|e| e.to_string())
}

#[tauri::command]
fn list_docs(
    include_deleted: Option<bool>,
    state: tauri::State<'_, db::DbPool>,
) -> Result<Vec<doc::Document>, String> {
    let conn = state.get().map_err(|e| e.to_string())?;
    doc::list_docs(&conn, include_deleted.unwrap_or(false)).map_err(|e| e.to_string())
}

#[tauri::command]
fn list_trash(
    state: tauri::State<'_, db::DbPool>,
) -> Result<Vec<doc::Document>, String> {
    let conn = state.get().map_err(|e| e.to_string())?;
    doc::list_trash(&conn).map_err(|e| e.to_string())
}

#[tauri::command]
fn search_docs(
    query: String,
    limit: Option<i64>,
    state: tauri::State<'_, db::DbPool>,
) -> Result<Vec<doc::Document>, String> {
    let conn = state.get().map_err(|e| e.to_string())?;
    search::search_docs(&conn, &query, limit.unwrap_or(20)).map_err(|e| e.to_string())
}

#[tauri::command]
fn backup_db(
    backup_dir: Option<String>,
    state: tauri::State<'_, db::DbPool>,
) -> Result<backup::BackupResult, String> {
    let conn = state.get().map_err(|e| e.to_string())?;
    let dir = match backup_dir {
        Some(d) => PathBuf::from(d),
        None => {
            let db_path = db::get_default_db_path();
            let parent = db_path.parent().unwrap_or(Path::new("."));
            parent.join("backups")
        }
    };
    backup::backup_db(&conn, &dir).map_err(|e| e.to_string())
}

#[tauri::command]
fn export_all(
    export_path: String,
    state: tauri::State<'_, db::DbPool>,
) -> Result<backup::ExportReport, String> {
    let conn = state.get().map_err(|e| e.to_string())?;
    backup::export_all(&conn, &export_path).map_err(|e| e.to_string())
}

#[tauri::command]
fn get_storage_info() -> db::StorageInfo {
    db::get_storage_info()
}

#[tauri::command]
fn save_attachment(
    file_name: String,
    data_base64: String,
) -> Result<String, String> {
    let db_path = db::get_default_db_path();
    let base_data_dir = db_path.parent().unwrap_or(Path::new("data"));
    attachment::save_attachment_file(base_data_dir, &file_name, &data_base64)
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let db_path = db::get_default_db_path();
    let pool = db::create_pool(&db_path).expect("Failed to initialize SQLite connection pool");
    db::init_pool(&pool).expect("Failed to run database migrations");

    let ai_state = ai::AiState::new();

    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .manage(pool)
        .manage(ai_state)
        .invoke_handler(tauri::generate_handler![
            greet,
            get_or_create_daily,
            get_doc,
            create_or_update_doc,
            delete_doc,
            restore_doc,
            purge_trash,
            list_docs,
            list_trash,
            search_docs,
            backup_db,
            export_all,
            get_storage_info,
            save_attachment,
            ai::generate_ai_stream,
            ai::cancel_ai_stream,
            ai::get_ai_config,
            ai::set_ai_config,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

#[cfg(test)]
mod chaos_tests;

#[cfg(test)]
mod tests {
    use rusqlite::Connection;

    #[test]
    fn test_fts5_trigram() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("
            CREATE VIRTUAL TABLE test_fts USING fts5(title, content, tokenize='trigram');
            INSERT INTO test_fts (title, content) VALUES ('안녕하세요', '삼영 스마트팩토리 테스트');
        ").unwrap();

        let mut stmt = conn.prepare("SELECT title FROM test_fts WHERE test_fts MATCH '스마트'").unwrap();
        let mut rows = stmt.query([]).unwrap();
        let title: String = rows.next().unwrap().unwrap().get(0).unwrap();
        assert_eq!(title, "안녕하세요");
    }
}
