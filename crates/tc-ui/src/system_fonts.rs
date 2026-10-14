//! The fonts installed on this computer, for the terminal font choice:
//! family names, monospace and bold faces. Only the few tables needed
//! (`name`, `post`, `OS/2`, `head`) are read from each file, so scanning
//! hundreds of fonts stays fast; the chosen face is loaded in full later.

use std::collections::BTreeMap;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

/// One face of a font file (a `.ttc` holds several).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Face {
    pub family: String,
    pub bold: bool,
    pub italic: bool,
    pub monospace: bool,
    /// Face index in a font collection (0 for single fonts).
    pub index: u32,
}

/// Where a face is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FaceRef {
    pub path: PathBuf,
    pub index: u32,
}

/// Upright faces of one family.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Family {
    pub regular: Option<FaceRef>,
    pub bold: Option<FaceRef>,
    pub monospace: bool,
}

/// Installed fonts by family name.
#[derive(Debug, Clone, Default)]
pub struct Index {
    pub families: BTreeMap<String, Family>,
}

impl Index {
    /// Monospace families, sorted.
    pub fn monospace(&self) -> Vec<String> {
        self.families
            .iter()
            .filter(|(_, f)| f.monospace && f.regular.is_some())
            .map(|(name, _)| name.clone())
            .collect()
    }

    /// The face to use for `family` (bold if asked and installed).
    pub fn face(&self, family: &str, bold: bool) -> Option<&FaceRef> {
        let f = self.families.get(family)?;
        if bold {
            f.bold.as_ref().or(f.regular.as_ref())
        } else {
            f.regular.as_ref().or(f.bold.as_ref())
        }
    }

    fn add(&mut self, path: &Path, face: Face) {
        if face.italic || face.family.is_empty() {
            return;
        }
        let f = self.families.entry(face.family).or_default();
        f.monospace |= face.monospace;
        let r = FaceRef {
            path: path.to_path_buf(),
            index: face.index,
        };
        let slot = if face.bold {
            &mut f.bold
        } else {
            &mut f.regular
        };
        slot.get_or_insert(r);
    }
}

/// Font folders of this system (and of the user).
pub fn font_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    let home = directories::BaseDirs::new().map(|b| b.home_dir().to_path_buf());
    if cfg!(windows) {
        if let Some(windir) = std::env::var_os("WINDIR") {
            dirs.push(PathBuf::from(windir).join("Fonts"));
        }
        if let Some(local) = std::env::var_os("LOCALAPPDATA") {
            dirs.push(PathBuf::from(local).join(r"Microsoft\Windows\Fonts"));
        }
    } else if cfg!(target_os = "macos") {
        dirs.push("/System/Library/Fonts".into());
        dirs.push("/Library/Fonts".into());
        if let Some(h) = &home {
            dirs.push(h.join("Library/Fonts"));
        }
    } else {
        dirs.push("/usr/share/fonts".into());
        dirs.push("/usr/local/share/fonts".into());
        if let Some(h) = &home {
            dirs.push(h.join(".local/share/fonts"));
            dirs.push(h.join(".fonts"));
        }
    }
    dirs
}

/// Scan `dirs` (sub-folders too). Unreadable files are skipped.
pub fn scan(dirs: &[PathBuf]) -> Index {
    const MAX_FILES: usize = 10_000;
    let mut index = Index::default();
    let mut pending: Vec<(PathBuf, u8)> = dirs.iter().map(|d| (d.clone(), 0)).collect();
    let mut seen = 0;
    while let Some((dir, depth)) = pending.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let Ok(kind) = entry.file_type() else {
                continue;
            };
            if kind.is_dir() {
                if depth < 6 {
                    pending.push((path, depth + 1));
                }
                continue;
            }
            let ext = path
                .extension()
                .and_then(|e| e.to_str())
                .map(str::to_ascii_lowercase);
            if !matches!(ext.as_deref(), Some("ttf" | "otf" | "ttc" | "otc")) {
                continue;
            }
            seen += 1;
            if seen > MAX_FILES {
                return index;
            }
            if let Ok(mut file) = std::fs::File::open(&path)
                && let Ok(faces) = faces(&mut file)
            {
                for face in faces {
                    index.add(&path, face);
                }
            }
        }
    }
    index
}

fn be16(b: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_be_bytes(b.get(at..at + 2)?.try_into().ok()?))
}

fn be32(b: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_be_bytes(b.get(at..at + 4)?.try_into().ok()?))
}

fn read_at(f: &mut (impl Read + Seek), offset: u64, len: usize) -> std::io::Result<Vec<u8>> {
    f.seek(SeekFrom::Start(offset))?;
    let mut buf = Vec::with_capacity(len);
    f.take(len as u64).read_to_end(&mut buf)?;
    Ok(buf)
}

/// The faces of one font file (single font or collection).
pub fn faces(f: &mut (impl Read + Seek)) -> std::io::Result<Vec<Face>> {
    let head = read_at(f, 0, 12)?;
    let bad = || std::io::Error::other("not a font");
    if head.get(..4) == Some(b"ttcf") {
        let count = be32(&head, 8).ok_or_else(bad)?.min(64);
        let offsets = read_at(f, 12, 4 * count as usize)?;
        let mut out = Vec::new();
        for i in 0..count {
            let Some(offset) = be32(&offsets, 4 * i as usize) else {
                break;
            };
            if let Some(face) = face(f, u64::from(offset), i)? {
                out.push(face);
            }
        }
        return Ok(out);
    }
    Ok(face(f, 0, 0)?.into_iter().collect())
}

/// One face whose table directory starts at `offset`.
fn face(f: &mut (impl Read + Seek), offset: u64, index: u32) -> std::io::Result<Option<Face>> {
    let dir = read_at(f, offset, 12)?;
    let Some(count) = be16(&dir, 4) else {
        return Ok(None);
    };
    let records = read_at(f, offset + 12, 16 * usize::from(count.min(256)))?;
    let mut tables: BTreeMap<[u8; 4], (u32, u32)> = BTreeMap::new();
    for r in records.as_chunks::<16>().0 {
        let tag: [u8; 4] = r[..4].try_into().unwrap_or_default();
        if let (Some(at), Some(len)) = (be32(r, 8), be32(r, 12)) {
            tables.insert(tag, (at, len));
        }
    }
    let mut table = |tag: &[u8; 4], max: u32| -> std::io::Result<Option<Vec<u8>>> {
        match tables.get(tag) {
            Some(&(at, len)) => read_at(f, u64::from(at), len.min(max) as usize).map(Some),
            None => Ok(None),
        }
    };
    let Some(family) = table(b"name", 256 * 1024)?.as_deref().and_then(family_name) else {
        return Ok(None);
    };
    let post = table(b"post", 16)?;
    let os2 = table(b"OS/2", 64)?;
    let head = table(b"head", 54)?;
    let monospace = post
        .as_deref()
        .and_then(|p| be32(p, 12))
        .is_some_and(|v| v != 0);
    let weight = os2.as_deref().and_then(|t| be16(t, 4)).unwrap_or(400);
    let selection = os2.as_deref().and_then(|t| be16(t, 62)).unwrap_or(0);
    let mac_style = head.as_deref().and_then(|t| be16(t, 44)).unwrap_or(0);
    Ok(Some(Face {
        family,
        bold: weight >= 600 || selection & 0x20 != 0 || mac_style & 1 != 0,
        italic: selection & 1 != 0 || mac_style & 2 != 0,
        monospace,
        index,
    }))
}

/// Typographic family (name id 16), else family (id 1); Windows English
/// names first.
fn family_name(t: &[u8]) -> Option<String> {
    let count = usize::from(be16(t, 2)?);
    let strings = usize::from(be16(t, 4)?);
    let mut best: Option<(u8, String)> = None;
    for i in 0..count.min(2048) {
        let r = 6 + 12 * i;
        let (platform, language, id) = (be16(t, r)?, be16(t, r + 4)?, be16(t, r + 6)?);
        let (len, at) = (usize::from(be16(t, r + 8)?), usize::from(be16(t, r + 10)?));
        if id != 1 && id != 16 {
            continue;
        }
        let Some(raw) = t.get(strings + at..strings + at + len) else {
            continue;
        };
        let text = match platform {
            0 | 3 => {
                let units: Vec<u16> = raw
                    .as_chunks::<2>()
                    .0
                    .iter()
                    .map(|c| u16::from_be_bytes(*c))
                    .collect();
                String::from_utf16_lossy(&units)
            }
            1 => raw.iter().map(|&b| char::from(b)).collect(),
            _ => continue,
        };
        let text = text.trim().to_string();
        if text.is_empty() {
            continue;
        }
        // Lower is better: typographic family before family, English
        // Windows names before the others.
        let rank = u8::from(id != 16) * 4
            + u8::from(platform != 3) * 2
            + u8::from(platform == 3 && language != 0x409);
        if best.as_ref().is_none_or(|(r, _)| rank < *r) {
            best = Some((rank, text));
        }
    }
    best.map(|(_, name)| name)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bundled(name: &str) -> Vec<u8> {
        egui::FontDefinitions::default().font_data[name]
            .font
            .to_vec()
    }

    #[test]
    fn reads_the_bundled_fonts() {
        let hack = faces(&mut std::io::Cursor::new(bundled("Hack"))).unwrap();
        assert_eq!(hack.len(), 1);
        assert_eq!(hack[0].family, "Hack");
        assert!(hack[0].monospace && !hack[0].bold && !hack[0].italic);

        let ubuntu = faces(&mut std::io::Cursor::new(bundled("Ubuntu-Light"))).unwrap();
        assert!(ubuntu[0].family.starts_with("Ubuntu"), "{ubuntu:?}");
        assert!(!ubuntu[0].monospace);
    }

    #[test]
    fn garbage_is_not_a_font() {
        let mut junk = std::io::Cursor::new(vec![7u8; 40]);
        assert!(faces(&mut junk).map(|f| f.is_empty()).unwrap_or(true));
        assert!(
            faces(&mut std::io::Cursor::new(Vec::new()))
                .map(|f| f.is_empty())
                .unwrap_or(true)
        );
    }

    /// `cargo test -p tc-ui scan_this_computer -- --ignored --nocapture`
    #[test]
    #[ignore = "reads this computer's fonts; for manual runs"]
    fn scan_this_computer() {
        let start = std::time::Instant::now();
        let index = scan(&font_dirs());
        let mono = index.monospace();
        println!(
            "{} families, {} monospace in {:?}: {mono:?}",
            index.families.len(),
            mono.len(),
            start.elapsed()
        );
    }

    #[test]
    fn index_and_scan() {
        let dir = tempfile::tempdir().unwrap();
        let sub = dir.path().join("a/b");
        std::fs::create_dir_all(&sub).unwrap();
        std::fs::write(sub.join("Hack-Regular.TTF"), bundled("Hack")).unwrap();
        std::fs::write(dir.path().join("notes.txt"), b"x").unwrap();
        std::fs::write(dir.path().join("broken.ttf"), b"nope").unwrap();
        let index = scan(&[dir.path().to_path_buf()]);
        assert_eq!(index.monospace(), ["Hack"]);
        let face = index.face("Hack", true).unwrap();
        assert!(
            face.path.ends_with("Hack-Regular.TTF"),
            "bold falls back to regular"
        );
        assert!(index.face("Nope", false).is_none());
    }
}
