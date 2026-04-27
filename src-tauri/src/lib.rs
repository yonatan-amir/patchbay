use std::sync::Mutex;
use serde::Serialize;
use tauri::Manager;
use patchbay_core::db::Database;
use patchbay_core::{indexer, scanner};

#[derive(Serialize)]
struct PluginEntry {
    name: String,
    vendor: Option<String>,
    format: String,
    category: Option<String>,
}

#[derive(Serialize)]
struct ScanResult {
    plugins_found: usize,
    plugins_skipped: usize,
    errors: Vec<String>,
}

#[derive(Serialize)]
struct FormatInstance {
    format: String,
    path: String,
    version: Option<String>,
}

#[derive(Serialize)]
struct ManualEntry {
    id: i64,
    source: String,
    path_or_url: String,
}

#[derive(Serialize)]
struct PluginDetailEntry {
    id: i64,
    name: String,
    vendor: Option<String>,
    category: Option<String>,
    instances: Vec<FormatInstance>,
    note: String,
    manuals: Vec<ManualEntry>,
}

fn device_id() -> String {
    std::env::var("COMPUTERNAME")
        .or_else(|_| std::env::var("HOSTNAME"))
        .unwrap_or_else(|_| "local".to_string())
}

/// Scan all plugin formats (VST3, VST2, CLAP, AU) against default system paths
/// and persist results to SQLite. Bundles unchanged since the last scan are skipped.
#[tauri::command]
fn scan_plugins(state: tauri::State<'_, Mutex<Database>>) -> Result<ScanResult, String> {
    let did = device_id();
    let db = state.lock().map_err(|e| e.to_string())?;
    let known_mtimes = db.get_known_mtimes(&did).map_err(|e| e.to_string())?;

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

    let plugins_found = indexer::index_plugins(&db, all_plugins, &did)
        .map_err(|e| e.to_string())?;

    Ok(ScanResult {
        plugins_found,
        plugins_skipped: total_skipped,
        errors: all_errors,
    })
}

/// Return all indexed plugins for the current device.
#[tauri::command]
fn list_plugins(state: tauri::State<'_, Mutex<Database>>) -> Result<Vec<PluginEntry>, String> {
    let did = device_id();
    let db = state.lock().map_err(|e| e.to_string())?;
    let rows = db.list_plugins(&did).map_err(|e| e.to_string())?;
    Ok(rows.into_iter().map(|r| PluginEntry {
        name: r.name,
        vendor: r.vendor,
        format: r.format,
        category: r.category,
    }).collect())
}

/// Return full detail for a plugin by name: all format instances, user note, and manuals.
#[tauri::command]
fn get_plugin_detail(name: String, state: tauri::State<'_, Mutex<Database>>) -> Result<Option<PluginDetailEntry>, String> {
    let did = device_id();
    let db = state.lock().map_err(|e| e.to_string())?;
    let detail = db.get_plugin_detail(&name, &did).map_err(|e| e.to_string())?;
    Ok(detail.map(|d| PluginDetailEntry {
        id: d.id,
        name: d.name,
        vendor: d.vendor,
        category: d.category,
        instances: d.instances.into_iter().map(|i| FormatInstance {
            format: i.format,
            path: i.path,
            version: i.version,
        }).collect(),
        note: d.note,
        manuals: d.manuals.into_iter().map(|m| ManualEntry {
            id: m.id,
            source: m.source,
            path_or_url: m.path_or_url,
        }).collect(),
    }))
}

/// Save or replace the user note for a plugin (identified by its primary row id).
#[tauri::command]
fn save_plugin_note(plugin_id: i64, body: String, state: tauri::State<'_, Mutex<Database>>) -> Result<(), String> {
    let db = state.lock().map_err(|e| e.to_string())?;
    db.upsert_plugin_note(plugin_id, &body).map_err(|e| e.to_string())
}

/// Attach a manual URL or local path to a plugin.
#[tauri::command]
fn save_plugin_manual(plugin_id: i64, source: String, path_or_url: String, state: tauri::State<'_, Mutex<Database>>) -> Result<i64, String> {
    let db = state.lock().map_err(|e| e.to_string())?;
    db.save_plugin_manual(plugin_id, &source, &path_or_url).map_err(|e| e.to_string())
}

/// Remove a manual entry by id.
#[tauri::command]
fn delete_plugin_manual(manual_id: i64, state: tauri::State<'_, Mutex<Database>>) -> Result<(), String> {
    let db = state.lock().map_err(|e| e.to_string())?;
    db.delete_plugin_manual(manual_id).map_err(|e| e.to_string())
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
        .invoke_handler(tauri::generate_handler![
            scan_plugins,
            list_plugins,
            get_plugin_detail,
            save_plugin_note,
            save_plugin_manual,
            delete_plugin_manual,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
