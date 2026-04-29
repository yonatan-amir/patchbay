//! Writer for Reaper FX chain presets (`.RfxChain`).
//!
//! An `.RfxChain` is a standalone FXCHAIN block — the same syntax as the
//! FXCHAIN block inside a `.rpp` project file.  Reaper identifies plugins
//! by GUID (VST3), numeric UID (VST2), or type/subtype/manufacturer codes
//! (AU), so the filename field is left empty and Reaper resolves it from
//! the installed-plugin registry.

use crate::db::ChainSlotRow;

/// Render a slice of chain slots into `.RfxChain` file contents (UTF-8 text).
pub fn write_rfxchain(slots: &[ChainSlotRow]) -> String {
    let mut out = String::new();
    out.push_str("<FXCHAIN\n");
    out.push_str("  SHOW 0\n");
    out.push_str("  LASTSEL 0\n");
    out.push_str("  DOCKED 0\n");
    out.push_str("  BYPASS 0 0 0\n");

    for slot in slots {
        let id: serde_json::Value =
            serde_json::from_str(&slot.plugin_identity).unwrap_or(serde_json::Value::Null);

        let name = id["name"].as_str().unwrap_or("Unknown");
        let vendor = id["vendor"].as_str().unwrap_or("");
        let format = id["format"].as_str().unwrap_or("VST2");

        let (block_tag, prefix) = match format {
            "VST3" => ("VST", "VST3: "),
            "AU" => ("AU", "AU: "),
            "CLAP" => ("CLAP", "CLAP: "),
            _ => ("VST", "VST: "),
        };

        let display = if vendor.is_empty() {
            format!("{}{}", prefix, name)
        } else {
            format!("{}{} ({})", prefix, name, vendor)
        };

        let bypass = if slot.bypass { "1" } else { "0" };
        let preset = slot.preset_name.as_deref().unwrap_or("");

        // UID field: VST3 = "<numeric_uid><{GUID}>", VST2/AU/CLAP = uid string.
        let uid = match format {
            "VST3" => {
                let num = id["vst_uid"].as_str().unwrap_or("");
                let guid = id["vst3_guid"].as_str().unwrap_or("");
                format!("{}{}", num, guid)
            }
            _ => id["vst_uid"].as_str().unwrap_or("").to_string(),
        };

        out.push_str(&format!(
            "  <{block_tag} \"{display}\" \"\" {bypass} \"{preset}\" {uid}\n"
        ));

        if let Some(state) = &slot.opaque_state {
            // Reaper conventionally wraps state lines at 72 base64 chars.
            for chunk in state.as_bytes().chunks(72) {
                out.push_str("    ");
                out.push_str(std::str::from_utf8(chunk).unwrap_or(""));
                out.push('\n');
            }
        }

        out.push_str("  >\n");
    }

    out.push_str(">\n");
    out
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn slot(
        format: &str,
        name: &str,
        vendor: &str,
        bypass: bool,
        preset_name: Option<&str>,
        vst_uid: Option<&str>,
        vst3_guid: Option<&str>,
        opaque_state: Option<&str>,
    ) -> ChainSlotRow {
        let identity = serde_json::json!({
            "name": name,
            "vendor": vendor,
            "format": format,
            "vst_uid": vst_uid,
            "vst3_guid": vst3_guid,
        });
        ChainSlotRow {
            id: 1,
            plugin_id: None,
            plugin_identity: identity.to_string(),
            position: 0,
            bypass,
            wet: 1.0,
            preset_name: preset_name.map(str::to_string),
            opaque_state: opaque_state.map(str::to_string),
        }
    }

    #[test]
    fn vst3_slot_round_trip() {
        let slots = [slot(
            "VST3",
            "Pro-Q 3",
            "FabFilter",
            false,
            None,
            Some("1397572658"),
            Some("{D8D91CE4-6A7D-4670-8BDE-4B688EC18B43}"),
            Some("AQIDBA=="),
        )];
        let out = write_rfxchain(&slots);
        assert!(out.starts_with("<FXCHAIN\n"));
        assert!(out.contains("<VST \"VST3: Pro-Q 3 (FabFilter)\""));
        assert!(out.contains("1397572658{D8D91CE4-6A7D-4670-8BDE-4B688EC18B43}"));
        assert!(out.contains("AQIDBA=="));
        assert!(out.ends_with(">\n"));
    }

    #[test]
    fn vst2_bypassed_with_preset() {
        let slots = [slot(
            "VST2",
            "Serum",
            "Xfer Records",
            true,
            Some("Patch"),
            Some("1936880749"),
            None,
            Some("c2VydW0="),
        )];
        let out = write_rfxchain(&slots);
        assert!(out.contains("<VST \"VST: Serum (Xfer Records)\" \"\" 1 \"Patch\" 1936880749"));
    }

    #[test]
    fn au_slot_no_filename() {
        let slots = [slot(
            "AU",
            "AUPeakLimiter",
            "Apple",
            false,
            None,
            Some("1819304812/aufx/lmtr/appl"),
            None,
            None,
        )];
        let out = write_rfxchain(&slots);
        assert!(out.contains("<AU \"AU: AUPeakLimiter (Apple)\" \"\" 0 \"\" 1819304812/aufx/lmtr/appl"));
    }

    #[test]
    fn empty_slots_produces_bare_fxchain() {
        let out = write_rfxchain(&[]);
        assert_eq!(out, "<FXCHAIN\n  SHOW 0\n  LASTSEL 0\n  DOCKED 0\n  BYPASS 0 0 0\n>\n");
    }
}
