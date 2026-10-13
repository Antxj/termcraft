//! End-to-end tests against the in-process development server.

use russh::keys::PrivateKey;
use russh::keys::ssh_key::LineEnding;
use russh::keys::ssh_key::private::Ed25519Keypair;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;
use tc_devserver::{DEV_PASSWORD, DEV_USER, DevConfig, bind};
use tc_ssh::{ConnectParams, Reply, SshEvent, SshSession};

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
        gateway: None,
        known_hosts: known_hosts.to_path_buf(),
        compression: false,
        command: Some(command.into()),
        term: "xterm-256color".into(),
        cols: 80,
        rows: 24,
        sftp: false,
        keepalive: None,
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
            SshEvent::UnknownHostKey { .. } => out.host_key_prompts += 1,
            SshEvent::SecretNeeded { .. } => out.secret_prompts += 1,
            SshEvent::Error(e) => out.errors.push(e.clone()),
            SshEvent::Closed { exit_status } => {
                out.exit_status = *exit_status;
                return out;
            }
            SshEvent::Info(_)
            | SshEvent::Connected
            | SshEvent::Sftp(_)
            | SshEvent::Listening(_) => {}
        }
        if let Some(reply) = answer(&event) {
            session.reply(reply);
        }
    }
}

fn accept_and_login(event: &SshEvent) -> Option<Reply> {
    match event {
        SshEvent::UnknownHostKey { .. } => Some(Reply::HostKey {
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
        SshEvent::UnknownHostKey { .. } => Some(Reply::HostKey {
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

#[test]
fn changed_host_key_is_refused() {
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
    let out = drive(&s, accept_and_login);
    assert_eq!(out.host_key_prompts, 0, "{out:?}");
    assert!(out.errors.iter().any(|e| e.contains("CHANGED")), "{out:?}");
    assert!(!out.output.contains("should-not-run"));
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

    let done = |s: &SshSession, id: u64| {
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
    });
    done(&s, id);
    assert_eq!(std::fs::read(&dl).unwrap(), b"hello over sftp");

    // upload a file larger than one chunk
    let big: Vec<u8> = (0..200_000u32).map(|i| (i % 251) as u8).collect();
    let up = local.path().join("up.bin");
    std::fs::write(&up, &big).unwrap();
    let id = s.sftp(SftpRequest::Upload {
        local: up,
        remote: "/up.bin".into(),
    });
    done(&s, id);
    assert_eq!(std::fs::read(root.path().join("up.bin")).unwrap(), big);

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
            ssh: endpoint(server.port),
            gateway: None,
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
    p.gateway = Some(endpoint(gateway.port));
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
