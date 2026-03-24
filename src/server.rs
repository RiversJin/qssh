use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use quinn::crypto::rustls::QuicServerConfig;

use crate::config::ResolvedServerConfig;
use crate::{cert, relay};

pub async fn run(config: ResolvedServerConfig) -> Result<()> {
    let (certs, key) = cert::load_or_generate_server_cert(
        config.cert_path.as_deref(),
        config.key_path.as_deref(),
        &config.cert_dir,
        &config.cert_sans,
    )?;

    let server_crypto = rustls::ServerConfig::builder()
        .with_no_client_auth()
        .with_single_cert(certs, key)
        .context("failed to build TLS server config")?;

    let mut server_config = quinn::ServerConfig::with_crypto(Arc::new(
        QuicServerConfig::try_from(server_crypto).context("failed to build QUIC server config")?,
    ));

    let transport = Arc::get_mut(&mut server_config.transport).unwrap();
    transport.max_concurrent_uni_streams(0u8.into());
    transport.max_idle_timeout(Some(
        quinn::IdleTimeout::try_from(Duration::from_millis(config.max_idle_timeout_ms))
            .map_err(|e| anyhow::anyhow!("invalid idle timeout: {}", e))?,
    ));
    transport.keep_alive_interval(Some(Duration::from_secs(config.keepalive_interval_secs)));
    transport.mtu_discovery_config(Some(quinn::MtuDiscoveryConfig::default()));

    let endpoint = quinn::Endpoint::server(server_config, config.listen)
        .context("failed to bind QUIC server")?;

    tracing::info!(listen = %config.listen, proxy_to = %config.proxy_to, "server listening");

    let endpoint_for_signal = endpoint.clone();
    tokio::spawn(async move {
        tokio::signal::ctrl_c().await.ok();
        tracing::info!("shutdown signal received, draining connections");
        endpoint_for_signal.close(0u32.into(), b"server shutdown");
    });

    let routes = Arc::new(config.routes);
    let default_proxy = config.proxy_to;
    let buf_size = config.buffer_size;

    while let Some(incoming) = endpoint.accept().await {
        let routes = routes.clone();
        tokio::spawn(async move {
            if let Err(e) =
                handle_incoming(incoming, &routes, default_proxy, buf_size).await
            {
                tracing::error!(error = %e, "connection handler failed");
            }
        });
    }

    tracing::info!("server shut down");
    Ok(())
}

async fn handle_incoming(
    incoming: quinn::Incoming,
    routes: &HashMap<String, SocketAddr>,
    default_proxy: SocketAddr,
    buf_size: usize,
) -> Result<()> {
    let connection = incoming.await.context("QUIC handshake failed")?;
    let remote = connection.remote_address();

    let sni = connection
        .handshake_data()
        .and_then(|hd| {
            hd.downcast::<quinn::crypto::rustls::HandshakeData>()
                .ok()
        })
        .and_then(|hd| hd.server_name.clone())
        .unwrap_or_else(|| remote.ip().to_string());

    let proxy_to = routes.get(&sni).copied().unwrap_or(default_proxy);
    tracing::info!(remote = %remote, sni = %sni, proxy_to = %proxy_to, "connection accepted");

    let (send, recv) = connection
        .accept_bi()
        .await
        .context("failed to accept bidirectional stream")?;

    let tcp = tokio::net::TcpStream::connect(proxy_to)
        .await
        .with_context(|| format!("failed to connect to upstream SSH at {}", proxy_to))?;

    let (tcp_read, tcp_write) = tcp.into_split();

    relay::bidirectional(recv, send, tcp_read, tcp_write, buf_size).await?;

    tracing::info!(remote = %remote, "connection closed");
    Ok(())
}
