//! `ludenea serve`: the project service over WebSocket, the editor's static files, the
//! WebAssembly builds of the game, and the plugin bundles, for the browser editor.

use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};

use axum::Router;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Path as UrlPath, State};
use axum::http::{HeaderMap, StatusCode, Uri, header};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use ludenea_project::protocol::{ProjectEvent, ProjectRequest, ProjectServerMessage, ServiceMode};
use ludenea_project::service::ProjectService;
use tokio::sync::broadcast::error::RecvError;

#[cfg(feature = "embed-editor")]
static EMBEDDED_EDITOR: include_dir::Dir<'_> =
    include_dir::include_dir!("$CARGO_MANIFEST_DIR/../../editor/apps/web/dist");

pub struct Options {
    pub project: PathBuf,
    pub host: String,
    pub port: u16,
    pub editor_dir: Option<PathBuf>,
    pub engine_path: Option<PathBuf>,
}

#[derive(Clone)]
struct App {
    // ponytail: one lock for the whole service, so a slow request (bundling plugins) delays
    // the others; finer locking inside the service if that ever shows.
    service: Arc<Mutex<ProjectService>>,
    editor_dir: Option<Arc<PathBuf>>,
}

/// Opens the project, then serves until the process ends. Prints `listening on http://<address>`
/// once it accepts connections.
pub async fn run(options: Options) -> Result<(), String> {
    let mut service = ProjectService::new(ServiceMode::Browser, options.engine_path);
    let path = options.project.to_string_lossy().into_owned();
    service
        .handle(ProjectRequest::Open { path })
        .map_err(|failure| failure.message)?;
    let app = App {
        service: Arc::new(Mutex::new(service)),
        editor_dir: options.editor_dir.map(Arc::new),
    };
    let router = Router::new()
        .route("/ws/project", get(project_socket))
        .route("/plugins/{file}", get(plugin))
        .route("/game/{hash}/{file}", get(game_file))
        .fallback(editor_file)
        .with_state(app);
    let listener = tokio::net::TcpListener::bind((options.host.as_str(), options.port))
        .await
        .map_err(|error| {
            format!(
                "cannot listen on {}:{}: {error}",
                options.host, options.port
            )
        })?;
    let address: SocketAddr = listener.local_addr().map_err(|error| error.to_string())?;
    println!("listening on http://{address}");
    axum::serve(listener, router)
        .await
        .map_err(|error| error.to_string())
}

async fn project_socket(
    State(app): State<App>,
    headers: HeaderMap,
    upgrade: WebSocketUpgrade,
) -> Response {
    // Browsers let any page open a WebSocket to localhost: only the editor's own pages may.
    if !same_origin(&headers) {
        return StatusCode::FORBIDDEN.into_response();
    }
    upgrade.on_upgrade(move |socket| session(app, socket))
}

/// A connection without `Origin` is not from a browser; one with it must match `Host`.
fn same_origin(headers: &HeaderMap) -> bool {
    let Some(origin) = headers.get(header::ORIGIN) else {
        return true;
    };
    let origin_host = origin
        .to_str()
        .ok()
        .and_then(|origin| origin.split_once("://"))
        .map(|(_, host)| host);
    let host = headers
        .get(header::HOST)
        .and_then(|host| host.to_str().ok());
    origin_host.is_some() && origin_host == host
}

/// One editor: requests answered in order, and the project's events once it said hello.
async fn session(app: App, mut socket: WebSocket) {
    let mut events = lock(&app.service).subscribe();
    let mut connected = false;
    loop {
        let reply = tokio::select! {
            frame = socket.recv() => {
                let text = match frame {
                    Some(Ok(Message::Text(text))) => text,
                    Some(Ok(Message::Close(_)) | Err(_)) | None => return,
                    Some(Ok(_)) => continue,
                };
                let service = Arc::clone(&app.service);
                let answered = tokio::task::spawn_blocking(move || {
                    let mut now_connected = connected;
                    let reply = lock(&service).answer(&mut now_connected, text.as_str());
                    (reply, now_connected)
                })
                .await;
                let Ok((reply, now_connected)) = answered else { return };
                connected = now_connected;
                reply
            }
            event = events.recv(), if connected => match event {
                Ok(event) => ProjectServerMessage::Event { event },
                Err(RecvError::Lagged(missed)) => ProjectServerMessage::Event {
                    event: ProjectEvent::Warning {
                        message: format!(
                            "the editor missed {missed} project events; reload it to catch up"
                        ),
                    },
                },
                Err(RecvError::Closed) => return,
            },
        };
        let text = serde_json::to_string(&reply).expect("protocol messages serialize");
        if socket.send(Message::Text(text.into())).await.is_err() {
            return;
        }
    }
}

async fn plugin(State(app): State<App>, UrlPath(file): UrlPath<String>) -> Response {
    if !plain_name(&file) {
        return StatusCode::BAD_REQUEST.into_response();
    }
    serve_from_project(&app, &Path::new(".ludenea/plugins").join(file)).await
}

async fn game_file(
    State(app): State<App>,
    UrlPath((hash, file)): UrlPath<(String, String)>,
) -> Response {
    if !plain_name(&hash) || !plain_name(&file) {
        return StatusCode::BAD_REQUEST.into_response();
    }
    let path = Path::new(".ludenea/build/wasm").join(hash).join(file);
    serve_from_project(&app, &path).await
}

async fn serve_from_project(app: &App, relative: &Path) -> Response {
    let Some(root) = lock(&app.service).root() else {
        return StatusCode::NOT_FOUND.into_response();
    };
    serve_file(&root.join(relative)).await
}

async fn editor_file(State(app): State<App>, uri: Uri) -> Response {
    let segments: Vec<&str> = uri.path().split('/').filter(|s| !s.is_empty()).collect();
    if !segments.iter().all(|segment| plain_name(segment)) {
        return StatusCode::BAD_REQUEST.into_response();
    }
    let relative = if segments.is_empty() {
        "index.html".to_string()
    } else {
        segments.join("/")
    };
    if let Some(directory) = &app.editor_dir {
        return serve_file(&directory.join(&relative)).await;
    }
    #[cfg(feature = "embed-editor")]
    if let Some(file) = EMBEDDED_EDITOR.get_file(&relative) {
        let kind = content_type(Path::new(&relative));
        return ([(header::CONTENT_TYPE, kind)], file.contents()).into_response();
    }
    (
        StatusCode::NOT_FOUND,
        "no editor here: pass --editor-dir <editor/apps/web/dist>",
    )
        .into_response()
}

async fn serve_file(path: &Path) -> Response {
    match tokio::fs::read(path).await {
        Ok(bytes) => ([(header::CONTENT_TYPE, content_type(path))], bytes).into_response(),
        Err(_) => StatusCode::NOT_FOUND.into_response(),
    }
}

/// A single file or directory name: nothing that could lead out of the directory it is
/// looked up in.
fn plain_name(name: &str) -> bool {
    !name.is_empty() && name != "." && name != ".." && !name.contains(['/', '\\', ':', '\0'])
}

fn content_type(path: &Path) -> &'static str {
    match path.extension().and_then(|extension| extension.to_str()) {
        Some("html") => "text/html; charset=utf-8",
        Some("js" | "mjs") => "text/javascript",
        Some("wasm") => "application/wasm",
        Some("css") => "text/css",
        Some("json" | "map") => "application/json",
        Some("svg") => "image/svg+xml",
        Some("png") => "image/png",
        Some("ico") => "image/x-icon",
        Some("woff2") => "font/woff2",
        _ => "application/octet-stream",
    }
}

fn lock(service: &Mutex<ProjectService>) -> MutexGuard<'_, ProjectService> {
    service
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}
