use serde::{Deserialize, Serialize};
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
        }
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
}

impl Session {
    pub fn new(name: impl Into<String>, kind: SessionKind) -> Self {
        Self {
            id: SessionId::new(),
            name: name.into(),
            folder: String::new(),
            last_used: None,
            kind,
        }
    }
}
