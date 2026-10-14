//! `cargo xtask <cmd>`: development tasks.
//!
//! `extract`   — static extraction of the reference app into `reference/extracted/`
//!               (nothing is executed; see docs/CLEAN_ROOM.md).
//! `inventory` — generate parity inventories into `docs/inventory/`.
//! `brand`     — a ready-made portable folder with default settings and
//!               sessions for a team.
//! `app-icon`  — the program icon (PNG, .ico, .icns).
//! `package`   — release packages (portable zip, .deb, macOS .app) from
//!               built binaries.

mod app_icon;
mod brand;
mod dfm;
mod icons;
mod inventory;
mod package;
mod pe;
mod schema;

use anyhow::{Context as _, Result, bail};
use clap::{Parser, Subcommand};
use std::collections::BTreeMap;
use std::io::Read as _;
use std::path::{Path, PathBuf};

#[derive(Parser)]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Extract forms, resources and strings from the reference app.
    Extract {
        /// Portable zip or executable; default: newest zip in reference/download/.
        input: Option<PathBuf>,
    },
    /// Generate docs/inventory/*.md from the reference app.
    Inventory { input: Option<PathBuf> },
    /// Generate crates/tc-core/src/schema_gen.rs (session parameter schema).
    Schema { input: Option<PathBuf> },
    /// SVG icons for the README (docs/icons/).
    Icons,
    /// The program icon: PNG, .ico and .icns (crates/tc-ui/assets/).
    AppIcon,
    /// A portable TermCraft with default settings and sessions for a team.
    Brand {
        /// Folder with termcraft and termcraft-cli (default: target/release).
        #[arg(long)]
        bin_dir: Option<PathBuf>,
        /// TermCraft settings.toml with the defaults.
        #[arg(long)]
        settings: Option<PathBuf>,
        /// MobaXterm sessions export or TermCraft sessions.toml.
        #[arg(long)]
        sessions: Option<PathBuf>,
        /// Output folder (default: dist/brand).
        #[arg(long)]
        out: Option<PathBuf>,
    },
    /// Build release packages from binaries (`cargo build --release`).
    Package {
        /// Folder with termcraft and termcraft-cli (default: target/release).
        #[arg(long)]
        bin_dir: Option<PathBuf>,
        /// windows-x86_64, linux-x86_64, macos-aarch64 … (default: this one).
        #[arg(long)]
        platform: Option<String>,
        /// Package version (default: the workspace version).
        #[arg(long)]
        version: Option<String>,
        /// Output folder (default: dist/packages).
        #[arg(long)]
        out: Option<PathBuf>,
    },
}

/// `<os>-<arch>` of this machine, as release assets name it.
fn host_platform() -> String {
    let os = match std::env::consts::OS {
        "macos" => "macos",
        "windows" => "windows",
        _ => "linux",
    };
    format!("{os}-{}", std::env::consts::ARCH)
}

pub struct Reference {
    pub version: String,
    pub forms: BTreeMap<String, dfm::Object>,
    /// Decompressed `EXEFILE` resources; zips are kept as raw zip bytes.
    pub files: BTreeMap<String, Vec<u8>>,
    pub strings: Vec<(u32, String)>,
    pub manifest: Vec<(String, String, usize)>,
}

/// `[workspace.package] version` of the root Cargo.toml.
fn workspace_version() -> Result<String> {
    let text = std::fs::read_to_string(root().join("Cargo.toml"))?;
    let section = text
        .split("[workspace.package]")
        .nth(1)
        .context("no [workspace.package]")?;
    section
        .lines()
        .find_map(|l| {
            let v = l
                .trim()
                .strip_prefix("version")?
                .trim_start()
                .strip_prefix('=')?;
            Some(v.trim().trim_matches('"').to_string())
        })
        .context("no workspace version")
}

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("xtask lives in the workspace")
        .to_path_buf()
}

fn main() -> Result<()> {
    match Cli::parse().cmd {
        Cmd::Extract { input } => extract(&load(input)?),
        Cmd::Inventory { input } => {
            let reference = load(input)?;
            let out = root().join("docs/inventory");
            inventory::write_all(&reference, &out)?;
            println!("wrote {}", out.display());
            Ok(())
        }
        Cmd::Icons => {
            let out = root().join("docs/icons");
            let n = icons::write_all(&out)?;
            println!("wrote {n} icons to {}", out.display());
            Ok(())
        }
        Cmd::AppIcon => {
            app_icon::write_all(&root())?;
            println!("wrote the program icon to crates/tc-ui/assets/");
            Ok(())
        }
        Cmd::Brand {
            bin_dir,
            settings,
            sessions,
            out,
        } => {
            let bin_dir = bin_dir.unwrap_or_else(|| root().join("target/release"));
            let out = out.unwrap_or_else(|| root().join("dist/brand"));
            let folder = brand::run(&brand::Brand {
                bin_dir: &bin_dir,
                out: &out,
                settings: settings.as_deref(),
                sessions: sessions.as_deref(),
            })?;
            println!("wrote {}", folder.display());
            Ok(())
        }
        Cmd::Package {
            bin_dir,
            platform,
            version,
            out,
        } => {
            let bin_dir = bin_dir.unwrap_or_else(|| root().join("target/release"));
            let platform = platform.unwrap_or_else(host_platform);
            let version = version.unwrap_or_else(|| env!("CARGO_PKG_VERSION").to_string());
            let version = if version == "0.0.0" {
                workspace_version()?
            } else {
                version
            };
            let out = out.unwrap_or_else(|| root().join("dist/packages"));
            for made in package::run(&package::Package {
                bin_dir: &bin_dir,
                platform: &platform,
                version: &version,
                out: &out,
            })? {
                println!("wrote {}", made.display());
            }
            Ok(())
        }
        Cmd::Schema { input } => {
            let code = schema::generate(&load(input)?)?;
            let out = root().join("crates/tc-core/src/schema_gen.rs");
            std::fs::write(&out, code)?;
            println!("wrote {}", out.display());
            Ok(())
        }
    }
}

fn default_input() -> Result<PathBuf> {
    let dir = root().join("reference/download");
    let mut zips: Vec<_> = std::fs::read_dir(&dir)
        .with_context(|| format!("{} (download the portable zip there)", dir.display()))?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|x| x.eq_ignore_ascii_case("zip")))
        .collect();
    zips.sort();
    zips.pop().context("no zip in reference/download")
}

/// Read the main executable, from a zip or directly.
fn read_exe(input: &Path) -> Result<Vec<u8>> {
    if input
        .extension()
        .is_some_and(|x| x.eq_ignore_ascii_case("zip"))
    {
        let mut zip = zip::ZipArchive::new(std::fs::File::open(input)?)?;
        for i in 0..zip.len() {
            let mut entry = zip.by_index(i)?;
            if entry.name()?.to_ascii_lowercase().ends_with(".exe") {
                let mut buf = Vec::new();
                entry.read_to_end(&mut buf)?;
                return Ok(buf);
            }
        }
        bail!("no .exe inside {}", input.display());
    }
    Ok(std::fs::read(input)?)
}

fn load(input: Option<PathBuf>) -> Result<Reference> {
    let input = match input {
        Some(p) => p,
        None => default_input()?,
    };
    let exe = read_exe(&input)?;
    let resources = pe::resources(&exe)?;

    let mut forms = BTreeMap::new();
    let mut files = BTreeMap::new();
    let mut strings = Vec::new();
    let mut manifest = Vec::new();
    for r in &resources {
        let kind = pe::type_label(&r.kind);
        let name = r.name.to_string();
        manifest.push((kind.clone(), name.clone(), r.data.len()));
        match (&r.kind, kind.as_str()) {
            (pe::ResName::Id(pe::RT_RCDATA), _) if dfm::is_form(r.data) => {
                let form = dfm::parse(r.data).with_context(|| format!("form {name}"))?;
                forms.insert(name, form);
            }
            (pe::ResName::Id(pe::RT_STRING), _) => {
                if let pe::ResName::Id(block) = r.name {
                    strings.extend(pe::string_table(block, r.data));
                }
            }
            (_, "EXEFILE") => {
                // LZMA-alone streams; anything else (fonts, …) is kept raw.
                let mut out = Vec::new();
                let data =
                    match lzma_rs::lzma_decompress(&mut std::io::Cursor::new(r.data), &mut out) {
                        Ok(()) => out,
                        Err(_) => r.data.to_vec(),
                    };
                files.insert(name, data);
            }
            _ => {}
        }
    }
    let version = version_string(&exe).unwrap_or_else(|| "unknown".into());
    Ok(Reference {
        version,
        forms,
        files,
        strings,
        manifest,
    })
}

/// ProductVersion from the version resource (UTF-16 key/value scan).
fn version_string(exe: &[u8]) -> Option<String> {
    let key: Vec<u8> = "FileVersion\0"
        .encode_utf16()
        .flat_map(u16::to_le_bytes)
        .collect();
    let pos = exe.windows(key.len()).position(|w| w == key)? + key.len();
    let rest = &exe[pos..];
    let start = rest.as_chunks::<2>().0.iter().position(|c| c != &[0, 0])? * 2;
    let units: Vec<u16> = rest[start..]
        .as_chunks::<2>()
        .0
        .iter()
        .map(|c| u16::from_le_bytes(*c))
        .take_while(|&u| u != 0)
        .collect();
    Some(String::from_utf16_lossy(&units))
}

fn extract(r: &Reference) -> Result<()> {
    let out = root().join("reference/extracted");
    let forms_dir = out.join("forms");
    std::fs::create_dir_all(&forms_dir)?;
    for (name, form) in &r.forms {
        std::fs::write(forms_dir.join(format!("{name}.dfm")), dfm::to_text(form))?;
    }

    let files_dir = out.join("exefile");
    std::fs::create_dir_all(&files_dir)?;
    for (name, data) in &r.files {
        if data.starts_with(b"PK\x03\x04") {
            unzip_lenient(data, &files_dir.join(name))?;
        } else {
            std::fs::write(files_dir.join(format!("{name}.bin")), data)?;
        }
    }

    let strings: String = r
        .strings
        .iter()
        .map(|(id, s)| format!("{id}\t{}\n", s.escape_debug()))
        .collect();
    std::fs::write(out.join("rt_string.tsv"), strings)?;
    let manifest: String = r
        .manifest
        .iter()
        .map(|(k, n, size)| format!("{k}\t{n}\t{size}\n"))
        .collect();
    std::fs::write(out.join("manifest.tsv"), manifest)?;

    println!(
        "{}: {} forms, {} embedded files, {} strings → {}",
        r.version,
        r.forms.len(),
        r.files.len(),
        r.strings.len(),
        out.display()
    );
    Ok(())
}

/// Unpack a zip, skipping entries that are unsafe or fail their CRC.
fn unzip_lenient(data: &[u8], dest: &Path) -> Result<()> {
    let mut zip = match zip::ZipArchive::new(std::io::Cursor::new(data)) {
        Ok(zip) => zip,
        Err(e) => {
            eprintln!("  {}: not a readable zip ({e}); kept raw", dest.display());
            std::fs::write(dest.with_extension("zip"), data)?;
            return Ok(());
        }
    };
    let mut skipped = 0;
    for i in 0..zip.len() {
        let mut entry = zip.by_index(i)?;
        let Some(rel) = entry.enclosed_name() else {
            skipped += 1;
            continue;
        };
        let target = dest.join(rel);
        if entry.is_dir() {
            std::fs::create_dir_all(&target)?;
            continue;
        }
        let mut buf = Vec::new();
        if entry.read_to_end(&mut buf).is_err() {
            skipped += 1;
            continue;
        }
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(target, buf)?;
    }
    if skipped > 0 {
        eprintln!("  {}: skipped {skipped} unreadable entries", dest.display());
    }
    Ok(())
}
