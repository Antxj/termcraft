//! Self-update.
//!
//! Release CI publishes, per channel, `update-<channel>.json` plus a detached
//! ed25519 signature `update-<channel>.json.sig` (base64). The manifest lists
//! one asset per platform with its SHA-256. The client:
//!
//! 1. downloads manifest + signature and verifies the signature against a
//!    public key compiled into the binary (`TERMCRAFT_UPDATE_PUBKEY`);
//! 2. rejects manifests for another channel and versions that are not newer;
//! 3. downloads the asset for this platform, checking size and SHA-256;
//! 4. swaps the running executable in place (`self-replace`), which works the
//!    same for the portable zip and for installed copies.
//!
//! macOS `.app` bundles must be replaced as a whole and are not handled yet.

use base64::Engine as _;
use ed25519_dalek::{Signature, VerifyingKey};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::io::{Read, Write};
use std::path::Path;

/// Base64 ed25519 public key(s), comma-separated, injected at build time by
/// the release pipeline. Unset in dev builds → updates disabled.
pub const BUILD_PUBLIC_KEYS: Option<&str> = option_env!("TERMCRAFT_UPDATE_PUBKEY");
/// Where update manifests are fetched from; the release pipeline sets
/// `TERMCRAFT_UPDATE_BASE_URL` to the repository's latest-release URL.
pub const DEFAULT_BASE_URL: &str = match option_env!("TERMCRAFT_UPDATE_BASE_URL") {
    Some(url) => url,
    None => "https://github.com/Antxj/termcraft/releases/latest/download",
};
const MAX_MANIFEST_BYTES: u64 = 256 * 1024;

#[derive(Debug, thiserror::Error)]
pub enum UpdateError {
    #[error("updates are disabled in this build (no trusted public key)")]
    NoTrustedKey,
    #[error("invalid public key: {0}")]
    BadKey(String),
    #[error("signature verification failed")]
    BadSignature,
    #[error("manifest is for channel {found:?}, expected {expected:?}")]
    WrongChannel { expected: String, found: String },
    #[error("invalid manifest: {0}")]
    BadManifest(String),
    #[error("no update asset for platform {0}")]
    NoAsset(String),
    #[error("downloaded file does not match manifest ({0})")]
    Integrity(&'static str),
    #[error("self-update is not supported for this install type: {0}")]
    Unsupported(&'static str),
    #[error("network error: {0}")]
    Network(String),
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Manifest {
    pub channel: String,
    pub version: String,
    #[serde(default)]
    pub notes: String,
    pub assets: Vec<Asset>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Asset {
    /// `<os>-<arch>`, e.g. `windows-x86_64`, `linux-aarch64`, `macos-aarch64`.
    pub platform: String,
    pub url: String,
    pub sha256: String,
    pub size: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct Available {
    pub version: String,
    pub notes: String,
    pub asset: Asset,
}

pub struct Updater {
    base_url: String,
    channel: String,
    keys: Vec<VerifyingKey>,
}

pub fn current_platform() -> String {
    format!("{}-{}", std::env::consts::OS, std::env::consts::ARCH)
}

pub fn parse_keys(list: &str) -> Result<Vec<VerifyingKey>, UpdateError> {
    list.split(',')
        .map(str::trim)
        .filter(|k| !k.is_empty())
        .map(|k| {
            let bytes = base64::engine::general_purpose::STANDARD
                .decode(k)
                .map_err(|e| UpdateError::BadKey(e.to_string()))?;
            let bytes: [u8; 32] = bytes
                .try_into()
                .map_err(|_| UpdateError::BadKey("expected 32 bytes".into()))?;
            VerifyingKey::from_bytes(&bytes).map_err(|e| UpdateError::BadKey(e.to_string()))
        })
        .collect()
}

/// Verify `manifest_bytes` against a base64 detached signature with any of
/// `keys`, then parse it and check the channel.
pub fn verify_manifest(
    manifest_bytes: &[u8],
    signature_b64: &str,
    keys: &[VerifyingKey],
    channel: &str,
) -> Result<Manifest, UpdateError> {
    if keys.is_empty() {
        return Err(UpdateError::NoTrustedKey);
    }
    let sig = base64::engine::general_purpose::STANDARD
        .decode(signature_b64.trim())
        .map_err(|_| UpdateError::BadSignature)?;
    let sig: [u8; 64] = sig.try_into().map_err(|_| UpdateError::BadSignature)?;
    let sig = Signature::from_bytes(&sig);
    if !keys
        .iter()
        .any(|k| k.verify_strict(manifest_bytes, &sig).is_ok())
    {
        return Err(UpdateError::BadSignature);
    }
    let manifest: Manifest = serde_json::from_slice(manifest_bytes)
        .map_err(|e| UpdateError::BadManifest(e.to_string()))?;
    if manifest.channel != channel {
        return Err(UpdateError::WrongChannel {
            expected: channel.into(),
            found: manifest.channel,
        });
    }
    Ok(manifest)
}

/// Pick the asset for `platform` if `manifest` is strictly newer than `current`.
pub fn select_update(
    manifest: &Manifest,
    current: &str,
    platform: &str,
) -> Result<Option<Available>, UpdateError> {
    let parse = |v: &str| {
        semver::Version::parse(v.trim_start_matches('v'))
            .map_err(|e| UpdateError::BadManifest(format!("version {v:?}: {e}")))
    };
    if parse(&manifest.version)? <= parse(current)? {
        return Ok(None);
    }
    let asset = manifest
        .assets
        .iter()
        .find(|a| a.platform == platform)
        .ok_or_else(|| UpdateError::NoAsset(platform.into()))?;
    Ok(Some(Available {
        version: manifest.version.clone(),
        notes: manifest.notes.clone(),
        asset: asset.clone(),
    }))
}

impl Updater {
    /// Updater using the keys baked in at build time.
    pub fn from_build(channel: &str) -> Result<Self, UpdateError> {
        let keys = parse_keys(BUILD_PUBLIC_KEYS.ok_or(UpdateError::NoTrustedKey)?)?;
        Ok(Self::new(DEFAULT_BASE_URL, channel, keys))
    }

    pub fn new(base_url: &str, channel: &str, keys: Vec<VerifyingKey>) -> Self {
        Self {
            base_url: base_url.trim_end_matches('/').into(),
            channel: channel.into(),
            keys,
        }
    }

    pub fn check(&self, current_version: &str) -> Result<Option<Available>, UpdateError> {
        let name = format!("update-{}.json", self.channel);
        let manifest = fetch_small(&format!("{}/{name}", self.base_url))?;
        let signature = fetch_small(&format!("{}/{name}.sig", self.base_url))?;
        let signature = String::from_utf8(signature).map_err(|_| UpdateError::BadSignature)?;
        let manifest = verify_manifest(&manifest, &signature, &self.keys, &self.channel)?;
        select_update(&manifest, current_version, &current_platform())
    }

    /// Download, verify, and replace the running executable.
    /// The new version takes effect on next start.
    pub fn install(&self, update: &Available) -> Result<(), UpdateError> {
        // A copy installed by the system package manager (.deb …) is
        // updated by it, not replaced behind its back.
        if let Ok(exe) = std::env::current_exe()
            && managed_by_system(&exe)
        {
            return Err(UpdateError::Unsupported(
                "installed by the system package manager: update it with the package manager",
            ));
        }
        // macOS: the program inside TermCraft.app is replaced in place.
        let file = download_verified(&update.asset)?;
        self_replace::self_replace(file.path())?;
        Ok(())
    }
}

/// Programs under `/usr` (Linux packages) belong to the package manager.
pub fn managed_by_system(exe: &Path) -> bool {
    cfg!(target_os = "linux") && exe.starts_with("/usr")
}

fn fetch_small(url: &str) -> Result<Vec<u8>, UpdateError> {
    let mut resp = ureq::get(url)
        .call()
        .map_err(|e| UpdateError::Network(format!("{url}: {e}")))?;
    let mut out = Vec::new();
    resp.body_mut()
        .as_reader()
        .take(MAX_MANIFEST_BYTES + 1)
        .read_to_end(&mut out)?;
    if out.len() as u64 > MAX_MANIFEST_BYTES {
        return Err(UpdateError::BadManifest("manifest too large".into()));
    }
    Ok(out)
}

fn download_verified(asset: &Asset) -> Result<tempfile::NamedTempFile, UpdateError> {
    let mut resp = ureq::get(&asset.url)
        .call()
        .map_err(|e| UpdateError::Network(format!("{}: {e}", asset.url)))?;
    let mut reader = resp.body_mut().as_reader().take(asset.size + 1);
    let mut file = tempfile::NamedTempFile::new()?;
    copy_and_verify(&mut reader, file.as_file_mut(), asset)?;
    Ok(file)
}

/// Stream `reader` into `out` while hashing; enforce exact size and SHA-256.
fn copy_and_verify(
    reader: &mut dyn Read,
    out: &mut dyn Write,
    asset: &Asset,
) -> Result<(), UpdateError> {
    let mut hasher = Sha256::new();
    let mut total = 0u64;
    let mut buf = [0u8; 64 * 1024];
    loop {
        let n = reader.read(&mut buf)?;
        if n == 0 {
            break;
        }
        total += n as u64;
        if total > asset.size {
            return Err(UpdateError::Integrity("size"));
        }
        hasher.update(&buf[..n]);
        out.write_all(&buf[..n])?;
    }
    out.flush()?;
    if total != asset.size {
        return Err(UpdateError::Integrity("size"));
    }
    if !hex::encode(hasher.finalize()).eq_ignore_ascii_case(&asset.sha256) {
        return Err(UpdateError::Integrity("sha256"));
    }
    Ok(())
}

/// Sign a manifest (used by the release pipeline via `termcraft-cli`).
pub fn sign_manifest(manifest_bytes: &[u8], secret_key_b64: &str) -> Result<String, UpdateError> {
    use ed25519_dalek::Signer as _;
    let secret = base64::engine::general_purpose::STANDARD
        .decode(secret_key_b64.trim())
        .map_err(|e| UpdateError::BadKey(e.to_string()))?;
    let secret: [u8; 32] = secret
        .try_into()
        .map_err(|_| UpdateError::BadKey("expected 32-byte seed".into()))?;
    let key = ed25519_dalek::SigningKey::from_bytes(&secret);
    Ok(base64::engine::general_purpose::STANDARD.encode(key.sign(manifest_bytes).to_bytes()))
}

/// Build a release manifest from local files: `(platform, file)` pairs,
/// each published at `<base_url>/<file name>`.
pub fn build_manifest(
    channel: &str,
    version: &str,
    notes: &str,
    base_url: &str,
    assets: &[(String, std::path::PathBuf)],
) -> Result<Manifest, UpdateError> {
    semver::Version::parse(version.trim_start_matches('v'))
        .map_err(|e| UpdateError::BadManifest(format!("version {version:?}: {e}")))?;
    let base = base_url.trim_end_matches('/');
    let assets = assets
        .iter()
        .map(|(platform, path)| {
            let name = path.file_name().and_then(|n| n.to_str()).ok_or_else(|| {
                UpdateError::BadManifest(format!("bad file name {}", path.display()))
            })?;
            Ok(Asset {
                platform: platform.clone(),
                url: format!("{base}/{name}"),
                sha256: sha256_file(path)?,
                size: std::fs::metadata(path)?.len(),
            })
        })
        .collect::<Result<Vec<_>, UpdateError>>()?;
    Ok(Manifest {
        channel: channel.into(),
        version: version.trim_start_matches('v').into(),
        notes: notes.into(),
        assets,
    })
}

/// Hash a local file the way manifests expect (hex SHA-256).
pub fn sha256_file(path: &Path) -> Result<String, UpdateError> {
    let mut file = std::fs::File::open(path)?;
    let mut hasher = Sha256::new();
    std::io::copy(&mut file, &mut HashWriter(&mut hasher))?;
    Ok(hex::encode(hasher.finalize()))
}

struct HashWriter<'a>(&'a mut Sha256);

impl Write for HashWriter<'_> {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.update(buf);
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SEED: [u8; 32] = [7; 32];

    fn keypair() -> (String, Vec<VerifyingKey>) {
        let sk = ed25519_dalek::SigningKey::from_bytes(&SEED);
        let secret = base64::engine::general_purpose::STANDARD.encode(SEED);
        (secret, vec![sk.verifying_key()])
    }

    fn manifest(version: &str, channel: &str) -> Vec<u8> {
        serde_json::to_vec(&Manifest {
            channel: channel.into(),
            version: version.into(),
            notes: "fixes".into(),
            assets: vec![Asset {
                platform: "windows-x86_64".into(),
                url: "https://example.invalid/termcraft.exe".into(),
                sha256: hex::encode(Sha256::digest(b"new binary")),
                size: 10,
            }],
        })
        .unwrap()
    }

    #[test]
    fn valid_signature_accepted() {
        let (secret, keys) = keypair();
        let m = manifest("0.2.0", "stable");
        let sig = sign_manifest(&m, &secret).unwrap();
        let parsed = verify_manifest(&m, &sig, &keys, "stable").unwrap();
        let update = select_update(&parsed, "0.1.0", "windows-x86_64").unwrap();
        assert_eq!(update.unwrap().version, "0.2.0");
    }

    #[test]
    fn tampered_manifest_rejected() {
        let (secret, keys) = keypair();
        let m = manifest("0.2.0", "stable");
        let sig = sign_manifest(&m, &secret).unwrap();
        let mut evil = m.clone();
        let pos = evil.iter().position(|&b| b == b'2').unwrap();
        evil[pos] = b'9';
        assert!(matches!(
            verify_manifest(&evil, &sig, &keys, "stable"),
            Err(UpdateError::BadSignature)
        ));
    }

    #[test]
    fn wrong_key_and_no_key_rejected() {
        let (secret, _) = keypair();
        let other = ed25519_dalek::SigningKey::from_bytes(&[9; 32]).verifying_key();
        let m = manifest("0.2.0", "stable");
        let sig = sign_manifest(&m, &secret).unwrap();
        assert!(verify_manifest(&m, &sig, &[other], "stable").is_err());
        assert!(matches!(
            verify_manifest(&m, &sig, &[], "stable"),
            Err(UpdateError::NoTrustedKey)
        ));
    }

    #[test]
    fn channel_mismatch_rejected() {
        let (secret, keys) = keypair();
        let m = manifest("0.2.0", "beta");
        let sig = sign_manifest(&m, &secret).unwrap();
        assert!(matches!(
            verify_manifest(&m, &sig, &keys, "stable"),
            Err(UpdateError::WrongChannel { .. })
        ));
    }

    #[test]
    fn no_downgrade_or_same_version() {
        let m: Manifest = serde_json::from_slice(&manifest("0.1.0", "stable")).unwrap();
        assert!(
            select_update(&m, "0.1.0", "windows-x86_64")
                .unwrap()
                .is_none()
        );
        assert!(
            select_update(&m, "0.3.0", "windows-x86_64")
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn missing_platform_is_an_error() {
        let m: Manifest = serde_json::from_slice(&manifest("0.2.0", "stable")).unwrap();
        assert!(matches!(
            select_update(&m, "0.1.0", "linux-riscv64"),
            Err(UpdateError::NoAsset(_))
        ));
    }

    #[test]
    fn manifest_from_files_signs_and_verifies() {
        let dir = tempfile::tempdir().unwrap();
        let exe = dir.path().join("termcraft-windows-x86_64.exe");
        std::fs::write(&exe, b"binary").unwrap();
        let m = build_manifest(
            "stable",
            "v1.2.3",
            "notes",
            "https://example.invalid/dl/",
            &[("windows-x86_64".into(), exe)],
        )
        .unwrap();
        assert_eq!(m.version, "1.2.3");
        assert_eq!(
            m.assets[0].url,
            "https://example.invalid/dl/termcraft-windows-x86_64.exe"
        );
        assert_eq!(m.assets[0].size, 6);
        let bytes = serde_json::to_vec(&m).unwrap();
        let (secret, keys) = keypair();
        let sig = sign_manifest(&bytes, &secret).unwrap();
        assert_eq!(verify_manifest(&bytes, &sig, &keys, "stable").unwrap(), m);
        assert!(build_manifest("stable", "not-semver", "", "x", &[]).is_err());
    }

    /// A release served over HTTP: check finds the update, the download
    /// is verified, a tampered signature is refused.
    #[test]
    fn check_and_download_from_a_server() {
        use std::io::{BufRead, BufReader};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let dir = tempfile::tempdir().unwrap();
        let platform = current_platform();
        let exe = dir.path().join(format!("termcraft-{platform}"));
        std::fs::write(&exe, b"the new release").unwrap();
        let m = build_manifest(
            "stable",
            "99.0.0",
            "notes",
            &base,
            &[(platform.clone(), exe.clone())],
        )
        .unwrap();
        let bytes = serde_json::to_vec(&m).unwrap();
        let (secret, keys) = keypair();
        let sig = sign_manifest(&bytes, &secret).unwrap();
        let files: std::collections::HashMap<String, Vec<u8>> = [
            ("/update-stable.json".to_string(), bytes.clone()),
            ("/update-stable.json.sig".to_string(), sig.into_bytes()),
            (
                format!("/termcraft-{platform}"),
                b"the new release".to_vec(),
            ),
        ]
        .into();
        std::thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                let mut line = String::new();
                reader.read_line(&mut line).unwrap();
                loop {
                    let mut h = String::new();
                    reader.read_line(&mut h).unwrap();
                    if h == "\r\n" || h.is_empty() {
                        break;
                    }
                }
                let path = line.split_whitespace().nth(1).unwrap_or("/").to_string();
                let mut out = stream;
                match files.get(&path) {
                    Some(body) => {
                        let _ = write!(
                            out,
                            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                            body.len()
                        );
                        let _ = out.write_all(body);
                    }
                    None => {
                        let _ = out.write_all(b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
                    }
                }
            }
        });
        let updater = Updater::new(&base, "stable", keys.clone());
        let update = updater.check("1.0.0").unwrap().expect("a newer version");
        assert_eq!(update.version, "99.0.0");
        let file = download_verified(&update.asset).unwrap();
        assert_eq!(std::fs::read(file.path()).unwrap(), b"the new release");
        assert!(
            updater.check("99.0.0").unwrap().is_none(),
            "already up to date"
        );
        let other = ed25519_dalek::SigningKey::from_bytes(&[9; 32]).verifying_key();
        assert!(
            Updater::new(&base, "stable", vec![other])
                .check("1.0.0")
                .is_err()
        );
        assert!(managed_by_system(Path::new("/usr/bin/termcraft")) == cfg!(target_os = "linux"));
    }

    #[test]
    fn download_integrity() {
        let m: Manifest = serde_json::from_slice(&manifest("0.2.0", "stable")).unwrap();
        let asset = &m.assets[0];
        let mut out = Vec::new();
        copy_and_verify(&mut &b"new binary"[..], &mut out, asset).unwrap();
        assert_eq!(out, b"new binary");

        assert!(copy_and_verify(&mut &b"evil binar"[..], &mut Vec::new(), asset).is_err());
        assert!(copy_and_verify(&mut &b"new binary!!"[..], &mut Vec::new(), asset).is_err());
        assert!(copy_and_verify(&mut &b"short"[..], &mut Vec::new(), asset).is_err());
    }
}
