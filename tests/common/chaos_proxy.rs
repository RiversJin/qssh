use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use tokio::net::UdpSocket;
use tokio::sync::RwLock;

/// A UDP chaos proxy that sits between QUIC client and server.
/// Supports packet loss simulation, delay injection, and source address rewriting
/// (to simulate connection migration).
pub struct ChaosProxy {
    listen_addr: SocketAddr,
    target_addr: SocketAddr,
    config: Arc<ChaosConfig>,
    stop: Arc<AtomicBool>,
}

pub struct ChaosConfig {
    /// Packet loss rate (0.0 - 1.0)
    pub loss_rate: RwLock<f64>,
    /// Per-packet delay in milliseconds
    pub delay_ms: RwLock<u64>,
    /// Counters
    pub packets_forwarded: AtomicU64,
    pub packets_dropped: AtomicU64,
}

impl ChaosConfig {
    pub fn new(loss_rate: f64, delay_ms: u64) -> Self {
        Self {
            loss_rate: RwLock::new(loss_rate),
            delay_ms: RwLock::new(delay_ms),
            packets_forwarded: AtomicU64::new(0),
            packets_dropped: AtomicU64::new(0),
        }
    }

    pub async fn set_loss_rate(&self, rate: f64) {
        *self.loss_rate.write().await = rate;
    }

    pub async fn set_delay_ms(&self, ms: u64) {
        *self.delay_ms.write().await = ms;
    }

    pub fn forwarded(&self) -> u64 {
        self.packets_forwarded.load(Ordering::Relaxed)
    }

    pub fn dropped(&self) -> u64 {
        self.packets_dropped.load(Ordering::Relaxed)
    }
}

impl ChaosProxy {
    pub async fn start(
        target_addr: SocketAddr,
        loss_rate: f64,
        delay_ms: u64,
    ) -> anyhow::Result<(Self, SocketAddr)> {
        let socket = UdpSocket::bind("127.0.0.1:0").await?;
        let listen_addr = socket.local_addr()?;
        let config = Arc::new(ChaosConfig::new(loss_rate, delay_ms));
        let stop = Arc::new(AtomicBool::new(false));

        let proxy = Self {
            listen_addr,
            target_addr,
            config: config.clone(),
            stop: stop.clone(),
        };

        let target = target_addr;
        tokio::spawn(Self::run_loop(socket, target, config, stop));

        Ok((proxy, listen_addr))
    }

    pub fn config(&self) -> &Arc<ChaosConfig> {
        &self.config
    }

    pub fn listen_addr(&self) -> SocketAddr {
        self.listen_addr
    }

    async fn run_loop(
        socket: UdpSocket,
        target_addr: SocketAddr,
        config: Arc<ChaosConfig>,
        stop: Arc<AtomicBool>,
    ) {
        let socket = Arc::new(socket);
        let mut buf = vec![0u8; 65535];
        // Map: server-side "client addr" as seen by server -> real client addr
        // Since we forward from our socket to server, server sees our addr.
        // We track which real client each packet came from.
        let clients: Arc<RwLock<HashMap<SocketAddr, SocketAddr>>> =
            Arc::new(RwLock::new(HashMap::new()));

        // We need to remember: when we get a packet from a client,
        // we forward it to the target. When target replies, we send back to the client.
        // Since we use one socket, target replies come back to us.
        // We identify client vs server packets by source address.
        let mut client_addr: Option<SocketAddr> = None;

        loop {
            if stop.load(Ordering::Relaxed) {
                break;
            }

            let (len, from) = match socket.recv_from(&mut buf).await {
                Ok(r) => r,
                Err(_) => break,
            };

            let loss_rate = *config.loss_rate.read().await;
            let delay_ms = *config.delay_ms.read().await;

            // Determine direction
            let (dest, direction) = if from == target_addr {
                // Server -> Client (response)
                match client_addr {
                    Some(addr) => (addr, "server->client"),
                    None => continue, // No client registered yet
                }
            } else {
                // Client -> Server
                client_addr = Some(from);
                (target_addr, "client->server")
            };

            // Apply packet loss
            if loss_rate > 0.0 && rand_drop(loss_rate) {
                config.packets_dropped.fetch_add(1, Ordering::Relaxed);
                continue;
            }

            let data = buf[..len].to_vec();
            let socket = socket.clone();
            let config = config.clone();

            // Apply delay
            if delay_ms > 0 {
                tokio::spawn(async move {
                    tokio::time::sleep(Duration::from_millis(delay_ms)).await;
                    if socket.send_to(&data, dest).await.is_ok() {
                        config.packets_forwarded.fetch_add(1, Ordering::Relaxed);
                    }
                });
            } else {
                if socket.send_to(&data[..len], dest).await.is_ok() {
                    config.packets_forwarded.fetch_add(1, Ordering::Relaxed);
                }
            }
        }
    }
}

impl Drop for ChaosProxy {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

/// Simple pseudo-random drop decision using thread-local RNG.
fn rand_drop(rate: f64) -> bool {
    use std::hash::{Hash, Hasher};
    use std::collections::hash_map::DefaultHasher;

    let mut hasher = DefaultHasher::new();
    std::thread::current().id().hash(&mut hasher);
    std::time::Instant::now().hash(&mut hasher);
    // Mix in a counter for better distribution
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    COUNTER.fetch_add(1, Ordering::Relaxed).hash(&mut hasher);

    let hash = hasher.finish();
    let normalized = (hash as f64) / (u64::MAX as f64);
    normalized < rate
}
