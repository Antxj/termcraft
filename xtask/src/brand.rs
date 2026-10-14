//! `cargo xtask brand`: a ready-made portable TermCraft for a team (the
//! reference "Customizer"): the programs, default settings and sessions
//! (a MobaXterm export or a TermCraft `sessions.toml`) in one folder.

use anyhow::{Context, Result, bail};
use std::path::{Path, PathBuf};

pub struct Brand<'a> {
    pub bin_dir: &'a Path,
    pub out: &'a Path,
    /// A TermCraft `settings.toml` with the defaults to ship.
    pub settings: Option<&'a Path>,
    /// `.mxtsessions` / `MobaXterm.ini`, or a TermCraft `sessions.toml`.
    pub sessions: Option<&'a Path>,
}

fn exe(name: &str) -> String {
    format!("{name}{}", std::env::consts::EXE_SUFFIX)
}

/// Build the folder; returns it.
pub fn run(b: &Brand<'_>) -> Result<PathBuf> {
    let folder = b.out.join("TermCraft");
    let data = folder.join("termcraft-data");
    std::fs::create_dir_all(&data)?;
    for name in ["termcraft", "termcraft-cli"] {
        let from = b.bin_dir.join(exe(name));
        if !from.is_file() {
            bail!("{} is missing (build with --release first)", from.display());
        }
        std::fs::copy(&from, folder.join(exe(name)))?;
    }
    std::fs::write(folder.join("termcraft.portable"), b"")?;
    if let Some(path) = b.settings {
        let text = std::fs::read_to_string(path).with_context(|| path.display().to_string())?;
        // Refuse settings TermCraft could not read.
        let settings: tc_core::Settings = toml::from_str(&text)
            .with_context(|| format!("{} is not a TermCraft settings file", path.display()))?;
        settings.save(&data.join("settings.toml"))?;
    }
    if let Some(path) = b.sessions {
        let store = sessions(path)?;
        store.save(&data.join("sessions.toml"))?;
        println!("{} session(s) included", store.sessions.len());
    }
    Ok(folder)
}

fn sessions(path: &Path) -> Result<tc_core::SessionStore> {
    let is_toml = path
        .extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("toml"));
    if is_toml {
        return tc_core::SessionStore::load(path).with_context(|| path.display().to_string());
    }
    let text = tc_import::read_text_lossy(path).with_context(|| path.display().to_string())?;
    let report = tc_import::mobaxterm::parse(&text);
    for u in &report.unsupported {
        println!("skipped: {u:?}");
    }
    let mut store = tc_core::SessionStore::default();
    store.sessions = report.sessions;
    Ok(store)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_a_portable_folder() {
        let dir = tempfile::tempdir().unwrap();
        for n in ["termcraft", "termcraft-cli"] {
            std::fs::write(dir.path().join(exe(n)), b"bin").unwrap();
        }
        let settings = dir.path().join("defaults.toml");
        std::fs::write(&settings, "[view]\ntheme = \"dark\"\n").unwrap();
        let sessions = dir.path().join("team.mxtsessions");
        std::fs::write(
            &sessions,
            "[Bookmarks]\nSubRep=\nImgNum=42\nweb=#109#0%web.example%22%ops%%-1%-1%%%%%0%0%0%%%-1%0%0%0%%1080%%0%0%1#MobaFont#0# #-1\n",
        )
        .unwrap();
        let out = dir.path().join("out");
        let folder = run(&Brand {
            bin_dir: dir.path(),
            out: &out,
            settings: Some(&settings),
            sessions: Some(&sessions),
        })
        .unwrap();
        assert!(folder.join("termcraft.portable").is_file());
        let store =
            tc_core::SessionStore::load(&folder.join("termcraft-data/sessions.toml")).unwrap();
        assert_eq!(store.sessions[0].name, "web");
        let s = tc_core::Settings::load(&folder.join("termcraft-data/settings.toml")).unwrap();
        assert_eq!(s.view.theme, tc_core::settings::Theme::Dark);
        std::fs::write(&settings, "view = 3").unwrap();
        assert!(
            run(&Brand {
                bin_dir: dir.path(),
                out: &out,
                settings: Some(&settings),
                sessions: None,
            })
            .is_err()
        );
    }
}
