//! Reader for Reaper project files (`.rpp` / `.rpp-bak`).
//!
//! RPP is plain-text LISP-like markup. Each block opens with
//! `<TAGNAME args...` and closes with a bare `>`. Attribute lines inside
//! blocks are whitespace-tokenised key/value pairs. Base64 state lines have
//! no keyword prefix.
//!
//! # Block structure
//! ```text
//! <REAPER_PROJECT 0.1 "6.77/win64" 1714123456
//!   <TRACK {A1B2C3D4-E5F6-7890-ABCD-EF1234567890}
//!     NAME "Drums"
//!     <FXCHAIN
//!       BYPASS 0 0 0
//!       <VST "VST3: Pro-Q 3 (FabFilter)" "FabFilter Pro-Q 3.vst3" 0 "" 1397572658{D8D91CE4-6A7D-4670-8BDE-4B688EC18B43}
//!         AQIDBA==
//!       >
//!     >
//!   >
//! >
//! ```
//!
//! # Plugin identification
//! The first quoted argument to `<VST` (or `<AU`) encodes the format:
//! - `"VST3: Name (Vendor)"` → VST3; GUID parsed from 5th field `uid{GUID}`
//! - `"VST: Name (Vendor)"` → VST2; numeric unique ID in 5th field
//! - `"AU: Name (Vendor)"` → Audio Unit (macOS)
//! - `"CLAP: Name (Vendor)"` → CLAP
//!
//! Bypass state is the 3rd positional field (0 = active, non-zero = bypassed).
//!
//! # State blob
//! Base64-encoded plugin state follows the opening line inside the block.
//! Multiple continuation lines are concatenated directly to form the full blob.

use std::path::Path;

use serde::{Deserialize, Serialize};
use thiserror::Error;

// ─── Errors ──────────────────────────────────────────────────────────────────

#[derive(Debug, Error)]
pub enum ReaperError {
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("Format error: {0}")]
    Format(String),
}

// ─── Public types ────────────────────────────────────────────────────────────

#[derive(Debug, Serialize, Deserialize)]
pub struct ReaperProject {
    /// RPP format version, e.g. `"0.1"`.
    pub version: String,
    /// Reaper application version string, e.g. `"6.77/win64"`.
    pub app_version: String,
    pub tracks: Vec<Track>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Track {
    pub name: String,
    pub guid: Option<String>,
    pub fx_chain: Vec<FxPlugin>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct FxPlugin {
    pub name: String,
    pub vendor: Option<String>,
    pub format: PluginFormat,
    pub filename: String,
    pub is_bypassed: bool,
    pub preset_name: Option<String>,
    /// Numeric UID portion of the VST identifier (VST2: plugin ID; VST3: leading digits).
    pub vst_uid: Option<String>,
    /// VST3-only GUID in `{XXXXXXXX-…}` form.
    pub vst3_guid: Option<String>,
    /// Base64 plugin state blob, stored verbatim.
    pub opaque_state: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum PluginFormat {
    Vst2,
    Vst3,
    Au,
    Clap,
    Unknown,
}

// ─── Public API ──────────────────────────────────────────────────────────────

pub fn read_rpp(path: &Path) -> Result<ReaperProject, ReaperError> {
    let content = std::fs::read_to_string(path)?;
    parse_rpp(&content)
}

// ─── Internal tree ───────────────────────────────────────────────────────────

struct Node {
    tag: String,
    args: Vec<String>,
    /// Non-block lines, each split into tokens by [`tokenize`].
    lines: Vec<Vec<String>>,
    children: Vec<Node>,
}

// ─── Parser ──────────────────────────────────────────────────────────────────

pub(crate) fn parse_rpp(content: &str) -> Result<ReaperProject, ReaperError> {
    let raw: Vec<&str> = content.lines().collect();
    let mut pos = 0;

    while pos < raw.len() {
        let line = raw[pos].trim();
        pos += 1;
        if let Some(rest) = line.strip_prefix('<') {
            let toks = tokenize(rest);
            if toks.first().map(|s| s.eq_ignore_ascii_case("REAPER_PROJECT")).unwrap_or(false) {
                let args: Vec<String> = toks[1..].to_vec();
                let version = args.first().cloned().unwrap_or_default();
                let app_version = args.get(1).cloned().unwrap_or_default();
                let root = read_node(&raw, &mut pos, toks[0].clone(), args);
                return Ok(build_project(version, app_version, &root));
            }
        }
    }

    Err(ReaperError::Format("REAPER_PROJECT block not found".into()))
}

fn read_node(lines: &[&str], pos: &mut usize, tag: String, args: Vec<String>) -> Node {
    let mut node = Node { tag, args, lines: Vec::new(), children: Vec::new() };

    while *pos < lines.len() {
        let line = lines[*pos].trim();
        *pos += 1;

        if line == ">" {
            break;
        }
        if line.is_empty() {
            continue;
        }
        if let Some(rest) = line.strip_prefix('<') {
            let toks = tokenize(rest);
            if let Some(first) = toks.first().cloned() {
                let child = read_node(lines, pos, first, toks[1..].to_vec());
                node.children.push(child);
            }
        } else {
            let toks = tokenize(line);
            if !toks.is_empty() {
                node.lines.push(toks);
            }
        }
    }

    node
}

fn tokenize(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut chars = s.trim().chars().peekable();

    while let Some(&c) = chars.peek() {
        if c.is_whitespace() {
            chars.next();
            continue;
        }
        if c == '"' {
            chars.next();
            let mut buf = String::new();
            loop {
                match chars.next() {
                    None | Some('"') => break,
                    Some('\\') => {
                        if let Some(esc) = chars.next() {
                            buf.push(match esc {
                                'n' => '\n',
                                'r' => '\r',
                                't' => '\t',
                                c => c,
                            });
                        }
                    }
                    Some(c) => buf.push(c),
                }
            }
            out.push(buf);
        } else {
            let mut buf = String::new();
            while let Some(&c) = chars.peek() {
                if c.is_whitespace() {
                    break;
                }
                buf.push(c);
                chars.next();
            }
            out.push(buf);
        }
    }

    out
}

// ─── Tree → domain types ─────────────────────────────────────────────────────

fn build_project(version: String, app_version: String, root: &Node) -> ReaperProject {
    let tracks = root
        .children
        .iter()
        .filter(|n| n.tag.eq_ignore_ascii_case("TRACK"))
        .map(build_track)
        .collect();
    ReaperProject { version, app_version, tracks }
}

fn build_track(node: &Node) -> Track {
    let guid = node.args.first().filter(|s| s.starts_with('{')).cloned();

    let name = attr_val(node, "NAME").unwrap_or_default();

    let fx_chain = node
        .children
        .iter()
        .find(|n| n.tag.eq_ignore_ascii_case("FXCHAIN"))
        .map(build_fxchain)
        .unwrap_or_default();

    Track { name, guid, fx_chain }
}

fn build_fxchain(node: &Node) -> Vec<FxPlugin> {
    node.children
        .iter()
        .filter(|n| matches!(n.tag.to_uppercase().as_str(), "VST" | "AU" | "CLAP"))
        .filter_map(build_fx_plugin)
        .collect()
}

fn build_fx_plugin(node: &Node) -> Option<FxPlugin> {
    let display = node.args.first()?.clone();
    let filename = node.args.get(1).cloned().unwrap_or_default();
    let is_bypassed = node.args.get(2).map(|s| s != "0").unwrap_or(false);
    let preset_name = node.args.get(3).cloned().filter(|s| !s.is_empty());
    let uid_tok = node.args.get(4).cloned();

    let (format, name, vendor) = split_display_name(&display, &node.tag);
    let (vst_uid, vst3_guid) = uid_tok.as_deref().map(split_uid).unwrap_or((None, None));
    let opaque_state = state_blob(node);

    Some(FxPlugin { name, vendor, format, filename, is_bypassed, preset_name, vst_uid, vst3_guid, opaque_state })
}

fn split_display_name(display: &str, block_tag: &str) -> (PluginFormat, String, Option<String>) {
    let (fmt, rest) = if let Some(r) = display.strip_prefix("VST3: ") {
        (PluginFormat::Vst3, r)
    } else if let Some(r) = display.strip_prefix("VST: ") {
        (PluginFormat::Vst2, r)
    } else if let Some(r) = display.strip_prefix("AU: ") {
        (PluginFormat::Au, r)
    } else if let Some(r) = display.strip_prefix("CLAP: ") {
        (PluginFormat::Clap, r)
    } else if block_tag.eq_ignore_ascii_case("AU") {
        (PluginFormat::Au, display.as_ref())
    } else if block_tag.eq_ignore_ascii_case("CLAP") {
        (PluginFormat::Clap, display.as_ref())
    } else {
        // No recognised prefix and not AU/CLAP block — assume VST2.
        (PluginFormat::Vst2, display.as_ref())
    };

    if let Some(paren) = rest.rfind('(') {
        let name = rest[..paren].trim().to_string();
        let vendor = rest[paren + 1..].trim_end_matches(')').trim().to_string();
        (fmt, name, if vendor.is_empty() { None } else { Some(vendor) })
    } else {
        (fmt, rest.to_string(), None)
    }
}

/// Parse `1397572658{D8D91CE4-…}` or `{GUID}` or `1936880749` into (uid, guid).
fn split_uid(uid: &str) -> (Option<String>, Option<String>) {
    if let Some(brace) = uid.find('{') {
        let num = &uid[..brace];
        let guid = &uid[brace..];
        (
            if num.is_empty() { None } else { Some(num.to_string()) },
            if guid.len() > 2 { Some(guid.to_string()) } else { None },
        )
    } else if !uid.is_empty() {
        (Some(uid.to_string()), None)
    } else {
        (None, None)
    }
}

/// Collect the base64 state blob from a VST/AU node.
///
/// All non-block lines that don't begin with a known Reaper attribute keyword
/// are treated as continuation base64 data and concatenated directly.
fn state_blob(node: &Node) -> Option<String> {
    const SKIP: &[&str] = &["BYPASS", "PRESETNAME", "FLOATPOS", "FXID", "SHOW", "WAK"];

    let blob: String = node
        .lines
        .iter()
        .filter(|l| {
            l.first()
                .map(|t| !SKIP.contains(&t.to_uppercase().as_str()))
                .unwrap_or(false)
        })
        .flat_map(|l| l.iter().map(|s| s.as_str()))
        .collect();

    if blob.is_empty() { None } else { Some(blob) }
}

fn attr_val(node: &Node, key: &str) -> Option<String> {
    node.lines
        .iter()
        .find(|l| l.first().map(|t| t.eq_ignore_ascii_case(key)).unwrap_or(false))
        .and_then(|l| l.get(1))
        .cloned()
}

// ─── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    const RPP: &str = r#"<REAPER_PROJECT 0.1 "6.77/win64" 1714123456
  <TRACK {A1B2C3D4-E5F6-7890-ABCD-EF1234567890}
    NAME "Drums"
    PEAKCOL 16576
    <FXCHAIN
      SHOW 0
      LASTSEL 0
      DOCKED 0
      BYPASS 0 0 0
      <VST "VST3: Pro-Q 3 (FabFilter)" "FabFilter Pro-Q 3.vst3" 0 "" 1397572658{D8D91CE4-6A7D-4670-8BDE-4B688EC18B43}
        AQIDBA==
      >
      <VST "VST: Serum (Xfer Records)" "Serum_x64.dll" 1 "Patch" 1936880749
        c2VydW0=
      >
    >
  >
  <TRACK {B2C3D4E5-F6A7-8901-BCDE-F12345678901}
    NAME "Synth"
    <FXCHAIN
      SHOW 0
      LASTSEL 0
      DOCKED 0
      BYPASS 0 0 0
      <AU "AU: AUPeakLimiter (Apple)" "" 0 "" 1819304812/aufx/lmtr/appl
        AAAAAAAAAAAAAAABAAAAAAAAAAAAAAAAAAAAAgIA=
      >
    >
  >
  <TRACK {C3D4E5F6-A7B8-9012-CDEF-012345678901}
    NAME "Bus"
  >
>"#;

    #[test]
    fn track_count() {
        let p = parse_rpp(RPP).unwrap();
        assert_eq!(p.tracks.len(), 3);
    }

    #[test]
    fn project_version_fields() {
        let p = parse_rpp(RPP).unwrap();
        assert_eq!(p.version, "0.1");
        assert_eq!(p.app_version, "6.77/win64");
    }

    #[test]
    fn track_names_and_guids() {
        let p = parse_rpp(RPP).unwrap();
        assert_eq!(p.tracks[0].name, "Drums");
        assert_eq!(p.tracks[0].guid.as_deref(), Some("{A1B2C3D4-E5F6-7890-ABCD-EF1234567890}"));
        assert_eq!(p.tracks[1].name, "Synth");
        assert_eq!(p.tracks[2].name, "Bus");
    }

    #[test]
    fn vst3_plugin_name_vendor_format_guid() {
        let p = parse_rpp(RPP).unwrap();
        let fx = &p.tracks[0].fx_chain[0];
        assert_eq!(fx.name, "Pro-Q 3");
        assert_eq!(fx.vendor.as_deref(), Some("FabFilter"));
        assert_eq!(fx.format, PluginFormat::Vst3);
        assert_eq!(fx.filename, "FabFilter Pro-Q 3.vst3");
        assert_eq!(fx.vst_uid.as_deref(), Some("1397572658"));
        assert_eq!(fx.vst3_guid.as_deref(), Some("{D8D91CE4-6A7D-4670-8BDE-4B688EC18B43}"));
    }

    #[test]
    fn vst3_active_with_state() {
        let p = parse_rpp(RPP).unwrap();
        let fx = &p.tracks[0].fx_chain[0];
        assert!(!fx.is_bypassed);
        assert_eq!(fx.opaque_state.as_deref(), Some("AQIDBA=="));
    }

    #[test]
    fn vst2_bypassed_with_preset_name() {
        let p = parse_rpp(RPP).unwrap();
        let fx = &p.tracks[0].fx_chain[1];
        assert_eq!(fx.name, "Serum");
        assert_eq!(fx.vendor.as_deref(), Some("Xfer Records"));
        assert_eq!(fx.format, PluginFormat::Vst2);
        assert!(fx.is_bypassed);
        assert_eq!(fx.preset_name.as_deref(), Some("Patch"));
        assert_eq!(fx.vst_uid.as_deref(), Some("1936880749"));
        assert!(fx.vst3_guid.is_none());
        assert_eq!(fx.opaque_state.as_deref(), Some("c2VydW0="));
    }

    #[test]
    fn au_plugin_format_and_name() {
        let p = parse_rpp(RPP).unwrap();
        let fx = &p.tracks[1].fx_chain[0];
        assert_eq!(fx.format, PluginFormat::Au);
        assert_eq!(fx.name, "AUPeakLimiter");
        assert_eq!(fx.vendor.as_deref(), Some("Apple"));
        assert!(!fx.is_bypassed);
        assert!(fx.opaque_state.is_some());
    }

    #[test]
    fn track_without_fxchain_is_empty() {
        let p = parse_rpp(RPP).unwrap();
        assert!(p.tracks[2].fx_chain.is_empty());
    }

    #[test]
    fn multiline_state_blob_concatenated() {
        let rpp = r#"<REAPER_PROJECT 0.1 "6.0" 0
  <TRACK {}
    NAME "T"
    <FXCHAIN
      BYPASS 0 0 0
      <VST "VST: Plugin (Vendor)" "plugin.dll" 0 "" 12345
        AAAA
        BBBB
        CC==
      >
    >
  >
>"#;
        let p = parse_rpp(rpp).unwrap();
        assert_eq!(p.tracks[0].fx_chain[0].opaque_state.as_deref(), Some("AAAABBBBCC=="));
    }

    #[test]
    fn empty_preset_name_returns_none() {
        let p = parse_rpp(RPP).unwrap();
        assert!(p.tracks[0].fx_chain[0].preset_name.is_none());
    }

    #[test]
    fn no_reaper_project_block_is_error() {
        assert!(parse_rpp("nothing here\n").is_err());
    }
}
