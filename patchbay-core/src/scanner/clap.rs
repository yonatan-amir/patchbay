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
/// `probe` is the path to the `patchbay-clap-probe` binary. Pass `None` to
/// skip the probe step (metadata will come from Info.plist / filename only).
pub fn scan_clap(paths: &[PathBuf], probe: Option<&Path>) -> (Vec<ScannedPlugin>, Vec<ScanError>) {
    let mut plugins = Vec::new();

    for bundle in walk_clap_bundles(paths) {
        let descriptors = probe.and_then(|p| run_probe(p, &bundle));
        extend_plugins(&bundle, descriptors, &mut plugins);
    }

    (plugins, Vec::new())
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
fn extend_plugins(bundle: &Path, probe: Option<Vec<ProbeDescriptor>>, out: &mut Vec<ScannedPlugin>) {
    match probe {
        Some(descs) if !descs.is_empty() => {
            for desc in descs {
                out.push(build_from_probe(bundle, desc));
            }
        }
        _ => out.push(build_fallback(bundle)),
    }
}

fn build_from_probe(bundle: &Path, desc: ProbeDescriptor) -> ScannedPlugin {
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
    }
}

fn build_fallback(bundle: &Path) -> ScannedPlugin {
    let (name, version, vendor) = plist_metadata(bundle);
    ScannedPlugin {
        name: name.unwrap_or_else(|| super::file_stem(bundle)),
        vendor,
        version,
        category: None,
        class_id: None,
        path: bundle.to_path_buf(),
        format: PluginFormat::Clap,
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

        let plugin = build_fallback(&bundle);
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

        let plugin = build_fallback(&bundle);
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

        let plugin = build_from_probe(&bundle, desc);
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

        let plugin = build_from_probe(&bundle, desc);
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
        extend_plugins(&bundle, Some(vec![]), &mut plugins);
        assert_eq!(plugins.len(), 1);
        assert_eq!(plugins[0].name, "Serum 2");
    }

    #[test]
    fn none_probe_falls_back_to_single_record() {
        let tmp = TempDir::new().unwrap();
        let bundle = tmp.path().join("Massive X.clap");
        fs::create_dir_all(&bundle).unwrap();

        let mut plugins = Vec::new();
        extend_plugins(&bundle, None, &mut plugins);
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
        extend_plugins(&bundle, Some(descs), &mut plugins);
        assert_eq!(plugins.len(), 2);
        assert_eq!(plugins[0].name, "Plugin A");
        assert_eq!(plugins[0].category.as_deref(), Some("audio-effect"));
        assert_eq!(plugins[1].name, "Plugin B");
        assert_eq!(plugins[1].category.as_deref(), Some("instrument|synthesizer"));
    }
}
