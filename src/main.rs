use std::{
    collections::HashMap,
    path::{Path as FsPath, PathBuf},
    sync::Arc,
    time::Instant,
};

mod config;

use axum::{
    Json, Router,
    extract::{Multipart, Path, State},
    http::{HeaderMap, HeaderValue, StatusCode, header},
    response::{Html, IntoResponse, Response},
    routing::{get, post},
};
use bytes::Bytes;
use serde::Serialize;
use thiserror::Error;
use tokio::{fs, sync::Mutex, time::interval};
use tracing::{error, info, warn};
use uuid::Uuid;

use crate::config::{AppConfig, load_env_file};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();

    load_env_file();

    let config = AppConfig::from_env()?;
    fs::create_dir_all(&config.storage_dir).await?;

    let state = Arc::new(AppState::new(config.clone()));
    spawn_cleanup(state.clone());

    let app = Router::new()
        .route("/upload", post(upload))
        .route("/", get(upload_page))
        .route("/d/:id", get(download))
        .with_state(state);

    let listener = tokio::net::TcpListener::bind(config.address).await?;
    info!("listening on {}", config.address);
    axum::serve(listener, app).await?;

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

#[derive(Debug, Error)]
enum AppError {
    #[error("file not found")]
    NotFound,
    #[error("no file provided in multipart field 'file'")]
    NoFileProvided,
    #[error("invalid upload password")]
    Unauthorized,
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
            Self::Unauthorized => {
                (StatusCode::UNAUTHORIZED, "invalid upload password").into_response()
            }
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
    expires_in_minutes: u64,
    remaining_downloads: u32,
}

async fn upload(
    State(state): State<Arc<AppState>>,
    mut multipart: Multipart,
) -> Result<Json<UploadResponse>, AppError> {
    let mut provided_password: Option<String> = None;
    let mut file_data: Option<(String, Option<String>, Bytes)> = None;

    while let Some(field) = multipart.next_field().await? {
        match field.name() {
            Some("password") => {
                provided_password = field.text().await.ok();
            }
            Some("file") => {
                let filename = field
                    .file_name()
                    .map(|v| v.to_string())
                    .unwrap_or_else(|| "upload.bin".to_string());
                let content_type = field.content_type().map(|v| v.to_string());
                let data = field.bytes().await?;
                file_data = Some((filename, content_type, data));
            }
            _ => {}
        }
    }

    if state.config.upload_page_enabled
        && state.config.upload_password != provided_password.as_deref().unwrap_or("")
    {
        return Err(AppError::Unauthorized);
    }

    let Some((filename, content_type, data)) = file_data else {
        return Err(AppError::NoFileProvided);
    };

    let id = Uuid::new_v4().to_string();
    let suffix = if state.config.use_filename_suffix {
        FsPath::new(&filename)
            .extension()
            .and_then(|ext| ext.to_str())
            .filter(|ext| !ext.is_empty())
            .map(|ext| format!(".{}", ext))
    } else {
        None
    };

    let download_id = suffix
        .as_deref()
        .map(|ext| format!("{}{}", id, ext))
        .unwrap_or_else(|| id.clone());

    let path = state.config.storage_dir.join(&download_id);
    fs::write(&path, &data).await?;

    let expires_at = Instant::now() + state.config.ttl;
    let entry = FileEntry {
        path: path.clone(),
        filename,
        expires_at,
        remaining_hits: state.config.max_downloads,
        content_type,
    };

    state
        .entries
        .lock()
        .await
        .insert(download_id.clone(), entry);

    let response = UploadResponse {
        url: state.config.build_download_url(&download_id),
        expires_in_minutes: state.config.ttl.as_secs() / 60,
        remaining_downloads: state.config.max_downloads,
    };

    Ok(Json(response))
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

async fn delete_file(path: &FsPath) {
    if let Err(err) = fs::remove_file(path).await {
        if err.kind() != std::io::ErrorKind::NotFound {
            warn!(%err, "failed to remove file {:?}", path);
        }
    }
}

async fn upload_page(State(state): State<Arc<AppState>>) -> Response {
    if !state.config.upload_page_enabled {
        return StatusCode::NOT_FOUND.into_response();
    }

    let body = r#"<!doctype html>
<html lang=\"en\">
<head>
  <meta charset=\"utf-8\" />
  <title>newtemp.sh upload</title>
  <style>
    :root {
      color-scheme: light dark;
      --bg: linear-gradient(135deg, #0d1117 0%, #161b22 50%, #0a66c2 100%);
      --card: rgba(255, 255, 255, 0.08);
      --border: rgba(255, 255, 255, 0.18);
      --text: #f6f8fa;
      --muted: #c9d1d9;
      --accent: #58a6ff;
    }
    * { box-sizing: border-box; }
    body {
      margin: 0;
      min-height: 100vh;
      font-family: 'Inter', 'Segoe UI', system-ui, -apple-system, sans-serif;
      background: var(--bg);
      color: var(--text);
      display: flex;
      align-items: center;
      justify-content: center;
      padding: 2.5rem 1.5rem;
    }
    .shell {
      width: min(720px, 100%);
      background: var(--card);
      border: 1px solid var(--border);
      border-radius: 18px;
      box-shadow: 0 20px 60px rgba(0, 0, 0, 0.35);
      backdrop-filter: blur(18px);
      padding: 1.75rem 2rem;
    }
    header { display: flex; align-items: center; gap: 0.75rem; margin-bottom: 1rem; }
    header h1 { margin: 0; font-size: 1.6rem; letter-spacing: 0.01em; }
    header span { padding: 0.35rem 0.65rem; border-radius: 999px; background: rgba(88, 166, 255, 0.15); border: 1px solid var(--border); color: var(--accent); font-weight: 600; font-size: 0.85rem; }
    p { color: var(--muted); margin: 0.25rem 0 1rem; }
    form { margin-top: 1rem; display: grid; gap: 0.9rem; }
    label { font-weight: 600; letter-spacing: 0.01em; }
    input[type=\"password\"], input[type=\"file\"] {
      width: 100%;
      font-size: 1rem;
      padding: 0.65rem 0.75rem;
      border-radius: 12px;
      border: 1px solid var(--border);
      background: rgba(255, 255, 255, 0.06);
      color: var(--text);
    }
    input[type=\"file\"] { padding: 0.5rem 0.75rem; }
    .file-row { display: flex; gap: 0.6rem; align-items: center; }
    #file-name { flex: 1; padding: 0.65rem 0.75rem; border-radius: 12px; background: rgba(255, 255, 255, 0.06); border: 1px dashed var(--border); color: var(--muted); min-height: 44px; display: flex; align-items: center; }
    button {
      font-size: 1rem;
      font-weight: 700;
      padding: 0.75rem 1rem;
      border-radius: 12px;
      border: none;
      cursor: pointer;
      transition: transform 120ms ease, box-shadow 120ms ease, opacity 120ms ease;
    }
    #file-button { background: rgba(88, 166, 255, 0.2); color: var(--accent); border: 1px solid var(--border); }
    #submit { background: linear-gradient(120deg, #4096ff, #58a6ff); color: #0b1221; box-shadow: 0 14px 40px rgba(88, 166, 255, 0.35); }
    button:active { transform: translateY(1px); }
    #result { margin-top: 1.25rem; }
    pre { background: rgba(0, 0, 0, 0.35); padding: 0.85rem; border-radius: 12px; overflow: auto; border: 1px solid var(--border); }
  </style>
</head>
<body>
  <div class=\"shell\">
    <header>
      <h1>newtemp.sh uploader</h1>
      <span>Secure</span>
    </header>
    <p>Upload a file with the shared password to receive a download link instantly.</p>
    <form id=\"upload-form\">
      <div>
        <label for=\"password\">Upload password</label>
        <input id=\"password\" name=\"password\" type=\"password\" required placeholder=\"Enter the upload password\" />
      </div>
      <div>
        <label for=\"file\">Choose a file</label>
        <div class=\"file-row\">
          <input id=\"file\" name=\"file\" type=\"file\" required />
          <button type=\"button\" id=\"file-button\">Browse</button>
        </div>
        <div id=\"file-name\">No file chosen yet</div>
      </div>
      <button type=\"submit\" id=\"submit\">Upload &amp; get link</button>
    </form>
    <div id=\"result\"></div>
  </div>
  <script>
    const form = document.getElementById('upload-form');
    const result = document.getElementById('result');
    const fileInput = document.getElementById('file');
    const fileButton = document.getElementById('file-button');
    const fileName = document.getElementById('file-name');

    fileButton.addEventListener('click', () => fileInput.click());
    fileInput.addEventListener('change', () => {
      fileName.textContent = fileInput.files[0]?.name || 'No file chosen yet';
    });

    form.addEventListener('submit', async (e) => {
      e.preventDefault();
      const file = fileInput.files[0];
      const password = document.getElementById('password').value;
      if (!file) {
        fileName.textContent = 'Please choose a file first';
        return;
      }
      const data = new FormData();
      data.append('password', password);
      data.append('file', file);
      result.textContent = 'Uploading...';
      try {
        const response = await fetch('/upload', { method: 'POST', body: data });
        const text = await response.text();
        result.innerHTML = '<pre>' + text + '</pre>';
      } catch (err) {
        result.textContent = 'Upload failed: ' + err;
      }
    });
  </script>
</body>
</html>
"#;

    Html(body).into_response()
}
