//! Starting each kind of connection.

use super::*;

impl Tab {
    pub(super) fn external_result(
        &mut self,
        kind: &str,
        host: &str,
        port: u16,
        result: Result<String, String>,
    ) {
        match result {
            Ok(client) => {
                self.message(&format!(
                    "\x1b[32m{kind} session to {host}:{port} opened in {client}.\x1b[0m"
                ));
                self.message("\x1b[90mAn embedded viewer is planned (see docs/PARITY.md).\x1b[0m");
                self.status = Some(format!("{kind} opened externally"));
            }
            Err(e) => {
                self.message(&format!("\x1b[31m{kind}: {e}\x1b[0m"));
                self.status = Some(format!("{kind} unavailable"));
            }
        }
    }

    /// "File" / "Browser" sessions: the target opened in another program.
    pub(super) fn opened_externally(
        &mut self,
        kind: &str,
        target: &str,
        result: Result<String, String>,
    ) {
        match result {
            Ok(app) => {
                self.message(&format!("\x1b[32m{target} opened in {app}.\x1b[0m"));
                self.status = Some(format!("{kind} opened externally"));
            }
            Err(e) => {
                self.message(&format!("\x1b[31m{kind}: {e}\x1b[0m"));
                self.status = Some(format!("{kind} unavailable"));
            }
        }
    }

    /// Mosh: start `mosh-server` over SSH (3_4 SSH port, 3_9 key, 3_5 UDP
    /// port, 3_6 charset, 3_7 prediction, 3_8 server path).
    pub(super) fn connect_mosh(&mut self, user: String) {
        let Some(Session {
            kind: SessionKind::Mosh { host, ssh_port, .. },
            options,
            ..
        }) = &self.session
        else {
            return;
        };
        let (host, ssh_port) = (host.clone(), *ssh_port);
        let opt = |id: &str| options.get(id).cloned().unwrap_or_default();
        let locale = schema::type_params(12)
            .iter()
            .find(|p| p.id == "3_6")
            .and_then(|p| match p.widget {
                schema::Widget::Choice(items) => {
                    items.get(opt("3_6").parse::<usize>().unwrap_or(0)).copied()
                }
                _ => None,
            })
            .unwrap_or("auto");
        let server = crate::mosh::server_command(&opt("3_8"), &opt("3_5"), locale);
        let key = Some(opt("3_9"))
            .filter(|k| !k.trim().is_empty())
            .map(|k| local_path(&k));
        let prediction = crate::mosh::prediction(&opt("3_7"));
        let Some(client) = crate::mosh::find_client() else {
            self.message(
                "\x1b[31mmosh-client was not found in PATH. Install Mosh (https://mosh.org) \
                 to use Mosh sessions.\x1b[0m",
            );
            self.status = Some("Mosh unavailable".into());
            return;
        };
        let server = match server {
            Ok(cmd) => cmd,
            Err(e) => {
                self.message(&format!("\x1b[31mMosh: {e}\x1b[0m"));
                return;
            }
        };
        use std::net::ToSocketAddrs;
        let Some(ip) = (host.as_str(), ssh_port)
            .to_socket_addrs()
            .ok()
            .and_then(|mut a| a.next())
            .map(|a| a.ip())
        else {
            self.message(&format!("\x1b[31mMosh: cannot resolve {host}\x1b[0m"));
            return;
        };
        self.message(&format!(
            "\x1b[90mStarting mosh-server on {host} over SSH …\x1b[0m"
        ));
        let (rows, cols) = self.term.size();
        self.credentials.reset();
        let (user, credential_error) = self.credentials.use_named(&user, &host, ssh_port);
        if let Some(e) = credential_error {
            self.message(&format!("\x1b[33m{e}\x1b[0m"));
        }
        let params = ConnectParams {
            host,
            port: ssh_port,
            user,
            private_key: key,
            gateways: Vec::new(),
            known_hosts: self.known_hosts.clone(),
            compression: false,
            command: Some(server),
            term: "xterm-256color".into(),
            cols,
            rows,
            sftp: false,
            browser_protocol: Default::default(),
            keepalive: None,
            detect_login_shell: false,
            remote_monitoring: false,
            agent: agent_config(&self.ssh_settings),
            shell: true,
            proxy: None,
            agent_forwarding: false,
            x11_display: None,
            refuse_trivial_auth: false,
        };
        self.mosh = Some(MoshStart {
            output: Vec::new(),
            ip,
            client,
            prediction,
        });
        self.backend = Backend::Ssh(SshSession::connect(params, self.repaint.clone()));
    }

    /// Mosh: collect the server's answer; once it has the session key,
    /// leave SSH and run `mosh-client`.
    pub(super) fn mosh_output(&mut self, bytes: &[u8]) {
        let Some(m) = &mut self.mosh else {
            return;
        };
        m.output.extend_from_slice(bytes);
        if m.output.len() > 64 * 1024 {
            m.output.drain(..32 * 1024);
        }
        if let Some(connect) = crate::mosh::parse_connect(&m.output) {
            let m = self.mosh.take().expect("checked");
            let command = crate::mosh::client_command(&m.client, m.ip, &connect, m.prediction);
            self.message(&format!(
                "\x1b[90mmosh-server is listening on UDP {}; starting mosh-client …\x1b[0m",
                connect.port
            ));
            // mosh-server keeps running on its own; SSH is no longer needed.
            self.backend = Backend::None;
            self.spawn_command(&command);
        }
    }

    /// SFTP session: SSH with only the file browser (no terminal); the
    /// tab shows the connection status.
    pub(super) fn connect_sftp(&mut self, user: String) {
        let Some(Session {
            kind: SessionKind::Sftp { host, port, .. },
            ..
        }) = &self.session
        else {
            return;
        };
        let (host, port) = (host.clone(), *port);
        self.credentials.reset();
        let (user, credential_error) = self.credentials.use_named(&user, &host, port);
        if let Some(e) = credential_error {
            self.message(&format!("\x1b[33m{e}\x1b[0m"));
        }
        self.message(&format!(
            "\x1b[90mConnecting to {user}@{host}:{port} (SFTP) …\x1b[0m"
        ));
        let (rows, cols) = self.term.size();
        let params = ConnectParams {
            host,
            port,
            user,
            private_key: None,
            gateways: Vec::new(),
            known_hosts: self.known_hosts.clone(),
            compression: false,
            command: None,
            term: "xterm-256color".into(),
            cols,
            rows,
            sftp: true,
            browser_protocol: Default::default(),
            keepalive: (self.ssh_settings.keepalive_secs > 0)
                .then(|| std::time::Duration::from_secs(self.ssh_settings.keepalive_secs)),
            detect_login_shell: false,
            remote_monitoring: false,
            agent: agent_config(&self.ssh_settings),
            shell: false,
            proxy: None,
            agent_forwarding: false,
            x11_display: None,
            refuse_trivial_auth: false,
        };
        self.sftp = SftpStatus::Connecting;
        self.backend = Backend::Ssh(SshSession::connect(params, self.repaint.clone()));
    }

    /// "Rsh" session over rlogin (TCP 513).
    pub(super) fn open_rlogin(&mut self, host: String, user: Option<String>) {
        self.message(&format!(
            "\x1b[90mConnecting to {host} (rlogin, not encrypted) …\x1b[0m"
        ));
        let (rows, cols) = self.term.size();
        let term = self.term_type.clone().unwrap_or_else(|| "xterm".into());
        let repaint = self.repaint.clone();
        self.open_in_background(move || {
            tc_net::rlogin::Rlogin::connect(
                &host,
                tc_net::rlogin::DEFAULT_PORT,
                user.as_deref().unwrap_or(""),
                &term,
                rows,
                cols,
                repaint,
            )
            .map(|r| Box::new(r) as Box<dyn tc_net::Stream>)
            .map_err(|e| format!("cannot connect to {host}: {e}"))
        });
    }

    /// WSL session: 3_6 user, 3_3 startup directory.
    pub(super) fn open_wsl(&mut self, distro: &str) {
        let opt = |id: &str| {
            self.session
                .as_ref()
                .and_then(|s| s.options.get(id))
                .cloned()
        };
        let (user, dir) = (opt("3_6"), opt("3_3"));
        match tc_pty::shells::wsl_command(distro, user.as_deref(), dir.as_deref()) {
            Some(command) => self.spawn_command(&command),
            None => {
                self.message("\x1b[31mWSL is not installed (wsl.exe not found).\x1b[0m");
                self.status = Some("WSL unavailable".into());
            }
        }
    }

    pub(super) fn spawn_local(&mut self, program: Option<&str>, shell: LocalShell, env: &TabEnv) {
        // 4_5 "Use Windows PATH" of the session, else the global setting.
        let windows_path = self
            .session
            .as_ref()
            .and_then(|s| s.options.get("4_5"))
            .map_or(env.use_windows_path, |v| schema::is_true(v));
        let command = match program {
            Some(p) => tc_pty::ShellCommand {
                program: p.to_string(),
                args: Vec::new(),
                env: Vec::new(),
            },
            None => match tc_pty::shells::locate(shell_kind(shell)) {
                Some(c) => c,
                None => {
                    self.message(&format!(
                        "\x1b[33m{} is not installed on this machine; using the system shell.\x1b[0m",
                        shell.label()
                    ));
                    if shell == LocalShell::Bash && cfg!(windows) {
                        self.message(
                            "\x1b[90mInstall Git for Windows or MSYS2 to get Bash.\x1b[0m",
                        );
                    }
                    tc_pty::ShellCommand {
                        program: tc_pty::default_shell(),
                        args: Vec::new(),
                        env: Vec::new(),
                    }
                }
            },
        };
        let mut command = command;
        // Git for Windows and MSYS2 read MSYS2_PATH_TYPE in /etc/profile.
        if cfg!(windows) && program.is_none() && matches!(shell, LocalShell::Bash | LocalShell::Zsh)
        {
            let kind = if windows_path { "inherit" } else { "minimal" };
            command.env.push(("MSYS2_PATH_TYPE".into(), kind.into()));
        }
        // "Home directory location" (Bash / Zsh read HOME).
        if program.is_none()
            && matches!(shell, LocalShell::Bash | LocalShell::Zsh)
            && let Some(home) = &env.home
        {
            command
                .env
                .push(("HOME".into(), home.display().to_string()));
        }
        self.spawn_command(&command);
    }

    pub(super) fn spawn_command(&mut self, command: &tc_pty::ShellCommand) {
        match PtySession::spawn_command(command, INITIAL_ROWS, INITIAL_COLS, self.repaint.clone()) {
            Ok(pty) => self.backend = Backend::Pty(pty),
            Err(e) => {
                self.message(&format!("\x1b[31mfailed to start: {e:#}\x1b[0m"));
                self.status = Some("failed to start".into());
            }
        }
    }

    pub(super) fn connect_ssh(&mut self, user: String) {
        let Some(Session {
            kind:
                SessionKind::Ssh {
                    host,
                    port,
                    private_key,
                    x11_forwarding,
                    ..
                },
            options,
            ..
        }) = &self.session
        else {
            return;
        };
        // Schema ids: 3_7 Compression, 3_8 Execute command.
        let opt = |id: &str| options.get(id).map(String::as_str);
        let (rows, cols) = self.term.size();
        self.credentials.reset();
        let (user, credential_error) = self.credentials.use_named(&user, host, *port);
        // Network settings: the SSH jump hosts (3_9 host, 3_10 port,
        // 3_11 login, 3_16 key), one or several.
        let gateways = tc_core::jump_hosts::parse(options, tc_core::jump_hosts::SSH)
            .into_iter()
            .map(|hop| tc_ssh::Endpoint {
                port: hop.port_number(),
                user: if hop.user.is_empty() {
                    user.clone()
                } else {
                    hop.user
                },
                private_key: (!hop.key.is_empty()).then(|| local_path(&hop.key)),
                host: hop.host,
            })
            .collect();
        let x11 = if *x11_forwarding {
            x11_display()
        } else {
            (None, None)
        };
        let mut params = ConnectParams {
            host: host.clone(),
            port: *port,
            user,
            private_key: private_key.as_deref().map(local_path),
            gateways,
            known_hosts: self.known_hosts.clone(),
            compression: opt("3_7").is_some_and(schema::is_true),
            command: opt("3_8")
                .map(str::trim)
                .filter(|c| !c.is_empty())
                .map(|c| keep_open(c, opt("3_12").is_some_and(schema::is_true))),
            term: self
                .term_type
                .clone()
                .unwrap_or_else(|| "xterm-256color".into()),
            cols,
            rows,
            // 3_26 SSH-browser type: 0 = none.
            sftp: self.ssh_settings.browser && opt("3_26") != Some("0"),
            // 3_26: 2 "SCP (enhanced speed)", 3 "SCP (normal speed)".
            browser_protocol: match opt("3_26") {
                Some("2") => tc_ssh::BrowserProtocol::Scp { enhanced: true },
                Some("3") => tc_ssh::BrowserProtocol::Scp { enhanced: false },
                _ => tc_ssh::BrowserProtocol::Sftp,
            },
            keepalive: (self.ssh_settings.keepalive_secs > 0)
                .then(|| std::time::Duration::from_secs(self.ssh_settings.keepalive_secs)),
            detect_login_shell: false,
            remote_monitoring: self.ssh_settings.remote_monitoring,
            // 3_34 "Attempt SSH agent authentication" (default on).
            agent: if opt("3_34").is_none_or(schema::is_true) {
                agent_config(&self.ssh_settings)
            } else {
                tc_ssh::AgentConfig::default()
            },
            shell: true,
            proxy: tc_ssh::proxy::from_options(&|id| opt(id).map(String::from)),
            agent_forwarding: opt("3_35").is_some_and(schema::is_true),
            x11_display: x11.0,
            refuse_trivial_auth: opt("3_32").is_some_and(schema::is_true),
        };
        if params.remote_monitoring {
            self.monitor = Some(Box::default());
        }
        // 3_18 "Try to follow SSH path in browser" (default on).
        params.detect_login_shell =
            params.sftp && params.command.is_none() && opt("3_18").is_none_or(schema::is_true);
        self.folder_hook = params.detect_login_shell.then(FolderHook::new);
        if let Some(e) = credential_error {
            self.message(&format!("\x1b[33m{e}\x1b[0m"));
        }
        if params.sftp {
            self.sftp = SftpStatus::Connecting;
        }
        self.backend = Backend::Ssh(SshSession::connect(params, self.repaint.clone()));
    }

    /// Open a connection off the UI thread (TCP connect can take seconds).
    pub(super) fn open_in_background(
        &mut self,
        open: impl FnOnce() -> StreamResult + Send + 'static,
    ) {
        let (tx, rx) = crossbeam_channel::bounded(1);
        let repaint = self.repaint.clone();
        std::thread::spawn(move || {
            let _ = tx.send(open());
            repaint();
        });
        self.backend = Backend::Connecting(rx);
    }

    /// Open the local tunnel to the end server through the jump hosts.
    /// Hops without a login ask for one first ("login as:").
    pub(super) fn start_via_gateway(&mut self, via: ViaGateway, login: Option<String>) {
        let needs_login = via.hops.iter().any(|h| h.user.trim().is_empty());
        let login = match login {
            Some(l) => l,
            None if needs_login => {
                self.message("\x1b[90mLogin for the SSH gateway:\x1b[0m");
                self.term.feed(b"login as: ");
                self.line = Some(LineInput::Login(String::new()));
                self.via_gateway = Some(via);
                return;
            }
            None => String::new(),
        };
        let mut endpoints: Vec<tc_ssh::Endpoint> = via
            .hops
            .iter()
            .map(|h| tc_ssh::Endpoint {
                host: h.host.clone(),
                port: h.port_number(),
                user: if h.user.trim().is_empty() {
                    login.clone()
                } else {
                    h.user.trim().to_string()
                },
                private_key: (!h.key.is_empty()).then(|| local_path(&h.key)),
            })
            .collect();
        let Some(last) = endpoints.pop() else {
            return;
        };
        self.message(&format!(
            "\x1b[90mOpening a tunnel to {}:{} through {} …\x1b[0m",
            via.host,
            via.port,
            tc_core::jump_hosts::summary(&via.hops)
        ));
        let params = tc_ssh::tunnel::TunnelParams {
            ssh: last,
            gateways: endpoints,
            known_hosts: self.known_hosts.clone(),
            spec: tc_ssh::tunnel::TunnelSpec::Local {
                bind: "127.0.0.1".into(),
                port: 0,
                dest_host: via.host.clone(),
                dest_port: via.port,
            },
            agent: agent_config(&self.ssh_settings),
        };
        self.credentials.reset();
        self.via_gateway = Some(via);
        self.backend = Backend::Ssh(SshSession::tunnel(params, self.repaint.clone()));
    }

    /// The tunnel listens: keep it, and start the session through it.
    pub(super) fn gateway_ready(&mut self, local_port: u16) {
        let Some(via) = self.via_gateway.take() else {
            return;
        };
        if let Backend::Ssh(tunnel) = std::mem::replace(&mut self.backend, Backend::None) {
            self.gateway_tunnel = Some(tunnel);
        }
        let local = "127.0.0.1";
        let external = !matches!(via.target, GatewayTarget::Telnet);
        match via.target {
            GatewayTarget::Telnet => self.open_telnet(local.into(), local_port),
            GatewayTarget::Rdp(user) => {
                let temp = std::env::temp_dir().join("termcraft-rdp");
                let result = crate::external::launch_rdp(local, local_port, user.as_deref(), &temp);
                self.external_result("RDP", &via.host, via.port, result);
            }
            GatewayTarget::Vnc => {
                let result = crate::external::launch_vnc(local, local_port);
                self.external_result("VNC", &via.host, via.port, result);
            }
        }
        if external {
            self.message(&format!(
                "\x1b[90mThe tunnel stays open on {local}:{local_port} while this tab is \
                 open.\x1b[0m"
            ));
        }
    }

    /// Telnet / VNC through the session proxy: a loopback port bridged
    /// through it. Refused (never a direct connection) when that is not
    /// possible.
    pub(super) fn open_via_proxy(&mut self, proxy: tc_ssh::proxy::Proxy) {
        let Some((host, port, telnet)) = (match &self.session.as_ref().map(|s| &s.kind) {
            Some(SessionKind::Telnet { host, port }) => Some((host.clone(), *port, true)),
            Some(SessionKind::Vnc { host, port }) => Some((host.clone(), *port, false)),
            _ => None,
        }) else {
            return;
        };
        let refuse = |tab: &mut Self, why: &str| {
            tab.message(&format!("\x1b[31mNot connected: {why}.\x1b[0m"));
            tab.session_ended("proxy not available");
        };
        if matches!(proxy.kind, tc_ssh::proxy::ProxyKind::SshCommand(_)) {
            return refuse(
                self,
                "the \"SSH command\" proxy type is only available for SSH sessions",
            );
        }
        if via_gateway(self.session.as_ref()).is_some() {
            return refuse(
                self,
                "a proxy together with an SSH gateway is only available for SSH sessions",
            );
        }
        let (proxy_host, proxy_port) = (proxy.host.clone(), proxy.port);
        let forward = match tc_ssh::proxy::forward_local(proxy, host.clone(), port) {
            Ok(f) => f,
            Err(e) => return refuse(self, &e.to_string()),
        };
        let local_port = forward.port;
        self.proxy_forward = Some(forward);
        self.message(&format!(
            "\x1b[90mThrough proxy {proxy_host}:{proxy_port} …\x1b[0m"
        ));
        if telnet {
            self.open_telnet("127.0.0.1".into(), local_port);
        } else {
            let result = crate::external::launch_vnc("127.0.0.1", local_port);
            self.external_result("VNC", &host, port, result);
            self.message(&format!(
                "\x1b[90mThe proxy connection stays open on 127.0.0.1:{local_port} while \
                 this tab is open.\x1b[0m"
            ));
        }
    }

    pub(super) fn open_telnet(&mut self, host: String, port: u16) {
        self.message(&format!(
            "\x1b[90mConnecting to {host}:{port} (Telnet) …\x1b[0m"
        ));
        let (rows, cols) = self.term.size();
        let term = self.term_type.clone().unwrap_or_else(|| "xterm".into());
        let repaint = self.repaint.clone();
        self.open_in_background(move || {
            tc_net::telnet::Telnet::connect(&host, port, &term, rows, cols, repaint)
                .map(|t| Box::new(t) as Box<dyn tc_net::Stream>)
                .map_err(|e| format!("{host}:{port}: {e}"))
        });
    }

    pub(super) fn open_serial(&mut self, port: String, baud: u32) {
        // Advanced Serial settings: 3_4 data bits, 3_5 stop bits, 3_6 parity, 3_7 flow.
        let session = self.session.clone();
        let idx = |id: &str, default: u8| -> u8 {
            let param = tc_core::schema::type_params(8).iter().find(|p| p.id == id);
            match (session.as_ref(), param) {
                (Some(s), Some(p)) => s.option(p).parse().unwrap_or(default),
                _ => default,
            }
        };
        let config = tc_net::serial::SerialConfig::from_indices(
            &port,
            baud,
            idx("3_4", 3),
            idx("3_5", 0),
            idx("3_6", 0),
            idx("3_7", 1),
        );
        self.message(&format!("\x1b[90mOpening {port} at {baud} baud …\x1b[0m"));
        let repaint = self.repaint.clone();
        self.open_in_background(move || {
            let config = config?;
            tc_net::serial::Serial::open(&config, repaint)
                .map(|s| Box::new(s) as Box<dyn tc_net::Stream>)
        });
    }

    pub(super) fn open_ftp(&mut self, host: String, port: u16, user: Option<String>) {
        // Advanced Ftp settings: 3_5 passive, 3_9 ASCII, 3_6 remote startup folder.
        let session = self.session.clone();
        let opt = |id: &str| -> Option<String> {
            let param = tc_core::schema::type_params(6)
                .iter()
                .find(|p| p.id == id)?;
            Some(session.as_ref()?.option(param).to_string())
        };
        let params = tc_net::ftp::FtpParams {
            host: host.clone(),
            port,
            user: user.clone().unwrap_or_default(),
            password: String::new(),
            passive: opt("3_5").is_none_or(|v| schema::is_true(&v)),
            ascii: opt("3_9").is_some_and(|v| schema::is_true(&v)),
            start_dir: opt("3_6").filter(|d| !d.trim().is_empty()),
            // 3_7 FTPS, 3_20 implicit, 3_19 TLS 1.2, 3_22 TLS 1.3.
            tls: opt("3_7")
                .is_some_and(|v| schema::is_true(&v))
                .then(|| tc_net::ftp::FtpsParams {
                    implicit: opt("3_20").is_some_and(|v| schema::is_true(&v)),
                    versions: tc_net::ftps::TlsVersions {
                        tls12: opt("3_19").is_none_or(|v| schema::is_true(&v)),
                        tls13: opt("3_22").is_some_and(|v| schema::is_true(&v)),
                    },
                    pinned: tc_net::ftps::trusted(&self.ftps_certificates(), &host, port),
                }),
        };
        match user.filter(|u| !u.is_empty()) {
            // Anonymous login needs no password.
            None => self.start_ftp(params),
            Some(user) => {
                self.pending_ftp = Some(params);
                let target = Target {
                    protocol: "ftp".into(),
                    username: user.clone(),
                    server: format!("{host}:{port}"),
                };
                let prompt = format!("{user}@{host}'s FTP password:");
                self.on_secret_needed(prompt, SecretKind::Password, target);
            }
        }
    }

    /// A password (typed or saved) for a pending FTP / S3 login; given back
    /// when no such login waits (it is then for the SSH connection).
    pub(super) fn pending_login(&mut self, secret: String) -> Option<String> {
        if let Some(mut ftp) = self.pending_ftp.take() {
            ftp.password = secret;
            self.start_ftp(ftp);
            return None;
        }
        if let Some(mut s3) = self.pending_s3.take() {
            s3.secret = zeroize::Zeroizing::new(secret);
            self.start_s3(s3);
            return None;
        }
        Some(secret)
    }

    /// XDMCP through an X server (3_5 "Specify server to connect to" with
    /// 3_2, else broadcast; 3_3 sender IP, 3_12 screen, 3_14 / 3_15 size).
    pub(super) fn open_xdmcp(&mut self, host: &str) {
        let opt = |id: &str| {
            self.session
                .as_ref()
                .and_then(|s| s.options.get(id))
                .cloned()
        };
        let query = opt("3_5").is_some_and(|v| schema::is_true(&v)) || !host.trim().is_empty();
        let num = |id: &str, default: u32| {
            opt(id)
                .and_then(|v| v.trim().parse::<u32>().ok())
                .unwrap_or(default)
        };
        let from = opt("3_3");
        let request = crate::external::Xdmcp {
            host: (query && !host.trim().is_empty()).then_some(host.trim()),
            from: from.as_deref(),
            display: u16::try_from(num("3_12", 1)).unwrap_or(1),
            size: (num("3_14", 1024), num("3_15", 768)),
        };
        let shown = request.host.unwrap_or("any server (broadcast)").to_string();
        let result = crate::external::launch_xdmcp(&request);
        match result {
            Ok(server) => {
                self.message(&format!(
                    "\x1b[32mXDMCP session to {shown} opened in {server} (display :{}).\x1b[0m",
                    request.display
                ));
                self.status = Some("XDMCP opened externally".into());
            }
            Err(e) => {
                self.message(&format!("\x1b[31mXDMCP: {e}\x1b[0m"));
                self.status = Some("XDMCP unavailable".into());
            }
        }
    }

    /// "Aws S3": the secret access key is asked for (or taken from the
    /// password store), then the buckets are listed.
    pub(super) fn open_s3(&mut self, key_id: String) {
        let key_id = key_id.trim().to_string();
        if key_id.is_empty() {
            self.message("\x1b[31mAws S3: no Key ID in this session\x1b[0m");
            self.status = Some("no Key ID".into());
            return;
        }
        self.pending_s3 = Some(tc_net::s3::S3Params {
            key_id: key_id.clone(),
            secret: zeroize::Zeroizing::new(String::new()),
            region: "us-east-1".into(),
            start_path: None,
            endpoint: None,
        });
        let target = Target {
            protocol: "s3".into(),
            username: key_id.clone(),
            server: "s3.amazonaws.com".into(),
        };
        self.on_secret_needed(
            format!("Secret access key for {key_id}:"),
            SecretKind::Password,
            target,
        );
    }

    pub(super) fn start_s3(&mut self, params: tc_net::s3::S3Params) {
        self.message("\x1b[90mConnecting to Amazon S3 …\x1b[0m");
        self.sftp = SftpStatus::Connecting;
        self.backend = Backend::Ftp(Box::new(tc_net::s3::S3Session::connect(
            params,
            self.repaint.clone(),
        )));
    }

    /// FTPS certificates the user trusted (next to known_hosts).
    pub(super) fn ftps_certificates(&self) -> PathBuf {
        self.known_hosts.with_file_name("ftps_certificates")
    }

    /// FTPS certificate question answered: reconnect trusting it (and
    /// remember it unless "Connect once").
    pub fn answer_certificate(&mut self, accept: bool, remember: bool) {
        let Some(prompt) = self.cert_prompt.take() else {
            return;
        };
        let Some(mut params) = self.last_ftp.take() else {
            return;
        };
        if !accept {
            self.message("\x1b[90m[connection cancelled]\x1b[0m");
            return;
        }
        if remember
            && let Err(e) = tc_net::ftps::trust(
                &self.ftps_certificates(),
                &params.host,
                params.port,
                &prompt.fingerprint,
            )
        {
            self.message(&format!(
                "\x1b[33mcould not save the certificate: {e}\x1b[0m"
            ));
        }
        if let Some(tls) = &mut params.tls {
            tls.pinned = Some(prompt.fingerprint);
        }
        self.start_ftp(params);
    }

    pub(super) fn start_ftp(&mut self, params: tc_net::ftp::FtpParams) {
        self.message(&format!(
            "\x1b[90mConnecting to {}:{} ({}) …\x1b[0m",
            params.host,
            params.port,
            if params.tls.is_some() { "FTPS" } else { "FTP" }
        ));
        self.last_ftp = Some(params.clone());
        self.sftp = SftpStatus::Connecting;
        self.backend = Backend::Ftp(Box::new(tc_net::ftp::FtpSession::connect(
            params,
            self.repaint.clone(),
        )));
    }
}
