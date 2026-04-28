use std::sync::Mutex;
use serde::Serialize;
use tauri::Manager;
use patchbay_core::db::{Database, DossierPlugin};
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
struct PluginDetailEntry {
    id: i64,
    name: String,
    vendor: Option<String>,
    category: Option<String>,
    instances: Vec<FormatInstance>,
    note: String,
}

#[derive(Serialize)]
struct ExportResult {
    plugin_count: usize,
    json_path: String,
    html_path: String,
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

/// Return full detail for a plugin by name: all format instances and user note.
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
    }))
}

/// Save or replace the user note for a plugin (identified by its primary row id).
#[tauri::command]
fn save_plugin_note(plugin_id: i64, body: String, state: tauri::State<'_, Mutex<Database>>) -> Result<(), String> {
    let db = state.lock().map_err(|e| e.to_string())?;
    db.upsert_plugin_note(plugin_id, &body).map_err(|e| e.to_string())
}

/// Export a full inventory of indexed plugins as JSON + HTML to ~/Documents/Patchbay/.
#[tauri::command]
fn export_library_dossier(
    app: tauri::AppHandle,
    state: tauri::State<'_, Mutex<Database>>,
) -> Result<ExportResult, String> {
    let did = device_id();
    let db = state.lock().map_err(|e| e.to_string())?;
    let plugins = db.export_dossier(&did).map_err(|e| e.to_string())?;
    let plugin_count = plugins.len();

    let docs_dir = app.path().document_dir().map_err(|e| e.to_string())?;
    let out_dir = docs_dir.join("Patchbay");
    std::fs::create_dir_all(&out_dir).map_err(|e| e.to_string())?;

    let now = chrono::Utc::now();
    let exported_at = now.format("%Y-%m-%dT%H:%M:%SZ").to_string();
    let date_slug = now.format("%Y-%m-%d").to_string();

    // JSON export
    let json_path = out_dir.join(format!("plugin-dossier-{}.json", date_slug));
    let json_export = serde_json::json!({
        "exported_at": exported_at,
        "device_id": did,
        "plugin_count": plugin_count,
        "plugins": plugins,
    });
    let json = serde_json::to_string_pretty(&json_export).map_err(|e| e.to_string())?;
    std::fs::write(&json_path, json).map_err(|e| e.to_string())?;

    // HTML export
    let html_path = out_dir.join(format!("plugin-dossier-{}.html", date_slug));
    let html = render_dossier_html(&did, &plugins, &exported_at);
    std::fs::write(&html_path, html).map_err(|e| e.to_string())?;

    Ok(ExportResult {
        plugin_count,
        json_path: json_path.to_string_lossy().into_owned(),
        html_path: html_path.to_string_lossy().into_owned(),
    })
}

/// Open a file or folder in the platform's default handler (browser for HTML,
/// Finder/Explorer for directories).
#[tauri::command]
fn open_path(path: String) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    {
        std::process::Command::new("open")
            .arg(&path)
            .spawn()
            .map_err(|e| e.to_string())?;
    }
    #[cfg(target_os = "windows")]
    {
        std::process::Command::new("cmd")
            .args(["/c", "start", "", &path])
            .spawn()
            .map_err(|e| e.to_string())?;
    }
    Ok(())
}

const DOSSIER_CSS: &str = r#"
*, *::before, *::after { box-sizing: border-box; margin: 0; padding: 0; }
:root {
  --bg: #ffffff; --surface: #f4f4f5; --border: #e4e4e7;
  --text: #18181b; --muted: #71717a;
  --font: system-ui, -apple-system, 'Segoe UI', sans-serif;
  --mono: 'SF Mono', 'Cascadia Code', 'Fira Code', monospace;
}
@media (prefers-color-scheme: dark) {
  :root { --bg: #09090b; --surface: #18181b; --border: #27272a; --text: #fafafa; --muted: #71717a; }
}
body { font-family: var(--font); font-size: 13px; background: var(--bg); color: var(--text); }
header { padding: 24px 32px 16px; border-bottom: 1px solid var(--border); }
h1 { font-size: 20px; font-weight: 700; letter-spacing: -0.02em; margin-bottom: 8px; }
.meta { display: flex; gap: 24px; color: var(--muted); font-size: 12px; flex-wrap: wrap; }
table { width: 100%; border-collapse: collapse; }
thead { position: sticky; top: 0; background: var(--bg); }
th { text-align: left; padding: 10px 12px 8px; color: var(--muted); font-weight: 500; font-size: 11px; text-transform: uppercase; letter-spacing: 0.05em; border-bottom: 1px solid var(--border); }
td { padding: 8px 12px; border-bottom: 1px solid var(--border); vertical-align: top; }
tr:hover td { background: var(--surface); }
tr:last-child td { border-bottom: none; }
.num { color: var(--muted); font-variant-numeric: tabular-nums; white-space: nowrap; }
.name { font-weight: 500; }
.vendor { color: var(--muted); white-space: nowrap; }
.badge { font-size: 11px; font-weight: 600; padding: 1px 5px; background: var(--surface); border-radius: 3px; display: inline-block; margin: 1px 2px 1px 0; }
.path-line { font-family: var(--mono); font-size: 11px; color: var(--muted); word-break: break-all; }
.ver { color: var(--muted); font-size: 11px; font-style: normal; }
.note-cell { font-style: italic; color: var(--muted); font-size: 12px; max-width: 220px; }
@media print { thead { position: static; } tr { page-break-inside: avoid; } }
"#;

fn he(s: &str) -> String {
    s.replace('&', "&amp;")
     .replace('<', "&lt;")
     .replace('>', "&gt;")
     .replace('"', "&quot;")
}

fn format_badge(fmt: &str) -> String {
    let color = match fmt {
        "VST3" => "#60a5fa",
        "AU"   => "#4ade80",
        "VST2" => "#facc15",
        "CLAP" => "#c084fc",
        _      => "#a1a1aa",
    };
    format!(r#"<span class="badge" style="color:{color}">{}</span>"#, he(fmt))
}

fn render_dossier_html(device_id: &str, plugins: &[DossierPlugin], exported_at: &str) -> String {
    let mut html = String::with_capacity(plugins.len() * 256 + 4096);

    html.push_str("<!DOCTYPE html>\n<html lang=\"en\">\n<head>\n");
    html.push_str("<meta charset=\"UTF-8\">\n");
    html.push_str("<meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\n");
    html.push_str("<title>Patchbay Plugin Dossier</title>\n");
    html.push_str("<style>");
    html.push_str(DOSSIER_CSS);
    html.push_str("</style>\n</head>\n<body>\n");

    html.push_str("<header>\n<h1>Plugin Dossier</h1>\n<div class=\"meta\">\n");
    html.push_str(&format!("<span>Exported {}</span>\n", he(exported_at)));
    html.push_str(&format!("<span>Device: {}</span>\n", he(device_id)));
    html.push_str(&format!("<span>{} plugins</span>\n", plugins.len()));
    html.push_str("</div>\n</header>\n");

    html.push_str("<table>\n<thead>\n<tr>\n");
    for th in &["#", "Name", "Vendor", "Formats", "Category", "Install Paths", "Note"] {
        html.push_str(&format!("<th>{th}</th>"));
    }
    html.push_str("</tr>\n</thead>\n<tbody>\n");

    for (i, p) in plugins.iter().enumerate() {
        html.push_str("<tr>\n");
        html.push_str(&format!("<td class=\"num\">{}</td>", i + 1));
        html.push_str(&format!("<td class=\"name\">{}</td>", he(&p.name)));
        html.push_str(&format!("<td class=\"vendor\">{}</td>", he(p.vendor.as_deref().unwrap_or("—"))));

        let badges: String = p.formats.iter().map(|f| format_badge(f)).collect::<Vec<_>>().join("");
        html.push_str(&format!("<td class=\"formats\">{badges}</td>"));

        html.push_str(&format!("<td>{}</td>", he(p.category.as_deref().unwrap_or(""))));

        html.push_str("<td>");
        for inst in &p.instances {
            let ver = inst.version.as_deref()
                .map(|v| format!(" <span class=\"ver\">v{}</span>", he(v)))
                .unwrap_or_default();
            html.push_str(&format!("<div class=\"path-line\">{}{ver}</div>", he(&inst.path)));
        }
        html.push_str("</td>");

        let note = p.note.as_deref().filter(|n| !n.is_empty()).map(|n| he(n)).unwrap_or_default();
        html.push_str(&format!("<td class=\"note-cell\">{note}</td>"));

        html.push_str("</tr>\n");
    }

    html.push_str("</tbody>\n</table>\n</body>\n</html>");
    html
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
            export_library_dossier,
            open_path,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
