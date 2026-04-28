//! Reader for Logic Pro projects (`.logicx` packages).
//!
//! A `.logicx` file is a macOS package — a directory that Finder presents as a
//! single file. The project state lives in binary files inside that directory.
//!
//! # Format (reverse-engineered, undocumented)
//! Logic Pro X (10.x+) writes `ProjectData` in a **proprietary chunked binary
//! format**, not a plist. The file begins with a 6-byte magic header:
//!
//! ```text
//! 23 47 c0 ab cf 09  ...  (Logic "Song" magic)
//! ```
//!
//! Data is organised as chunks whose 4-byte tags are stored in reversed byte
//! order (little-endian 4CC), e.g.:
//!
//! | Bytes   | Reversed tag | Meaning          |
//! |---------|--------------|------------------|
//! | `karT`  | `Trak`       | Track            |
//! | `qeSM`  | `MSeq`       | MIDI/Audio Seq.  |
//! | `gnoS`  | `SonG`       | Song root        |
//! | `tSnI`  | `InSt`       | Instrument/Score |
//! | `MneG`  | `GenM`       | Generic device   |
//!
//! Each `karT` chunk contains a `qeSM` sub-chunk. The track name is a
//! length-prefixed, null-terminated ASCII string embedded within `qeSM` at a
//! variable offset (~52–64 bytes from the `qeSM` tag). We locate it by
//! scanning for the first valid `len_u16LE + ascii_bytes + '\0'` triplet.
//!
//! Plugin/device data (AU state blobs) lives in other sub-chunks not yet
//! decoded; devices are returned empty in Phase 1.
//!
//! # Fallback
//! Older Logic 9 projects used a plain plist dictionary. The reader detects
//! the file type by magic bytes and falls back to plist parsing when needed.

use std::path::{Path, PathBuf};

use plist::{Dictionary, Value};
use serde::{Deserialize, Serialize};
use thiserror::Error;

// ─── Magic / chunk constants ──────────────────────────────────────────────────

/// First 6 bytes of every Logic Pro X `ProjectData` file.
const LOGIC_MAGIC: &[u8] = &[0x23, 0x47, 0xc0, 0xab, 0xcf, 0x09];

/// 4-byte reversed tag for a Track chunk (`Trak` LE).
const TRAK: &[u8] = b"karT";

/// 4-byte reversed tag for the MIDI/Audio Sequence sub-chunk (`MSeq` LE).
const QESM: &[u8] = b"qeSM";

// ─── Errors ───────────────────────────────────────────────────────────────────

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
    /// Logic version string, e.g. `"Logic Pro 11.2.2 (6387)"`.
    pub logic_version: String,
    pub tracks: Vec<LogicTrack>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct LogicTrack {
    pub name: String,
    pub kind: TrackKind,
    /// AU device chain. Empty in Phase 1 — proprietary plugin sub-format not
    /// yet decoded.
    pub devices: Vec<LogicDevice>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TrackKind {
    Audio,
    SoftwareInstrument,
    Aux,
    Master,
    /// Track type could not be determined from the proprietary binary.
    Unknown,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct LogicDevice {
    pub name: String,
    pub manufacturer: String,
    /// Four-character code string, e.g. `"aufx"` (effect) or `"aumu"` (instrument).
    pub component_type: String,
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

    let name = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("Untitled")
        .to_string();

    // ProjectInformation.plist is a standard binary plist — parse for metadata.
    let info_path = path.join("Resources").join("ProjectInformation.plist");
    let (project_name, logic_version) = parse_project_info(&info_path);

    let data_path = locate_project_data(path)?;
    let data = std::fs::read(&data_path)?;

    let tracks = if data.starts_with(LOGIC_MAGIC) {
        scan_proprietary_tracks(&data)
    } else {
        let root = plist::Value::from_reader(std::io::Cursor::new(&data))?;
        extract_tracks_plist(root.as_dictionary())
    };

    Ok(LogicProject {
        name: project_name.unwrap_or(name),
        logic_version,
        tracks,
    })
}

// ─── Package layout ───────────────────────────────────────────────────────────

fn locate_project_data(logicx: &Path) -> Result<PathBuf, LogicError> {
    // Logic 10.1+: data in a named alternative slot.
    let modern = logicx.join("Alternatives").join("000").join("ProjectData");
    if modern.exists() {
        return Ok(modern);
    }
    // Pre-10.1: data at the package root (note: older versions used lowercase).
    for name in &["projectData", "ProjectData"] {
        let legacy = logicx.join(name);
        if legacy.exists() {
            return Ok(legacy);
        }
    }
    Err(LogicError::ProjectDataNotFound(modern))
}

fn parse_project_info(plist_path: &Path) -> (Option<String>, String) {
    let value = match plist::Value::from_file(plist_path) {
        Ok(v) => v,
        Err(_) => return (None, String::new()),
    };
    let dict = match value.as_dictionary() {
        Some(d) => d,
        None => return (None, String::new()),
    };

    // VariantNames is a dict of slot-index → name; slot "0" is the default.
    let name = dict
        .get("VariantNames")
        .and_then(|v| v.as_dictionary())
        .and_then(|d| d.get("0"))
        .and_then(|v| v.as_string())
        .filter(|s| !s.is_empty() && !s.contains('{'))  // reject template placeholders
        .map(str::to_string);

    let version = dict
        .get("LastSavedFrom")
        .and_then(|v| v.as_string())
        .unwrap_or("")
        .to_string();

    (name, version)
}

// ─── Proprietary binary format parser ────────────────────────────────────────

/// Scan the raw `ProjectData` bytes for `karT` chunks and extract track names
/// from their embedded `qeSM` sub-chunks.
///
/// Track kind and AU device chain are not yet decoded (Phase 1 limitation).
fn scan_proprietary_tracks(data: &[u8]) -> Vec<LogicTrack> {
    let mut tracks = Vec::new();
    let mut pos = 0;

    while pos + 4 <= data.len() {
        if data[pos..].starts_with(TRAK) {
            let window_end = (pos + 512).min(data.len());
            let window = &data[pos..window_end];

            if let Some(qesm_rel) = find_subsequence(window, QESM) {
                let qesm_abs = pos + qesm_rel;
                if let Some(name) = extract_name_from_qesm(data, qesm_abs) {
                    tracks.push(LogicTrack {
                        name,
                        kind: TrackKind::Unknown,
                        devices: vec![],
                    });
                }
            }

            // Skip to the next potential track — minimum chunk size is ~50 bytes.
            pos += 50;
        } else {
            pos += 1;
        }
    }

    tracks
}

/// Extract the track name from a `qeSM` chunk.
///
/// The name is encoded as:
///   `<len: u16LE>  <len ASCII bytes>  <null: u8>`
///
/// This triplet appears at a variable offset (~48–72 bytes) within the chunk.
/// We scan forward from offset 48, testing each position for a valid triplet.
fn extract_name_from_qesm(data: &[u8], qesm_start: usize) -> Option<String> {
    let lo = qesm_start + 48;
    let hi = (qesm_start + 128).min(data.len().saturating_sub(3));

    for i in lo..hi {
        let len = u16::from_le_bytes([data[i], data[i + 1]]) as usize;
        if len < 2 || len > 63 {
            continue;
        }
        let end = i + 2 + len;
        if end >= data.len() {
            continue;
        }
        let name_bytes = &data[i + 2..end];
        let null_byte = data[end];

        if null_byte != 0 {
            continue;
        }
        if name_bytes.iter().all(|&b| b >= 0x20 && b <= 0x7e) {
            let name = String::from_utf8_lossy(name_bytes).to_string();
            return Some(name);
        }
    }

    None
}

/// Return the byte offset of `needle` within `haystack`, or `None`.
fn find_subsequence(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|w| w == needle)
}

// ─── Legacy plist format fallback ─────────────────────────────────────────────

fn extract_tracks_plist(dict: Option<&Dictionary>) -> Vec<LogicTrack> {
    dict.and_then(|d| d.get("tracks"))
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_dictionary())
                .filter_map(parse_track_plist)
                .collect()
        })
        .unwrap_or_default()
}

fn parse_track_plist(dict: &Dictionary) -> Option<LogicTrack> {
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
                .filter_map(parse_device_plist)
                .collect()
        })
        .unwrap_or_default();
    Some(LogicTrack { name, kind, devices })
}

fn parse_device_plist(dict: &Dictionary) -> Option<LogicDevice> {
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
            .and_then(|v| plist_four_cc(v))
            .unwrap_or_default(),
        component_subtype: dict
            .get("componentSubType")
            .and_then(|v| plist_four_cc(v))
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

fn plist_four_cc(v: &Value) -> Option<String> {
    v.as_signed_integer()
        .map(|n| four_cc(n))
        .or_else(|| v.as_string().map(str::to_string))
}

// ─── Helpers ──────────────────────────────────────────────────────────────────

/// Convert a big-endian 4CC integer to a human-readable string.
/// e.g. `0x61756678` → `"aufx"`.
fn four_cc(n: i64) -> String {
    (n as u32)
        .to_be_bytes()
        .iter()
        .map(|&b| if b.is_ascii_graphic() { b as char } else { '?' })
        .collect()
}

// ─── Unit tests ───────────────────────────────────────────────────────────────

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
    fn magic_detection() {
        let mut data = LOGIC_MAGIC.to_vec();
        data.extend_from_slice(&[0u8; 100]);
        assert!(data.starts_with(LOGIC_MAGIC));
        assert!(!b"bplist00".starts_with(LOGIC_MAGIC));
    }

    #[test]
    fn qesm_name_extraction_synthetic() {
        // Craft a synthetic qeSM block at offset 0 with a name at offset 52.
        let mut data = vec![0u8; 52];
        data.push(5);  // len low byte
        data.push(0);  // len high byte
        data.extend_from_slice(b"SYNTH");
        data.push(0);  // null terminator
        data.extend_from_slice(&[0u8; 20]);

        // extract_name_from_qesm expects the qeSM chunk start at the given offset.
        // We start scanning at qesm_start + 48, so we need 4 bytes of qeSM tag
        // plus the rest of our data.
        let mut full = b"qeSM".to_vec();
        full.extend_from_slice(&[0u8; 44]);  // filler so name lands at qeSM+48+4
        full.push(5);
        full.push(0);
        full.extend_from_slice(b"SYNTH");
        full.push(0);
        full.extend_from_slice(&[0u8; 20]);

        let name = extract_name_from_qesm(&full, 0);
        assert_eq!(name, Some("SYNTH".to_string()));
    }

    #[test]
    fn plist_round_trip() {
        use plist::{Dictionary, Value};

        let mut plugin = Dictionary::new();
        plugin.insert("name".into(), Value::String("EQ Eight".into()));
        plugin.insert("manufacturer".into(), Value::String("Ableton".into()));
        plugin.insert("componentType".into(), Value::Integer(0x61756678i64.into()));
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

        let tracks = extract_tracks_plist(Some(&root));
        assert_eq!(tracks.len(), 1);
        assert_eq!(tracks[0].name, "Drums");
        assert_eq!(tracks[0].devices[0].name, "EQ Eight");
        assert_eq!(tracks[0].devices[0].component_type, "aufx");
    }

    // ─── Integration test (macOS only) ────────────────────────────────────────

    /// Reads a real `.logicx` project on the developer's Mac and validates the
    /// output. The path is the "Save and collect" template, which is always
    /// present after installing Logic Pro.
    ///
    /// Skipped on non-macOS platforms.
    #[test]
    #[cfg(target_os = "macos")]
    fn integration_save_and_collect() {
        let path = std::path::Path::new(
            "/Users/jonathanamir/Music/Audio Music Apps/Project Templates/Save and collect.logicx",
        );
        if !path.exists() {
            eprintln!("integration test skipped: project not found at {path:?}");
            return;
        }

        let project = read_logicx(path).expect("read_logicx must not error on a real project");

        assert!(
            !project.logic_version.is_empty(),
            "logic_version should be populated from ProjectInformation.plist"
        );
        assert!(
            project.logic_version.contains("Logic"),
            "logic_version should contain 'Logic', got {:?}",
            project.logic_version
        );
        assert!(
            !project.tracks.is_empty(),
            "at least one track should be extracted from a real project"
        );

        let names: Vec<&str> = project.tracks.iter().map(|t| t.name.as_str()).collect();
        eprintln!("logic_version: {}", project.logic_version);
        eprintln!("track count:   {}", project.tracks.len());
        eprintln!("track names:   {names:?}");

        // Verify at least one track has a non-empty, non-trivial name.
        let has_real_name = project
            .tracks
            .iter()
            .any(|t| t.name.len() >= 2 && t.name != "Untitled");
        assert!(has_real_name, "at least one track should have a real name");
    }

    #[test]
    #[cfg(target_os = "macos")]
    fn integration_library_project() {
        let path = std::path::Path::new(
            "/Users/jonathanamir/Music/Logic Pro Library.bundle/Projects/Live Loop Grids/Skyline Masher.logicx",
        );
        if !path.exists() {
            eprintln!("integration test skipped: project not found");
            return;
        }

        let project = read_logicx(path).expect("must not error on Skyline Masher");
        eprintln!("Skyline Masher — version: {}, tracks: {}", project.logic_version, project.tracks.len());
        eprintln!("names: {:?}", project.tracks.iter().map(|t| &t.name).collect::<Vec<_>>());

        // Library templates should have at least one track.
        assert!(!project.tracks.is_empty());
    }
}
