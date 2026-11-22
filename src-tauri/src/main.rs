// Prevents additional console window on Windows in release, DO NOT REMOVE!!
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::path::PathBuf;

fn setup_portable_env() {
    // Portable mode: Pin WEBVIEW2_USER_DATA_FOLDER to exe_dir/data/EBWebView
    let portable_data = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|dir| dir.join("data")));

    let data_dir: Option<PathBuf> = match portable_data {
        Some(dir) => {
            let webview_dir = dir.join("EBWebView");
            if std::fs::create_dir_all(&webview_dir).is_ok() {
                Some(webview_dir)
            } else {
                fallback_appdata_dir()
            }
        }
        None => fallback_appdata_dir(),
    };

    if let Some(path) = data_dir {
        // Safe here because this is called at the very beginning of main before spawning any threads
        unsafe {
            std::env::set_var("WEBVIEW2_USER_DATA_FOLDER", path);
        }
    }
}

fn fallback_appdata_dir() -> Option<PathBuf> {
    std::env::var("APPDATA").ok().map(|appdata| {
        let dir = PathBuf::from(appdata).join("sy-notes").join("data").join("EBWebView");
        let _ = std::fs::create_dir_all(&dir);
        dir
    })
}

fn main() {
    setup_portable_env();
    sy_notes_lib::run();
}
