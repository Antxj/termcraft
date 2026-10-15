//! "SSH key generator" (MobaKeyGen / PuTTYgen-style): generate Ed25519,
//! ECDSA or RSA keys, load existing ones (OpenSSH or PuTTY `.ppk`), and
//! save the private key as OpenSSH or PPK v3 (optionally with a passphrase:
//! OpenSSH bcrypt-pbkdf, or PPK Argon2id + AES-256-CBC).

use base64::Engine as _;
use russh::keys::ssh_key::private::{EcdsaKeypair, KeypairData, RsaKeypair};
use russh::keys::ssh_key::{self, Algorithm, EcdsaCurve, HashAlg, LineEnding, Mpint, PrivateKey};
use ssh_key::encoding::Encode;
use zeroize::Zeroizing;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyKind {
    Ed25519,
    EcdsaP256,
    EcdsaP384,
    EcdsaP521,
    Rsa(usize),
}

impl KeyKind {
    pub fn label(self) -> String {
        match self {
            Self::Ed25519 => "EdDSA (Ed25519)".into(),
            Self::EcdsaP256 => "ECDSA (nistp256)".into(),
            Self::EcdsaP384 => "ECDSA (nistp384)".into(),
            Self::EcdsaP521 => "ECDSA (nistp521)".into(),
            Self::Rsa(bits) => format!("RSA ({bits} bits)"),
        }
    }
}

#[derive(Debug)]
pub enum LoadError {
    /// The key is encrypted: ask for its passphrase.
    NeedsPassphrase,
    Invalid(String),
}

pub fn generate(kind: KeyKind, comment: &str) -> Result<PrivateKey, String> {
    let mut rng = russh::keys::key::safe_rng();
    let err = |e: ssh_key::Error| e.to_string();
    let key = match kind {
        KeyKind::Ed25519 => PrivateKey::random(&mut rng, Algorithm::Ed25519).map_err(err)?,
        KeyKind::EcdsaP256 | KeyKind::EcdsaP384 | KeyKind::EcdsaP521 => {
            let curve = match kind {
                KeyKind::EcdsaP256 => EcdsaCurve::NistP256,
                KeyKind::EcdsaP384 => EcdsaCurve::NistP384,
                _ => EcdsaCurve::NistP521,
            };
            let pair = EcdsaKeypair::random(&mut rng, curve).map_err(err)?;
            PrivateKey::new(KeypairData::from(pair), comment).map_err(err)?
        }
        KeyKind::Rsa(bits) => {
            if !(2048..=8192).contains(&bits) {
                return Err("RSA keys must have 2048 to 8192 bits".into());
            }
            let pair = RsaKeypair::random(&mut rng, bits).map_err(err)?;
            PrivateKey::new(KeypairData::from(pair), comment).map_err(err)?
        }
    };
    let mut key = key;
    key.set_comment(comment);
    Ok(key)
}

/// Load an OpenSSH or PPK private key from its text.
pub fn load(text: &str, passphrase: Option<&str>) -> Result<PrivateKey, LoadError> {
    let invalid = |e: ssh_key::Error| LoadError::Invalid(e.to_string());
    if text.trim_start().starts_with("PuTTY-User-Key-File-") {
        let encrypted = text.lines().any(|l| l.trim() == "Encryption: aes256-cbc");
        if encrypted && passphrase.is_none() {
            return Err(LoadError::NeedsPassphrase);
        }
        // Ed25519 and RSA in format 3 are read here (the ssh-key reader
        // rejects some Ed25519 keys and has no RSA); the rest goes to it.
        if let Some(key) = read_ppk3(text, passphrase)? {
            return Ok(key);
        }
        return PrivateKey::from_ppk(text, passphrase.map(String::from)).map_err(invalid);
    }
    let key = PrivateKey::from_openssh(text).map_err(invalid)?;
    if key.is_encrypted() {
        let Some(pass) = passphrase else {
            return Err(LoadError::NeedsPassphrase);
        };
        return key
            .decrypt(pass)
            .map_err(|_| LoadError::Invalid("wrong passphrase".into()));
    }
    Ok(key)
}

/// The algorithm and SHA-256 fingerprint of a private key file's public
/// part, without its passphrase (OpenSSH keys keep it in clear; PPK files
/// have "Public-Lines").
pub fn describe_public(text: &str) -> Option<String> {
    let public = if text.trim_start().starts_with("PuTTY-User-Key-File-") {
        let mut lines = text.lines().map(str::trim_end);
        let mut blob = None;
        while let Some(line) = lines.next() {
            if let Some(n) = line.strip_prefix("Public-Lines: ") {
                let n: usize = n.trim().parse().ok().filter(|n| *n <= 1000)?;
                let b64: String = lines.by_ref().take(n).collect();
                blob = base64::engine::general_purpose::STANDARD.decode(b64).ok();
                break;
            }
        }
        ssh_key::PublicKey::from_bytes(&blob?).ok()?
    } else {
        PrivateKey::from_openssh(text).ok()?.public_key().clone()
    };
    Some(format!(
        "{} {}",
        public.algorithm(),
        public.fingerprint(HashAlg::Sha256)
    ))
}

/// "Public key for pasting into OpenSSH authorized_keys file".
pub fn public_line(key: &PrivateKey) -> String {
    key.public_key().to_openssh().unwrap_or_default()
}

pub fn fingerprint(key: &PrivateKey) -> String {
    key.public_key().fingerprint(HashAlg::Sha256).to_string()
}

/// OpenSSH private key file, encrypted when a passphrase is given.
pub fn to_openssh(key: &PrivateKey, passphrase: Option<&str>) -> Result<Zeroizing<String>, String> {
    let key = match passphrase.filter(|p| !p.is_empty()) {
        Some(p) => key
            .encrypt(&mut russh::keys::key::safe_rng(), p)
            .map_err(|e| e.to_string())?,
        None => key.clone(),
    };
    key.to_openssh(LineEnding::LF).map_err(|e| e.to_string())
}

/// Argon2id cost for PPK files (PuTTYgen-like: 8 MiB, single lane).
const PPK_ARGON2_MEMORY: u32 = 8192;
const PPK_ARGON2_PASSES: u32 = 13;

fn base64_lines(bytes: &[u8]) -> Vec<String> {
    let b64 = base64::engine::general_purpose::STANDARD.encode(bytes);
    b64.as_bytes()
        .chunks(64)
        .map(|c| String::from_utf8_lossy(c).into_owned())
        .collect()
}

fn mpint(bytes: &[u8], out: &mut Vec<u8>) -> Result<(), String> {
    Mpint::from_positive_bytes(bytes)
        .encode(out)
        .map_err(|e| e.to_string())
}

/// PuTTY's private part of the key: Ed25519 / ECDSA keep the private value
/// as an mpint; RSA keeps d, p, q, iqmp.
fn ppk_private_blob(key: &PrivateKey) -> Result<Zeroizing<Vec<u8>>, String> {
    let mut out = Zeroizing::new(Vec::new());
    match key.key_data() {
        KeypairData::Ed25519(k) => mpint(k.private.as_ref(), &mut out)?,
        KeypairData::Ecdsa(k) => mpint(k.private_key_bytes(), &mut out)?,
        KeypairData::Rsa(k) => {
            let r = k.private();
            for v in [r.d(), r.p(), r.q(), r.iqmp()] {
                v.encode(&mut *out).map_err(|e| e.to_string())?;
            }
        }
        _ => return Err("this key type cannot be saved as PPK".into()),
    }
    Ok(out)
}

/// PuTTY private key file, format 3.
pub fn to_ppk(key: &PrivateKey, passphrase: Option<&str>) -> Result<Zeroizing<String>, String> {
    use hmac::{Hmac, KeyInit, Mac};
    let algorithm = key.algorithm().to_string();
    let comment = key.comment().to_string();
    if comment.contains(['\r', '\n']) {
        return Err("the comment must be on one line".into());
    }
    let public = key.public_key().to_bytes().map_err(|e| e.to_string())?;
    let mut private = ppk_private_blob(key)?;
    let passphrase = passphrase.filter(|p| !p.is_empty());

    let mut header_kdf = String::new();
    let (encryption, mac_key, cipher_key) = match passphrase {
        None => ("none", Zeroizing::new(Vec::new()), None),
        Some(p) => {
            let salt: [u8; 16] = rand_bytes();
            let params = argon2::Params::new(PPK_ARGON2_MEMORY, PPK_ARGON2_PASSES, 1, Some(80))
                .map_err(|e| e.to_string())?;
            let mut material = Zeroizing::new([0u8; 80]);
            argon2::Argon2::new(argon2::Algorithm::Argon2id, argon2::Version::V0x13, params)
                .hash_password_into(p.as_bytes(), &salt, material.as_mut())
                .map_err(|e| e.to_string())?;
            header_kdf = format!(
                "Key-Derivation: Argon2id\nArgon2-Memory: {PPK_ARGON2_MEMORY}\n\
                 Argon2-Passes: {PPK_ARGON2_PASSES}\nArgon2-Parallelism: 1\nArgon2-Salt: {}\n",
                hex::encode(salt)
            );
            // Pad to the cipher block size with random bytes.
            let pad = (16 - private.len() % 16) % 16;
            let filler: [u8; 16] = rand_bytes();
            private.extend_from_slice(&filler[..pad]);
            (
                "aes256-cbc",
                Zeroizing::new(material[48..80].to_vec()),
                Some(Zeroizing::new((
                    material[..32].to_vec(),
                    material[32..48].to_vec(),
                ))),
            )
        }
    };

    // MAC over the plaintext: algorithm, encryption, comment, public, private.
    let mut mac_data = Zeroizing::new(Vec::new());
    for field in [
        algorithm.as_bytes(),
        encryption.as_bytes(),
        comment.as_bytes(),
        &public,
        &private,
    ] {
        field.encode(&mut *mac_data).map_err(|e| e.to_string())?;
    }
    let mut mac =
        <Hmac<sha2::Sha256> as KeyInit>::new_from_slice(&mac_key).map_err(|e| e.to_string())?;
    mac.update(&mac_data);
    let mac = hex::encode(mac.finalize().into_bytes());

    if let Some(keys) = cipher_key {
        let (key, iv) = &*keys;
        ssh_key::Cipher::Aes256Cbc
            .encrypt(key, iv, &mut private)
            .map_err(|e| e.to_string())?;
    }

    let public_lines = base64_lines(&public);
    let private_lines = base64_lines(&private);
    let mut out = Zeroizing::new(format!(
        "PuTTY-User-Key-File-3: {algorithm}\nEncryption: {encryption}\nComment: {comment}\n\
         Public-Lines: {}\n",
        public_lines.len()
    ));
    for l in &public_lines {
        out.push_str(l);
        out.push('\n');
    }
    out.push_str(&header_kdf);
    out.push_str(&format!("Private-Lines: {}\n", private_lines.len()));
    for l in &private_lines {
        out.push_str(l);
        out.push('\n');
    }
    out.push_str(&format!("Private-MAC: {mac}\n"));
    Ok(out)
}

/// PPK format 3 with Ed25519 or RSA: `Ok(None)` for anything else.
fn read_ppk3(text: &str, passphrase: Option<&str>) -> Result<Option<PrivateKey>, LoadError> {
    use hmac::{Hmac, KeyInit, Mac};
    use ssh_key::encoding::Decode;
    use ssh_key::private::{Ed25519Keypair, Ed25519PrivateKey, RsaPrivateKey};
    let bad = |m: &str| LoadError::Invalid(format!("PPK: {m}"));
    let mut lines = text.lines().map(str::trim_end);
    let mut header = std::collections::HashMap::new();
    let mut blobs: Vec<(String, Vec<u8>)> = Vec::new();
    let mut algorithm = String::new();
    while let Some(line) = lines.next() {
        let Some((key, value)) = line.split_once(": ") else {
            continue;
        };
        if let Some(alg) = key.strip_prefix("PuTTY-User-Key-File-") {
            if alg != "3" {
                return Ok(None);
            }
            algorithm = value.to_string();
            continue;
        }
        if key == "Public-Lines" || key == "Private-Lines" {
            let n: usize = value.parse().map_err(|_| bad("line count"))?;
            if n > 1000 {
                return Err(bad("too many lines"));
            }
            let b64: String = (&mut lines).take(n).collect();
            let bytes = base64::engine::general_purpose::STANDARD
                .decode(b64)
                .map_err(|_| bad("base64"))?;
            blobs.push((key.to_string(), bytes));
            continue;
        }
        header.insert(key.to_string(), value.to_string());
    }
    if algorithm != "ssh-ed25519" && algorithm != "ssh-rsa" {
        return Ok(None);
    }
    let blob = |name: &str| {
        blobs
            .iter()
            .find(|(k, _)| k == name)
            .map(|(_, b)| b.clone())
            .ok_or_else(|| bad("missing key data"))
    };
    let public = blob("Public-Lines")?;
    let mut private = Zeroizing::new(blob("Private-Lines")?);
    let encryption = header.get("Encryption").map_or("none", String::as_str);
    let comment = header.get("Comment").cloned().unwrap_or_default();
    let mac_key = match encryption {
        "none" => Zeroizing::new(Vec::new()),
        "aes256-cbc" => {
            let pass = passphrase.ok_or(LoadError::NeedsPassphrase)?;
            if header.get("Key-Derivation").map(String::as_str) != Some("Argon2id") {
                return Ok(None);
            }
            let num = |k: &str| -> Result<u32, LoadError> {
                header
                    .get(k)
                    .and_then(|v| v.parse().ok())
                    .ok_or_else(|| bad(k))
            };
            let salt = hex::decode(header.get("Argon2-Salt").ok_or_else(|| bad("salt"))?)
                .map_err(|_| bad("salt"))?;
            let (memory, passes, lanes) = (
                num("Argon2-Memory")?,
                num("Argon2-Passes")?,
                num("Argon2-Parallelism")?,
            );
            // Refuse costs that would hang the app (PuTTYgen uses 8 MiB).
            if memory > 1024 * 1024 || passes > 1000 || lanes > 16 {
                return Err(bad("key derivation cost too high"));
            }
            let params =
                argon2::Params::new(memory, passes, lanes, Some(80)).map_err(|_| bad("argon2"))?;
            let mut material = Zeroizing::new([0u8; 80]);
            argon2::Argon2::new(argon2::Algorithm::Argon2id, argon2::Version::V0x13, params)
                .hash_password_into(pass.as_bytes(), &salt, material.as_mut())
                .map_err(|_| bad("argon2"))?;
            if private.len() % 16 != 0 {
                return Err(bad("private data length"));
            }
            ssh_key::Cipher::Aes256Cbc
                .decrypt(&material[..32], &material[32..48], &mut private, None)
                .map_err(|_| bad("decryption"))?;
            Zeroizing::new(material[48..80].to_vec())
        }
        _ => return Ok(None),
    };
    let mut mac_data = Zeroizing::new(Vec::new());
    for field in [
        algorithm.as_bytes(),
        encryption.as_bytes(),
        comment.as_bytes(),
        &public,
        &private,
    ] {
        field.encode(&mut *mac_data).map_err(|_| bad("encoding"))?;
    }
    let expected = hex::decode(
        header
            .get("Private-MAC")
            .ok_or_else(|| bad("missing MAC"))?,
    )
    .map_err(|_| bad("MAC"))?;
    let mut mac =
        <Hmac<sha2::Sha256> as KeyInit>::new_from_slice(&mac_key).map_err(|_| bad("MAC"))?;
    mac.update(&mac_data);
    if mac.verify_slice(&expected).is_err() {
        return Err(LoadError::Invalid(if encryption == "none" {
            "the key file is damaged".into()
        } else {
            "wrong passphrase".into()
        }));
    }
    let public_key = ssh_key::PublicKey::from_bytes(&public).map_err(|_| bad("public key"))?;
    let mut reader = &private[..];
    let data = match public_key.key_data() {
        ssh_key::public::KeyData::Ed25519(pk) => {
            let e = Mpint::decode(&mut reader).map_err(|_| bad("private key"))?;
            let bytes = e.as_positive_bytes().ok_or_else(|| bad("private key"))?;
            if bytes.len() > 32 {
                return Err(bad("private key"));
            }
            let mut seed = Zeroizing::new([0u8; 32]);
            seed[32 - bytes.len()..].copy_from_slice(bytes);
            KeypairData::Ed25519(Ed25519Keypair {
                public: *pk,
                private: Ed25519PrivateKey::from_bytes(&seed),
            })
        }
        ssh_key::public::KeyData::Rsa(pk) => {
            let mut next = || Mpint::decode(&mut reader).map_err(|_| bad("private key"));
            let (d, p, q, iqmp) = (next()?, next()?, next()?, next()?);
            let private = RsaPrivateKey::new(d, iqmp, p, q).map_err(|_| bad("private key"))?;
            KeypairData::Rsa(RsaKeypair::new(pk.clone(), private).map_err(|_| bad("private key"))?)
        }
        _ => return Ok(None),
    };
    let key = PrivateKey::new(data, comment).map_err(|e| LoadError::Invalid(e.to_string()))?;
    // The private part must match the public one.
    if key.public_key().key_data() != public_key.key_data() {
        return Err(bad("public and private parts do not match"));
    }
    Ok(Some(key))
}

fn rand_bytes<const N: usize>() -> [u8; N] {
    use rand_core::Rng as _;
    let mut b = [0u8; N];
    russh::keys::key::safe_rng().fill_bytes(&mut b);
    b
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn public_part_without_the_passphrase() {
        let key = generate(KeyKind::Ed25519, "me").unwrap();
        let expected = format!("ssh-ed25519 {}", fingerprint(&key));
        let text = to_openssh(&key, Some("pw")).unwrap();
        assert_eq!(describe_public(&text), Some(expected.clone()));
        let ppk = to_ppk(&key, Some("pw")).unwrap();
        assert_eq!(describe_public(&ppk), Some(expected));
        assert_eq!(describe_public("not a key"), None);
    }

    #[test]
    fn ed25519_round_trips_through_openssh_and_ppk() {
        let key = generate(KeyKind::Ed25519, "me@box").unwrap();
        assert!(public_line(&key).starts_with("ssh-ed25519 AAAA"));
        assert!(public_line(&key).ends_with(" me@box"));
        assert!(fingerprint(&key).starts_with("SHA256:"));

        for pass in [None, Some("secret pass")] {
            let ssh = to_openssh(&key, pass).unwrap();
            let back = match pass {
                None => load(&ssh, None).unwrap(),
                Some(p) => {
                    assert!(matches!(load(&ssh, None), Err(LoadError::NeedsPassphrase)));
                    load(&ssh, Some(p)).unwrap()
                }
            };
            assert_eq!(back.public_key().key_data(), key.public_key().key_data());

            let ppk = to_ppk(&key, pass).unwrap();
            assert!(ppk.starts_with("PuTTY-User-Key-File-3: ssh-ed25519\n"));
            let back = load(&ppk, pass).unwrap();
            assert_eq!(back.key_data(), key.key_data(), "private part survives");
            assert_eq!(back.comment().to_string(), "me@box");
        }
        let ppk = to_ppk(&key, Some("right")).unwrap();
        assert!(matches!(load(&ppk, None), Err(LoadError::NeedsPassphrase)));
        assert!(load(&ppk, Some("wrong")).is_err(), "MAC check");
    }

    #[test]
    fn ecdsa_ppk_round_trip() {
        let key = generate(KeyKind::EcdsaP256, "ec").unwrap();
        let ppk = to_ppk(&key, None).unwrap();
        let back = load(&ppk, None).unwrap();
        assert_eq!(back.key_data(), key.key_data());
    }

    /// Ed25519 seeds with the top bit set need the reader above.
    #[test]
    fn many_ed25519_keys_round_trip_through_ppk() {
        for _ in 0..16 {
            let key = generate(KeyKind::Ed25519, "k").unwrap();
            let back = load(&to_ppk(&key, None).unwrap(), None).unwrap();
            assert_eq!(back.key_data(), key.key_data());
        }
    }

    #[test]
    fn rsa_ppk_round_trip() {
        let key = generate(KeyKind::Rsa(2048), "rsa").unwrap();
        for pass in [None, Some("p")] {
            let back = load(&to_ppk(&key, pass).unwrap(), pass).unwrap();
            assert_eq!(back.key_data(), key.key_data());
        }
    }

    #[test]
    fn rsa_bits_are_checked() {
        assert!(generate(KeyKind::Rsa(1024), "x").is_err());
        assert!(to_ppk(&generate(KeyKind::Ed25519, "a\nb").unwrap(), None).is_err());
    }
}
