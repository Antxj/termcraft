//! Interoperability importers. Formats are derived from files users export
//! from their own installations (see docs/CLEAN_ROOM.md). Importers are
//! tolerant: anything not understood is reported, never guessed.

pub mod mobaxterm;

use std::path::Path;

/// Read a text file that may be UTF-8 (with or without BOM) or a legacy
/// Windows code page. Invalid bytes are replaced rather than rejected.
pub fn read_text_lossy(path: &Path) -> std::io::Result<String> {
    let bytes = std::fs::read(path)?;
    let bytes = bytes.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(&bytes);
    Ok(String::from_utf8_lossy(bytes).into_owned())
}
