use std::collections::HashMap;
use std::net::SocketAddr;
use std::path::PathBuf;

use anyhow::{Context, Result};
use serde::Deserialize;

use crate::cert::CertVerifyMode;

// --- Defaults ---

const DEFAULT_BUFFER_SIZE: usize = 16384;
const DEFAULT_MAX_IDLE_TIMEOUT_MS: u64 = 60_000;
const DEFAULT_KEEPALIVE_INTERVAL_SECS: u64 = 5;
const DEFAULT_LISTEN: &str = "0.0.0.0:4433";
const DEFAULT_PROXY_TO: &str = "127.0.0.1:22";

// --- File config (TOML) ---

#[derive(Deserialize, Default)]
pub struct FileConfig {
    pub client: Option<ClientFileConfig>,
    pub server: Option<ServerFileConfig>,
}

#[derive(Deserialize, Default)]
pub struct ClientFileConfig {
    pub buffer_size: Option<usize>,
    pub max_idle_timeout_ms: Option<u64>,
    pub keepalive_interval_secs: Option<u64>,
    pub cert_verify: Option<String>,
    pub known_hosts: Option<PathBuf>,
}

#[derive(Deserialize, Default)]
pub struct ServerFileConfig {
    pub listen: Option<SocketAddr>,
    pub proxy_to: Option<SocketAddr>,
    pub buffer_size: Option<usize>,
    pub max_idle_timeout_ms: Option<u64>,
    pub keepalive_interval_secs: Option<u64>,
    pub cert_dir: Option<PathBuf>,
    pub routes: Option<HashMap<String, SocketAddr>>,
}

// --- Resolved configs ---

pub struct ResolvedClientConfig {
    pub url: url::Url,
    pub bind_addr: Option<SocketAddr>,
    pub buffer_size: usize,
    pub max_idle_timeout_ms: u64,
    pub keepalive_interval_secs: u64,
    pub cert_verify_mode: CertVerifyMode,
}

pub struct ResolvedServerConfig {
    pub listen: SocketAddr,
    pub proxy_to: SocketAddr,
    pub buffer_size: usize,
    pub max_idle_timeout_ms: u64,
    pub keepalive_interval_secs: u64,
    pub cert_path: Option<PathBuf>,
    pub key_path: Option<PathBuf>,
    pub cert_dir: PathBuf,
    pub cert_sans: Vec<String>,
    pub routes: HashMap<String, SocketAddr>,
}

// --- CLI args (used by main.rs) ---

#[derive(clap::Args)]
pub struct ClientArgs {
    /// Server URL (quic://host:port)
    pub url: url::Url,

    /// Local bind address
    #[arg(long, short = 'b')]
    pub bind: Option<SocketAddr>,

    /// Buffer size in bytes
    #[arg(long)]
    pub buffer_size: Option<usize>,

    /// Max idle timeout in milliseconds
    #[arg(long)]
    pub max_idle_timeout_ms: Option<u64>,

    /// Keep-alive interval in seconds
    #[arg(long)]
    pub keepalive_interval_secs: Option<u64>,

    /// Certificate verification: "none", "tofu", or path to CA cert
    #[arg(long)]
    pub cert_verify: Option<String>,

    /// Path to store known server fingerprints (for TOFU mode)
    #[arg(long)]
    pub known_hosts: Option<PathBuf>,
}

#[derive(clap::Args)]
pub struct ServerArgs {
    /// Listen address
    #[arg(long, short = 'l')]
    pub listen: Option<SocketAddr>,

    /// Default upstream SSH address
    #[arg(long, short = 'p')]
    pub proxy_to: Option<SocketAddr>,

    /// Buffer size in bytes
    #[arg(long)]
    pub buffer_size: Option<usize>,

    /// Max idle timeout in milliseconds
    #[arg(long)]
    pub max_idle_timeout_ms: Option<u64>,

    /// Keep-alive interval in seconds
    #[arg(long)]
    pub keepalive_interval_secs: Option<u64>,

    /// TLS certificate file (PEM)
    #[arg(long)]
    pub cert: Option<PathBuf>,

    /// TLS private key file (PEM)
    #[arg(long)]
    pub key: Option<PathBuf>,

    /// Directory to store auto-generated certs
    #[arg(long)]
    pub cert_dir: Option<PathBuf>,

    /// SANs for auto-generated cert (comma-separated)
    #[arg(long)]
    pub cert_sans: Option<String>,
}

fn default_cert_dir() -> PathBuf {
    dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("~/.config"))
        .join("qssh")
}

pub fn resolve_client(
    args: ClientArgs,
    file: Option<ClientFileConfig>,
) -> Result<ResolvedClientConfig> {
    let file = file.unwrap_or_default();

    let cert_verify_str = args
        .cert_verify
        .or(file.cert_verify)
        .unwrap_or_else(|| "tofu".to_string());

    let known_hosts = args
        .known_hosts
        .or(file.known_hosts)
        .unwrap_or_else(|| default_cert_dir().join("known_hosts"));

    let cert_verify_mode = match cert_verify_str.as_str() {
        "none" => CertVerifyMode::None,
        "tofu" => CertVerifyMode::Tofu {
            known_hosts_path: known_hosts,
        },
        path => CertVerifyMode::CaCert {
            ca_path: PathBuf::from(path),
        },
    };

    Ok(ResolvedClientConfig {
        url: args.url,
        bind_addr: args.bind,
        buffer_size: args
            .buffer_size
            .or(file.buffer_size)
            .unwrap_or(DEFAULT_BUFFER_SIZE),
        max_idle_timeout_ms: args
            .max_idle_timeout_ms
            .or(file.max_idle_timeout_ms)
            .unwrap_or(DEFAULT_MAX_IDLE_TIMEOUT_MS),
        keepalive_interval_secs: args
            .keepalive_interval_secs
            .or(file.keepalive_interval_secs)
            .unwrap_or(DEFAULT_KEEPALIVE_INTERVAL_SECS),
        cert_verify_mode,
    })
}

pub fn resolve_server(
    args: ServerArgs,
    file: Option<ServerFileConfig>,
) -> Result<ResolvedServerConfig> {
    let file = file.unwrap_or_default();

    let listen = args
        .listen
        .or(file.listen)
        .unwrap_or_else(|| DEFAULT_LISTEN.parse().unwrap());

    let proxy_to = args
        .proxy_to
        .or(file.proxy_to)
        .unwrap_or_else(|| DEFAULT_PROXY_TO.parse().unwrap());

    let cert_dir = args
        .cert_dir
        .or(file.cert_dir)
        .unwrap_or_else(default_cert_dir);

    let cert_sans = args
        .cert_sans
        .map(|s| s.split(',').map(|s| s.trim().to_string()).collect())
        .unwrap_or_else(|| vec!["localhost".to_string()]);

    let routes = file.routes.unwrap_or_default();

    Ok(ResolvedServerConfig {
        listen,
        proxy_to,
        buffer_size: args
            .buffer_size
            .or(file.buffer_size)
            .unwrap_or(DEFAULT_BUFFER_SIZE),
        max_idle_timeout_ms: args
            .max_idle_timeout_ms
            .or(file.max_idle_timeout_ms)
            .unwrap_or(DEFAULT_MAX_IDLE_TIMEOUT_MS),
        keepalive_interval_secs: args
            .keepalive_interval_secs
            .or(file.keepalive_interval_secs)
            .unwrap_or(DEFAULT_KEEPALIVE_INTERVAL_SECS),
        cert_path: args.cert,
        key_path: args.key,
        cert_dir,
        cert_sans,
        routes,
    })
}

pub fn load_file_config(path: &std::path::Path) -> Result<FileConfig> {
    let contents = std::fs::read_to_string(path)
        .with_context(|| format!("failed to read config file: {}", path.display()))?;
    toml::from_str(&contents)
        .with_context(|| format!("failed to parse config file: {}", path.display()))
}
