use std::path::{Path, PathBuf};

/// Marker file that switches TermCraft into portable mode when it sits next
/// to the executable. In portable mode all state lives beside the binary.
pub const PORTABLE_MARKER: &str = "termcraft.portable";

/// A Windows program by its full path in `%SystemRoot%\System32`: never
/// looked up in the current folder or `PATH`, where a file with the same
/// name could stand in for it.
pub fn system32(name: &str) -> std::path::PathBuf {
    windows_dir().join("System32").join(name)
}

/// `%SystemRoot%` (`C:\Windows` when unset).
pub fn windows_dir() -> std::path::PathBuf {
    std::env::var_os("SystemRoot")
        .filter(|r| !r.is_empty())
        .map_or_else(
            || std::path::PathBuf::from(r"C:\Windows"),
            std::path::PathBuf::from,
        )
}

/// MobaXterm's path placeholders in imported settings: `_MobaFolder_` (the
/// folder of MobaXterm and its MobaXterm.ini), `_CurrentDrive_` (that
/// folder's drive) and `_ProfileDir_` (the user's profile). Other text is
/// unchanged.
pub fn expand_mobaxterm_path(value: &str, moba_folder: &Path) -> String {
    if !value.contains('_') {
        return value.to_string();
    }
    let folder = moba_folder.display().to_string();
    let folder = folder.trim_end_matches(['\\', '/']);
    let drive = folder
        .get(..2)
        .filter(|d| d.ends_with(':'))
        .unwrap_or_default()
        .to_string();
    let profile = directories::BaseDirs::new()
        .map(|b| b.home_dir().display().to_string())
        .unwrap_or_default();
    let out = value
        .replace("_MobaFolder_", folder)
        .replace("_CurrentDrive_", &drive)
        .replace("_ProfileDir_", &profile);
    // MobaXterm paths are Windows paths.
    if out != value && !cfg!(windows) {
        return out.replace('\\', "/");
    }
    out
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Paths {
    pub config_dir: PathBuf,
    pub data_dir: PathBuf,
    pub portable: bool,
}

impl Paths {
    /// Resolve storage locations: portable if the marker exists next to the
    /// executable, otherwise the platform's standard config/data directories.
    pub fn resolve() -> Self {
        if let Some(dir) = std::env::current_exe()
            .ok()
            .and_then(|exe| exe.parent().map(Path::to_path_buf))
            && dir.join(PORTABLE_MARKER).exists()
        {
            return Self::portable(&dir);
        }
        match directories::ProjectDirs::from("", "", "TermCraft") {
            Some(dirs) => Self {
                config_dir: dirs.config_dir().to_path_buf(),
                data_dir: dirs.data_dir().to_path_buf(),
                portable: false,
            },
            None => Self::portable(Path::new(".")),
        }
    }

    pub fn portable(base: &Path) -> Self {
        let root = base.join("termcraft-data");
        Self {
            config_dir: root.clone(),
            data_dir: root,
            portable: true,
        }
    }

    pub fn sessions_file(&self) -> PathBuf {
        self.config_dir.join("sessions.toml")
    }

    pub fn settings_file(&self) -> PathBuf {
        self.config_dir.join("settings.toml")
    }

    /// OpenSSH-format known_hosts used by the native SSH client.
    pub fn known_hosts_file(&self) -> PathBuf {
        self.data_dir.join("known_hosts")
    }

    pub fn macros_file(&self) -> PathBuf {
        self.config_dir.join("macros.toml")
    }

    pub fn tunnels_file(&self) -> PathBuf {
        self.config_dir.join("tunnels.toml")
    }

    /// Port and token of the running window's control channel.
    pub fn control_file(&self) -> PathBuf {
        self.data_dir.join("control.json")
    }

    /// Names of saved passwords (the secrets live in the OS keychain).
    pub fn passwords_index_file(&self) -> PathBuf {
        self.data_dir.join("saved_passwords.toml")
    }
}

#[cfg(test)]
mod placeholder_tests {
    use super::*;

    #[test]
    fn mobaxterm_placeholders() {
        let base = if cfg!(windows) {
            Path::new(r"D:\Tools\MobaXterm")
        } else {
            Path::new("/opt/moba")
        };
        let key = expand_mobaxterm_path(r"_MobaFolder_\keys\id.ppk", base);
        if cfg!(windows) {
            assert_eq!(key, r"D:\Tools\MobaXterm\keys\id.ppk");
            assert_eq!(expand_mobaxterm_path(r"_CurrentDrive_\x", base), r"D:\x");
        } else {
            assert_eq!(key, "/opt/moba/keys/id.ppk");
        }
        assert_eq!(expand_mobaxterm_path("plain/path", base), "plain/path");
        assert_eq!(
            expand_mobaxterm_path("_Std_Colors_0_", base),
            "_Std_Colors_0_"
        );
    }
}
