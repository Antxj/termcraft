//! Spawn a local shell on a pseudo-terminal. Output arrives on a channel from
//! a reader thread; `on_output` lets the UI request a repaint.

use anyhow::Context as _;
use crossbeam_channel::{Receiver, Sender};
use portable_pty::{Child, CommandBuilder, MasterPty, PtySize, native_pty_system};
use std::io::{Read, Write};
use std::path::PathBuf;
use std::sync::Arc;

pub enum PtyEvent {
    Output(Vec<u8>),
    Exited,
}

pub struct PtySession {
    master: Box<dyn MasterPty + Send>,
    writer: Box<dyn Write + Send>,
    child: Box<dyn Child + Send + Sync>,
    events: Receiver<PtyEvent>,
}

impl PtySession {
    /// `program`: program to run; `None` = platform default shell.
    pub fn spawn(
        program: Option<&str>,
        args: &[&str],
        rows: u16,
        cols: u16,
        on_output: Arc<dyn Fn() + Send + Sync>,
    ) -> anyhow::Result<Self> {
        let pair = native_pty_system()
            .openpty(size(rows, cols))
            .context("opening pseudo-terminal")?;

        let program = program.map(String::from).unwrap_or_else(default_shell);
        let mut cmd = CommandBuilder::new(&program);
        cmd.args(args);
        cmd.env("TERM", "xterm-256color");
        cmd.env("COLORTERM", "truecolor");
        if let Some(home) = home_dir() {
            cmd.cwd(home);
        }

        let child = pair
            .slave
            .spawn_command(cmd)
            .with_context(|| format!("spawning {program}"))?;
        drop(pair.slave);

        let reader = pair.master.try_clone_reader()?;
        let writer = pair.master.take_writer()?;
        let (tx, events) = crossbeam_channel::unbounded();
        std::thread::Builder::new()
            .name("pty-reader".into())
            .spawn(move || read_loop(reader, tx, on_output))?;

        Ok(Self {
            master: pair.master,
            writer,
            child,
            events,
        })
    }

    pub fn write(&mut self, bytes: &[u8]) -> std::io::Result<()> {
        self.writer.write_all(bytes)?;
        self.writer.flush()
    }

    pub fn resize(&self, rows: u16, cols: u16) -> anyhow::Result<()> {
        self.master.resize(size(rows, cols))
    }

    pub fn events(&self) -> &Receiver<PtyEvent> {
        &self.events
    }

    pub fn kill(&mut self) {
        let _ = self.child.kill();
    }
}

impl Drop for PtySession {
    fn drop(&mut self) {
        self.kill();
    }
}

fn read_loop(
    mut reader: Box<dyn Read + Send>,
    tx: Sender<PtyEvent>,
    on_output: Arc<dyn Fn() + Send + Sync>,
) {
    let mut buf = [0u8; 8192];
    loop {
        match reader.read(&mut buf) {
            Ok(0) | Err(_) => break,
            Ok(n) => {
                if tx.send(PtyEvent::Output(buf[..n].to_vec())).is_err() {
                    return;
                }
                on_output();
            }
        }
    }
    let _ = tx.send(PtyEvent::Exited);
    on_output();
}

fn size(rows: u16, cols: u16) -> PtySize {
    PtySize {
        rows: rows.max(1),
        cols: cols.max(1),
        pixel_width: 0,
        pixel_height: 0,
    }
}

/// Windows: PowerShell 7 if installed, else Windows PowerShell.
/// Unix: `$SHELL`, else `/bin/sh`.
pub fn default_shell() -> String {
    if cfg!(windows) {
        if find_in_path("pwsh.exe").is_some() {
            "pwsh.exe".into()
        } else {
            "powershell.exe".into()
        }
    } else {
        std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".into())
    }
}

fn find_in_path(exe: &str) -> Option<PathBuf> {
    std::env::split_paths(&std::env::var_os("PATH")?)
        .map(|dir| dir.join(exe))
        .find(|p| p.is_file())
}

fn home_dir() -> Option<PathBuf> {
    std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" }).map(PathBuf::from)
}
