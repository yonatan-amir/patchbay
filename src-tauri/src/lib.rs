use std::sync::Mutex;
use serde::Serialize;
use tauri::Manager;
use patchbay_core::db::Database;
use patchbay_core::{indexer, scanner};

#[derive(Serialize)]
struct ScanResult {
    plugins_found: usize,
    plugins_skipped: usize,
    errors: Vec<String>,
}

/// Scan all plugin formats (VST3, VST2, CLAP, AU) against default system paths
/// and persist results to SQLite. Bundles unchanged since the last scan are skipped.
#[tauri::command]
fn scan_plugins(state: tauri::State<'_, Mutex<Database>>) -> Result<ScanResult, String> {
    let device_id = std::env::var("COMPUTERNAME")
        .or_else(|_| std::env::var("HOSTNAME"))
        .unwrap_or_else(|_| "local".to_string());

    let db = state.lock().map_err(|e| e.to_string())?;
    let known_mtimes = db.get_known_mtimes(&device_id).map_err(|e| e.to_string())?;

    let vst2_probe = scanner::find_vst2_probe();
    let clap_probe = scanner::find_clap_probe();

    let mut all_plugins = Vec::new();
    let mut total_skipped = 0usize;
    let mut all_errors: Vec<String> = Vec::new();

    let (plugins, skipped, errors) = scanner::scan_vst3(&scanner::default_vst3_paths(), &known_mtimes);
    all_plugins.extend(plugins);
    total_skipped += skipped;
    all_errors.extend(errors.iter().map(|e| e.to_string()));

    let (plugins, skipped, errors) = scanner::scan_vst2(&scanner::default_vst2_paths(), vst2_probe.as_deref(), &known_mtimes);
    all_plugins.extend(plugins);
    total_skipped += skipped;
    all_errors.extend(errors.iter().map(|e| e.to_string()));

    let (plugins, skipped, errors) = scanner::scan_clap(&scanner::default_clap_paths(), clap_probe.as_deref(), &known_mtimes);
    all_plugins.extend(plugins);
    total_skipped += skipped;
    all_errors.extend(errors.iter().map(|e| e.to_string()));

    // scan_au returns empty on non-macOS; no cfg guard needed here
    let (plugins, skipped, errors) = scanner::scan_au();
    all_plugins.extend(plugins);
    total_skipped += skipped;
    all_errors.extend(errors.iter().map(|e| e.to_string()));

    let plugins_found = indexer::index_plugins(&db, all_plugins, &device_id)
        .map_err(|e| e.to_string())?;

    Ok(ScanResult {
        plugins_found,
        plugins_skipped: total_skipped,
        errors: all_errors,
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
