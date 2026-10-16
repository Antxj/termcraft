//! Sessions that open in another program: RDP, VNC and XDMCP (an X
//! server: VcXsrv / Xming on Windows, Xephyr elsewhere) in the system's own
//! client (an embedded IronRDP / VNC view is tracked in docs/PARITY.md),
//! "File" and "Browser" sessions with the default application.
//!
//! Values from sessions are checked before they reach a command line or
//! a `.rdp` file: no control characters (no injected `.rdp` lines) and no
//! leading `-` (no injected options).

use std::path::{Path, PathBuf};
use std::process::Command;

/// A host name or user that is safe to pass to another program.
fn check_value(what: &str, value: &str) -> Result<(), String> {
    if value.chars().any(char::is_control) {
        return Err(format!("{what} contains control characters"));
    }
    if value.starts_with('-') {
        return Err(format!("{what} must not start with '-'"));
    }
    if what == "host" && (value.is_empty() || value.contains(char::is_whitespace)) {
        return Err("invalid host name".into());
    }
    Ok(())
}

/// The advanced settings of an RDP session that the system clients take.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RdpOptions {
    /// 3_5 "Connect to admin console".
    pub admin: bool,
    /// 3_13 "Remote command": run instead of the desktop.
    pub command: Option<String>,
    /// 3_31 "Authentication level" (server authentication): 0 none,
    /// 1 force, 2 prompt.
    pub auth_level: u8,
    /// 3_22 "Forward keyboard shortcuts" (Alt+Tab ... to the remote side).
    pub keyboard_shortcuts: bool,
    /// 3_20, 3_7, 3_8, 3_6, 3_26, 3_30, 3_33: redirected devices.
    pub clipboard: bool,
    pub drives: bool,
    pub printers: bool,
    pub ports: bool,
    pub microphone: bool,
    pub smartcards: bool,
    pub webauthn: bool,
    /// 3_17 "Redirect audio": 0 no audio, 1 here, 2 on the remote server.
    pub audio: u8,
    /// 3_11 "Display resolution".
    pub size: RdpSize,
    /// 3_27 "Autoscale": scale the remote desktop to the window.
    pub autoscale: bool,
    /// 3_10 "Enhanced graphics" (built-in client: desktop composition).
    pub enhanced_graphics: bool,
    /// Configuration › Security "Disable RDP \"reconnect\"".
    pub no_reconnect: bool,
}

/// 3_11 "Display resolution".
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RdpSize {
    /// "Fit to terminal": a window (the client's own size).
    Window,
    /// "Fit to screen": full screen.
    FullScreen,
    Fixed(u32, u32),
}

impl Default for RdpOptions {
    /// The schema defaults.
    fn default() -> Self {
        Self::from_options(&|_| None)
    }
}

impl RdpOptions {
    /// What only the system client does (device redirection, audio,
    /// admin console, remote command): the built-in client is not used.
    pub fn needs_system_client(&self) -> Option<&'static str> {
        let reasons = [
            (self.admin, "admin console"),
            (self.command.is_some(), "remote command"),
            (self.drives, "drive redirection"),
            (self.printers, "printer redirection"),
            (self.ports, "port redirection"),
            (self.microphone, "microphone"),
            (self.smartcards, "smart cards"),
            (self.webauthn, "WebAuthn"),
            (self.audio != 0, "audio"),
        ];
        reasons.iter().find(|(on, _)| *on).map(|(_, what)| *what)
    }

    /// From the session options (`get` = the stored value of an id; the
    /// schema default when `None`).
    pub fn from_options(get: &dyn Fn(&str) -> Option<String>) -> Self {
        let params = tc_core::schema::type_params(4);
        let value = |id: &str| {
            get(id).unwrap_or_else(|| {
                params
                    .iter()
                    .find(|p| p.id == id)
                    .map(|p| p.default.to_string())
                    .unwrap_or_default()
            })
        };
        let flag = |id: &str| tc_core::schema::is_true(value(id).trim());
        let index = |id: &str| value(id).trim().parse::<usize>().unwrap_or(0);
        let size = match index("3_11") {
            0 => RdpSize::Window,
            1 => RdpSize::FullScreen,
            i => params
                .iter()
                .find(|p| p.id == "3_11")
                .and_then(|p| match p.widget {
                    tc_core::schema::Widget::Choice(items) => items.get(i).copied(),
                    _ => None,
                })
                .and_then(|label| label.split_once('x'))
                .and_then(|(w, h)| Some(RdpSize::Fixed(w.parse().ok()?, h.parse().ok()?)))
                .unwrap_or(RdpSize::Window),
        };
        Self {
            admin: flag("3_5"),
            command: Some(value("3_13"))
                .map(|c| c.trim().to_string())
                .filter(|c| !c.is_empty()),
            auth_level: u8::try_from(index("3_31").min(2)).unwrap_or(0),
            keyboard_shortcuts: flag("3_22"),
            clipboard: flag("3_20"),
            drives: flag("3_7"),
            printers: flag("3_8"),
            ports: flag("3_6"),
            microphone: flag("3_26"),
            smartcards: flag("3_30"),
            webauthn: flag("3_33"),
            audio: u8::try_from(index("3_17").min(2)).unwrap_or(0),
            size,
            autoscale: flag("3_27"),
            enhanced_graphics: flag("3_10"),
            no_reconnect: false,
        }
    }
}

/// Contents of the `.rdp` file for mstsc.
pub fn rdp_file(host: &str, port: u16, user: Option<&str>, o: &RdpOptions) -> String {
    let mut s = format!("full address:s:{host}:{port}\r\nprompt for credentials:i:1\r\n");
    let mut line = |l: String| {
        s.push_str(&l);
        s.push_str("\r\n");
    };
    if let Some(u) = user.filter(|u| !u.is_empty()) {
        line(format!("username:s:{u}"));
    }
    let i = |b: bool| u8::from(b);
    line(format!("administrative session:i:{}", i(o.admin)));
    if let Some(c) = &o.command {
        line(format!("alternate shell:s:{c}"));
    }
    line(format!("authentication level:i:{}", o.auth_level));
    // 1 = on the remote computer, 2 = only full screen.
    line(format!(
        "keyboardhook:i:{}",
        if o.keyboard_shortcuts { 1 } else { 2 }
    ));
    line(format!("redirectclipboard:i:{}", i(o.clipboard)));
    line(format!(
        "drivestoredirect:s:{}",
        if o.drives { "*" } else { "" }
    ));
    line(format!("redirectprinters:i:{}", i(o.printers)));
    line(format!("redirectcomports:i:{}", i(o.ports)));
    line(format!("audiocapturemode:i:{}", i(o.microphone)));
    line(format!("redirectsmartcards:i:{}", i(o.smartcards)));
    line(format!("redirectwebauthn:i:{}", i(o.webauthn)));
    // audiomode: 0 here, 1 on the remote computer, 2 none.
    let audio = match o.audio {
        1 => 0,
        2 => 1,
        _ => 2,
    };
    line(format!("audiomode:i:{audio}"));
    match o.size {
        RdpSize::Window => line("screen mode id:i:1".into()),
        RdpSize::FullScreen => line("screen mode id:i:2".into()),
        RdpSize::Fixed(w, h) => {
            line("screen mode id:i:1".into());
            line(format!("desktopwidth:i:{w}"));
            line(format!("desktopheight:i:{h}"));
        }
    }
    line(format!("smart sizing:i:{}", i(o.autoscale)));
    if o.no_reconnect {
        line("autoreconnection enabled:i:0".into());
    }
    s
}

/// FreeRDP arguments for the same settings.
fn freerdp_args(host: &str, port: u16, user: Option<&str>, o: &RdpOptions) -> Vec<String> {
    let mut args = vec![format!("/v:{host}:{port}")];
    if let Some(u) = user.filter(|u| !u.is_empty()) {
        args.push(format!("/u:{u}"));
    }
    let mut push = |on: bool, arg: &str| {
        if on {
            args.push(arg.to_string());
        }
    };
    push(o.admin, "/admin");
    push(o.clipboard, "+clipboard");
    push(o.drives, "/drive:home,~");
    push(o.printers, "/printer");
    push(o.smartcards, "/smartcard");
    push(o.microphone, "/microphone");
    push(o.audio == 1, "/sound");
    push(o.audio == 2, "/audio-mode:1");
    push(o.autoscale, "/smart-sizing");
    match o.size {
        RdpSize::Window => {}
        RdpSize::FullScreen => args.push("/f".into()),
        RdpSize::Fixed(w, h) => args.push(format!("/size:{w}x{h}")),
    }
    if let Some(c) = &o.command {
        args.push(format!("/shell:{c}"));
    }
    args
}

fn find_in_path(exe: &str) -> Option<PathBuf> {
    std::env::split_paths(&std::env::var_os("PATH")?)
        .map(|dir| dir.join(exe))
        .find(|p| p.is_file())
}

fn spawn(program: &Path, args: &[String]) -> Result<(), String> {
    Command::new(program)
        .args(args)
        .spawn()
        .map(drop)
        .map_err(|e| format!("{}: {e}", program.display()))
}

/// Start the platform RDP client; returns a description of what ran.
pub fn launch_rdp(
    host: &str,
    port: u16,
    user: Option<&str>,
    options: &RdpOptions,
    temp_dir: &Path,
) -> Result<String, String> {
    check_value("host", host)?;
    if let Some(u) = user {
        check_value("user name", u)?;
    }
    if let Some(c) = &options.command {
        check_value("remote command", c)?;
    }
    if cfg!(windows) {
        std::fs::create_dir_all(temp_dir).map_err(|e| e.to_string())?;
        let file = temp_dir.join(format!("{host}-{port}.rdp").replace([':', '/', '\\'], "_"));
        std::fs::write(&file, rdp_file(host, port, user, options)).map_err(|e| e.to_string())?;
        let mstsc = tc_core::paths::system32("mstsc.exe");
        if !mstsc.is_file() {
            return Err(format!(
                "the Windows Remote Desktop client is not installed ({} is missing)",
                mstsc.display()
            ));
        }
        spawn(&mstsc, &[file.display().to_string()])?;
        return Ok("Windows Remote Desktop (mstsc)".into());
    }
    if cfg!(target_os = "macos") {
        let mut url = format!("rdp://full%20address=s:{host}:{port}");
        if let Some(u) = user.filter(|u| !u.is_empty()) {
            url.push_str(&format!("&username=s:{u}"));
        }
        spawn(Path::new("open"), &[url])?;
        return Ok("the default RDP app".into());
    }
    if let Some(p) = find_in_path("xfreerdp3").or_else(|| find_in_path("xfreerdp")) {
        spawn(&p, &freerdp_args(host, port, user, options))?;
        return Ok("FreeRDP".into());
    }
    if let Some(p) = find_in_path("remmina") {
        spawn(&p, &["-c".into(), format!("rdp://{host}:{port}")])?;
        return Ok("Remmina".into());
    }
    Err("no RDP client found (install FreeRDP or Remmina)".into())
}

/// Tools › "Command Prompt (admin)" / "Windows Powershell (admin)": Windows
/// asks for elevation (UAC) and opens it in its own console.
pub fn run_elevated(powershell: bool) -> Result<(), String> {
    if !cfg!(windows) {
        return Err("elevated prompts are a Windows feature".into());
    }
    let ps = tc_core::paths::system32(r"WindowsPowerShell\v1.0\powershell.exe");
    let target = if powershell {
        ps.clone()
    } else {
        tc_core::paths::system32("cmd.exe")
    };
    // A path from %SystemRoot%, quoted for PowerShell.
    let target = target.display().to_string().replace('\'', "''");
    spawn(
        &ps,
        &[
            "-NoProfile".into(),
            "-WindowStyle".into(),
            "Hidden".into(),
            "-Command".into(),
            format!("Start-Process -Verb RunAs -FilePath '{target}'"),
        ],
    )
}

/// Start the platform VNC viewer; returns a description of what ran.
pub fn launch_vnc(host: &str, port: u16) -> Result<String, String> {
    check_value("host", host)?;
    if cfg!(target_os = "macos") {
        spawn(Path::new("open"), &[format!("vnc://{host}:{port}")])?;
        return Ok("Screen Sharing".into());
    }
    let candidates: &[&str] = if cfg!(windows) {
        &["vncviewer.exe", "tvnviewer.exe", "uvnc_viewer.exe"]
    } else {
        &["vncviewer", "xtigervncviewer", "remmina"]
    };
    for exe in candidates {
        if let Some(p) = find_in_path(exe) {
            let args = if exe.starts_with("remmina") {
                vec!["-c".into(), format!("vnc://{host}:{port}")]
            } else {
                vec![format!("{host}::{port}")]
            };
            spawn(&p, &args)?;
            return Ok((*exe).to_string());
        }
    }
    Err("no VNC viewer found in PATH (e.g. TigerVNC or TightVNC)".into())
}

/// "Open with...": run `program` with the file as its only argument (no
/// shell). macOS application bundles open through `open -a`.
pub fn open_with(program: &Path, file: &Path) -> Result<String, String> {
    let name = program.file_name().map_or_else(
        || program.display().to_string(),
        |n| n.to_string_lossy().into_owned(),
    );
    if cfg!(target_os = "macos") && program.extension().is_some_and(|e| e == "app") {
        spawn(
            Path::new("open"),
            &[
                "-a".into(),
                program.display().to_string(),
                file.display().to_string(),
            ],
        )?;
    } else {
        Command::new(program)
            .arg(file)
            .spawn()
            .map_err(|e| format!("{}: {e}", program.display()))?;
    }
    Ok(name)
}

/// macOS without `$DISPLAY`: start XQuartz (when installed) so X11
/// forwarding has a server on `:0`. Returns whether it was started.
pub fn start_xquartz() -> bool {
    if !cfg!(target_os = "macos") {
        return false;
    }
    let installed = [
        "/Applications/Utilities/XQuartz.app",
        "/Applications/XQuartz.app",
    ]
    .iter()
    .any(|p| Path::new(p).exists());
    installed && spawn(Path::new("open"), &["-a".into(), "XQuartz".into()]).is_ok()
}

/// An XDMCP session for an X server.
#[derive(Debug, Clone, Copy)]
pub struct Xdmcp<'a> {
    /// `None` = broadcast ("Connect to any server").
    pub host: Option<&'a str>,
    /// "Sender IP" (the address the display manager answers to).
    pub from: Option<&'a str>,
    /// "Start XDMCP on screen number".
    pub display: u16,
    pub size: (u32, u32),
    /// "Enable font server": host and port.
    pub font_server: Option<(&'a str, u16)>,
    /// "Keyboard type": an XKB layout (`None` = automatic).
    pub keyboard: Option<&'a str>,
    /// "Clipboard": 0 disabled, 1 enabled, 2 enabled without the primary
    /// selection (VcXsrv / Xming only).
    pub clipboard: u8,
}

/// X server arguments; `nested` = Xephyr's `-screen WxH` (VcXsrv / Xming
/// take `-screen 0 WxH`).
pub fn xdmcp_args(x: &Xdmcp<'_>, nested: bool) -> Result<Vec<String>, String> {
    let mut args = vec![format!(":{}", x.display)];
    match x.host {
        Some(h) => {
            check_value("host", h)?;
            args.extend(["-query".into(), h.to_string()]);
        }
        None => args.push("-broadcast".into()),
    }
    if let Some(from) = x.from.filter(|f| !f.trim().is_empty()) {
        let from = from.trim();
        from.parse::<std::net::IpAddr>()
            .map_err(|_| format!("invalid sender IP {from:?}"))?;
        args.extend(["-from".into(), from.to_string()]);
    }
    let (w, h) = (x.size.0.clamp(320, 16_384), x.size.1.clamp(200, 16_384));
    if nested {
        args.extend(["-screen".into(), format!("{w}x{h}")]);
    } else {
        args.extend(["-screen".into(), "0".into(), format!("{w}x{h}")]);
    }
    if let Some((host, port)) = x.font_server {
        let host = host.trim();
        check_value("host", host)?;
        args.extend(["-fp".into(), format!("tcp/{host}:{port}")]);
    }
    // VcXsrv / Xming take a layout; Xephyr follows the host's keyboard.
    if let Some(layout) = x.keyboard.filter(|_| !nested) {
        if layout.is_empty() || !layout.chars().all(|c| c.is_ascii_alphanumeric()) {
            return Err(format!("invalid keyboard layout {layout:?}"));
        }
        args.extend(["-xkblayout".into(), layout.to_string()]);
    }
    if !nested {
        match x.clipboard {
            0 => args.push("-noclipboard".into()),
            2 => args.extend(["-clipboard".into(), "-noprimary".into()]),
            _ => args.push("-clipboard".into()),
        }
    }
    Ok(args)
}

/// VcXsrv or Xming, where their installers put them.
fn windows_x_server() -> Option<PathBuf> {
    let pf = |var: &str, rel: &str| std::env::var_os(var).map(|p| PathBuf::from(p).join(rel));
    [
        pf("ProgramFiles", r"VcXsrv\vcxsrv.exe"),
        pf("ProgramFiles(x86)", r"Xming\Xming.exe"),
        find_in_path("vcxsrv.exe"),
    ]
    .into_iter()
    .flatten()
    .find(|p| p.is_file())
}

/// VcXsrv / Xming arguments for the Configuration › X11 settings.
pub fn x_server_args(
    x: &tc_core::settings::XServerSettings,
    vcxsrv: bool,
) -> Result<Vec<String>, String> {
    use tc_core::settings::XDisplayMode;
    let mut args = vec![format!(":{}", x.display.min(99))];
    match x.display_mode {
        XDisplayMode::Multiwindow => args.push("-multiwindow".into()),
        XDisplayMode::Windowed => {}
        XDisplayMode::Fullscreen => args.push("-fullscreen".into()),
    }
    match x.clipboard {
        0 => args.push("-noclipboard".into()),
        2 => args.extend(["-clipboard".into(), "-noprimary".into()]),
        _ => args.push("-clipboard".into()),
    }
    if vcxsrv {
        args.push(if x.opengl_hardware { "-wgl" } else { "-nowgl" }.into());
    }
    if x.remote_access {
        args.push("-ac".into());
    }
    if x.dpi > 0 {
        args.extend(["-dpi".into(), x.dpi.clamp(48, 480).to_string()]);
    }
    for ext in &x.extensions {
        match ext.as_str() {
            "Xinerama" => args.push("+xinerama".into()),
            "Composite" | "Damage" | "Randr" | "Xfixes" | "Xtest" => {
                args.extend(["+extension".into(), ext.to_uppercase()]);
            }
            _ => {}
        }
    }
    let layout = x.keyboard.trim();
    if !layout.is_empty() {
        if !layout.chars().all(|c| c.is_ascii_alphanumeric()) {
            return Err(format!("invalid keyboard layout {layout:?}"));
        }
        args.extend(["-xkblayout".into(), layout.to_string()]);
    }
    Ok(args)
}

/// Start the local X server. Returns the process (when TermCraft owns
/// it) and what runs.
pub fn start_x_server(
    x: &tc_core::settings::XServerSettings,
) -> Result<(Option<std::process::Child>, String), String> {
    if cfg!(windows) {
        let Some(server) = windows_x_server() else {
            return Err("no X server found (install VcXsrv or Xming)".into());
        };
        let name = server
            .file_name()
            .map_or_else(|| "X server".into(), |n| n.to_string_lossy().into_owned());
        let vcxsrv = name.eq_ignore_ascii_case("vcxsrv.exe");
        let child = Command::new(&server)
            .args(x_server_args(x, vcxsrv)?)
            .spawn()
            .map_err(|e| format!("{}: {e}", server.display()))?;
        return Ok((Some(child), name));
    }
    if cfg!(target_os = "macos") {
        return if start_xquartz() {
            Ok((None, "XQuartz".into()))
        } else {
            Err("XQuartz is not installed".into())
        };
    }
    match std::env::var("DISPLAY") {
        Ok(d) if !d.trim().is_empty() => Ok((None, format!("the desktop's X server ({d})"))),
        _ => Err("no X server: DISPLAY is not set".into()),
    }
}

/// Start an X server that runs the XDMCP session.
pub fn launch_xdmcp(x: &Xdmcp<'_>) -> Result<String, String> {
    if cfg!(windows) {
        let Some(server) = windows_x_server() else {
            return Err("no X server found (install VcXsrv or Xming)".into());
        };
        spawn(&server, &xdmcp_args(x, false)?)?;
        return Ok(server
            .file_name()
            .map_or_else(|| "X server".into(), |n| n.to_string_lossy().into_owned()));
    }
    let candidates = [PathBuf::from("/opt/X11/bin/Xephyr")];
    let found = candidates
        .into_iter()
        .find(|p| p.is_file())
        .or_else(|| find_in_path("Xephyr"));
    let Some(server) = found else {
        return Err("Xephyr was not found (install it, e.g. the xserver-xephyr package)".into());
    };
    spawn(&server, &xdmcp_args(x, true)?)?;
    Ok("Xephyr".into())
}

/// The system "open" command: Explorer, `open` or `xdg-open`. They take the
/// target as one argument, without a shell, so nothing in it is executed.
fn opener() -> PathBuf {
    if cfg!(windows) {
        // explorer.exe lives in %SystemRoot% itself.
        tc_core::paths::windows_dir().join("explorer.exe")
    } else if cfg!(target_os = "macos") {
        "open".into()
    } else {
        "xdg-open".into()
    }
}

/// "File" session: open a local file or folder with its default application.
pub fn open_path(path: &str) -> Result<String, String> {
    let path = path.trim();
    check_value("path", path)?;
    let p = Path::new(path);
    if !p.exists() {
        return Err(format!("{path} does not exist"));
    }
    // An absolute path, so a file named like an option cannot be one.
    let full = std::fs::canonicalize(p).map_err(|e| e.to_string())?;
    let full = full.display().to_string();
    // Windows `canonicalize` adds a `\\?\` prefix Explorer does not take.
    let full = full
        .strip_prefix(r"\\?\")
        .map(str::to_string)
        .unwrap_or(full);
    spawn(&opener(), &[full])?;
    Ok("the default application".into())
}

/// Open a local file in a text editor (never its default program, so a
/// downloaded script or executable is not run): Notepad on Windows,
/// TextEdit / the default text editor on macOS, a known GUI editor on Linux.
pub fn open_in_text_editor(path: &Path) -> Result<String, String> {
    let file = path.display().to_string();
    if cfg!(windows) {
        spawn(&tc_core::paths::system32("notepad.exe"), &[file])?;
        return Ok("Notepad".into());
    }
    if cfg!(target_os = "macos") {
        spawn(Path::new("open"), &["-t".into(), file])?;
        return Ok("the default text editor".into());
    }
    for exe in [
        "gnome-text-editor",
        "gedit",
        "kate",
        "kwrite",
        "mousepad",
        "xed",
        "pluma",
        "geany",
    ] {
        if let Some(p) = find_in_path(exe) {
            spawn(&p, &[file])?;
            return Ok(exe.to_string());
        }
    }
    Err("no graphical text editor found (install gedit, kate or mousepad)".into())
}

/// Only web addresses: no other URL scheme (`file:`, app protocol
/// handlers…) is handed to the system.
pub fn web_url(url: &str) -> Result<String, String> {
    let url = url.trim();
    check_value("URL", url)?;
    if url.contains(char::is_whitespace) || url.is_empty() {
        return Err("invalid URL".into());
    }
    let lower = url.to_ascii_lowercase();
    if lower.starts_with("http://") || lower.starts_with("https://") {
        Ok(url.to_string())
    } else if lower.contains("://") || lower.contains(':') && !lower.contains('.') {
        Err("only http:// and https:// addresses are opened".into())
    } else {
        Ok(format!("https://{url}"))
    }
}

/// "Browser" session: open the page in the default browser.
pub fn open_url(url: &str) -> Result<String, String> {
    let url = web_url(url)?;
    spawn(&opener(), &[url])?;
    Ok("the default browser".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn xdmcp_arguments() {
        let q = Xdmcp {
            host: Some("dm.lan"),
            from: Some("192.168.1.5"),
            display: 1,
            size: (1024, 768),
            font_server: None,
            keyboard: None,
            clipboard: 1,
        };
        assert_eq!(
            xdmcp_args(&q, false).unwrap(),
            [
                ":1",
                "-query",
                "dm.lan",
                "-from",
                "192.168.1.5",
                "-screen",
                "0",
                "1024x768",
                "-clipboard"
            ]
        );
        let any = Xdmcp {
            host: None,
            from: None,
            display: 2,
            size: (10, 10),
            ..q
        };
        assert_eq!(
            xdmcp_args(&any, true).unwrap(),
            [":2", "-broadcast", "-screen", "320x200"]
        );
        assert!(
            xdmcp_args(
                &Xdmcp {
                    host: Some("-ac"),
                    ..q
                },
                true
            )
            .is_err(),
            "no option injection"
        );
        assert!(
            xdmcp_args(
                &Xdmcp {
                    from: Some("x; rm"),
                    ..q
                },
                true
            )
            .is_err()
        );
        let more = Xdmcp {
            font_server: Some(("fonts.lan", 7100)),
            keyboard: Some("br"),
            clipboard: 2,
            ..q
        };
        let args = xdmcp_args(&more, false).unwrap();
        assert!(args.ends_with(&[
            "-fp".to_string(),
            "tcp/fonts.lan:7100".into(),
            "-xkblayout".into(),
            "br".into(),
            "-clipboard".into(),
            "-noprimary".into(),
        ]));
        let bad = Xdmcp {
            keyboard: Some("us -ac"),
            ..q
        };
        assert!(xdmcp_args(&bad, false).is_err(), "no option injection");
    }

    #[test]
    fn x_server_arguments() {
        use tc_core::settings::{XDisplayMode, XServerSettings};
        let x = XServerSettings::default();
        assert_eq!(
            x_server_args(&x, true).unwrap(),
            [":0", "-multiwindow", "-clipboard", "-wgl"]
        );
        let x = XServerSettings {
            display_mode: XDisplayMode::Fullscreen,
            clipboard: 0,
            display: 2,
            dpi: 120,
            remote_access: true,
            keyboard: "br".into(),
            ..XServerSettings::default()
        };
        assert_eq!(
            x_server_args(&x, false).unwrap(),
            [
                ":2",
                "-fullscreen",
                "-noclipboard",
                "-ac",
                "-dpi",
                "120",
                "-xkblayout",
                "br"
            ]
        );
        let ext = XServerSettings {
            extensions: vec!["Randr".into(), "Xinerama".into(), "-ac".into()],
            ..XServerSettings::default()
        };
        assert!(x_server_args(&ext, true).unwrap().ends_with(&[
            "+extension".to_string(),
            "RANDR".into(),
            "+xinerama".into()
        ]));
        let bad = XServerSettings {
            keyboard: "us -query evil".into(),
            ..XServerSettings::default()
        };
        assert!(x_server_args(&bad, true).is_err(), "no option injection");
    }

    #[test]
    fn values_are_checked() {
        assert!(check_value("host", "srv.local").is_ok());
        assert!(check_value("host", "-oProxyCommand=x").is_err());
        assert!(check_value("host", "a\r\nalternate shell:s:cmd").is_err());
        assert!(check_value("host", "two words").is_err());
        assert!(launch_vnc("-via evil", 5900).is_err());
    }

    #[test]
    fn only_web_urls() {
        assert_eq!(web_url("example.com").unwrap(), "https://example.com");
        assert_eq!(web_url("http://x.test/a?b").unwrap(), "http://x.test/a?b");
        assert!(web_url("file:///etc/passwd").is_err());
        assert!(web_url("ms-settings:privacy").is_err());
        assert!(web_url("javascript:alert(1)").is_err());
        assert!(web_url("-x").is_err());
    }

    #[test]
    fn rdp_file_has_address_and_user() {
        let o = RdpOptions::default();
        let f = rdp_file("srv.local", 3390, Some("admin"), &o);
        assert!(f.contains("full address:s:srv.local:3390\r\n"));
        assert!(f.contains("username:s:admin\r\n"));
        assert!(!rdp_file("h", 3389, None, &o).contains("username"));
    }

    #[test]
    fn rdp_session_settings() {
        let o = RdpOptions::default();
        assert!(
            o.clipboard && o.keyboard_shortcuts && o.autoscale,
            "schema defaults"
        );
        assert!(!o.drives && !o.admin);
        let f = rdp_file("h", 3389, None, &o);
        assert!(f.contains("redirectclipboard:i:1\r\n"));
        assert!(f.contains("audiomode:i:2\r\n"), "no audio");
        assert!(f.contains("screen mode id:i:1\r\n"));

        let stored: std::collections::BTreeMap<&str, &str> = [
            ("3_5", "-1"),
            ("3_7", "-1"),
            ("3_20", "0"),
            ("3_17", "1"),
            ("3_11", "4"),
            ("3_13", "notepad.exe"),
        ]
        .into_iter()
        .collect();
        let o = RdpOptions::from_options(&|id| stored.get(id).map(|v| v.to_string()));
        assert_eq!(o.size, RdpSize::Fixed(1024, 768));
        let f = rdp_file("h", 3389, None, &o);
        for line in [
            "administrative session:i:1",
            "drivestoredirect:s:*",
            "redirectclipboard:i:0",
            "audiomode:i:0",
            "desktopwidth:i:1024",
            "desktopheight:i:768",
            "alternate shell:s:notepad.exe",
        ] {
            assert!(f.contains(&format!("{line}\r\n")), "{line}");
        }
        let args = freerdp_args("h", 3389, None, &o);
        assert!(args.contains(&"/admin".to_string()) && args.contains(&"/size:1024x768".into()));
        assert!(!args.contains(&"+clipboard".to_string()));
    }
}
