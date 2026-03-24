mod common;

use std::time::Duration;

use anyhow::Result;

/// Helper: create a full test stack (echo server + QUIC relay + optional chaos proxy).
/// Returns (client_endpoint, target_addr_for_client, server_endpoint).
async fn setup_stack(
    loss_rate: f64,
    delay_ms: u64,
) -> Result<(
    quinn::Endpoint,
    std::net::SocketAddr,
    quinn::Endpoint,
    Option<common::chaos_proxy::ChaosProxy>,
)> {
    let echo_addr = common::start_echo_server().await?;
    let (server_config, client_config) = common::make_quic_configs(30_000, 1)?;
    let (quic_addr, server_ep) =
        common::start_quic_relay_server(server_config, echo_addr).await?;

    let (proxy, connect_addr) = if loss_rate > 0.0 || delay_ms > 0 {
        let (p, addr) =
            common::chaos_proxy::ChaosProxy::start(quic_addr, loss_rate, delay_ms).await?;
        (Some(p), addr)
    } else {
        (None, quic_addr)
    };

    let mut client_ep = quinn::Endpoint::client("127.0.0.1:0".parse()?)?;
    client_ep.set_default_client_config(client_config);

    Ok((client_ep, connect_addr, server_ep, proxy))
}

/// Send data through the QUIC tunnel and verify echo.
async fn echo_roundtrip(
    client_ep: &quinn::Endpoint,
    connect_addr: std::net::SocketAddr,
    data: &[u8],
    timeout_secs: u64,
) -> Result<()> {
    let conn = client_ep.connect(connect_addr, "localhost")?.await?;

    let (mut send, mut recv) = conn.open_bi().await?;

    // Send data and signal EOF
    send.write_all(data).await?;
    send.finish()?;

    // Read the echo response
    let response = tokio::time::timeout(
        Duration::from_secs(timeout_secs),
        recv.read_to_end(data.len()),
    )
    .await??;

    assert_eq!(
        response.len(),
        data.len(),
        "response length mismatch: got {} expected {}",
        response.len(),
        data.len()
    );
    assert_eq!(&response, data, "response data mismatch");

    conn.close(0u32.into(), b"done");
    Ok(())
}

// ============================================================
// Tests
// ============================================================

#[tokio::test]
async fn test_basic_relay_no_loss() -> Result<()> {
    let (client_ep, connect_addr, _server_ep, _proxy) = setup_stack(0.0, 0).await?;

    let data = b"hello quicssh";
    echo_roundtrip(&client_ep, connect_addr, data, 5).await?;

    Ok(())
}

#[tokio::test]
async fn test_large_data_no_loss() -> Result<()> {
    let (client_ep, connect_addr, _server_ep, _proxy) = setup_stack(0.0, 0).await?;

    // 1MB of patterned data
    let data: Vec<u8> = (0..1_048_576).map(|i| (i % 256) as u8).collect();
    echo_roundtrip(&client_ep, connect_addr, &data, 10).await?;

    Ok(())
}

#[tokio::test]
async fn test_packet_loss_10_percent() -> Result<()> {
    let (client_ep, connect_addr, _server_ep, proxy) = setup_stack(0.10, 0).await?;

    let data: Vec<u8> = (0..65536).map(|i| (i % 256) as u8).collect();
    echo_roundtrip(&client_ep, connect_addr, &data, 15).await?;

    let cfg = proxy.as_ref().unwrap().config();
    let dropped = cfg.dropped();
    let forwarded = cfg.forwarded();
    eprintln!(
        "10% loss test: forwarded={}, dropped={}, drop_rate={:.1}%",
        forwarded,
        dropped,
        (dropped as f64 / (forwarded + dropped) as f64) * 100.0
    );
    assert!(dropped > 0, "expected some packets to be dropped");

    Ok(())
}

#[tokio::test]
async fn test_packet_loss_30_percent() -> Result<()> {
    let (client_ep, connect_addr, _server_ep, proxy) = setup_stack(0.30, 0).await?;

    let data: Vec<u8> = (0..32768).map(|i| (i % 256) as u8).collect();
    echo_roundtrip(&client_ep, connect_addr, &data, 30).await?;

    let cfg = proxy.as_ref().unwrap().config();
    let dropped = cfg.dropped();
    let forwarded = cfg.forwarded();
    eprintln!(
        "30% loss test: forwarded={}, dropped={}, drop_rate={:.1}%",
        forwarded,
        dropped,
        (dropped as f64 / (forwarded + dropped) as f64) * 100.0
    );
    assert!(dropped > 0, "expected some packets to be dropped");

    Ok(())
}

#[tokio::test]
async fn test_high_latency_200ms() -> Result<()> {
    let (client_ep, connect_addr, _server_ep, _proxy) = setup_stack(0.0, 100).await?;

    // Smaller data due to high latency
    let data = b"latency test data - quicssh handles delay well";
    echo_roundtrip(&client_ep, connect_addr, data, 30).await?;

    Ok(())
}

#[tokio::test]
async fn test_connection_migration() -> Result<()> {
    // Test that QUIC survives endpoint rebind (simulates IP change)
    let echo_addr = common::start_echo_server().await?;
    let (server_config, client_config) = common::make_quic_configs(30_000, 1)?;
    let (quic_addr, _server_ep) =
        common::start_quic_relay_server(server_config, echo_addr).await?;

    let mut client_ep = quinn::Endpoint::client("127.0.0.1:0".parse()?)?;
    client_ep.set_default_client_config(client_config);

    let conn = client_ep.connect(quic_addr, "localhost")?.await?;

    // First exchange — before migration
    {
        let (mut send, mut recv) = conn.open_bi().await?;
        send.write_all(b"before migration").await?;
        send.finish()?;
        let resp = recv.read_to_end(64).await?;
        assert_eq!(&resp, b"before migration");
    }

    let old_addr = client_ep.local_addr()?;

    // Simulate connection migration: rebind to a new UDP socket
    let new_socket = std::net::UdpSocket::bind("127.0.0.1:0")?;
    let new_addr = new_socket.local_addr()?;
    client_ep.rebind(new_socket)?;

    eprintln!(
        "connection migration: {} -> {}",
        old_addr, new_addr
    );
    assert_ne!(old_addr, new_addr, "should have new address after rebind");

    // Small delay for migration to propagate
    tokio::time::sleep(Duration::from_millis(100)).await;

    // Second exchange — after migration
    {
        let (mut send, mut recv) = conn.open_bi().await?;
        send.write_all(b"after migration").await?;
        send.finish()?;
        let resp = tokio::time::timeout(Duration::from_secs(10), recv.read_to_end(64)).await??;
        assert_eq!(&resp, b"after migration");
    }

    conn.close(0u32.into(), b"done");
    eprintln!("connection migration test passed!");

    Ok(())
}

#[tokio::test]
async fn test_loss_with_latency() -> Result<()> {
    // Realistic bad network: 10% loss + 50ms delay
    let (client_ep, connect_addr, _server_ep, proxy) = setup_stack(0.10, 50).await?;

    let data: Vec<u8> = (0..16384).map(|i| (i % 256) as u8).collect();
    echo_roundtrip(&client_ep, connect_addr, &data, 30).await?;

    let cfg = proxy.as_ref().unwrap().config();
    eprintln!(
        "loss+latency test: forwarded={}, dropped={}",
        cfg.forwarded(),
        cfg.dropped()
    );

    Ok(())
}

#[tokio::test]
async fn test_dynamic_loss_change() -> Result<()> {
    // Start with chaos proxy at 0% loss, then increase mid-connection
    let echo_addr = common::start_echo_server().await?;
    let (server_config, client_config) = common::make_quic_configs(30_000, 1)?;
    let (quic_addr, _server_ep) =
        common::start_quic_relay_server(server_config, echo_addr).await?;
    let (proxy, connect_addr) =
        common::chaos_proxy::ChaosProxy::start(quic_addr, 0.0, 0).await?;
    let mut client_ep = quinn::Endpoint::client("127.0.0.1:0".parse()?)?;
    client_ep.set_default_client_config(client_config);
    let cfg = proxy.config().clone();

    // First exchange: no loss
    let conn = client_ep.connect(connect_addr, "localhost")?.await?;
    {
        let (mut send, mut recv) = conn.open_bi().await?;
        send.write_all(b"clean network").await?;
        send.finish()?;
        let resp = recv.read_to_end(64).await?;
        assert_eq!(&resp, b"clean network");
    }

    // Switch to 20% packet loss
    cfg.set_loss_rate(0.20).await;
    eprintln!("switched to 20% packet loss");

    // Second exchange: with loss (new bidi stream on same connection)
    {
        let (mut send, mut recv) = conn.open_bi().await?;
        let data: Vec<u8> = (0..262144).map(|i| (i % 256) as u8).collect();
        send.write_all(&data).await?;
        send.finish()?;
        let resp = tokio::time::timeout(Duration::from_secs(15), recv.read_to_end(data.len()))
            .await??;
        assert_eq!(resp, data);
    }

    let dropped = cfg.dropped();
    eprintln!("dynamic loss test: dropped {} packets after enabling loss", dropped);
    assert!(dropped > 0, "expected drops after enabling loss");

    conn.close(0u32.into(), b"done");
    Ok(())
}
