//! Writer for Logic Pro channel strip settings (`.cst`).
//!
//! # Not yet implemented — two blockers
//!
//! ## Blocker 1: Logic reader returns empty device lists
//! `daw_readers/logicpro.rs` reads track names but AU plugin device entries
//! are not yet decoded. The investigation (2026-04-29) found that:
//! - Built-in Logic effects use `GAME`/`TSPP` float-parameter chunks in a
//!   global section whose track→effect mapping is not yet determined.
//! - Third-party AU state blobs were not observed in the test project.
//! - AU component `type`/`subtype`/`manufacturer` 4CC codes are absent from
//!   the `GAME`/`TSPP` structure.
//!
//! ## Blocker 2: `.cst` is Logic's proprietary `OCuA` binary format
//! Sample inspection confirmed `.cst` files begin with magic bytes
//! `4F 43 75 41` ("OCuA") — **not** a plist.  `plutil` rejects them.
//! The format uses 4-char-tagged chunks (`GAME`, `TSPP`, `UCuA`, …) with
//! embedded NSKeyedArchiver blobs. There is no public spec.
//!
//! ## Alternative path (future)
//! The standard `.aupreset` plist format is writable and Logic imports it.
//! It requires `type` / `subtype` / `manufacturer` as big-endian uint32 4CC
//! codes, which must be read from the ProjectData binary (not yet implemented).
//! This would allow single-plugin presets but not a full chain in one file.
//!
//! ## How to unblock
//! 1. Obtain a `.logicx` project with third-party AU plugins and known state.
//! 2. Map track indices to their `GAME` effect entries in the binary.
//! 3. Capture AU component codes and state blobs from the `GAME`/`UCuA` blocks.
//! 4. Choose export target: multiple `.aupreset` files (standard) OR
//!    reverse-engineer the `.cst` OCuA writer (full chain, proprietary).

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
