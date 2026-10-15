//! Polling the backends for new events.

use super::*;

impl Tab {
    /// Drain backend events into the emulator. Returns true if anything changed.
    pub fn pump(&mut self) -> bool {
        self.pump_paste();
        if let Some(title) = self.term.take_title()
            && !self.lock_title
            && !title.is_empty()
        {
            self.title = title;
        }
        self.upload_prompts.extend(self.edits.changed());
        match &mut self.backend {
            Backend::None => false,
            Backend::Pty(_) => self.pump_pty(),
            Backend::Ssh(_) => self.pump_ssh(),
            Backend::Connecting(_) | Backend::Stream(_) => self.pump_stream(),
            Backend::Ftp(_) => self.pump_ftp(),
        }
    }

    pub(super) fn pump_ftp(&mut self) -> bool {
        let Backend::Ftp(ftp) = &self.backend else {
            return false;
        };
        let events: Vec<SftpEvent> = ftp.events().try_iter().collect();
        let changed = !events.is_empty();
        for event in events {
            match &event {
                SftpEvent::Ready { home } => {
                    self.message(&format!(
                        "\x1b[32mConnected ({home}). Drag files between the local and remote panes to transfer them.\x1b[0m"
                    ));
                    self.status = None;
                    self.after_login();
                }
                SftpEvent::Unavailable(e) => {
                    let kind = self.session.as_ref().map_or("FTP", |s| s.kind.label());
                    self.message(&format!("\x1b[31m{kind}: {e}\x1b[0m"));
                    self.status = Some("connection failed".into());
                }
                SftpEvent::UntrustedCertificate {
                    fingerprint,
                    reason,
                } => {
                    self.message(&format!(
                        "\x1b[33mThe server certificate is not trusted: {reason}\x1b[0m"
                    ));
                    self.cert_prompt = Some(CertPrompt {
                        fingerprint: fingerprint.clone(),
                        reason: reason.clone(),
                    });
                }
                SftpEvent::Failed { error, .. } => self.message(&format!("\x1b[33m{error}\x1b[0m")),
                _ => {}
            }
            let unavailable = matches!(event, SftpEvent::Unavailable(_));
            self.on_sftp(event);
            if unavailable {
                self.backend = Backend::None;
            }
        }
        changed
    }

    pub(super) fn pump_stream(&mut self) -> bool {
        if let Backend::Connecting(rx) = &self.backend {
            match rx.try_recv() {
                Ok(Ok(stream)) => {
                    self.backend = Backend::Stream(stream);
                    self.status = None;
                }
                Ok(Err(e)) => {
                    self.backend = Backend::None;
                    self.status = Some("connection failed".into());
                    self.message(&format!("\x1b[31m{e}\x1b[0m"));
                    return true;
                }
                Err(_) => return false,
            }
        }
        let Backend::Stream(stream) = &mut self.backend else {
            return false;
        };
        let events: Vec<tc_net::StreamEvent> = stream.events().try_iter().collect();
        let changed = !events.is_empty();
        for event in events {
            match event {
                tc_net::StreamEvent::Output(bytes) => {
                    let mut reply = self.output(&bytes);
                    if self.auto_login.is_some() && is_login_prompt(&bytes) {
                        let user = self.auto_login.take().expect("checked");
                        reply.extend_from_slice(user.as_bytes());
                        reply.push(b'\r');
                    }
                    if !reply.is_empty()
                        && let Backend::Stream(stream) = &mut self.backend
                    {
                        let _ = stream.write(&reply);
                    }
                }
                tc_net::StreamEvent::Closed(reason) => {
                    self.backend = Backend::None;
                    self.status = Some("connection closed".into());
                    let why = reason.map(|r| format!(": {r}")).unwrap_or_default();
                    self.session_ended(&format!("connection closed{why}"));
                    break;
                }
            }
        }
        changed
    }

    pub(super) fn pump_pty(&mut self) -> bool {
        let Backend::Pty(pty) = &self.backend else {
            return false;
        };
        let events: Vec<PtyEvent> = pty.events().try_iter().collect();
        let changed = !events.is_empty();
        let mut exited = false;
        for event in events {
            match event {
                PtyEvent::Output(bytes) => {
                    let reply = self.output(&bytes);
                    if !reply.is_empty()
                        && let Backend::Pty(pty) = &mut self.backend
                    {
                        let _ = pty.write(&reply);
                    }
                }
                PtyEvent::Exited => exited = true,
            }
        }
        if exited {
            self.backend = Backend::None;
            self.status = Some("process exited".into());
            self.session_ended("process exited");
        }
        changed
    }

    pub(super) fn pump_ssh(&mut self) -> bool {
        let Backend::Ssh(ssh) = &self.backend else {
            return false;
        };
        let events: Vec<SshEvent> = ssh.events().try_iter().collect();
        let changed = !events.is_empty();
        for event in events {
            if !matches!(self.backend, Backend::Ssh(_)) {
                break; // Mosh switched to mosh-client
            }
            match event {
                SshEvent::Output(bytes) if self.mosh.is_some() => {
                    self.mosh_output(&bytes);
                }
                SshEvent::Output(bytes) => {
                    let bytes = match &mut self.folder_hook {
                        Some(hook) => hook.filter(&bytes, Instant::now()),
                        None => bytes,
                    };
                    let reply = self.output(&bytes);
                    if !reply.is_empty()
                        && let Backend::Ssh(ssh) = &self.backend
                    {
                        ssh.write(&reply);
                    }
                    if let Some(dir) = self.term.take_cwd_report() {
                        self.on_cwd_report(dir);
                    }
                }
                SshEvent::Info(text) => self.message(&format!("\x1b[90m{text}\x1b[0m")),
                SshEvent::HostKey(info) => self.host_key = Some(info),
                SshEvent::SecretNeeded {
                    prompt,
                    kind,
                    user,
                    host,
                    port,
                    key_path,
                } => {
                    let target = target_for(kind, &user, &host, port, key_path.as_deref());
                    self.on_secret_needed(prompt, kind, target)
                }
                SshEvent::Connected => {
                    self.status = None;
                    self.after_login();
                }
                SshEvent::Listening(message) if self.via_gateway.is_some() => {
                    match tc_ssh::tunnel::listening_port(&message) {
                        Some(port) => self.gateway_ready(port),
                        None => self.message(&format!("\x1b[31mtunnel: {message}\x1b[0m")),
                    }
                }
                SshEvent::Error(e) => self.message(&format!("\x1b[31m{e}\x1b[0m")),
                SshEvent::Sftp(e) => self.on_sftp(e),
                SshEvent::Listening(_) => {}
                SshEvent::Monitor(sample) => {
                    if self.monitor.is_some() {
                        self.monitor = Some(sample);
                    }
                }
                SshEvent::LoginShell(shell) => {
                    if let Some(hook) = &mut self.folder_hook {
                        hook.on_login_shell(&shell);
                    }
                }
                SshEvent::Closed { exit_status } => {
                    if let Some(m) = self.mosh.take() {
                        let text = crate::mosh::without_key(&m.output);
                        self.message(&format!(
                            "\x1b[31mmosh-server did not start{}\x1b[0m",
                            if text.trim().is_empty() {
                                String::new()
                            } else {
                                format!(":\r\n{text}")
                            }
                        ));
                    }
                    self.folder_hook = None;
                    self.monitor = None;
                    self.backend = Backend::None;
                    self.sftp = SftpStatus::Off;
                    self.line = None;
                    self.host_key = None;
                    let code =
                        exit_status.map_or_else(String::new, |c| format!(" (exit status {c})"));
                    self.status = Some("session closed".into());
                    self.session_ended(&format!("session closed{code}"));
                }
            }
        }
        changed | self.poll_folder_hook()
    }

    /// Type the prompt hook when it is due, or show output held back.
    pub(super) fn poll_folder_hook(&mut self) -> bool {
        let Some(hook) = &mut self.folder_hook else {
            return false;
        };
        let action = hook.poll(Instant::now());
        let pending = !hook.is_done();
        let changed = match action {
            folder_hook::Action::None => false,
            folder_hook::Action::Type => {
                if let Backend::Ssh(ssh) = &self.backend {
                    ssh.write(FOLDER_TRACKING_HOOK.as_bytes());
                }
                false
            }
            folder_hook::Action::Feed(bytes) => {
                self.output(&bytes);
                true
            }
        };
        if !pending {
            self.folder_hook = None;
        }
        if let SftpStatus::Ready(v) = &mut self.sftp {
            v.auto_hook_pending = pending;
        }
        changed
    }

    /// When the tab needs a frame even without new events (hook and paste
    /// timers).
    pub fn wakeup(&self) -> Option<std::time::Duration> {
        let now = Instant::now();
        let paste =
            (!self.paste_queue.is_empty()).then(|| self.next_paste.saturating_duration_since(now));
        let hook = self.folder_hook.as_ref().and_then(|h| h.wakeup(now));
        // Watch edited files once a second.
        let hook = match (hook, self.edits.is_watching()) {
            (Some(h), true) => Some(h.min(std::time::Duration::from_secs(1))),
            (None, true) => Some(std::time::Duration::from_secs(1)),
            (h, false) => h,
        };
        match (paste, hook) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (a, b) => a.or(b),
        }
    }

    /// Auto-fill a saved password once per target; otherwise prompt in the
    /// terminal like PuTTY.
    pub(super) fn on_secret_needed(&mut self, prompt: String, kind: SecretKind, target: Target) {
        match self.credentials.on_prompt(kind, &target) {
            PromptAction::AutoFill(password) => {
                self.message(&format!(
                    "\x1b[90mUsing saved password for {}\x1b[0m",
                    target.key()
                ));
                if let Some(password) = self.pending_login(password)
                    && let Backend::Ssh(ssh) = &self.backend
                {
                    ssh.reply(Reply::Secret(password));
                }
            }
            PromptAction::Ask { saved_rejected } => {
                if saved_rejected {
                    self.message("\x1b[33mThe saved password was rejected.\x1b[0m");
                }
                self.term.feed(format!("{prompt} ").as_bytes());
                self.line = Some(LineInput::Secret(String::new(), kind, target));
            }
        }
    }

    /// After a successful login, save typed passwords (Always) or queue the
    /// "Do you want to save this password?" questions (Ask).
    pub(super) fn after_login(&mut self) {
        let (offers, errors) = self.credentials.after_login();
        self.offers.extend(offers);
        for e in errors {
            self.message(&format!("\x1b[31m{e}\x1b[0m"));
        }
    }
}

/// The end of `bytes` is a login prompt ("login:", "Username:").
fn is_login_prompt(bytes: &[u8]) -> bool {
    let text = String::from_utf8_lossy(bytes).to_ascii_lowercase();
    let text = text.trim_end();
    ["login:", "username:", "user name:"]
        .iter()
        .any(|p| text.ends_with(p))
}

#[cfg(test)]
mod login_prompt_tests {
    #[test]
    fn login_prompts() {
        assert!(super::is_login_prompt(b"\r\nrouter login: "));
        assert!(super::is_login_prompt(b"Username:"));
        assert!(!super::is_login_prompt(b"Last login: Mon\r\n$ "));
        assert!(!super::is_login_prompt(b"Password: "));
    }
}
