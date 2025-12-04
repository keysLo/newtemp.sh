use std::{env, io::ErrorKind, net::SocketAddr, path::PathBuf, time::Duration};

use dotenvy::dotenv;
use tracing::warn;

use crate::AppError;

#[derive(Clone)]
pub struct AppConfig {
    pub address: SocketAddr,
    pub storage_dir: PathBuf,
    pub ttl: Duration,
    pub cleanup_interval: Duration,
    pub max_downloads: u32,
}

impl AppConfig {
    pub fn from_env() -> Result<Self, AppError> {
        let address = env::var("ADDRESS").unwrap_or_else(|_| "0.0.0.0:8080".to_string());

        let storage_dir = env::var("STORAGE_DIR").unwrap_or_else(|_| "data".to_string());

        let ttl = env::var("DEFAULT_TTL_MINS")
            .ok()
            .and_then(|v| v.parse::<u64>().ok())
            .map(|minutes| minutes.saturating_mul(60))
            .map(Duration::from_secs)
            .unwrap_or_else(|| Duration::from_secs(60 * 60));

        let cleanup_interval = env::var("CLEANUP_INTERVAL_MINS")
            .ok()
            .and_then(|v| v.parse::<u64>().ok())
            .map(|minutes| minutes.saturating_mul(60))
            .map(Duration::from_secs)
            .unwrap_or_else(|| Duration::from_secs(60));

        let max_downloads = env::var("MAX_DOWNLOADS")
            .ok()
            .and_then(|v| v.parse::<u32>().ok())
            .unwrap_or(3);

        Ok(Self {
            address: address.parse().unwrap_or_else(|err| {
                warn!(%err, "invalid ADDRESS value, falling back to default");
                SocketAddr::from(([0, 0, 0, 0], 8080))
            }),
            storage_dir: PathBuf::from(storage_dir),
            ttl,
            cleanup_interval,
            max_downloads,
        })
    }
}

pub fn load_env_file() {
    if let Err(err) = dotenv() {
        if !matches!(err, dotenvy::Error::Io(ref io_err) if io_err.kind() == ErrorKind::NotFound) {
            warn!(%err, "failed to load .env file");
        }
    }
}
