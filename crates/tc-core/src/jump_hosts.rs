//! SSH jump hosts of a session (Network settings: gateway server, port,
//! login and private key). Several hops are stored in the same four
//! values, one entry per hop, separated by `__PIPE__` (a typed `|` is
//! accepted too). The first hop is reached first.

use std::collections::BTreeMap;

/// Separator between hops inside one value.
pub const SEPARATOR: &str = "__PIPE__";

/// Schema ids of the four gateway values of a session type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GatewayIds {
    pub host: &'static str,
    pub port: &'static str,
    pub user: &'static str,
    pub key: &'static str,
}

/// SSH sessions: 3_9 / 3_10 / 3_11 / 3_16.
pub const SSH: GatewayIds = GatewayIds {
    host: "3_9",
    port: "3_10",
    user: "3_11",
    key: "3_16",
};

/// The gateway ids of a session type's Network settings (SSH, Telnet,
/// RDP and VNC have them, under different ids), found by label.
pub fn ids_for(type_index: usize) -> Option<GatewayIds> {
    let params = crate::schema::network_params(type_index);
    let id = |label: &str| params.iter().find(|p| p.label == label).map(|p| p.id);
    Some(GatewayIds {
        host: id("SSH gateway server")?,
        port: id("SSH gateway port number")?,
        user: id("SSH gateway login")?,
        key: id("SSH gateway private key")?,
    })
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct JumpHost {
    pub host: String,
    /// As typed; empty = 22.
    pub port: String,
    /// Empty = the session's user.
    pub user: String,
    /// Empty = no key file.
    pub key: String,
}

impl JumpHost {
    pub fn port_number(&self) -> u16 {
        self.port.trim().parse().unwrap_or(22)
    }
}

fn split(value: &str) -> Vec<&str> {
    value
        .split(SEPARATOR)
        .flat_map(|v| v.split('|'))
        .map(str::trim)
        .collect()
}

/// The hops stored in a session's options (hops without a host are
/// skipped).
pub fn parse(options: &BTreeMap<String, String>, ids: GatewayIds) -> Vec<JumpHost> {
    let get = |id: &str| options.get(id).map(String::as_str).unwrap_or("");
    let (hosts, ports, users, keys) = (
        split(get(ids.host)),
        split(get(ids.port)),
        split(get(ids.user)),
        split(get(ids.key)),
    );
    let at = |v: &[&str], i: usize| v.get(i).copied().unwrap_or("").to_string();
    hosts
        .iter()
        .enumerate()
        .filter(|(_, h)| !h.is_empty())
        .map(|(i, h)| JumpHost {
            host: h.to_string(),
            port: at(&ports, i),
            user: at(&users, i),
            key: at(&keys, i),
        })
        .collect()
}

/// Store the hops back (empty values when there is none).
pub fn store(options: &mut BTreeMap<String, String>, ids: GatewayIds, hops: &[JumpHost]) {
    let hops: Vec<&JumpHost> = hops.iter().filter(|h| !h.host.trim().is_empty()).collect();
    let join = |f: &dyn Fn(&JumpHost) -> &str| -> String {
        hops.iter()
            .map(|h| f(h).trim().replace('|', ""))
            .collect::<Vec<_>>()
            .join(SEPARATOR)
    };
    let values = [
        (ids.host, join(&|h| &h.host)),
        (ids.port, join(&|h| &h.port)),
        (ids.user, join(&|h| &h.user)),
        (ids.key, join(&|h| &h.key)),
    ];
    for (id, mut value) in values {
        // No hop, or one on the default port: keep the reference default.
        if id == ids.port && hops.len() <= 1 && value.is_empty() {
            value = "22".to_string();
        }
        options.insert(id.to_string(), value);
    }
}

/// "gw1 > gw2" summary for the session dialog button.
pub fn summary(hops: &[JumpHost]) -> String {
    hops.iter()
        .map(|h| {
            let user = if h.user.is_empty() {
                String::new()
            } else {
                format!("{}@", h.user)
            };
            match h.port_number() {
                22 => format!("{user}{}", h.host),
                p => format!("{user}{}:{p}", h.host),
            }
        })
        .collect::<Vec<_>>()
        .join(" > ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn single_and_several_hops() {
        let mut o = BTreeMap::new();
        assert!(parse(&o, SSH).is_empty());
        o.insert("3_9".into(), "gw.example".into());
        o.insert("3_10".into(), "22".into());
        let hops = parse(&o, SSH);
        assert_eq!(hops.len(), 1);
        assert_eq!(hops[0].port_number(), 22);

        o.insert("3_9".into(), "a__PIPE__b|c".into());
        o.insert("3_10".into(), "2222__PIPE__".into());
        o.insert("3_11".into(), "__PIPE__bob".into());
        let hops = parse(&o, SSH);
        assert_eq!(hops.len(), 3);
        assert_eq!((hops[0].port_number(), hops[0].user.as_str()), (2222, ""));
        assert_eq!((hops[1].port_number(), hops[1].user.as_str()), (22, "bob"));
        assert_eq!(hops[2].host, "c");
        assert_eq!(summary(&hops), "a:2222 > bob@b > c");
    }

    #[test]
    fn store_round_trip() {
        let hops = vec![
            JumpHost {
                host: "a".into(),
                port: "2222".into(),
                ..Default::default()
            },
            JumpHost {
                host: " ".into(),
                ..Default::default()
            },
            JumpHost {
                host: "b".into(),
                user: "bob".into(),
                key: "C:\\k.ppk".into(),
                ..Default::default()
            },
        ];
        let mut o = BTreeMap::new();
        store(&mut o, SSH, &hops);
        assert_eq!(o["3_9"], "a__PIPE__b");
        assert_eq!(o["3_10"], "2222__PIPE__");
        assert_eq!(o["3_11"], "__PIPE__bob");
        let back = parse(&o, SSH);
        assert_eq!(back.len(), 2);
        assert_eq!(back[1].key, "C:\\k.ppk");

        store(&mut o, SSH, &[]);
        assert!(parse(&o, SSH).is_empty());
        assert_eq!(o["3_10"], "22");
    }

    #[test]
    fn ids_from_the_schema() {
        assert_eq!(ids_for(0), Some(SSH));
        let telnet = ids_for(1).unwrap();
        assert_eq!((telnet.host, telnet.key), ("3_7", "3_10"));
        assert_eq!(ids_for(10), None, "Shell has no gateway");
    }
}
