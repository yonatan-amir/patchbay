//! Writer for DAWproject archives (`.dawproject`).
//!
//! DAWproject is an open ZIP-based format co-developed by Bitwig and PreSonus.
//! Both Bitwig Studio and Studio One import `.dawproject` natively.  We emit
//! a minimal single-track project wrapping the chain's devices; state blobs
//! are decoded from their stored base64 and written as binary files inside the
//! ZIP, then referenced via `<State path="..."/>` child elements.

use std::io::{Cursor, Write};

use zip::ZipWriter;
use zip::write::SimpleFileOptions;

use crate::db::{ChainDetail, ChainSlotRow};

// ── Public API ────────────────────────────────────────────────────────────────

/// Render a chain into a `.dawproject` archive (binary ZIP).
pub fn write_dawproject(chain: &ChainDetail) -> Result<Vec<u8>, String> {
    let mut buf = Vec::new();
    {
        let cursor = Cursor::new(&mut buf);
        let mut zip = ZipWriter::new(cursor);
        let opts = SimpleFileOptions::default();

        // Build project.xml and collect state blobs.
        let mut state_files: Vec<(String, Vec<u8>)> = Vec::new();
        let xml = build_project_xml(chain, &mut state_files);

        zip.start_file("project.xml", opts).map_err(|e| e.to_string())?;
        zip.write_all(xml.as_bytes()).map_err(|e| e.to_string())?;

        for (path, bytes) in &state_files {
            zip.start_file(path, opts).map_err(|e| e.to_string())?;
            zip.write_all(bytes).map_err(|e| e.to_string())?;
        }

        zip.finish().map_err(|e| e.to_string())?;
    }
    Ok(buf)
}

// ── XML construction ──────────────────────────────────────────────────────────

fn build_project_xml(
    chain: &ChainDetail,
    state_files: &mut Vec<(String, Vec<u8>)>,
) -> String {
    let track_name = xe(&chain.name);
    let devices = devices_xml(&chain.slots, state_files);

    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<Project version="1.0">
  <Application name="Patchbay" version="1.0.0"/>
  <Structure>
    <Track contentType="audio" id="t0" name="{track_name}">
      <Channel audioChannels="2" role="regular" id="ch0">
        <Devices>
{devices}
        </Devices>
      </Channel>
    </Track>
  </Structure>
</Project>"#
    )
}

fn devices_xml(slots: &[ChainSlotRow], state_files: &mut Vec<(String, Vec<u8>)>) -> String {
    slots
        .iter()
        .enumerate()
        .map(|(i, s)| device_element(s, i, state_files))
        .collect::<Vec<_>>()
        .join("\n")
}

fn device_element(
    slot: &ChainSlotRow,
    idx: usize,
    state_files: &mut Vec<(String, Vec<u8>)>,
) -> String {
    let id: serde_json::Value =
        serde_json::from_str(&slot.plugin_identity).unwrap_or(serde_json::Value::Null);

    let name = xe(id["name"].as_str().unwrap_or("Unknown"));
    let format = id["format"].as_str().unwrap_or("Builtin");
    let enabled = if slot.bypass { "false" } else { "true" };

    let state_elem = state_child(slot, idx, state_files);

    match format {
        "VST3" => {
            let device_id = xe(id["device_id"].as_str().unwrap_or(""));
            let vendor = xe(id["vendor"].as_str().unwrap_or(""));
            format!(
                "          <Vst3Plugin deviceID=\"{device_id}\" name=\"{name}\" vendor=\"{vendor}\">\n\
                             <Enabled value=\"{enabled}\"/>\n\
                             {state_elem}\
                           </Vst3Plugin>"
            )
        }
        "VST2" => {
            let uid = id["unique_id"].as_i64().unwrap_or(0);
            let vendor = xe(id["vendor"].as_str().unwrap_or(""));
            format!(
                "          <Vst2Plugin uniqueId=\"{uid}\" name=\"{name}\" vendor=\"{vendor}\">\n\
                             <Enabled value=\"{enabled}\"/>\n\
                             {state_elem}\
                           </Vst2Plugin>"
            )
        }
        "AU" => {
            let type_code = xe(id["type_code"].as_str().unwrap_or(""));
            let sub_type = xe(id["sub_type"].as_str().unwrap_or(""));
            let mfr = xe(id["manufacturer"].as_str().unwrap_or(""));
            format!(
                "          <AuPlugin type=\"{type_code}\" subType=\"{sub_type}\" manufacturer=\"{mfr}\" name=\"{name}\">\n\
                             <Enabled value=\"{enabled}\"/>\n\
                             {state_elem}\
                           </AuPlugin>"
            )
        }
        "CLAP" => {
            let clap_id = xe(id["id"].as_str().unwrap_or(""));
            let vendor = xe(id["vendor"].as_str().unwrap_or(""));
            format!(
                "          <ClapPlugin id=\"{clap_id}\" name=\"{name}\" vendor=\"{vendor}\">\n\
                             <Enabled value=\"{enabled}\"/>\n\
                             {state_elem}\
                           </ClapPlugin>"
            )
        }
        _ => {
            // Built-in or unknown — emit a placeholder element.
            format!(
                "          <Builtin name=\"{name}\">\n\
                             <Enabled value=\"{enabled}\"/>\n\
                           </Builtin>"
            )
        }
    }
}

/// If the slot has an opaque_state, decode it and register a state file.
/// Returns the `<State path="..."/>` child element string (with trailing
/// newline), or an empty string when there is no state.
fn state_child(
    slot: &ChainSlotRow,
    idx: usize,
    state_files: &mut Vec<(String, Vec<u8>)>,
) -> String {
    let state_b64 = match &slot.opaque_state {
        Some(s) => s,
        None => return String::new(),
    };

    let bytes = match b64_decode(state_b64) {
        Some(b) => b,
        None => return String::new(),
    };

    let path = format!("plugins/state_{idx}.bin");
    state_files.push((path.clone(), bytes));
    format!("<State path=\"{path}\"/>\n")
}

// ── Base64 decoder ────────────────────────────────────────────────────────────

fn b64_decode(s: &str) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity((s.len() * 3) / 4 + 3);
    let mut buf = [0u8; 4];
    let mut n = 0usize;

    for &byte in s.as_bytes() {
        let v = match byte {
            b'A'..=b'Z' => byte - b'A',
            b'a'..=b'z' => byte - b'a' + 26,
            b'0'..=b'9' => byte - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            b'=' => break,
            b'\n' | b'\r' | b' ' | b'\t' => continue,
            _ => return None,
        };
        buf[n] = v;
        n += 1;
        if n == 4 {
            out.push((buf[0] << 2) | (buf[1] >> 4));
            out.push((buf[1] << 4) | (buf[2] >> 2));
            out.push((buf[2] << 6) | buf[3]);
            n = 0;
        }
    }

    match n {
        2 => out.push((buf[0] << 2) | (buf[1] >> 4)),
        3 => {
            out.push((buf[0] << 2) | (buf[1] >> 4));
            out.push((buf[1] << 4) | (buf[2] >> 2));
        }
        _ => {}
    }

    Some(out)
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

    fn chain(slots: Vec<ChainSlotRow>) -> ChainDetail {
        ChainDetail {
            id: 1,
            sync_id: "test".to_string(),
            name: "My Chain".to_string(),
            daw: "Bitwig Studio".to_string(),
            source_track: None,
            notes: None,
            tags: None,
            created_at: "2026-01-01T00:00:00Z".to_string(),
            slots,
        }
    }

    fn vst3_slot(state: Option<&str>) -> ChainSlotRow {
        ChainSlotRow {
            id: 1,
            plugin_id: None,
            plugin_identity: serde_json::json!({
                "name": "Pro-Q 3",
                "vendor": "FabFilter",
                "format": "VST3",
                "device_id": "{D8D91CE4-6A7D-4670-8BDE-4B688EC18B43}",
            })
            .to_string(),
            position: 0,
            bypass: false,
            wet: 1.0,
            preset_name: None,
            opaque_state: state.map(str::to_string),
        }
    }

    #[test]
    fn produces_zip_archive() {
        let bytes = write_dawproject(&chain(vec![vst3_slot(None)])).unwrap();
        // ZIP magic bytes
        assert_eq!(&bytes[..4], b"PK\x03\x04");
    }

    #[test]
    fn zip_contains_project_xml() {
        use std::io::Read;
        let bytes = write_dawproject(&chain(vec![vst3_slot(None)])).unwrap();
        let cursor = Cursor::new(bytes);
        let mut archive = zip::ZipArchive::new(cursor).unwrap();
        let mut xml_entry = archive.by_name("project.xml").unwrap();
        let mut xml = String::new();
        xml_entry.read_to_string(&mut xml).unwrap();
        assert!(xml.contains("<Project version=\"1.0\">"));
        assert!(xml.contains("Pro-Q 3"));
    }

    #[test]
    fn state_blob_written_to_zip_file() {
        use std::io::Read;
        // base64("hello") = "aGVsbG8="
        let bytes =
            write_dawproject(&chain(vec![vst3_slot(Some("aGVsbG8="))])).unwrap();
        let cursor = Cursor::new(bytes);
        let mut archive = zip::ZipArchive::new(cursor).unwrap();

        let mut state_entry = archive.by_name("plugins/state_0.bin").unwrap();
        let mut state_bytes = Vec::new();
        state_entry.read_to_end(&mut state_bytes).unwrap();
        assert_eq!(state_bytes, b"hello");
    }

    #[test]
    fn xml_references_state_path() {
        use std::io::Read;
        let bytes = write_dawproject(&chain(vec![vst3_slot(Some("aGVsbG8="))])).unwrap();
        let cursor = Cursor::new(bytes);
        let mut archive = zip::ZipArchive::new(cursor).unwrap();
        let mut xml_entry = archive.by_name("project.xml").unwrap();
        let mut xml = String::new();
        xml_entry.read_to_string(&mut xml).unwrap();
        assert!(xml.contains("plugins/state_0.bin"));
    }

    #[test]
    fn b64_decode_known_vectors() {
        assert_eq!(b64_decode(""), Some(vec![]));
        assert_eq!(b64_decode("Zg=="), Some(b"f".to_vec()));
        assert_eq!(b64_decode("Zm8="), Some(b"fo".to_vec()));
        assert_eq!(b64_decode("Zm9v"), Some(b"foo".to_vec()));
        assert_eq!(b64_decode("AAECAw=="), Some(vec![0, 1, 2, 3]));
    }
}
