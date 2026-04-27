use std::collections::HashMap;
use std::ffi::c_void;
use std::path::{Path, PathBuf};

use super::{PluginFormat, ScanError, ScannedPlugin};

// ── CoreAudio / CoreFoundation raw FFI ────────────────────────────────────────

type OSType = u32;
type AudioComponent = *mut c_void;
type CFStringRef = *const c_void;

#[repr(C)]
struct AudioComponentDescription {
    component_type:      OSType,
    component_sub_type:  OSType,
    component_mfr:       OSType,
    component_flags:     u32,
    component_flag_mask: u32,
}

#[link(name = "AudioToolbox", kind = "framework")]
extern "C" {
    fn AudioComponentFindNext(
        inComponent: AudioComponent,
        inDesc: *const AudioComponentDescription,
    ) -> AudioComponent;

    fn AudioComponentCopyName(
        inComponent: AudioComponent,
        outName: *mut CFStringRef,
    ) -> i32;

    fn AudioComponentGetDescription(
        inComponent: AudioComponent,
        outDesc: *mut AudioComponentDescription,
    ) -> i32;

}

#[link(name = "CoreFoundation", kind = "framework")]
extern "C" {
    fn CFStringGetCString(
        theString: CFStringRef,
        buffer: *mut u8,
        bufferSize: isize,
        encoding: u32,
    ) -> bool;

    fn CFRelease(cf: *const c_void);
}

const CF_STRING_ENCODING_UTF8: u32 = 0x0800_0100;

// ── Helpers ───────────────────────────────────────────────────────────────────

fn ostype_to_str(t: OSType) -> String {
    let bytes = t.to_be_bytes();
    if bytes.iter().all(|b| b.is_ascii_graphic()) {
        String::from_utf8_lossy(&bytes).into_owned()
    } else {
        format!("{t:#010x}")
    }
}

unsafe fn cfstring_to_rust(cfstr: CFStringRef) -> Option<String> {
    if cfstr.is_null() {
        return None;
    }
    let mut buf = vec![0u8; 1024];
    let ok = unsafe {
        CFStringGetCString(cfstr, buf.as_mut_ptr(), buf.len() as isize, CF_STRING_ENCODING_UTF8)
    };
    if !ok {
        return None;
    }
    let end = buf.iter().position(|&b| b == 0).unwrap_or(buf.len());
    String::from_utf8(buf[..end].to_vec()).ok()
}

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

fn split_au_name(full_name: &str) -> (String, Option<String>) {
    if let Some((vendor, name)) = full_name.split_once(": ") {
        (name.to_string(), Some(vendor.to_string()))
    } else {
        (full_name.to_string(), None)
    }
}

// ── Filesystem walk ───────────────────────────────────────────────────────────

struct ComponentInfo {
    type_str: String,
    subtype_str: String,
    mfr_str: String,
    plist_name: Option<String>,
}

/// Parse **all** `AudioComponents` array entries from a `.component` bundle's `Info.plist`.
/// A single bundle can declare multiple components (e.g. stereo + mono variants of an instrument).
fn read_audio_components(bundle: &Path) -> Vec<ComponentInfo> {
    let plist_path = bundle.join("Contents").join("Info.plist");
    let Ok(val) = plist::Value::from_file(&plist_path) else { return vec![] };
    let Some(dict) = val.as_dictionary() else { return vec![] };
    let Some(array) = dict.get("AudioComponents").and_then(|v| v.as_array()) else {
        return vec![];
    };
    array
        .iter()
        .filter_map(|v| v.as_dictionary())
        .filter_map(|comp| {
            let t = plist_ostype(comp, "type")?;
            let s = plist_ostype(comp, "subtype")?;
            let m = plist_ostype(comp, "manufacturer")?;
            let plist_name = comp.get("name").and_then(|v| v.as_string()).map(str::to_string);
            Some(ComponentInfo { type_str: t, subtype_str: s, mfr_str: m, plist_name })
        })
        .collect()
}

fn read_bundle_version(bundle: &Path) -> Option<String> {
    let plist_path = bundle.join("Contents").join("Info.plist");
    let Ok(val) = plist::Value::from_file(&plist_path) else { return None };
    let dict = val.as_dictionary()?;
    dict.get("CFBundleShortVersionString")
        .and_then(|v| v.as_string())
        .map(str::to_string)
}

/// Walk `dirs` for `.component` bundle directories. Never executes plugin code.
pub fn walk_component_bundles(dirs: &[PathBuf]) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for dir in dirs {
        collect_components(dir, &mut out);
    }
    out
}

fn collect_components(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().map(|e| e.eq_ignore_ascii_case("component")).unwrap_or(false)
            && path.is_dir()
        {
            out.push(path);
        } else if path.is_dir() {
            collect_components(&path, out);
        }
    }
}

pub fn default_au_dirs() -> Vec<PathBuf> {
    let mut dirs = vec![
        PathBuf::from("/System/Library/Components"),
        PathBuf::from("/Library/Audio/Plug-Ins/Components"),
        PathBuf::from("/Library/Components"),
    ];
    if let Ok(home) = std::env::var("HOME") {
        dirs.push(PathBuf::from(&home).join("Library/Audio/Plug-Ins/Components"));
        dirs.push(PathBuf::from(&home).join("Library/Components"));
    }
    dirs
}

/// Scan AU plugins by walking the filesystem and reading `Info.plist` directly.
///
/// Emits one `ScannedPlugin` per `AudioComponents` plist entry, so a single bundle
/// that declares multiple components (e.g. stereo + mono + multi-output variants)
/// produces multiple records — the same way Ableton counts them.
///
/// Unlike the registry approach, this finds AUs regardless of whether CoreAudio
/// has registered them, which is important for Intel-only or damaged plugins.
///
/// Returns `(plugins, errors)`.
pub fn scan_au_filesystem(dirs: &[PathBuf]) -> (Vec<ScannedPlugin>, Vec<ScanError>) {
    let mut plugins = Vec::new();

    for bundle in walk_component_bundles(dirs) {
        let file_mtime = super::bundle_mtime(&bundle);
        let version = read_bundle_version(&bundle);
        let components = read_audio_components(&bundle);

        if components.is_empty() {
            continue;
        }

        for comp in components {
            let class_id = format!("{}/{}/{}", comp.type_str, comp.subtype_str, comp.mfr_str);
            let category = au_type_to_category(&comp.type_str).map(str::to_string);

            let (name, vendor) = match comp.plist_name.as_deref() {
                Some(s) if !s.is_empty() => split_au_name(s),
                _ => (super::file_stem(&bundle), None),
            };

            plugins.push(ScannedPlugin {
                name,
                vendor,
                version: version.clone(),
                category,
                class_id: Some(class_id),
                path: bundle.clone(),
                format: PluginFormat::Au,
                file_mtime,
            });
        }
    }

    (plugins, Vec::new())
}

// ── Path index (used by registry scanner) ────────────────────────────────────

/// Walk the standard AU search directories and build a `class_id → bundle path` map.
/// Used to correlate registry entries (which carry no path) with filesystem bundles.
fn build_path_index() -> HashMap<String, PathBuf> {
    let mut map = HashMap::new();
    let mut dirs = vec![
        PathBuf::from("/System/Library/Components"),
        PathBuf::from("/Library/Audio/Plug-Ins/Components"),
        PathBuf::from("/Library/Components"),
    ];
    if let Ok(home) = std::env::var("HOME") {
        dirs.push(PathBuf::from(&home).join("Library/Audio/Plug-Ins/Components"));
        dirs.push(PathBuf::from(&home).join("Library/Components"));
    }
    for dir in dirs {
        let Ok(entries) = std::fs::read_dir(&dir) else { continue };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().map(|e| e.eq_ignore_ascii_case("component")).unwrap_or(false) {
                if let Some(id) = extract_class_id(&path) {
                    map.insert(id, path);
                }
            }
        }
    }
    map
}

/// Parse a `.component` bundle's `Info.plist` and return its `"type/subtype/mfr"` class ID.
fn extract_class_id(bundle: &Path) -> Option<String> {
    let plist_path = bundle.join("Contents").join("Info.plist");
    let val = plist::Value::from_file(&plist_path).ok()?;
    let dict = val.as_dictionary()?;
    let comp = dict
        .get("AudioComponents")
        .and_then(|v| v.as_array())
        .and_then(|a| a.first())
        .and_then(|v| v.as_dictionary())?;

    let t = plist_ostype(comp, "type")?;
    let s = plist_ostype(comp, "subtype")?;
    let m = plist_ostype(comp, "manufacturer")?;
    Some(format!("{t}/{s}/{m}"))
}

/// Read an OSType field from a plist dictionary entry, normalizing to the same
/// 4-char string encoding as `ostype_to_str` so keys match registry-derived class IDs.
fn plist_ostype(dict: &plist::Dictionary, key: &str) -> Option<String> {
    match dict.get(key)? {
        plist::Value::String(s) => Some(s.clone()),
        plist::Value::Integer(i) => Some(ostype_to_str(i.as_unsigned()? as u32)),
        _ => None,
    }
}

// ── Public API ────────────────────────────────────────────────────────────────

/// Enumerate all AudioUnits registered with the CoreAudio component registry.
///
/// Uses `AudioComponentFindNext` rather than walking the filesystem — the OS
/// registry is authoritative and catches AUs registered from non-standard paths
/// or dynamically at runtime (e.g. via `AudioComponentRegister`).
///
/// Returns `(plugins, errors)`. The registry walk itself cannot produce partial
/// errors, so the errors vec is always empty; it is kept for API consistency
/// with the VST3 scanner.
pub fn scan_au_registry() -> (Vec<ScannedPlugin>, Vec<ScanError>) {
    let mut plugins = Vec::new();
    let path_index = build_path_index();

    // All-zero descriptor is the wildcard: match every registered component.
    let wildcard = AudioComponentDescription {
        component_type:      0,
        component_sub_type:  0,
        component_mfr:       0,
        component_flags:     0,
        component_flag_mask: 0,
    };

    let mut comp: AudioComponent = std::ptr::null_mut();

    loop {
        // SAFETY: AudioComponentFindNext is thread-safe and returns NULL to signal end.
        comp = unsafe { AudioComponentFindNext(comp, &wildcard) };
        if comp.is_null() {
            break;
        }

        // ── type / subtype / manufacturer from registry ──────────────────────
        let mut desc = AudioComponentDescription {
            component_type:      0,
            component_sub_type:  0,
            component_mfr:       0,
            component_flags:     0,
            component_flag_mask: 0,
        };
        unsafe { AudioComponentGetDescription(comp, &mut desc) };

        let type_str = ostype_to_str(desc.component_type);
        let sub_str  = ostype_to_str(desc.component_sub_type);
        let mfr_str  = ostype_to_str(desc.component_mfr);

        let category = au_type_to_category(&type_str).map(str::to_string);
        let class_id = format!("{type_str}/{sub_str}/{mfr_str}");

        // ── display name via AudioComponentCopyName ──────────────────────────
        // Caller owns the returned CFStringRef (Copy rule) — must CFRelease.
        let (name, vendor) = unsafe {
            let mut name_ref: CFStringRef = std::ptr::null();
            AudioComponentCopyName(comp, &mut name_ref);
            let full = cfstring_to_rust(name_ref);
            if !name_ref.is_null() {
                CFRelease(name_ref);
            }
            match full {
                Some(s) => split_au_name(&s),
                None    => (class_id.clone(), None),
            }
        };

        let path = path_index.get(&class_id).cloned().unwrap_or_default();
        let file_mtime = if path == std::path::PathBuf::new() {
            None
        } else {
            super::bundle_mtime(&path)
        };
        plugins.push(ScannedPlugin {
            name,
            vendor,
            version: None,
            category,
            class_id: Some(class_id),
            path,
            format: PluginFormat::Au,
            file_mtime,
        });
    }

    (plugins, Vec::new())
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    #[test]
    fn ostype_printable_roundtrips() {
        let t: OSType = u32::from_be_bytes(*b"aufx");
        assert_eq!(ostype_to_str(t), "aufx");
    }

    #[test]
    fn ostype_non_printable_formats_as_hex() {
        assert_eq!(ostype_to_str(1), "0x00000001");
    }

    #[test]
    fn split_au_name_with_colon_separator() {
        let (name, vendor) = split_au_name("Native Instruments: Battery 4");
        assert_eq!(name, "Battery 4");
        assert_eq!(vendor.as_deref(), Some("Native Instruments"));
    }

    #[test]
    fn split_au_name_without_separator() {
        let (name, vendor) = split_au_name("AUDelay");
        assert_eq!(name, "AUDelay");
        assert!(vendor.is_none());
    }

    #[test]
    fn au_type_codes_map_to_categories() {
        assert_eq!(au_type_to_category("aufx"), Some("Fx"));
        assert_eq!(au_type_to_category("aumu"), Some("Instrument"));
        assert_eq!(au_type_to_category("aumi"), Some("MIDI Processor"));
        assert_eq!(au_type_to_category("auou"), Some("Output"));
        assert_eq!(au_type_to_category("aumf"), Some("Mixer"));
        assert_eq!(au_type_to_category("aupn"), Some("Panner"));
        assert_eq!(au_type_to_category("augn"), Some("Generator"));
        assert_eq!(au_type_to_category("xxxx"), None);
    }

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

    // Same plugin but with OSType fields stored as integers.
    // "aufx" = 0x61756678 = 1635083896
    // "FPQ3" = 0x46505133 = 1179668787
    // "FabF" = 0x46616246 = 1180787270
    const PRO_Q_AU_PLIST_INT: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
    <key>AudioComponents</key><array><dict>
        <key>name</key><string>FabFilter: Pro-Q 3</string>
        <key>type</key><integer>1635083896</integer>
        <key>subtype</key><integer>1179668787</integer>
        <key>manufacturer</key><integer>1180787270</integer>
    </dict></array>
</dict></plist>"#;

    fn make_component(dir: &std::path::Path, name: &str, plist: &str) -> PathBuf {
        let bundle = dir.join(format!("{name}.component"));
        fs::create_dir_all(bundle.join("Contents")).unwrap();
        fs::write(bundle.join("Contents").join("Info.plist"), plist).unwrap();
        bundle
    }

    #[test]
    fn extract_class_id_from_string_ostype() {
        let tmp = TempDir::new().unwrap();
        let bundle = make_component(tmp.path(), "Pro-Q 3", PRO_Q_AU_PLIST);
        assert_eq!(extract_class_id(&bundle).as_deref(), Some("aufx/FPQ3/FabF"));
    }

    #[test]
    fn extract_class_id_from_integer_ostype() {
        let tmp = TempDir::new().unwrap();
        let bundle = make_component(tmp.path(), "Pro-Q 3 (int)", PRO_Q_AU_PLIST_INT);
        // Integer OSTypes must decode to the same 4-char string as the registry.
        assert_eq!(extract_class_id(&bundle).as_deref(), Some("aufx/FPQ3/FabF"));
    }

    #[test]
    fn extract_class_id_returns_none_for_missing_plist() {
        let tmp = TempDir::new().unwrap();
        let bundle = tmp.path().join("Empty.component");
        fs::create_dir_all(bundle.join("Contents")).unwrap();
        assert!(extract_class_id(&bundle).is_none());
    }

    // ── filesystem-walk tests ─────────────────────────────────────────────────

    const MULTI_COMPONENT_PLIST: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
    <key>CFBundleShortVersionString</key><string>7.4.3</string>
    <key>AudioComponents</key><array>
        <dict>
            <key>name</key><string>NI: Kontakt</string>
            <key>type</key><string>aumu</string>
            <key>subtype</key><string>nikt</string>
            <key>manufacturer</key><string>NInm</string>
        </dict>
        <dict>
            <key>name</key><string>NI: Kontakt (Mono)</string>
            <key>type</key><string>aumu</string>
            <key>subtype</key><string>nkmo</string>
            <key>manufacturer</key><string>NInm</string>
        </dict>
    </array>
</dict></plist>"#;

    #[test]
    fn read_audio_components_parses_all_entries() {
        let tmp = TempDir::new().unwrap();
        let bundle = make_component(tmp.path(), "Kontakt", MULTI_COMPONENT_PLIST);
        let comps = read_audio_components(&bundle);
        assert_eq!(comps.len(), 2);
        assert_eq!(comps[0].plist_name.as_deref(), Some("NI: Kontakt"));
        assert_eq!(comps[0].type_str, "aumu");
        assert_eq!(comps[1].plist_name.as_deref(), Some("NI: Kontakt (Mono)"));
    }

    #[test]
    fn read_audio_components_returns_empty_when_no_plist() {
        let tmp = TempDir::new().unwrap();
        let bundle = tmp.path().join("Broken.component");
        fs::create_dir_all(bundle.join("Contents")).unwrap();
        assert!(read_audio_components(&bundle).is_empty());
    }

    #[test]
    fn read_audio_components_returns_empty_when_no_audio_components_key() {
        let tmp = TempDir::new().unwrap();
        let plist = r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
    <key>CFBundleName</key><string>NoAudioComponentsBundle</string>
</dict></plist>"#;
        let bundle = make_component(tmp.path(), "NoAC", plist);
        assert!(read_audio_components(&bundle).is_empty());
    }

    #[test]
    fn read_bundle_version_reads_cfbundle_short_version() {
        let tmp = TempDir::new().unwrap();
        let bundle = make_component(tmp.path(), "Kontakt", MULTI_COMPONENT_PLIST);
        assert_eq!(read_bundle_version(&bundle).as_deref(), Some("7.4.3"));
    }

    #[test]
    fn walk_component_bundles_finds_nested() {
        let tmp = TempDir::new().unwrap();
        // Flat
        make_component(tmp.path(), "Pro-Q 3", PRO_Q_AU_PLIST);
        // Nested inside a vendor subdirectory
        let vendor = tmp.path().join("Native Instruments");
        fs::create_dir_all(&vendor).unwrap();
        make_component(&vendor, "Kontakt", MULTI_COMPONENT_PLIST);

        let found = walk_component_bundles(&[tmp.path().to_path_buf()]);
        assert_eq!(found.len(), 2);
    }

    #[test]
    fn scan_au_filesystem_expands_multi_component_bundle() {
        let tmp = TempDir::new().unwrap();
        make_component(tmp.path(), "Kontakt", MULTI_COMPONENT_PLIST);

        let (plugins, errors) = scan_au_filesystem(&[tmp.path().to_path_buf()]);
        assert!(errors.is_empty());
        // Two AudioComponents entries → two plugins
        assert_eq!(plugins.len(), 2);
        assert_eq!(plugins[0].name, "Kontakt");
        assert_eq!(plugins[0].vendor.as_deref(), Some("NI"));
        assert_eq!(plugins[0].version.as_deref(), Some("7.4.3"));
        assert_eq!(plugins[0].category.as_deref(), Some("Instrument"));
        assert!(plugins[0].class_id.as_deref().is_some());
        assert_eq!(plugins[1].name, "Kontakt (Mono)");
    }

    #[test]
    fn scan_au_filesystem_skips_bundles_without_audio_components() {
        let tmp = TempDir::new().unwrap();
        let plist = r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
    <key>CFBundleName</key><string>NotAnAU</string>
</dict></plist>"#;
        make_component(tmp.path(), "NotAnAU", plist);

        let (plugins, errors) = scan_au_filesystem(&[tmp.path().to_path_buf()]);
        assert!(errors.is_empty());
        assert!(plugins.is_empty());
    }

    #[test]
    fn scan_au_filesystem_falls_back_to_filename_when_no_plist_name() {
        let tmp = TempDir::new().unwrap();
        let plist = r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
    <key>AudioComponents</key><array><dict>
        <key>type</key><string>aufx</string>
        <key>subtype</key><string>test</string>
        <key>manufacturer</key><string>demo</string>
    </dict></array>
</dict></plist>"#;
        make_component(tmp.path(), "MyPlugin", plist);

        let (plugins, _) = scan_au_filesystem(&[tmp.path().to_path_buf()]);
        assert_eq!(plugins.len(), 1);
        assert_eq!(plugins[0].name, "MyPlugin");
        assert!(plugins[0].vendor.is_none());
    }

    // ── registry integration test ─────────────────────────────────────────────

    #[test]
    #[ignore = "requires macOS with installed AUs — run manually: cargo test -- --ignored"]
    fn registry_returns_results_with_paths() {
        let (plugins, errors) = scan_au_registry();
        assert!(errors.is_empty());
        assert!(
            plugins.len() > 100,
            "expected >100 registered AUs, got {}",
            plugins.len()
        );
        let with_path = plugins.iter().filter(|p| p.path != PathBuf::new()).count();
        let pct = with_path * 100 / plugins.len();
        assert!(
            with_path > 0,
            "expected at least one AU to have a resolved path, got 0/{} — \
             check that /System/Library/Components is readable",
            plugins.len()
        );
        eprintln!("AU path coverage: {pct}% ({with_path}/{}) resolved", plugins.len());
    }

    #[test]
    #[ignore = "requires macOS with installed AUs — run manually: cargo test -- --ignored"]
    fn filesystem_scan_finds_more_than_registry() {
        let (fs_plugins, _) = scan_au_filesystem(&default_au_dirs());
        let (reg_plugins, _) = scan_au_registry();
        eprintln!(
            "filesystem: {}  registry: {}",
            fs_plugins.len(),
            reg_plugins.len()
        );
        assert!(
            fs_plugins.len() >= reg_plugins.len(),
            "filesystem scan ({}) should find at least as many AUs as registry ({})",
            fs_plugins.len(),
            reg_plugins.len()
        );
    }
}
