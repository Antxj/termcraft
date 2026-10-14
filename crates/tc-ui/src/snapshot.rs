//! Development snapshots: the app renders a scripted state and saves its own
//! frame as PNG, without any OS-level input injection. Enabled only through
//! environment variables:
//!
//! - `TERMCRAFT_SNAPSHOT=out.png` — where to write the image (required)
//! - `TERMCRAFT_SNAPSHOT_SESSION=name` — saved session to open first
//! - `TERMCRAFT_SNAPSHOT_PASSWORD=…` — typed at the first password prompt
//!   (use only with the local dev server credentials)
//! - `TERMCRAFT_SNAPSHOT_DELAY_MS=6000` — time to let the scene settle
//! - `TERMCRAFT_SNAPSHOT_LOCAL_TABS=3` — also open N local terminals
//! - `TERMCRAFT_SNAPSHOT_WINDOW=config|passwords|credentials|master|master_settings|hotkeys|keygen|known_hosts|jump_hosts|syntax|expert|servers|editor|diff|new_session:<type>` — open that window
//! - `TERMCRAFT_SNAPSHOT_TYPE=text` — typed (with Enter) into the active
//!   tab two seconds after start
//! - `TERMCRAFT_SNAPSHOT_HOSTKEY=1` — leave host key questions open (to
//!   capture the host key dialog)
//! - `TERMCRAFT_SNAPSHOT_TUNNELS=1` — open the SSH tunnels window and start
//!   every saved tunnel (prompts are answered like for the session)

use crate::tab::Tab;
use crate::tunnels::RunningTunnel;
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::{Duration, Instant};
use tc_commands::Command;
use tc_core::TunnelId;

enum Phase {
    Start,
    Waiting,
    Requested,
}

pub struct Snapshot {
    out: PathBuf,
    session: Option<String>,
    password: Option<String>,
    /// Typed into the active tab once it had time to start.
    typed: Option<String>,
    delay: Duration,
    pub tunnels: bool,
    /// Window to open on the first frame (taken once).
    pub window: Option<String>,
    started: Instant,
    phase: Phase,
}

impl Snapshot {
    pub fn from_env() -> Option<Self> {
        let out = PathBuf::from(std::env::var_os("TERMCRAFT_SNAPSHOT")?);
        let delay = std::env::var("TERMCRAFT_SNAPSHOT_DELAY_MS")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(6000);
        Some(Self {
            out,
            session: std::env::var("TERMCRAFT_SNAPSHOT_SESSION").ok(),
            password: std::env::var("TERMCRAFT_SNAPSHOT_PASSWORD").ok(),
            typed: std::env::var("TERMCRAFT_SNAPSHOT_TYPE").ok(),
            delay: Duration::from_millis(delay),
            tunnels: std::env::var_os("TERMCRAFT_SNAPSHOT_TUNNELS").is_some(),
            window: std::env::var("TERMCRAFT_SNAPSHOT_WINDOW").ok(),
            started: Instant::now(),
            phase: Phase::Start,
        })
    }

    /// Advance the script by one frame. Returns a command to run, if any.
    pub fn step(
        &mut self,
        ctx: &egui::Context,
        active: Option<&mut Tab>,
        running: &mut BTreeMap<TunnelId, RunningTunnel>,
    ) -> Vec<Command> {
        ctx.request_repaint_after(Duration::from_millis(50));
        match self.phase {
            Phase::Start => {
                self.phase = Phase::Waiting;
                let locals: usize = std::env::var("TERMCRAFT_SNAPSHOT_LOCAL_TABS")
                    .ok()
                    .and_then(|v| v.parse().ok())
                    .unwrap_or(0);
                let mut cmds: Vec<Command> = (0..locals)
                    .map(|_| Command::TabOpen { session: None })
                    .collect();
                // The session last, so it ends up as the active tab.
                cmds.extend(
                    self.session
                        .clone()
                        .map(|s| Command::TabOpen { session: Some(s) }),
                );
                return cmds;
            }
            Phase::Waiting => {
                let answer_keys = std::env::var_os("TERMCRAFT_SNAPSHOT_HOSTKEY").is_none();
                if let Some(tab) = active {
                    if answer_keys && tab.host_key.is_some() {
                        tab.answer_host_key(true, false);
                    }
                    if tab.awaiting_secret()
                        && let Some(pw) = self.password.take()
                    {
                        tab.send(format!("{pw}\r").as_bytes());
                    }
                    if self.started.elapsed() >= Duration::from_secs(2)
                        && let Some(text) = self.typed.take()
                    {
                        tab.send(format!("{text}\r").as_bytes());
                    }
                    // Keep the save-password question out of the picture.
                    tab.offers.clear();
                }
                for t in running.values_mut() {
                    if answer_keys && t.host_key.is_some() {
                        t.answer_host_key(true, false);
                    }
                    if t.secret.is_some() {
                        let pw = std::env::var("TERMCRAFT_SNAPSHOT_PASSWORD").ok();
                        t.answer_secret(pw);
                    }
                    t.offers.clear();
                }
                if self.started.elapsed() >= self.delay {
                    ctx.send_viewport_cmd(egui::ViewportCommand::Screenshot(
                        egui::UserData::default(),
                    ));
                    self.phase = Phase::Requested;
                }
            }
            Phase::Requested => {
                let shot = ctx.input(|i| {
                    i.events.iter().find_map(|e| match e {
                        egui::Event::Screenshot { image, .. } => Some(image.clone()),
                        _ => None,
                    })
                });
                if let Some(image) = shot {
                    let [w, h] = image.size;
                    let result = image::save_buffer(
                        &self.out,
                        image.as_raw(),
                        w as u32,
                        h as u32,
                        image::ExtendedColorType::Rgba8,
                    );
                    match result {
                        Ok(()) => eprintln!("snapshot written to {}", self.out.display()),
                        Err(e) => eprintln!("snapshot failed: {e}"),
                    }
                    ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                }
            }
        }
        Vec::new()
    }
}
