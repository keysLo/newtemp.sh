use std::{env, io::ErrorKind, path::PathBuf, time::Duration};

use dotenvy::dotenv;
use tracing::warn;

use crate::AppError;

#[derive(Clone)]
pub struct AppConfig {
    pub storage_dir: PathBuf,
    pub ttl: Duration,
    pub cleanup_interval: Duration,
    pub max_downloads: u32,
}

impl AppConfig {
    pub fn from_env() -> Result<Self, AppError> {
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
