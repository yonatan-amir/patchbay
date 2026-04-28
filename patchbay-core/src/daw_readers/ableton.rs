//! Reader for Ableton Live Sets (`.als`) and Device Groups (`.adg`).
//!
//! Both formats are gzip-compressed XML. `.als` is a full project; `.adg` is a
//! single device or rack preset. The reader returns structured data — it does
//! not write to the database.
//!
//! # State blobs
//! Third-party plugin state is an opaque base64 blob produced by the plugin
//! itself. We extract and store it as-is inside `DeviceType::Plugin`.
//! Ableton built-in device state is plain XML; we capture the raw XML of the
//! device element inside `DeviceType::AbletonBuiltin`.

use std::io::Read;
use std::path::Path;

use flate2::read::GzDecoder;
use serde::{Deserialize, Serialize};
use thiserror::Error;

// ─── Errors ──────────────────────────────────────────────────────────────────

#[derive(Debug, Error)]
pub enum AbletonError {
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("XML error: {0}")]
    Xml(#[from] roxmltree::Error),
    #[error("Format error: {0}")]
    Format(String),
}

// ─── Public types ────────────────────────────────────────────────────────────

/// Parsed Ableton Live Set (`.als`).
#[derive(Debug, Serialize, Deserialize)]
pub struct AbletonProject {
    pub creator: String,
    pub major_version: String,
    pub tracks: Vec<Track>,
}

/// Parsed Ableton Device Group / Rack (`.adg`).
#[derive(Debug, Serialize, Deserialize)]
pub struct AbletonRack {
    pub creator: String,
    pub major_version: String,
    pub device: DeviceNode,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum TrackKind {
    Midi,
    Audio,
    Return,
    Group,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Track {
    pub kind: TrackKind,
    pub name: String,
    pub color_index: Option<i32>,
    pub chain: Vec<DeviceNode>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct DeviceNode {
    pub name: String,
    /// `false` means the device is bypassed.
    pub is_active: bool,
    pub device_type: DeviceType,
    /// Populated only for rack devices (Instrument / Audio Effect / Drum racks).
    pub macros: Vec<MacroControl>,
    /// Devices inside rack branches, flattened across all branches.
    pub children: Vec<DeviceNode>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "kind")]
pub enum DeviceType {
    /// Third-party VST2 / VST3 / AU — state is an opaque base64 blob from the plugin.
    Plugin {
        plugin_name: String,
        vendor: Option<String>,
        format: PluginFormat,
        /// Base64 blob as extracted from `<Preset>/<*Preset>/<Data>`. Stored verbatim.
        opaque_state: Option<String>,
    },
    /// Ableton Instrument, Audio Effect, or Drum rack.
    Rack { rack_type: RackType },
    /// Any Ableton built-in (EQ Eight, Compressor, etc.) — state is the raw device XML.
    AbletonBuiltin { tag: String, params_xml: String },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum PluginFormat {
    Vst2,
    Vst3,
    Au,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum RackType {
    Instrument,
    AudioEffect,
    Drum,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct MacroControl {
    /// 0-based index (Ableton labels them 1-8 in the UI).
    pub index: u8,
    pub name: String,
    pub value: f64,
    pub min: f64,
    pub max: f64,
}

/// Return type for [`read_file`].
pub enum AbletonFile {
    Project(AbletonProject),
    Rack(AbletonRack),
}

// ─── Public API ──────────────────────────────────────────────────────────────

/// Read an Ableton Live Set (`.als`).
pub fn read_als(path: &Path) -> Result<AbletonProject, AbletonError> {
    let xml = decompress(path)?;
    parse_als_xml(&xml)
}

/// Read an Ableton Device Group / Rack (`.adg`).
pub fn read_adg(path: &Path) -> Result<AbletonRack, AbletonError> {
    let xml = decompress(path)?;
    parse_adg_xml(&xml)
}

/// Dispatch on file extension (`.als` → `Project`, `.adg` → `Rack`).
pub fn read_file(path: &Path) -> Result<AbletonFile, AbletonError> {
    match path.extension().and_then(|e| e.to_str()) {
        Some("als") => Ok(AbletonFile::Project(read_als(path)?)),
        Some("adg") => Ok(AbletonFile::Rack(read_adg(path)?)),
        _ => Err(AbletonError::Format(format!(
            "Unsupported extension: {}",
            path.display()
        ))),
    }
}

// ─── Gzip decompression ──────────────────────────────────────────────────────

fn decompress(path: &Path) -> Result<String, AbletonError> {
    let file = std::fs::File::open(path)?;
    let mut decoder = GzDecoder::new(file);
    let mut xml = String::new();
    decoder.read_to_string(&mut xml)?;
    Ok(xml)
}

// ─── XML parsing (internal, exposed for tests) ───────────────────────────────

pub(crate) fn parse_als_xml(xml: &str) -> Result<AbletonProject, AbletonError> {
    let doc = roxmltree::Document::parse(xml)?;
    let root = doc.root_element();

    let creator = root.attribute("Creator").unwrap_or("").to_string();
    let major_version = root.attribute("MajorVersion").unwrap_or("").to_string();

    let live_set = root
        .children()
        .find(|n| n.has_tag_name("LiveSet"))
        .ok_or_else(|| AbletonError::Format("Missing <LiveSet>".into()))?;

    let tracks_node = live_set
        .children()
        .find(|n| n.has_tag_name("Tracks"))
        .ok_or_else(|| AbletonError::Format("Missing <Tracks>".into()))?;

    let tracks = tracks_node
        .children()
        .filter(|n| n.is_element())
        .filter_map(parse_track)
        .collect();

    Ok(AbletonProject { creator, major_version, tracks })
}

pub(crate) fn parse_adg_xml(xml: &str) -> Result<AbletonRack, AbletonError> {
    let doc = roxmltree::Document::parse(xml)?;
    let root = doc.root_element();

    let creator = root.attribute("Creator").unwrap_or("").to_string();
    let major_version = root.attribute("MajorVersion").unwrap_or("").to_string();

    let device_elem = root
        .children()
        .find(|n| n.is_element())
        .ok_or_else(|| AbletonError::Format("No device element in ADG".into()))?;

    let device = parse_device(device_elem)
        .ok_or_else(|| AbletonError::Format("Could not parse root device".into()))?;

    Ok(AbletonRack { creator, major_version, device })
}

// ─── Track parsing ───────────────────────────────────────────────────────────

fn parse_track(node: roxmltree::Node) -> Option<Track> {
    let kind = match node.tag_name().name() {
        "MidiTrack" => TrackKind::Midi,
        "AudioTrack" => TrackKind::Audio,
        "ReturnTrack" => TrackKind::Return,
        "GroupTrack" => TrackKind::Group,
        _ => return None,
    };

    let name = effective_name(node).unwrap_or_default();
    let color_index = child_attr_value(node, "ColorIndex", "Value")
        .and_then(|v| v.parse().ok());

    // DeviceChain > MidiToAudioDeviceChain|AudioToAudioDeviceChain > Devices
    let chain = node
        .children()
        .find(|n| n.has_tag_name("DeviceChain"))
        .and_then(|dc| dc.children().find(|n| n.is_element()))
        .and_then(|inner| inner.children().find(|n| n.has_tag_name("Devices")))
        .map(|devs| {
            devs.children()
                .filter(|n| n.is_element())
                .filter_map(parse_device)
                .collect()
        })
        .unwrap_or_default();

    Some(Track { kind, name, color_index, chain })
}

// ─── Device parsing ──────────────────────────────────────────────────────────

const RACK_TAGS: &[&str] = &["InstrumentGroupDevice", "GroupDevice", "DrumGroupDevice"];

fn parse_device(node: roxmltree::Node) -> Option<DeviceNode> {
    let tag = node.tag_name().name();
    let name = effective_name(node).unwrap_or_else(|| tag.to_string());
    let is_active = device_is_active(node);

    if tag == "PluginDevice" {
        let (plugin_name, vendor, format, opaque_state) = parse_plugin_device(node)?;
        return Some(DeviceNode {
            name,
            is_active,
            device_type: DeviceType::Plugin { plugin_name, vendor, format, opaque_state },
            macros: vec![],
            children: vec![],
        });
    }

    if RACK_TAGS.contains(&tag) {
        let rack_type = match tag {
            "InstrumentGroupDevice" => RackType::Instrument,
            "DrumGroupDevice" => RackType::Drum,
            _ => RackType::AudioEffect,
        };
        return Some(DeviceNode {
            name,
            is_active,
            device_type: DeviceType::Rack { rack_type },
            macros: parse_macros(node),
            children: parse_rack_branches(node),
        });
    }

    // Ableton built-in device — capture the raw XML as the decoded state.
    Some(DeviceNode {
        name,
        is_active,
        device_type: DeviceType::AbletonBuiltin {
            tag: tag.to_string(),
            params_xml: serialize_node(node),
        },
        macros: vec![],
        children: vec![],
    })
}

fn parse_plugin_device(
    node: roxmltree::Node,
) -> Option<(String, Option<String>, PluginFormat, Option<String>)> {
    let plugin_desc = node.children().find(|n| n.has_tag_name("PluginDesc"))?;

    if let Some(info) = plugin_desc.children().find(|n| n.has_tag_name("Vst3PluginInfo")) {
        let name = child_attr_value(info, "Name", "Value").unwrap_or_else(|| "Unknown".into());
        let vendor = child_attr_value(info, "VendorString", "Value");
        return Some((name, vendor, PluginFormat::Vst3, extract_opaque_state(node)));
    }

    if let Some(info) = plugin_desc.children().find(|n| n.has_tag_name("VstPluginInfo")) {
        let name = child_attr_value(info, "PlugName", "Value").unwrap_or_else(|| "Unknown".into());
        let vendor = child_attr_value(info, "VendorString", "Value");
        return Some((name, vendor, PluginFormat::Vst2, extract_opaque_state(node)));
    }

    if let Some(info) = plugin_desc.children().find(|n| n.has_tag_name("AuPluginInfo")) {
        let name = child_attr_value(info, "Name", "Value").unwrap_or_else(|| "Unknown".into());
        let vendor = child_attr_value(info, "Manufacturer", "Value");
        return Some((name, vendor, PluginFormat::Au, extract_opaque_state(node)));
    }

    None
}

/// Extract the base64 opaque state blob from `<Preset>/<*Preset>/<Data>`.
fn extract_opaque_state(node: roxmltree::Node) -> Option<String> {
    let preset = node.children().find(|n| n.has_tag_name("Preset"))?;
    find_data_text(preset)
}

fn find_data_text(node: roxmltree::Node) -> Option<String> {
    if node.has_tag_name("Data") {
        let text: String = node.children().filter_map(|n| n.text()).collect();
        let trimmed = text.trim().to_string();
        if !trimmed.is_empty() {
            return Some(trimmed);
        }
    }
    for child in node.children() {
        if let Some(found) = find_data_text(child) {
            return Some(found);
        }
    }
    None
}

// ─── Macro parsing ───────────────────────────────────────────────────────────

fn parse_macros(node: roxmltree::Node) -> Vec<MacroControl> {
    let macros_node = match node.children().find(|n| n.has_tag_name("Macros")) {
        Some(m) => m,
        None => return vec![],
    };

    (0u8..8)
        .filter_map(|i| {
            let ctrl_tag = format!("MacroControls.{i}");
            let ctrl = macros_node.children().find(|n| n.has_tag_name(ctrl_tag.as_str()))?;

            let value = child_attr_value(ctrl, "Manual", "Value")
                .and_then(|v| v.parse().ok())
                .unwrap_or(0.0);

            let range = ctrl.children().find(|n| n.has_tag_name("MidiControllerRange"));
            let min = range
                .and_then(|r| child_attr_value(r, "Min", "Value"))
                .and_then(|v| v.parse().ok())
                .unwrap_or(0.0);
            let max = range
                .and_then(|r| child_attr_value(r, "Max", "Value"))
                .and_then(|v| v.parse().ok())
                .unwrap_or(127.0);

            // Name: MacroNames sibling → Name child of control → default label.
            let names_tag = format!("MacroNames.{i}");
            let name = node
                .children()
                .find(|n| n.has_tag_name("MacroNames"))
                .and_then(|mn| mn.children().find(|n| n.has_tag_name(names_tag.as_str())))
                .and_then(|n| n.attribute("Value"))
                .or_else(|| {
                    ctrl.children()
                        .find(|n| n.has_tag_name("Name"))
                        .and_then(|n| n.attribute("Value"))
                })
                .map(|s| s.to_string())
                .unwrap_or_else(|| format!("Macro {}", i + 1));

            Some(MacroControl { index: i, name, value, min, max })
        })
        .collect()
}

// ─── Rack branch parsing ─────────────────────────────────────────────────────

/// Collect all devices from all rack branches into a flat list.
fn parse_rack_branches(node: roxmltree::Node) -> Vec<DeviceNode> {
    let branches = match node.children().find(|n| n.has_tag_name("Branches")) {
        Some(b) => b,
        None => return vec![],
    };

    branches
        .children()
        .filter(|n| n.is_element())
        .flat_map(|branch| {
            branch
                .children()
                .find(|n| n.has_tag_name("DeviceChain"))
                .and_then(|dc| dc.children().find(|n| n.is_element()))
                .and_then(|inner| inner.children().find(|n| n.has_tag_name("Devices")))
                .map(|devs| {
                    devs.children()
                        .filter(|n| n.is_element())
                        .filter_map(parse_device)
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default()
        })
        .collect()
}

// ─── XML utilities ───────────────────────────────────────────────────────────

fn device_is_active(node: roxmltree::Node) -> bool {
    node.children()
        .find(|n| n.has_tag_name("On"))
        .and_then(|on| on.children().find(|n| n.has_tag_name("Manual")))
        .and_then(|m| m.attribute("Value"))
        .map(|v| v.eq_ignore_ascii_case("true"))
        .unwrap_or(true)
}

fn effective_name(node: roxmltree::Node) -> Option<String> {
    node.children()
        .find(|n| n.has_tag_name("Name"))
        .and_then(|name_node| {
            name_node
                .children()
                .find(|n| n.has_tag_name("EffectiveName"))
                .and_then(|n| n.attribute("Value"))
                .map(|s| s.to_string())
        })
}

fn child_attr_value(node: roxmltree::Node, child_tag: &str, attr: &str) -> Option<String> {
    node.children()
        .find(|n| n.has_tag_name(child_tag))
        .and_then(|n| n.attribute(attr))
        .map(|s| s.to_string())
}

/// Serialize a DOM node back to an XML string (used for built-in device state).
fn serialize_node(node: roxmltree::Node) -> String {
    let mut buf = String::new();
    write_node(node, &mut buf);
    buf
}

fn write_node(node: roxmltree::Node, buf: &mut String) {
    if node.is_text() {
        if let Some(t) = node.text() {
            xml_escape(t, buf);
        }
        return;
    }
    if !node.is_element() {
        return;
    }
    buf.push('<');
    buf.push_str(node.tag_name().name());
    for attr in node.attributes() {
        buf.push(' ');
        buf.push_str(attr.name());
        buf.push_str("=\"");
        xml_escape(attr.value(), buf);
        buf.push('"');
    }
    let has_children = node.children().next().is_some();
    if has_children {
        buf.push('>');
        for child in node.children() {
            write_node(child, buf);
        }
        buf.push_str("</");
        buf.push_str(node.tag_name().name());
        buf.push('>');
    } else {
        buf.push_str("/>");
    }
}

fn xml_escape(s: &str, buf: &mut String) {
    for c in s.chars() {
        match c {
            '&' => buf.push_str("&amp;"),
            '<' => buf.push_str("&lt;"),
            '>' => buf.push_str("&gt;"),
            '"' => buf.push_str("&quot;"),
            _ => buf.push(c),
        }
    }
}

// ─── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    const ALS_XML: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<Ableton MajorVersion="11" Creator="Ableton Live 11.2.2">
  <LiveSet>
    <Tracks>
      <AudioTrack Id="0">
        <Name><EffectiveName Value="Drums"/></Name>
        <ColorIndex Value="5"/>
        <DeviceChain>
          <AudioToAudioDeviceChain>
            <Devices>
              <Eq8 Id="0">
                <Name><EffectiveName Value="EQ Eight"/></Name>
                <On><Manual Value="true"/></On>
              </Eq8>
              <PluginDevice Id="1">
                <Name><EffectiveName Value="Pro-Q 3"/></Name>
                <On><Manual Value="false"/></On>
                <PluginDesc>
                  <Vst3PluginInfo Id="0">
                    <Name Value="Pro-Q 3"/>
                    <VendorString Value="FabFilter"/>
                  </Vst3PluginInfo>
                </PluginDesc>
                <Preset>
                  <Vst3Preset Id="0">
                    <Data>AQIDBA==</Data>
                  </Vst3Preset>
                </Preset>
              </PluginDevice>
            </Devices>
          </AudioToAudioDeviceChain>
        </DeviceChain>
      </AudioTrack>
      <MidiTrack Id="1">
        <Name><EffectiveName Value="Synth"/></Name>
        <ColorIndex Value="2"/>
        <DeviceChain>
          <MidiToAudioDeviceChain>
            <Devices>
              <PluginDevice Id="2">
                <Name><EffectiveName Value="Serum"/></Name>
                <On><Manual Value="true"/></On>
                <PluginDesc>
                  <VstPluginInfo Id="0">
                    <PlugName Value="Serum"/>
                    <VendorString Value="Xfer Records"/>
                  </VstPluginInfo>
                </PluginDesc>
                <Preset>
                  <VstPreset Id="0">
                    <PluginDataChunkList>
                      <PluginDataChunk Id="0">
                        <Data>c2VydW0=</Data>
                      </PluginDataChunk>
                    </PluginDataChunkList>
                  </VstPreset>
                </Preset>
              </PluginDevice>
            </Devices>
          </MidiToAudioDeviceChain>
        </DeviceChain>
      </MidiTrack>
    </Tracks>
  </LiveSet>
</Ableton>"#;

    const ADG_XML: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<Ableton MajorVersion="11" Creator="Ableton Live 11.2.2">
  <InstrumentGroupDevice Id="0">
    <Name><EffectiveName Value="Synth Rack"/></Name>
    <On><Manual Value="true"/></On>
    <Macros>
      <MacroControls.0>
        <Manual Value="0.5"/>
        <MidiControllerRange><Min Value="0"/><Max Value="1"/></MidiControllerRange>
      </MacroControls.0>
      <MacroControls.1>
        <Manual Value="0.75"/>
        <MidiControllerRange><Min Value="0"/><Max Value="127"/></MidiControllerRange>
      </MacroControls.1>
    </Macros>
    <MacroNames>
      <MacroNames.0 Value="Cutoff"/>
      <MacroNames.1 Value="Resonance"/>
    </MacroNames>
    <Branches>
      <InstrumentBranch Id="0">
        <DeviceChain>
          <MidiToAudioDeviceChain>
            <Devices>
              <Eq8 Id="0">
                <Name><EffectiveName Value="Post EQ"/></Name>
                <On><Manual Value="true"/></On>
              </Eq8>
            </Devices>
          </MidiToAudioDeviceChain>
        </DeviceChain>
      </InstrumentBranch>
    </Branches>
  </InstrumentGroupDevice>
</Ableton>"#;

    #[test]
    fn track_count_and_kinds() {
        let project = parse_als_xml(ALS_XML).unwrap();
        assert_eq!(project.tracks.len(), 2);
        assert_eq!(project.tracks[0].kind, TrackKind::Audio);
        assert_eq!(project.tracks[1].kind, TrackKind::Midi);
    }

    #[test]
    fn track_name_and_color() {
        let project = parse_als_xml(ALS_XML).unwrap();
        assert_eq!(project.tracks[0].name, "Drums");
        assert_eq!(project.tracks[0].color_index, Some(5));
    }

    #[test]
    fn builtin_device_is_active_and_has_xml() {
        let project = parse_als_xml(ALS_XML).unwrap();
        let eq = &project.tracks[0].chain[0];
        assert_eq!(eq.name, "EQ Eight");
        assert!(eq.is_active);
        match &eq.device_type {
            DeviceType::AbletonBuiltin { tag, params_xml } => {
                assert_eq!(tag, "Eq8");
                assert!(params_xml.contains("Eq8"));
            }
            _ => panic!("expected AbletonBuiltin"),
        }
    }

    #[test]
    fn vst3_plugin_bypassed_with_state() {
        let project = parse_als_xml(ALS_XML).unwrap();
        let plugin = &project.tracks[0].chain[1];
        assert_eq!(plugin.name, "Pro-Q 3");
        assert!(!plugin.is_active);
        match &plugin.device_type {
            DeviceType::Plugin { plugin_name, vendor, format, opaque_state } => {
                assert_eq!(plugin_name, "Pro-Q 3");
                assert_eq!(vendor.as_deref(), Some("FabFilter"));
                assert_eq!(*format, PluginFormat::Vst3);
                assert_eq!(opaque_state.as_deref(), Some("AQIDBA=="));
            }
            _ => panic!("expected Plugin"),
        }
    }

    #[test]
    fn vst2_plugin_state_nested_in_chunk_list() {
        let project = parse_als_xml(ALS_XML).unwrap();
        let plugin = &project.tracks[1].chain[0];
        assert_eq!(plugin.name, "Serum");
        match &plugin.device_type {
            DeviceType::Plugin { format, opaque_state, .. } => {
                assert_eq!(*format, PluginFormat::Vst2);
                assert_eq!(opaque_state.as_deref(), Some("c2VydW0="));
            }
            _ => panic!("expected Plugin"),
        }
    }

    #[test]
    fn adg_rack_name_and_type() {
        let rack = parse_adg_xml(ADG_XML).unwrap();
        assert_eq!(rack.device.name, "Synth Rack");
        assert!(rack.device.is_active);
        assert!(matches!(
            rack.device.device_type,
            DeviceType::Rack { rack_type: RackType::Instrument }
        ));
    }

    #[test]
    fn adg_macros_name_value_range() {
        let rack = parse_adg_xml(ADG_XML).unwrap();
        let macros = &rack.device.macros;
        assert_eq!(macros.len(), 2);

        assert_eq!(macros[0].index, 0);
        assert_eq!(macros[0].name, "Cutoff");
        assert_eq!(macros[0].value, 0.5);
        assert_eq!(macros[0].min, 0.0);
        assert_eq!(macros[0].max, 1.0);

        assert_eq!(macros[1].index, 1);
        assert_eq!(macros[1].name, "Resonance");
        assert_eq!(macros[1].value, 0.75);
        assert_eq!(macros[1].max, 127.0);
    }

    #[test]
    fn adg_branch_device_parsed() {
        let rack = parse_adg_xml(ADG_XML).unwrap();
        assert_eq!(rack.device.children.len(), 1);
        assert_eq!(rack.device.children[0].name, "Post EQ");
    }
}
