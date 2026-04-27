use std::path::{Path, PathBuf};
use serde::Deserialize;
use thiserror::Error;

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
}

#[derive(Debug, Clone, Copy)]
pub enum PluginFormat {
    Vst3,
    Au,
}

impl PluginFormat {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Vst3 => "VST3",
            Self::Au => "AU",
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

/// Walk `paths`, find every `.vst3` bundle, parse metadata.
/// Returns (successful results, non-fatal errors) so callers see partial progress.
pub fn scan_vst3(paths: &[PathBuf]) -> (Vec<ScannedPlugin>, Vec<ScanError>) {
    let mut plugins = Vec::new();
    let mut errors = Vec::new();

    for root in paths {
        if !root.exists() {
            continue;
        }
        collect_bundles(root, &mut plugins, &mut errors);
    }

    (plugins, errors)
}

// --- VST3 Internals ---

fn collect_bundles(
    dir: &Path,
    plugins: &mut Vec<ScannedPlugin>,
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
            match parse_vst3_bundle(&path) {
                Ok(p) => plugins.push(p),
                Err(e) => errors.push(e),
            }
        } else if path.is_dir() {
            // Vendors often nest plugins one level deep (e.g. "Fabfilter/FabFilter Pro-Q 3.vst3")
            collect_bundles(&path, plugins, errors);
        }
    }
}

fn parse_vst3_bundle(bundle: &Path) -> Result<ScannedPlugin, ScanError> {
    let moduleinfo_path = bundle.join("Contents").join("moduleinfo.json");

    if moduleinfo_path.exists() {
        return parse_from_moduleinfo(bundle, &moduleinfo_path);
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
    })
}

fn parse_from_moduleinfo(bundle: &Path, path: &Path) -> Result<ScannedPlugin, ScanError> {
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
    })
}

// --- AU Public API ---

pub fn default_au_paths() -> Vec<PathBuf> {
    let mut paths = Vec::new();
    #[cfg(target_os = "macos")]
    {
        paths.push(PathBuf::from("/Library/Audio/Plug-Ins/Components"));
        if let Ok(home) = std::env::var("HOME") {
            paths.push(PathBuf::from(home).join("Library/Audio/Plug-Ins/Components"));
        }
    }
    paths
}

/// Walk `paths`, find every `.component` bundle, parse metadata.
/// Returns (successful results, non-fatal errors) so callers see partial progress.
pub fn scan_au(paths: &[PathBuf]) -> (Vec<ScannedPlugin>, Vec<ScanError>) {
    let mut plugins = Vec::new();
    let mut errors = Vec::new();
    for root in paths {
        if !root.exists() {
            continue;
        }
        collect_au_bundles(root, &mut plugins, &mut errors);
    }
    (plugins, errors)
}

// --- AU Internals ---

fn collect_au_bundles(
    dir: &Path,
    plugins: &mut Vec<ScannedPlugin>,
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
        if path.extension().map(|e| e.eq_ignore_ascii_case("component")).unwrap_or(false) {
            match parse_au_bundle(&path) {
                Ok(p) => plugins.push(p),
                Err(e) => errors.push(e),
            }
        }
    }
}

fn parse_au_bundle(bundle: &Path) -> Result<ScannedPlugin, ScanError> {
    let plist_path = bundle.join("Contents").join("Info.plist");
    if !plist_path.exists() {
        return Ok(ScannedPlugin {
            name: file_stem(bundle),
            vendor: None,
            version: None,
            category: None,
            class_id: None,
            path: bundle.to_path_buf(),
            format: PluginFormat::Au,
        });
    }
    if let Some((name, vendor, version, category, class_id)) = read_audio_components(&plist_path) {
        return Ok(ScannedPlugin {
            name,
            vendor,
            version,
            category,
            class_id,
            path: bundle.to_path_buf(),
            format: PluginFormat::Au,
        });
    }
    // Fallback for V1 AUs without AudioComponents array
    let (plist_name, version, vendor) = read_info_plist(&plist_path);
    Ok(ScannedPlugin {
        name: plist_name.unwrap_or_else(|| file_stem(bundle)),
        vendor,
        version,
        category: None,
        class_id: None,
        path: bundle.to_path_buf(),
        format: PluginFormat::Au,
    })
}

/// Extracts plugin info from the `AudioComponents` array in an Info.plist.
/// Returns (name, vendor, version, category, class_id) on success.
fn read_audio_components(
    plist_path: &Path,
) -> Option<(String, Option<String>, Option<String>, Option<String>, Option<String>)> {
    let val = plist::Value::from_file(plist_path).ok()?;
    let dict = val.as_dictionary()?;
    let comp = dict
        .get("AudioComponents")
        .and_then(|v| v.as_array())
        .and_then(|a| a.first())
        .and_then(|v| v.as_dictionary())?;

    let full_name = comp.get("name").and_then(|v| v.as_string()).map(str::to_string)?;
    let (name, vendor) = split_au_name(&full_name);

    let type_code = au_ostype(comp, "type");
    let subtype   = au_ostype(comp, "subtype");
    let mfr       = au_ostype(comp, "manufacturer");

    let category = type_code.as_deref().and_then(au_type_to_category).map(str::to_string);
    let class_id = match (&type_code, &subtype, &mfr) {
        (Some(t), Some(s), Some(m)) => Some(format!("{t}/{s}/{m}")),
        _ => None,
    };

    let version = comp
        .get("version")
        .and_then(|v| match v {
            plist::Value::Integer(i) => i.as_unsigned().map(decode_au_version),
            plist::Value::String(s) => Some(s.clone()),
            _ => None,
        })
        .or_else(|| {
            dict.get("CFBundleShortVersionString")
                .and_then(|v| v.as_string())
                .map(str::to_string)
        });

    Some((name, vendor, version, category, class_id))
}

/// Reads an OSType field, handling both string ("aufx") and integer (0x61756678) forms.
fn au_ostype(dict: &plist::Dictionary, key: &str) -> Option<String> {
    match dict.get(key)? {
        plist::Value::String(s) => Some(s.clone()),
        plist::Value::Integer(i) => {
            let v = i.as_unsigned()? as u32;
            let bytes = v.to_be_bytes();
            if bytes.iter().all(|b| b.is_ascii_graphic()) {
                Some(String::from_utf8_lossy(&bytes).into_owned())
            } else {
                Some(format!("{v:#010x}"))
            }
        }
        _ => None,
    }
}

/// Splits "Vendor: Plugin Name" into ("Plugin Name", Some("Vendor")).
/// Returns the full string as name when no ": " separator is present.
fn split_au_name(full_name: &str) -> (String, Option<String>) {
    if let Some((vendor, name)) = full_name.split_once(": ") {
        (name.to_string(), Some(vendor.to_string()))
    } else {
        (full_name.to_string(), None)
    }
}

/// Maps a 4-char AU type code to a human-readable category.
fn au_type_to_category(type_code: &str) -> Option<&'static str> {
    match type_code {
        "aufx" => Some("Fx"),
        "aumu" => Some("Instrument"),
        "aumi" => Some("MIDI Processor"),
        "auou" => Some("Output"),
        "aumf" => Some("Mixer"),
        "aupn" => Some("Panner"),
        "augn" => Some("Generator"),
        _ => None,
    }
}

/// Decodes an AudioComponents version integer (major<<16 | minor<<8 | patch).
fn decode_au_version(v: u64) -> String {
    let major = (v >> 16) & 0xFF;
    let minor = (v >> 8) & 0xFF;
    let patch = v & 0xFF;
    if patch == 0 {
        format!("{major}.{minor}")
    } else {
        format!("{major}.{minor}.{patch}")
    }
}

// --- Shared Internals ---

/// Returns (name, version, vendor) from an Info.plist.
/// All fields are best-effort — parse failures return None rather than errors.
fn read_info_plist(path: &Path) -> (Option<String>, Option<String>, Option<String>) {
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

fn file_stem(path: &Path) -> String {
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

        let (plugins, errors) = scan_vst3(&[tmp.path().to_path_buf()]);
        assert!(errors.is_empty(), "unexpected errors: {errors:?}");
        assert_eq!(plugins.len(), 1);
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

        let (plugins, errors) = scan_vst3(&[tmp.path().to_path_buf()]);
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

        let (plugins, errors) = scan_vst3(&[tmp.path().to_path_buf()]);
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

        let (plugins, errors) = scan_vst3(&[tmp.path().to_path_buf()]);
        assert!(errors.is_empty());
        assert_eq!(plugins.len(), 1);
        assert_eq!(plugins[0].name, "Legacy Plugin");
        assert!(plugins[0].vendor.is_none());
    }

    #[test]
    fn skips_nonexistent_paths() {
        let (plugins, errors) = scan_vst3(&[PathBuf::from("/does/not/exist/VST3")]);
        assert!(plugins.is_empty());
        assert!(errors.is_empty());
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

        let (plugins, errors) = scan_vst3(&[tmp.path().to_path_buf()]);
        assert!(errors.is_empty());
        assert_eq!(plugins.len(), 1);
        assert_eq!(plugins[0].name, "Pro-Q 3");
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

    // --- AU helpers ---

    fn make_au_bundle(dir: &Path, name: &str, plist: Option<&str>) -> PathBuf {
        let bundle = dir.join(format!("{name}.component"));
        fs::create_dir_all(bundle.join("Contents")).unwrap();
        if let Some(xml) = plist {
            fs::write(bundle.join("Contents").join("Info.plist"), xml).unwrap();
        }
        bundle
    }

    // 262913 = 0x040301 → major=4, minor=3, patch=1 → "4.3.1"
    const BATTERY_AU_PLIST: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
    <key>CFBundleShortVersionString</key><string>4.3.1</string>
    <key>AudioComponents</key><array><dict>
        <key>name</key><string>Native Instruments: Battery 4</string>
        <key>type</key><string>aumu</string>
        <key>subtype</key><string>Bat4</string>
        <key>manufacturer</key><string>NInv</string>
        <key>version</key><integer>262913</integer>
    </dict></array>
</dict></plist>"#;

    // 196608 = 0x030000 → major=3, minor=0, patch=0 → "3.0"
    const PRO_Q_AU_PLIST: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
    <key>AudioComponents</key><array><dict>
        <key>name</key><string>FabFilter: Pro-Q 3</string>
        <key>type</key><string>aufx</string>
        <key>subtype</key><string>FPQ3</string>
        <key>manufacturer</key><string>FabF</string>
        <key>version</key><integer>196608</integer>
    </dict></array>
</dict></plist>"#;

    #[test]
    fn parses_au_instrument_bundle() {
        let tmp = TempDir::new().unwrap();
        make_au_bundle(tmp.path(), "Battery 4", Some(BATTERY_AU_PLIST));

        let (plugins, errors) = scan_au(&[tmp.path().to_path_buf()]);
        assert!(errors.is_empty(), "unexpected errors: {errors:?}");
        assert_eq!(plugins.len(), 1);
        let p = &plugins[0];
        assert_eq!(p.name, "Battery 4");
        assert_eq!(p.vendor.as_deref(), Some("Native Instruments"));
        assert_eq!(p.version.as_deref(), Some("4.3.1"));
        assert_eq!(p.category.as_deref(), Some("Instrument"));
        assert_eq!(p.class_id.as_deref(), Some("aumu/Bat4/NInv"));
        assert!(matches!(p.format, PluginFormat::Au));
    }

    #[test]
    fn parses_au_fx_bundle() {
        let tmp = TempDir::new().unwrap();
        make_au_bundle(tmp.path(), "Pro-Q 3", Some(PRO_Q_AU_PLIST));

        let (plugins, errors) = scan_au(&[tmp.path().to_path_buf()]);
        assert!(errors.is_empty());
        assert_eq!(plugins.len(), 1);
        let p = &plugins[0];
        assert_eq!(p.name, "Pro-Q 3");
        assert_eq!(p.vendor.as_deref(), Some("FabFilter"));
        assert_eq!(p.version.as_deref(), Some("3.0"));
        assert_eq!(p.category.as_deref(), Some("Fx"));
        assert_eq!(p.class_id.as_deref(), Some("aufx/FPQ3/FabF"));
    }

    #[test]
    fn falls_back_to_bundle_name_for_au_without_plist() {
        let tmp = TempDir::new().unwrap();
        make_au_bundle(tmp.path(), "Legacy AU", None);

        let (plugins, errors) = scan_au(&[tmp.path().to_path_buf()]);
        assert!(errors.is_empty());
        assert_eq!(plugins.len(), 1);
        assert_eq!(plugins[0].name, "Legacy AU");
        assert!(plugins[0].vendor.is_none());
    }

    #[test]
    fn split_au_name_with_colon_separator() {
        assert_eq!(
            split_au_name("Native Instruments: Battery 4"),
            ("Battery 4".to_string(), Some("Native Instruments".to_string()))
        );
        assert_eq!(
            split_au_name("UnprefixedPlugin"),
            ("UnprefixedPlugin".to_string(), None)
        );
    }

    #[test]
    fn decode_au_version_formats_correctly() {
        assert_eq!(decode_au_version(262913), "4.3.1"); // 0x040301
        assert_eq!(decode_au_version(196608), "3.0");   // 0x030000
        assert_eq!(decode_au_version(131072), "2.0");   // 0x020000
    }

    #[test]
    fn au_type_codes_map_to_categories() {
        assert_eq!(au_type_to_category("aufx"), Some("Fx"));
        assert_eq!(au_type_to_category("aumu"), Some("Instrument"));
        assert_eq!(au_type_to_category("aumi"), Some("MIDI Processor"));
        assert_eq!(au_type_to_category("xxxx"), None);
    }
}
