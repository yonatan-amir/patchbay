//! Reader for Logic Pro projects (`.logicx` packages).
//!
//! A `.logicx` file is a macOS package — a directory that Finder presents as a
//! single file. The project state lives inside that directory as binary plists.
//!
//! # Format notes (reverse-engineered)
//! Logic Pro X writes `projectData` using `NSKeyedArchiver`. The binary plist
//! has the standard keyed-archiver envelope:
//!
//! ```text
//! {
//!   "$archiver": "NSKeyedArchiver",
//!   "$objects": [ "$null", {…}, {…}, … ],   // flat object pool
//!   "$top":     { "root": <UID → object 1> },
//!   "$version": 100000
//! }
//! ```
//!
//! All cross-object references are `plist::Value::Uid` integers that index into
//! `$objects`. We resolve them as needed rather than building a full object graph.
//!
//! # Plugin state blobs
//! Each device carries an opaque `presetData`/`pluginState` byte blob produced
//! by the AU itself. We store it raw without interpretation.

use std::path::{Path, PathBuf};

use plist::{Dictionary, Value};
use serde::{Deserialize, Serialize};
use thiserror::Error;

// ─── Errors ──────────────────────────────────────────────────────────────────

#[derive(Debug, Error)]
pub enum LogicError {
    #[error("path is not a .logicx package directory: {0}")]
    NotAPackage(PathBuf),
    #[error("project data not found; expected at {0}")]
    ProjectDataNotFound(PathBuf),
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("plist parse error: {0}")]
    Plist(#[from] plist::Error),
}

// ─── Public types ─────────────────────────────────────────────────────────────

#[derive(Debug, Serialize, Deserialize)]
pub struct LogicProject {
    pub name: String,
    pub tracks: Vec<LogicTrack>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct LogicTrack {
    pub name: String,
    pub kind: TrackKind,
    pub devices: Vec<LogicDevice>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TrackKind {
    Audio,
    SoftwareInstrument,
    Aux,
    Master,
    Unknown,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct LogicDevice {
    pub name: String,
    pub manufacturer: String,
    /// Four-character code string, e.g. `"aufx"` (effect) or `"aumu"` (instrument).
    pub component_type: String,
    /// Four-character code identifying the specific plugin, e.g. `"dcmp"`.
    pub component_subtype: String,
    pub bypassed: bool,
    /// Opaque AU preset / state blob; stored raw without interpretation.
    pub state: Vec<u8>,
}

// ─── Public API ───────────────────────────────────────────────────────────────

pub fn read_logicx(path: &Path) -> Result<LogicProject, LogicError> {
    if !path.is_dir() {
        return Err(LogicError::NotAPackage(path.to_path_buf()));
    }

    let data_path = locate_project_data(path)?;
    let root = Value::from_file(&data_path)?;

    let name = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("Untitled")
        .to_string();

    let tracks = if let Some(archive) = Archive::from_value(&root) {
        archive.extract_tracks()
    } else {
        extract_tracks_simple(root.as_dictionary())
    };

    Ok(LogicProject { name, tracks })
}

// ─── Package layout ───────────────────────────────────────────────────────────

fn locate_project_data(logicx: &Path) -> Result<PathBuf, LogicError> {
    // Logic 10.1+: data lives in a named alternative slot.
    let modern = logicx.join("Alternatives").join("000").join("projectData");
    if modern.exists() {
        return Ok(modern);
    }
    // Pre-10.1 layout: data at the package root.
    let legacy = logicx.join("projectData");
    if legacy.exists() {
        return Ok(legacy);
    }
    Err(LogicError::ProjectDataNotFound(modern))
}

// ─── NSKeyedArchiver decoder ──────────────────────────────────────────────────

struct Archive {
    objects: Vec<Value>,
}

impl Archive {
    fn from_value(root: &Value) -> Option<Self> {
        let dict = root.as_dictionary()?;
        if dict.get("$archiver")?.as_string()? != "NSKeyedArchiver" {
            return None;
        }
        Some(Self {
            objects: dict.get("$objects")?.as_array()?.clone(),
        })
    }

    /// Resolve a `Uid` reference to the object it points to; returns the value
    /// unchanged if it is not a `Uid`.
    fn resolve<'a>(&'a self, v: &'a Value) -> &'a Value {
        if let Value::Uid(uid) = v {
            if let Some(obj) = self.objects.get(uid.get() as usize) {
                return obj;
            }
        }
        v
    }

    fn resolve_str<'a>(&'a self, v: &'a Value) -> Option<&'a str> {
        self.resolve(v).as_string()
    }

    /// Return the `$classname` string for a keyed-archiver object dictionary.
    fn class_name<'a>(&'a self, dict: &'a Dictionary) -> Option<&'a str> {
        let class_obj = self.resolve(dict.get("$class")?).as_dictionary()?;
        class_obj.get("$classname")?.as_string()
    }

    /// Scan the flat `$objects` pool for entries whose class looks like a track.
    /// Logic stores all objects in one flat array; we don't need graph traversal.
    fn extract_tracks(&self) -> Vec<LogicTrack> {
        self.objects
            .iter()
            .filter_map(|obj| obj.as_dictionary())
            .filter(|dict| self.class_name(dict).map(is_track_class).unwrap_or(false))
            .filter_map(|dict| self.parse_track(dict))
            .collect()
    }

    fn parse_track(&self, dict: &Dictionary) -> Option<LogicTrack> {
        let name = dict
            .get("name")
            .and_then(|v| self.resolve_str(v))
            .unwrap_or("Unnamed")
            .to_string();

        let kind = self.resolve_track_kind(dict);
        let devices = self.collect_devices(dict);

        Some(LogicTrack { name, kind, devices })
    }

    fn resolve_track_kind(&self, dict: &Dictionary) -> TrackKind {
        let class = self.class_name(dict).unwrap_or("");
        match dict
            .get("trackType")
            .and_then(|v| self.resolve(v).as_signed_integer())
        {
            Some(0) => TrackKind::Audio,
            Some(1) => TrackKind::SoftwareInstrument,
            Some(2) => TrackKind::Aux,
            Some(3) => TrackKind::Master,
            // Fall back to class name when trackType is absent.
            _ => match class {
                "EMAudioTrack" => TrackKind::Audio,
                "EMSoftwareSynthTrack" => TrackKind::SoftwareInstrument,
                "EMBusTrack" => TrackKind::Aux,
                "EMMasterTrack" => TrackKind::Master,
                _ => TrackKind::Unknown,
            },
        }
    }

    fn collect_devices(&self, track_dict: &Dictionary) -> Vec<LogicDevice> {
        // Try several key names that have been observed across Logic versions.
        for key in &["plugins", "channelStrip", "devices", "pluginList", "audioUnitList"] {
            let Some(v) = track_dict.get(*key) else {
                continue;
            };
            let resolved = self.resolve(v);

            // Direct plugin array.
            if let Some(arr) = resolved.as_array() {
                let devs = self.parse_device_array(arr);
                if !devs.is_empty() {
                    return devs;
                }
            }

            // Channel strip dict containing a nested plugin array.
            if let Some(cs) = resolved.as_dictionary() {
                for inner_key in &["plugins", "audioUnitList"] {
                    if let Some(inner) = cs.get(*inner_key) {
                        if let Some(arr) = self.resolve(inner).as_array() {
                            let devs = self.parse_device_array(arr);
                            if !devs.is_empty() {
                                return devs;
                            }
                        }
                    }
                }
            }
        }
        vec![]
    }

    fn parse_device_array(&self, arr: &[Value]) -> Vec<LogicDevice> {
        arr.iter()
            .filter_map(|item| {
                self.resolve(item)
                    .as_dictionary()
                    .and_then(|d| self.parse_device(d))
            })
            .collect()
    }

    fn parse_device(&self, dict: &Dictionary) -> Option<LogicDevice> {
        let class = self.class_name(dict)?;
        if !is_device_class(class) {
            return None;
        }

        let name = dict
            .get("pluginName")
            .or_else(|| dict.get("name"))
            .and_then(|v| self.resolve_str(v))
            .filter(|s| !s.is_empty())?
            .to_string();

        let manufacturer = dict
            .get("manufacturerName")
            .or_else(|| dict.get("manufacturer"))
            .and_then(|v| self.resolve_str(v))
            .unwrap_or("")
            .to_string();

        let component_type = resolve_four_cc(self, dict, "componentType");
        let component_subtype = resolve_four_cc(self, dict, "componentSubType");

        let bypassed = dict
            .get("bypassState")
            .and_then(|v| self.resolve(v).as_boolean())
            .unwrap_or(false);

        let state = dict
            .get("presetData")
            .or_else(|| dict.get("pluginState"))
            .or_else(|| dict.get("data"))
            .and_then(|v| self.resolve(v).as_data())
            .map(<[u8]>::to_vec)
            .unwrap_or_default();

        Some(LogicDevice {
            name,
            manufacturer,
            component_type,
            component_subtype,
            bypassed,
            state,
        })
    }
}

// ─── Simple (non-NSKeyedArchiver) plist fallback ──────────────────────────────
// Kept for Logic 9 / older project formats that used a plain plist dictionary.

fn extract_tracks_simple(dict: Option<&Dictionary>) -> Vec<LogicTrack> {
    dict.and_then(|d| d.get("tracks"))
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_dictionary())
                .filter_map(parse_track_simple)
                .collect()
        })
        .unwrap_or_default()
}

fn parse_track_simple(dict: &Dictionary) -> Option<LogicTrack> {
    let name = dict.get("name")?.as_string()?.to_string();
    let kind = match dict.get("trackType").and_then(|v| v.as_signed_integer()) {
        Some(0) => TrackKind::Audio,
        Some(1) => TrackKind::SoftwareInstrument,
        Some(2) => TrackKind::Aux,
        Some(3) => TrackKind::Master,
        _ => TrackKind::Unknown,
    };
    let devices = dict
        .get("plugins")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_dictionary())
                .filter_map(parse_device_simple)
                .collect()
        })
        .unwrap_or_default();
    Some(LogicTrack { name, kind, devices })
}

fn parse_device_simple(dict: &Dictionary) -> Option<LogicDevice> {
    let name = dict.get("name")?.as_string()?;
    if name.is_empty() {
        return None;
    }
    Some(LogicDevice {
        name: name.to_string(),
        manufacturer: dict
            .get("manufacturer")
            .and_then(|v| v.as_string())
            .unwrap_or("")
            .to_string(),
        component_type: dict
            .get("componentType")
            .and_then(|v| simple_four_cc(v))
            .unwrap_or_default(),
        component_subtype: dict
            .get("componentSubType")
            .and_then(|v| simple_four_cc(v))
            .unwrap_or_default(),
        bypassed: dict
            .get("bypassState")
            .and_then(|v| v.as_boolean())
            .unwrap_or(false),
        state: dict
            .get("presetData")
            .or_else(|| dict.get("pluginState"))
            .and_then(|v| v.as_data())
            .map(<[u8]>::to_vec)
            .unwrap_or_default(),
    })
}

// ─── Helpers ──────────────────────────────────────────────────────────────────

fn is_track_class(class: &str) -> bool {
    matches!(
        class,
        "EMTrack"
            | "EMAudioTrack"
            | "EMSoftwareSynthTrack"
            | "EMBusTrack"
            | "EMMasterTrack"
            | "EMGlobalTrack"
            | "EMArrangerTrack"
    )
}

fn is_device_class(class: &str) -> bool {
    matches!(
        class,
        "EMDevice"
            | "EMInstrument"
            | "EMAudioEffect"
            | "EMPluginDevice"
            | "MPluginDevice"
            | "EMSoftSynthDevice"
    ) || (class.starts_with("EM") && class.contains("Plugin"))
}

/// Read a 4CC field from an archive-object dict: prefer integer (stored as
/// big-endian u32), fall back to string representation.
fn resolve_four_cc(archive: &Archive, dict: &Dictionary, key: &str) -> String {
    dict.get(key)
        .map(|v| {
            let r = archive.resolve(v);
            r.as_signed_integer()
                .map(four_cc)
                .or_else(|| r.as_string().map(str::to_string))
                .unwrap_or_default()
        })
        .unwrap_or_default()
}

/// Same as `resolve_four_cc` but without archive UID resolution (simple plists).
fn simple_four_cc(v: &Value) -> Option<String> {
    v.as_signed_integer()
        .map(four_cc)
        .or_else(|| v.as_string().map(str::to_string))
}

/// Convert a big-endian 4CC integer to a human-readable string.
/// e.g. `0x61756678` → `"aufx"`.
fn four_cc(n: i64) -> String {
    (n as u32)
        .to_be_bytes()
        .iter()
        .map(|&b| if b.is_ascii_graphic() { b as char } else { '?' })
        .collect()
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn four_cc_effect() {
        assert_eq!(four_cc(0x61756678), "aufx");
    }

    #[test]
    fn four_cc_instrument() {
        assert_eq!(four_cc(0x61756d75), "aumu");
    }

    #[test]
    fn not_a_package_error_on_file() {
        let dir = tempfile::tempdir().unwrap();
        let f = dir.path().join("test.logicx");
        std::fs::write(&f, b"not a dir").unwrap();
        assert!(matches!(read_logicx(&f), Err(LogicError::NotAPackage(_))));
    }

    #[test]
    fn missing_project_data_error() {
        let dir = tempfile::tempdir().unwrap();
        let pkg = dir.path().join("test.logicx");
        std::fs::create_dir(&pkg).unwrap();
        assert!(matches!(
            read_logicx(&pkg),
            Err(LogicError::ProjectDataNotFound(_))
        ));
    }

    #[test]
    fn simple_plist_round_trip() {
        use plist::{Dictionary, Value};

        // Build a minimal simple-format plist in memory and verify parsing.
        let mut plugin = Dictionary::new();
        plugin.insert("name".into(), Value::String("EQ Eight".into()));
        plugin.insert("manufacturer".into(), Value::String("Ableton".into()));
        plugin.insert("componentType".into(), Value::Integer(0x61756678i64.into()));
        plugin.insert("componentSubType".into(), Value::Integer(0x65713869i64.into()));
        plugin.insert("bypassState".into(), Value::Boolean(false));

        let mut track = Dictionary::new();
        track.insert("name".into(), Value::String("Drums".into()));
        track.insert("trackType".into(), Value::Integer(0i64.into()));
        track.insert(
            "plugins".into(),
            Value::Array(vec![Value::Dictionary(plugin)]),
        );

        let mut root = Dictionary::new();
        root.insert("tracks".into(), Value::Array(vec![Value::Dictionary(track)]));

        let tracks = extract_tracks_simple(Some(&root));
        assert_eq!(tracks.len(), 1);
        assert_eq!(tracks[0].name, "Drums");
        assert!(matches!(tracks[0].kind, TrackKind::Audio));
        assert_eq!(tracks[0].devices.len(), 1);
        assert_eq!(tracks[0].devices[0].name, "EQ Eight");
        assert_eq!(tracks[0].devices[0].component_type, "aufx");
    }
}
