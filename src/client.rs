use std::net::{SocketAddr, ToSocketAddrs};
use std::sync::Arc;
use std::time::Duration;

use anyhow::{bail, Context, Result};
use quinn::crypto::rustls::QuicClientConfig;

use crate::config::ResolvedClientConfig;
use crate::{cert, relay};

pub async fn run(config: ResolvedClientConfig) -> Result<()> {
    if config.url.scheme() != "quic" {
        bail!("invalid URL scheme: expected 'quic', got '{}'", config.url.scheme());
    }

    let host = config
        .url
        .host_str()
        .context("URL has no host")?;
    let port = config.url.port().unwrap_or(4433);

    let remote_addr: SocketAddr = format!("{}:{}", host, port)
        .to_socket_addrs()
        .with_context(|| format!("failed to resolve {}", host))?
        .next()
        .with_context(|| format!("no addresses found for {}", host))?;

    let sni = host.trim_start_matches('[').trim_end_matches(']');

    let crypto = cert::build_client_crypto(&config.cert_verify_mode)?;
    let mut client_config = quinn::ClientConfig::new(Arc::new(
        QuicClientConfig::try_from(crypto).context("failed to build QUIC client config")?,
    ));

    let mut transport = quinn::TransportConfig::default();
    transport.max_idle_timeout(Some(
        quinn::IdleTimeout::try_from(Duration::from_millis(config.max_idle_timeout_ms))
            .map_err(|e| anyhow::anyhow!("invalid idle timeout: {}", e))?,
    ));
    transport.keep_alive_interval(Some(Duration::from_secs(config.keepalive_interval_secs)));
    transport.mtu_discovery_config(Some(quinn::MtuDiscoveryConfig::default()));
    client_config.transport_config(Arc::new(transport));

    let bind_addr = config.bind_addr.unwrap_or_else(|| {
        if remote_addr.is_ipv6() {
            "[::]:0".parse().unwrap()
        } else {
            "0.0.0.0:0".parse().unwrap()
        }
    });

    let mut endpoint = quinn::Endpoint::client(bind_addr)
        .context("failed to create QUIC endpoint")?;
    endpoint.set_default_client_config(client_config);

    let connection = endpoint
        .connect(remote_addr, sni)?
        .await
        .context("QUIC connection failed")?;

    tracing::info!(remote = %remote_addr, sni = %sni, "connected to server");

    let (send, recv) = connection
        .open_bi()
        .await
        .context("failed to open bidirectional stream")?;

    let stdin = tokio::io::stdin();
    let stdout = tokio::io::stdout();

    let relay = relay::bidirectional(stdin, stdout, recv, send, config.buffer_size);

    let shutdown = async {
        #[cfg(unix)]
        {
            let mut hup = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::hangup())
                .expect("failed to register SIGHUP handler");
            tokio::select! {
                _ = tokio::signal::ctrl_c() => {}
                _ = hup.recv() => {}
            }
        }
        #[cfg(not(unix))]
        {
            tokio::signal::ctrl_c().await.ok();
        }
    };

    tokio::select! {
        result = relay => {
            if let Err(e) = &result {
                tracing::debug!(error = %e, "relay finished with error");
            }
        }
        _ = shutdown => {
            tracing::info!("shutdown signal received");
            connection.close(0u32.into(), b"client shutdown");
        }
    }

    endpoint.wait_idle().await;
    Ok(())
}
