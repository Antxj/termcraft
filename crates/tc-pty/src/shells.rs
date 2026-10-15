//! "Local terminal environment": find a Bash / Zsh / Cmd / PowerShell / WSL
//! executable on this machine. On Windows, Bash and Zsh come from Git for
//! Windows or MSYS2 (`System32\bash.exe` is the WSL launcher and is skipped).

use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ShellKind {
    Bash,
    Zsh,
    Cmd,
    PowerShell,
    Wsl,
}

impl ShellKind {
    pub const ALL: [ShellKind; 5] = [
        Self::Bash,
        Self::Zsh,
        Self::Cmd,
        Self::PowerShell,
        Self::Wsl,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::Bash => "Bash",
            Self::Zsh => "Zsh",
            Self::Cmd => "Cmd",
            Self::PowerShell => "PowerShell",
            Self::Wsl => "Windows Subsystem for Linux (WSL)",
        }
    }
}

/// A resolved shell: program, arguments and extra environment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShellCommand {
    pub program: String,
    pub args: Vec<String>,
    pub env: Vec<(String, String)>,
    /// Working directory; `None` = the user's home.
    pub cwd: Option<PathBuf>,
}

fn find_in_path(exe: &str) -> Option<PathBuf> {
    std::env::split_paths(&std::env::var_os("PATH")?)
        .map(|dir| dir.join(exe))
        .find(|p| p.is_file())
}

fn existing(paths: impl IntoIterator<Item = PathBuf>) -> Option<PathBuf> {
    paths.into_iter().find(|p| p.is_file())
}

fn is_wsl_launcher(p: &Path) -> bool {
    p.to_string_lossy()
        .to_ascii_lowercase()
        .contains("\\system32\\")
}

fn windows_unix_shell(name: &str) -> Option<ShellCommand> {
    let exe = format!("{name}.exe");
    let program_files = std::env::var_os("ProgramFiles").map(PathBuf::from);
    let git = program_files.map(|pf| pf.join("Git").join("bin").join(&exe));
    let msys = PathBuf::from(r"C:\msys64\usr\bin").join(&exe);
    if let Some(p) = existing(git.into_iter().chain([msys.clone()])) {
        let mut env = Vec::new();
        if p == msys {
            // Keep the start directory and use the plain MSYS environment.
            env.push(("MSYSTEM".into(), "MSYS".into()));
            env.push(("CHERE_INVOKING".into(), "1".into()));
        }
        return Some(ShellCommand {
            program: p.display().to_string(),
            args: vec!["--login".into(), "-i".into()],
            env,
            cwd: None,
        });
    }
    let in_path = find_in_path(&exe).filter(|p| !is_wsl_launcher(p))?;
    Some(ShellCommand {
        program: in_path.display().to_string(),
        args: vec!["--login".into(), "-i".into()],
        env: Vec::new(),
        cwd: None,
    })
}

fn simple(program: impl Into<String>) -> ShellCommand {
    ShellCommand {
        program: program.into(),
        args: Vec::new(),
        env: Vec::new(),
        cwd: None,
    }
}

/// A Windows program by its full path in `%SystemRoot%\System32`
/// (`C:\Windows` when unset): never looked up in the current folder or
/// `PATH`, where a file with the same name could stand in for it.
pub fn system32(name: &str) -> std::path::PathBuf {
    let root = std::env::var_os("SystemRoot")
        .filter(|r| !r.is_empty())
        .map_or_else(
            || std::path::PathBuf::from(r"C:\Windows"),
            std::path::PathBuf::from,
        );
    root.join("System32").join(name)
}

/// Find `kind` on this machine; `None` if it is not installed.
pub fn locate(kind: ShellKind) -> Option<ShellCommand> {
    if cfg!(windows) {
        return match kind {
            ShellKind::Bash => windows_unix_shell("bash"),
            ShellKind::Zsh => windows_unix_shell("zsh"),
            ShellKind::Cmd => Some(simple(system32("cmd.exe").display().to_string())),
            // PowerShell 7 when installed, else Windows PowerShell.
            ShellKind::PowerShell => Some(simple(
                find_in_path("pwsh.exe")
                    .unwrap_or_else(|| system32(r"WindowsPowerShell\v1.0\powershell.exe"))
                    .display()
                    .to_string(),
            )),
            ShellKind::Wsl => wsl_exe().map(|p| simple(p.display().to_string())),
        };
    }
    let login = |p: PathBuf| ShellCommand {
        program: p.display().to_string(),
        args: vec!["-l".into()],
        env: Vec::new(),
        cwd: None,
    };
    match kind {
        ShellKind::Bash => existing([PathBuf::from("/bin/bash"), PathBuf::from("/usr/bin/bash")])
            .or_else(|| find_in_path("bash"))
            .map(login),
        ShellKind::Zsh => existing([PathBuf::from("/bin/zsh"), PathBuf::from("/usr/bin/zsh")])
            .or_else(|| find_in_path("zsh"))
            .map(login),
        ShellKind::PowerShell => find_in_path("pwsh").map(|p| simple(p.display().to_string())),
        ShellKind::Cmd | ShellKind::Wsl => None,
    }
}

/// `wsl.exe`: in System32 when WSL is installed, else on `PATH`.
fn wsl_exe() -> Option<PathBuf> {
    Some(system32("wsl.exe"))
        .filter(|p| p.is_file())
        .or_else(|| find_in_path("wsl.exe"))
}

/// `wsl.exe` for a WSL session: distribution ("Default" = the default
/// one), user and start folder.
pub fn wsl_command(
    distro: &str,
    user: Option<&str>,
    start_dir: Option<&str>,
) -> Option<ShellCommand> {
    let program = wsl_exe()?.display().to_string();
    let mut args = Vec::new();
    let distro = distro.trim();
    if !distro.is_empty() && !distro.eq_ignore_ascii_case("default") {
        args.extend(["-d".to_string(), distro.to_string()]);
    }
    if let Some(u) = user.map(str::trim).filter(|u| !u.is_empty()) {
        args.extend(["-u".to_string(), u.to_string()]);
    }
    if let Some(d) = start_dir.map(str::trim).filter(|d| !d.is_empty()) {
        args.extend(["--cd".to_string(), d.to_string()]);
    }
    Some(ShellCommand {
        program,
        args,
        env: Vec::new(),
        cwd: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn locates_platform_shells() {
        if cfg!(windows) {
            // By full path, never looked up in the current folder.
            let cmd = locate(ShellKind::Cmd).unwrap().program;
            assert!(
                cmd.to_ascii_lowercase().ends_with(r"\system32\cmd.exe"),
                "{cmd}"
            );
            assert!(Path::new(&cmd).is_absolute());
            assert!(locate(ShellKind::PowerShell).is_some());
            if let Some(bash) = locate(ShellKind::Bash) {
                assert!(!is_wsl_launcher(Path::new(&bash.program)), "{bash:?}");
                assert!(bash.args.contains(&"--login".to_string()));
            }
        } else {
            assert!(locate(ShellKind::Cmd).is_none());
            assert!(locate(ShellKind::Bash).is_some(), "CI images ship bash");
        }
    }

    #[test]
    fn wsl_launcher_detection() {
        assert!(is_wsl_launcher(Path::new(r"C:\Windows\System32\bash.exe")));
        assert!(!is_wsl_launcher(Path::new(
            r"C:\Program Files\Git\bin\bash.exe"
        )));
    }
}
