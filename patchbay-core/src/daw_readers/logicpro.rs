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
//! order (little-endian 4CC).
//!
//! # Third-party AU plugin state — investigation results (2026-04-30)
//! Analysis of `take me away v0.logicx` (a real production session with
//! Omnisphere 3 and Soundtoys Little Plate) revealed the following:
//!
//! ## Plugin slot marker: `UCuA`
//! Every plugin slot (built-in or third-party) is preceded by a `UCuA` 4-byte
//! tag. What follows determines the plugin type:
//!
//! - **Built-in Logic effects** (`GAME`/`GAMETSPP`): float parameter arrays,
//!   no AU state blob. These appear immediately after `UCuA`.
//! - **NSKeyedArchiver blobs** (`bplist00`): Smart Controls / MIDI layer
//!   settings (`MAKeyboardLayer`, `MAPlugInParameterMapping`). Not AU state.
//! - **Third-party AU plugins**: an inline `.aupreset`-format XML plist
//!   (`<?xml...`) that contains the complete AU state and component identity.
//!
//! ## Embedded `.aupreset` plist structure
//! Each third-party AU block holds a standard Apple `.aupreset` XML plist:
//! ```xml
//! <dict>
//!   <key>type</key>         <integer>1635083896</integer>  <!-- "aufx" -->
//!   <key>subtype</key>      <integer>1280330808</integer>  <!-- "LPL8" -->
//!   <key>manufacturer</key> <integer>1398042489</integer>  <!-- "SToy" -->
//!   <key>data</key>         <data>...</data>               <!-- ClassInfo blob -->
//!   <!-- vendor-specific extra keys, e.g. soundtoys-data -->
//! </dict>
//! ```
//! The `type`/`subtype`/`manufacturer` integers are big-endian 4CC codes that
//! uniquely identify the AU component. The `data` field is the standard AU
//! `kAudioUnitProperty_ClassInfo` blob.
//!
//! ## Known AU components in the test session
//! | Plugin          | type   | subtype | mfr  |
//! |-----------------|--------|---------|------|
//! | Soundtoys LP8   | `aufx` | `LPL8`  | `SToy` |
//! | Omnisphere 3    | `aumu` | `Ambr`  | `GOSW` |
//!
//! ## Track → plugin association
//! `karT`/`qeSM` chunks give track names; `UCuA` chunks give plugin slots.
//! The reader correlates them by file position: each `UCuA` is assigned to
//! the nearest preceding `karT`. Tracks with no plugins are discarded before
//! returning. This is a positional heuristic — not a spec-level guarantee —
//! but it produces the correct result for standard sequential project layouts.
//! Plugins that appear before the first `karT` (rare) fall back to
//! `LogicProject::all_devices`.
//!
//! # Fallback
//! Older Logic 9 projects used a plain plist dictionary. The reader detects
//! the file type by magic bytes and falls back to plist parsing when needed.

use std::path::{Path, PathBuf};

use plist::{Dictionary, Value};
use serde::{Deserialize, Serialize};
use thiserror::Error;

// ─── Magic / chunk constants ──────────────────────────────────────────────────

/// First 4 bytes of every Logic Pro X `ProjectData` file.
/// Bytes 4-5 vary between Logic versions/projects (e.g. 0xcf vs 0xd0),
/// so only the stable prefix is checked.
const LOGIC_MAGIC: &[u8] = &[0x23, 0x47, 0xc0, 0xab];

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
    /// All third-party AU plugin instances found in the project, extracted from
    /// embedded `.aupreset` plists. Track association is not yet resolved, so
    /// these are returned as a flat list regardless of which track they belong to.
    pub all_devices: Vec<LogicDevice>,
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

#[derive(Debug, Clone, Serialize, Deserialize)]
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

    let (tracks, all_devices) = if data.starts_with(LOGIC_MAGIC) {
        let traks = collect_trak_entries(&data);
        let ucuas = collect_ucua_entries(&data);
        associate_plugins_to_tracks(traks, ucuas)
    } else {
        let root = plist::Value::from_reader(std::io::Cursor::new(&data))?;
        (extract_tracks_plist(root.as_dictionary()), vec![])
    };

    Ok(LogicProject {
        name: project_name.unwrap_or(name),
        logic_version,
        tracks,
        all_devices,
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

struct TrakEntry {
    pos: usize,
    name: String,
}

enum UcuaKind {
    ThirdParty(LogicDevice),
    BuiltIn(String),
}

struct UcuaEntry {
    pos: usize,
    kind: UcuaKind,
}

/// Collect all `karT` chunk positions and their track names from `qeSM` sub-chunks.
fn collect_trak_entries(data: &[u8]) -> Vec<TrakEntry> {
    let mut entries = Vec::new();
    let mut pos = 0;

    while pos + 4 <= data.len() {
        if data[pos..].starts_with(TRAK) {
            let window_end = (pos + 512).min(data.len());
            if let Some(qesm_rel) = find_subsequence(&data[pos..window_end], QESM) {
                let qesm_abs = pos + qesm_rel;
                if let Some(name) = extract_name_from_qesm(data, qesm_abs) {
                    entries.push(TrakEntry { pos, name });
                }
            }
            pos += 50;
        } else {
            pos += 1;
        }
    }

    entries
}

/// Collect all `UCuA` plugin slot positions, capturing both built-in Logic
/// effects (`GAME` tag) and third-party AU plugins (embedded XML plist).
/// Smart Controls blobs (`bplist00`) are skipped — they are not plugin slots.
fn collect_ucua_entries(data: &[u8]) -> Vec<UcuaEntry> {
    let mut entries = Vec::new();
    let mut search_from = 0;

    loop {
        let Some(rel) = find_subsequence(&data[search_from..], b"UCuA") else { break };
        let ucua_pos = search_from + rel;
        let window_end = (ucua_pos + 51_200).min(data.len());
        let window = &data[ucua_pos..window_end];

        let game_pos    = find_subsequence(window, b"GAME");
        let bplist_pos  = find_subsequence(window, b"bplist00");
        let xml_pos     = find_subsequence(window, b"<?xml");

        // Determine which signal appears first in the file.
        let first: Option<(&str, usize)> = [
            game_pos.map(|p|   ("game",   p)),
            bplist_pos.map(|p| ("bplist", p)),
            xml_pos.map(|p|    ("xml",    p)),
        ]
        .into_iter()
        .flatten()
        .min_by_key(|&(_, p)| p);

        match first {
            Some(("xml", xml_rel)) => {
                let xml_start = ucua_pos + xml_rel;
                let Some(end_rel) = find_subsequence(&data[xml_start..], b"</plist>") else {
                    search_from = ucua_pos + 4;
                    continue;
                };
                let plist_end = xml_start + end_rel + 8;
                let plist_bytes = &data[xml_start..plist_end];
                let pre_xml = &data[ucua_pos..xml_start];
                if let Some(device) = parse_aupreset_plist(plist_bytes, pre_xml) {
                    entries.push(UcuaEntry { pos: ucua_pos, kind: UcuaKind::ThirdParty(device) });
                }
                search_from = plist_end;
            }
            Some(("game", game_rel)) => {
                let between_end = (ucua_pos + game_rel).min(data.len());
                let between = &data[ucua_pos + 4..between_end];
                let name = extract_plugin_name(between)
                    .unwrap_or_else(|| "Logic built-in".to_string());
                entries.push(UcuaEntry { pos: ucua_pos, kind: UcuaKind::BuiltIn(name) });
                search_from = ucua_pos + 4;
            }
            // bplist00 (Smart Controls) or no signal — not a plugin slot.
            _ => {
                search_from = ucua_pos + 4;
            }
        }
    }

    entries
}

/// Associate each `UCuA` entry with its nearest preceding `karT` entry by file
/// position, then return only tracks that have at least one plugin.
///
/// Plugins that appear before the first TRAK (rare) are returned separately as
/// the fallback `all_devices` list used by the legacy "Plugin Chain" display.
fn associate_plugins_to_tracks(
    mut traks: Vec<TrakEntry>,
    ucuas: Vec<UcuaEntry>,
) -> (Vec<LogicTrack>, Vec<LogicDevice>) {
    traks.sort_unstable_by_key(|t| t.pos);

    let mut track_devices: Vec<Vec<LogicDevice>> = vec![vec![]; traks.len()];
    let mut unassociated: Vec<LogicDevice> = Vec::new();

    for ucua in ucuas {
        let device = match ucua.kind {
            UcuaKind::ThirdParty(d) => d,
            UcuaKind::BuiltIn(name) => LogicDevice {
                name,
                manufacturer: "Logic".to_string(),
                component_type: String::new(),
                component_subtype: String::new(),
                bypassed: false,
                state: vec![],
            },
        };
        // partition_point gives the first index where trak.pos >= ucua.pos,
        // so idx-1 is the last TRAK that starts before this UCuA.
        let idx = traks.partition_point(|t| t.pos < ucua.pos);
        if idx == 0 {
            unassociated.push(device);
        } else {
            track_devices[idx - 1].push(device);
        }
    }

    let tracks = traks
        .into_iter()
        .zip(track_devices)
        .filter(|(_, devices)| !devices.is_empty())
        .map(|(trak, devices)| LogicTrack {
            name: trak.name,
            kind: TrackKind::Unknown,
            devices,
        })
        .collect();

    (tracks, unassociated)
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

/// Parse an embedded `.aupreset` XML plist and return a `LogicDevice`.
fn parse_aupreset_plist(bytes: &[u8], pre_xml: &[u8]) -> Option<LogicDevice> {
    let cursor = std::io::Cursor::new(bytes);
    let value = plist::Value::from_reader(cursor).ok()?;
    let dict = value.as_dictionary()?;

    let type_i = dict.get("type")?.as_signed_integer()?;
    let subtype_i = dict.get("subtype")?.as_signed_integer()?;
    let mfr_i = dict.get("manufacturer")?.as_signed_integer()?;

    let component_type = four_cc(type_i);
    let component_subtype = four_cc(subtype_i);
    let manufacturer_cc = four_cc(mfr_i);

    // Derive a human-readable name: Soundtoys stores "WIDGET = <name>" in a
    // vendor-specific key; for all others we try the binary context.
    let name = soundtoys_widget_name(dict)
        .or_else(|| extract_plugin_name(pre_xml))
        .filter(|n| !n.is_empty() && n != "Untitled")
        .unwrap_or_else(|| component_subtype.clone());

    // The full plist bytes IS the standard .aupreset payload — store verbatim.
    let state = bytes.to_vec();

    Some(LogicDevice {
        name,
        manufacturer: manufacturer_cc,
        component_type,
        component_subtype,
        bypassed: false,
        state,
    })
}

/// Extract the plugin name from Soundtoys' `soundtoys-data` string.
/// Format: `"WIDGET = Little Plate;VERSION = 4;..."`
fn soundtoys_widget_name(dict: &plist::Dictionary) -> Option<String> {
    let data = dict.get("soundtoys-data")?.as_string()?;
    data.split(';')
        .find(|part| part.trim_start().starts_with("WIDGET"))
        .and_then(|part| part.splitn(2, '=').nth(1))
        .map(|v| v.trim().to_string())
        .filter(|s| !s.is_empty())
}

/// Scan the bytes between a `UCuA` marker and an XML plist for the plugin name.
///
/// Logic stores the plugin name as a printable ASCII string a few bytes before
/// the 12-character base64-encoded AU component identifier. We collect all
/// printable runs, drop the component ID (12-char all-alphanumeric), and return
/// the last remaining candidate.
fn extract_plugin_name(pre_xml: &[u8]) -> Option<String> {
    const SKIP_FRAGMENTS: &[&str] = &["Untitled", "aupreset", "#Custom", "#default", "46ia"];

    // Collect all runs of printable ASCII (0x20–0x7e), length >= 4.
    let text: String = pre_xml
        .iter()
        .map(|&b| if (0x20..=0x7e).contains(&b) { b as char } else { '\x00' })
        .collect();

    text.split('\x00')
        .map(str::trim)
        .filter(|s| s.len() >= 4)
        .filter(|s| s.chars().any(|c| c.is_alphabetic()))
        .filter(|s| !SKIP_FRAGMENTS.iter().any(|skip| s.contains(skip)))
        // Drop 12-char all-alphanumeric strings — those are base64-encoded component IDs.
        .filter(|s| !(s.len() == 12 && s.chars().all(|c| c.is_ascii_alphanumeric())))
        .last()
        .map(str::to_string)
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
        // Verify both known byte-4 variants are detected by the 4-byte prefix.
        let mut data_cf = LOGIC_MAGIC.to_vec();
        data_cf.extend_from_slice(&[0xcf, 0x09, 0x03, 0x00]);
        assert!(data_cf.starts_with(LOGIC_MAGIC));

        let mut data_d0 = LOGIC_MAGIC.to_vec();
        data_d0.extend_from_slice(&[0xd0, 0x09, 0x03, 0x00]);
        assert!(data_d0.starts_with(LOGIC_MAGIC));

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
    fn integration_real_session_third_party_au() {
        let path = std::path::Path::new(
            "/Users/jonathanamir/projects/take me away/take me away v0.logicx",
        );
        if !path.exists() {
            eprintln!("integration test skipped: project not found");
            return;
        }

        let project = read_logicx(path).expect("must not error on real session");
        let all_track_devices: Vec<&LogicDevice> = project.tracks.iter()
            .flat_map(|t| t.devices.iter())
            .collect();
        let all_devices_any: Vec<&LogicDevice> = all_track_devices.iter()
            .copied()
            .chain(project.all_devices.iter())
            .collect();

        eprintln!("take me away — version: {}, tracks (with plugins): {}, unassociated: {}",
            project.logic_version, project.tracks.len(), project.all_devices.len());
        for t in &project.tracks {
            eprintln!("  track {:?}: {} plugin(s)", t.name, t.devices.len());
            for d in &t.devices {
                eprintln!("    device: {:?} type={} sub={} mfr={}", d.name, d.component_type, d.component_subtype, d.manufacturer);
            }
        }

        assert!(
            !all_devices_any.is_empty(),
            "real session should have third-party AU devices"
        );

        for d in all_track_devices.iter().filter(|d| !d.state.is_empty()) {
            assert!(!d.component_type.is_empty());
            assert!(!d.component_subtype.is_empty());
        }

        // Expect at least Soundtoys Little Plate and Omnisphere (in tracks or fallback)
        let has_soundtoys = all_devices_any.iter().any(|d| d.manufacturer == "SToy");
        let has_omnisphere = all_devices_any.iter().any(|d| d.component_subtype == "Ambr");
        assert!(has_soundtoys, "expected Soundtoys device");
        assert!(has_omnisphere, "expected Omnisphere device");
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
