// FILE: src/tls/mod.rs
// VERSION: 0.1.1
// START_MODULE_CONTRACT
//   PURPOSE: Load TLS material and expose accept/connect operations used by the websocket transport boundary.
//   SCOPE: TLS material parsing, deterministic validation, rustls context construction, and typed accept/connect helpers.
//   DEPENDS: std, thiserror, tokio, tokio-rustls, rustls-pemfile, rustls-pki-types, tracing, x509-parser
//   LINKS: M-TLS, V-M-TLS, DF-WSS-HANDSHAKE, VF-002
// END_MODULE_CONTRACT
//
// START_MODULE_MAP
//   TlsConfig - file paths for certificate, key, and trust anchor material
//   TlsContextHandle - built server and client TLS runtime context
//   from_config - load deterministic TLS context from PEM material
//   accept - wrap a server-side TCP stream in TLS
//   connect - wrap a client-side TCP stream in TLS and validate the server name
// END_MODULE_MAP
//
// START_CHANGE_SUMMARY
//   LAST_CHANGE: v0.1.1 - Installed a deterministic rustls crypto provider inside the TLS boundary to avoid cross-module feature drift.
// END_CHANGE_SUMMARY

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};
use std::time::SystemTime;

use rustls_pemfile::{certs, pkcs8_private_keys, rsa_private_keys};
use rustls_pki_types::{CertificateDer, PrivateKeyDer, ServerName};
use thiserror::Error;
use tokio::net::TcpStream;
use tokio_rustls::client::TlsStream as ClientTlsStream;
use tokio_rustls::rustls::{ClientConfig, RootCertStore, ServerConfig};
use tokio_rustls::server::TlsStream as ServerTlsStream;
use tokio_rustls::{TlsAcceptor, TlsConnector};
use tracing::{error, info};
use x509_parser::certificate::X509Certificate;
use x509_parser::prelude::FromDer;

#[cfg(test)]
#[path = "mod.test.rs"]
mod tests;

static TLS_CRYPTO_PROVIDER: OnceLock<()> = OnceLock::new();

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TlsConfig {
    pub cert_path: PathBuf,
    pub key_path: PathBuf,
    pub trust_anchor_path: PathBuf,
}

#[derive(Clone)]
pub struct TlsContextHandle {
    acceptor: TlsAcceptor,
    connector: TlsConnector,
    pub leaf_subject: String,
}

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum TlsError {
    #[error("missing TLS material: {path}")]
    MissingFile { path: PathBuf },
    #[error("certificate PEM is invalid")]
    InvalidCertificatePem,
    #[error("private key PEM is invalid")]
    InvalidPrivateKeyPem,
    #[error("trust anchor PEM is invalid")]
    InvalidTrustAnchorPem,
    #[error("certificate has expired")]
    CertificateExpired,
    #[error("TLS material is not trusted by the configured trust anchors")]
    UntrustedMaterial,
    #[error("invalid server name: {0}")]
    InvalidServerName(String),
    #[error("TLS handshake failed: {0}")]
    HandshakeFailed(String),
    #[error("TLS configuration invalid: {0}")]
    ConfigInvalid(String),
}

impl TlsContextHandle {
    // START_CONTRACT: from_config
    //   PURPOSE: Build a deterministic TLS context from configured certificate material.
    //   INPUTS: { config: &TlsConfig - certificate, key, and trust-anchor file paths }
    //   OUTPUTS: { Result<TlsContextHandle, TlsError> - ready server/client TLS runtime context }
    //   SIDE_EFFECTS: [reads PEM files from disk and emits structured validation logs]
    //   LINKS: [M-TLS, V-M-TLS]
    // END_CONTRACT: from_config
    pub fn from_config(config: &TlsConfig) -> Result<Self, TlsError> {
        // START_BLOCK_LOAD_TLS_MATERIAL
        install_crypto_provider();

        let cert_bytes = read_file(&config.cert_path)?;
        let key_bytes = read_file(&config.key_path)?;
        let trust_anchor_bytes = read_file(&config.trust_anchor_path)?;

        let cert_chain = parse_certificates(&cert_bytes, TlsError::InvalidCertificatePem)?;
        let private_key = parse_private_key(&key_bytes)?;
        let trust_anchors =
            parse_certificates(&trust_anchor_bytes, TlsError::InvalidTrustAnchorPem)?;

        validate_leaf_certificate(&cert_chain[0])?;
        validate_trust_relationship(&cert_chain[0], &trust_anchors)?;

        let mut root_store = RootCertStore::empty();
        for trust_anchor in trust_anchors {
            root_store
                .add(trust_anchor)
                .map_err(|err| TlsError::ConfigInvalid(err.to_string()))?;
        }

        let server_config = ServerConfig::builder()
            .with_no_client_auth()
            .with_single_cert(cert_chain.clone(), private_key)
            .map_err(|err| TlsError::ConfigInvalid(err.to_string()))?;

        let client_config = ClientConfig::builder()
            .with_root_certificates(root_store)
            .with_no_client_auth();

        let leaf_subject = describe_subject(&cert_chain[0])?;

        info!(
            cert_path = %config.cert_path.display(),
            key_path = %config.key_path.display(),
            trust_anchor_path = %config.trust_anchor_path.display(),
            subject = %leaf_subject,
            "[TlsContext][fromConfig][BLOCK_LOAD_TLS_MATERIAL] loaded TLS material"
        );

        Ok(Self {
            acceptor: TlsAcceptor::from(Arc::new(server_config)),
            connector: TlsConnector::from(Arc::new(client_config)),
            leaf_subject,
        })
        // END_BLOCK_LOAD_TLS_MATERIAL
    }

    // START_CONTRACT: accept
    //   PURPOSE: Wrap a server-side TCP stream with TLS.
    //   INPUTS: { stream: TcpStream - accepted TCP socket }
    //   OUTPUTS: { Result<ServerTlsStream<TcpStream>, TlsError> - TLS-wrapped server stream }
    //   SIDE_EFFECTS: [performs TLS handshake]
    //   LINKS: [M-TLS, M-WSS-GATEWAY]
    // END_CONTRACT: accept
    pub async fn accept(&self, stream: TcpStream) -> Result<ServerTlsStream<TcpStream>, TlsError> {
        self.acceptor
            .accept(stream)
            .await
            .map_err(|err| TlsError::HandshakeFailed(err.to_string()))
    }

    // START_CONTRACT: connect
    //   PURPOSE: Wrap a client-side TCP stream with TLS and SNI.
    //   INPUTS: { stream: TcpStream - connected TCP socket, domain: &str - server name for TLS verification }
    //   OUTPUTS: { Result<ClientTlsStream<TcpStream>, TlsError> - TLS-wrapped client stream }
    //   SIDE_EFFECTS: [performs TLS handshake]
    //   LINKS: [M-TLS, M-WSS-GATEWAY]
    // END_CONTRACT: connect
    pub async fn connect(
        &self,
        stream: TcpStream,
        domain: &str,
    ) -> Result<ClientTlsStream<TcpStream>, TlsError> {
        let server_name = ServerName::try_from(domain.to_string())
            .map_err(|_| TlsError::InvalidServerName(domain.to_string()))?;

        self.connector
            .connect(server_name, stream)
            .await
            .map_err(|err| TlsError::HandshakeFailed(err.to_string()))
    }
}

fn install_crypto_provider() {
    let _ = TLS_CRYPTO_PROVIDER.get_or_init(|| {
        let _ = tokio_rustls::rustls::crypto::ring::default_provider().install_default();
    });
}

fn read_file(path: &Path) -> Result<Vec<u8>, TlsError> {
    fs::read(path).map_err(|_| {
        error!(
            path = %path.display(),
            "[TlsContext][fromConfig][BLOCK_LOAD_TLS_MATERIAL] missing TLS material"
        );
        TlsError::MissingFile {
            path: path.to_path_buf(),
        }
    })
}

fn parse_certificates(
    input: &[u8],
    parse_error: TlsError,
) -> Result<Vec<CertificateDer<'static>>, TlsError> {
    let mut reader = input;
    let cert_chain: Vec<CertificateDer<'static>> = certs(&mut reader)
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| parse_error.clone())?;

    if cert_chain.is_empty() {
        return Err(parse_error);
    }

    Ok(cert_chain)
}

fn parse_private_key(input: &[u8]) -> Result<PrivateKeyDer<'static>, TlsError> {
    let mut pkcs8_reader = input;
    if let Some(private_key) = pkcs8_private_keys(&mut pkcs8_reader)
        .next()
        .transpose()
        .map_err(|_| TlsError::InvalidPrivateKeyPem)?
    {
        return Ok(PrivateKeyDer::Pkcs8(private_key));
    }

    let mut rsa_reader = input;
    if let Some(private_key) = rsa_private_keys(&mut rsa_reader)
        .next()
        .transpose()
        .map_err(|_| TlsError::InvalidPrivateKeyPem)?
    {
        return Ok(PrivateKeyDer::Pkcs1(private_key));
    }

    Err(TlsError::InvalidPrivateKeyPem)
}

fn validate_leaf_certificate(leaf: &CertificateDer<'static>) -> Result<(), TlsError> {
    let (_, certificate) =
        X509Certificate::from_der(leaf.as_ref()).map_err(|_| TlsError::InvalidCertificatePem)?;
    let _ = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map_err(|err| TlsError::ConfigInvalid(err.to_string()))?;

    if certificate.validity().is_valid() {
        Ok(())
    } else {
        Err(TlsError::CertificateExpired)
    }
}

fn validate_trust_relationship(
    leaf: &CertificateDer<'static>,
    trust_anchors: &[CertificateDer<'static>],
) -> Result<(), TlsError> {
    let (_, certificate) =
        X509Certificate::from_der(leaf.as_ref()).map_err(|_| TlsError::InvalidCertificatePem)?;
    let is_self_issued = certificate.issuer() == certificate.subject();
    let trusted_exact_match = trust_anchors
        .iter()
        .any(|anchor| anchor.as_ref() == leaf.as_ref());

    if is_self_issued && !trusted_exact_match {
        return Err(TlsError::UntrustedMaterial);
    }

    Ok(())
}

fn describe_subject(leaf: &CertificateDer<'static>) -> Result<String, TlsError> {
    let (_, certificate) =
        X509Certificate::from_der(leaf.as_ref()).map_err(|_| TlsError::InvalidCertificatePem)?;
    Ok(certificate.subject().to_string())
}
