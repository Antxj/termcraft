//! "Expert SSH settings": the order of key exchange algorithms, host key
//! types and ciphers. Each list holds the reference group names (ECDH,
//! AES …), most wanted first; what sits below "=Warn below=" is not
//! offered at all. An empty list keeps the built-in order.

use russh::keys::{Algorithm, EcdsaCurve, HashAlg};
use russh::{Preferred, cipher, kex};
use std::borrow::Cow;

/// The line under which algorithms are not offered.
pub const WARN_BELOW: &str = "=Warn below=";

/// The reference groups, in their default order.
pub const KEX_GROUPS: &[&str] = &[
    "ECDH",
    "NTRU-Curve25519",
    "ML-KEM-Curve25519",
    "ML-KEM-Nist-ECDH",
    "dh-gex-sha1",
    "dh-group18-sha512",
    "dh-group17-sha512",
    "dh-group16-sha512",
    "dh-group15-sha512",
    "dh-group14-sha1",
    "RSA",
    WARN_BELOW,
    "dh-group1-sha1",
];
pub const HOST_KEY_GROUPS: &[&str] = &["Ed448", "Ed25519", "ECDSA", "RSA", "DSA", WARN_BELOW];
pub const CIPHER_GROUPS: &[&str] = &[
    "AES", "ChaCha20", "AESGCM", "3DES", WARN_BELOW, "DES", "Blowfish", "Arcfour",
];

/// The three lists, as group names in order (above the warn line).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Algorithms {
    pub kex: Vec<String>,
    pub host_keys: Vec<String>,
    pub ciphers: Vec<String>,
    /// 3_33 "Prefer hostkey algorithms for which a hostkey is already
    /// known": the key types saved in known_hosts for the server go first.
    pub prefer_known: bool,
}

/// Same key family (RSA signatures differ only by hash).
fn same_family(a: &Algorithm, b: &Algorithm) -> bool {
    match (a, b) {
        (Algorithm::Rsa { .. }, Algorithm::Rsa { .. }) => true,
        _ => a == b,
    }
}

/// Move the host key types of `known` keys to the front, keeping the
/// order otherwise.
pub fn prefer_known(p: &mut Preferred, known: &[Algorithm]) {
    if known.is_empty() {
        return;
    }
    let (mut first, rest): (Vec<Algorithm>, Vec<Algorithm>) = p
        .key
        .iter()
        .cloned()
        .partition(|a| known.iter().any(|k| same_family(a, k)));
    first.extend(rest);
    p.key = Cow::Owned(first);
}

/// A stored list (`ECDH,AES,…`): the groups above the warn line, in
/// order. Unknown names are dropped; nothing known = the built-in order.
pub fn parse_list(value: &str, groups: &[&str]) -> Vec<String> {
    value
        .split(',')
        .map(str::trim)
        .take_while(|n| *n != WARN_BELOW)
        .filter(|n| groups.contains(n))
        .map(String::from)
        .collect()
}

fn kex_names(group: &str) -> &'static [kex::Name] {
    match group {
        "ECDH" => &[
            kex::CURVE25519,
            kex::CURVE25519_PRE_RFC_8731,
            kex::ECDH_SHA2_NISTP256,
            kex::ECDH_SHA2_NISTP384,
            kex::ECDH_SHA2_NISTP521,
        ],
        "ML-KEM-Curve25519" => &[kex::MLKEM768X25519_SHA256],
        "dh-gex-sha1" => &[kex::DH_GEX_SHA256, kex::DH_GEX_SHA1],
        "dh-group18-sha512" => &[kex::DH_G18_SHA512],
        "dh-group17-sha512" => &[kex::DH_G17_SHA512],
        "dh-group16-sha512" => &[kex::DH_G16_SHA512],
        "dh-group15-sha512" => &[kex::DH_G15_SHA512],
        "dh-group14-sha1" => &[kex::DH_G14_SHA256, kex::DH_G14_SHA1],
        "dh-group1-sha1" => &[kex::DH_G1_SHA1],
        _ => &[],
    }
}

fn host_key_algorithms(group: &str) -> Vec<Algorithm> {
    match group {
        "Ed25519" => vec![Algorithm::Ed25519],
        "ECDSA" => [
            EcdsaCurve::NistP256,
            EcdsaCurve::NistP384,
            EcdsaCurve::NistP521,
        ]
        .into_iter()
        .map(|curve| Algorithm::Ecdsa { curve })
        .collect(),
        "RSA" => vec![
            Algorithm::Rsa {
                hash: Some(HashAlg::Sha512),
            },
            Algorithm::Rsa {
                hash: Some(HashAlg::Sha256),
            },
            Algorithm::Rsa { hash: None },
        ],
        _ => Vec::new(),
    }
}

fn cipher_names(group: &str) -> &'static [cipher::Name] {
    match group {
        "AES" => &[
            cipher::AES_256_CTR,
            cipher::AES_192_CTR,
            cipher::AES_128_CTR,
        ],
        "ChaCha20" => &[cipher::CHACHA20_POLY1305],
        "AESGCM" => &[cipher::AES_256_GCM, cipher::AES_128_GCM],
        _ => &[],
    }
}

/// The negotiation preferences for these lists.
pub fn preferred(a: &Algorithms) -> Preferred {
    let mut p = Preferred::default();
    let kex: Vec<kex::Name> = a.kex.iter().flat_map(|g| kex_names(g)).copied().collect();
    if !kex.is_empty() {
        let mut all = kex;
        // Extension markers, not algorithms: always sent.
        all.extend([
            kex::EXTENSION_SUPPORT_AS_CLIENT,
            kex::EXTENSION_OPENSSH_STRICT_KEX_AS_CLIENT,
        ]);
        p.kex = Cow::Owned(all);
    }
    let keys: Vec<Algorithm> = a
        .host_keys
        .iter()
        .flat_map(|g| host_key_algorithms(g))
        .collect();
    if !keys.is_empty() {
        p.key = Cow::Owned(keys);
    }
    let ciphers: Vec<cipher::Name> = a
        .ciphers
        .iter()
        .flat_map(|g| cipher_names(g))
        .copied()
        .collect();
    if !ciphers.is_empty() {
        p.cipher = Cow::Owned(ciphers);
    }
    p
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_key_types_go_first() {
        let mut p = Preferred::default();
        prefer_known(&mut p, &[Algorithm::Rsa { hash: None }]);
        assert!(matches!(p.key[0], Algorithm::Rsa { .. }));
        assert!(p.key.contains(&Algorithm::Ed25519), "nothing removed");
        let before = Preferred::default().key.len();
        assert_eq!(p.key.len(), before);
    }

    #[test]
    fn lists_stop_at_the_warn_line() {
        let l = parse_list("AESGCM, AES,=Warn below=,3DES,bogus", CIPHER_GROUPS);
        assert_eq!(l, ["AESGCM", "AES"]);
        assert!(parse_list("", CIPHER_GROUPS).is_empty());
        assert!(
            parse_list("1,2,3", KEX_GROUPS).is_empty(),
            "unknown format = defaults"
        );
    }

    #[test]
    fn preferences_follow_the_lists() {
        let defaults = Preferred::default();
        assert_eq!(preferred(&Algorithms::default()).kex, defaults.kex);
        let a = Algorithms {
            kex: vec!["dh-group16-sha512".into(), "ECDH".into()],
            host_keys: vec!["RSA".into(), "Ed25519".into()],
            ciphers: vec!["AESGCM".into()],
            prefer_known: false,
        };
        let p = preferred(&a);
        assert_eq!(p.kex[0], kex::DH_G16_SHA512);
        assert_eq!(p.kex[1], kex::CURVE25519);
        assert!(p.kex.contains(&kex::EXTENSION_OPENSSH_STRICT_KEX_AS_CLIENT));
        assert!(matches!(p.key[0], Algorithm::Rsa { .. }));
        assert_eq!(p.key.last(), Some(&Algorithm::Ed25519));
        assert_eq!(&p.cipher[..], [cipher::AES_256_GCM, cipher::AES_128_GCM]);
        // Groups TermCraft has no algorithm for fall back to the defaults.
        let none = Algorithms {
            kex: vec!["NTRU-Curve25519".into()],
            ..Algorithms::default()
        };
        assert_eq!(preferred(&none).kex, defaults.kex);
    }
}
