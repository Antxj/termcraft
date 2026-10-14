//! "Create a desktop shortcut" to a session: a launcher that starts the
//! GUI with `--bookmark NAME` (plus the "Shortcut options").
//!
//! - Windows: a `.lnk` shell link.
//! - Linux: a `.desktop` entry.
//! - macOS: an executable `.command` script.
//!
//! The session name is never interpreted: each format gets its own
//! quoting, so a name cannot add arguments or commands.

use std::path::{Path, PathBuf};

/// "Shortcut options".
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ShortcutOptions {
    /// "Hide terminal on startup": start minimized (`--hideterm`).
    pub hide_terminal: bool,
    /// "Close … on exit": quit when the session ends (`--exitwhendone`).
    pub close_on_exit: bool,
}

/// GUI arguments of a shortcut.
pub fn arguments(session: &str, opts: ShortcutOptions) -> Vec<String> {
    let mut args = vec!["--bookmark".to_string(), session.to_string()];
    if opts.hide_terminal {
        args.push("--hideterm".into());
    }
    if opts.close_on_exit {
        args.push("--exitwhendone".into());
    }
    args
}

/// The user's desktop folder.
pub fn desktop_dir() -> Option<PathBuf> {
    directories::UserDirs::new().and_then(|d| d.desktop_dir().map(Path::to_path_buf))
}

/// The GUI executable: this program if it is the GUI, else `termcraft`
/// next to it (for `termcraft-cli`).
pub fn gui_exe() -> Option<PathBuf> {
    let me = std::env::current_exe().ok()?;
    let gui = format!("termcraft{}", std::env::consts::EXE_SUFFIX);
    if me.file_name().is_some_and(|n| n == gui.as_str()) {
        return Some(me);
    }
    Some(me.with_file_name(gui)).filter(|p| p.is_file())
}

/// A file name for the session: no path separators, reserved or control
/// characters, at most 80 characters.
pub fn file_stem(session: &str) -> String {
    let cleaned: String = session
        .chars()
        .map(|c| {
            if c.is_control() || r#"<>:"/\|?*"#.contains(c) {
                '_'
            } else {
                c
            }
        })
        .take(80)
        .collect();
    let trimmed = cleaned.trim().trim_matches('.');
    if trimmed.is_empty() {
        "TermCraft session".into()
    } else {
        trimmed.to_string()
    }
}

fn extension() -> &'static str {
    if cfg!(windows) {
        "lnk"
    } else if cfg!(target_os = "macos") {
        "command"
    } else {
        "desktop"
    }
}

/// A path in `dir` that does not exist yet (`name.ext`, `name (2).ext` …).
fn free_path(dir: &Path, stem: &str, ext: &str) -> PathBuf {
    let mut path = dir.join(format!("{stem}.{ext}"));
    let mut n = 2;
    while path.exists() && n < 1000 {
        path = dir.join(format!("{stem} ({n}).{ext}"));
        n += 1;
    }
    path
}

/// Create the shortcut in `dir`; returns its path.
pub fn create(
    dir: &Path,
    exe: &Path,
    session: &str,
    opts: ShortcutOptions,
) -> std::io::Result<PathBuf> {
    let path = free_path(dir, &file_stem(session), extension());
    let args = arguments(session, opts);
    write(&path, exe, session, &args)?;
    Ok(path)
}

#[cfg(windows)]
fn write(path: &Path, exe: &Path, session: &str, args: &[String]) -> std::io::Result<()> {
    let to_io = |e: mslnk::MSLinkError| std::io::Error::other(e.to_string());
    let mut link = mslnk::ShellLink::new(exe).map_err(to_io)?;
    link.set_arguments(Some(
        args.iter()
            .map(|a| windows_quote(a))
            .collect::<Vec<_>>()
            .join(" "),
    ));
    link.set_name(Some(format!("TermCraft: {session}")));
    if let Some(dir) = exe.parent() {
        link.set_working_dir(Some(dir.display().to_string()));
    }
    link.set_icon_location(Some(exe.display().to_string()));
    link.create_lnk(path).map_err(to_io)
}

#[cfg(target_os = "macos")]
fn write(path: &Path, exe: &Path, _session: &str, args: &[String]) -> std::io::Result<()> {
    let mut script = String::from("#!/bin/sh\nexec ");
    script.push_str(&shell_quote(&exe.display().to_string()));
    for a in args {
        script.push(' ');
        script.push_str(&shell_quote(a));
    }
    script.push_str(" >/dev/null 2>&1 &\n");
    std::fs::write(path, script)?;
    make_executable(path)
}

#[cfg(all(unix, not(target_os = "macos")))]
fn write(path: &Path, exe: &Path, session: &str, args: &[String]) -> std::io::Result<()> {
    std::fs::write(path, desktop_entry(exe, session, args))?;
    make_executable(path)
}

#[cfg(unix)]
fn make_executable(path: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755))
}

/// One argument for a Windows command line (`CommandLineToArgvW` rules).
pub fn windows_quote(arg: &str) -> String {
    if !arg.is_empty() && !arg.contains([' ', '\t', '"']) {
        return arg.to_string();
    }
    let mut out = String::from('"');
    let mut backslashes = 0;
    for c in arg.chars() {
        match c {
            '\\' => backslashes += 1,
            '"' => {
                out.extend(std::iter::repeat_n('\\', backslashes * 2 + 1));
                out.push('"');
                backslashes = 0;
            }
            c => {
                out.extend(std::iter::repeat_n('\\', backslashes));
                out.push(c);
                backslashes = 0;
            }
        }
    }
    out.extend(std::iter::repeat_n('\\', backslashes * 2));
    out.push('"');
    out
}

/// One argument for `sh`.
pub fn shell_quote(arg: &str) -> String {
    format!("'{}'", arg.replace('\'', r"'\''"))
}

/// A `.desktop` entry (freedesktop.org): `Exec` arguments are quoted and
/// `%` is doubled so no field code expands.
pub fn desktop_entry(exe: &Path, session: &str, args: &[String]) -> String {
    let quote = |a: &str| {
        let mut q = String::from('"');
        for c in a.chars() {
            match c {
                '"' | '`' | '$' | '\\' => {
                    q.push('\\');
                    q.push(c);
                }
                '%' => q.push_str("%%"),
                c if c.is_control() => {}
                c => q.push(c),
            }
        }
        q.push('"');
        q
    };
    let exec: Vec<String> = std::iter::once(exe.display().to_string())
        .chain(args.iter().cloned())
        .map(|a| quote(&a))
        .collect();
    // Values of Name= end at the line: no control characters there.
    let name: String = session.chars().filter(|c| !c.is_control()).collect();
    format!(
        "[Desktop Entry]\nType=Application\nName=TermCraft: {name}\nExec={}\nIcon=utilities-terminal\nTerminal=false\n",
        exec.join(" ")
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn arguments_and_names() {
        let opts = ShortcutOptions {
            hide_terminal: true,
            close_on_exit: true,
        };
        assert_eq!(
            arguments("web 01", opts),
            ["--bookmark", "web 01", "--hideterm", "--exitwhendone"]
        );
        assert_eq!(
            arguments("x", ShortcutOptions::default()),
            ["--bookmark", "x"]
        );
        assert_eq!(file_stem("a/b:c*?"), "a_b_c__");
        assert_eq!(file_stem(" .. "), "TermCraft session");
        assert_eq!(file_stem(&"x".repeat(200)).len(), 80);
    }

    #[test]
    fn quoting() {
        assert_eq!(windows_quote("web01"), "web01");
        assert_eq!(windows_quote("web 01"), "\"web 01\"");
        assert_eq!(windows_quote(r#"a"b"#), r#""a\"b""#);
        assert_eq!(windows_quote(r"C:\dir\ x\"), r#""C:\dir\ x\\""#);
        assert_eq!(windows_quote(""), "\"\"");
        assert_eq!(shell_quote("it's; rm"), r"'it'\''s; rm'");
        let entry = desktop_entry(
            Path::new("/opt/tc/termcraft"),
            "a\nb",
            &["--bookmark".into(), "$(x) 100%\"".into()],
        );
        assert!(
            entry.contains(r#"Exec="/opt/tc/termcraft" "--bookmark" "\$(x) 100%%\"""#),
            "{entry}"
        );
        assert!(entry.contains("Name=TermCraft: ab\n"));
    }

    #[test]
    fn creates_a_file_without_overwriting() {
        let dir = tempfile::tempdir().unwrap();
        let exe = std::env::current_exe().unwrap();
        let a = create(dir.path(), &exe, "web01", ShortcutOptions::default()).unwrap();
        let b = create(dir.path(), &exe, "web01", ShortcutOptions::default()).unwrap();
        assert!(a.is_file() && b.is_file() && a != b);
        assert!(
            b.file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with("web01 (2)")
        );
    }
}
