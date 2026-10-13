//! Saved SSH tunnels (reference MobaSSHTunnel): one forwarding rule plus
//! the SSH server that carries it.

use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct TunnelId(pub uuid::Uuid);

impl TunnelId {
    pub fn new() -> Self {
        Self(uuid::Uuid::new_v4())
    }
}

impl Default for TunnelId {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Display for TunnelId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum TunnelKind {
    /// Local clients → forwarded port here → destination via SSH server.
    #[default]
    Local,
    /// Clients of the SSH server → forwarded port there → destination here.
    Remote,
    /// SOCKS proxy here; destinations chosen by the client.
    Dynamic,
}

impl TunnelKind {
    pub fn label(self) -> &'static str {
        match self {
            Self::Local => "Local",
            Self::Remote => "Remote",
            Self::Dynamic => "Dynamic",
        }
    }
}

fn default_bind() -> String {
    "127.0.0.1".into()
}

fn default_ssh_port() -> u16 {
    22
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Tunnel {
    #[serde(default)]
    pub id: TunnelId,
    pub name: String,
    #[serde(default)]
    pub kind: TunnelKind,
    /// Address the forwarded port listens on (here, or on the SSH server
    /// for remote tunnels).
    #[serde(default = "default_bind")]
    pub bind: String,
    pub forward_port: u16,
    /// Destination (unused for dynamic tunnels).
    #[serde(default)]
    pub dest_host: String,
    #[serde(default)]
    pub dest_port: u16,
    pub ssh_host: String,
    #[serde(default = "default_ssh_port")]
    pub ssh_port: u16,
    pub ssh_user: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub private_key: Option<String>,
    /// Start automatically when TermCraft starts.
    #[serde(default)]
    pub autostart: bool,
}

impl Tunnel {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            id: TunnelId::new(),
            name: name.into(),
            kind: TunnelKind::Local,
            bind: default_bind(),
            forward_port: 0,
            dest_host: String::new(),
            dest_port: 0,
            ssh_host: String::new(),
            ssh_port: 22,
            ssh_user: String::new(),
            private_key: None,
            autostart: false,
        }
    }

    /// Problems that prevent starting the tunnel.
    pub fn validate(&self) -> Result<(), String> {
        if self.name.trim().is_empty() {
            return Err("name is required".into());
        }
        if self.ssh_host.trim().is_empty() || self.ssh_user.trim().is_empty() {
            return Err("SSH server and SSH login are required".into());
        }
        if self.forward_port == 0 {
            return Err("forwarded port is required".into());
        }
        if self.kind != TunnelKind::Dynamic
            && (self.dest_host.trim().is_empty() || self.dest_port == 0)
        {
            return Err("destination server and port are required".into());
        }
        Ok(())
    }

    pub fn destination(&self) -> String {
        match self.kind {
            TunnelKind::Dynamic => "(SOCKS: any)".into(),
            _ => format!("{}:{}", self.dest_host, self.dest_port),
        }
    }

    pub fn ssh_server(&self) -> String {
        format!("{}@{}:{}", self.ssh_user, self.ssh_host, self.ssh_port)
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TunnelStore {
    #[serde(default, rename = "tunnel")]
    pub tunnels: Vec<Tunnel>,
}

impl TunnelStore {
    pub fn load(path: &Path) -> Result<Self, crate::StoreError> {
        crate::store::load_toml(path)
    }

    pub fn save(&self, path: &Path) -> Result<(), crate::StoreError> {
        crate::store::save_toml(path, self)
    }

    pub fn get(&self, id: TunnelId) -> Option<&Tunnel> {
        self.tunnels.iter().find(|t| t.id == id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_and_validation() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("tunnels.toml");
        let mut t = Tunnel::new("db");
        assert!(t.validate().is_err());
        t.forward_port = 15432;
        t.dest_host = "db.internal".into();
        t.dest_port = 5432;
        t.ssh_host = "bastion".into();
        t.ssh_user = "me".into();
        t.validate().unwrap();
        let mut socks = Tunnel::new("socks");
        socks.kind = TunnelKind::Dynamic;
        socks.forward_port = 1080;
        socks.ssh_host = "bastion".into();
        socks.ssh_user = "me".into();
        socks.validate().unwrap();

        let store = TunnelStore {
            tunnels: vec![t, socks],
        };
        store.save(&path).unwrap();
        assert_eq!(TunnelStore::load(&path).unwrap(), store);
    }
}
