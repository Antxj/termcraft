//! Mosh sessions: `mosh-server` is started over SSH, it answers with
//! `MOSH CONNECT <port> <key>`, then the local `mosh-client` runs in the tab
//! with the key in its environment (as the `mosh` wrapper does). The key is
//! never shown in the terminal.

use std::net::IpAddr;
use std::path::PathBuf;

/// What came back from `mosh-server new`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Connect {
    pub port: u16,
    pub key: String,
}

/// `mosh-server new` command line: 3_8 server path, 3_5 UDP port
/// ("automatic" or a number / range), 3_6 locale ("auto" = the server's).
pub fn server_command(path: &str, udp_port: &str, locale: &str) -> Result<String, String> {
    let path = path.trim();
    let path = if path.is_empty() { "mosh-server" } else { path };
    if !path
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || "/._-~".contains(c))
    {
        return Err(format!("invalid mosh-server path {path:?}"));
    }
    let mut cmd = format!("{path} new -s -c 256");
    let udp_port = udp_port.trim();
    if !udp_port.is_empty() && !udp_port.eq_ignore_ascii_case("automatic") {
        if !udp_port.chars().all(|c| c.is_ascii_digit() || c == ':') {
            return Err(format!("invalid Mosh UDP port {udp_port:?}"));
        }
        cmd.push_str(&format!(" -p {udp_port}"));
    }
    let locale = locale.trim();
    if !locale.is_empty() && !locale.eq_ignore_ascii_case("auto") {
        if !locale
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "._-@".contains(c))
        {
            return Err(format!("invalid charset {locale:?}"));
        }
        cmd.push_str(&format!(" -l LANG={locale}"));
    }
    Ok(cmd)
}

/// Find `MOSH CONNECT <port> <key>` in the server output.
pub fn parse_connect(output: &[u8]) -> Option<Connect> {
    let text = String::from_utf8_lossy(output);
    let line = text
        .lines()
        .find_map(|l| l.trim().strip_prefix("MOSH CONNECT "))?;
    let mut parts = line.split_whitespace();
    let port = parts.next()?.parse().ok().filter(|&p| p > 0)?;
    let key = parts.next()?;
    // The session key is 22 base64 characters (128 bits).
    let valid = key.len() == 22
        && key
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '+' || c == '/');
    valid.then(|| Connect {
        port,
        key: key.to_string(),
    })
}

/// Server output without the `MOSH CONNECT` line (shown when it fails).
pub fn without_key(output: &[u8]) -> String {
    String::from_utf8_lossy(output)
        .lines()
        .filter(|l| !l.contains("MOSH CONNECT"))
        .collect::<Vec<_>>()
        .join("\r\n")
}

pub fn find_client() -> Option<PathBuf> {
    let exe = if cfg!(windows) {
        "mosh-client.exe"
    } else {
        "mosh-client"
    };
    std::env::split_paths(&std::env::var_os("PATH")?)
        .map(|dir| dir.join(exe))
        .find(|p| p.is_file())
}

/// 3_7 prediction type: 0 adaptive, 1 always, 2 never.
pub fn prediction(index: &str) -> &'static str {
    match index.trim() {
        "1" => "always",
        "2" => "never",
        _ => "adaptive",
    }
}

pub fn client_command(
    client: &std::path::Path,
    ip: IpAddr,
    connect: &Connect,
    prediction: &str,
) -> tc_pty::ShellCommand {
    tc_pty::ShellCommand {
        program: client.display().to_string(),
        args: vec![ip.to_string(), connect.port.to_string()],
        env: vec![
            ("MOSH_KEY".into(), connect.key.clone()),
            ("MOSH_PREDICTION_DISPLAY".into(), prediction.into()),
        ],
        cwd: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn server_command_is_checked() {
        assert_eq!(
            server_command("", "automatic", "auto").unwrap(),
            "mosh-server new -s -c 256"
        );
        assert_eq!(
            server_command("/usr/bin/mosh-server", "60001", "en_US.UTF-8").unwrap(),
            "/usr/bin/mosh-server new -s -c 256 -p 60001 -l LANG=en_US.UTF-8"
        );
        assert!(server_command("mosh-server; rm -rf ~", "", "").is_err());
        assert!(server_command("", "1; id", "").is_err());
        assert!(server_command("", "", "C; id").is_err());
    }

    #[test]
    fn connect_line() {
        let out =
            b"\r\nMOSH CONNECT 60004 4NeCCgvZFe2RnPgrcU1PQw\r\n\r\nmosh-server (mosh 1.4.0)\r\n";
        let c = parse_connect(out).unwrap();
        assert_eq!(c.port, 60004);
        assert_eq!(c.key, "4NeCCgvZFe2RnPgrcU1PQw");
        assert!(!without_key(out).contains("4NeCC"));
        assert!(parse_connect(b"MOSH CONNECT 0 4NeCCgvZFe2RnPgrcU1PQw").is_none());
        assert!(parse_connect(b"MOSH CONNECT 60004 short").is_none());
        assert!(parse_connect(b"command not found").is_none());
    }

    #[test]
    fn client_gets_the_key_in_its_environment() {
        let c = Connect {
            port: 60001,
            key: "4NeCCgvZFe2RnPgrcU1PQw".into(),
        };
        let cmd = client_command(
            std::path::Path::new("mosh-client"),
            "10.0.0.1".parse().unwrap(),
            &c,
            prediction("1"),
        );
        assert_eq!(cmd.args, ["10.0.0.1", "60001"]);
        assert!(
            !cmd.args.iter().any(|a| a.contains(&c.key)),
            "not on the command line"
        );
        assert!(cmd.env.contains(&("MOSH_KEY".into(), c.key.clone())));
        assert!(
            cmd.env
                .contains(&("MOSH_PREDICTION_DISPLAY".into(), "always".into()))
        );
    }
}
