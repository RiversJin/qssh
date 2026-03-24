use std::fs;
use std::io::BufReader;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{bail, Context, Result};
use base64::Engine;
use rustls::pki_types::{CertificateDer, PrivateKeyDer, ServerName};
use sha2::{Digest, Sha256};

// --- Certificate verification modes ---

pub enum CertVerifyMode {
    None,
    Tofu { known_hosts_path: PathBuf },
    CaCert { ca_path: PathBuf },
}

// --- Server: load or generate certs ---

pub fn load_or_generate_server_cert(
    cert_path: Option<&Path>,
    key_path: Option<&Path>,
    cert_dir: &Path,
    sans: &[String],
) -> Result<(Vec<CertificateDer<'static>>, PrivateKeyDer<'static>)> {
    // Mode 1: explicit cert+key
    if let (Some(cert_p), Some(key_p)) = (cert_path, key_path) {
        return load_pem_cert_and_key(cert_p, key_p);
    }

    // Mode 2: try loading persisted certs from cert_dir
    let auto_cert = cert_dir.join("cert.pem");
    let auto_key = cert_dir.join("key.pem");

    if auto_cert.exists() && auto_key.exists() {
        tracing::info!(path = %auto_cert.display(), "loading persisted certificate");
        return load_pem_cert_and_key(&auto_cert, &auto_key);
    }

    // Mode 3: generate and persist
    tracing::info!("generating self-signed certificate");
    let cert = rcgen::generate_simple_self_signed(sans.to_vec())
        .context("failed to generate self-signed certificate")?;

    let cert_pem = cert.cert.pem();
    let key_pem = cert.key_pair.serialize_pem();

    // Try to persist
    if let Err(e) = fs::create_dir_all(cert_dir) {
        tracing::warn!(error = %e, "could not create cert directory, using ephemeral cert");
    } else {
        fs::write(&auto_cert, &cert_pem)
            .and_then(|_| fs::write(&auto_key, &key_pem))
            .unwrap_or_else(|e| {
                tracing::warn!(error = %e, "could not persist certificate to disk");
            });
        tracing::info!(path = %cert_dir.display(), "certificate persisted");
    }

    // Write PEM then reload to get owned DER values
    // This avoids lifetime issues with rcgen's borrowed serialized_der()
    let cert_pem_bytes = cert_pem.as_bytes().to_vec();
    let key_pem_bytes = key_pem.as_bytes().to_vec();

    let certs: Vec<CertificateDer<'static>> =
        rustls_pemfile::certs(&mut &cert_pem_bytes[..])
            .collect::<std::result::Result<Vec<_>, _>>()
            .context("failed to parse generated cert PEM")?;

    let key = rustls_pemfile::private_key(&mut &key_pem_bytes[..])
        .context("failed to parse generated key PEM")?
        .context("no private key in generated PEM")?;

    Ok((certs, key))
}

fn load_pem_cert_and_key(
    cert_path: &Path,
    key_path: &Path,
) -> Result<(Vec<CertificateDer<'static>>, PrivateKeyDer<'static>)> {
    let cert_file = fs::File::open(cert_path)
        .with_context(|| format!("failed to open cert file: {}", cert_path.display()))?;
    let mut cert_reader = BufReader::new(cert_file);
    let certs: Vec<CertificateDer<'static>> = rustls_pemfile::certs(&mut cert_reader)
        .collect::<std::result::Result<Vec<_>, _>>()
        .context("failed to parse certificate PEM")?;

    if certs.is_empty() {
        bail!("no certificates found in {}", cert_path.display());
    }

    let key_file = fs::File::open(key_path)
        .with_context(|| format!("failed to open key file: {}", key_path.display()))?;
    let mut key_reader = BufReader::new(key_file);
    let key = rustls_pemfile::private_key(&mut key_reader)
        .context("failed to parse private key PEM")?
        .ok_or_else(|| anyhow::anyhow!("no private key found in {}", key_path.display()))?;

    Ok((certs, key))
}

// --- Client: build crypto config ---

pub fn build_client_crypto(mode: &CertVerifyMode) -> Result<rustls::ClientConfig> {
    match mode {
        CertVerifyMode::None => {
            let config = rustls::ClientConfig::builder()
                .dangerous()
                .with_custom_certificate_verifier(Arc::new(NoVerifier))
                .with_no_client_auth();
            Ok(config)
        }
        CertVerifyMode::Tofu { known_hosts_path } => {
            let config = rustls::ClientConfig::builder()
                .dangerous()
                .with_custom_certificate_verifier(Arc::new(TofuVerifier {
                    known_hosts_path: known_hosts_path.clone(),
                }))
                .with_no_client_auth();
            Ok(config)
        }
        CertVerifyMode::CaCert { ca_path } => {
            let ca_file = fs::File::open(ca_path)
                .with_context(|| format!("failed to open CA cert: {}", ca_path.display()))?;
            let mut reader = BufReader::new(ca_file);
            let mut root_store = rustls::RootCertStore::empty();
            let certs: Vec<CertificateDer<'static>> = rustls_pemfile::certs(&mut reader)
                .collect::<std::result::Result<Vec<_>, _>>()
                .context("failed to parse CA certificate")?;
            for cert in certs {
                root_store.add(cert).context("failed to add CA cert to root store")?;
            }
            let config = rustls::ClientConfig::builder()
                .with_root_certificates(root_store)
                .with_no_client_auth();
            Ok(config)
        }
    }
}

// --- No verification ---

#[derive(Debug)]
struct NoVerifier;

impl rustls::client::danger::ServerCertVerifier for NoVerifier {
    fn verify_server_cert(
        &self,
        _end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp_response: &[u8],
        _now: rustls::pki_types::UnixTime,
    ) -> Result<rustls::client::danger::ServerCertVerified, rustls::Error> {
        Ok(rustls::client::danger::ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        _message: &[u8],
        _cert: &CertificateDer<'_>,
        _dss: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        Ok(rustls::client::danger::HandshakeSignatureValid::assertion())
    }

    fn verify_tls13_signature(
        &self,
        _message: &[u8],
        _cert: &CertificateDer<'_>,
        _dss: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        Ok(rustls::client::danger::HandshakeSignatureValid::assertion())
    }

    fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
        rustls::crypto::ring::default_provider()
            .signature_verification_algorithms
            .supported_schemes()
    }
}

// --- TOFU verification ---

#[derive(Debug)]
struct TofuVerifier {
    known_hosts_path: PathBuf,
}

impl TofuVerifier {
    fn fingerprint(cert: &CertificateDer<'_>) -> String {
        let hash = Sha256::digest(cert.as_ref());
        format!(
            "SHA256:{}",
            base64::engine::general_purpose::STANDARD_NO_PAD.encode(hash)
        )
    }

    fn lookup(&self, host: &str) -> Option<String> {
        let content = fs::read_to_string(&self.known_hosts_path).ok()?;
        for line in content.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            if let Some((h, fp)) = line.split_once(' ') {
                if h == host {
                    return Some(fp.to_string());
                }
            }
        }
        None
    }

    fn store(&self, host: &str, fingerprint: &str) -> Result<(), std::io::Error> {
        if let Some(parent) = self.known_hosts_path.parent() {
            fs::create_dir_all(parent)?;
        }
        use std::io::Write;
        let mut file = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.known_hosts_path)?;
        writeln!(file, "{} {}", host, fingerprint)?;
        Ok(())
    }
}

impl rustls::client::danger::ServerCertVerifier for TofuVerifier {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        server_name: &ServerName<'_>,
        _ocsp_response: &[u8],
        _now: rustls::pki_types::UnixTime,
    ) -> Result<rustls::client::danger::ServerCertVerified, rustls::Error> {
        let host = server_name.to_str().to_string();
        let fp = Self::fingerprint(end_entity);

        match self.lookup(&host) {
            Some(stored_fp) if stored_fp == fp => {
                tracing::debug!(host = %host, "TOFU: known host, fingerprint matches");
                Ok(rustls::client::danger::ServerCertVerified::assertion())
            }
            Some(stored_fp) => {
                tracing::error!(
                    host = %host,
                    expected = %stored_fp,
                    got = %fp,
                    "TOFU: fingerprint mismatch!"
                );
                Err(rustls::Error::General(format!(
                    "TOFU verification failed for {}: fingerprint changed (expected {}, got {})",
                    host, stored_fp, fp
                )))
            }
            None => {
                tracing::info!(host = %host, fingerprint = %fp, "TOFU: new host, trusting on first use");
                if let Err(e) = self.store(&host, &fp) {
                    tracing::warn!(error = %e, "failed to persist TOFU fingerprint");
                }
                Ok(rustls::client::danger::ServerCertVerified::assertion())
            }
        }
    }

    fn verify_tls12_signature(
        &self,
        _message: &[u8],
        _cert: &CertificateDer<'_>,
        _dss: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        Ok(rustls::client::danger::HandshakeSignatureValid::assertion())
    }

    fn verify_tls13_signature(
        &self,
        _message: &[u8],
        _cert: &CertificateDer<'_>,
        _dss: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        Ok(rustls::client::danger::HandshakeSignatureValid::assertion())
    }

    fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
        rustls::crypto::ring::default_provider()
            .signature_verification_algorithms
            .supported_schemes()
    }
}
