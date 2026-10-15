//! Loopback RDP test server (IronRDP): one solid-color desktop, NLA
//! (CredSSP) with the dev user, a self-signed certificate made at start,
//! and a log of the keyboard and mouse input it receives. For tc-rdp's
//! integration tests only.

use core::num::{NonZeroU16, NonZeroUsize};
use ironrdp_server::{
    BitmapUpdate, Credentials, DesktopSize, DisplayUpdate, KeyboardEvent, MouseEvent, PixelFormat,
    RdpServer, RdpServerDisplay, RdpServerDisplayUpdates, RdpServerInputHandler,
};
use std::sync::{Arc, Mutex};

/// What the test server shows and accepts.
#[derive(Debug, Clone)]
pub struct RdpDevConfig {
    pub user: String,
    pub password: String,
    pub width: u16,
    pub height: u16,
    /// The desktop color (RGB).
    pub color: [u8; 3],
}

impl Default for RdpDevConfig {
    fn default() -> Self {
        Self {
            user: crate::DEV_USER.into(),
            password: crate::DEV_PASSWORD.into(),
            width: 320,
            height: 200,
            color: [0x20, 0x80, 0xC0],
        }
    }
}

/// A running test server.
pub struct RdpDevServer {
    pub port: u16,
    /// DER of its certificate (to check what the client pins).
    pub certificate: Vec<u8>,
    /// Input received, one line each (`key 30 down`, `mouse 10,20` …).
    pub input: Arc<Mutex<Vec<String>>>,
}

#[derive(Clone)]
struct Handler {
    config: RdpDevConfig,
    input: Arc<Mutex<Vec<String>>>,
}

impl RdpServerInputHandler for Handler {
    fn keyboard(&mut self, event: KeyboardEvent) {
        let line = match event {
            KeyboardEvent::Pressed { code, extended } => {
                format!("key {code}{} down", if extended { "e" } else { "" })
            }
            KeyboardEvent::Released { code, extended } => {
                format!("key {code}{} up", if extended { "e" } else { "" })
            }
            KeyboardEvent::UnicodePressed(c) => format!("unicode {c} down"),
            KeyboardEvent::UnicodeReleased(c) => format!("unicode {c} up"),
            KeyboardEvent::Synchronize(_) => return,
        };
        self.input.lock().expect("lock").push(line);
    }

    fn mouse(&mut self, event: MouseEvent) {
        let line = match event {
            MouseEvent::Move { x, y } => format!("mouse {x},{y}"),
            MouseEvent::LeftPressed => "left down".into(),
            MouseEvent::LeftReleased => "left up".into(),
            MouseEvent::RightPressed => "right down".into(),
            MouseEvent::RightReleased => "right up".into(),
            MouseEvent::VerticalScroll { value } => format!("wheel {value}"),
            _ => return,
        };
        self.input.lock().expect("lock").push(line);
    }
}

struct Updates {
    config: RdpDevConfig,
    sent: bool,
}

#[async_trait::async_trait]
impl RdpServerDisplayUpdates for Updates {
    async fn next_update(&mut self) -> anyhow::Result<Option<DisplayUpdate>> {
        if self.sent {
            // One picture, then nothing changes.
            std::future::pending::<()>().await;
        }
        self.sent = true;
        let (w, h) = (self.config.width, self.config.height);
        let [r, g, b] = self.config.color;
        let pixels = usize::from(w) * usize::from(h);
        let mut data = Vec::with_capacity(pixels * 4);
        for _ in 0..pixels {
            data.extend_from_slice(&[b, g, r, 0xFF]);
        }
        Ok(Some(DisplayUpdate::Bitmap(BitmapUpdate {
            x: 0,
            y: 0,
            width: NonZeroU16::new(w).expect("width"),
            height: NonZeroU16::new(h).expect("height"),
            format: PixelFormat::BgrA32,
            data: data.into(),
            stride: NonZeroUsize::new(usize::from(w) * 4).expect("stride"),
        })))
    }
}

#[async_trait::async_trait]
impl RdpServerDisplay for Handler {
    async fn size(&mut self) -> DesktopSize {
        DesktopSize {
            width: self.config.width,
            height: self.config.height,
        }
    }

    async fn updates(&mut self) -> anyhow::Result<Box<dyn RdpServerDisplayUpdates>> {
        Ok(Box::new(Updates {
            config: self.config.clone(),
            sent: false,
        }))
    }
}

/// Start the server on 127.0.0.1 (a free port) in its own thread.
pub fn start(config: RdpDevConfig) -> anyhow::Result<RdpDevServer> {
    let key = rcgen::KeyPair::generate()?;
    let cert =
        rcgen::CertificateParams::new(vec!["localhost".to_string(), "127.0.0.1".to_string()])?
            .self_signed(&key)?;
    let cert_der = cert.der().to_vec();
    let pub_key = {
        use x509_cert::der::Decode as _;
        let parsed = x509_cert::Certificate::from_der(&cert_der)?;
        parsed
            .tbs_certificate
            .subject_public_key_info
            .subject_public_key
            .as_bytes()
            .ok_or_else(|| anyhow::anyhow!("unaligned public key"))?
            .to_vec()
    };
    let tls = rustls::ServerConfig::builder_with_provider(Arc::new(
        rustls::crypto::ring::default_provider(),
    ))
    .with_safe_default_protocol_versions()?
    .with_no_client_auth()
    .with_single_cert(
        vec![rustls::pki_types::CertificateDer::from(cert_der.clone())],
        rustls::pki_types::PrivateKeyDer::Pkcs8(key.serialize_der().into()),
    )?;
    let acceptor = ironrdp_server::tokio_rustls::TlsAcceptor::from(Arc::new(tls));

    let port = std::net::TcpListener::bind("127.0.0.1:0")?
        .local_addr()?
        .port();
    let input = Arc::new(Mutex::new(Vec::new()));
    let handler = Handler {
        config: config.clone(),
        input: input.clone(),
    };
    let credentials = Credentials {
        username: config.user.clone(),
        password: config.password.clone(),
        domain: None,
    };
    std::thread::Builder::new()
        .name("rdp-devserver".into())
        .spawn(move || {
            let runtime = tokio::runtime::Builder::new_multi_thread()
                .worker_threads(2)
                .enable_all()
                .build()
                .expect("runtime");
            runtime.block_on(async move {
                let mut server = RdpServer::builder()
                    .with_addr(([127, 0, 0, 1], port))
                    .with_hybrid(acceptor, pub_key)
                    .with_input_handler(handler.clone())
                    .with_display_handler(handler)
                    .build();
                server.set_credentials(Some(credentials));
                let _ = server.run().await;
            });
        })?;
    // Wait until it listens.
    for _ in 0..100 {
        if std::net::TcpStream::connect(("127.0.0.1", port)).is_ok() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    Ok(RdpDevServer {
        port,
        certificate: cert_der,
        input,
    })
}
