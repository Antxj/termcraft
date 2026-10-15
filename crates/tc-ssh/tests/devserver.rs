//! End-to-end tests against the in-process development server.

use russh::keys::PrivateKey;
use russh::keys::ssh_key::LineEnding;
use russh::keys::ssh_key::private::Ed25519Keypair;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;
use tc_devserver::{DEV_PASSWORD, DEV_USER, DevConfig, bind};
use tc_ssh::{ConnectParams, Reply, SecretKind, SshEvent, SshSession};

struct Server {
    _rt: tokio::runtime::Runtime,
    port: u16,
}

fn start(config: DevConfig) -> Server {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let (addr, fut) = rt.block_on(bind(0, config)).unwrap();
    rt.spawn(fut);
    Server {
        _rt: rt,
        port: addr.port(),
    }
}

fn params(port: u16, known_hosts: &Path, command: &str) -> ConnectParams {
    ConnectParams {
        host: "127.0.0.1".into(),
        port,
        user: DEV_USER.into(),
        private_key: None,
        gateways: Vec::new(),
        known_hosts: known_hosts.to_path_buf(),
        compression: false,
        command: Some(command.into()),
        term: "xterm-256color".into(),
        cols: 80,
        rows: 24,
        sftp: false,
        browser_protocol: Default::default(),
        keepalive: None,
        detect_login_shell: false,
        remote_monitoring: false,
        // Tests must not pick up the developer's agent keys.
        agent: Default::default(),
        shell: true,
        proxy: None,
        agent_forwarding: false,
        x11_display: None,
        refuse_trivial_auth: false,
        show_banner: false,
        preserve_dates: true,
        algorithms: Default::default(),
        certificate: None,
    }
}

#[derive(Default, Debug)]
struct Outcome {
    output: String,
    host_key_prompts: usize,
    secret_prompts: usize,
    errors: Vec<String>,
    exit_status: Option<u32>,
}

/// Drive a session to completion; `answer` decides replies to prompts.
fn drive(session: &SshSession, mut answer: impl FnMut(&SshEvent) -> Option<Reply>) -> Outcome {
    let mut out = Outcome::default();
    loop {
        let event = session
            .events()
            .recv_timeout(Duration::from_secs(20))
            .expect("ssh event timeout");
        match &event {
            SshEvent::Output(b) => out.output.push_str(&String::from_utf8_lossy(b)),
            SshEvent::HostKey(_) => out.host_key_prompts += 1,
            SshEvent::SecretNeeded { .. } => out.secret_prompts += 1,
            SshEvent::Error(e) => out.errors.push(e.clone()),
            SshEvent::Closed { exit_status } => {
                out.exit_status = *exit_status;
                return out;
            }
            SshEvent::Info(_)
            | SshEvent::Connected
            | SshEvent::Sftp(_)
            | SshEvent::Listening(_)
            | SshEvent::LoginShell(_)
            | SshEvent::Monitor(_) => {}
        }
        if let Some(reply) = answer(&event) {
            session.reply(reply);
        }
    }
}

fn accept_and_login(event: &SshEvent) -> Option<Reply> {
    match event {
        SshEvent::HostKey(_) => Some(Reply::HostKey {
            accept: true,
            remember: true,
        }),
        SshEvent::SecretNeeded { .. } => Some(Reply::Secret(DEV_PASSWORD.into())),
        _ => None,
    }
}

fn noop() -> Arc<dyn Fn() + Send + Sync> {
    Arc::new(|| {})
}

#[test]
fn password_exec_and_known_hosts() {
    let server = start(DevConfig::default());
    let dir = tempfile::tempdir().unwrap();
    let known = dir.path().join("known_hosts");

    let s = SshSession::connect(params(server.port, &known, "echo hello-termcraft"), noop());
    let first = drive(&s, accept_and_login);
    assert!(first.errors.is_empty(), "{first:?}");
    assert!(first.output.contains("hello-termcraft"), "{first:?}");
    assert_eq!(first.exit_status, Some(0));
    assert_eq!(first.host_key_prompts, 1);
    let recorded = std::fs::read_to_string(&known).unwrap();
    assert!(
        recorded.contains(&format!("[127.0.0.1]:{}", server.port)),
        "{recorded}"
    );

    // Second connection: host key is known, no prompt.
    let s = SshSession::connect(params(server.port, &known, "echo again"), noop());
    let second = drive(&s, accept_and_login);
    assert_eq!(second.host_key_prompts, 0, "{second:?}");
    assert!(second.output.contains("again"));
}

#[test]
fn wrong_password_then_cancel() {
    let server = start(DevConfig::default());
    let dir = tempfile::tempdir().unwrap();
    let known = dir.path().join("known_hosts");
    let s = SshSession::connect(params(server.port, &known, "echo never"), noop());
    let mut tries = 0;
    let out = drive(&s, |e| match e {
        SshEvent::HostKey(_) => Some(Reply::HostKey {
            accept: true,
            remember: false,
        }),
        SshEvent::SecretNeeded { .. } => {
            tries += 1;
            Some(if tries == 1 {
                Reply::Secret("wrong".into())
            } else {
                Reply::Cancel
            })
        }
        _ => None,
    });
    assert_eq!(out.secret_prompts, 2, "{out:?}");
    assert!(out.output.contains("Access denied"), "{out:?}");
    assert!(!out.output.contains("never"));
    assert!(!out.errors.is_empty());
    assert!(!known.exists(), "remember=false must not write known_hosts");
}

#[test]
fn public_key_login() {
    let key = PrivateKey::from(Ed25519Keypair::from_seed(&[42; 32]));
    let config = DevConfig {
        authorized_keys: vec![key.public_key().clone()],
        ..DevConfig::default()
    };
    let server = start(config);
    let dir = tempfile::tempdir().unwrap();
    let key_path = dir.path().join("id_ed25519");
    std::fs::write(
        &key_path,
        key.to_openssh(LineEnding::LF).unwrap().as_bytes(),
    )
    .unwrap();

    let mut p = params(server.port, &dir.path().join("known_hosts"), "echo via-key");
    p.private_key = Some(key_path);
    let s = SshSession::connect(p, noop());
    let out = drive(&s, accept_and_login);
    assert_eq!(out.secret_prompts, 0, "{out:?}");
    assert!(out.output.contains("via-key"), "{out:?}");
}

/// Login with a key held by an SSH agent (russh's agent server on a Unix
/// socket); TermCraft never sees the private key.
#[cfg(unix)]
#[test]
fn agent_login() {
    use russh::keys::agent::client::AgentClient;
    let key = PrivateKey::from(Ed25519Keypair::from_seed(&[43; 32]));
    let server = start(DevConfig {
        authorized_keys: vec![key.public_key().clone()],
        ..DevConfig::default()
    });
    let dir = tempfile::tempdir().unwrap();
    let sock = dir.path().join("agent.sock");
    let rt = tokio::runtime::Runtime::new().unwrap();
    let listener = rt
        .block_on(async { tokio::net::UnixListener::bind(&sock) })
        .unwrap();
    let incoming = futures::stream::unfold(listener, |l| async move {
        Some((l.accept().await.map(|(s, _)| s), l))
    });
    rt.spawn(russh::keys::agent::server::serve(Box::pin(incoming), ()));
    rt.block_on(async {
        let mut client = AgentClient::connect_uds(&sock).await.unwrap();
        client.add_identity(&key, &[]).await.unwrap();
    });

    let mut p = params(
        server.port,
        &dir.path().join("known_hosts"),
        "echo via-agent",
    );
    p.agent = tc_ssh::AgentConfig {
        system: true,
        pageant: false,
        socket: Some(sock),
        keys: Vec::new(),
    };
    let s = SshSession::connect(p, noop());
    let out = drive(&s, accept_and_login);
    assert_eq!(out.secret_prompts, 0, "{out:?}");
    assert!(out.output.contains("via-agent"), "{out:?}");
}

/// A PuTTY key file with a passphrase (written by the key generator).
#[test]
fn encrypted_ppk_key_login() {
    let key = tc_ssh::keygen::generate(tc_ssh::keygen::KeyKind::Ed25519, "ppk-test").unwrap();
    let server = start(DevConfig {
        authorized_keys: vec![key.public_key().clone()],
        ..DevConfig::default()
    });
    let dir = tempfile::tempdir().unwrap();
    let key_path = dir.path().join("id.ppk");
    std::fs::write(
        &key_path,
        tc_ssh::keygen::to_ppk(&key, Some("pp")).unwrap().as_bytes(),
    )
    .unwrap();
    let mut p = params(server.port, &dir.path().join("known_hosts"), "echo via-ppk");
    p.private_key = Some(key_path);
    let s = SshSession::connect(p, noop());
    let out = drive(&s, |e| match e {
        SshEvent::SecretNeeded {
            kind: SecretKind::KeyPassphrase,
            ..
        } => Some(Reply::Secret("pp".into())),
        other => accept_and_login(other),
    });
    assert_eq!(out.secret_prompts, 1, "only the passphrase: {out:?}");
    assert!(out.output.contains("via-ppk"), "{out:?}");
}

#[test]
fn changed_host_key_warns_and_cancel_refuses() {
    let server = start(DevConfig::default());
    let dir = tempfile::tempdir().unwrap();
    let known = dir.path().join("known_hosts");
    // Record a *different* ed25519 key for this host:port.
    let other = PrivateKey::from(Ed25519Keypair::from_seed(&[7; 32]));
    russh::keys::known_hosts::learn_known_hosts_path(
        "127.0.0.1",
        server.port,
        other.public_key(),
        &known,
    )
    .unwrap();

    let s = SshSession::connect(params(server.port, &known, "echo should-not-run"), noop());
    let mut changed_line = None;
    let out = drive(&s, |e| match e {
        SshEvent::HostKey(info) => {
            changed_line = info.changed_line;
            Some(Reply::HostKey {
                accept: false,
                remember: false,
            })
        }
        _ => None,
    });
    assert_eq!(out.host_key_prompts, 1, "{out:?}");
    assert!(!out.output.contains("should-not-run"));
    let saved = std::fs::read_to_string(&known).unwrap();
    let line = changed_line.expect("the alert names the saved key");
    let other_b64 = other.public_key().to_openssh().unwrap();
    let other_b64 = other_b64.split_whitespace().nth(1).unwrap();
    assert!(saved.lines().nth(line - 1).unwrap().contains(other_b64));
    assert_eq!(entries(&saved), 1, "cancel leaves known_hosts alone");
}

#[test]
fn accepting_a_changed_host_key_replaces_the_old_one() {
    let server = start(DevConfig::default());
    let dir = tempfile::tempdir().unwrap();
    let known = dir.path().join("known_hosts");
    let other = PrivateKey::from(Ed25519Keypair::from_seed(&[7; 32]));
    russh::keys::known_hosts::learn_known_hosts_path(
        "127.0.0.1",
        server.port,
        other.public_key(),
        &known,
    )
    .unwrap();

    let s = SshSession::connect(params(server.port, &known, "echo replaced-ok"), noop());
    let out = drive(&s, accept_and_login);
    assert!(out.output.contains("replaced-ok"), "{out:?}");
    // Next time the new key is known: no question.
    let s = SshSession::connect(params(server.port, &known, "echo again"), noop());
    let out = drive(&s, accept_and_login);
    assert_eq!(out.host_key_prompts, 0, "{out:?}");
    assert_eq!(entries(&std::fs::read_to_string(&known).unwrap()), 1);
}

fn entries(known_hosts: &str) -> usize {
    known_hosts.lines().filter(|l| !l.trim().is_empty()).count()
}

/// Wait for the first event matching `want`, answering prompts on the way.
fn wait_for(session: &SshSession, mut want: impl FnMut(&SshEvent) -> bool) -> SshEvent {
    loop {
        let event = session
            .events()
            .recv_timeout(Duration::from_secs(20))
            .expect("ssh event timeout");
        if want(&event) {
            return event;
        }
        assert!(
            !matches!(event, SshEvent::Closed { .. }),
            "session closed early"
        );
        if let Some(reply) = accept_and_login(&event) {
            session.reply(reply);
        }
    }
}

/// The monitoring script reads `/proc`, so this runs on Linux only (the dev
/// server's exec goes through `sh` there).
#[cfg(target_os = "linux")]
#[test]
fn remote_monitoring_reports_samples() {
    // The monitoring script never ends: its samples must be streamed.
    let server = start(DevConfig {
        exec_shell: Some("sh".into()),
        ..DevConfig::default()
    });
    let dir = tempfile::tempdir().unwrap();
    let mut p = params(server.port, &dir.path().join("known_hosts"), "sleep 10");
    p.remote_monitoring = true;
    let s = SshSession::connect(p, noop());
    let SshEvent::Monitor(first) = wait_for(&s, |e| matches!(e, SshEvent::Monitor(_))) else {
        unreachable!()
    };
    assert!(first.mem_total.is_some(), "{first:?}");
    assert!(first.uptime_secs.is_some(), "{first:?}");
    // The second reading has rates.
    let SshEvent::Monitor(second) = wait_for(&s, |e| matches!(e, SshEvent::Monitor(_))) else {
        unreachable!()
    };
    assert!(second.cpu_percent.is_some(), "{second:?}");
    s.close();
}

/// A tiny SOCKS5 proxy (no auth) on a thread; counts the connections.
fn socks5_proxy() -> (u16, std::sync::Arc<std::sync::atomic::AtomicUsize>) {
    use std::io::{Read, Write};
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let used = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let counter = used.clone();
    std::thread::spawn(move || {
        for client in listener.incoming().flatten() {
            counter.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            std::thread::spawn(move || {
                let mut c = client;
                let mut hello = [0u8; 3];
                c.read_exact(&mut hello).unwrap();
                c.write_all(&[5, 0]).unwrap();
                let mut head = [0u8; 4];
                c.read_exact(&mut head).unwrap();
                let host = match head[3] {
                    1 => {
                        let mut ip = [0u8; 4];
                        c.read_exact(&mut ip).unwrap();
                        std::net::Ipv4Addr::from(ip).to_string()
                    }
                    3 => {
                        let mut len = [0u8; 1];
                        c.read_exact(&mut len).unwrap();
                        let mut name = vec![0u8; usize::from(len[0])];
                        c.read_exact(&mut name).unwrap();
                        String::from_utf8(name).unwrap()
                    }
                    _ => panic!("address type"),
                };
                let mut port = [0u8; 2];
                c.read_exact(&mut port).unwrap();
                let target =
                    std::net::TcpStream::connect((host.as_str(), u16::from_be_bytes(port)))
                        .unwrap();
                c.write_all(&[5, 0, 0, 1, 0, 0, 0, 0, 0, 0]).unwrap();
                let (mut c2, mut t2) = (c.try_clone().unwrap(), target.try_clone().unwrap());
                let mut t = target;
                std::thread::spawn(move || std::io::copy(&mut t2, &mut c2));
                let _ = std::io::copy(&mut c, &mut t);
            });
        }
    });
    (port, used)
}

#[test]
fn login_through_a_socks5_proxy() {
    let server = start(DevConfig::default());
    let (proxy_port, used) = socks5_proxy();
    let dir = tempfile::tempdir().unwrap();
    let mut p = params(
        server.port,
        &dir.path().join("known_hosts"),
        "echo via-proxy",
    );
    p.proxy = Some(tc_ssh::proxy::Proxy {
        kind: tc_ssh::proxy::ProxyKind::Socks5,
        host: "127.0.0.1".into(),
        port: proxy_port,
        user: None,
    });
    let s = SshSession::connect(p, noop());
    let out = drive(&s, accept_and_login);
    assert!(out.output.contains("via-proxy"), "{out:?}");
    assert_eq!(used.load(std::sync::atomic::Ordering::SeqCst), 1);
}

/// SFTP session: no terminal at all, only the browser.
#[test]
fn sftp_session_without_a_shell() {
    use tc_ssh::{SftpEvent, SftpRequest};
    let root = tempfile::tempdir().unwrap();
    std::fs::write(root.path().join("only.txt"), b"x").unwrap();
    let server = start(DevConfig {
        sftp_root: root.path().to_path_buf(),
        ..DevConfig::default()
    });
    let local = tempfile::tempdir().unwrap();
    let mut p = params(server.port, &local.path().join("known_hosts"), "");
    p.command = None;
    p.sftp = true;
    p.shell = false;
    let s = SshSession::connect(p, noop());
    wait_for(&s, |e| matches!(e, SshEvent::Sftp(SftpEvent::Ready { .. })));
    let id = s.sftp(SftpRequest::List("/".into()));
    let listing = wait_for(
        &s,
        |e| matches!(e, SshEvent::Sftp(SftpEvent::Listing { id: got, .. }) if *got == id),
    );
    let SshEvent::Sftp(SftpEvent::Listing { entries, .. }) = listing else {
        unreachable!()
    };
    assert_eq!(entries[0].name, "only.txt");
    // No shell was started, so no terminal output arrives.
    assert!(
        s.events()
            .try_iter()
            .all(|e| !matches!(e, SshEvent::Output(_)))
    );
    s.close();
}

#[test]
fn sftp_browser_operations() {
    use tc_ssh::{SftpEvent, SftpRequest};
    let root = tempfile::tempdir().unwrap();
    std::fs::write(root.path().join("hello.txt"), b"hello over sftp").unwrap();
    let server = start(DevConfig {
        sftp_root: root.path().to_path_buf(),
        ..DevConfig::default()
    });
    let local = tempfile::tempdir().unwrap();
    let mut p = params(server.port, &local.path().join("known_hosts"), "");
    p.command = None;
    p.sftp = true;
    let s = SshSession::connect(p, noop());

    let ready = wait_for(&s, |e| matches!(e, SshEvent::Sftp(SftpEvent::Ready { .. })));
    assert_eq!(ready, SshEvent::Sftp(SftpEvent::Ready { home: "/".into() }));

    // Each step is printed, so a hang shows where it happened.
    let done = |s: &SshSession, id: u64| {
        eprintln!("waiting for request {id}");
        let e = wait_for(
            s,
            |e| matches!(e, SshEvent::Sftp(SftpEvent::Done { id: i } | SftpEvent::Failed { id: i, .. }) if *i == id),
        );
        assert_eq!(
            e,
            SshEvent::Sftp(SftpEvent::Done { id }),
            "request {id} failed"
        );
    };

    let id = s.sftp(SftpRequest::List("/".into()));
    let listing = wait_for(
        &s,
        |e| matches!(e, SshEvent::Sftp(SftpEvent::Listing { id: i, .. }) if *i == id),
    );
    let SshEvent::Sftp(SftpEvent::Listing { entries, .. }) = listing else {
        unreachable!()
    };
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].name, "hello.txt");
    assert_eq!(entries[0].size, 15);
    assert!(!entries[0].is_dir);

    // download
    let dl = local.path().join("dl.txt");
    let id = s.sftp(SftpRequest::Download {
        remote: "/hello.txt".into(),
        local: dl.clone(),
        ascii: false,
    });
    done(&s, id);
    assert_eq!(std::fs::read(&dl).unwrap(), b"hello over sftp");

    // upload a file larger than one chunk
    let big: Vec<u8> = (0..200_000u32).map(|i| (i % 251) as u8).collect();
    let up = local.path().join("up.bin");
    std::fs::write(&up, &big).unwrap();
    let id = s.sftp(SftpRequest::Upload {
        local: up.clone(),
        remote: "/up.bin".into(),
        ascii: false,
    });
    done(&s, id);
    assert_eq!(std::fs::read(root.path().join("up.bin")).unwrap(), big);

    // "Preserve files dates": both ways.
    let old = std::time::UNIX_EPOCH + std::time::Duration::from_secs(1_500_000_000);
    let mtime = |p: &std::path::Path| std::fs::metadata(p).unwrap().modified().unwrap();
    std::fs::File::options()
        .write(true)
        .open(&up)
        .unwrap()
        .set_modified(old)
        .unwrap();
    let id = s.sftp(SftpRequest::Upload {
        local: up.clone(),
        remote: "/dated.bin".into(),
        ascii: false,
    });
    done(&s, id);
    assert_eq!(mtime(&root.path().join("dated.bin")), old);
    let dated = local.path().join("dated.bin");
    let id = s.sftp(SftpRequest::Download {
        remote: "/dated.bin".into(),
        local: dated.clone(),
        ascii: false,
    });
    done(&s, id);
    assert_eq!(mtime(&dated), old);

    // ASCII mode: CRLF goes up as LF, and comes back with local endings.
    let text = local.path().join("text.txt");
    std::fs::write(&text, b"one\r\ntwo\r\n").unwrap();
    let id = s.sftp(SftpRequest::Upload {
        local: text,
        remote: "/text.txt".into(),
        ascii: true,
    });
    done(&s, id);
    assert_eq!(
        std::fs::read(root.path().join("text.txt")).unwrap(),
        b"one\ntwo\n"
    );
    let back = local.path().join("back.txt");
    let id = s.sftp(SftpRequest::Download {
        remote: "/text.txt".into(),
        local: back.clone(),
        ascii: true,
    });
    done(&s, id);
    let expected: &[u8] = if cfg!(windows) {
        b"one\r\ntwo\r\n"
    } else {
        b"one\ntwo\n"
    };
    assert_eq!(std::fs::read(&back).unwrap(), expected);

    // Permissions: read-only, then writable again.
    done(
        &s,
        s.sftp(SftpRequest::SetPermissions {
            path: "/text.txt".into(),
            mode: 0o444,
        }),
    );
    let meta = std::fs::metadata(root.path().join("text.txt")).unwrap();
    assert!(meta.permissions().readonly());
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(meta.permissions().mode() & 0o777, 0o444);
    }
    done(
        &s,
        s.sftp(SftpRequest::SetPermissions {
            path: "/text.txt".into(),
            mode: 0o644,
        }),
    );
    assert!(
        !std::fs::metadata(root.path().join("text.txt"))
            .unwrap()
            .permissions()
            .readonly()
    );

    // mkdir, rename into it, remove both
    done(&s, s.sftp(SftpRequest::Mkdir("/d".into())));
    done(
        &s,
        s.sftp(SftpRequest::Rename {
            from: "/up.bin".into(),
            to: "/d/up2.bin".into(),
        }),
    );
    assert!(root.path().join("d/up2.bin").exists());
    done(
        &s,
        s.sftp(SftpRequest::Remove {
            path: "/d/up2.bin".into(),
            is_dir: false,
        }),
    );
    done(
        &s,
        s.sftp(SftpRequest::Remove {
            path: "/d".into(),
            is_dir: true,
        }),
    );
    assert!(!root.path().join("d").exists());

    // errors are reported, not fatal
    let id = s.sftp(SftpRequest::Remove {
        path: "/missing".into(),
        is_dir: false,
    });
    let e = wait_for(
        &s,
        |e| matches!(e, SshEvent::Sftp(SftpEvent::Failed { id: i, .. }) if *i == id),
    );
    assert!(matches!(e, SshEvent::Sftp(SftpEvent::Failed { .. })));
    s.close();
}

/// A POSIX shell for the dev server's exec requests (Git Bash on Windows).
fn posix_shell() -> Option<String> {
    if cfg!(windows) {
        let pf = std::env::var_os("ProgramFiles")?;
        let bash = Path::new(&pf).join("Git").join("bin").join("bash.exe");
        bash.is_file().then(|| bash.display().to_string())
    } else {
        Some("sh".into())
    }
}

/// No SFTP subsystem on the server: the browser falls back to SCP, and
/// every operation works through scp / ls / mkdir / mv / rm / chmod.
#[test]
fn scp_browser_when_sftp_is_missing() {
    use tc_ssh::{SftpEvent, SftpRequest};
    let Some(shell) = posix_shell() else {
        eprintln!("no POSIX shell; skipping");
        return;
    };
    let root = tempfile::tempdir().unwrap();
    std::fs::write(root.path().join("hello.txt"), b"hello over scp").unwrap();
    let dir = root.path().display().to_string().replace('\\', "/");
    let server = start(DevConfig {
        exec_shell: Some(shell),
        sftp_subsystem: false,
        ..DevConfig::default()
    });
    let local = tempfile::tempdir().unwrap();
    let mut p = params(server.port, &local.path().join("known_hosts"), "");
    p.command = None;
    p.sftp = true;
    p.shell = false;
    let s = SshSession::connect(p, noop());
    let ready = wait_for(&s, |e| {
        matches!(
            e,
            SshEvent::Sftp(SftpEvent::Ready { .. } | SftpEvent::Unavailable(_))
        )
    });
    if let SshEvent::Sftp(SftpEvent::Unavailable(e)) = &ready {
        eprintln!("scp is not available here ({e}); skipping");
        return;
    }
    let done = |s: &SshSession, id: u64| {
        let e = wait_for(
            s,
            |e| matches!(e, SshEvent::Sftp(SftpEvent::Done { id: i } | SftpEvent::Failed { id: i, .. }) if *i == id),
        );
        assert_eq!(e, SshEvent::Sftp(SftpEvent::Done { id }), "request {id}");
    };
    let list = |s: &SshSession| {
        let id = s.sftp(SftpRequest::List(dir.clone()));
        eprintln!("listing (request {id})");
        match wait_for(
            s,
            |e| matches!(e, SshEvent::Sftp(SftpEvent::Listing { id: i, .. } | SftpEvent::Failed { id: i, .. }) if *i == id),
        ) {
            SshEvent::Sftp(SftpEvent::Listing { entries, .. }) => entries,
            other => panic!("{other:?}"),
        }
    };
    let entries = list(&s);
    assert_eq!(entries.len(), 1, "{entries:?}");
    assert_eq!(
        (entries[0].name.as_str(), entries[0].size),
        ("hello.txt", 14)
    );

    // Download, and an upload bigger than one block.
    let dl = local.path().join("dl.txt");
    done(
        &s,
        s.sftp(SftpRequest::Download {
            remote: format!("{dir}/hello.txt"),
            local: dl.clone(),
            ascii: false,
        }),
    );
    assert_eq!(std::fs::read(&dl).unwrap(), b"hello over scp");
    let big: Vec<u8> = (0..300_000u32).map(|i| (i % 251) as u8).collect();
    let up = local.path().join("up.bin");
    std::fs::write(&up, &big).unwrap();
    done(
        &s,
        s.sftp(SftpRequest::Upload {
            local: up,
            remote: format!("{dir}/it's up.bin"),
            ascii: false,
        }),
    );
    assert_eq!(std::fs::read(root.path().join("it's up.bin")).unwrap(), big);

    // mkdir, rename into it, remove both; an empty file.
    done(&s, s.sftp(SftpRequest::Mkdir(format!("{dir}/d"))));
    done(
        &s,
        s.sftp(SftpRequest::Rename {
            from: format!("{dir}/it's up.bin"),
            to: format!("{dir}/d/up2.bin"),
        }),
    );
    assert!(root.path().join("d/up2.bin").exists());
    done(
        &s,
        s.sftp(SftpRequest::CreateFile(format!("{dir}/new.txt"))),
    );
    assert!(root.path().join("new.txt").is_file());
    let names: Vec<String> = list(&s).into_iter().map(|e| e.name).collect();
    assert_eq!(names, ["d", "hello.txt", "new.txt"]);
    done(
        &s,
        s.sftp(SftpRequest::Remove {
            path: format!("{dir}/d/up2.bin"),
            is_dir: false,
        }),
    );
    done(
        &s,
        s.sftp(SftpRequest::Remove {
            path: format!("{dir}/d"),
            is_dir: true,
        }),
    );
    assert!(!root.path().join("d").exists());

    // Errors are reported.
    let id = s.sftp(SftpRequest::Download {
        remote: format!("{dir}/missing"),
        local: local.path().join("x"),
        ascii: false,
    });
    let e = wait_for(
        &s,
        |e| matches!(e, SshEvent::Sftp(SftpEvent::Done { id: i } | SftpEvent::Failed { id: i, .. }) if *i == id),
    );
    assert!(
        matches!(e, SshEvent::Sftp(SftpEvent::Failed { .. })),
        "{e:?}"
    );
    s.close();
}

// ---------------------------------------------------------------- tunnels

/// Plain TCP echo server on a free loopback port (std, own thread).
fn echo_server() -> u16 {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            std::thread::spawn(move || {
                let mut s = stream;
                let mut buf = [0u8; 1024];
                while let Ok(n) = std::io::Read::read(&mut s, &mut buf) {
                    if n == 0 || std::io::Write::write_all(&mut s, &buf[..n]).is_err() {
                        break;
                    }
                }
            });
        }
    });
    port
}

fn endpoint(port: u16) -> tc_ssh::Endpoint {
    tc_ssh::Endpoint {
        host: "127.0.0.1".into(),
        port,
        user: DEV_USER.into(),
        private_key: None,
    }
}

/// Start a tunnel and return the address it listens on.
fn start_tunnel(server: &Server, spec: tc_ssh::TunnelSpec, known: &Path) -> (SshSession, String) {
    let s = SshSession::tunnel(
        tc_ssh::TunnelParams {
            agent: Default::default(),
            ssh: endpoint(server.port),
            gateways: Vec::new(),
            known_hosts: known.to_path_buf(),
            spec,
        },
        noop(),
    );
    let SshEvent::Listening(text) = wait_for(&s, |e| matches!(e, SshEvent::Listening(_))) else {
        unreachable!()
    };
    // "127.0.0.1:1234 → …", "SOCKS5 proxy on 127.0.0.1:1234", "127.0.0.1:1234 on the server → …"
    let addr = text
        .split_whitespace()
        .find(|w| w.starts_with("127.0.0.1:"))
        .expect("address in Listening text")
        .to_string();
    (s, addr)
}

fn roundtrip(stream: &mut std::net::TcpStream, payload: &[u8]) -> Vec<u8> {
    use std::io::{Read, Write};
    stream
        .set_read_timeout(Some(Duration::from_secs(10)))
        .unwrap();
    stream.write_all(payload).unwrap();
    let mut got = vec![0u8; payload.len()];
    stream.read_exact(&mut got).unwrap();
    got
}

#[test]
fn local_tunnel_forwards_bytes() {
    let echo = echo_server();
    let server = start(DevConfig::default());
    let dir = tempfile::tempdir().unwrap();
    let spec = tc_ssh::TunnelSpec::Local {
        bind: "127.0.0.1".into(),
        port: 0,
        dest_host: "127.0.0.1".into(),
        dest_port: echo,
    };
    let (_tunnel, addr) = start_tunnel(&server, spec, &dir.path().join("kh"));
    let mut c = std::net::TcpStream::connect(&addr).unwrap();
    assert_eq!(
        roundtrip(&mut c, b"ping through local tunnel"),
        b"ping through local tunnel"
    );
}

#[test]
fn remote_tunnel_forwards_bytes() {
    let echo = echo_server();
    let server = start(DevConfig::default());
    let dir = tempfile::tempdir().unwrap();
    let spec = tc_ssh::TunnelSpec::Remote {
        bind: "127.0.0.1".into(),
        port: 0,
        dest_host: "127.0.0.1".into(),
        dest_port: echo,
    };
    let (_tunnel, addr) = start_tunnel(&server, spec, &dir.path().join("kh"));
    let mut c = std::net::TcpStream::connect(&addr).unwrap();
    assert_eq!(
        roundtrip(&mut c, b"ping through remote tunnel"),
        b"ping through remote tunnel"
    );
}

#[test]
fn dynamic_socks5_tunnel() {
    use std::io::{Read, Write};
    let echo = echo_server();
    let server = start(DevConfig::default());
    let dir = tempfile::tempdir().unwrap();
    let spec = tc_ssh::TunnelSpec::Dynamic {
        bind: "127.0.0.1".into(),
        port: 0,
    };
    let (_tunnel, addr) = start_tunnel(&server, spec, &dir.path().join("kh"));
    let mut c = std::net::TcpStream::connect(&addr).unwrap();
    c.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
    c.write_all(&[5, 1, 0]).unwrap();
    let mut r = [0u8; 2];
    c.read_exact(&mut r).unwrap();
    assert_eq!(r, [5, 0]);
    let mut req = vec![5, 1, 0, 3, 9];
    req.extend_from_slice(b"127.0.0.1");
    req.extend_from_slice(&echo.to_be_bytes());
    c.write_all(&req).unwrap();
    let mut rep = [0u8; 10];
    c.read_exact(&mut rep).unwrap();
    assert_eq!(rep[1], 0, "SOCKS reply {rep:?}");
    assert_eq!(roundtrip(&mut c, b"hello socks"), b"hello socks");
}

#[test]
fn jump_host_shell() {
    let gateway = start(DevConfig::default());
    let target = start(DevConfig::default());
    let dir = tempfile::tempdir().unwrap();
    let mut p = params(target.port, &dir.path().join("kh"), "echo via-gateway");
    p.gateways = vec![endpoint(gateway.port)];
    let s = SshSession::connect(p, noop());
    let out = drive(&s, accept_and_login);
    assert!(out.errors.is_empty(), "{out:?}");
    assert_eq!(
        out.host_key_prompts, 2,
        "gateway and target keys are both checked"
    );
    assert_eq!(out.secret_prompts, 2, "one login per hop");
    assert!(out.output.contains("via-gateway"), "{out:?}");
}

#[test]
fn two_jump_hosts_shell() {
    let first = start(DevConfig::default());
    let second = start(DevConfig::default());
    let target = start(DevConfig::default());
    let dir = tempfile::tempdir().unwrap();
    let mut p = params(target.port, &dir.path().join("kh"), "echo via-two-gateways");
    // The second gateway is reached from the first one, on its loopback.
    p.gateways = vec![endpoint(first.port), endpoint(second.port)];
    let s = SshSession::connect(p, noop());
    let out = drive(&s, accept_and_login);
    assert!(out.errors.is_empty(), "{out:?}");
    assert_eq!(out.host_key_prompts, 3, "every hop's key is checked");
    assert_eq!(out.secret_prompts, 3, "one login per hop");
    assert!(out.output.contains("via-two-gateways"), "{out:?}");
}

fn ssh_proxy(kind: tc_ssh::proxy::ProxyKind, port: u16) -> tc_ssh::proxy::Proxy {
    tc_ssh::proxy::Proxy {
        kind,
        host: "127.0.0.1".into(),
        port,
        user: Some(DEV_USER.into()),
    }
}

/// Proxy type "SSH forwarding": the proxy is an SSH server, logged in to
/// first, that forwards the connection like a gateway.
#[test]
fn login_through_an_ssh_forwarding_proxy() {
    let proxy = start(DevConfig::default());
    let target = start(DevConfig::default());
    let dir = tempfile::tempdir().unwrap();
    let mut p = params(target.port, &dir.path().join("kh"), "echo via-ssh-proxy");
    p.proxy = Some(ssh_proxy(tc_ssh::proxy::ProxyKind::SshForward, proxy.port));
    let s = SshSession::connect(p, noop());
    let out = drive(&s, accept_and_login);
    assert!(out.errors.is_empty(), "{out:?}");
    assert_eq!(out.host_key_prompts, 2, "proxy and target keys are checked");
    assert_eq!(out.secret_prompts, 2, "one login for each");
    assert!(out.output.contains("via-ssh-proxy"), "{out:?}");
}

/// Proxy type "SSH command": `nc` on the proxy carries the connection.
#[cfg(unix)]
#[test]
fn login_through_an_ssh_command_proxy() {
    let has_nc = std::process::Command::new("sh")
        .args(["-c", "command -v nc"])
        .output()
        .is_ok_and(|o| o.status.success());
    if !has_nc {
        eprintln!("skipped: no nc");
        return;
    }
    // The command's output must be streamed, not buffered until it ends.
    let proxy = start(DevConfig {
        exec_shell: Some("sh".into()),
        ..DevConfig::default()
    });
    let target = start(DevConfig::default());
    let dir = tempfile::tempdir().unwrap();
    let mut p = params(target.port, &dir.path().join("kh"), "echo via-nc");
    p.proxy = Some(ssh_proxy(
        tc_ssh::proxy::ProxyKind::SshCommand(tc_ssh::proxy::DEFAULT_SSH_COMMAND.into()),
        proxy.port,
    ));
    let s = SshSession::connect(p, noop());
    let out = drive(&s, accept_and_login);
    assert!(out.errors.is_empty(), "{out:?}");
    assert!(out.output.contains("via-nc"), "{out:?}");
}

/// Names that could change the proxy command are refused.
#[test]
fn ssh_command_proxy_refuses_odd_host_names() {
    let proxy = start(DevConfig::default());
    let dir = tempfile::tempdir().unwrap();
    let mut p = params(1, &dir.path().join("kh"), "true");
    p.host = "x;touch pwned".into();
    p.proxy = Some(ssh_proxy(
        tc_ssh::proxy::ProxyKind::SshCommand(tc_ssh::proxy::DEFAULT_SSH_COMMAND.into()),
        proxy.port,
    ));
    let s = SshSession::connect(p, noop());
    let out = drive(&s, accept_and_login);
    assert!(
        out.errors.iter().any(|e| e.contains("cannot be passed")),
        "{out:?}"
    );
}

/// A hostile server that lets anyone in without asking anything (here:
/// keyboard-interactive with no questions after the password is refused).
#[test]
fn trivial_authentication_is_refused_when_asked() {
    let server = start(DevConfig {
        trivial_auth: true,
        ..DevConfig::default()
    });
    let dir = tempfile::tempdir().unwrap();
    let kh = dir.path().join("known_hosts");

    let s = SshSession::connect(params(server.port, &kh, "echo in"), noop());
    let out = drive(&s, accept_and_login);
    assert!(out.errors.is_empty(), "accepted by default: {out:?}");

    let mut p = params(server.port, &kh, "echo in");
    p.refuse_trivial_auth = true;
    let s = SshSession::connect(p, noop());
    let out = drive(&s, accept_and_login);
    assert!(
        out.errors.iter().any(|e| e.contains("without asking")),
        "{out:?}"
    );
    assert!(!out.output.contains("in\r\n"), "no command ran: {out:?}");
}

#[test]
fn expert_algorithm_lists_are_negotiated() {
    let server = start(DevConfig::default());
    let dir = tempfile::tempdir().unwrap();
    let known = dir.path().join("known_hosts");
    let mut p = params(server.port, &known, "echo algorithms-ok");
    p.algorithms = tc_ssh::algorithms::Algorithms {
        kex: vec!["dh-group16-sha512".into()],
        host_keys: vec!["Ed25519".into()],
        ciphers: vec!["AESGCM".into()],
        prefer_known: true,
    };
    let s = SshSession::connect(p, noop());
    let out = drive(&s, accept_and_login);
    assert!(out.errors.is_empty(), "{out:?}");
    assert!(out.output.contains("algorithms-ok"), "{out:?}");
}

/// A certificate that cannot be read is reported; the key is still used.
#[test]
fn unreadable_certificate_falls_back_to_the_key() {
    let key = PrivateKey::from(Ed25519Keypair::from_seed(&[43; 32]));
    let config = DevConfig {
        authorized_keys: vec![key.public_key().clone()],
        ..DevConfig::default()
    };
    let server = start(config);
    let dir = tempfile::tempdir().unwrap();
    let key_path = dir.path().join("id_ed25519");
    std::fs::write(
        &key_path,
        key.to_openssh(LineEnding::LF).unwrap().as_bytes(),
    )
    .unwrap();
    let cert = dir.path().join("id_ed25519-cert.pub");
    std::fs::write(&cert, "not a certificate").unwrap();

    let mut p = params(server.port, &dir.path().join("known_hosts"), "echo via-key");
    p.private_key = Some(key_path);
    p.certificate = Some(cert);
    let s = SshSession::connect(p, noop());
    let mut infos = Vec::new();
    let out = drive(&s, |e| {
        if let SshEvent::Info(i) = e {
            infos.push(i.clone());
        }
        accept_and_login(e)
    });
    assert!(out.output.contains("via-key"), "{out:?}");
    assert!(
        infos.iter().any(|i| i.contains("cannot load certificate")),
        "{infos:?}"
    );
}

/// "Use certificate with the private key": the server trusts the CA,
/// not the key itself.
#[test]
fn certificate_login() {
    use russh::keys::ssh_key::certificate::{Builder, CertType};
    let ca = PrivateKey::from(Ed25519Keypair::from_seed(&[7; 32]));
    let key = PrivateKey::from(Ed25519Keypair::from_seed(&[8; 32]));
    let config = DevConfig {
        user_ca: Some(ca.public_key().clone()),
        ..DevConfig::default()
    };
    let user = config.user.clone();
    let server = start(config);
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    let mut builder = Builder::new_with_random_nonce(
        &mut russh::keys::key::safe_rng(),
        key.public_key(),
        now - 60,
        now + 3600,
    )
    .unwrap();
    builder.cert_type(CertType::User).unwrap();
    builder.valid_principal(&user).unwrap();
    let cert = builder.sign(&ca).unwrap();

    let dir = tempfile::tempdir().unwrap();
    let key_path = dir.path().join("id_ed25519");
    std::fs::write(
        &key_path,
        key.to_openssh(LineEnding::LF).unwrap().as_bytes(),
    )
    .unwrap();
    let cert_path = dir.path().join("id_ed25519-cert.pub");
    std::fs::write(&cert_path, cert.to_openssh().unwrap()).unwrap();

    let mut p = params(
        server.port,
        &dir.path().join("known_hosts"),
        "echo via-cert",
    );
    p.private_key = Some(key_path);
    p.certificate = Some(cert_path);
    let s = SshSession::connect(p, noop());
    let out = drive(&s, accept_and_login);
    assert_eq!(out.secret_prompts, 0, "no password: {out:?}");
    assert!(out.output.contains("via-cert"), "{out:?}");
}
