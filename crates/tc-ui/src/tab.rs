use std::sync::Arc;
use tc_core::{Session, SessionKind};
use tc_pty::{PtyEvent, PtySession};
use tc_term::Terminal;

pub struct Tab {
    pub id: u64,
    pub title: String,
    /// Session this tab was opened from (`None` = default local shell).
    pub session: Option<Session>,
    pub term: Terminal,
    pty: Option<PtySession>,
    pub status: Option<String>,
}

const INITIAL_ROWS: u16 = 24;
const INITIAL_COLS: u16 = 80;

impl Tab {
    pub fn open(
        id: u64,
        session: Option<Session>,
        scrollback: usize,
        repaint: Arc<dyn Fn() + Send + Sync>,
    ) -> Self {
        let title = session
            .as_ref()
            .map_or_else(|| "Local terminal".to_string(), |s| s.name.clone());
        let mut tab = Self {
            id,
            title,
            session,
            term: Terminal::new(INITIAL_ROWS, INITIAL_COLS, scrollback),
            pty: None,
            status: None,
        };

        let (program, args) = match tab.session.as_ref().map(|s| &s.kind) {
            None => (None, Vec::new()),
            Some(SessionKind::LocalShell { program }) => (program.clone(), Vec::new()),
            // Stopgap until tc-ssh (russh) lands: drive the system OpenSSH client.
            Some(SessionKind::Ssh {
                host,
                port,
                user,
                private_key,
                x11_forwarding,
            }) => {
                let mut args = vec!["-p".to_string(), port.to_string()];
                if let Some(key) = private_key {
                    args.extend(["-i".into(), key.clone()]);
                }
                if *x11_forwarding {
                    args.push("-X".into());
                }
                let options = &tab.session.as_ref().expect("ssh tab has a session").options;
                let opt = |id: &str| options.get(id).map(String::as_str);
                // 3_7 Compression, 3_8 Execute command (parameter schema ids).
                if opt("3_7").is_some_and(tc_core::schema::is_true) {
                    args.push("-C".into());
                }
                let command = opt("3_8").map(str::trim).filter(|c| !c.is_empty());
                if command.is_some() {
                    args.push("-t".into());
                }
                args.push(match user {
                    Some(u) => format!("{u}@{host}"),
                    None => host.clone(),
                });
                args.extend(command.map(String::from));
                (Some("ssh".to_string()), args)
            }
            Some(other) => {
                let msg = format!(
                    "\x1b[33m{} sessions are not implemented yet - see docs/PARITY.md\x1b[0m\r\n",
                    other.label()
                );
                tab.term.feed(msg.as_bytes());
                tab.status = Some("unsupported session type".into());
                return tab;
            }
        };

        let arg_refs: Vec<&str> = args.iter().map(String::as_str).collect();
        match PtySession::spawn(
            program.as_deref(),
            &arg_refs,
            INITIAL_ROWS,
            INITIAL_COLS,
            repaint,
        ) {
            Ok(pty) => tab.pty = Some(pty),
            Err(e) => {
                tab.term
                    .feed(format!("\x1b[31mfailed to start: {e:#}\x1b[0m\r\n").as_bytes());
                tab.status = Some("failed to start".into());
            }
        }
        tab
    }

    /// Drain PTY output into the emulator. Returns true if anything changed.
    pub fn pump(&mut self) -> bool {
        let Some(pty) = &mut self.pty else {
            return false;
        };
        let mut changed = false;
        let mut exited = false;
        while let Ok(event) = pty.events().try_recv() {
            changed = true;
            match event {
                PtyEvent::Output(bytes) => {
                    let reply = self.term.feed(&bytes);
                    if !reply.is_empty() {
                        let _ = pty.write(&reply);
                    }
                }
                PtyEvent::Exited => exited = true,
            }
        }
        if exited {
            self.pty = None;
            self.status = Some("process exited".into());
            self.term.feed(
                b"\r\n\x1b[90m[process exited - close the tab or reopen the session]\x1b[0m\r\n",
            );
        }
        changed
    }

    pub fn send(&mut self, bytes: &[u8]) {
        if let Some(pty) = &mut self.pty {
            self.term.set_scrollback(0);
            let _ = pty.write(bytes);
        }
    }

    pub fn resize(&mut self, rows: u16, cols: u16) {
        if self.term.size() == (rows, cols) {
            return;
        }
        self.term.resize(rows, cols);
        if let Some(pty) = &self.pty {
            let _ = pty.resize(rows, cols);
        }
    }

    pub fn is_alive(&self) -> bool {
        self.pty.is_some()
    }
}
