//! Reader for DAWproject files (`.dawproject`).
//!
//! DAWproject is an open XML container format co-developed by Bitwig and PreSonus.
//! Bitwig Studio exports it natively via File → Export DAWproject; Studio One 6.5+
//! reads and writes it. One reader serves both DAWs.
//!
//! # Format
//! A `.dawproject` file is a ZIP archive containing:
//! - `project.xml` — full project structure in the DAWproject XML schema
//! - State files referenced by `<State path="..."/>` elements at arbitrary paths inside the ZIP
//! - Optional media files (audio, MIDI)
//!
//! # Plugin identification
//! - **VST3**: `<Vst3Plugin deviceID="{GUID}" name="..." vendor="...">` — class ID GUID
//! - **VST2**: `<Vst2Plugin uniqueId="1936880749" name="..." vendor="...">` — 4-byte UID
//! - **AU**: `<AuPlugin type="4CC" subType="4CC" manufacturer="4CC">` — component codes
//! - **CLAP**: `<ClapPlugin id="com.vendor.plugin" name="..." vendor="...">`
//! - **Built-in**: any other element inside `<Devices>` (Compressor, Equalizer, etc.)
//!
//! # State blobs
//! Plugin state files are stored inside the ZIP and referenced via `path` attributes on
//! `<State>` child elements. This reader reads them and re-encodes as base64 strings so
//! storage is uniform with other DAW readers. A missing file produces `opaque_state: None`
//! rather than an error, since some exporters omit state for bypassed or default devices.

use std::io::{self, Read, Seek};
use std::path::Path;

use serde::{Deserialize, Serialize};
use thiserror::Error;

// ─── Errors ──────────────────────────────────────────────────────────────────

#[derive(Debug, Error)]
pub enum DawProjectError {
    #[error("I/O error: {0}")]
    Io(#[from] io::Error),
    #[error("ZIP error: {0}")]
    Zip(#[from] zip::result::ZipError),
    #[error("XML error: {0}")]
    Xml(#[from] roxmltree::Error),
    #[error("Format error: {0}")]
    Format(String),
}

// ─── Public types ────────────────────────────────────────────────────────────

/// Parsed DAWproject file.
#[derive(Debug, Serialize, Deserialize)]
pub struct DawProject {
    /// Schema version from `<Project version="...">`, e.g. `"1.0"`.
    pub version: String,
    pub application: Option<ApplicationInfo>,
    pub tracks: Vec<Track>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ApplicationInfo {
    pub name: String,
    pub version: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Track {
    pub id: Option<String>,
    pub name: String,
    /// Hex color string, e.g. `"#FFA500"`.
    pub color: Option<String>,
    /// DAWproject content type bitmask, e.g. `"audio notes"`.
    pub content_type: Option<String>,
    pub channel: Option<Channel>,
    /// Nested sub-tracks (group / folder tracks).
    pub children: Vec<Track>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Channel {
    pub id: Option<String>,
    /// DAWproject channel role: `"regular"`, `"master"`, `"effect"`, `"sends"`.
    pub role: Option<String>,
    pub audio_channels: Option<u32>,
    pub devices: Vec<Device>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Device {
    pub name: String,
    pub is_enabled: bool,
    pub kind: DeviceKind,
    /// Plugin state read from the ZIP and base64-encoded. `None` when no `<State>` is present
    /// or the referenced file is missing from the archive.
    pub opaque_state: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "format")]
pub enum DeviceKind {
    Vst3 {
        /// Class ID GUID: `{XXXXXXXX-XXXX-XXXX-XXXX-XXXXXXXXXXXX}`.
        device_id: String,
        vendor: Option<String>,
        plugin_version: Option<String>,
    },
    Vst2 {
        /// 4-byte plugin UID as a signed integer (matches VST2 `uniqueID` field).
        unique_id: i64,
        vendor: Option<String>,
        plugin_version: Option<String>,
    },
    Au {
        /// `type` 4CC (e.g. `"aufx"`, `"aumu"`).
        type_code: String,
        /// `subType` 4CC.
        sub_type: String,
        /// `manufacturer` 4CC (e.g. `"appl"`, `"FabF"`).
        manufacturer: String,
    },
    Clap {
        /// Reverse-DNS plugin ID (e.g. `"com.xferrecords.serum"`).
        id: String,
        vendor: Option<String>,
        plugin_version: Option<String>,
    },
    /// Any other element inside `<Devices>` — built-in processors, routing utilities, etc.
    Builtin,
}

// ─── Public API ──────────────────────────────────────────────────────────────

/// Open and parse a `.dawproject` archive.
pub fn read_dawproject(path: &Path) -> Result<DawProject, DawProjectError> {
    let file = std::fs::File::open(path)?;
    let mut zip = zip::ZipArchive::new(file)?;
    let xml = read_zip_text(&mut zip, "project.xml")?;
    parse_project_xml(&xml, &mut |state_path| load_state_from_zip(&mut zip, state_path))
}

// ─── Core XML parser (pub(crate) for tests) ──────────────────────────────────

/// Parse `project.xml` content.
///
/// `load_state` is called for each `<State path="..."/>` found; it receives the path
/// string and returns the raw bytes (or `None` to skip). This indirection lets tests
/// inject in-memory state without a real ZIP archive.
pub(crate) fn parse_project_xml(
    xml: &str,
    load_state: &mut impl FnMut(&str) -> Option<Vec<u8>>,
) -> Result<DawProject, DawProjectError> {
    let doc = roxmltree::Document::parse(xml)?;
    let root = doc.root_element();

    let version = root.attribute("version").unwrap_or("").to_string();

    let application = root.children().find(|n| n.has_tag_name("Application")).map(|n| {
        ApplicationInfo {
            name: n.attribute("name").unwrap_or("").to_string(),
            version: n.attribute("version").unwrap_or("").to_string(),
        }
    });

    let tracks = root
        .children()
        .find(|n| n.has_tag_name("Structure"))
        .map(|s| {
            s.children()
                .filter(|n| n.has_tag_name("Track"))
                .map(|n| parse_track(n, load_state))
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();

    Ok(DawProject { version, application, tracks })
}

// ─── Track / channel / device ────────────────────────────────────────────────

fn parse_track(
    node: roxmltree::Node,
    load_state: &mut impl FnMut(&str) -> Option<Vec<u8>>,
) -> Track {
    let id = node.attribute("id").map(str::to_string);
    let name = node.attribute("name").unwrap_or("").to_string();
    let color = node.attribute("color").map(str::to_string);
    let content_type = node.attribute("contentType").map(str::to_string);

    let channel = node
        .children()
        .find(|n| n.has_tag_name("Channel"))
        .map(|n| parse_channel(n, load_state));

    let children = node
        .children()
        .filter(|n| n.has_tag_name("Track"))
        .map(|n| parse_track(n, load_state))
        .collect();

    Track { id, name, color, content_type, channel, children }
}

fn parse_channel(
    node: roxmltree::Node,
    load_state: &mut impl FnMut(&str) -> Option<Vec<u8>>,
) -> Channel {
    let id = node.attribute("id").map(str::to_string);
    let role = node.attribute("role").map(str::to_string);
    let audio_channels = node.attribute("audioChannels").and_then(|v| v.parse().ok());

    let devices = node
        .children()
        .find(|n| n.has_tag_name("Devices"))
        .map(|devs| {
            devs.children()
                .filter(|n| n.is_element())
                .map(|n| parse_device(n, load_state))
                .collect()
        })
        .unwrap_or_default();

    Channel { id, role, audio_channels, devices }
}

fn parse_device(
    node: roxmltree::Node,
    load_state: &mut impl FnMut(&str) -> Option<Vec<u8>>,
) -> Device {
    let tag = node.tag_name().name();
    let name = node.attribute("name").unwrap_or(tag).to_string();
    let is_enabled = device_is_enabled(node);
    let opaque_state = read_state(node, load_state);

    let kind = match tag {
        "Vst3Plugin" => DeviceKind::Vst3 {
            device_id: node.attribute("deviceID").unwrap_or("").to_string(),
            vendor: node.attribute("vendor").map(str::to_string),
            plugin_version: node.attribute("pluginVersion").map(str::to_string),
        },
        "Vst2Plugin" => DeviceKind::Vst2 {
            unique_id: node.attribute("uniqueId").and_then(|v| v.parse().ok()).unwrap_or(0),
            vendor: node.attribute("vendor").map(str::to_string),
            plugin_version: node.attribute("pluginVersion").map(str::to_string),
        },
        "AuPlugin" => DeviceKind::Au {
            type_code: node.attribute("type").unwrap_or("").to_string(),
            sub_type: node.attribute("subType").unwrap_or("").to_string(),
            manufacturer: node.attribute("manufacturer").unwrap_or("").to_string(),
        },
        "ClapPlugin" => DeviceKind::Clap {
            id: node.attribute("id").unwrap_or("").to_string(),
            vendor: node.attribute("vendor").map(str::to_string),
            plugin_version: node.attribute("pluginVersion").map(str::to_string),
        },
        _ => DeviceKind::Builtin,
    };

    Device { name, is_enabled, kind, opaque_state }
}

// ─── Element helpers ─────────────────────────────────────────────────────────

fn device_is_enabled(node: roxmltree::Node) -> bool {
    node.children()
        .find(|n| n.has_tag_name("Enabled"))
        .and_then(|n| n.attribute("value"))
        .map(|v| !v.eq_ignore_ascii_case("false"))
        .unwrap_or(true)
}

fn read_state(
    node: roxmltree::Node,
    load_state: &mut impl FnMut(&str) -> Option<Vec<u8>>,
) -> Option<String> {
    let path = node
        .children()
        .find(|n| n.has_tag_name("State"))
        .and_then(|n| n.attribute("path"))?;

    load_state(path).map(|bytes| base64_encode(&bytes))
}

// ─── ZIP helpers ─────────────────────────────────────────────────────────────

fn read_zip_text<R: Read + Seek>(
    zip: &mut zip::ZipArchive<R>,
    name: &str,
) -> Result<String, DawProjectError> {
    let mut entry = zip.by_name(name)?;
    let mut text = String::new();
    entry.read_to_string(&mut text)?;
    Ok(text)
}

fn load_state_from_zip<R: Read + Seek>(
    zip: &mut zip::ZipArchive<R>,
    path: &str,
) -> Option<Vec<u8>> {
    let mut entry = zip.by_name(path).ok()?;
    let mut bytes = Vec::new();
    entry.read_to_end(&mut bytes).ok()?;
    Some(bytes)
}

// ─── Base64 encoder (no external dep) ────────────────────────────────────────

const B64_ALPHABET: &[u8; 64] =
    b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

fn base64_encode(data: &[u8]) -> String {
    let mut out = String::with_capacity((data.len() + 2) / 3 * 4);
    for chunk in data.chunks(3) {
        let b0 = chunk[0] as usize;
        let b1 = chunk.get(1).copied().unwrap_or(0) as usize;
        let b2 = chunk.get(2).copied().unwrap_or(0) as usize;
        let n = (b0 << 16) | (b1 << 8) | b2;
        out.push(B64_ALPHABET[(n >> 18) & 0x3F] as char);
        out.push(B64_ALPHABET[(n >> 12) & 0x3F] as char);
        out.push(if chunk.len() > 1 { B64_ALPHABET[(n >> 6) & 0x3F] as char } else { '=' });
        out.push(if chunk.len() > 2 { B64_ALPHABET[n & 0x3F] as char } else { '=' });
    }
    out
}

// ─── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn no_state(_: &str) -> Option<Vec<u8>> {
        None
    }

    const PROJECT_XML: &str = r##"<?xml version="1.0" encoding="UTF-8"?>
<Project version="1.0">
  <Application name="Bitwig Studio" version="5.1.0"/>
  <Transport>
    <Tempo>120.0</Tempo>
    <TimeSignature numerator="4" denominator="4"/>
  </Transport>
  <Structure>
    <Track contentType="audio notes" id="t0" name="Drums" color="#FF8800">
      <Channel audioChannels="2" role="regular" id="ch0">
        <Devices>
          <Vst3Plugin deviceID="{D8D91CE4-6A7D-4670-8BDE-4B688EC18B43}"
                      name="Pro-Q 3" pluginVersion="3.21" vendor="FabFilter">
            <Enabled value="true"/>
            <State path="plugins/proq3.vstpreset"/>
          </Vst3Plugin>
          <Vst2Plugin uniqueId="1936880749" name="Serum"
                      pluginVersion="1.3.0b8" vendor="Xfer Records">
            <Enabled value="false"/>
            <State path="plugins/serum.fxp"/>
          </Vst2Plugin>
        </Devices>
      </Channel>
    </Track>
    <Track contentType="audio" id="t1" name="Bus">
      <Track contentType="audio" id="t2" name="Nested">
        <Channel audioChannels="2" role="regular" id="ch2">
          <Devices/>
        </Channel>
      </Track>
      <Channel audioChannels="2" role="master" id="ch1">
        <Devices>
          <AuPlugin type="aufx" subType="lmtr" manufacturer="appl" name="AUPeakLimiter">
            <Enabled value="true"/>
          </AuPlugin>
          <ClapPlugin id="com.xferrecords.serum" name="Serum CLAP" vendor="Xfer Records">
            <Enabled value="true"/>
          </ClapPlugin>
          <Compressor name="Compressor">
            <Enabled value="true"/>
          </Compressor>
        </Devices>
      </Channel>
    </Track>
  </Structure>
</Project>"##;

    #[test]
    fn project_version_and_application() {
        let p = parse_project_xml(PROJECT_XML, &mut no_state).unwrap();
        assert_eq!(p.version, "1.0");
        let app = p.application.unwrap();
        assert_eq!(app.name, "Bitwig Studio");
        assert_eq!(app.version, "5.1.0");
    }

    #[test]
    fn top_level_track_count() {
        let p = parse_project_xml(PROJECT_XML, &mut no_state).unwrap();
        assert_eq!(p.tracks.len(), 2);
    }

    #[test]
    fn track_attributes() {
        let p = parse_project_xml(PROJECT_XML, &mut no_state).unwrap();
        let t = &p.tracks[0];
        assert_eq!(t.name, "Drums");
        assert_eq!(t.id.as_deref(), Some("t0"));
        assert_eq!(t.color.as_deref(), Some("#FF8800"));
        assert_eq!(t.content_type.as_deref(), Some("audio notes"));
    }

    #[test]
    fn nested_tracks() {
        let p = parse_project_xml(PROJECT_XML, &mut no_state).unwrap();
        let bus = &p.tracks[1];
        assert_eq!(bus.name, "Bus");
        assert_eq!(bus.children.len(), 1);
        assert_eq!(bus.children[0].name, "Nested");
    }

    #[test]
    fn channel_role_and_audio_channels() {
        let p = parse_project_xml(PROJECT_XML, &mut no_state).unwrap();
        let ch = p.tracks[0].channel.as_ref().unwrap();
        assert_eq!(ch.role.as_deref(), Some("regular"));
        assert_eq!(ch.audio_channels, Some(2));
    }

    #[test]
    fn vst3_plugin_fields() {
        let p = parse_project_xml(PROJECT_XML, &mut no_state).unwrap();
        let dev = &p.tracks[0].channel.as_ref().unwrap().devices[0];
        assert_eq!(dev.name, "Pro-Q 3");
        assert!(dev.is_enabled);
        match &dev.kind {
            DeviceKind::Vst3 { device_id, vendor, plugin_version } => {
                assert_eq!(device_id, "{D8D91CE4-6A7D-4670-8BDE-4B688EC18B43}");
                assert_eq!(vendor.as_deref(), Some("FabFilter"));
                assert_eq!(plugin_version.as_deref(), Some("3.21"));
            }
            _ => panic!("expected Vst3"),
        }
    }

    #[test]
    fn vst2_bypassed_plugin() {
        let p = parse_project_xml(PROJECT_XML, &mut no_state).unwrap();
        let dev = &p.tracks[0].channel.as_ref().unwrap().devices[1];
        assert_eq!(dev.name, "Serum");
        assert!(!dev.is_enabled);
        match &dev.kind {
            DeviceKind::Vst2 { unique_id, vendor, .. } => {
                assert_eq!(*unique_id, 1936880749);
                assert_eq!(vendor.as_deref(), Some("Xfer Records"));
            }
            _ => panic!("expected Vst2"),
        }
    }

    #[test]
    fn au_plugin_four_ccs() {
        let p = parse_project_xml(PROJECT_XML, &mut no_state).unwrap();
        let bus_ch = p.tracks[1].channel.as_ref().unwrap();
        let dev = &bus_ch.devices[0];
        assert_eq!(dev.name, "AUPeakLimiter");
        match &dev.kind {
            DeviceKind::Au { type_code, sub_type, manufacturer } => {
                assert_eq!(type_code, "aufx");
                assert_eq!(sub_type, "lmtr");
                assert_eq!(manufacturer, "appl");
            }
            _ => panic!("expected Au"),
        }
    }

    #[test]
    fn clap_plugin_id() {
        let p = parse_project_xml(PROJECT_XML, &mut no_state).unwrap();
        let bus_ch = p.tracks[1].channel.as_ref().unwrap();
        let dev = &bus_ch.devices[1];
        assert_eq!(dev.name, "Serum CLAP");
        match &dev.kind {
            DeviceKind::Clap { id, vendor, .. } => {
                assert_eq!(id, "com.xferrecords.serum");
                assert_eq!(vendor.as_deref(), Some("Xfer Records"));
            }
            _ => panic!("expected Clap"),
        }
    }

    #[test]
    fn builtin_device_kind() {
        let p = parse_project_xml(PROJECT_XML, &mut no_state).unwrap();
        let bus_ch = p.tracks[1].channel.as_ref().unwrap();
        let dev = &bus_ch.devices[2];
        assert_eq!(dev.name, "Compressor");
        assert!(matches!(dev.kind, DeviceKind::Builtin));
    }

    #[test]
    fn empty_devices_list() {
        let p = parse_project_xml(PROJECT_XML, &mut no_state).unwrap();
        let nested_ch = p.tracks[1].children[0].channel.as_ref().unwrap();
        assert!(nested_ch.devices.is_empty());
    }

    #[test]
    fn state_blob_injected_via_loader() {
        let state_bytes: &[u8] = &[0x00, 0x01, 0x02, 0x03];
        let mut loader = |path: &str| {
            if path == "plugins/proq3.vstpreset" { Some(state_bytes.to_vec()) } else { None }
        };
        let p = parse_project_xml(PROJECT_XML, &mut loader).unwrap();
        let dev = &p.tracks[0].channel.as_ref().unwrap().devices[0];
        assert_eq!(dev.opaque_state.as_deref(), Some("AAECAw=="));
        // Serum's path returns None → opaque_state is None
        let serum = &p.tracks[0].channel.as_ref().unwrap().devices[1];
        assert!(serum.opaque_state.is_none());
    }

    #[test]
    fn no_state_element_means_none() {
        let p = parse_project_xml(PROJECT_XML, &mut no_state).unwrap();
        // AU plugin has no <State> element → None regardless of loader
        let au = &p.tracks[1].channel.as_ref().unwrap().devices[0];
        assert!(au.opaque_state.is_none());
    }

    #[test]
    fn missing_structure_gives_empty_tracks() {
        let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<Project version="1.0">
  <Application name="Bitwig Studio" version="5.0"/>
</Project>"#;
        let p = parse_project_xml(xml, &mut no_state).unwrap();
        assert!(p.tracks.is_empty());
    }

    #[test]
    fn base64_encode_known_vectors() {
        assert_eq!(base64_encode(b""), "");
        assert_eq!(base64_encode(b"f"), "Zg==");
        assert_eq!(base64_encode(b"fo"), "Zm8=");
        assert_eq!(base64_encode(b"foo"), "Zm9v");
        assert_eq!(base64_encode(b"foob"), "Zm9vYg==");
        assert_eq!(base64_encode(b"fooba"), "Zm9vYmE=");
        assert_eq!(base64_encode(b"foobar"), "Zm9vYmFy");
        assert_eq!(base64_encode(&[0x00, 0x01, 0x02, 0x03]), "AAECAw==");
    }

    #[test]
    fn zip_round_trip() {
        use std::io::{Cursor, Write};
        use zip::write::SimpleFileOptions;

        let xml = PROJECT_XML.as_bytes();
        let state_bytes: &[u8] = &[0xDE, 0xAD, 0xBE, 0xEF];

        let mut buf = Vec::new();
        {
            let cursor = Cursor::new(&mut buf);
            let mut w = zip::ZipWriter::new(cursor);
            let opts = SimpleFileOptions::default();
            w.start_file("project.xml", opts).unwrap();
            w.write_all(xml).unwrap();
            w.start_file("plugins/proq3.vstpreset", opts).unwrap();
            w.write_all(state_bytes).unwrap();
            w.finish().unwrap();
        }

        let cursor = Cursor::new(buf);
        let mut archive = zip::ZipArchive::new(cursor).unwrap();
        let xml_text = read_zip_text(&mut archive, "project.xml").unwrap();
        let project =
            parse_project_xml(&xml_text, &mut |p| load_state_from_zip(&mut archive, p)).unwrap();

        let dev = &project.tracks[0].channel.as_ref().unwrap().devices[0];
        assert_eq!(dev.name, "Pro-Q 3");
        assert_eq!(dev.opaque_state.as_deref(), Some(base64_encode(state_bytes).as_str()));
    }
}
