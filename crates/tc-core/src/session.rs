use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct SessionId(pub uuid::Uuid);

impl SessionId {
    pub fn new() -> Self {
        Self(uuid::Uuid::new_v4())
    }
}

impl Default for SessionId {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Display for SessionId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

/// Connection type and its type-specific parameters.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum SessionKind {
    LocalShell {
        /// Shell program; `None` means the platform default.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        program: Option<String>,
    },
    Ssh {
        host: String,
        #[serde(default = "default_ssh_port")]
        port: u16,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        user: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        private_key: Option<String>,
        #[serde(default)]
        x11_forwarding: bool,
    },
    Telnet {
        host: String,
        #[serde(default = "default_telnet_port")]
        port: u16,
    },
    Serial {
        port_name: String,
        #[serde(default = "default_baud")]
        baud: u32,
    },
    Rdp {
        host: String,
        #[serde(default = "default_rdp_port")]
        port: u16,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        user: Option<String>,
    },
    Vnc {
        host: String,
        #[serde(default = "default_vnc_port")]
        port: u16,
    },
    Sftp {
        host: String,
        #[serde(default = "default_ssh_port")]
        port: u16,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        user: Option<String>,
    },
    Ftp {
        host: String,
        #[serde(default = "default_ftp_port")]
        port: u16,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        user: Option<String>,
    },
    /// Remote login over rlogin (TCP 513). Plain text, like the reference.
    Rsh {
        host: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        user: Option<String>,
    },
    /// A local file or folder opened with its default application.
    File { path: String },
    /// A web page opened in the browser.
    Browser { url: String },
    /// Mosh: `mosh-server` started over SSH, then the local `mosh-client`.
    Mosh {
        host: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        user: Option<String>,
        #[serde(default = "default_ssh_port")]
        ssh_port: u16,
    },
    /// XDMCP: an X server asks a display manager for a graphical login
    /// (`host` empty = broadcast to any server).
    Xdmcp {
        #[serde(default)]
        host: String,
    },
    /// "Aws S3": browse buckets with an access key (the secret is asked
    /// for, or saved in the password store).
    AwsS3 { key_id: String },
    /// Windows Subsystem for Linux; `distro` "Default" = the default one.
    Wsl {
        #[serde(default = "default_wsl_distro")]
        distro: String,
    },
}

fn default_wsl_distro() -> String {
    "Default".into()
}

fn default_ssh_port() -> u16 {
    22
}
fn default_telnet_port() -> u16 {
    23
}
fn default_rdp_port() -> u16 {
    3389
}
fn default_vnc_port() -> u16 {
    5900
}
fn default_ftp_port() -> u16 {
    21
}
fn default_baud() -> u32 {
    115_200
}

impl SessionKind {
    pub fn label(&self) -> &'static str {
        match self {
            Self::LocalShell { .. } => "Shell",
            Self::Ssh { .. } => "SSH",
            Self::Telnet { .. } => "Telnet",
            Self::Serial { .. } => "Serial",
            Self::Rdp { .. } => "RDP",
            Self::Vnc { .. } => "VNC",
            Self::Sftp { .. } => "SFTP",
            Self::Ftp { .. } => "FTP",
            Self::Rsh { .. } => "Rsh",
            Self::File { .. } => "File",
            Self::Browser { .. } => "Browser",
            Self::Mosh { .. } => "Mosh",
            Self::Wsl { .. } => "WSL",
            Self::AwsS3 { .. } => "Aws S3",
            Self::Xdmcp { .. } => "Xdmcp",
        }
    }

    /// Reference session type index (`[1_N]` in the parameter schema).
    pub fn type_index(&self) -> usize {
        match self {
            Self::Ssh { .. } => 0,
            Self::Telnet { .. } => 1,
            Self::Rdp { .. } => 4,
            Self::Vnc { .. } => 5,
            Self::Ftp { .. } => 6,
            Self::Sftp { .. } => 7,
            Self::Serial { .. } => 8,
            Self::LocalShell { .. } => 10,
            Self::Rsh { .. } => 2,
            Self::File { .. } => 9,
            Self::Browser { .. } => 11,
            Self::Mosh { .. } => 12,
            Self::Wsl { .. } => 14,
            Self::AwsS3 { .. } => 13,
            Self::Xdmcp { .. } => 3,
        }
    }

    /// Default kind for a type index, if TermCraft supports that type yet.
    pub fn default_for(type_index: usize) -> Option<Self> {
        Some(match type_index {
            0 => Self::Ssh {
                host: String::new(),
                port: 22,
                user: None,
                private_key: None,
                x11_forwarding: true,
            },
            1 => Self::Telnet {
                host: String::new(),
                port: 23,
            },
            4 => Self::Rdp {
                host: String::new(),
                port: 3389,
                user: None,
            },
            5 => Self::Vnc {
                host: String::new(),
                port: 5900,
            },
            6 => Self::Ftp {
                host: String::new(),
                port: 21,
                user: None,
            },
            7 => Self::Sftp {
                host: String::new(),
                port: 22,
                user: None,
            },
            8 => Self::Serial {
                port_name: String::new(),
                baud: 115_200,
            },
            10 => Self::LocalShell { program: None },
            2 => Self::Rsh {
                host: String::new(),
                user: None,
            },
            9 => Self::File {
                path: String::new(),
            },
            11 => Self::Browser { url: String::new() },
            12 => Self::Mosh {
                host: String::new(),
                user: None,
                ssh_port: 22,
            },
            3 => Self::Xdmcp {
                host: String::new(),
            },
            13 => Self::AwsS3 {
                key_id: String::new(),
            },
            14 => Self::Wsl {
                distro: default_wsl_distro(),
            },
            _ => return None,
        })
    }

    /// Schema parameter ids that live in typed fields of this kind rather
    /// than in `Session::options`.
    pub fn typed_ids(&self) -> &'static [&'static str] {
        match self {
            Self::Ssh { .. } => &["3_2", "3_3", "3_4", "3_6", "3_15"],
            Self::Telnet { .. } | Self::Vnc { .. } => &["3_2", "3_3"],
            Self::Rdp { .. } | Self::Ftp { .. } | Self::Sftp { .. } => &["3_2", "3_3", "3_4"],
            Self::Serial { .. } => &["3_9", "3_3"],
            Self::LocalShell { .. } => &[],
            Self::Rsh { .. } => &["3_2", "3_3"],
            Self::File { .. }
            | Self::Browser { .. }
            | Self::Wsl { .. }
            | Self::AwsS3 { .. }
            | Self::Xdmcp { .. } => &["3_2"],
            Self::Mosh { .. } => &["3_2", "3_3", "3_4"],
        }
    }

    /// Read a typed field by schema id, in the schema's string encoding.
    pub fn typed_get(&self, id: &str) -> Option<String> {
        let opt = |v: &Option<String>| v.clone().unwrap_or_default();
        Some(match (self, id) {
            (
                Self::Ssh { host, .. }
                | Self::Telnet { host, .. }
                | Self::Rdp { host, .. }
                | Self::Vnc { host, .. }
                | Self::Ftp { host, .. }
                | Self::Sftp { host, .. },
                "3_2",
            ) => host.clone(),
            (
                Self::Ssh { port, .. }
                | Self::Telnet { port, .. }
                | Self::Rdp { port, .. }
                | Self::Vnc { port, .. }
                | Self::Ftp { port, .. }
                | Self::Sftp { port, .. },
                "3_3",
            ) => port.to_string(),
            (
                Self::Ssh { user, .. }
                | Self::Rdp { user, .. }
                | Self::Ftp { user, .. }
                | Self::Sftp { user, .. },
                "3_4",
            ) => opt(user),
            (Self::Ssh { x11_forwarding, .. }, "3_6") => {
                crate::schema::bool_value(*x11_forwarding).to_string()
            }
            (Self::Ssh { private_key, .. }, "3_15") => opt(private_key),
            (Self::Serial { port_name, .. }, "3_9") => port_name.clone(),
            (Self::Serial { baud, .. }, "3_3") => baud.to_string(),
            (Self::Rsh { host, .. } | Self::Mosh { host, .. }, "3_2") => host.clone(),
            (Self::Rsh { user, .. } | Self::Mosh { user, .. }, "3_3") => opt(user),
            (Self::Mosh { ssh_port, .. }, "3_4") => ssh_port.to_string(),
            (Self::File { path }, "3_2") => path.clone(),
            (Self::Browser { url }, "3_2") => url.clone(),
            (Self::Wsl { distro }, "3_2") => distro.clone(),
            (Self::AwsS3 { key_id }, "3_2") => key_id.clone(),
            (Self::Xdmcp { host }, "3_2") => host.clone(),
            _ => return None,
        })
    }

    /// Write a typed field by schema id. Returns an error message for values
    /// that do not fit the field (e.g. a port outside 1–65535).
    pub fn typed_set(&mut self, id: &str, value: &str) -> Result<(), String> {
        let value = value.trim();
        let opt = || (!value.is_empty()).then(|| value.to_string());
        match (self, id) {
            (
                Self::Ssh { host, .. }
                | Self::Telnet { host, .. }
                | Self::Rdp { host, .. }
                | Self::Vnc { host, .. }
                | Self::Ftp { host, .. }
                | Self::Sftp { host, .. },
                "3_2",
            ) => *host = value.to_string(),
            (
                Self::Ssh { port, .. }
                | Self::Telnet { port, .. }
                | Self::Rdp { port, .. }
                | Self::Vnc { port, .. }
                | Self::Ftp { port, .. }
                | Self::Sftp { port, .. },
                "3_3",
            ) => {
                *port = value
                    .parse()
                    .ok()
                    .filter(|&p| p > 0)
                    .ok_or_else(|| format!("invalid port {value:?}"))?;
            }
            (
                Self::Ssh { user, .. }
                | Self::Rdp { user, .. }
                | Self::Ftp { user, .. }
                | Self::Sftp { user, .. },
                "3_4",
            ) => *user = opt(),
            (Self::Ssh { x11_forwarding, .. }, "3_6") => {
                *x11_forwarding = crate::schema::is_true(value);
            }
            (Self::Ssh { private_key, .. }, "3_15") => *private_key = opt(),
            (Self::Serial { port_name, .. }, "3_9") => *port_name = value.to_string(),
            (Self::Serial { baud, .. }, "3_3") => {
                *baud = value
                    .parse()
                    .ok()
                    .filter(|&b| b > 0)
                    .ok_or_else(|| format!("invalid speed {value:?}"))?;
            }
            (Self::Rsh { host, .. } | Self::Mosh { host, .. }, "3_2") => *host = value.to_string(),
            (Self::Rsh { user, .. } | Self::Mosh { user, .. }, "3_3") => *user = opt(),
            (Self::Mosh { ssh_port, .. }, "3_4") => {
                *ssh_port = value
                    .parse()
                    .ok()
                    .filter(|&p| p > 0)
                    .ok_or_else(|| format!("invalid port {value:?}"))?;
            }
            (Self::File { path }, "3_2") => *path = value.to_string(),
            (Self::Browser { url }, "3_2") => *url = value.to_string(),
            (Self::AwsS3 { key_id }, "3_2") => *key_id = value.to_string(),
            (Self::Xdmcp { host }, "3_2") => *host = value.to_string(),
            (Self::Wsl { distro }, "3_2") => {
                *distro = if value.is_empty() {
                    default_wsl_distro()
                } else {
                    value.to_string()
                };
            }
            (_, other) => return Err(format!("{other} is not a typed field")),
        }
        Ok(())
    }

    /// Hostname-like target used for default session names.
    pub fn target(&self) -> Option<&str> {
        match self {
            Self::Ssh { host, .. }
            | Self::Telnet { host, .. }
            | Self::Rdp { host, .. }
            | Self::Vnc { host, .. }
            | Self::Ftp { host, .. }
            | Self::Sftp { host, .. } => Some(host.as_str()),
            Self::Serial { port_name, .. } => Some(port_name.as_str()),
            Self::Rsh { host, .. } | Self::Mosh { host, .. } | Self::Xdmcp { host } => {
                Some(host.as_str())
            }
            Self::File { path } => Some(path.as_str()),
            Self::Browser { url } => Some(url.as_str()),
            Self::LocalShell { .. } | Self::Wsl { .. } | Self::AwsS3 { .. } => None,
        }
        .filter(|s| !s.is_empty())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn typed_ids_roundtrip() {
        for index in 0..15 {
            let Some(mut kind) = SessionKind::default_for(index) else {
                continue;
            };
            assert_eq!(kind.type_index(), index);
            for id in kind.typed_ids() {
                let v = kind
                    .typed_get(id)
                    .unwrap_or_else(|| panic!("{index}:{id} get"));
                kind.typed_set(id, &v)
                    .unwrap_or_else(|e| panic!("{index}:{id} set: {e}"));
            }
        }
    }

    #[test]
    fn typed_set_validates() {
        let mut kind = SessionKind::default_for(0).unwrap();
        kind.typed_set("3_2", " box ").unwrap();
        kind.typed_set("3_4", "").unwrap();
        assert!(kind.typed_set("3_3", "0").is_err());
        assert!(kind.typed_set("3_3", "99999").is_err());
        kind.typed_set("3_6", "0").unwrap();
        assert_eq!(
            kind,
            SessionKind::Ssh {
                host: "box".into(),
                port: 22,
                user: None,
                private_key: None,
                x11_forwarding: false
            }
        );
    }
}

/// "Bookmark icon": a built-in icon (optionally tinted) or a small PNG
/// stored in the session so it travels with it ("Select a specific icon
/// for this session").
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct SessionIcon {
    /// Built-in icon name (`"server"`, `"linux"`, …); empty = type default.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub name: String,
    /// Tint as `#rrggbb`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,
    /// Base64 PNG (at most [`SessionIcon::MAX_PNG_BYTES`] bytes decoded).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub png: Option<String>,
}

impl SessionIcon {
    pub const MAX_PNG_BYTES: usize = 64 * 1024;

    /// `#rrggbb` → RGB.
    pub fn rgb(&self) -> Option<[u8; 3]> {
        let hex = self.color.as_deref()?.strip_prefix('#')?;
        if hex.len() != 6 {
            return None;
        }
        let byte = |i: usize| u8::from_str_radix(hex.get(i..i + 2)?, 16).ok();
        Some([byte(0)?, byte(2)?, byte(4)?])
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Session {
    #[serde(default)]
    pub id: SessionId,
    pub name: String,
    /// Slash-separated folder path in the sidebar tree, e.g. `"prod/web"`.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub folder: String,
    /// Unix seconds of the last time the session was opened (home "Recent sessions").
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_used: Option<u64>,
    #[serde(flatten)]
    pub kind: SessionKind,
    /// Remaining parameters by schema id (`"3_7"` = SSH compression, …);
    /// only values that differ from the schema default are stored.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub options: BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub icon: Option<SessionIcon>,
}

impl Session {
    pub fn new(name: impl Into<String>, kind: SessionKind) -> Self {
        Self {
            id: SessionId::new(),
            name: name.into(),
            folder: String::new(),
            last_used: None,
            kind,
            options: BTreeMap::new(),
            icon: None,
        }
    }

    /// Value of a schema parameter: stored option, else the schema default.
    pub fn option<'a>(&'a self, param: &'a crate::schema::Param) -> &'a str {
        self.options
            .get(param.id)
            .map_or(param.default, String::as_str)
    }

    /// Store a parameter, dropping it when it equals the schema default.
    pub fn set_option(&mut self, param: &crate::schema::Param, value: String) {
        if value == param.default {
            self.options.remove(param.id);
        } else {
            self.options.insert(param.id.to_string(), value);
        }
    }
}
