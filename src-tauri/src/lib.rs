use std::sync::Mutex;
use serde::Serialize;
use tauri::Manager;
use patchbay_core::db::Database;
use patchbay_core::{indexer, scanner};

#[derive(Serialize)]
struct ScanResult {
    plugins_found: usize,
    errors: Vec<String>,
}

/// Run the VST3 scanner against the default system paths and persist results to SQLite.
#[tauri::command]
fn scan_plugins(state: tauri::State<'_, Mutex<Database>>) -> Result<ScanResult, String> {
    let paths = scanner::default_vst3_paths();
    let (plugins, errors) = scanner::scan_vst3(&paths);

    let device_id = std::env::var("COMPUTERNAME")
        .or_else(|_| std::env::var("HOSTNAME"))
        .unwrap_or_else(|_| "local".to_string());

    let db = state.lock().map_err(|e| e.to_string())?;
    let plugins_found = indexer::index_plugins(&db, plugins, &device_id)
        .map_err(|e| e.to_string())?;

    Ok(ScanResult {
        plugins_found,
        errors: errors.iter().map(|e| e.to_string()).collect(),
    })
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .setup(|app| {
            let data_dir = app.path().app_data_dir()?;
            std::fs::create_dir_all(&data_dir)?;
            let db = Database::open(&data_dir.join("patchbay.db"))?;
            app.manage(Mutex::new(db));
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![scan_plugins])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
