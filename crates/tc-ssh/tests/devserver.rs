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
        known_hosts: known_hosts.to_path_buf(),
        compression: false,
        command: Some(command.into()),
        term: "xterm-256color".into(),
        cols: 80,
        rows: 24,
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
            SshEvent::Info(_) | SshEvent::Connected => {}
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
