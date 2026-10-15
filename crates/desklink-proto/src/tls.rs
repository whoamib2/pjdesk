//! TLS identities and certificate pinning.
//!
//! DeskLink doesn't use certificate authorities. The server and each host have
//! self-signed certificates, and the other side checks the SHA-256 fingerprint
//! of the certificate instead:
//!
//! * The **server** fingerprint is trusted on first use (TOFU) and saved in the
//!   client config; it can also be pinned up front from the server console.
//! * The **host** fingerprint is delivered by the server for each session.
//!   Even if the server lied about it, the SPAKE2 password check is bound to
//!   the TLS session ([`crate::auth`]), so a man in the middle still fails.

use std::path::Path;
use std::sync::{Arc, Mutex};

use anyhow::{bail, Context, Result};
use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::crypto::{verify_tls12_signature, verify_tls13_signature, CryptoProvider};
use rustls::pki_types::pem::PemObject;
use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer, ServerName, UnixTime};
use rustls::{DigitallySignedStruct, SignatureScheme};
use sha2::{Digest, Sha256};

/// SHA-256 of a DER-encoded certificate.
pub type Fingerprint = [u8; 32];

/// The crypto provider used everywhere (*ring*: no CMake/NASM needed to build).
pub fn provider() -> Arc<CryptoProvider> {
    Arc::new(rustls::crypto::ring::default_provider())
}

/// Fingerprint of a DER certificate.
pub fn fingerprint(cert_der: &[u8]) -> Fingerprint {
    Sha256::digest(cert_der).into()
}

/// Formats a fingerprint as 16 dash-separated groups of 4 hex digits.
pub fn format_fingerprint(fp: &Fingerprint) -> String {
    hex::encode(fp).as_bytes().chunks(4).map(|c| std::str::from_utf8(c).unwrap()).collect::<Vec<_>>().join("-")
}

/// Parses a fingerprint, ignoring separators, spaces and an optional
/// `sha256:` prefix.
pub fn parse_fingerprint(text: &str) -> Result<Fingerprint> {
    let text = text.trim();
    let text = text.strip_prefix("sha256:").or_else(|| text.strip_prefix("SHA256:")).unwrap_or(text);
    let digits: String = text.chars().filter(|c| c.is_ascii_hexdigit()).collect();
    let bytes = hex::decode(&digits).context("fingerprint is not hexadecimal")?;
    match <[u8; 32]>::try_from(bytes.as_slice()) {
        Ok(fp) => Ok(fp),
        Err(_) => bail!("a fingerprint has 64 hex digits, got {}", digits.len()),
    }
}

/// A certificate and its private key.
pub struct TlsIdentity {
    pub cert: CertificateDer<'static>,
    pub key: PrivateKeyDer<'static>,
}

impl TlsIdentity {
    /// Creates a new self-signed ECDSA P-256 certificate.
    pub fn generate(name: &str) -> Result<Self> {
        let certified = rcgen::generate_simple_self_signed(vec![name.to_string()])?;
        Ok(Self {
            cert: certified.cert.der().clone(),
            key: PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(certified.signing_key.serialize_der())),
        })
    }

    /// Loads the identity from PEM files, creating them on first run.
    pub fn load_or_create(cert_path: &Path, key_path: &Path, name: &str) -> Result<Self> {
        if cert_path.exists() && key_path.exists() {
            let cert =
                CertificateDer::from_pem_file(cert_path).with_context(|| format!("reading {}", cert_path.display()))?;
            let key =
                PrivateKeyDer::from_pem_file(key_path).with_context(|| format!("reading {}", key_path.display()))?;
            return Ok(Self { cert, key });
        }
        let certified = rcgen::generate_simple_self_signed(vec![name.to_string()])?;
        std::fs::write(cert_path, certified.cert.pem()).with_context(|| format!("writing {}", cert_path.display()))?;
        std::fs::write(key_path, certified.signing_key.serialize_pem())
            .with_context(|| format!("writing {}", key_path.display()))?;
        Ok(Self {
            cert: certified.cert.der().clone(),
            key: PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(certified.signing_key.serialize_der())),
        })
    }

    pub fn fingerprint(&self) -> Fingerprint {
        fingerprint(self.cert.as_ref())
    }
}

/// TLS 1.3 server configuration presenting `identity`.
pub fn server_config(identity: &TlsIdentity, alpn: &[u8]) -> Result<rustls::ServerConfig> {
    let mut config = rustls::ServerConfig::builder_with_provider(provider())
        .with_protocol_versions(&[&rustls::version::TLS13])?
        .with_no_client_auth()
        .with_single_cert(vec![identity.cert.clone()], identity.key.clone_key())?;
    config.alpn_protocols = vec![alpn.to_vec()];
    Ok(config)
}

/// TLS 1.3 client configuration that accepts only the pinned certificate
/// (or, with `expected == None`, any certificate — trust on first use).
pub fn client_config(verifier: Arc<PinnedCertVerifier>, alpn: &[u8]) -> Result<rustls::ClientConfig> {
    let mut config = rustls::ClientConfig::builder_with_provider(provider())
        .with_protocol_versions(&[&rustls::version::TLS13])?
        .dangerous()
        .with_custom_certificate_verifier(verifier)
        .with_no_client_auth();
    config.alpn_protocols = vec![alpn.to_vec()];
    Ok(config)
}

/// Verifies the server certificate by fingerprint instead of by CA chain.
/// Handshake signatures are still fully verified, so the peer must really
/// hold the pinned private key.
#[derive(Debug)]
pub struct PinnedCertVerifier {
    expected: Option<Fingerprint>,
    observed: Mutex<Option<Fingerprint>>,
    provider: Arc<CryptoProvider>,
}

impl PinnedCertVerifier {
    pub fn new(expected: Option<Fingerprint>) -> Arc<Self> {
        Arc::new(Self { expected, observed: Mutex::new(None), provider: provider() })
    }

    /// Fingerprint of the certificate the peer presented (after the handshake).
    pub fn observed(&self) -> Option<Fingerprint> {
        *self.observed.lock().unwrap()
    }

    /// True if a certificate was presented and it did not match the pin.
    pub fn mismatch(&self) -> bool {
        matches!((self.expected, self.observed()), (Some(e), Some(o)) if e != o)
    }
}

impl ServerCertVerifier for PinnedCertVerifier {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp_response: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        let fp = fingerprint(end_entity.as_ref());
        *self.observed.lock().unwrap() = Some(fp);
        match self.expected {
            Some(expected) if expected != fp => Err(rustls::Error::General(format!(
                "certificate fingerprint mismatch: expected {}, got {}",
                format_fingerprint(&expected),
                format_fingerprint(&fp)
            ))),
            _ => Ok(ServerCertVerified::assertion()),
        }
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        verify_tls12_signature(message, cert, dss, &self.provider.signature_verification_algorithms)
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        verify_tls13_signature(message, cert, dss, &self.provider.signature_verification_algorithms)
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.provider.signature_verification_algorithms.supported_schemes()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fingerprint_format_roundtrip() {
        let id = TlsIdentity::generate("test").unwrap();
        let fp = id.fingerprint();
        let text = format_fingerprint(&fp);
        assert_eq!(text.len(), 64 + 15);
        assert_eq!(parse_fingerprint(&text).unwrap(), fp);
        assert_eq!(parse_fingerprint(&format!("sha256:{}", hex::encode(fp))).unwrap(), fp);
        assert!(parse_fingerprint("abcd").is_err());
    }

    #[test]
    fn configs_build() {
        let id = TlsIdentity::generate("test").unwrap();
        server_config(&id, b"x").unwrap();
        client_config(PinnedCertVerifier::new(Some(id.fingerprint())), b"x").unwrap();
    }
}
