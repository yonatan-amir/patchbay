//! Writer for Logic Pro channel strip settings (`.cst`).
//!
//! # Not yet implemented — two blockers
//!
//! ## Blocker 1: Logic reader returns empty device lists
//! `daw_readers/logicpro.rs` reads track names but the proprietary AU plugin
//! sub-format inside `ProjectData` is not yet decoded.  Logic chains saved to
//! the DB therefore have zero slots, so there is nothing to export even if a
//! writer existed.
//!
//! ## Blocker 2: `.cst` is Logic's proprietary `OCuA` binary format
//! Sample inspection (2026-04-29) confirmed that `.cst` files begin with magic
//! bytes `4F 43 75 41` ("OCuA") — **not** a plist.  `plutil` rejects them.
//! The format is an undocumented binary tree of 4-char-tagged chunks
//! (`MELC`, `GAME`, `EDAF`, `TSPP`, …) with embedded `bplist00` state blobs.
//! There is no public spec and reverse-engineering the full structure is a
//! substantial separate effort.
//!
//! ## Alternative path (future)
//! For per-plugin AU state, the standard `.aupreset` plist format is writable
//! and Logic imports it.  It requires `type` / `subtype` / `manufacturer` as
//! big-endian uint32 4CC codes.  The Logic reader would need to be extended to
//! capture the manufacturer 4CC (currently only the vendor name string is stored).
//! This would allow single-plugin presets but not a full chain in one file.
//!
//! # How to unblock
//! 1. Extend `logicpro.rs` reader to decode AU device entries (component codes
//!    + state blobs) from the `ProjectData` binary.
//! 2. Choose export target: multiple `.aupreset` files (per-plugin, standard
//!    format) OR reverse-engineer `.cst` writer (full chain, proprietary).

use crate::db::ChainDetail;

/// Returns `Err` with a clear message until both blockers are resolved.
pub fn write_cst(_chain: &ChainDetail) -> Result<Vec<u8>, String> {
    Err(
        "Logic export is not yet available: (1) the Logic reader does not decode AU device \
         state yet, and (2) the .cst format is a proprietary OCuA binary — see \
         daw_writers/logicpro.rs for the full investigation notes."
            .to_string(),
    )
}
