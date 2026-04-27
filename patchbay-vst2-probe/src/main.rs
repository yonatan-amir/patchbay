//! Sandboxed VST2 metadata probe.
//!
//! Usage: patchbay-vst2-probe <bundle-or-dll-path>
//!
//! Outputs a single JSON line to stdout:
//!   success  ->  {"name":"...","vendor":"...","category":N}
//!   failure  ->  (non-zero exit; stdout is empty or irrelevant)
//!
//! The parent scanner spawns one process per plugin and reads stdout.
//! Crashes and timeouts are harmless -- the parent falls back to filename metadata.

use std::path::{Path, PathBuf};
use std::process;

fn main() {
    let arg = std::env::args().nth(1).unwrap_or_default();
    let bundle = PathBuf::from(&arg);

    if bundle.as_os_str().is_empty() {
        eprintln!("usage: patchbay-vst2-probe <bundle-path>");
        process::exit(1);
    }

    match probe(&bundle) {
        Ok(json) => println!("{json}"),
        Err(e) => {
            eprintln!("probe failed: {e}");
            process::exit(1);
        }
    }
}

// -- platform dispatch --------------------------------------------------------

#[cfg(target_os = "macos")]
fn probe(bundle: &Path) -> Result<String, String> {
    macos::probe_bundle(bundle)
}

#[cfg(not(target_os = "macos"))]
fn probe(_bundle: &Path) -> Result<String, String> {
    Err("VST2 probe not yet implemented on this platform".to_string())
}

// -- macOS implementation -----------------------------------------------------

#[cfg(target_os = "macos")]
mod macos {
    use std::ffi::c_void;
    use std::path::{Path, PathBuf};
    use libloading::{Library, Symbol};
    use serde::Serialize;

    // VST2 dispatcher opcodes (VST 2.4 spec)
    const EFF_OPEN: i32 = 0;
    const EFF_CLOSE: i32 = 1;
    const EFF_GET_PLUG_CATEGORY: i32 = 35;
    const EFF_GET_EFFECT_NAME: i32 = 45;
    const EFF_GET_VENDOR_STRING: i32 = 47;

    // audioMasterCallback opcode
    const AUDIO_MASTER_VERSION: i32 = 6;
    const VST_2_4_VERSION: isize = 2400;

    // First four bytes of every valid AEffect: 'VstP' (big-endian)
    const VST_MAGIC: i32 = 0x56737450_u32 as i32;

    type DispatcherFn =
        unsafe extern "C" fn(*mut AEffectHeader, i32, i32, isize, *mut c_void, f32) -> isize;
    type AudioMasterFn =
        unsafe extern "C" fn(*mut AEffectHeader, i32, i32, isize, *mut c_void, f32) -> isize;
    type VstPlugMainFn = unsafe extern "C" fn(AudioMasterFn) -> *mut AEffectHeader;

    /// Minimal prefix of the VST2 AEffect struct.
    ///
    /// On 64-bit systems:
    ///   offset 0 : magic      (i32, 4 bytes)
    ///   offset 4 : [padding]  (4 bytes, inserted by repr(C))
    ///   offset 8 : dispatcher (fn pointer, 8 bytes)
    ///
    /// The compile-time assert below guards this layout assumption.
    #[repr(C)]
    pub struct AEffectHeader {
        pub magic: i32,
        pub dispatcher: DispatcherFn,
    }

    const _: () = {
        assert!(
            std::mem::offset_of!(AEffectHeader, dispatcher) == 8,
            "AEffect layout mismatch: dispatcher must be at byte offset 8"
        );
    };

    unsafe extern "C" fn audio_master_callback(
        _effect: *mut AEffectHeader,
        opcode: i32,
        _index: i32,
        _value: isize,
        _ptr: *mut c_void,
        _opt: f32,
    ) -> isize {
        if opcode == AUDIO_MASTER_VERSION { VST_2_4_VERSION } else { 0 }
    }

    #[derive(Serialize)]
    struct ProbeOutput {
        #[serde(skip_serializing_if = "Option::is_none")]
        name: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        vendor: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        category: Option<i32>,
    }

    pub fn probe_bundle(bundle: &Path) -> Result<String, String> {
        let binary = find_binary(bundle)?;

        unsafe {
            let lib = Library::new(&binary)
                .map_err(|e| format!("dlopen {}: {e}", binary.display()))?;

            let plug_main: Symbol<VstPlugMainFn> = lib
                .get(b"VSTPlugMain\0")
                .or_else(|_| lib.get(b"main\0"))
                .map_err(|e| format!("no VST2 entry point in {}: {e}", binary.display()))?;

            let effect: *mut AEffectHeader = plug_main(audio_master_callback);
            if effect.is_null() {
                return Err("VSTPlugMain returned null".to_string());
            }

            if (*effect).magic != VST_MAGIC {
                return Err(format!("bad magic: 0x{:08x}", (*effect).magic as u32));
            }

            let dispatch = (*effect).dispatcher;

            // effOpen: required by some plugins before other dispatches
            dispatch(effect, EFF_OPEN, 0, 0, std::ptr::null_mut(), 0.0);

            let name = dispatch_string(effect, dispatch, EFF_GET_EFFECT_NAME, 32);
            let vendor = dispatch_string(effect, dispatch, EFF_GET_VENDOR_STRING, 64);
            let category =
                dispatch(effect, EFF_GET_PLUG_CATEGORY, 0, 0, std::ptr::null_mut(), 0.0);

            // effClose: best-effort cleanup
            dispatch(effect, EFF_CLOSE, 0, 0, std::ptr::null_mut(), 0.0);

            // Skip dlclose: some plugins crash on unload; we exit immediately anyway.
            std::mem::forget(lib);

            let out = ProbeOutput {
                name: non_empty(name),
                vendor: non_empty(vendor),
                category: Some(category as i32),
            };
            serde_json::to_string(&out).map_err(|e| e.to_string())
        }
    }

    unsafe fn dispatch_string(
        effect: *mut AEffectHeader,
        dispatch: DispatcherFn,
        opcode: i32,
        max_len: usize,
    ) -> String {
        let mut buf = vec![0u8; max_len + 1];
        dispatch(effect, opcode, 0, 0, buf.as_mut_ptr() as *mut c_void, 0.0);
        buf[max_len] = 0;
        let end = buf.iter().position(|&b| b == 0).unwrap_or(max_len);
        String::from_utf8_lossy(&buf[..end]).trim().to_string()
    }

    fn non_empty(s: String) -> Option<String> {
        if s.is_empty() { None } else { Some(s) }
    }

    /// Locate the Mach-O binary inside a `.vst` bundle.
    ///
    /// Tries, in order:
    ///  1. `Contents/MacOS/<bundle-stem>`  (standard convention)
    ///  2. `CFBundleExecutable` from `Contents/Info.plist`
    ///  3. First file found in `Contents/MacOS/`
    fn find_binary(bundle: &Path) -> Result<PathBuf, String> {
        let macos_dir = bundle.join("Contents").join("MacOS");

        if let Some(stem) = bundle.file_stem() {
            let candidate = macos_dir.join(stem);
            if candidate.is_file() {
                return Ok(candidate);
            }
        }

        let plist_path = bundle.join("Contents").join("Info.plist");
        if plist_path.exists() {
            if let Ok(val) = plist::Value::from_file(&plist_path) {
                if let Some(dict) = val.as_dictionary() {
                    if let Some(exe) = dict.get("CFBundleExecutable").and_then(|v| v.as_string()) {
                        let candidate = macos_dir.join(exe);
                        if candidate.is_file() {
                            return Ok(candidate);
                        }
                    }
                }
            }
        }

        let first = std::fs::read_dir(&macos_dir)
            .map_err(|e| format!("cannot read {}: {e}", macos_dir.display()))?
            .flatten()
            .find(|e| e.path().is_file())
            .map(|e| e.path());

        first.ok_or_else(|| format!("no binary found in {}", macos_dir.display()))
    }
}
