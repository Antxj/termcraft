//! The built-in RDP client against the loopback test server (NLA).

// The test server's dependencies export symbols from this test program
// (MSVC then reports the .lib / .exp it writes); TermCraft itself does not.
#![allow(linker_messages)]

use std::sync::Arc;
use std::time::{Duration, Instant};
use tc_devserver::rdp::{RdpDevConfig, start};
use tc_rdp::{RdpEvent, RdpInput, RdpParams, RdpSession};

fn params(port: u16, password: &str, pinned: Option<String>) -> RdpParams {
    RdpParams {
        host: "127.0.0.1".into(),
        port,
        user: tc_devserver::DEV_USER.into(),
        domain: None,
        password: password.to_string().into(),
        width: 320,
        height: 200,
        pinned,
        keyboard_layout: 0,
        via: None,
        clipboard: true,
        enhanced_graphics: false,
    }
}

fn next(s: &RdpSession) -> RdpEvent {
    s.events()
        .recv_timeout(Duration::from_secs(30))
        .expect("rdp event timeout")
}

#[test]
fn certificate_question_then_login_picture_and_input() {
    let config = RdpDevConfig::default();
    let color = config.color;
    let server = start(config).unwrap();

    // Self-signed: the user is asked first.
    let s = RdpSession::connect(
        params(server.port, tc_devserver::DEV_PASSWORD, None),
        Arc::new(|| {}),
    );
    let RdpEvent::UntrustedCertificate { fingerprint, .. } = next(&s) else {
        panic!("expected the certificate question");
    };
    let expected = tc_net::ftps::fingerprint(&rustls::pki_types::CertificateDer::from(
        server.certificate.clone(),
    ));
    assert_eq!(fingerprint, expected);

    // Accepted: NLA login, then the desktop.
    let s = RdpSession::connect(
        params(server.port, tc_devserver::DEV_PASSWORD, Some(fingerprint)),
        Arc::new(|| {}),
    );
    assert_eq!(
        next(&s),
        RdpEvent::Connected {
            width: 320,
            height: 200
        }
    );
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        assert!(Instant::now() < deadline, "no picture");
        if let Ok(RdpEvent::Frame) = s.events().recv_timeout(Duration::from_secs(1)) {
            let f = s.frame();
            let mid =
                (usize::from(f.height) / 2 * usize::from(f.width) + usize::from(f.width) / 2) * 4;
            // Lossless pictures (RemoteFX is not offered): exact colours.
            let close = f.rgba.get(mid..mid + 3).is_some_and(|px| px == color);
            if close && f.rgba[mid + 3] == 0xFF {
                break;
            }
        }
    }

    // Keyboard and mouse reach the server.
    s.send(RdpInput::Key {
        scancode: 0x1E,
        down: true,
    });
    s.send(RdpInput::Key {
        scancode: 0x1E,
        down: false,
    });
    s.send(RdpInput::MouseMove { x: 10, y: 20 });
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let got = server.input.lock().unwrap().clone();
        if got.contains(&"key 30 down".to_string()) && got.contains(&"mouse 10,20".to_string()) {
            break;
        }
        assert!(Instant::now() < deadline, "input not received: {got:?}");
        std::thread::sleep(Duration::from_millis(50));
    }
}

#[test]
fn wrong_password_is_refused() {
    let server = start(RdpDevConfig::default()).unwrap();
    let pinned = tc_net::ftps::fingerprint(&rustls::pki_types::CertificateDer::from(
        server.certificate.clone(),
    ));
    let s = RdpSession::connect(params(server.port, "wrong", Some(pinned)), Arc::new(|| {}));
    match next(&s) {
        RdpEvent::Closed(Some(e)) => assert!(!e.is_empty()),
        other => panic!("expected a refusal, got {other:?}"),
    }
}

fn wait_for_line(server: &tc_devserver::rdp::RdpDevServer, line: &str) {
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        let got = server.input.lock().unwrap().clone();
        if got.iter().any(|l| l == line) {
            return;
        }
        assert!(Instant::now() < deadline, "{line:?} not received: {got:?}");
        std::thread::sleep(Duration::from_millis(50));
    }
}

#[test]
fn paste_without_sharing_and_resize() {
    let server = start(RdpDevConfig::default()).unwrap();
    let pinned = tc_net::ftps::fingerprint(&rustls::pki_types::CertificateDer::from(
        server.certificate.clone(),
    ));
    let mut p = params(server.port, tc_devserver::DEV_PASSWORD, Some(pinned));
    p.clipboard = false;
    let s = RdpSession::connect(p, Arc::new(|| {}));
    assert!(matches!(next(&s), RdpEvent::Connected { .. }));

    // Without clipboard sharing a paste is typed.
    s.send(RdpInput::Paste(
        "ok
"
        .into(),
    ));
    wait_for_line(&server, "unicode 111 down");
    wait_for_line(&server, "unicode 107 down");
    wait_for_line(&server, "key 28 down");

    // The window changed: the server is asked for the new size.
    s.send(RdpInput::Resize {
        width: 641,
        height: 480,
    });
    wait_for_line(&server, "layout 640x480");
}
