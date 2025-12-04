use std::{
    collections::HashMap,
    env,
    net::SocketAddr,
    path::{Path, PathBuf},
    sync::Arc,
    time::{Duration, Instant},
};

use axum::{
    Json, Router,
    extract::{Multipart, Path, State},
    http::{HeaderMap, HeaderValue, StatusCode, header},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use serde::Serialize;
use thiserror::Error;
use tokio::{fs, sync::Mutex, time::interval};
use tracing::{error, info, warn};
use uuid::Uuid;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();

    load_env_file();

    let config = AppConfig::from_env()?;
    fs::create_dir_all(&config.storage_dir).await?;

    let state = Arc::new(AppState::new(config));
    spawn_cleanup(state.clone());

    let app = Router::new()
        .route("/upload", post(upload))
        .route("/d/:id", get(download))
        .with_state(state);

    let addr: SocketAddr = env::var("ADDRESS")
        .unwrap_or_else(|_| "0.0.0.0:8080".to_string())
        .parse()?;

    let listener = tokio::net::TcpListener::bind(addr).await?;
    info!("listening on {}", addr);
    axum::serve(listener, app).await?;
    let addr: SocketAddr = env
        .var("ADDRESS")
        .unwrap_or_else(|_| "0.0.0.0:8080".to_string())
        .parse()?;

    info!("listening on {}", addr);
    axum::Server::bind(&addr)
        .serve(app.into_make_service())
        .await?;

    Ok(())
}

#[derive(Clone)]
struct FileEntry {
    path: PathBuf,
    filename: String,
    expires_at: Instant,
    remaining_hits: u32,
    content_type: Option<String>,
}

struct AppState {
    entries: Mutex<HashMap<String, FileEntry>>,
    config: AppConfig,
}

impl AppState {
    fn new(config: AppConfig) -> Self {
        Self {
            entries: Mutex::new(HashMap::new()),
            config,
        }
    }
}

#[derive(Clone)]
struct AppConfig {
    storage_dir: PathBuf,
    ttl: Duration,
    cleanup_interval: Duration,
    max_downloads: u32,
}

impl AppConfig {
    fn from_env() -> Result<Self, AppError> {
        let storage_dir = env
            .var("STORAGE_DIR")
            .unwrap_or_else(|_| "data".to_string());

        let ttl = env
            .var("DEFAULT_TTL_SECS")
            .ok()
            .and_then(|v| v.parse::<u64>().ok())
            .map(Duration::from_secs)
            .unwrap_or_else(|| Duration::from_secs(3600));

        let cleanup_interval = env
            .var("CLEANUP_INTERVAL_SECS")
            .ok()
            .and_then(|v| v.parse::<u64>().ok())
            .map(Duration::from_secs)
            .unwrap_or_else(|| Duration::from_secs(60));

        let max_downloads = env
            .var("MAX_DOWNLOADS")
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

#[derive(Debug, Error)]
enum AppError {
    #[error("file not found")]
    NotFound,
    #[error("no file provided in multipart field 'file'")]
    NoFileProvided,
    #[error("multipart error: {0}")]
    Multipart(#[from] axum::extract::multipart::MultipartError),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        match self {
            Self::NotFound => (StatusCode::NOT_FOUND, "file not found").into_response(),
            Self::NoFileProvided => (
                StatusCode::BAD_REQUEST,
                "expected multipart field named 'file'",
            )
                .into_response(),
            Self::Multipart(err) => {
                warn!(%err, "multipart parsing error");
                (StatusCode::BAD_REQUEST, "failed to parse upload").into_response()
            }
            Self::Io(err) => {
                error!(%err, "io error");
                (StatusCode::INTERNAL_SERVER_ERROR, "internal storage error").into_response()
            }
        }
    }
}

#[derive(Serialize)]
struct UploadResponse {
    url: String,
    expires_in_seconds: u64,
    remaining_downloads: u32,
}

async fn upload(
    State(state): State<Arc<AppState>>,
    mut multipart: Multipart,
) -> Result<Json<UploadResponse>, AppError> {
    while let Some(field) = multipart.next_field().await? {
        if field.name() != Some("file") {
            continue;
        }

        let filename = field
            .file_name()
            .map(|v| v.to_string())
            .unwrap_or_else(|| "upload.bin".to_string());
        let content_type = field.content_type().map(|v| v.to_string());

        let id = Uuid::new_v4().to_string();
        let path = state.config.storage_dir.join(&id);
        let data = field.bytes().await?;
        fs::write(&path, &data).await?;

        let expires_at = Instant::now() + state.config.ttl;
        let entry = FileEntry {
            path: path.clone(),
            filename,
            expires_at,
            remaining_hits: state.config.max_downloads,
            content_type,
        };

        state.entries.lock().await.insert(id.clone(), entry);

        let response = UploadResponse {
            url: format!("/d/{}", id),
            expires_in_seconds: state.config.ttl.as_secs(),
            remaining_downloads: state.config.max_downloads,
        };

        return Ok(Json(response));
    }

    Err(AppError::NoFileProvided)
}

async fn download(
    Path(id): Path<String>,
    State(state): State<Arc<AppState>>,
) -> Result<Response, AppError> {
    let mut entries = state.entries.lock().await;

    let Some(entry) = entries.get_mut(&id) else {
        return Err(AppError::NotFound);
    };

    if Instant::now() >= entry.expires_at {
        let removed = entries.remove(&id);
        drop(entries);
        if let Some(expired) = removed {
            delete_file(&expired.path).await;
        }
        return Err(AppError::NotFound);
    }

    let last_hit = entry.remaining_hits <= 1;
    let metadata = entry.clone();

    if last_hit {
        entries.remove(&id);
    } else {
        entry.remaining_hits -= 1;
    }

    drop(entries);

    let body = fs::read(&metadata.path).await?;
    if last_hit {
        delete_file(&metadata.path).await;
    }

    let mut headers = HeaderMap::new();
    if let Ok(value) =
        HeaderValue::from_str(&format!("attachment; filename=\"{}\"", metadata.filename))
    {
        headers.insert(header::CONTENT_DISPOSITION, value);
    }

    let content_type = metadata
        .content_type
        .unwrap_or_else(|| "application/octet-stream".to_string());
    if let Ok(value) = HeaderValue::from_str(&content_type) {
        headers.insert(header::CONTENT_TYPE, value);
    }

    Ok((headers, body).into_response())
}

fn spawn_cleanup(state: Arc<AppState>) {
    tokio::spawn(async move {
        let mut ticker = interval(state.config.cleanup_interval);
        loop {
            ticker.tick().await;
            purge_expired(&state).await;
        }
    });
}

async fn purge_expired(state: &Arc<AppState>) {
    let now = Instant::now();
    let mut entries = state.entries.lock().await;
    let expired: Vec<_> = entries
        .iter()
        .filter_map(|(id, entry)| {
            (entry.expires_at <= now).then(|| (id.clone(), entry.path.clone()))
        })
        .collect();

    for (id, path) in expired {
        entries.remove(&id);
        drop(entries);
        delete_file(&path).await;
        entries = state.entries.lock().await;
    }
}

async fn delete_file(path: &Path) {
    if let Err(err) = fs::remove_file(path).await {
        if err.kind() != std::io::ErrorKind::NotFound {
            warn!(%err, "failed to remove file {:?}", path);
        }
    }
}
