//! Keyboard input, paste and terminal output.

use super::*;

impl Tab {
    /// Output from the program: decoded from the session charset,
    /// logged, then shown.
    pub(super) fn output(&mut self, bytes: &[u8]) -> Vec<u8> {
        let decoded = self.charset.decode(bytes);
        let bytes: &[u8] = &decoded;
        let flush = self.expert.flush_logs;
        if let Some(log) = &mut self.log
            && let Err(e) = log
                .write(bytes)
                .and_then(|()| if flush { log.flush() } else { Ok(()) })
        {
            let path = log.path().display().to_string();
            self.log = None;
            self.message(&format!("\x1b[33mlogging to {path} stopped: {e}\x1b[0m"));
        }
        let replies = self.term.feed(bytes);
        if let Some(keys) = self.start_macro.take() {
            self.send(keys.as_bytes());
        }
        replies
    }

    /// Paste text: line endings as Enter, bracketed when the program asks,
    /// line by line when the session has a paste delay.
    pub fn paste(&mut self, text: &str) {
        let bytes = crate::term_view::paste_bytes(self, text);
        if self.paste_delay.is_zero() {
            self.send(&bytes);
            return;
        }
        for line in bytes.split_inclusive(|&b| b == b'\r') {
            self.paste_queue.push_back(line.to_vec());
        }
        self.next_paste = Instant::now();
        self.pump_paste();
    }

    pub(super) fn pump_paste(&mut self) {
        while !self.paste_queue.is_empty() && Instant::now() >= self.next_paste {
            let line = self.paste_queue.pop_front().expect("checked");
            self.send(&line);
            self.next_paste = Instant::now() + self.paste_delay;
        }
    }

    pub fn send(&mut self, bytes: &[u8]) {
        if self.ended {
            if let Some(action) = ended_key(bytes, &self.expert) {
                self.ended_action = Some(action);
            }
            return;
        }
        self.term.set_scrollback(0);
        if self.line.is_some() {
            self.line_input(bytes);
            return;
        }
        if let Some(hook) = &mut self.folder_hook {
            hook.on_user_input();
        }
        if self.expert.local_line_editing == Some(true) {
            let edit = self.line_editor.feed(bytes);
            if self.expert.local_echo != Some(false) && !edit.shown.is_empty() {
                self.term.feed(&edit.shown);
            }
            if !edit.send.is_empty() {
                self.write(&edit.send);
            }
            return;
        }
        if self.expert.local_echo == Some(true) {
            self.local_echo(bytes);
        }
        self.write(bytes);
    }

    /// To the program, in the session charset.
    fn write(&mut self, bytes: &[u8]) {
        let bytes: &[u8] = &self.charset.encode(bytes);
        match &mut self.backend {
            Backend::None => {}
            Backend::Pty(pty) => {
                let _ = pty.write(bytes);
            }
            Backend::Ssh(ssh) => ssh.write(bytes),
            Backend::Stream(stream) => {
                let _ = stream.write(bytes);
            }
            Backend::Connecting(_) | Backend::Ftp(_) => {}
        }
    }

    /// "Force local echo on": show what is typed without waiting for the
    /// remote side (control keys and escape sequences are not shown).
    pub(super) fn local_echo(&mut self, bytes: &[u8]) {
        if bytes.first() == Some(&0x1b) {
            return;
        }
        let mut shown = Vec::with_capacity(bytes.len());
        for &b in bytes {
            match b {
                b'\r' => shown.extend_from_slice(b"\r\n"),
                0x7f | 0x08 => shown.extend_from_slice(b"\x08 \x08"),
                b'\t' => shown.push(b),
                b if b < 0x20 => {}
                b => shown.push(b),
            }
        }
        if !shown.is_empty() {
            self.term.feed(&shown);
        }
    }

    pub(super) fn line_input(&mut self, bytes: &[u8]) {
        for ch in String::from_utf8_lossy(bytes).chars() {
            let Some(line) = &mut self.line else { return };
            let (buffer, echo) = match line {
                LineInput::Login(b) => (b, true),
                LineInput::Secret(b, ..) => (b, false),
            };
            match ch {
                '\r' | '\n' => {
                    self.term.feed(b"\r\n");
                    match self.line.take() {
                        Some(LineInput::Login(user))
                            if !user.is_empty() && self.via_gateway.is_some() =>
                        {
                            let via = self.via_gateway.take().expect("checked");
                            self.start_via_gateway(via, Some(user));
                        }
                        Some(LineInput::Login(user)) if !user.is_empty() => {
                            match self.session.as_ref().map(|s| &s.kind) {
                                Some(SessionKind::Mosh { .. }) => self.connect_mosh(user),
                                Some(SessionKind::Sftp { .. }) => self.connect_sftp(user),
                                Some(SessionKind::Rdp { .. }) if self.pending_rdp_waiting() => {
                                    self.rdp_login(user);
                                }
                                _ => self.connect_ssh(user),
                            }
                        }
                        Some(LineInput::Login(_)) => {
                            self.term.feed(b"login as: ");
                            self.line = Some(LineInput::Login(String::new()));
                        }
                        Some(LineInput::Secret(secret, kind, target)) => {
                            self.credentials.on_typed(kind, target, secret.clone());
                            if let Some(secret) = self.pending_login(secret)
                                && let Backend::Ssh(ssh) = &self.backend
                            {
                                ssh.reply(Reply::Secret(secret));
                            }
                        }
                        None => {}
                    }
                }
                '\u{7f}' | '\u{8}' => {
                    if buffer.pop().is_some() && echo {
                        self.term.feed(b"\x08 \x08");
                    }
                }
                // Ctrl+C / Ctrl+D abort the prompt.
                '\u{3}' | '\u{4}' => {
                    self.term.feed(b"^C\r\n");
                    self.line = None;
                    match &self.backend {
                        Backend::Ssh(ssh) => ssh.reply(Reply::Cancel),
                        _ => self.message("\x1b[90m[cancelled]\x1b[0m"),
                    }
                }
                c if !c.is_control() => {
                    buffer.push(c);
                    if echo {
                        let mut tmp = [0u8; 4];
                        self.term.feed(c.encode_utf8(&mut tmp).as_bytes());
                    }
                }
                _ => {}
            }
        }
    }

    pub fn resize(&mut self, rows: u16, cols: u16) {
        if self.term.size() == (rows, cols) {
            return;
        }
        self.term.resize(rows, cols);
        match &self.backend {
            Backend::None => {}
            Backend::Pty(pty) => {
                let _ = pty.resize(rows, cols);
            }
            Backend::Ssh(ssh) => ssh.resize(rows, cols),
            Backend::Stream(_) | Backend::Connecting(_) | Backend::Ftp(_) => {}
        }
        if let Backend::Stream(stream) = &mut self.backend {
            stream.resize(rows, cols);
        }
    }
}
