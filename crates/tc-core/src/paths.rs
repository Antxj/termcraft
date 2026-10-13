use std::path::{Path, PathBuf};

/// Marker file that switches TermCraft into portable mode when it sits next
/// to the executable. In portable mode all state lives beside the binary.
pub const PORTABLE_MARKER: &str = "termcraft.portable";

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

    /// Names of saved passwords (the secrets live in the OS keychain).
    pub fn passwords_index_file(&self) -> PathBuf {
        self.data_dir.join("saved_passwords.toml")
    }
}
