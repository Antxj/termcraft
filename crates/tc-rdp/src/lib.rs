//! Built-in RDP client (IronRDP): the connection with NLA (CredSSP), the
//! session loop that keeps an RGBA picture of the remote desktop, and the
//! keyboard / mouse input. No GUI here: the app draws [`Frame`] and sends
//! [`RdpInput`].
//!
//! TLS: the server certificate is checked against the web PKI roots; one
//! that does not verify (the usual self-signed RDP certificate) is only
//! accepted when its SHA-256 fingerprint is pinned, after the user saw it
//! ([`RdpEvent::UntrustedCertificate`]). TLS resumption is off (CredSSP
//! does not allow it) and no TLS key log is ever written.

mod clipboard;

use clipboard::{ClipEvent, TextClipboard};
use ironrdp_cliprdr::CliprdrClient;
use ironrdp_cliprdr::pdu::{ClipboardFormatId, OwnedFormatDataResponse};
use ironrdp_connector::connection_activation::ConnectionActivationState;
use ironrdp_connector::{ClientConnector, ConnectionResult, Credentials, DesktopSize};
use ironrdp_core::WriteBuf;
use ironrdp_displaycontrol::client::DisplayControlClient;
use ironrdp_dvc::DrdynvcClient;
use ironrdp_graphics::image_processing::PixelFormat;
use ironrdp_input::{Database, MouseButton, MousePosition, Operation, Scancode, WheelRotations};
use ironrdp_pdu::gcc::KeyboardType;
use ironrdp_pdu::rdp::capability_sets::MajorPlatformType;
use ironrdp_pdu::rdp::client_info::{PerformanceFlags, TimezoneInfo};
use ironrdp_session::image::DecodedImage;
use ironrdp_session::{ActiveStage, ActiveStageOutput, fast_path};
use ironrdp_tokio::{FramedWrite as _, single_sequence_step_read, split_tokio_framed};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::io::{AsyncRead, AsyncWrite};
use zeroize::Zeroizing;

pub const DEFAULT_PORT: u16 = 3389;
const CONNECT_TIMEOUT: Duration = Duration::from_secs(20);

/// What to connect to, and as whom.
#[derive(Clone)]
pub struct RdpParams {
    pub host: String,
    pub port: u16,
    pub user: String,
    pub domain: Option<String>,
    pub password: Zeroizing<String>,
    /// Requested desktop size (the server may choose another).
    pub width: u16,
    pub height: u16,
    /// Fingerprint (`SHA256:…`) of a certificate the user accepted for
    /// this server.
    pub pinned: Option<String>,
    /// Windows keyboard layout id (0 = the server's default).
    pub keyboard_layout: u32,
    /// Connect to this address instead (a tunnel to `host:port`); the
    /// certificate is still the one of `host`.
    pub via: Option<(String, u16)>,
    /// Share the clipboard (text).
    pub clipboard: bool,
}

/// What the session reports to the app.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RdpEvent {
    /// Logged in; the desktop has this size.
    Connected { width: u16, height: u16 },
    /// The picture changed ([`RdpSession::frame`]).
    Frame,
    /// The server certificate does not verify and is not pinned: ask the
    /// user, then connect again with `pinned` set to accept it.
    UntrustedCertificate { fingerprint: String, reason: String },
    /// The connection ended: `None` = the server closed the session.
    Closed(Option<String>),
    /// Text copied on the remote side (for the local clipboard).
    RemoteClipboard(String),
}

/// Mouse buttons.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Button {
    Left,
    Middle,
    Right,
}

/// Input for the remote desktop.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RdpInput {
    /// A key by PC/AT (set 1) scancode.
    Key {
        scancode: u16,
        down: bool,
    },
    /// A character the scancodes cannot express (typed text).
    Char {
        ch: char,
        down: bool,
    },
    MouseMove {
        x: u16,
        y: u16,
    },
    Button {
        button: Button,
        down: bool,
    },
    /// Wheel units (120 per notch); positive = up / right.
    Wheel {
        delta: i16,
        horizontal: bool,
    },
    /// Release every key and button (focus lost).
    ReleaseAll,
    /// Paste: offer this text as our clipboard, then press Ctrl+V on the
    /// remote side (without clipboard sharing, the text is typed).
    Paste(String),
    /// Ask the server for another desktop size (the window changed).
    Resize {
        width: u16,
        height: u16,
    },
    Close,
}

/// The remote desktop picture (RGBA, row-major).
#[derive(Debug, Default)]
pub struct Frame {
    pub width: u16,
    pub height: u16,
    pub rgba: Vec<u8>,
    /// Changes at each update (the app re-uploads its texture then).
    pub generation: u64,
}

/// A running session: events out, input in, the picture shared.
pub struct RdpSession {
    events: crossbeam_channel::Receiver<RdpEvent>,
    input: tokio::sync::mpsc::UnboundedSender<RdpInput>,
    frame: Arc<Mutex<Frame>>,
}

impl RdpSession {
    /// Connect in a background thread; `notify` is called after each
    /// event (to repaint the window).
    pub fn connect(params: RdpParams, notify: Arc<dyn Fn() + Send + Sync>) -> Self {
        let (events_tx, events) = crossbeam_channel::unbounded();
        let (input, input_rx) = tokio::sync::mpsc::unbounded_channel();
        let frame = Arc::new(Mutex::new(Frame::default()));
        let shared = frame.clone();
        let spawned = std::thread::Builder::new()
            .name(format!("rdp {}", params.host))
            .spawn(move || {
                let send = |e: RdpEvent| {
                    let _ = events_tx.send(e);
                    notify();
                };
                let runtime = match tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                {
                    Ok(r) => r,
                    Err(e) => return send(RdpEvent::Closed(Some(e.to_string()))),
                };
                let end = runtime.block_on(run(params, input_rx, &shared, &send));
                send(RdpEvent::Closed(end.err()));
            });
        if let Err(e) = spawned {
            // No thread: report it as the end of the session.
            let (tx, rx) = crossbeam_channel::unbounded();
            let _ = tx.send(RdpEvent::Closed(Some(e.to_string())));
            return Self {
                events: rx,
                input,
                frame,
            };
        }
        Self {
            events,
            input,
            frame,
        }
    }

    pub fn events(&self) -> &crossbeam_channel::Receiver<RdpEvent> {
        &self.events
    }

    pub fn send(&self, input: RdpInput) {
        let _ = self.input.send(input);
    }

    /// The current picture.
    pub fn frame(&self) -> std::sync::MutexGuard<'_, Frame> {
        self.frame
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

impl Drop for RdpSession {
    fn drop(&mut self) {
        let _ = self.input.send(RdpInput::Close);
    }
}

fn platform() -> MajorPlatformType {
    if cfg!(windows) {
        MajorPlatformType::WINDOWS
    } else if cfg!(target_os = "macos") {
        MajorPlatformType::MACINTOSH
    } else {
        MajorPlatformType::UNIX
    }
}

fn connector_config(p: &RdpParams) -> ironrdp_connector::Config {
    ironrdp_connector::Config {
        desktop_size: DesktopSize {
            width: p.width.clamp(200, 8192),
            height: p.height.clamp(200, 8192),
        },
        desktop_scale_factor: 0,
        // NLA first; the graphical login (TLS only) when the server has no NLA.
        enable_tls: true,
        enable_credssp: true,
        credentials: Credentials::UsernamePassword {
            username: p.user.clone(),
            password: p.password.to_string(),
        },
        domain: p.domain.clone().filter(|d| !d.is_empty()),
        client_build: 0,
        client_name: "TermCraft".to_owned(),
        keyboard_type: KeyboardType::IbmEnhanced,
        keyboard_subtype: 0,
        keyboard_functional_keys_count: 12,
        keyboard_layout: p.keyboard_layout,
        ime_file_name: String::new(),
        bitmap: None,
        dig_product_id: String::new(),
        client_dir: "C:\\Windows\\System32\\mstscax.dll".to_owned(),
        platform: platform(),
        hardware_id: None,
        request_data: None,
        autologon: false,
        enable_audio_playback: false,
        performance_flags: PerformanceFlags::default(),
        license_cache: None,
        timezone_info: TimezoneInfo::default(),
        enable_server_pointer: false,
        pointer_software_rendering: true,
    }
}

/// The TLS client: web PKI, else the pinned fingerprint; no resumption.
#[allow(clippy::type_complexity)]
fn tls_config(
    pinned: Option<String>,
) -> Result<
    (
        Arc<rustls::ClientConfig>,
        Arc<Mutex<Option<(String, String)>>>,
    ),
    String,
> {
    let versions = tc_net::ftps::TlsVersions {
        tls12: true,
        tls13: true,
    };
    let (config, rejected) = tc_net::ftps::client_config(versions, pinned)?;
    let mut config = (*config).clone();
    config.resumption = rustls::client::Resumption::disabled();
    Ok((Arc::new(config), rejected))
}

/// The public key CredSSP binds to (subjectPublicKey of the certificate).
fn server_public_key(der: &[u8]) -> Result<Vec<u8>, String> {
    use x509_cert::der::Decode as _;
    let cert = x509_cert::Certificate::from_der(der).map_err(|e| e.to_string())?;
    cert.tbs_certificate
        .subject_public_key_info
        .subject_public_key
        .as_bytes()
        .map(<[u8]>::to_vec)
        .ok_or_else(|| "unaligned certificate public key".to_string())
}

trait Stream: AsyncRead + AsyncWrite + Unpin + Send + Sync {}
impl<T: AsyncRead + AsyncWrite + Unpin + Send + Sync> Stream for T {}
type Upgraded = ironrdp_tokio::TokioFramed<Box<dyn Stream>>;

async fn connect(
    p: &RdpParams,
    clipboard: Option<tokio::sync::mpsc::UnboundedSender<ClipEvent>>,
    send: &dyn Fn(RdpEvent),
) -> Result<Option<(ConnectionResult, Upgraded)>, String> {
    let (address, port) = p
        .via
        .as_ref()
        .map_or((p.host.as_str(), p.port), |(h, port)| (h.as_str(), *port));
    let tcp = tokio::time::timeout(
        CONNECT_TIMEOUT,
        tokio::net::TcpStream::connect((address, port)),
    )
    .await
    .map_err(|_| format!("{}:{}: no answer", p.host, p.port))?
    .map_err(|e| format!("{}:{}: {e}", p.host, p.port))?;
    let _ = tcp.set_nodelay(true);
    let client_addr = tcp.local_addr().map_err(|e| e.to_string())?;
    let mut framed = ironrdp_tokio::TokioFramed::new(tcp);
    let mut connector = ClientConnector::new(connector_config(p), client_addr).with_static_channel(
        // Display Control: the desktop follows the window size.
        DrdynvcClient::new().with_dynamic_channel(DisplayControlClient::new(|_| Ok(Vec::new()))),
    );
    if let Some(events) = clipboard {
        connector =
            connector.with_static_channel(CliprdrClient::new(Box::new(TextClipboard { events })));
    }
    let should_upgrade = ironrdp_tokio::connect_begin(&mut framed, &mut connector)
        .await
        .map_err(|e| format!("connection: {e}"))?;

    let (tcp, leftover) = framed.into_inner();
    let (tls, rejected) = tls_config(p.pinned.clone())?;
    let name = rustls::pki_types::ServerName::try_from(p.host.clone())
        .map_err(|e| format!("{}: {e}", p.host))?;
    let stream = match tokio_rustls::TlsConnector::from(tls)
        .connect(name, tcp)
        .await
    {
        Ok(s) => s,
        Err(e) => {
            let question = rejected.lock().ok().and_then(|mut r| r.take());
            return match question {
                Some((fingerprint, reason)) => {
                    send(RdpEvent::UntrustedCertificate {
                        fingerprint,
                        reason,
                    });
                    Ok(None)
                }
                None => Err(format!("TLS: {e}")),
            };
        }
    };
    let der = stream
        .get_ref()
        .1
        .peer_certificates()
        .and_then(|c| c.first())
        .map(|c| c.as_ref().to_vec())
        .ok_or("the server sent no certificate")?;
    let public_key = server_public_key(&der)?;
    let upgraded = ironrdp_tokio::mark_as_upgraded(should_upgrade, &mut connector);
    let boxed: Box<dyn Stream> = Box::new(stream);
    let mut framed = ironrdp_tokio::TokioFramed::new_with_leftover(boxed, leftover);
    let result = ironrdp_tokio::connect_finalize(
        upgraded,
        &mut framed,
        connector,
        p.host.clone().into(),
        public_key,
        None,
        None,
    )
    .await
    .map_err(|e| login_error(&e.to_string()))?;
    Ok(Some((result, framed)))
}

/// A connector error in words for the user.
fn login_error(e: &str) -> String {
    let lower = e.to_lowercase();
    if lower.contains("logon") || lower.contains("credssp") || lower.contains("authentication") {
        format!("login failed (user name or password?): {e}")
    } else {
        format!("connection: {e}")
    }
}

/// Copy the decoded picture into the shared frame.
fn publish(image: &DecodedImage, frame: &Mutex<Frame>) {
    let mut f = frame
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    f.width = image.width();
    f.height = image.height();
    f.rgba.clear();
    f.rgba.extend_from_slice(image.data());
    // The decoder leaves alpha at 0: an opaque desktop.
    for px in f.rgba.as_chunks_mut::<4>().0 {
        px[3] = 0xFF;
    }
    f.generation = f.generation.wrapping_add(1);
}

/// Our input as IronRDP operations.
fn operations(input: &RdpInput) -> Vec<Operation> {
    let button = |b: Button| match b {
        Button::Left => MouseButton::Left,
        Button::Middle => MouseButton::Middle,
        Button::Right => MouseButton::Right,
    };
    match *input {
        RdpInput::Key { scancode, down } => {
            let code = Scancode::from_u16(scancode);
            vec![if down {
                Operation::KeyPressed(code)
            } else {
                Operation::KeyReleased(code)
            }]
        }
        RdpInput::Char { ch, down } => vec![if down {
            Operation::UnicodeKeyPressed(ch)
        } else {
            Operation::UnicodeKeyReleased(ch)
        }],
        RdpInput::MouseMove { x, y } => vec![Operation::MouseMove(MousePosition { x, y })],
        RdpInput::Button { button: b, down } => vec![if down {
            Operation::MouseButtonPressed(button(b))
        } else {
            Operation::MouseButtonReleased(button(b))
        }],
        RdpInput::Wheel { delta, horizontal } => vec![Operation::WheelRotations(WheelRotations {
            is_vertical: !horizontal,
            rotation_units: delta,
        })],
        RdpInput::ReleaseAll | RdpInput::Close | RdpInput::Paste(_) | RdpInput::Resize { .. } => {
            Vec::new()
        }
    }
}

async fn run(
    params: RdpParams,
    mut input: tokio::sync::mpsc::UnboundedReceiver<RdpInput>,
    frame: &Mutex<Frame>,
    send: &dyn Fn(RdpEvent),
) -> Result<(), String> {
    let (clip_tx, mut clip_rx) = tokio::sync::mpsc::unbounded_channel();
    let sharing = params.clipboard;
    let Some((result, framed)) = connect(&params, sharing.then_some(clip_tx), send).await? else {
        // The certificate question went out; the app connects again.
        return Ok(());
    };
    drop(params);
    let size = result.desktop_size;
    send(RdpEvent::Connected {
        width: size.width,
        height: size.height,
    });
    let (mut reader, mut writer) = split_tokio_framed(framed);
    let mut image = DecodedImage::new(PixelFormat::RgbA32, size.width, size.height);
    let mut stage = ActiveStage::new(result);
    let mut database = Database::new();
    let io = |e: std::io::Error| e.to_string();
    // Our clipboard text, offered to the server on paste.
    let mut local_text: Option<String> = None;
    // Ctrl+V for the remote side, sent shortly after the offer.
    let mut paste_keys: Option<tokio::time::Instant> = None;
    let err = |e: ironrdp_session::SessionError| e.to_string();
    loop {
        let outputs = tokio::select! {
            pdu = reader.read_pdu() => {
                let (action, payload) = pdu.map_err(io)?;
                stage.process(&mut image, action, &payload).map_err(err)?
            }
            () = async {
                match paste_keys {
                    Some(at) => tokio::time::sleep_until(at).await,
                    None => std::future::pending().await,
                }
            } => {
                paste_keys = None;
                let ctrl_held = database.is_key_pressed(Scancode::from_u8(false, 0x1D));
                let mut ops = Vec::new();
                if !ctrl_held {
                    ops.push(Operation::KeyPressed(Scancode::from_u8(false, 0x1D)));
                }
                ops.push(Operation::KeyPressed(Scancode::from_u8(false, 0x2F)));
                ops.push(Operation::KeyReleased(Scancode::from_u8(false, 0x2F)));
                if !ctrl_held {
                    ops.push(Operation::KeyReleased(Scancode::from_u8(false, 0x1D)));
                }
                let events = database.apply(ops);
                stage.process_fastpath_input(&mut image, &events).map_err(err)?
            }
            clip = clip_rx.recv() => {
                let Some(clip) = clip else { continue };
                let Some(cliprdr) = stage.get_svc_processor::<CliprdrClient>() else { continue };
                let messages = match clip {
                    ClipEvent::Ready => cliprdr.initiate_copy(&clipboard::offered(local_text.as_deref())),
                    ClipEvent::RemoteText => cliprdr.initiate_paste(ClipboardFormatId::CF_UNICODETEXT),
                    ClipEvent::DataRequest => cliprdr.submit_format_data(
                        OwnedFormatDataResponse::new_unicode_string(local_text.as_deref().unwrap_or("")),
                    ),
                    ClipEvent::Text(text) => {
                        send(RdpEvent::RemoteClipboard(text));
                        continue;
                    }
                };
                let messages = messages.map_err(|e| e.to_string())?;
                let bytes = stage.process_svc_processor_messages(messages).map_err(err)?;
                vec![ActiveStageOutput::ResponseFrame(bytes)]
            }
            event = input.recv() => {
                let Some(event) = event else { break };
                match event {
                    RdpInput::Close => stage.graceful_shutdown().map_err(err)?,
                    RdpInput::Paste(text) if sharing => {
                        // Offer the text, then paste it on the remote side.
                        local_text = Some(text);
                        paste_keys = Some(tokio::time::Instant::now() + Duration::from_millis(150));
                        match stage.get_svc_processor::<CliprdrClient>() {
                            Some(cliprdr) => {
                                let messages = cliprdr
                                    .initiate_copy(&clipboard::offered(local_text.as_deref()))
                                    .map_err(|e| e.to_string())?;
                                vec![ActiveStageOutput::ResponseFrame(
                                    stage.process_svc_processor_messages(messages).map_err(err)?,
                                )]
                            }
                            None => Vec::new(),
                        }
                    }
                    RdpInput::Paste(text) => {
                        // No clipboard channel: type it.
                        let mut events = Vec::new();
                        let ctrl = Scancode::from_u8(false, 0x1D);
                        let ctrl_held = database.is_key_pressed(ctrl);
                        if ctrl_held {
                            events.extend(database.apply([Operation::KeyReleased(ctrl)]));
                        }
                        for ch in text.chars().filter(|c| *c != '\r') {
                            let enter = Scancode::from_u8(false, 0x1C);
                            let ops = if ch == '\n' {
                                [Operation::KeyPressed(enter), Operation::KeyReleased(enter)]
                            } else {
                                [Operation::UnicodeKeyPressed(ch), Operation::UnicodeKeyReleased(ch)]
                            };
                            events.extend(database.apply(ops));
                        }
                        if ctrl_held {
                            events.extend(database.apply([Operation::KeyPressed(ctrl)]));
                        }
                        stage.process_fastpath_input(&mut image, &events).map_err(err)?
                    }
                    RdpInput::Resize { width, height } => {
                        // Even widths, 200 … 8192 (MS-RDPEDISP).
                        let w = u32::from(width.clamp(200, 8192)) & !1;
                        let h = u32::from(height.clamp(200, 8192));
                        match stage.encode_resize(w, h, None, None) {
                            Some(bytes) => vec![ActiveStageOutput::ResponseFrame(bytes.map_err(err)?)],
                            None => Vec::new(),
                        }
                    }
                    RdpInput::ReleaseAll => {
                        let events = database.release_all();
                        stage.process_fastpath_input(&mut image, &events).map_err(err)?
                    }
                    other => {
                        let events = database.apply(operations(&other));
                        stage.process_fastpath_input(&mut image, &events).map_err(err)?
                    }
                }
            }
        };
        let mut changed = false;
        for out in outputs {
            match out {
                ActiveStageOutput::ResponseFrame(bytes) => {
                    writer.write_all(&bytes).await.map_err(io)?;
                }
                ActiveStageOutput::GraphicsUpdate(_) => changed = true,
                ActiveStageOutput::Terminate(_) => {
                    if changed {
                        publish(&image, frame);
                    }
                    return Ok(());
                }
                ActiveStageOutput::DeactivateAll(mut activation) => {
                    // Deactivation-reactivation: the server may change the size.
                    let mut buf = WriteBuf::new();
                    loop {
                        let written =
                            single_sequence_step_read(&mut reader, &mut *activation, &mut buf)
                                .await
                                .map_err(|e| e.to_string())?;
                        if written.size().is_some() {
                            writer.write_all(buf.filled()).await.map_err(io)?;
                        }
                        if let ConnectionActivationState::Finalized {
                            io_channel_id,
                            user_channel_id,
                            desktop_size,
                            enable_server_pointer,
                            pointer_software_rendering,
                        } = activation.state
                        {
                            image = DecodedImage::new(
                                PixelFormat::RgbA32,
                                desktop_size.width,
                                desktop_size.height,
                            );
                            stage.set_fastpath_processor(
                                fast_path::ProcessorBuilder {
                                    io_channel_id,
                                    user_channel_id,
                                    enable_server_pointer,
                                    pointer_software_rendering,
                                }
                                .build(),
                            );
                            stage.set_enable_server_pointer(enable_server_pointer);
                            send(RdpEvent::Connected {
                                width: desktop_size.width,
                                height: desktop_size.height,
                            });
                            break;
                        }
                    }
                }
                // Software pointer: drawn into the picture.
                _ => {}
            }
        }
        if changed {
            publish(&image, frame);
            send(RdpEvent::Frame);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn input_becomes_operations() {
        let ops = operations(&RdpInput::Key {
            scancode: 0xE01D,
            down: true,
        });
        assert!(matches!(&ops[..], [Operation::KeyPressed(s)] if s.as_u8() == (true, 0x1D)));
        assert!(operations(&RdpInput::Close).is_empty());
        assert!(matches!(
            &operations(&RdpInput::Wheel { delta: 120, horizontal: false })[..],
            [Operation::WheelRotations(w)] if w.is_vertical && w.rotation_units == 120
        ));
    }
}
