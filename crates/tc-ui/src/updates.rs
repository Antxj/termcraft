//! Background update check/install so the UI thread never blocks on network.

use crossbeam_channel::{Receiver, Sender};
use tc_update::{Available, Updater};

pub enum UpdateState {
    Idle,
    Checking,
    UpToDate,
    Available(Available),
    Installing,
    Installed(String),
    /// Updates disabled (dev build) or failed; message shown in the status bar.
    Error(String),
}

pub struct Updates {
    pub state: UpdateState,
    tx: Sender<UpdateState>,
    rx: Receiver<UpdateState>,
    channel: &'static str,
}

impl Updates {
    pub fn new(channel: &'static str) -> Self {
        let (tx, rx) = crossbeam_channel::unbounded();
        Self {
            state: UpdateState::Idle,
            tx,
            rx,
            channel,
        }
    }

    pub fn poll(&mut self) {
        while let Ok(state) = self.rx.try_recv() {
            self.state = state;
        }
    }

    pub fn check(&mut self, ctx: &egui::Context) {
        self.state = UpdateState::Checking;
        let (tx, ctx, channel) = (self.tx.clone(), ctx.clone(), self.channel);
        std::thread::spawn(move || {
            let state = match Updater::from_build(channel)
                .and_then(|u| u.check(env!("CARGO_PKG_VERSION")))
            {
                Ok(Some(update)) => UpdateState::Available(update),
                Ok(None) => UpdateState::UpToDate,
                Err(e) => UpdateState::Error(e.to_string()),
            };
            let _ = tx.send(state);
            ctx.request_repaint();
        });
    }

    pub fn install(&mut self, ctx: &egui::Context, update: Available) {
        self.state = UpdateState::Installing;
        let (tx, ctx, channel) = (self.tx.clone(), ctx.clone(), self.channel);
        std::thread::spawn(move || {
            let state = match Updater::from_build(channel).and_then(|u| u.install(&update)) {
                Ok(()) => UpdateState::Installed(update.version),
                Err(e) => UpdateState::Error(e.to_string()),
            };
            let _ = tx.send(state);
            ctx.request_repaint();
        });
    }
}
