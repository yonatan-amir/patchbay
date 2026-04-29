//! Writer for Logic Pro channel strip settings (`.cst`).
//!
//! # Not yet implemented
//! Logic `.cst` files are NSKeyedArchiver binary plists.  The exact key
//! structure is undocumented and differs between Logic versions.  Per
//! project policy we do not implement a writer without first inspecting a
//! real sample file (`hexdump -C | head -200`, `plutil -convert xml1`, etc.)
//! on macOS — the NSKeyedArchiver assumption already caused one full rewrite.
//!
//! To implement: SSH to the Mac, capture a `.cst` export from Logic Pro,
//! reverse-engineer the plist structure, then write the serialiser here.

use crate::db::ChainDetail;

/// Returns `Err` with a clear message until the format is implemented.
pub fn write_cst(_chain: &ChainDetail) -> Result<Vec<u8>, String> {
    Err(
        "Logic .cst export is not yet implemented. \
         Inspect a real .cst file on macOS first (see daw_writers/logicpro.rs)."
            .to_string(),
    )
}
