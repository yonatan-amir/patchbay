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

// ── Path index ────────────────────────────────────────────────────────────────

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
        // Some registered AUs on modern macOS are dynamically registered without a
        // traditional .component on disk, so 100% coverage isn't achievable.
        assert!(
            with_path > 0,
            "expected at least one AU to have a resolved path, got 0/{} — \
             check that /System/Library/Components is readable",
            plugins.len()
        );
        eprintln!("AU path coverage: {pct}% ({with_path}/{}) resolved", plugins.len());
    }
}
