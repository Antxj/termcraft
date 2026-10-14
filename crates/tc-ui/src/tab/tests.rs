use super::*;
use std::time::{Duration, Instant};

/// Pump a tab until `done` holds (answering the host key prompt), or panic.
fn pump_until(tab: &mut Tab, mut done: impl FnMut(&mut Tab) -> bool) {
    let deadline = Instant::now() + Duration::from_secs(20);
    while Instant::now() < deadline {
        tab.pump();
        if tab.host_key.is_some() {
            tab.answer_host_key(true, false);
        }
        if done(tab) {
            return;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    panic!("timeout; screen:\n{}", tab.term.contents());
}

fn dev_session(port: u16) -> Session {
    let mut s = Session::new(
        "dev",
        SessionKind::Ssh {
            host: "127.0.0.1".into(),
            port,
            user: Some(tc_devserver::DEV_USER.into()),
            private_key: None,
            x11_forwarding: false,
        },
    );
    s.options
        .insert("3_8".into(), "echo saved-password-test".into());
    s.options.insert("3_26".into(), "0".into());
    s
}

/// A tab environment in `dir`, with an in-memory vault.
fn test_env(dir: &std::path::Path) -> TabEnv {
    TabEnv {
        scrollback: 100,
        known_hosts: dir.join("kh"),
        repaint: Arc::new(|| {}),
        vault: Arc::new(Vault::open(
            dir.join("i.toml"),
            Box::new(tc_vault::MemoryStore::default()),
        )),
        save_passwords: SavePasswords::Ask,
        ssh: Default::default(),
        backspace_ctrl_h: false,
        paste_right_click: true,
        local_shell: Default::default(),
        save_key_passphrases: false,
        log_dir: None,
        use_windows_path: true,
        home: None,
    }
}

#[test]
fn term_charset_choices_match_the_schema() {
    let param = schema::terminal_params()
        .iter()
        .find(|p| p.id == "4_6")
        .expect("4_6");
    let schema::Widget::Choice(choices) = param.widget else {
        panic!("4_6 is a choice");
    };
    assert_eq!(choices, tc_term::charset::NAMES);
    assert_eq!(param.default, tc_term::charset::UTF8.to_string());

    let mut s = Session::new(
        "x",
        SessionKind::Telnet {
            host: "h".into(),
            port: 23,
        },
    );
    assert!(term_charset(Some(&s)).is_utf8());
    s.options.insert("4_6".into(), "0".into());
    assert!(!term_charset(Some(&s)).is_utf8());
}

#[test]
fn paste_delay_and_log_folder_from_session_options() {
    let mut s = Session::new(
        "x",
        SessionKind::Telnet {
            host: "h".into(),
            port: 23,
        },
    );
    assert_eq!(paste_delay(Some(&s)), Duration::ZERO);
    s.options.insert("4_40".into(), "4".into());
    assert_eq!(paste_delay(Some(&s)), Duration::from_millis(30));
    s.options.insert("4_40".into(), "13".into());
    assert_eq!(paste_delay(Some(&s)), Duration::from_millis(360));

    let global = std::path::Path::new("/logs");
    assert_eq!(log_folder(Some(&s), None), None);
    assert_eq!(log_folder(None, Some(global)), Some(PathBuf::from("/logs")));
    s.options.insert("4_12".into(), "-1".into());
    assert_eq!(
        log_folder(Some(&s), Some(global)),
        Some(PathBuf::from("/logs"))
    );
    s.options.insert("4_13".into(), "/session-logs".into());
    assert_eq!(
        log_folder(Some(&s), Some(global)),
        Some(PathBuf::from("/session-logs"))
    );
}

#[test]
fn delphi_colors() {
    assert_eq!(delphi_color("255"), Some([255, 0, 0]), "red is 0x0000FF");
    assert_eq!(delphi_color("16711680"), Some([0, 0, 255]));
    assert_eq!(delphi_color("-1"), None);
    assert_eq!(delphi_color("x"), None);
}

#[test]
fn session_stopped_keys() {
    let keys = ExpertTerminal::default();
    assert_eq!(ended_key(b"\r", &keys), Some(EndedAction::Close));
    assert_eq!(ended_key(b"R", &keys), Some(EndedAction::Restart));
    assert_eq!(ended_key(b"s", &keys), Some(EndedAction::Save));
    assert_eq!(
        ended_key(b"x", &keys),
        Some(EndedAction::Close),
        "any other key"
    );
    assert_eq!(ended_key(b"rr", &keys), None, "a paste is not a key");
    let keys = ExpertTerminal {
        exit_key: "RETURN".into(),
        reconnect_key: "F5".into(),
        ..ExpertTerminal::default()
    };
    assert_eq!(ended_key(b"x", &keys), None);
    assert_eq!(ended_key(b"\x1b[15~", &keys), Some(EndedAction::Restart));
    assert_eq!(key_label("F5"), "<F5>");
    assert_eq!(key_label("Any other key"), "any other key");
}

#[test]
fn keep_open_runs_the_login_shell_after_the_command() {
    assert_eq!(keep_open("top", false), "top");
    assert_eq!(keep_open("top", true), "top; exec \"${SHELL:-/bin/sh}\" -l");
}

#[test]
fn telnet_tab_shows_server_output() {
    use std::io::Write;
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    std::thread::spawn(move || {
        let (mut s, _) = listener.accept().unwrap();
        // IAC WILL ECHO, then a banner.
        s.write_all(&[255, 251, 1]).unwrap();
        s.write_all(b"Welcome to telnet-test\r\n").unwrap();
        std::thread::sleep(Duration::from_millis(300));
    });
    let dir = tempfile::tempdir().unwrap();
    let env = test_env(dir.path());
    let session = Session::new(
        "t",
        SessionKind::Telnet {
            host: "127.0.0.1".into(),
            port,
        },
    );
    let mut tab = Tab::open(1, Some(session), &env);
    pump_until(&mut tab, |t| {
        t.term.contents().contains("Welcome to telnet-test")
    });
    pump_until(&mut tab, |t| {
        t.term.contents().contains("connection closed")
    });
}

/// Bookmark 1_1 off: no "Session stopped" prompt, the tab closes.
#[test]
fn no_reconnection_message_closes_the_tab() {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    std::thread::spawn(move || drop(listener.accept()));
    let dir = tempfile::tempdir().unwrap();
    let env = test_env(dir.path());
    let mut session = Session::new(
        "t",
        SessionKind::Telnet {
            host: "127.0.0.1".into(),
            port,
        },
    );
    session.options.insert("1_1".into(), "0".into());
    session.options.insert("5_1".into(), "2".into());
    let mut tab = Tab::open(1, Some(session), &env);
    assert!(
        tab.detached && tab.detached_maximized,
        "5_1 maximized detached"
    );
    pump_until(&mut tab, |t| t.ended_action.is_some());
    assert_eq!(tab.ended_action, Some(EndedAction::Close));
}

/// End to end: a Telnet session whose jump host has no login asks for
/// it, opens the tunnel through the gateway, then talks Telnet over it.
#[test]
fn telnet_through_an_ssh_gateway() {
    use std::io::Write;
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let telnet_port = listener.local_addr().unwrap().port();
    std::thread::spawn(move || {
        let (mut s, _) = listener.accept().unwrap();
        s.write_all(b"Welcome through the gateway\r\n").unwrap();
        std::thread::sleep(Duration::from_millis(500));
    });
    let rt = tokio::runtime::Runtime::new().unwrap();
    let (gw, fut) = rt
        .block_on(tc_devserver::bind(0, tc_devserver::DevConfig::default()))
        .unwrap();
    rt.spawn(fut);
    let dir = tempfile::tempdir().unwrap();
    let env = TabEnv {
        save_passwords: SavePasswords::Never,
        ..test_env(dir.path())
    };
    let mut session = Session::new(
        "t",
        SessionKind::Telnet {
            host: "127.0.0.1".into(),
            port: telnet_port,
        },
    );
    // Telnet Network settings: 3_7 gateway host, 3_8 port.
    session.options.insert("3_7".into(), "127.0.0.1".into());
    session.options.insert("3_8".into(), gw.port().to_string());
    let mut tab = Tab::open(1, Some(session), &env);
    pump_until(&mut tab, |t| t.term.contents().contains("login as:"));
    tab.send(format!("{}\r", tc_devserver::DEV_USER).as_bytes());
    pump_until(&mut tab, |t| t.term.contents().contains("password:"));
    tab.send(format!("{}\r", tc_devserver::DEV_PASSWORD).as_bytes());
    pump_until(&mut tab, |t| {
        t.term.contents().contains("Welcome through the gateway")
    });
    assert!(tab.gateway_tunnel.is_some());
}

/// End to end: the login shell is bash, so the hook is typed after the
/// first prompt, the browser follows `cd`, and the hook never shows.
/// Skipped when bash is not installed.
#[test]
fn folder_tracking_is_automatic_and_hidden_for_bash() {
    let Some(mut bash) = tc_pty::shells::locate(tc_pty::ShellKind::Bash) else {
        return;
    };
    bash.args = vec!["--norc".into(), "--noprofile".into(), "-i".into()];
    bash.env.push(("PS1".into(), "tc$ ".into()));
    let root = tempfile::tempdir().unwrap();
    std::fs::create_dir(root.path().join("sub")).unwrap();
    let rt = tokio::runtime::Runtime::new().unwrap();
    let config = tc_devserver::DevConfig {
        shell: Some(bash),
        login_shell: Some("/usr/bin/bash".into()),
        sftp_root: root.path().to_path_buf(),
        ..Default::default()
    };
    let (addr, fut) = rt.block_on(tc_devserver::bind(0, config)).unwrap();
    rt.spawn(fut);
    let dir = tempfile::tempdir().unwrap();
    let env = TabEnv {
        known_hosts: dir.path().join("known_hosts"),
        save_passwords: SavePasswords::Never,
        ..test_env(dir.path())
    };
    let mut session = dev_session(addr.port());
    session.options.remove("3_8");
    session.options.remove("3_26");
    let mut tab = Tab::open(1, Some(session), &env);
    pump_until(&mut tab, |t| t.term.contents().contains("password:"));
    tab.send(format!("{}\r", tc_devserver::DEV_PASSWORD).as_bytes());
    pump_until(
        &mut tab,
        |t| matches!(&t.sftp, SftpStatus::Ready(v) if v.reports_seen),
    );
    pump_until(&mut tab, |t| t.term.contents().contains("tc$ "));
    let screen = tab.term.contents();
    assert!(!screen.contains("PROMPT_COMMAND"), "{screen}");
}

#[test]
fn saved_password_offer_and_autofill() {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let (addr, fut) = rt
        .block_on(tc_devserver::bind(0, tc_devserver::DevConfig::default()))
        .unwrap();
    rt.spawn(fut);
    let dir = tempfile::tempdir().unwrap();
    let vault = Arc::new(Vault::open(
        dir.path().join("index.toml"),
        Box::new(tc_vault::MemoryStore::default()),
    ));
    let env = TabEnv {
        known_hosts: dir.path().join("known_hosts"),
        vault: vault.clone(),
        ..test_env(dir.path())
    };

    // 1st connection: no saved password -> prompt in the terminal.
    let mut tab = Tab::open(1, Some(dev_session(addr.port())), &env);
    pump_until(&mut tab, |t| t.term.contents().contains("password:"));
    tab.send(format!("{}\r", tc_devserver::DEV_PASSWORD).as_bytes());
    pump_until(&mut tab, |t| !t.offers.is_empty());
    let (target, password) = tab.offers.pop_front().unwrap();
    assert_eq!(
        target,
        Target::ssh(tc_devserver::DEV_USER, "127.0.0.1", addr.port())
    );
    vault.save(&target, &password).unwrap();
    pump_until(&mut tab, |t| !t.is_alive());
    assert!(tab.term.contents().contains("saved-password-test"));

    // 2nd connection: the saved password is used, nothing is typed.
    let mut tab = Tab::open(2, Some(dev_session(addr.port())), &env);
    pump_until(&mut tab, |t| !t.is_alive());
    let screen = tab.term.contents();
    assert!(screen.contains("Using saved password"), "{screen}");
    assert!(!screen.contains("password: "), "{screen}");
    assert!(screen.contains("saved-password-test"), "{screen}");
    assert!(tab.offers.is_empty());
}

/// Run the hook in a real bash (skipped when bash is not installed) and
/// check that the prompt command emits an OSC 7 report for `$PWD`.
#[test]
fn folder_tracking_hook_reports_pwd_in_bash() {
    let hook = FOLDER_TRACKING_HOOK.trim_end_matches('\r');
    let script = format!("{hook}\ncd /tmp && eval \"$PROMPT_COMMAND\"");
    // Like the app: Git Bash / MSYS2 on Windows, never the WSL launcher.
    let Some(bash) = tc_pty::shells::locate(tc_pty::ShellKind::Bash) else {
        eprintln!("bash not available; skipping");
        return;
    };
    let Ok(out) = std::process::Command::new(&bash.program)
        .args(["--norc", "--noprofile", "-c", &script])
        .output()
    else {
        eprintln!("bash not available; skipping");
        return;
    };
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        out.status.success(),
        "{stdout} {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(stdout.starts_with("\x1b]7;file://"), "{stdout:?}");
    assert!(stdout.ends_with("/tmp\x07"), "{stdout:?}");
}

/// A Telnet-like server: sends `banner`, then reports what it receives.
fn recording_server(banner: &'static [u8]) -> (u16, crossbeam_channel::Receiver<Vec<u8>>) {
    use std::io::{Read, Write};
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let (tx, rx) = crossbeam_channel::unbounded::<Vec<u8>>();
    std::thread::spawn(move || {
        let (mut s, _) = listener.accept().unwrap();
        let _ = s.write_all(banner);
        let mut buf = [0u8; 256];
        while let Ok(n) = s.read(&mut buf) {
            if n == 0 || tx.send(buf[..n].to_vec()).is_err() {
                break;
            }
        }
    });
    (port, rx)
}

/// Pump `tab` until the server got `want` (or panic); everything it got.
fn received(tab: &mut Tab, rx: &crossbeam_channel::Receiver<Vec<u8>>, want: &[u8]) -> Vec<u8> {
    let mut got = Vec::new();
    let deadline = Instant::now() + Duration::from_secs(10);
    let done = |got: &[u8]| got.windows(want.len()).any(|w| w == want);
    while !done(&got) && Instant::now() < deadline {
        tab.pump();
        if let Ok(chunk) = rx.recv_timeout(Duration::from_millis(50)) {
            got.extend(chunk);
        }
    }
    assert!(done(&got), "wanted {want:?}, got {got:?}");
    got
}

/// "Force local line editing on": the server gets whole, edited lines.
#[test]
fn local_line_editing_sends_edited_lines() {
    let (port, rx) = recording_server(b"");
    let dir = tempfile::tempdir().unwrap();
    let env = test_env(dir.path());
    let session = Session::new(
        "t",
        SessionKind::Telnet {
            host: "127.0.0.1".into(),
            port,
        },
    );
    let mut tab = Tab::open(1, Some(session), &env);
    tab.expert.local_line_editing = Some(true);
    pump_until(&mut tab, |t| matches!(t.backend, Backend::Stream(_)));
    tab.send(b"lx");
    tab.send(b"\x7fs -a\r");
    // Telnet sends CR as CR NUL (RFC 854); option answers may come first.
    let got = received(&mut tab, &rx, b"ls -a\r");
    assert!(!got.windows(2).any(|w| w == b"lx"), "nothing before Enter");
    assert!(tab.term.contents().contains("ls -a"), "echoed locally");
}

/// Telnet session with a Telnet-type proxy (3_12 = 4): the proxy gets its
/// command line, then carries the session.
#[test]
fn telnet_session_through_its_proxy() {
    use std::io::{BufRead, Write};
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let proxy_port = listener.local_addr().unwrap().port();
    let (tx, rx) = crossbeam_channel::bounded::<String>(1);
    std::thread::spawn(move || {
        let (s, _) = listener.accept().unwrap();
        let mut reader = std::io::BufReader::new(s.try_clone().unwrap());
        let mut line = String::new();
        reader.read_line(&mut line).unwrap();
        let _ = tx.send(line);
        let mut s = s;
        s.write_all(&[255, 251, 1]).unwrap();
        s.write_all(b"Welcome via proxy\r\n").unwrap();
        std::thread::sleep(Duration::from_millis(500));
    });
    let dir = tempfile::tempdir().unwrap();
    let mut session = Session::new(
        "t",
        SessionKind::Telnet {
            host: "target.invalid".into(),
            port: 23,
        },
    );
    for (k, v) in [("3_12", "4"), ("3_13", "127.0.0.1")] {
        session.options.insert(k.into(), v.into());
    }
    session
        .options
        .insert("3_14".into(), proxy_port.to_string());
    let mut tab = Tab::open(1, Some(session), &test_env(dir.path()));
    pump_until(&mut tab, |t| {
        t.term.contents().contains("Welcome via proxy")
    });
    assert_eq!(
        rx.recv_timeout(Duration::from_secs(5)).unwrap(),
        "connect target.invalid 23\n"
    );
}

/// A proxy that cannot be used for this session type stops it: never a
/// direct connection instead.
#[test]
fn unusable_proxy_refuses_instead_of_connecting_directly() {
    let target = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    target.set_nonblocking(true).unwrap();
    let dir = tempfile::tempdir().unwrap();
    let mut session = Session::new(
        "t",
        SessionKind::Telnet {
            host: "127.0.0.1".into(),
            port: target.local_addr().unwrap().port(),
        },
    );
    for (k, v) in [("3_12", "7"), ("3_13", "127.0.0.1")] {
        session.options.insert(k.into(), v.into());
    }
    let mut tab = Tab::open(1, Some(session), &test_env(dir.path()));
    pump_until(&mut tab, |t| t.term.contents().contains("Not connected"));
    assert!(target.accept().is_err(), "no direct connection");
}

/// 4_38 "Execute macro at session start": typed once the server talks.
#[test]
fn start_macro_runs_after_the_first_output() {
    let (port, rx) = recording_server(b"login: ");
    let dir = tempfile::tempdir().unwrap();
    let session = Session::new(
        "t",
        SessionKind::Telnet {
            host: "127.0.0.1".into(),
            port,
        },
    );
    let mut tab = Tab::open(1, Some(session), &test_env(dir.path()));
    tab.start_macro = Some("admin\r".into());
    received(&mut tab, &rx, b"admin\r");
    assert!(tab.start_macro.is_none(), "only once");
}
