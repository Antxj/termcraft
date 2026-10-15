//! General › "Windows right-click menu entries": "Open TermCraft here" on
//! folders and on the background of folder windows in Explorer, for the
//! current user only (HKCU). It runs `termcraft --folder "<folder>"`.

/// The registry keys of the entry (folder, and folder background).
const KEYS: [&str; 2] = [
    r"HKCU\Software\Classes\Directory\shell\TermCraft",
    r"HKCU\Software\Classes\Directory\Background\shell\TermCraft",
];

/// What Explorer passes for each key: the folder, or the open folder.
const ARGS: [&str; 2] = ["%1", "%V"];

fn reg(args: &[&str]) -> Result<bool, String> {
    Ok(
        std::process::Command::new(crate::paths::system32("reg.exe"))
            .args(args)
            .output()
            .map_err(|e| e.to_string())?
            .status
            .success(),
    )
}

/// The command line Explorer runs for `arg` (`%1` / `%V`).
pub fn command_line(exe: &std::path::Path, arg: &str) -> Result<String, String> {
    let exe = exe.display().to_string();
    if exe.contains('"') {
        return Err("unexpected program path".into());
    }
    Ok(format!("\"{exe}\" --folder \"{arg}\""))
}

/// Add "Open TermCraft here" (Windows only).
pub fn register(exe: &std::path::Path) -> Result<String, String> {
    if !cfg!(windows) {
        return Err("Explorer menu entries are a Windows feature".into());
    }
    for (key, arg) in KEYS.iter().zip(ARGS) {
        let command = command_line(exe, arg)?;
        let icon = format!("\"{}\"", exe.display());
        let command_key = format!(r"{key}\command");
        let steps: [&[&str]; 3] = [
            &["add", key, "/ve", "/d", "Open TermCraft here", "/f"],
            &["add", key, "/v", "Icon", "/d", &icon, "/f"],
            &["add", &command_key, "/ve", "/d", &command, "/f"],
        ];
        for args in steps {
            if !reg(args)? {
                return Err("reg.exe refused the change".into());
            }
        }
    }
    Ok("\"Open TermCraft here\" added to the Explorer folder menus".into())
}

/// Remove the entries added by [`register`].
pub fn unregister() -> Result<String, String> {
    if !cfg!(windows) {
        return Err("Explorer menu entries are a Windows feature".into());
    }
    for key in KEYS {
        // Already gone is fine.
        let _ = reg(&["delete", key, "/f"])?;
    }
    Ok("\"Open TermCraft here\" removed from the Explorer menus".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn command_lines() {
        let exe = std::path::Path::new(r"C:\Tools\TermCraft\termcraft.exe");
        assert_eq!(
            command_line(exe, "%V").unwrap(),
            r#""C:\Tools\TermCraft\termcraft.exe" --folder "%V""#
        );
        assert!(command_line(std::path::Path::new("a\"b"), "%1").is_err());
    }
}
