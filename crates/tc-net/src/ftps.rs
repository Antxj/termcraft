//! TLS for FTPS sessions (rustls with ring). Certificates are verified
//! against the web PKI roots; a server whose certificate does not verify
//! (self-signed is common on FTP servers) can still be used when the user
//! accepted that exact certificate: its SHA-256 fingerprint is pinned,
//! like an SSH host key.

use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::crypto::{CryptoProvider, WebPkiSupportedAlgorithms};
use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use rustls::{DigitallySignedStruct, SignatureScheme};
use sha2::Digest as _;
use std::sync::{Arc, Mutex};

/// `SHA256:` + hex of the leaf certificate.
pub fn fingerprint(cert: &CertificateDer<'_>) -> String {
    let digest = sha2::Sha256::digest(cert.as_ref());
    let hex: Vec<String> = digest.iter().map(|b| format!("{b:02x}")).collect();
    format!("SHA256:{}", hex.join(":"))
}

#[derive(Debug)]
struct PinningVerifier {
    webpki: Arc<rustls::client::WebPkiServerVerifier>,
    algorithms: WebPkiSupportedAlgorithms,
    /// Fingerprint the user accepted for this server.
    pinned: Option<String>,
    /// The certificate that failed, for the "trust it?" question.
    rejected: Arc<Mutex<Option<(String, String)>>>,
}

impl ServerCertVerifier for PinningVerifier {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        intermediates: &[CertificateDer<'_>],
        server_name: &ServerName<'_>,
        ocsp: &[u8],
        now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        match self
            .webpki
            .verify_server_cert(end_entity, intermediates, server_name, ocsp, now)
        {
            Ok(ok) => Ok(ok),
            Err(e) => {
                let fp = fingerprint(end_entity);
                if self.pinned.as_deref() == Some(fp.as_str()) {
                    return Ok(ServerCertVerified::assertion());
                }
                *self.rejected.lock().expect("lock") = Some((fp, e.to_string()));
                Err(e)
            }
        }
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls12_signature(message, cert, dss, &self.algorithms)
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls13_signature(message, cert, dss, &self.algorithms)
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.algorithms.supported_schemes()
    }
}

/// Which TLS versions the session allows (3_19 TLS 1.2, 3_22 TLS 1.3).
/// Older versions are never offered.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TlsVersions {
    pub tls12: bool,
    pub tls13: bool,
}

/// A client configuration and where a rejected certificate is reported.
#[allow(clippy::type_complexity)]
pub fn client_config(
    versions: TlsVersions,
    pinned: Option<String>,
) -> Result<
    (
        Arc<rustls::ClientConfig>,
        Arc<Mutex<Option<(String, String)>>>,
    ),
    String,
> {
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let mut list: Vec<&'static rustls::SupportedProtocolVersion> = Vec::new();
    if versions.tls13 {
        list.push(&rustls::version::TLS13);
    }
    if versions.tls12 {
        list.push(&rustls::version::TLS12);
    }
    if list.is_empty() {
        return Err("FTPS needs TLS 1.2 or TLS 1.3 (older versions are not supported)".into());
    }
    let mut roots = rustls::RootCertStore::empty();
    roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
    let webpki = rustls::client::WebPkiServerVerifier::builder_with_provider(
        Arc::new(roots),
        provider.clone(),
    )
    .build()
    .map_err(|e| e.to_string())?;
    let rejected = Arc::new(Mutex::new(None));
    let verifier = PinningVerifier {
        webpki,
        algorithms: provider_algorithms(&provider),
        pinned,
        rejected: rejected.clone(),
    };
    let config = rustls::ClientConfig::builder_with_provider(provider)
        .with_protocol_versions(&list)
        .map_err(|e| e.to_string())?
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(verifier))
        .with_no_client_auth();
    Ok((Arc::new(config), rejected))
}

fn provider_algorithms(p: &CryptoProvider) -> WebPkiSupportedAlgorithms {
    p.signature_verification_algorithms
}

/// Saved FTPS certificates the user trusted: `host:port SHA256:…` lines.
pub fn trusted(file: &std::path::Path, host: &str, port: u16) -> Option<String> {
    let key = format!("{host}:{port}");
    std::fs::read_to_string(file).ok()?.lines().find_map(|l| {
        let (k, fp) = l.split_once(' ')?;
        (k == key).then(|| fp.trim().to_string())
    })
}

pub fn trust(
    file: &std::path::Path,
    host: &str,
    port: u16,
    fingerprint: &str,
) -> std::io::Result<()> {
    let key = format!("{host}:{port}");
    let old = std::fs::read_to_string(file).unwrap_or_default();
    let mut lines: Vec<String> = old
        .lines()
        .filter(|l| l.split_once(' ').map(|(k, _)| k) != Some(key.as_str()))
        .map(String::from)
        .collect();
    lines.push(format!("{key} {fingerprint}"));
    if let Some(dir) = file.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(file, lines.join("\n") + "\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions_and_trust_file() {
        assert!(
            client_config(
                TlsVersions {
                    tls12: false,
                    tls13: false
                },
                None
            )
            .is_err()
        );
        assert!(
            client_config(
                TlsVersions {
                    tls12: true,
                    tls13: true
                },
                None
            )
            .is_ok()
        );
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("ftps_certs");
        assert_eq!(trusted(&file, "h", 21), None);
        trust(&file, "h", 21, "SHA256:aa").unwrap();
        trust(&file, "other", 990, "SHA256:bb").unwrap();
        trust(&file, "h", 21, "SHA256:cc").unwrap();
        assert_eq!(trusted(&file, "h", 21).as_deref(), Some("SHA256:cc"));
        assert_eq!(trusted(&file, "other", 990).as_deref(), Some("SHA256:bb"));
    }

    #[test]
    fn fingerprint_format() {
        let fp = fingerprint(&CertificateDer::from(vec![1u8, 2, 3]));
        assert!(fp.starts_with("SHA256:"));
        assert_eq!(fp.matches(':').count(), 32);
    }
}
