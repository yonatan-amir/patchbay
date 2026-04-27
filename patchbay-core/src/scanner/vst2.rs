//! VST2 scanner.
//!
//! Two-phase approach:
//!  1. Filesystem walk -- finds every `.vst` bundle (macOS) or `.dll` (Windows)
//!     without executing any plugin code.
//!  2. Subprocess probe -- spawns `patchbay-vst2-probe` per bundle to extract
//!     name/vendor/category by calling `VSTPlugMain`. If the probe crashes,
//!     times out, or the probe binary is absent, the scanner falls back to
//!     Info.plist + filename metadata.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::mpsc;
use std::thread;
use std::time::Duration;
use serde::Deserialize;

use super::{PluginFormat, ScanError, ScannedPlugin};

// -- public API ---------------------------------------------------------------

/// Scan `paths` for VST2 plugins.
///
/// Bundles whose path appears in `known_mtimes` with a matching mtime are skipped
/// without spawning the probe subprocess.
///
/// Returns `(plugins, skipped, errors)`.
pub fn scan_vst2(
    paths: &[PathBuf],
    probe: Option<&Path>,
    known_mtimes: &HashMap<String, i64>,
) -> (Vec<ScannedPlugin>, usize, Vec<ScanError>) {
    let mut plugins = Vec::new();
    let mut skipped = 0;

    for bundle in walk_vst_bundles(paths) {
        let current_mtime = super::bundle_mtime(&bundle);
        let path_key = bundle.to_string_lossy();
        if let (Some(&known), Some(current)) = (known_mtimes.get(path_key.as_ref()), current_mtime) {
            if known == current {
                skipped += 1;
                continue;
            }
        }
        let probe_out = probe.and_then(|p| run_probe(p, &bundle));
        plugins.push(build_plugin(&bundle, probe_out, current_mtime));
    }

    (plugins, skipped, Vec::new())
}

/// Locate the `patchbay-vst2-probe` binary next to the current executable.
/// Returns `None` if the binary is not present (probe step is skipped gracefully).
pub fn find_probe() -> Option<PathBuf> {
    let mut exe = std::env::current_exe().ok()?;
    exe.set_file_name(probe_name());
    exe.exists().then_some(exe)
}

// -- filesystem walk ----------------------------------------------------------

/// Walk `paths` for VST2 bundle/file entries. Never executes plugin code.
pub fn walk_vst_bundles(paths: &[PathBuf]) -> Vec<PathBuf> {
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
        if is_vst2_entry(&path) {
            out.push(path);
        } else if path.is_dir() {
            collect(&path, out);
        }
    }
}

#[cfg(target_os = "macos")]
fn is_vst2_entry(path: &Path) -> bool {
    path.extension()
        .map(|e| e.eq_ignore_ascii_case("vst"))
        .unwrap_or(false)
        && path.is_dir()
}

#[cfg(target_os = "windows")]
fn is_vst2_entry(path: &Path) -> bool {
    path.extension()
        .map(|e| e.eq_ignore_ascii_case("dll"))
        .unwrap_or(false)
        && path.is_file()
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
fn is_vst2_entry(_path: &Path) -> bool {
    false
}

// -- probe subprocess ---------------------------------------------------------

#[derive(Deserialize)]
struct ProbeOutput {
    name: Option<String>,
    vendor: Option<String>,
    category: Option<i32>,
}

/// Spawn the probe with an 8-second timeout. Returns `None` on crash / timeout / parse failure.
///
/// The spawned thread is leaked on timeout -- the probe process keeps running
/// but the OS reclaims it. Acceptable for Phase 1.
fn run_probe(probe: &Path, bundle: &Path) -> Option<ProbeOutput> {
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
    serde_json::from_slice::<ProbeOutput>(&output.stdout).ok()
}

// -- plugin assembly ----------------------------------------------------------

fn build_plugin(bundle: &Path, probe: Option<ProbeOutput>, file_mtime: Option<i64>) -> ScannedPlugin {
    let plist_path = bundle.join("Contents").join("Info.plist");
    let (plist_name, plist_version, plist_vendor) = if plist_path.exists() {
        super::read_info_plist(&plist_path)
    } else {
        (None, None, None)
    };

    let stem = super::file_stem(bundle);

    let name = probe
        .as_ref()
        .and_then(|p| p.name.as_deref())
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .or(plist_name)
        .unwrap_or(stem);

    let vendor = probe
        .as_ref()
        .and_then(|p| p.vendor.as_deref())
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .or(plist_vendor);

    let category = probe
        .as_ref()
        .and_then(|p| p.category)
        .map(|c| vst_category_str(c).to_string());

    ScannedPlugin {
        name,
        vendor,
        version: plist_version,
        category,
        class_id: None,
        path: bundle.to_path_buf(),
        format: PluginFormat::Vst2,
        file_mtime,
    }
}

fn vst_category_str(cat: i32) -> &'static str {
    match cat {
        1 => "Fx",
        2 => "Synth",
        3 => "Analysis",
        4 => "Mastering",
        5 => "Spacializer",
        6 => "RoomFx",
        7 => "SurroundFx",
        8 => "Restoration",
        9 => "OfflineProcess",
        10 => "Shell",
        11 => "Generator",
        _ => "Unknown",
    }
}

#[cfg(target_os = "macos")]
fn probe_name() -> &'static str { "patchbay-vst2-probe" }

#[cfg(target_os = "windows")]
fn probe_name() -> &'static str { "patchbay-vst2-probe.exe" }

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
fn probe_name() -> &'static str { "patchbay-vst2-probe" }

// -- tests --------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scanner::bundle_mtime;
    use std::fs;
    use tempfile::TempDir;

    fn make_vst_bundle(dir: &Path, name: &str, plist: Option<&str>) -> PathBuf {
        let bundle = dir.join(format!("{name}.vst"));
        fs::create_dir_all(bundle.join("Contents").join("MacOS")).unwrap();
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
    <key>CFBundleGetInfoString</key><string>4.3.1, Copyright 2025 Native Instruments GmbH</string>
    <key>CFBundleIdentifier</key><string>com.native-instruments.Battery4</string>
</dict></plist>"#;

    #[cfg(target_os = "macos")]
    #[test]
    fn walk_finds_vst_bundles() {
        let tmp = TempDir::new().unwrap();
        make_vst_bundle(tmp.path(), "Serum", None);
        make_vst_bundle(tmp.path(), "Massive", None);

        let vendor = tmp.path().join("Xfer");
        fs::create_dir_all(&vendor).unwrap();
        make_vst_bundle(&vendor, "OTT", None);

        let found = walk_vst_bundles(&[tmp.path().to_path_buf()]);
        assert_eq!(found.len(), 3);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn skips_nonexistent_paths() {
        let found = walk_vst_bundles(&[PathBuf::from("/does/not/exist/VST")]);
        assert!(found.is_empty());
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn fallback_uses_plist_name_and_vendor() {
        let tmp = TempDir::new().unwrap();
        let bundle = make_vst_bundle(tmp.path(), "Battery 4", Some(BATTERY_PLIST));

        let plugin = build_plugin(&bundle, None, None);
        assert_eq!(plugin.name, "Battery 4");
        assert_eq!(plugin.version.as_deref(), Some("4.3.1"));
        assert_eq!(plugin.vendor.as_deref(), Some("Native Instruments GmbH"));
        assert!(matches!(plugin.format, PluginFormat::Vst2));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn fallback_uses_filename_when_no_plist() {
        let tmp = TempDir::new().unwrap();
        let bundle = make_vst_bundle(tmp.path(), "Serum", None);

        let plugin = build_plugin(&bundle, None, None);
        assert_eq!(plugin.name, "Serum");
        assert!(plugin.vendor.is_none());
        assert!(plugin.version.is_none());
    }

    #[test]
    fn probe_metadata_wins_over_plist() {
        let tmp = TempDir::new().unwrap();
        let bundle = make_vst_bundle(tmp.path(), "Battery 4", Some(BATTERY_PLIST));

        let probe_out = ProbeOutput {
            name: Some("Battery 4 (VST2)".to_string()),
            vendor: Some("Native Instruments".to_string()),
            category: Some(2),
        };

        let plugin = build_plugin(&bundle, Some(probe_out), None);
        assert_eq!(plugin.name, "Battery 4 (VST2)");
        assert_eq!(plugin.vendor.as_deref(), Some("Native Instruments"));
        assert_eq!(plugin.category.as_deref(), Some("Synth"));
        assert_eq!(plugin.version.as_deref(), Some("4.3.1"));
    }

    #[test]
    fn probe_empty_name_falls_back_to_plist() {
        let tmp = TempDir::new().unwrap();
        let bundle = make_vst_bundle(tmp.path(), "Battery 4", Some(BATTERY_PLIST));

        let probe_out = ProbeOutput {
            name: Some(String::new()),
            vendor: None,
            category: Some(1),
        };

        let plugin = build_plugin(&bundle, Some(probe_out), None);
        assert_eq!(plugin.name, "Battery 4");
    }

    #[test]
    fn vst_category_str_maps_correctly() {
        assert_eq!(vst_category_str(1), "Fx");
        assert_eq!(vst_category_str(2), "Synth");
        assert_eq!(vst_category_str(0), "Unknown");
        assert_eq!(vst_category_str(99), "Unknown");
    }

    #[test]
    fn build_plugin_stores_file_mtime() {
        let tmp = TempDir::new().unwrap();
        let bundle = make_vst_bundle(tmp.path(), "Serum", None);
        let mtime = bundle_mtime(&bundle);
        assert!(mtime.is_some());
        let plugin = build_plugin(&bundle, None, mtime);
        assert_eq!(plugin.file_mtime, mtime);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn skips_unchanged_vst2_by_mtime() {
        let tmp = TempDir::new().unwrap();
        let bundle = make_vst_bundle(tmp.path(), "Serum", None);

        // First scan: nothing known — plugin is returned
        let (plugins, skipped, _) = scan_vst2(&[tmp.path().to_path_buf()], None, &HashMap::new());
        assert_eq!(plugins.len(), 1);
        assert_eq!(skipped, 0);

        // Second scan: mtime matches — bundle is skipped
        let current = bundle_mtime(&bundle).unwrap();
        let mut known = HashMap::new();
        known.insert(bundle.to_string_lossy().into_owned(), current);
        let (plugins, skipped, _) = scan_vst2(&[tmp.path().to_path_buf()], None, &known);
        assert_eq!(plugins.len(), 0);
        assert_eq!(skipped, 1);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn rescans_vst2_when_mtime_changes() {
        let tmp = TempDir::new().unwrap();
        let bundle = make_vst_bundle(tmp.path(), "Serum", None);

        let mut known = HashMap::new();
        known.insert(bundle.to_string_lossy().into_owned(), 0i64);
        let (plugins, skipped, _) = scan_vst2(&[tmp.path().to_path_buf()], None, &known);
        assert_eq!(plugins.len(), 1);
        assert_eq!(skipped, 0);
    }
}
