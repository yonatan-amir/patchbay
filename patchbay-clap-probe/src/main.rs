//! Sandboxed CLAP metadata probe.
//!
//! Usage: patchbay-clap-probe <bundle-or-dll-path>
//!
//! Outputs a JSON array to stdout (one entry per plugin exported by the bundle):
//!   success  ->  [{"id":"...","name":"...","vendor":"...","version":"...","features":[...]}]
//!   empty    ->  []   (factory returned 0 plugins)
//!   failure  ->  (non-zero exit; stdout is empty or irrelevant)
//!
//! One .clap bundle can export multiple plugins — hence array output.
//! The parent scanner spawns one process per bundle and reads stdout.
//! Crashes and timeouts are harmless: the parent falls back to filename metadata.

use std::path::{Path, PathBuf};
use std::process;

fn main() {
    let arg = std::env::args().nth(1).unwrap_or_default();
    let bundle = PathBuf::from(&arg);

    if bundle.as_os_str().is_empty() {
        eprintln!("usage: patchbay-clap-probe <bundle-path>");
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
    Err("CLAP probe not yet implemented on this platform".to_string())
}

// -- macOS implementation -----------------------------------------------------

#[cfg(target_os = "macos")]
mod macos {
    use std::ffi::{c_char, c_void, CStr, CString};
    use std::path::{Path, PathBuf};
    use libloading::{Library, Symbol};
    use serde::Serialize;

    // "clap.plugin-factory" as a null-terminated C string
    const CLAP_PLUGIN_FACTORY_ID: &[u8] = b"clap.plugin-factory\0";

    // CLAP ABI structs — mirror of clap/include/clap/entry.h and factory/plugin-factory.h

    #[repr(C)]
    struct ClapVersion {
        major: u32,
        minor: u32,
        revision: u32,
    }

    #[repr(C)]
    struct ClapPluginEntry {
        clap_version: ClapVersion,
        init: unsafe extern "C" fn(*const c_char) -> bool,
        deinit: unsafe extern "C" fn(),
        get_factory: unsafe extern "C" fn(*const c_char) -> *const c_void,
    }

    #[repr(C)]
    struct ClapPluginDescriptor {
        clap_version: ClapVersion,
        id: *const c_char,
        name: *const c_char,
        vendor: *const c_char,
        url: *const c_char,
        manual_url: *const c_char,
        support_url: *const c_char,
        version: *const c_char,
        description: *const c_char,
        // null-terminated array of feature strings
        features: *const *const c_char,
    }

    #[repr(C)]
    struct ClapPluginFactory {
        get_plugin_count: unsafe extern "C" fn(*const ClapPluginFactory) -> u32,
        get_plugin_descriptor:
            unsafe extern "C" fn(*const ClapPluginFactory, u32) -> *const ClapPluginDescriptor,
        // create_plugin is required for struct layout but not called
        create_plugin:
            unsafe extern "C" fn(*const ClapPluginFactory, *const c_void, *const c_char)
                -> *const c_void,
    }

    #[derive(Serialize)]
    struct PluginDescriptor {
        id: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        name: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        vendor: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        version: Option<String>,
        #[serde(skip_serializing_if = "Vec::is_empty")]
        features: Vec<String>,
    }

    pub fn probe_bundle(bundle: &Path) -> Result<String, String> {
        let binary = find_binary(bundle)?;
        let bundle_cstr = path_to_cstring(bundle)?;

        unsafe {
            let lib = Library::new(&binary)
                .map_err(|e| format!("dlopen {}: {e}", binary.display()))?;

            let entry: Symbol<ClapPluginEntry> = lib
                .get(b"clap_entry\0")
                .map_err(|e| format!("no clap_entry in {}: {e}", binary.display()))?;

            if !(entry.init)(bundle_cstr.as_ptr()) {
                return Err("clap_entry.init returned false".to_string());
            }

            let factory_raw =
                (entry.get_factory)(CLAP_PLUGIN_FACTORY_ID.as_ptr() as *const c_char);

            if factory_raw.is_null() {
                (entry.deinit)();
                std::mem::forget(lib);
                return Ok("[]".to_string());
            }

            let factory = &*(factory_raw as *const ClapPluginFactory);
            let count = (factory.get_plugin_count)(factory);

            let mut descriptors: Vec<PluginDescriptor> = Vec::with_capacity(count as usize);
            for i in 0..count {
                let desc_ptr = (factory.get_plugin_descriptor)(factory, i);
                if desc_ptr.is_null() {
                    continue;
                }
                let desc = &*desc_ptr;
                descriptors.push(PluginDescriptor {
                    id: read_cstr(desc.id).unwrap_or_default(),
                    name: read_cstr(desc.name),
                    vendor: read_cstr(desc.vendor),
                    version: read_cstr(desc.version),
                    features: read_cstr_array(desc.features),
                });
            }

            (entry.deinit)();
            // Skip dlclose: some plugins crash on unload; process exits immediately anyway.
            std::mem::forget(lib);

            serde_json::to_string(&descriptors).map_err(|e| e.to_string())
        }
    }

    unsafe fn read_cstr(ptr: *const c_char) -> Option<String> {
        if ptr.is_null() {
            return None;
        }
        let s = CStr::from_ptr(ptr).to_string_lossy().trim().to_string();
        if s.is_empty() { None } else { Some(s) }
    }

    unsafe fn read_cstr_array(ptr: *const *const c_char) -> Vec<String> {
        if ptr.is_null() {
            return Vec::new();
        }
        let mut out = Vec::new();
        let mut i = 0usize;
        loop {
            let p = *ptr.add(i);
            if p.is_null() {
                break;
            }
            out.push(CStr::from_ptr(p).to_string_lossy().to_string());
            i += 1;
        }
        out
    }

    fn path_to_cstring(path: &Path) -> Result<CString, String> {
        use std::os::unix::ffi::OsStrExt;
        CString::new(path.as_os_str().as_bytes())
            .map_err(|e| format!("path contains null byte: {e}"))
    }

    /// Locate the Mach-O binary inside a `.clap` bundle directory.
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
