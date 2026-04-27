//! CLAP plugin scanner.
//!
//! CLAP bundles expose a `clap_entry` symbol through which a factory returns
//! one or more plugin descriptors. Querying that entry requires loading the
//! binary, so we delegate to the `patchbay-clap-probe` subprocess exactly as
//! the VST2 scanner delegates to `patchbay-vst2-probe`.
//!
//! One `.clap` bundle can export multiple plugins, so `scan_clap` may return
//! more `ScannedPlugin`s than bundles found.
//!
//! When the probe is absent, crashes, or times out, we fall back to bundle
//! filename and Info.plist (macOS) for a single best-effort record.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::mpsc;
use std::thread;
use std::time::Duration;
use serde::Deserialize;

use super::{PluginFormat, ScanError, ScannedPlugin};

// -- public API ---------------------------------------------------------------

/// Scan `paths` for CLAP plugins.
///
/// Bundles whose path appears in `known_mtimes` with a matching mtime are skipped
/// without spawning the probe subprocess.
///
/// Returns `(plugins, skipped, errors)`.
pub fn scan_clap(
    paths: &[PathBuf],
    probe: Option<&Path>,
    known_mtimes: &HashMap<String, i64>,
) -> (Vec<ScannedPlugin>, usize, Vec<ScanError>) {
    let mut plugins = Vec::new();
    let mut skipped = 0;

    for bundle in walk_clap_bundles(paths) {
        let current_mtime = super::bundle_mtime(&bundle);
        let path_key = bundle.to_string_lossy();
        if let (Some(&known), Some(current)) = (known_mtimes.get(path_key.as_ref()), current_mtime) {
            if known == current {
                skipped += 1;
                continue;
            }
        }
        let descriptors = probe.and_then(|p| run_probe(p, &bundle));
        extend_plugins(&bundle, descriptors, current_mtime, &mut plugins);
    }

    (plugins, skipped, Vec::new())
}

/// Locate the `patchbay-clap-probe` binary next to the current executable.
/// Returns `None` if the binary is not present (probe step is skipped gracefully).
pub fn find_probe() -> Option<PathBuf> {
    let mut exe = std::env::current_exe().ok()?;
    exe.set_file_name(probe_name());
    exe.exists().then_some(exe)
}

// -- filesystem walk ----------------------------------------------------------

/// Walk `paths` for CLAP bundle/file entries. Never executes plugin code.
pub fn walk_clap_bundles(paths: &[PathBuf]) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for root in paths {
        if root.exists() {
            collect(root, &mut out);
        }
    }
    out
}

fn collect(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for entry in entries.flatten() {
        let path = entry.path();
        if is_clap_entry(&path) {
            out.push(path);
        } else if path.is_dir() {
            collect(&path, out);
        }
    }
}

fn is_clap_entry(path: &Path) -> bool {
    let has_ext = path
        .extension()
        .map(|e| e.eq_ignore_ascii_case("clap"))
        .unwrap_or(false);
    if !has_ext {
        return false;
    }
    #[cfg(target_os = "macos")]
    { path.is_dir() }
    #[cfg(target_os = "windows")]
    { path.is_file() }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    { path.is_file() }
}

// -- probe subprocess ---------------------------------------------------------

#[derive(Deserialize)]
struct ProbeDescriptor {
    id: Option<String>,
    name: Option<String>,
    vendor: Option<String>,
    version: Option<String>,
    #[serde(default)]
    features: Vec<String>,
}

/// Spawn the probe with an 8-second timeout. Returns `None` on crash / timeout / parse failure.
///
/// The spawned thread is leaked on timeout — the probe process keeps running
/// but the OS reclaims it. Acceptable for Phase 1.
fn run_probe(probe: &Path, bundle: &Path) -> Option<Vec<ProbeDescriptor>> {
    let (tx, rx) = mpsc::channel();
    let probe = probe.to_owned();
    let bundle = bundle.to_owned();

    thread::spawn(move || {
        let result = std::process::Command::new(&probe)
            .arg(&bundle)
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .output();
        let _ = tx.send(result);
    });

    let output = rx.recv_timeout(Duration::from_secs(8)).ok()?.ok()?;
    if !output.status.success() {
        return None;
    }
    serde_json::from_slice::<Vec<ProbeDescriptor>>(&output.stdout).ok()
}

// -- plugin assembly ----------------------------------------------------------

/// Push one `ScannedPlugin` per probe descriptor. Falls back to a single
/// filename/plist record when descriptors are absent or empty.
fn extend_plugins(
    bundle: &Path,
    probe: Option<Vec<ProbeDescriptor>>,
    file_mtime: Option<i64>,
    out: &mut Vec<ScannedPlugin>,
) {
    match probe {
        Some(descs) if !descs.is_empty() => {
            for desc in descs {
                out.push(build_from_probe(bundle, desc, file_mtime));
            }
        }
        _ => out.push(build_fallback(bundle, file_mtime)),
    }
}

fn build_from_probe(bundle: &Path, desc: ProbeDescriptor, file_mtime: Option<i64>) -> ScannedPlugin {
    let (plist_name, plist_version, plist_vendor) = plist_metadata(bundle);

    let name = desc
        .name
        .filter(|s| !s.is_empty())
        .or(plist_name)
        .unwrap_or_else(|| super::file_stem(bundle));

    let vendor = desc.vendor.filter(|s| !s.is_empty()).or(plist_vendor);
    let version = desc.version.filter(|s| !s.is_empty()).or(plist_version);

    let category = if desc.features.is_empty() {
        None
    } else {
        Some(desc.features.join("|"))
    };

    let class_id = desc.id.filter(|s| !s.is_empty());

    ScannedPlugin {
        name,
        vendor,
        version,
        category,
        class_id,
        path: bundle.to_path_buf(),
        format: PluginFormat::Clap,
        file_mtime,
    }
}

fn build_fallback(bundle: &Path, file_mtime: Option<i64>) -> ScannedPlugin {
    let (name, version, vendor) = plist_metadata(bundle);
    ScannedPlugin {
        name: name.unwrap_or_else(|| super::file_stem(bundle)),
        vendor,
        version,
        category: None,
        class_id: None,
        path: bundle.to_path_buf(),
        format: PluginFormat::Clap,
        file_mtime,
    }
}

fn plist_metadata(bundle: &Path) -> (Option<String>, Option<String>, Option<String>) {
    let plist_path = bundle.join("Contents").join("Info.plist");
    if plist_path.exists() {
        super::read_info_plist(&plist_path)
    } else {
        (None, None, None)
    }
}

#[cfg(target_os = "macos")]
fn probe_name() -> &'static str { "patchbay-clap-probe" }

#[cfg(target_os = "windows")]
fn probe_name() -> &'static str { "patchbay-clap-probe.exe" }

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
fn probe_name() -> &'static str { "patchbay-clap-probe" }

// -- tests --------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(target_os = "macos")]
    use crate::scanner::bundle_mtime;
    use std::fs;
    use tempfile::TempDir;

    #[cfg(target_os = "macos")]
    fn make_clap_bundle(dir: &Path, name: &str, plist: Option<&str>) -> PathBuf {
        let bundle = dir.join(format!("{name}.clap"));
        fs::create_dir_all(bundle.join("Contents").join("MacOS")).unwrap();
        if let Some(xml) = plist {
            fs::write(bundle.join("Contents").join("Info.plist"), xml).unwrap();
        }
        bundle
    }

    #[cfg(target_os = "macos")]
    const PRO_Q_PLIST: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
    <key>CFBundleName</key><string>FabFilter Pro-Q 3</string>
    <key>CFBundleShortVersionString</key><string>3.22</string>
    <key>CFBundleGetInfoString</key><string>3.22, Copyright 2023 FabFilter</string>
</dict></plist>"#;

    #[cfg(target_os = "macos")]
    #[test]
    fn walk_finds_clap_bundles() {
        let tmp = TempDir::new().unwrap();
        make_clap_bundle(tmp.path(), "Pro-Q 3", None);
        make_clap_bundle(tmp.path(), "Serum 2", None);

        let vendor_dir = tmp.path().join("FabFilter");
        fs::create_dir_all(&vendor_dir).unwrap();
        make_clap_bundle(&vendor_dir, "Pro-L 2", None);

        let found = walk_clap_bundles(&[tmp.path().to_path_buf()]);
        assert_eq!(found.len(), 3);
    }

    #[test]
    fn skips_nonexistent_paths() {
        let found = walk_clap_bundles(&[PathBuf::from("/does/not/exist/CLAP")]);
        assert!(found.is_empty());
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn fallback_reads_plist() {
        let tmp = TempDir::new().unwrap();
        let bundle = make_clap_bundle(tmp.path(), "FabFilter Pro-Q 3", Some(PRO_Q_PLIST));

        let plugin = build_fallback(&bundle, None);
        assert_eq!(plugin.name, "FabFilter Pro-Q 3");
        assert_eq!(plugin.version.as_deref(), Some("3.22"));
        assert_eq!(plugin.vendor.as_deref(), Some("FabFilter"));
        assert!(matches!(plugin.format, PluginFormat::Clap));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn fallback_uses_filename_when_no_plist() {
        let tmp = TempDir::new().unwrap();
        let bundle = make_clap_bundle(tmp.path(), "Serum 2", None);

        let plugin = build_fallback(&bundle, None);
        assert_eq!(plugin.name, "Serum 2");
        assert!(plugin.vendor.is_none());
        assert!(plugin.version.is_none());
    }

    #[test]
    fn probe_descriptor_builds_correctly() {
        let tmp = TempDir::new().unwrap();
        let bundle = tmp.path().join("Pro-Q 3.clap");
        fs::create_dir_all(&bundle).unwrap();

        let desc = ProbeDescriptor {
            id: Some("com.fabfilter.pro-q-3".to_string()),
            name: Some("FabFilter Pro-Q 3".to_string()),
            vendor: Some("FabFilter".to_string()),
            version: Some("3.22.0".to_string()),
            features: vec!["audio-effect".to_string(), "equalizer".to_string()],
        };

        let plugin = build_from_probe(&bundle, desc, None);
        assert_eq!(plugin.name, "FabFilter Pro-Q 3");
        assert_eq!(plugin.vendor.as_deref(), Some("FabFilter"));
        assert_eq!(plugin.version.as_deref(), Some("3.22.0"));
        assert_eq!(plugin.category.as_deref(), Some("audio-effect|equalizer"));
        assert_eq!(plugin.class_id.as_deref(), Some("com.fabfilter.pro-q-3"));
        assert!(matches!(plugin.format, PluginFormat::Clap));
    }

    #[test]
    fn probe_empty_name_falls_back_to_filename() {
        let tmp = TempDir::new().unwrap();
        let bundle = tmp.path().join("Serum 2.clap");
        fs::create_dir_all(&bundle).unwrap();

        let desc = ProbeDescriptor {
            id: None,
            name: None,
            vendor: None,
            version: None,
            features: vec![],
        };

        let plugin = build_from_probe(&bundle, desc, None);
        assert_eq!(plugin.name, "Serum 2");
        assert!(plugin.vendor.is_none());
        assert!(plugin.category.is_none());
        assert!(plugin.class_id.is_none());
    }

    #[test]
    fn empty_probe_result_falls_back_to_single_record() {
        let tmp = TempDir::new().unwrap();
        let bundle = tmp.path().join("Serum 2.clap");
        fs::create_dir_all(&bundle).unwrap();

        let mut plugins = Vec::new();
        extend_plugins(&bundle, Some(vec![]), None, &mut plugins);
        assert_eq!(plugins.len(), 1);
        assert_eq!(plugins[0].name, "Serum 2");
    }

    #[test]
    fn none_probe_falls_back_to_single_record() {
        let tmp = TempDir::new().unwrap();
        let bundle = tmp.path().join("Massive X.clap");
        fs::create_dir_all(&bundle).unwrap();

        let mut plugins = Vec::new();
        extend_plugins(&bundle, None, None, &mut plugins);
        assert_eq!(plugins.len(), 1);
        assert_eq!(plugins[0].name, "Massive X");
    }

    #[test]
    fn multiple_descriptors_yield_multiple_plugins() {
        let tmp = TempDir::new().unwrap();
        let bundle = tmp.path().join("BundleWithTwo.clap");
        fs::create_dir_all(&bundle).unwrap();

        let descs = vec![
            ProbeDescriptor {
                id: Some("com.vendor.plugin-a".to_string()),
                name: Some("Plugin A".to_string()),
                vendor: Some("Vendor".to_string()),
                version: Some("1.0".to_string()),
                features: vec!["audio-effect".to_string()],
            },
            ProbeDescriptor {
                id: Some("com.vendor.plugin-b".to_string()),
                name: Some("Plugin B".to_string()),
                vendor: Some("Vendor".to_string()),
                version: Some("1.0".to_string()),
                features: vec!["instrument".to_string(), "synthesizer".to_string()],
            },
        ];

        let mut plugins = Vec::new();
        extend_plugins(&bundle, Some(descs), None, &mut plugins);
        assert_eq!(plugins.len(), 2);
        assert_eq!(plugins[0].name, "Plugin A");
        assert_eq!(plugins[0].category.as_deref(), Some("audio-effect"));
        assert_eq!(plugins[1].name, "Plugin B");
        assert_eq!(plugins[1].category.as_deref(), Some("instrument|synthesizer"));
    }

    #[test]
    fn file_mtime_is_threaded_through_probe() {
        let tmp = TempDir::new().unwrap();
        let bundle = tmp.path().join("TestPlugin.clap");
        fs::create_dir_all(&bundle).unwrap();

        let desc = ProbeDescriptor {
            id: None,
            name: Some("Test".to_string()),
            vendor: None,
            version: None,
            features: vec![],
        };
        let plugin = build_from_probe(&bundle, desc, Some(9999));
        assert_eq!(plugin.file_mtime, Some(9999));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn skips_unchanged_clap_by_mtime() {
        let tmp = TempDir::new().unwrap();
        let bundle = make_clap_bundle(tmp.path(), "Pro-Q 3", None);

        let (plugins, skipped, _) = scan_clap(&[tmp.path().to_path_buf()], None, &HashMap::new());
        assert_eq!(plugins.len(), 1);
        assert_eq!(skipped, 0);

        let current = bundle_mtime(&bundle).unwrap();
        let mut known = HashMap::new();
        known.insert(bundle.to_string_lossy().into_owned(), current);
        let (plugins, skipped, _) = scan_clap(&[tmp.path().to_path_buf()], None, &known);
        assert_eq!(plugins.len(), 0);
        assert_eq!(skipped, 1);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn rescans_clap_when_mtime_changes() {
        let tmp = TempDir::new().unwrap();
        let bundle = make_clap_bundle(tmp.path(), "Pro-Q 3", None);

        let mut known = HashMap::new();
        known.insert(bundle.to_string_lossy().into_owned(), 0i64);
        let (plugins, skipped, _) = scan_clap(&[tmp.path().to_path_buf()], None, &known);
        assert_eq!(plugins.len(), 1);
        assert_eq!(skipped, 0);
    }
}
