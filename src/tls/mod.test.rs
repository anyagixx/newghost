// FILE: src/tls/mod.test.rs
// VERSION: 0.1.1
// START_MODULE_CONTRACT
//   PURPOSE: Verify deterministic TLS material loading for server and client trust bootstrap surfaces.
//   SCOPE: Valid, missing, malformed, expired, and untrusted TLS material cases plus client-only trust loading.
//   DEPENDS: src/tls/mod.rs
//   LINKS: V-M-TLS
// END_MODULE_CONTRACT
//
// START_MODULE_MAP
//   loads_valid_tls_material - validates full server-and-client TLS bootstrap
//   loads_client_trust_anchor_without_server_key_material - validates client-only trust bootstrap
//   rejects_missing_files - validates deterministic missing-file errors
//   rejects_malformed_certificate - validates malformed cert rejection
//   rejects_expired_certificate - validates expiry rejection
//   rejects_untrusted_self_signed_material - validates trust mismatch rejection
// END_MODULE_MAP
//
// START_CHANGE_SUMMARY
//   LAST_CHANGE: v0.1.1 - Added client-only trust-anchor coverage for live client TLS bootstrap.
// END_CHANGE_SUMMARY

use std::fs;
use std::time::{Duration, SystemTime};

use rcgen::{CertificateParams, DistinguishedName, DnType, KeyPair};
use tempfile::tempdir;

use super::{ClientTlsConfig, TlsConfig, TlsContextHandle, TlsError};

fn write_tls_material(
    cert_pem: &str,
    key_pem: &str,
    trust_pem: &str,
) -> (tempfile::TempDir, TlsConfig) {
    let dir = tempdir().expect("tempdir should build");
    let cert_path = dir.path().join("cert.pem");
    let key_path = dir.path().join("key.pem");
    let trust_anchor_path = dir.path().join("trust.pem");

    fs::write(&cert_path, cert_pem).expect("cert should write");
    fs::write(&key_path, key_pem).expect("key should write");
    fs::write(&trust_anchor_path, trust_pem).expect("trust should write");

    (
        dir,
        TlsConfig {
            cert_path,
            key_path,
            trust_anchor_path,
        },
    )
}

fn generate_self_signed_material() -> (String, String) {
    let key_pair = KeyPair::generate().expect("key pair should build");
    let mut params = CertificateParams::new(vec!["localhost".to_string()])
        .expect("certificate params should build");
    let mut distinguished_name = DistinguishedName::new();
    distinguished_name.push(DnType::CommonName, "localhost");
    params.distinguished_name = distinguished_name;

    let cert = params
        .self_signed(&key_pair)
        .expect("self-signed certificate should build");

    (cert.pem(), key_pair.serialize_pem())
}

fn generate_expired_material() -> (String, String) {
    let key_pair = KeyPair::generate().expect("key pair should build");
    let mut params = CertificateParams::new(vec!["localhost".to_string()])
        .expect("certificate params should build");
    params.not_before = (SystemTime::UNIX_EPOCH - Duration::from_secs(120)).into();
    params.not_after = (SystemTime::UNIX_EPOCH - Duration::from_secs(60)).into();

    let cert = params
        .self_signed(&key_pair)
        .expect("expired certificate should build");

    (cert.pem(), key_pair.serialize_pem())
}

#[test]
fn loads_valid_tls_material() {
    let (cert_pem, key_pem) = generate_self_signed_material();
    let (_dir, config) = write_tls_material(&cert_pem, &key_pem, &cert_pem);

    let context = TlsContextHandle::from_config(&config).expect("tls context should load");

    assert!(context.leaf_subject.contains("localhost"));
}

#[test]
fn loads_client_trust_anchor_without_server_key_material() {
    let (cert_pem, _key_pem) = generate_self_signed_material();
    let dir = tempdir().expect("tempdir should build");
    let trust_anchor_path = dir.path().join("trust.pem");
    fs::write(&trust_anchor_path, cert_pem).expect("trust should write");

    let context = TlsContextHandle::from_client_config(&ClientTlsConfig { trust_anchor_path })
        .expect("client trust context should load");

    assert!(context.leaf_subject.contains("localhost"));
}

#[test]
fn rejects_missing_files() {
    let dir = tempdir().expect("tempdir should build");
    let config = TlsConfig {
        cert_path: dir.path().join("missing-cert.pem"),
        key_path: dir.path().join("missing-key.pem"),
        trust_anchor_path: dir.path().join("missing-trust.pem"),
    };

    let err = match TlsContextHandle::from_config(&config) {
        Ok(_) => panic!("missing files must fail"),
        Err(err) => err,
    };

    assert!(matches!(err, TlsError::MissingFile { .. }));
}

#[test]
fn rejects_malformed_certificate() {
    let (_dir, config) = write_tls_material("not a cert", "not a key", "not a trust anchor");

    let err = match TlsContextHandle::from_config(&config) {
        Ok(_) => panic!("malformed cert must fail"),
        Err(err) => err,
    };

    assert_eq!(err, TlsError::InvalidCertificatePem);
}

#[test]
fn rejects_expired_certificate() {
    let (cert_pem, key_pem) = generate_expired_material();
    let (_dir, config) = write_tls_material(&cert_pem, &key_pem, &cert_pem);

    let err = match TlsContextHandle::from_config(&config) {
        Ok(_) => panic!("expired cert must fail"),
        Err(err) => err,
    };

    assert_eq!(err, TlsError::CertificateExpired);
}

#[test]
fn rejects_untrusted_self_signed_material() {
    let (cert_pem, key_pem) = generate_self_signed_material();
    let (other_cert_pem, _other_key_pem) = generate_self_signed_material();
    let (_dir, config) = write_tls_material(&cert_pem, &key_pem, &other_cert_pem);

    let err = match TlsContextHandle::from_config(&config) {
        Ok(_) => panic!("untrusted cert must fail"),
        Err(err) => err,
    };

    assert_eq!(err, TlsError::UntrustedMaterial);
}
