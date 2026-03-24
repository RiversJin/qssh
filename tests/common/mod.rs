pub mod chaos_proxy;

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use quinn::crypto::rustls::QuicClientConfig;
use quinn::crypto::rustls::QuicServerConfig;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

/// Start a TCP echo server, returns its listen address.
pub async fn start_echo_server() -> Result<SocketAddr> {
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let addr = listener.local_addr()?;

    tokio::spawn(async move {
        loop {
            let (mut stream, _) = match listener.accept().await {
                Ok(s) => s,
                Err(_) => break,
            };
            tokio::spawn(async move {
                let mut buf = vec![0u8; 16384];
                loop {
                    let n = match stream.read(&mut buf).await {
                        Ok(0) | Err(_) => break,
                        Ok(n) => n,
                    };
                    if stream.write_all(&buf[..n]).await.is_err() {
                        break;
                    }
                }
            });
        }
    });

    Ok(addr)
}

/// Create a self-signed cert and return (server_config, client_config).
pub fn make_quic_configs(
    idle_timeout_ms: u64,
    keepalive_secs: u64,
) -> Result<(quinn::ServerConfig, quinn::ClientConfig)> {
    let cert = rcgen::generate_simple_self_signed(vec!["localhost".into()])?;
    let cert_der = cert.cert.der().clone();
    let key_pem = cert.key_pair.serialize_pem();

    // Parse key from PEM
    let key_der = rustls_pemfile::private_key(&mut key_pem.as_bytes())?
        .expect("no key in PEM");

    // Server
    let server_crypto = rustls::ServerConfig::builder()
        .with_no_client_auth()
        .with_single_cert(vec![cert_der], key_der)?;

    let mut server_config = quinn::ServerConfig::with_crypto(Arc::new(
        QuicServerConfig::try_from(server_crypto)?,
    ));
    let transport = Arc::get_mut(&mut server_config.transport).unwrap();
    transport.max_concurrent_uni_streams(0u8.into());
    transport.max_idle_timeout(Some(quinn::IdleTimeout::try_from(
        Duration::from_millis(idle_timeout_ms),
    )?));
    transport.keep_alive_interval(Some(Duration::from_secs(keepalive_secs)));

    // Client (skip verification)
    let client_crypto = rustls::ClientConfig::builder()
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(NoVerifier))
        .with_no_client_auth();

    let mut client_config = quinn::ClientConfig::new(Arc::new(
        QuicClientConfig::try_from(client_crypto)?,
    ));
    let mut client_transport = quinn::TransportConfig::default();
    client_transport.max_idle_timeout(Some(quinn::IdleTimeout::try_from(
        Duration::from_millis(idle_timeout_ms),
    )?));
    client_transport.keep_alive_interval(Some(Duration::from_secs(keepalive_secs)));
    client_config.transport_config(Arc::new(client_transport));

    Ok((server_config, client_config))
}

/// Start a quicssh-like QUIC server that relays to a TCP target.
/// Returns the QUIC listen address.
pub async fn start_quic_relay_server(
    server_config: quinn::ServerConfig,
    proxy_to: SocketAddr,
) -> Result<(SocketAddr, quinn::Endpoint)> {
    let endpoint = quinn::Endpoint::server(server_config, "127.0.0.1:0".parse()?)?;
    let addr = endpoint.local_addr()?;

    let ep = endpoint.clone();
    tokio::spawn(async move {
        while let Some(incoming) = ep.accept().await {
            let proxy_to = proxy_to;
            tokio::spawn(async move {
                let connection = match incoming.await {
                    Ok(c) => c,
                    Err(_) => return,
                };
                // Accept multiple bidi streams per connection
                loop {
                    let (send, recv) = match connection.accept_bi().await {
                        Ok(s) => s,
                        Err(_) => break,
                    };
                    let proxy_to = proxy_to;
                    tokio::spawn(async move {
                        let tcp = match tokio::net::TcpStream::connect(proxy_to).await {
                            Ok(t) => t,
                            Err(_) => return,
                        };
                        let (tcp_read, tcp_write) = tcp.into_split();

                        let a = tokio::spawn(copy_stream(recv, tcp_write));
                        let b = tokio::spawn(copy_stream(tcp_read, send));
                        let _ = tokio::join!(a, b);
                    });
                }
            });
        }
    });

    Ok((addr, endpoint))
}

async fn copy_stream<R, W>(mut reader: R, mut writer: W)
where
    R: tokio::io::AsyncRead + Unpin,
    W: tokio::io::AsyncWrite + Unpin,
{
    let mut buf = vec![0u8; 16384];
    loop {
        let n = match reader.read(&mut buf).await {
            Ok(0) | Err(_) => break,
            Ok(n) => n,
        };
        if writer.write_all(&buf[..n]).await.is_err() {
            break;
        }
        if writer.flush().await.is_err() {
            break;
        }
    }
    writer.shutdown().await.ok();
}

// --- NoVerifier for tests ---

#[derive(Debug)]
struct NoVerifier;

impl rustls::client::danger::ServerCertVerifier for NoVerifier {
    fn verify_server_cert(
        &self,
        _: &rustls::pki_types::CertificateDer<'_>,
        _: &[rustls::pki_types::CertificateDer<'_>],
        _: &rustls::pki_types::ServerName<'_>,
        _: &[u8],
        _: rustls::pki_types::UnixTime,
    ) -> Result<rustls::client::danger::ServerCertVerified, rustls::Error> {
        Ok(rustls::client::danger::ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        _: &[u8],
        _: &rustls::pki_types::CertificateDer<'_>,
        _: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        Ok(rustls::client::danger::HandshakeSignatureValid::assertion())
    }

    fn verify_tls13_signature(
        &self,
        _: &[u8],
        _: &rustls::pki_types::CertificateDer<'_>,
        _: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        Ok(rustls::client::danger::HandshakeSignatureValid::assertion())
    }

    fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
        rustls::crypto::ring::default_provider()
            .signature_verification_algorithms
            .supported_schemes()
    }
}
