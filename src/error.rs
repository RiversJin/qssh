use thiserror::Error;

#[derive(Error, Debug)]
pub enum QuicsshError {
    #[error("invalid URL scheme: expected 'quic', got '{0}'")]
    InvalidScheme(String),

    #[error("could not resolve address for URL: {0}")]
    DnsResolution(String),

    #[error("certificate error: {0}")]
    Certificate(String),

    #[error("TOFU verification failed: server certificate fingerprint changed for {host}")]
    TofuMismatch { host: String },

    #[error("configuration error: {0}")]
    Config(String),
}
