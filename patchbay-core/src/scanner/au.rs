use std::ffi::c_void;
use std::path::PathBuf;

use super::{PluginFormat, ScanError, ScannedPlugin};

// ── CoreAudio / CoreFoundation raw FFI ────────────────────────────────────────

type OSType = u32;
type AudioComponent = *mut c_void;
type CFStringRef = *const c_void;
type CFURLRef = *const c_void;

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

    // "Get" rule: caller does NOT own the returned CFURLRef — do not CFRelease it.
    fn AudioComponentGetBundleURL(inComponent: AudioComponent) -> CFURLRef;
}

#[link(name = "CoreFoundation", kind = "framework")]
extern "C" {
    fn CFStringGetCString(
        theString: CFStringRef,
        buffer: *mut u8,
        bufferSize: isize,
        encoding: u32,
    ) -> bool;

    // "Copy" rule: caller owns the returned CFStringRef — must CFRelease it.
    fn CFURLCopyFileSystemPath(anURL: CFURLRef, pathStyle: i32) -> CFStringRef;

    fn CFRelease(cf: *const c_void);
}

const CF_STRING_ENCODING_UTF8: u32 = 0x0800_0100;
const CF_URL_POSIX_PATH_STYLE: i32 = 0;

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

        // ── bundle path via AudioComponentGetBundleURL ───────────────────────
        // Get rule: caller does NOT own the CFURLRef — do not CFRelease it.
        // CFURLCopyFileSystemPath returns an owned CFStringRef — must CFRelease.
        let path = unsafe {
            let url_ref = AudioComponentGetBundleURL(comp);
            if url_ref.is_null() {
                PathBuf::new()
            } else {
                let path_ref = CFURLCopyFileSystemPath(url_ref, CF_URL_POSIX_PATH_STYLE);
                let s = cfstring_to_rust(path_ref);
                if !path_ref.is_null() {
                    CFRelease(path_ref);
                }
                s.map(PathBuf::from).unwrap_or_default()
            }
        };

        plugins.push(ScannedPlugin {
            name,
            vendor,
            version: None,
            category,
            class_id: Some(class_id),
            path,
            format: PluginFormat::Au,
        });
    }

    (plugins, Vec::new())
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

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

    #[test]
    #[ignore = "requires macOS with installed AUs — run manually: cargo test -- --ignored"]
    fn registry_returns_expected_count() {
        let (plugins, errors) = scan_au_registry();
        assert!(errors.is_empty());
        assert!(
            plugins.len() > 100,
            "expected >100 registered AUs, got {}",
            plugins.len()
        );
    }
}
