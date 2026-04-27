use std::collections::HashMap;
use std::path::{Path, PathBuf};
use serde::Deserialize;
use thiserror::Error;

#[cfg(target_os = "macos")]
pub mod au;

pub mod clap;
pub mod vst2;

#[derive(Debug, Error)]
pub enum ScanError {
    #[error("IO error at {path}: {source}")]
    Io { path: PathBuf, source: std::io::Error },
    #[error("Bad moduleinfo.json at {path}: {source}")]
    Json { path: PathBuf, source: serde_json::Error },
}

pub struct ScannedPlugin {
    pub name: String,
    pub vendor: Option<String>,
    pub version: Option<String>,
    pub category: Option<String>,
    pub class_id: Option<String>,
    pub path: PathBuf,
    pub format: PluginFormat,
    /// Unix timestamp (seconds) of the bundle's last modification time.
    /// `None` when the filesystem metadata is unavailable (e.g. AU with no resolved path).
    pub file_mtime: Option<i64>,
}

#[derive(Debug, Clone, Copy)]
pub enum PluginFormat {
    Vst3,
    Au,
    Vst2,
    Clap,
}

impl PluginFormat {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Vst3 => "VST3",
            Self::Au => "AU",
            Self::Vst2 => "VST2",
            Self::Clap => "CLAP",
        }
    }
}

// --- moduleinfo.json structures (VST3 SDK format) ---

#[derive(Deserialize)]
struct ModuleInfo {
    #[serde(rename = "Name")]
    name: Option<String>,
    #[serde(rename = "Version")]
    version: Option<String>,
    #[serde(rename = "Factory Info")]
    factory_info: Option<FactoryInfo>,
    #[serde(rename = "Classes")]
    classes: Option<Vec<ClassInfo>>,
}

#[derive(Deserialize)]
struct FactoryInfo {
    #[serde(rename = "Vendor")]
    vendor: Option<String>,
}

#[derive(Deserialize)]
struct ClassInfo {
    #[serde(rename = "CID")]
    cid: Option<String>,
    #[serde(rename = "Name")]
    name: Option<String>,
    #[serde(rename = "Vendor")]
    vendor: Option<String>,
    #[serde(rename = "Version")]
    version: Option<String>,
    // Sub Categories carries the useful taxonomy ("Fx|EQ"); Category is always "Audio Module Class"
    #[serde(rename = "Sub Categories")]
    sub_categories: Option<Vec<String>>,
}

// --- VST3 Public API ---

pub fn default_vst3_paths() -> Vec<PathBuf> {
    let mut paths = Vec::new();

    #[cfg(target_os = "windows")]
    {
        let base = std::env::var("COMMONPROGRAMFILES")
            .unwrap_or_else(|_| r"C:\Program Files\Common Files".to_string());
        paths.push(PathBuf::from(base).join("VST3"));
    }

    #[cfg(target_os = "macos")]
    {
        paths.push(PathBuf::from("/Library/Audio/Plug-Ins/VST3"));
        if let Ok(home) = std::env::var("HOME") {
            paths.push(PathBuf::from(home).join("Library/Audio/Plug-Ins/VST3"));
        }
    }

    paths
}

pub fn default_vst2_paths() -> Vec<PathBuf> {
    let mut paths = Vec::new();

    #[cfg(target_os = "macos")]
    {
        paths.push(PathBuf::from("/Library/Audio/Plug-Ins/VST"));
        if let Ok(home) = std::env::var("HOME") {
            paths.push(PathBuf::from(home).join("Library/Audio/Plug-Ins/VST"));
        }
    }

    #[cfg(target_os = "windows")]
    {
        let pf = std::env::var("PROGRAMFILES")
            .unwrap_or_else(|_| r"C:\Program Files".to_string());
        let pf86 = std::env::var("PROGRAMFILES(X86)")
            .unwrap_or_else(|_| r"C:\Program Files (x86)".to_string());
        paths.push(PathBuf::from(&pf).join("VSTPlugins"));
        paths.push(PathBuf::from(&pf86).join("VSTPlugins"));
        paths.push(PathBuf::from(&pf).join("Common Files").join("VST2"));
        paths.push(PathBuf::from(&pf86).join("Common Files").join("VST2"));
        paths.push(PathBuf::from(&pf).join("Steinberg").join("VstPlugins"));
        paths.push(PathBuf::from(&pf86).join("Steinberg").join("VstPlugins"));
    }

    paths
}

pub fn default_clap_paths() -> Vec<PathBuf> {
    let mut paths = Vec::new();

    #[cfg(target_os = "macos")]
    {
        paths.push(PathBuf::from("/Library/Audio/Plug-Ins/CLAP"));
        if let Ok(home) = std::env::var("HOME") {
            paths.push(PathBuf::from(home).join("Library/Audio/Plug-Ins/CLAP"));
        }
    }

    #[cfg(target_os = "windows")]
    {
        let pf = std::env::var("COMMONPROGRAMFILES")
            .unwrap_or_else(|_| r"C:\Program Files\Common Files".to_string());
        let pf86 = std::env::var("COMMONPROGRAMFILES(X86)")
            .unwrap_or_else(|_| r"C:\Program Files (x86)\Common Files".to_string());
        paths.push(PathBuf::from(&pf).join("CLAP"));
        paths.push(PathBuf::from(&pf86).join("CLAP"));
    }

    paths
}

/// Walk `paths` for CLAP plugins, probing each binary for metadata when a probe binary is available.
///
/// Returns `(plugins, skipped, errors)` where `skipped` is the number of bundles whose
/// mtime matched `known_mtimes` and were not re-parsed.
pub fn scan_clap(
    paths: &[PathBuf],
    probe: Option<&std::path::Path>,
    known_mtimes: &HashMap<String, i64>,
) -> (Vec<ScannedPlugin>, usize, Vec<ScanError>) {
    clap::scan_clap(paths, probe, known_mtimes)
}

/// Walk `paths` for `.clap` bundle entries without loading any plugin code.
pub fn walk_clap_bundles(paths: &[PathBuf]) -> Vec<PathBuf> {
    clap::walk_clap_bundles(paths)
}

/// Locate the `patchbay-clap-probe` binary next to the current executable.
pub fn find_clap_probe() -> Option<PathBuf> {
    clap::find_probe()
}

/// Walk `paths` for VST2 plugins, probing each binary for metadata when a probe binary is available.
///
/// Returns `(plugins, skipped, errors)` where `skipped` is the number of bundles whose
/// mtime matched `known_mtimes` and were not re-parsed.
pub fn scan_vst2(
    paths: &[PathBuf],
    probe: Option<&std::path::Path>,
    known_mtimes: &HashMap<String, i64>,
) -> (Vec<ScannedPlugin>, usize, Vec<ScanError>) {
    vst2::scan_vst2(paths, probe, known_mtimes)
}

/// Walk `paths` for `.vst` bundle directories (macOS) or `.dll` files (Windows)
/// without loading any plugin code. Useful for fast inventory.
pub fn walk_vst2_bundles(paths: &[PathBuf]) -> Vec<PathBuf> {
    vst2::walk_vst_bundles(paths)
}

/// Locate the `patchbay-vst2-probe` binary next to the current executable.
pub fn find_vst2_probe() -> Option<PathBuf> {
    vst2::find_probe()
}

/// Walk `paths`, find every `.vst3` bundle, parse metadata.
///
/// Bundles whose path is in `known_mtimes` with a matching mtime are skipped.
/// Returns `(plugins, skipped, errors)`.
pub fn scan_vst3(
    paths: &[PathBuf],
    known_mtimes: &HashMap<String, i64>,
) -> (Vec<ScannedPlugin>, usize, Vec<ScanError>) {
    let mut plugins = Vec::new();
    let mut errors = Vec::new();
    let mut skipped = 0;

    for root in paths {
        if !root.exists() {
            continue;
        }
        collect_bundles(root, known_mtimes, &mut plugins, &mut skipped, &mut errors);
    }

    (plugins, skipped, errors)
}

// --- VST3 Internals ---

fn collect_bundles(
    dir: &Path,
    known_mtimes: &HashMap<String, i64>,
    plugins: &mut Vec<ScannedPlugin>,
    skipped: &mut usize,
    errors: &mut Vec<ScanError>,
) {
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(e) => {
            errors.push(ScanError::Io { path: dir.to_path_buf(), source: e });
            return;
        }
    };

    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().map(|e| e.eq_ignore_ascii_case("vst3")).unwrap_or(false) {
            let current_mtime = bundle_mtime(&path);
            let path_key = path.to_string_lossy();
            if let (Some(&known), Some(current)) = (known_mtimes.get(path_key.as_ref()), current_mtime) {
                if known == current {
                    *skipped += 1;
                    continue;
                }
            }
            match parse_vst3_bundle(&path, current_mtime) {
                Ok(p) => plugins.push(p),
                Err(e) => errors.push(e),
            }
        } else if path.is_dir() {
            // Vendors often nest plugins one level deep (e.g. "Fabfilter/FabFilter Pro-Q 3.vst3")
            collect_bundles(&path, known_mtimes, plugins, skipped, errors);
        }
    }
}

fn parse_vst3_bundle(bundle: &Path, file_mtime: Option<i64>) -> Result<ScannedPlugin, ScanError> {
    let moduleinfo_path = bundle.join("Contents").join("moduleinfo.json");

    if moduleinfo_path.exists() {
        if let Ok(p) = parse_from_moduleinfo(bundle, &moduleinfo_path, file_mtime) {
            return Ok(p);
        }
        // malformed/empty moduleinfo.json — fall through to plist/filename
    }

    // No moduleinfo.json (true for ~99% of real-world Mac VST3 plugins).
    // Extract what we can from Info.plist; fall back to bundle filename as last resort.
    let plist_path = bundle.join("Contents").join("Info.plist");
    let (plist_name, version, vendor) = if plist_path.exists() {
        read_info_plist(&plist_path)
    } else {
        (None, None, None)
    };

    Ok(ScannedPlugin {
        name: plist_name.unwrap_or_else(|| file_stem(bundle)),
        vendor,
        version,
        category: None,
        class_id: None,
        path: bundle.to_path_buf(),
        format: PluginFormat::Vst3,
        file_mtime,
    })
}

fn parse_from_moduleinfo(
    bundle: &Path,
    path: &Path,
    file_mtime: Option<i64>,
) -> Result<ScannedPlugin, ScanError> {
    let data = std::fs::read_to_string(path)
        .map_err(|e| ScanError::Io { path: path.to_path_buf(), source: e })?;

    let info: ModuleInfo = serde_json::from_str(&data)
        .map_err(|e| ScanError::Json { path: path.to_path_buf(), source: e })?;

    let first_class = info.classes.as_deref().and_then(|c| c.first());

    let name = first_class
        .and_then(|c| c.name.clone())
        .or_else(|| info.name.clone())
        .unwrap_or_else(|| file_stem(bundle));

    let vendor = first_class
        .and_then(|c| c.vendor.clone())
        .or_else(|| info.factory_info.as_ref().and_then(|f| f.vendor.clone()));

    let version = first_class
        .and_then(|c| c.version.clone())
        .or_else(|| info.version.clone());

    let category = first_class.and_then(|c| {
        c.sub_categories
            .as_ref()
            .filter(|sc| !sc.is_empty())
            .map(|sc| sc.join("|"))
    });

    let class_id = first_class.and_then(|c| c.cid.clone());

    Ok(ScannedPlugin {
        name,
        vendor,
        version,
        category,
        class_id,
        path: bundle.to_path_buf(),
        format: PluginFormat::Vst3,
        file_mtime,
    })
}

// --- AU Public API ---

/// Walk the standard AU component directories and scan all `.component` bundles.
/// Reads `Info.plist` directly — one record per `AudioComponents` plist entry —
/// so multi-component bundles (e.g. stereo + mono variants) are fully expanded.
/// AU directory walks are fast; all AUs are always re-scanned (no mtime skip).
/// On non-macOS platforms returns empty.
#[cfg(target_os = "macos")]
pub fn scan_au() -> (Vec<ScannedPlugin>, usize, Vec<ScanError>) {
    let (plugins, errors) = au::scan_au_filesystem(&au::default_au_dirs());
    (plugins, 0, errors)
}

#[cfg(not(target_os = "macos"))]
pub fn scan_au() -> (Vec<ScannedPlugin>, usize, Vec<ScanError>) {
    (Vec::new(), 0, Vec::new())
}

// --- Shared Internals ---

/// Read the filesystem mtime for `path` as a Unix timestamp in seconds.
/// Returns `None` if the metadata is unavailable.
pub(crate) fn bundle_mtime(path: &Path) -> Option<i64> {
    std::fs::metadata(path)
        .ok()
        .and_then(|m| m.modified().ok())
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs() as i64)
}

/// Returns (name, version, vendor) from an Info.plist.
/// All fields are best-effort — parse failures return None rather than errors.
pub(crate) fn read_info_plist(path: &Path) -> (Option<String>, Option<String>, Option<String>) {
    let Ok(val) = plist::Value::from_file(path) else {
        return (None, None, None);
    };
    let Some(dict) = val.as_dictionary() else {
        return (None, None, None);
    };

    let name = dict.get("CFBundleName")
        .and_then(|v| v.as_string())
        .map(str::to_string);

    let version = dict.get("CFBundleShortVersionString")
        .and_then(|v| v.as_string())
        .map(str::to_string);

    // Primary: extract vendor from the copyright string in CFBundleGetInfoString
    // e.g. "4.3.1 (R0), Copyright © 2025 Native Instruments GmbH" → "Native Instruments GmbH"
    let vendor = dict.get("CFBundleGetInfoString")
        .and_then(|v| v.as_string())
        .and_then(extract_vendor_from_copyright)
        .or_else(|| {
            // Fallback: reverse-DNS bundle identifier
            // e.g. "com.plugin-alliance.vst3.amek" → "plugin alliance"
            dict.get("CFBundleIdentifier")
                .and_then(|v| v.as_string())
                .and_then(extract_vendor_from_bundle_id)
        });

    (name, version, vendor)
}

/// Finds the last standalone 4-digit year in a copyright string and returns everything after it.
/// "4.3.1, Copyright © 2025 Native Instruments GmbH" → Some("Native Instruments GmbH")
fn extract_vendor_from_copyright(s: &str) -> Option<String> {
    let bytes = s.as_bytes();
    let len = bytes.len();
    let mut last_year_end: Option<usize> = None;
    let mut i = 0;
    while i + 4 <= len {
        if bytes[i..i + 4].iter().all(|b| b.is_ascii_digit()) {
            let before_ok = i == 0 || !bytes[i - 1].is_ascii_digit();
            let after_ok = i + 4 >= len || !bytes[i + 4].is_ascii_digit();
            if before_ok && after_ok {
                last_year_end = Some(i + 4);
            }
        }
        i += 1;
    }
    let after = &s[last_year_end?..];
    let vendor = after.trim_matches(|c: char| !c.is_alphanumeric() && c != '-').trim();
    if vendor.is_empty() { None } else { Some(vendor.to_string()) }
}

/// Extracts the vendor segment from a reverse-DNS bundle identifier.
/// "com.plugin-alliance.vst3.amek" → Some("plugin alliance")
fn extract_vendor_from_bundle_id(id: &str) -> Option<String> {
    let mut parts = id.splitn(3, '.');
    let tld = parts.next()?;
    let vendor = parts.next()?;
    if matches!(tld, "com" | "net" | "org" | "io") && !vendor.is_empty() && vendor != "apple" {
        Some(vendor.replace('-', " "))
    } else {
        None
    }
}

pub(crate) fn file_stem(path: &Path) -> String {
    path.file_stem()
        .unwrap_or_default()
        .to_string_lossy()
        .into_owned()
}

// --- Tests ---

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    // --- VST3 helpers ---

    fn make_bundle(dir: &Path, name: &str, moduleinfo: Option<&str>, plist: Option<&str>) -> PathBuf {
        let bundle = dir.join(format!("{name}.vst3"));
        fs::create_dir_all(bundle.join("Contents")).unwrap();
        if let Some(json) = moduleinfo {
            fs::write(bundle.join("Contents").join("moduleinfo.json"), json).unwrap();
        }
        if let Some(xml) = plist {
            fs::write(bundle.join("Contents").join("Info.plist"), xml).unwrap();
        }
        bundle
    }

    const BATTERY_PLIST: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
    <key>CFBundleName</key><string>Battery 4</string>
    <key>CFBundleShortVersionString</key><string>4.3.1</string>
    <key>CFBundleGetInfoString</key><string>4.3.1 (R0), Copyright © 2025 Native Instruments GmbH</string>
    <key>CFBundleIdentifier</key><string>com.native-instruments.Battery4.vst3</string>
</dict></plist>"#;

    const AMEK_PLIST: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
    <key>CFBundleShortVersionString</key><string>1.1.1</string>
    <key>CFBundleIdentifier</key><string>com.plugin-alliance.vst3.amekmasteringcompressor</string>
</dict></plist>"#;

    #[test]
    fn parses_moduleinfo_json() {
        let tmp = TempDir::new().unwrap();
        make_bundle(tmp.path(), "TestPlugin", Some(r#"{
            "Name": "Test Plugin",
            "Version": "2.1.0",
            "Factory Info": { "Vendor": "Acme Audio" },
            "Classes": [{
                "CID": "AABBCCDD11223344AABBCCDD11223344",
                "Name": "Test Plugin",
                "Vendor": "Acme Audio",
                "Version": "2.1.0",
                "Sub Categories": ["Fx", "EQ"]
            }]
        }"#), None);

        let (plugins, skipped, errors) = scan_vst3(&[tmp.path().to_path_buf()], &HashMap::new());
        assert!(errors.is_empty(), "unexpected errors: {errors:?}");
        assert_eq!(plugins.len(), 1);
        assert_eq!(skipped, 0);
        let p = &plugins[0];
        assert_eq!(p.name, "Test Plugin");
        assert_eq!(p.vendor.as_deref(), Some("Acme Audio"));
        assert_eq!(p.version.as_deref(), Some("2.1.0"));
        assert_eq!(p.category.as_deref(), Some("Fx|EQ"));
        assert_eq!(p.class_id.as_deref(), Some("AABBCCDD11223344AABBCCDD11223344"));
    }

    #[test]
    fn reads_info_plist_with_copyright_vendor() {
        let tmp = TempDir::new().unwrap();
        make_bundle(tmp.path(), "Battery 4", None, Some(BATTERY_PLIST));

        let (plugins, _, errors) = scan_vst3(&[tmp.path().to_path_buf()], &HashMap::new());
        assert!(errors.is_empty());
        assert_eq!(plugins.len(), 1);
        let p = &plugins[0];
        assert_eq!(p.name, "Battery 4");
        assert_eq!(p.version.as_deref(), Some("4.3.1"));
        assert_eq!(p.vendor.as_deref(), Some("Native Instruments GmbH"));
    }

    #[test]
    fn falls_back_to_bundle_id_vendor_when_no_copyright() {
        let tmp = TempDir::new().unwrap();
        make_bundle(tmp.path(), "AMEK Mastering Compressor", None, Some(AMEK_PLIST));

        let (plugins, _, errors) = scan_vst3(&[tmp.path().to_path_buf()], &HashMap::new());
        assert!(errors.is_empty());
        assert_eq!(plugins.len(), 1);
        let p = &plugins[0];
        assert_eq!(p.version.as_deref(), Some("1.1.1"));
        assert_eq!(p.vendor.as_deref(), Some("plugin alliance"));
    }

    #[test]
    fn falls_back_to_bundle_name_when_no_metadata() {
        let tmp = TempDir::new().unwrap();
        make_bundle(tmp.path(), "Legacy Plugin", None, None);

        let (plugins, _, errors) = scan_vst3(&[tmp.path().to_path_buf()], &HashMap::new());
        assert!(errors.is_empty());
        assert_eq!(plugins.len(), 1);
        assert_eq!(plugins[0].name, "Legacy Plugin");
        assert!(plugins[0].vendor.is_none());
    }

    #[test]
    fn skips_nonexistent_paths() {
        let (plugins, skipped, errors) = scan_vst3(&[PathBuf::from("/does/not/exist/VST3")], &HashMap::new());
        assert!(plugins.is_empty());
        assert_eq!(skipped, 0);
        assert!(errors.is_empty());
    }

    #[test]
    fn falls_back_when_moduleinfo_json_is_empty() {
        let tmp = TempDir::new().unwrap();
        // Empty moduleinfo.json — should fall back to filename, not error
        make_bundle(tmp.path(), "Serum", Some(""), None);

        let (plugins, _, errors) = scan_vst3(&[tmp.path().to_path_buf()], &HashMap::new());
        assert!(errors.is_empty(), "empty moduleinfo.json should not produce an error");
        assert_eq!(plugins.len(), 1);
        assert_eq!(plugins[0].name, "Serum");
    }

    #[test]
    fn recurses_into_vendor_subdirectory() {
        let tmp = TempDir::new().unwrap();
        let vendor_dir = tmp.path().join("Fabfilter");
        fs::create_dir_all(&vendor_dir).unwrap();
        make_bundle(&vendor_dir, "Pro-Q 3", Some(r#"{
            "Name": "Pro-Q 3",
            "Factory Info": { "Vendor": "FabFilter" },
            "Classes": [{ "CID": "AABB", "Name": "Pro-Q 3", "Sub Categories": ["Fx", "EQ"] }]
        }"#), None);

        let (plugins, _, errors) = scan_vst3(&[tmp.path().to_path_buf()], &HashMap::new());
        assert!(errors.is_empty());
        assert_eq!(plugins.len(), 1);
        assert_eq!(plugins[0].name, "Pro-Q 3");
    }

    #[test]
    fn vst3_records_file_mtime() {
        let tmp = TempDir::new().unwrap();
        make_bundle(tmp.path(), "TestPlugin", None, None);

        let (plugins, _, _) = scan_vst3(&[tmp.path().to_path_buf()], &HashMap::new());
        assert_eq!(plugins.len(), 1);
        assert!(plugins[0].file_mtime.is_some(), "expected file_mtime to be set");
        assert!(plugins[0].file_mtime.unwrap() > 0);
    }

    #[test]
    fn skips_unchanged_vst3_by_mtime() {
        let tmp = TempDir::new().unwrap();
        let bundle = make_bundle(tmp.path(), "TestPlugin", None, None);

        // First scan: nothing known — plugin is returned
        let (plugins, skipped, _) = scan_vst3(&[tmp.path().to_path_buf()], &HashMap::new());
        assert_eq!(plugins.len(), 1);
        assert_eq!(skipped, 0);

        // Build known_mtimes with the current mtime
        let current_mtime = bundle_mtime(&bundle).unwrap();
        let mut known = HashMap::new();
        known.insert(bundle.to_string_lossy().into_owned(), current_mtime);

        let (plugins, skipped, _) = scan_vst3(&[tmp.path().to_path_buf()], &known);
        assert_eq!(plugins.len(), 0, "unchanged bundle should be skipped");
        assert_eq!(skipped, 1);
    }

    #[test]
    fn rescans_vst3_when_mtime_changes() {
        let tmp = TempDir::new().unwrap();
        let bundle = make_bundle(tmp.path(), "TestPlugin", None, None);

        // Stale mtime (epoch 0 is always in the past)
        let mut known = HashMap::new();
        known.insert(bundle.to_string_lossy().into_owned(), 0i64);

        let (plugins, skipped, _) = scan_vst3(&[tmp.path().to_path_buf()], &known);
        assert_eq!(plugins.len(), 1, "changed mtime should trigger re-parse");
        assert_eq!(skipped, 0);
    }

    #[test]
    fn extract_vendor_copyright_parses_correctly() {
        assert_eq!(
            extract_vendor_from_copyright("4.3.1 (R0), Copyright © 2025 Native Instruments GmbH"),
            Some("Native Instruments GmbH".to_string())
        );
        assert_eq!(
            extract_vendor_from_copyright("v1.0, Copyright 2023 Acme Audio Ltd."),
            Some("Acme Audio Ltd".to_string())
        );
        assert_eq!(extract_vendor_from_copyright("no year here"), None);
    }

    #[test]
    fn extract_vendor_bundle_id_parses_correctly() {
        assert_eq!(
            extract_vendor_from_bundle_id("com.plugin-alliance.vst3.amek"),
            Some("plugin alliance".to_string())
        );
        assert_eq!(
            extract_vendor_from_bundle_id("com.native-instruments.Battery4"),
            Some("native instruments".to_string())
        );
        assert_eq!(extract_vendor_from_bundle_id("com.apple.coreaudio"), None);
        assert_eq!(extract_vendor_from_bundle_id("notreversedns"), None);
    }
}
