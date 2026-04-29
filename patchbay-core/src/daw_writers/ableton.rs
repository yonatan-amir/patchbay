//! Writer for Ableton Device Group presets (`.adg`).
//!
//! An `.adg` is a gzip-compressed XML file whose root element is an
//! `<Ableton>` wrapper containing a single device or rack.  We emit a
//! `<GroupDevice>` (AudioEffectRack) with one default branch so the file
//! lands as a loadable rack in Ableton's browser.
//!
//! # Plugin state
//! Third-party plugin state (`opaque_state`) is embedded inside the
//! format-appropriate `<*Preset>` element unchanged.  Ableton builtins
//! (format `"Ableton"`) only carry a name + tag in `plugin_identity`; we
//! emit a minimal device element with default parameter state (settings
//! are not preserved, but device ordering is).

use std::io::Write;

use flate2::Compression;
use flate2::write::GzEncoder;

use crate::db::{ChainDetail, ChainSlotRow};

// ── Public API ────────────────────────────────────────────────────────────────

/// Render a chain into a gzip-compressed `.adg` file (binary).
pub fn write_adg(chain: &ChainDetail) -> Result<Vec<u8>, String> {
    let xml = build_xml(chain);
    let mut enc = GzEncoder::new(Vec::new(), Compression::default());
    enc.write_all(xml.as_bytes()).map_err(|e| e.to_string())?;
    enc.finish().map_err(|e| e.to_string())
}

// ── XML construction ──────────────────────────────────────────────────────────

fn build_xml(chain: &ChainDetail) -> String {
    let chain_name = xe(&chain.name);

    let devices: String = chain
        .slots
        .iter()
        .enumerate()
        .map(|(i, s)| device_element(s, i))
        .collect::<Vec<_>>()
        .join("\n");

    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<Ableton MajorVersion="11" MinorVersion="11.0.0" Creator="Patchbay" Annotation="" Id="0">
  <GroupDevice Id="0">
    <Name><EffectiveName Value="{chain_name}"/></Name>
    <On><Manual Value="true"/></On>
    <Branches>
      <AudioEffectBranch Id="0">
        <Name Value="Default"/>
        <IsSelected Value="true"/>
        <DeviceChain>
          <AudioToAudioDeviceChain Id="0">
            <Devices>
{devices}
            </Devices>
          </AudioToAudioDeviceChain>
        </DeviceChain>
      </AudioEffectBranch>
    </Branches>
  </GroupDevice>
</Ableton>"#
    )
}

fn device_element(slot: &ChainSlotRow, id: usize) -> String {
    let identity: serde_json::Value =
        serde_json::from_str(&slot.plugin_identity).unwrap_or(serde_json::Value::Null);

    let name = xe(identity["name"].as_str().unwrap_or("Unknown"));
    let vendor = xe(identity["vendor"].as_str().unwrap_or(""));
    let format = identity["format"].as_str().unwrap_or("VST2");
    let active = if slot.bypass { "false" } else { "true" };

    match format {
        "Ableton" => {
            // Builtin: emit the device XML element with default state.
            let tag = identity["tag"].as_str().unwrap_or("UnknownDevice");
            format!(
                "              <{tag} Id=\"{id}\">\n\
                                 <Name><EffectiveName Value=\"{name}\"/></Name>\n\
                                 <On><Manual Value=\"{active}\"/></On>\n\
                               </{tag}>"
            )
        }
        _ => {
            let (info_tag, info_body, preset_tag) = plugin_desc(format, &name, &vendor);
            let state_xml = preset_xml(preset_tag, format, slot.opaque_state.as_deref());
            format!(
                "              <PluginDevice Id=\"{id}\">\n\
                                 <Name><EffectiveName Value=\"{name}\"/></Name>\n\
                                 <On><Manual Value=\"{active}\"/></On>\n\
                                 <PluginDesc><{info_tag} Id=\"0\">{info_body}</{info_tag}></PluginDesc>\n\
                                 <Preset>{state_xml}</Preset>\n\
                               </PluginDevice>"
            )
        }
    }
}

/// Returns `(info_tag, info_body_xml, preset_tag)` for a given plugin format.
fn plugin_desc<'a>(
    format: &str,
    name: &str,
    vendor: &str,
) -> (&'a str, String, &'a str) {
    match format {
        "VST3" => (
            "Vst3PluginInfo",
            format!("<Name Value=\"{name}\"/><VendorString Value=\"{vendor}\"/>"),
            "Vst3Preset",
        ),
        "AU" => (
            "AuPluginInfo",
            format!("<Name Value=\"{name}\"/><Manufacturer Value=\"{vendor}\"/>"),
            "AuPreset",
        ),
        _ => (
            "VstPluginInfo",
            format!("<PlugName Value=\"{name}\"/><VendorString Value=\"{vendor}\"/>"),
            "VstPreset",
        ),
    }
}

fn preset_xml(preset_tag: &str, format: &str, state: Option<&str>) -> String {
    match state {
        None => format!("<{preset_tag} Id=\"0\"/>"),
        Some(data) => {
            if format == "VST2" {
                // VST2 state sits inside a PluginDataChunkList wrapper.
                format!(
                    "<{preset_tag} Id=\"0\">\
                     <PluginDataChunkList>\
                     <PluginDataChunk Id=\"0\"><Data>{data}</Data></PluginDataChunk>\
                     </PluginDataChunkList>\
                     </{preset_tag}>"
                )
            } else {
                format!("<{preset_tag} Id=\"0\"><Data>{data}</Data></{preset_tag}>")
            }
        }
    }
}

/// XML-escape a string.
fn xe(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::ChainDetail;

    fn make_chain(slots: Vec<ChainSlotRow>) -> ChainDetail {
        ChainDetail {
            id: 1,
            sync_id: "test".to_string(),
            name: "My Chain".to_string(),
            daw: "Ableton".to_string(),
            source_track: None,
            notes: None,
            tags: None,
            created_at: "2026-01-01T00:00:00Z".to_string(),
            slots,
        }
    }

    fn slot_vst3(name: &str, vendor: &str, bypass: bool, state: Option<&str>) -> ChainSlotRow {
        ChainSlotRow {
            id: 1,
            plugin_id: None,
            plugin_identity: serde_json::json!({
                "name": name, "vendor": vendor, "format": "VST3"
            })
            .to_string(),
            position: 0,
            bypass,
            wet: 1.0,
            preset_name: None,
            opaque_state: state.map(str::to_string),
        }
    }

    fn slot_ableton_builtin(name: &str, tag: &str) -> ChainSlotRow {
        ChainSlotRow {
            id: 2,
            plugin_id: None,
            plugin_identity: serde_json::json!({
                "name": name, "vendor": "Ableton", "format": "Ableton", "tag": tag
            })
            .to_string(),
            position: 1,
            bypass: false,
            wet: 1.0,
            preset_name: None,
            opaque_state: None,
        }
    }

    #[test]
    fn produces_valid_gzipped_xml() {
        let chain = make_chain(vec![slot_vst3("Pro-Q 3", "FabFilter", false, Some("AQIDBA=="))]);
        let bytes = write_adg(&chain).unwrap();
        assert!(!bytes.is_empty());
        // gzip magic bytes
        assert_eq!(&bytes[..2], &[0x1f, 0x8b]);
    }

    #[test]
    fn xml_contains_plugin_name_and_vendor() {
        let chain = make_chain(vec![slot_vst3("Pro-Q 3", "FabFilter", false, Some("AQIDBA=="))]);
        let xml = build_xml(&chain);
        assert!(xml.contains("Pro-Q 3"));
        assert!(xml.contains("FabFilter"));
        assert!(xml.contains("Vst3PluginInfo"));
        assert!(xml.contains("AQIDBA=="));
    }

    #[test]
    fn ableton_builtin_emits_tag_element() {
        let chain = make_chain(vec![slot_ableton_builtin("EQ Eight", "Eq8")]);
        let xml = build_xml(&chain);
        assert!(xml.contains("<Eq8 Id=\"0\">"));
        assert!(xml.contains("</Eq8>"));
        assert!(xml.contains("EQ Eight"));
    }

    #[test]
    fn bypassed_plugin_sets_active_false() {
        let chain = make_chain(vec![slot_vst3("Serum", "Xfer", true, None)]);
        let xml = build_xml(&chain);
        assert!(xml.contains("<On><Manual Value=\"false\"/></On>"));
    }

    #[test]
    fn xml_escapes_special_chars_in_name() {
        let chain = make_chain(vec![slot_vst3("A & B <Test>", "FabFilter", false, None)]);
        let xml = build_xml(&chain);
        assert!(xml.contains("A &amp; B &lt;Test&gt;"));
    }

    #[test]
    fn vst2_state_wrapped_in_chunk_list() {
        let slot = ChainSlotRow {
            id: 1,
            plugin_id: None,
            plugin_identity: serde_json::json!({
                "name": "Serum", "vendor": "Xfer Records", "format": "VST2"
            })
            .to_string(),
            position: 0,
            bypass: false,
            wet: 1.0,
            preset_name: None,
            opaque_state: Some("c2VydW0=".to_string()),
        };
        let chain = make_chain(vec![slot]);
        let xml = build_xml(&chain);
        assert!(xml.contains("PluginDataChunkList"));
        assert!(xml.contains("c2VydW0="));
    }
}
