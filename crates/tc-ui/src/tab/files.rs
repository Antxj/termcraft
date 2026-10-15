//! The file browser (SFTP / SCP / FTP / S3) of a tab.

use super::*;

impl Tab {
    /// Send a browser request over SFTP or FTP.
    pub(super) fn fs_request(&self, request: SftpRequest) -> Option<u64> {
        match &self.backend {
            Backend::Ssh(ssh) => Some(ssh.sftp(request)),
            Backend::Ftp(ftp) => Some(ftp.request(request)),
            _ => None,
        }
    }

    pub(super) fn on_sftp(&mut self, event: SftpEvent) {
        match event {
            SftpEvent::Ready { home } => {
                if matches!(
                    self.session.as_ref().map(|s| &s.kind),
                    Some(SessionKind::Sftp { .. })
                ) && self.sftp_reopen_cwd.is_none()
                {
                    self.message(&format!(
                        "\x1b[32mConnected ({home}). Drag files between the local and remote panes to transfer them.\x1b[0m"
                    ));
                    self.status = None;
                }
                // 3_18 "Try to follow SSH path in browser" (default on).
                let follow = self
                    .session
                    .as_ref()
                    .and_then(|s| s.options.get("3_18"))
                    .is_none_or(|v| schema::is_true(v));
                let session = self.session.as_ref();
                let is_ftp = matches!(session.map(|s| &s.kind), Some(SessionKind::Ftp { .. }));
                let is_sftp = matches!(session.map(|s| &s.kind), Some(SessionKind::Sftp { .. }));
                // "ASCII mode": FTP 3_9, SFTP 3_8.
                let ascii_id = if is_ftp { "3_9" } else { "3_8" };
                let ascii = (is_ftp || is_sftp)
                    && session
                        .and_then(|s| s.options.get(ascii_id))
                        .is_some_and(|v| schema::is_true(v));
                // SFTP 3_7 "Remote startup folder" (relative = in home).
                let start = session
                    .filter(|_| is_sftp)
                    .and_then(|s| s.options.get("3_7"))
                    .map(|d| d.trim())
                    .filter(|d| !d.is_empty() && *d != "<>")
                    .map(|d| {
                        if d.starts_with('/') {
                            d.to_string()
                        } else {
                            tc_ssh::sftp::join(&home, d)
                        }
                    });
                self.sftp = SftpStatus::Ready(Box::new(SftpView {
                    cwd: home.clone(),
                    path_input: home.clone(),
                    follow,
                    ascii,
                    sudo: self.sftp_sudo,
                    sudo_supported: matches!(self.backend, Backend::Ssh(_)),
                    ..SftpView::default()
                }));
                // A sudo switch keeps the folder; a new login starts at home.
                match self.sftp_reopen_cwd.take() {
                    Some(cwd) => self.sftp_list(&cwd),
                    None => {
                        self.sftp_just_ready = true;
                        self.sftp_list(start.as_deref().unwrap_or(&home));
                    }
                }
            }
            SftpEvent::Unavailable(e) => self.sftp = SftpStatus::Unavailable(e),
            // Handled by pump_ftp (the question); the browser waits.
            SftpEvent::UntrustedCertificate { reason, .. } => {
                self.sftp = SftpStatus::Unavailable(format!("certificate not trusted: {reason}"));
            }
            SftpEvent::SudoPasswordNeeded => {
                if let SftpStatus::Ready(v) = &mut self.sftp {
                    v.prompt = Some(BrowserPrompt::SudoPassword(Default::default()));
                }
            }
            SftpEvent::SudoFailed(e) => {
                self.sftp_sudo = !self.sftp_sudo;
                self.sftp_reopen_cwd = None;
                if let SftpStatus::Ready(v) = &mut self.sftp {
                    v.sudo = self.sftp_sudo;
                    v.error = Some(e);
                }
            }
            SftpEvent::Listing { id, path, entries } => {
                if let SftpStatus::Ready(v) = &mut self.sftp
                    && v.listing == Some(id)
                {
                    v.listing = None;
                    v.path_input = path.clone();
                    v.cwd = path;
                    v.entries = entries;
                    v.selected.clear();
                    v.error = None;
                }
            }
            SftpEvent::Progress { id, done, total } => {
                if let SftpStatus::Ready(v) = &mut self.sftp
                    && let Some(t) = v.transfers.get_mut(&id)
                {
                    t.1 = done;
                    t.2 = total;
                }
            }
            SftpEvent::Done { id } => {
                if let Some((local, how)) = self.edits.downloaded(id) {
                    use crate::remote_edit::OpenWith;
                    let opened = match how {
                        // The built-in editor (the app opens it).
                        OpenWith::TextEditor => {
                            let remote = self.edits.remote_of(&local).unwrap_or_default();
                            self.open_in_editor.push((local.clone(), remote));
                            Ok(String::new())
                        }
                        OpenWith::DefaultProgram => {
                            crate::external::open_path(&local.display().to_string())
                        }
                        OpenWith::Compare => {
                            self.open_in_diff.push(local.clone());
                            Ok(String::new())
                        }
                        OpenWith::Image => {
                            let remote = self.edits.remote_of(&local).unwrap_or_default();
                            self.open_in_viewer.push((local.clone(), remote));
                            Ok(String::new())
                        }
                        OpenWith::Program(program) => crate::external::open_with(&program, &local),
                    };
                    if let (Err(e), SftpStatus::Ready(v)) = (opened, &mut self.sftp) {
                        v.error = Some(e);
                    }
                }
                if let SftpStatus::Ready(v) = &mut self.sftp {
                    v.ops.remove(&id);
                    v.transfers.remove(&id);
                    let cwd = v.cwd.clone();
                    self.sftp_list(&cwd);
                }
            }
            SftpEvent::Failed { id, error } => {
                self.edits.failed(id);
                if let SftpStatus::Ready(v) = &mut self.sftp {
                    let label = v.ops.remove(&id);
                    v.transfers.remove(&id);
                    if v.listing == Some(id) {
                        v.listing = None;
                    }
                    v.error = Some(match label {
                        Some(l) => format!("{l}: {error}"),
                        None => error,
                    });
                }
            }
        }
    }

    pub(super) fn on_cwd_report(&mut self, dir: String) {
        let SftpStatus::Ready(v) = &mut self.sftp else {
            return;
        };
        v.reports_seen = true;
        if v.follow && dir != v.cwd {
            self.sftp_list(&dir);
        }
    }

    /// Type the OSC 7 prompt hook into the remote shell.
    pub fn install_folder_tracking(&mut self) {
        if let Backend::Ssh(ssh) = &self.backend {
            ssh.write(FOLDER_TRACKING_HOOK.as_bytes());
        }
    }

    /// "Use SUDO for SSH-browser": reopen the browser elevated or not. The
    /// password, when sudo asks for one, goes straight to the server.
    pub fn sftp_sudo(&mut self, on: bool, password: Option<String>) {
        let Backend::Ssh(ssh) = &self.backend else {
            return;
        };
        if let SftpStatus::Ready(v) = &mut self.sftp {
            v.sudo = on;
            v.error = None;
            self.sftp_reopen_cwd = Some(v.cwd.clone());
        }
        self.sftp_sudo = on;
        ssh.sftp_sudo(on, password);
    }

    /// "Open with default text editor" / "Open with default program":
    /// download to a private folder, open, then watch for changes.
    pub fn sftp_edit(&mut self, remote: &str, how: crate::remote_edit::OpenWith) {
        let local = match self.edits.start(remote, how) {
            Ok(l) => l,
            Err(e) => {
                if let SftpStatus::Ready(v) = &mut self.sftp {
                    v.error = Some(format!("cannot open {remote}: {e}"));
                }
                return;
            }
        };
        let request = SftpRequest::Download {
            remote: remote.to_string(),
            local: local.clone(),
            ascii: false,
        };
        if let Some(id) = self.fs_request(request) {
            self.edits.set_download(&local, id);
            if let SftpStatus::Ready(v) = &mut self.sftp {
                v.ops.insert(id, format!("Open {remote}"));
            }
        }
    }

    /// Upload an edited copy back ("upload to the server?" answered yes).
    pub fn upload_edited(&mut self, changed: &crate::remote_edit::Changed) {
        self.sftp_op(SftpRequest::Upload {
            local: changed.local.clone(),
            remote: changed.remote.clone(),
            ascii: false,
        });
    }

    /// Browse to a remote directory.
    pub fn sftp_list(&mut self, path: &str) {
        if !matches!(self.sftp, SftpStatus::Ready(_)) {
            return;
        }
        let id = self.fs_request(SftpRequest::List(path.to_string()));
        if let SftpStatus::Ready(v) = &mut self.sftp {
            v.listing = id;
        }
    }

    /// Start a mutating SFTP operation (refreshes the listing when done).
    pub fn sftp_op(&mut self, request: SftpRequest) {
        if !matches!(self.sftp, SftpStatus::Ready(_)) {
            return;
        }
        let (label, transfer) = match &request {
            SftpRequest::Download { remote, .. } => (format!("Download {remote}"), true),
            SftpRequest::Upload { local, .. } => (format!("Upload {}", local.display()), true),
            SftpRequest::Mkdir(p) => (format!("Create folder {p}"), false),
            SftpRequest::CreateFile(p) => (format!("Create file {p}"), false),
            SftpRequest::Remove { path, .. } => (format!("Delete {path}"), false),
            SftpRequest::Rename { from, .. } => (format!("Rename {from}"), false),
            SftpRequest::List(p) => (format!("List {p}"), false),
            SftpRequest::SetPermissions { path, mode } => {
                (format!("Set permissions {mode:o} on {path}"), false)
            }
        };
        let Some(id) = self.fs_request(request) else {
            return;
        };
        let SftpStatus::Ready(v) = &mut self.sftp else {
            return;
        };
        if transfer {
            v.transfers.insert(id, (label.clone(), 0, None));
        }
        v.ops.insert(id, label);
    }
}
