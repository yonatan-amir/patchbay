use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::daw_readers::{ableton, dawproject, logicpro, reaper};
use crate::watcher::ParsedProject;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LiveProject {
    pub path: String,
    pub daw: String,
    pub tracks: Vec<LiveTrack>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LiveTrack {
    pub name: String,
    pub kind: String,
    pub slots: Vec<LiveSlot>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LiveSlot {
    pub position: i32,
    pub name: String,
    pub vendor: Option<String>,
    pub format: Option<String>,
    pub bypass: bool,
    pub wet: f64,
    pub preset_name: Option<String>,
    pub plugin_identity: Value,
    pub opaque_state: Option<String>,
}

/// Convert a parsed project and its file path into the frontend-facing LiveProject.
/// Returns `None` for `.adg` racks and unrecognised project types.
pub fn from_parsed(parsed: &ParsedProject, path: &str) -> Option<LiveProject> {
    match parsed {
        ParsedProject::Ableton(ableton::AbletonFile::Project(p)) => {
            Some(from_ableton_project(p, path))
        }
        ParsedProject::Ableton(ableton::AbletonFile::Rack(_)) => None,
        ParsedProject::Logic(p) => Some(from_logic_project(p, path)),
        ParsedProject::Reaper(p) => Some(from_reaper_project(p, path)),
        ParsedProject::DawProject(p) => Some(from_dawproject_file(p, path)),
        ParsedProject::Unrecognized { .. } => None,
    }
}

// ── Ableton ──────────────────────────────────────────────────────────────────

fn ableton_track_kind(kind: &ableton::TrackKind) -> &'static str {
    match kind {
        ableton::TrackKind::Midi => "instrument",
        ableton::TrackKind::Audio => "audio",
        ableton::TrackKind::Return => "bus",
        ableton::TrackKind::Group => "group",
    }
}

fn ableton_nodes_to_slots(nodes: &[ableton::DeviceNode], pos: &mut i32) -> Vec<LiveSlot> {
    let mut slots = Vec::new();
    for node in nodes {
        match &node.device_type {
            ableton::DeviceType::Plugin { plugin_name, vendor, format, opaque_state } => {
                let fmt = match format {
                    ableton::PluginFormat::Vst2 => "VST2",
                    ableton::PluginFormat::Vst3 => "VST3",
                    ableton::PluginFormat::Au => "AU",
                };
                slots.push(LiveSlot {
                    position: *pos,
                    name: plugin_name.clone(),
                    vendor: vendor.clone(),
                    format: Some(fmt.to_string()),
                    bypass: !node.is_active,
                    wet: 1.0,
                    preset_name: None,
                    plugin_identity: json!({
                        "name": plugin_name,
                        "vendor": vendor,
                        "format": fmt,
                    }),
                    opaque_state: opaque_state.clone(),
                });
                *pos += 1;
            }
            ableton::DeviceType::AbletonBuiltin { tag, .. } => {
                slots.push(LiveSlot {
                    position: *pos,
                    name: node.name.clone(),
                    vendor: Some("Ableton".to_string()),
                    format: Some("Ableton".to_string()),
                    bypass: !node.is_active,
                    wet: 1.0,
                    preset_name: None,
                    plugin_identity: json!({
                        "name": node.name,
                        "vendor": "Ableton",
                        "format": "Ableton",
                        "tag": tag,
                    }),
                    opaque_state: None,
                });
                *pos += 1;
            }
            ableton::DeviceType::Rack { .. } => {
                slots.extend(ableton_nodes_to_slots(&node.children, pos));
            }
        }
    }
    slots
}

fn from_ableton_project(project: &ableton::AbletonProject, path: &str) -> LiveProject {
    let tracks = project.tracks.iter().map(|t| {
        let mut pos = 0i32;
        LiveTrack {
            name: t.name.clone(),
            kind: ableton_track_kind(&t.kind).to_string(),
            slots: ableton_nodes_to_slots(&t.chain, &mut pos),
        }
    }).collect();
    LiveProject { path: path.to_string(), daw: "Ableton".to_string(), tracks }
}

// ── Logic Pro ────────────────────────────────────────────────────────────────

fn logic_track_kind(kind: logicpro::TrackKind) -> &'static str {
    match kind {
        logicpro::TrackKind::Audio => "audio",
        logicpro::TrackKind::SoftwareInstrument => "instrument",
        logicpro::TrackKind::Aux => "bus",
        logicpro::TrackKind::Master => "master",
        logicpro::TrackKind::Unknown => "unknown",
    }
}

fn b64_encode(data: &[u8]) -> String {
    const A: &[u8; 64] =
        b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity((data.len() + 2) / 3 * 4);
    for chunk in data.chunks(3) {
        let b0 = chunk[0] as usize;
        let b1 = chunk.get(1).copied().unwrap_or(0) as usize;
        let b2 = chunk.get(2).copied().unwrap_or(0) as usize;
        let n = (b0 << 16) | (b1 << 8) | b2;
        out.push(A[(n >> 18) & 0x3F] as char);
        out.push(A[(n >> 12) & 0x3F] as char);
        out.push(if chunk.len() > 1 { A[(n >> 6) & 0x3F] as char } else { '=' });
        out.push(if chunk.len() > 2 { A[n & 0x3F] as char } else { '=' });
    }
    out
}

fn from_logic_project(project: &logicpro::LogicProject, path: &str) -> LiveProject {
    let tracks = project.tracks.iter().map(|t| {
        let slots = t.devices.iter().enumerate().map(|(i, d)| {
            LiveSlot {
                position: i as i32,
                name: d.name.clone(),
                vendor: Some(d.manufacturer.clone()),
                format: Some("AU".to_string()),
                bypass: d.bypassed,
                wet: 1.0,
                preset_name: None,
                plugin_identity: json!({
                    "name": d.name,
                    "vendor": d.manufacturer,
                    "format": "AU",
                    "component_type": d.component_type,
                    "component_subtype": d.component_subtype,
                }),
                opaque_state: if d.state.is_empty() {
                    None
                } else {
                    Some(b64_encode(&d.state))
                },
            }
        }).collect();
        LiveTrack {
            name: t.name.clone(),
            kind: logic_track_kind(t.kind).to_string(),
            slots,
        }
    }).collect();
    LiveProject { path: path.to_string(), daw: "Logic".to_string(), tracks }
}

// ── Reaper ───────────────────────────────────────────────────────────────────

fn reaper_fmt_str(f: &reaper::PluginFormat) -> &'static str {
    match f {
        reaper::PluginFormat::Vst2 => "VST2",
        reaper::PluginFormat::Vst3 => "VST3",
        reaper::PluginFormat::Au => "AU",
        reaper::PluginFormat::Clap => "CLAP",
        reaper::PluginFormat::Unknown => "Unknown",
    }
}

fn from_reaper_project(project: &reaper::ReaperProject, path: &str) -> LiveProject {
    let tracks = project.tracks.iter().map(|t| {
        let slots = t.fx_chain.iter().enumerate().map(|(i, fx)| {
            let fmt = reaper_fmt_str(&fx.format);
            LiveSlot {
                position: i as i32,
                name: fx.name.clone(),
                vendor: fx.vendor.clone(),
                format: Some(fmt.to_string()),
                bypass: fx.is_bypassed,
                wet: 1.0,
                preset_name: fx.preset_name.clone(),
                plugin_identity: json!({
                    "name": fx.name,
                    "vendor": fx.vendor,
                    "format": fmt,
                    "vst3_guid": fx.vst3_guid,
                    "vst_uid": fx.vst_uid,
                }),
                opaque_state: fx.opaque_state.clone(),
            }
        }).collect();
        LiveTrack { name: t.name.clone(), kind: "audio".to_string(), slots }
    }).collect();
    LiveProject { path: path.to_string(), daw: "Reaper".to_string(), tracks }
}

// ── DAWproject ───────────────────────────────────────────────────────────────

fn dawproject_device_to_slot(device: &dawproject::Device, pos: i32) -> LiveSlot {
    let (fmt, vendor, identity) = match &device.kind {
        dawproject::DeviceKind::Vst3 { device_id, vendor, .. } => (
            "VST3",
            vendor.clone(),
            json!({ "name": device.name, "vendor": vendor, "format": "VST3", "device_id": device_id }),
        ),
        dawproject::DeviceKind::Vst2 { unique_id, vendor, .. } => (
            "VST2",
            vendor.clone(),
            json!({ "name": device.name, "vendor": vendor, "format": "VST2", "unique_id": unique_id }),
        ),
        dawproject::DeviceKind::Au { type_code, sub_type, manufacturer } => (
            "AU",
            None,
            json!({ "name": device.name, "format": "AU", "type_code": type_code, "sub_type": sub_type, "manufacturer": manufacturer }),
        ),
        dawproject::DeviceKind::Clap { id, vendor, .. } => (
            "CLAP",
            vendor.clone(),
            json!({ "name": device.name, "vendor": vendor, "format": "CLAP", "id": id }),
        ),
        dawproject::DeviceKind::Builtin => (
            "Builtin",
            None,
            json!({ "name": device.name, "format": "Builtin" }),
        ),
    };
    LiveSlot {
        position: pos,
        name: device.name.clone(),
        vendor,
        format: Some(fmt.to_string()),
        bypass: !device.is_enabled,
        wet: 1.0,
        preset_name: None,
        plugin_identity: identity,
        opaque_state: device.opaque_state.clone(),
    }
}

fn collect_dp_tracks(src: &[dawproject::Track]) -> Vec<LiveTrack> {
    let mut out = Vec::new();
    for t in src {
        if let Some(ch) = &t.channel {
            if !ch.devices.is_empty() {
                out.push(LiveTrack {
                    name: t.name.clone(),
                    kind: "audio".to_string(),
                    slots: ch.devices.iter().enumerate()
                        .map(|(i, d)| dawproject_device_to_slot(d, i as i32))
                        .collect(),
                });
            }
        }
        out.extend(collect_dp_tracks(&t.children));
    }
    out
}

fn from_dawproject_file(project: &dawproject::DawProject, path: &str) -> LiveProject {
    let daw = project.application.as_ref()
        .map(|a| a.name.clone())
        .unwrap_or_else(|| "DAWproject".to_string());
    LiveProject { path: path.to_string(), daw, tracks: collect_dp_tracks(&project.tracks) }
}
