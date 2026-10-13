//! Minimal PE resource-directory reader (read-only, bounds-checked).
//! Enough to enumerate `type / name / language` resources of a PE32 or PE32+.

use anyhow::{Context as _, Result, bail};

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum ResName {
    Id(u32),
    Str(String),
}

impl std::fmt::Display for ResName {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Id(id) => write!(f, "{id}"),
            Self::Str(s) => f.write_str(s),
        }
    }
}

pub struct Resource<'a> {
    pub kind: ResName,
    pub name: ResName,
    pub data: &'a [u8],
}

pub const RT_STRING: u32 = 6;
pub const RT_RCDATA: u32 = 10;

pub fn type_label(kind: &ResName) -> String {
    match kind {
        ResName::Id(id) => match id {
            1 => "RT_CURSOR".into(),
            2 => "RT_BITMAP".into(),
            3 => "RT_ICON".into(),
            4 => "RT_MENU".into(),
            5 => "RT_DIALOG".into(),
            6 => "RT_STRING".into(),
            10 => "RT_RCDATA".into(),
            12 => "RT_GROUP_CURSOR".into(),
            14 => "RT_GROUP_ICON".into(),
            16 => "RT_VERSION".into(),
            24 => "RT_MANIFEST".into(),
            other => format!("TYPE_{other}"),
        },
        ResName::Str(s) => s.clone(),
    }
}

struct Section {
    va: u32,
    vsize: u32,
    raw_ptr: u32,
    raw_size: u32,
}

fn u16_at(b: &[u8], off: usize) -> Result<u16> {
    let s = b.get(off..off + 2).context("truncated PE")?;
    Ok(u16::from_le_bytes([s[0], s[1]]))
}

fn u32_at(b: &[u8], off: usize) -> Result<u32> {
    let s = b.get(off..off + 4).context("truncated PE")?;
    Ok(u32::from_le_bytes([s[0], s[1], s[2], s[3]]))
}

pub fn resources(file: &[u8]) -> Result<Vec<Resource<'_>>> {
    if file.get(..2) != Some(b"MZ") {
        bail!("not a PE file");
    }
    let pe = u32_at(file, 0x3c)? as usize;
    if file.get(pe..pe + 4) != Some(b"PE\0\0") {
        bail!("missing PE signature");
    }
    let coff = pe + 4;
    let nsections = u16_at(file, coff + 2)? as usize;
    let opt_size = u16_at(file, coff + 16)? as usize;
    let opt = coff + 20;
    let data_dirs = match u16_at(file, opt)? {
        0x10b => opt + 96,  // PE32
        0x20b => opt + 112, // PE32+
        m => bail!("unknown optional header magic {m:#x}"),
    };
    let rsrc_rva = u32_at(file, data_dirs + 2 * 8)?;
    if rsrc_rva == 0 {
        return Ok(Vec::new());
    }

    let mut sections = Vec::new();
    let table = opt + opt_size;
    for i in 0..nsections {
        let s = table + i * 40;
        sections.push(Section {
            vsize: u32_at(file, s + 8)?,
            va: u32_at(file, s + 12)?,
            raw_size: u32_at(file, s + 16)?,
            raw_ptr: u32_at(file, s + 20)?,
        });
    }
    let rva_to_off = |rva: u32| -> Result<usize> {
        sections
            .iter()
            .find(|s| rva >= s.va && rva < s.va + s.vsize.max(s.raw_size))
            .map(|s| (rva - s.va + s.raw_ptr) as usize)
            .context("RVA outside sections")
    };
    let base = rva_to_off(rsrc_rva)?;

    let read_name = |field: u32| -> Result<ResName> {
        if field & 0x8000_0000 == 0 {
            return Ok(ResName::Id(field));
        }
        let off = base + (field & 0x7fff_ffff) as usize;
        let len = u16_at(file, off)? as usize;
        let units: Vec<u16> = (0..len)
            .map(|i| u16_at(file, off + 2 + 2 * i))
            .collect::<Result<_>>()?;
        Ok(ResName::Str(String::from_utf16_lossy(&units)))
    };
    let entries = |dir_off: usize| -> Result<Vec<(u32, u32)>> {
        let named = u16_at(file, dir_off + 12)? as usize;
        let ids = u16_at(file, dir_off + 14)? as usize;
        (0..named + ids)
            .map(|i| {
                let e = dir_off + 16 + i * 8;
                Ok((u32_at(file, e)?, u32_at(file, e + 4)?))
            })
            .collect()
    };

    let mut out = Vec::new();
    for (type_field, type_ptr) in entries(base)? {
        let kind = read_name(type_field)?;
        let names_dir = base + (type_ptr & 0x7fff_ffff) as usize;
        for (name_field, name_ptr) in entries(names_dir)? {
            let name = read_name(name_field)?;
            let langs_dir = base + (name_ptr & 0x7fff_ffff) as usize;
            for (_lang, data_ptr) in entries(langs_dir)? {
                if data_ptr & 0x8000_0000 != 0 {
                    continue; // deeper nesting is not used by real binaries
                }
                let entry = base + data_ptr as usize;
                let rva = u32_at(file, entry)?;
                let size = u32_at(file, entry + 4)? as usize;
                let off = rva_to_off(rva)?;
                let data = file
                    .get(off..off + size)
                    .context("resource data out of bounds")?;
                out.push(Resource {
                    kind: kind.clone(),
                    name: name.clone(),
                    data,
                });
            }
        }
    }
    Ok(out)
}

/// Decode an RT_STRING block (16 length-prefixed UTF-16 strings).
pub fn string_table(block_id: u32, data: &[u8]) -> Vec<(u32, String)> {
    let mut out = Vec::new();
    let mut p = 0usize;
    for i in 0..16 {
        let Ok(len) = u16_at(data, p) else { break };
        p += 2;
        let len = len as usize;
        let units: Vec<u16> = (0..len)
            .filter_map(|j| u16_at(data, p + 2 * j).ok())
            .collect();
        p += 2 * len;
        if !units.is_empty() {
            out.push(((block_id - 1) * 16 + i, String::from_utf16_lossy(&units)));
        }
    }
    out
}
