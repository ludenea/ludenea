//! The signaling listener of online matches: the WebRTC peers of a room find each other
//! through it, speaking matchbox's JSON messages as `matchbox_server` does in a full mesh,
//! and it serves a web build read-only beside its rooms.
//!
//! `GET /rooms/{code}?host=N` creates room `code` for N peers (2 to
//! [`ludenea_net::MAX_PEERS`]) and `GET /rooms/{code}` joins it; both upgrade to a WebSocket.
//! Everything a client sends is untrusted: every request, room, message and queue is bounded
//! by [`Limits`], and a client that breaks a rule is refused or closed, never served at the
//! expense of the others.

pub mod files;
mod limits;
mod rooms;
mod stall;

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use axum::Router;
use axum::extract::rejection::QueryRejection;
use axum::extract::ws::rejection::WebSocketUpgradeRejection;
use axum::extract::ws::{CloseFrame, Message, WebSocket, WebSocketUpgrade, close_code};
use axum::extract::{ConnectInfo, Path as UrlPath, Query, State};
use axum::http::{HeaderMap, Method, StatusCode, Uri, header};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use hyper_util::rt::{TokioExecutor, TokioIo, TokioTimer};
use hyper_util::server::conn::auto::Builder;
use hyper_util::service::TowerToHyperService;
use ludenea_net::{MAX_PEERS, RoomCode};
use matchbox_protocol::JsonPeerRequest;
use serde::Deserialize;
use tokio::net::TcpListener;
use tokio::sync::{OwnedSemaphorePermit, Semaphore};
use tungstenite::error::CapacityError;

pub use limits::Limits;
use limits::{ConnectionSlot, Connections};
use rooms::{Membership, Reservation, Rooms, Shared};

/// How long a WebSocket stays open after its close frame, before the connection closes. The
/// client may still be sending (the rest of a refused message, for example): closing at once
/// resets the connection, and the client, Chromium among them, can then lose the close frame
/// and what came before it.
// ponytail: a fixed wait after a refused message, since axum's WebSocket cannot read past it;
// reading and discarding the rest until the client closes would need the raw connection.
pub const LINGER: std::time::Duration = std::time::Duration::from_secs(1);

/// What the listener serves and how it bounds its clients.
#[derive(Debug, Clone, Default)]
pub struct Options {
    /// The folder of a web build (`ludenea build --web`) served read-only on every path but
    /// the rooms; without it those paths answer 404.
    pub page: Option<PathBuf>,
    pub limits: Limits,
    /// Origins (`scheme://host:port`) whose pages may open rooms besides the one matching
    /// `Host`.
    pub allowed_origins: Vec<String>,
}

#[derive(Clone)]
struct App {
    rooms: Shared,
    limits: Limits,
    allowed_origins: Arc<Vec<String>>,
}

/// The listener's routes: the rooms under `/rooms/{code}`, the page on every other path.
///
/// The rooms read the client's address from [`ConnectInfo<SocketAddr>`], for the limits per
/// address: serve the router with it, as [`serve`] does, or every room request answers 500.
/// Served on its own, it has none of the connection limits [`serve`] adds.
pub fn router(options: Options) -> Router {
    let app = App {
        rooms: Arc::new(Mutex::new(Rooms::new(options.limits.clone()))),
        limits: options.limits,
        allowed_origins: Arc::new(allowed_origins(options.allowed_origins)),
    };
    let page = Arc::new(options.page);
    Router::new()
        .route("/rooms/{code}", get(room))
        .with_state(app)
        .fallback(move |method: Method, uri: Uri| page_file(Arc::clone(&page), method, uri))
}

/// Serves [`router`] on every listener until the process ends: at most
/// [`Limits::max_connections`] connections at once, each given [`Limits::header_timeout`] to
/// send the headers of each request, and ended by a write that stays blocked for
/// [`Limits::idle`].
///
/// # Errors
///
/// When `listeners` is empty, or a listener's task fails.
pub async fn serve(listeners: Vec<TcpListener>, options: Options) -> Result<(), String> {
    if listeners.is_empty() {
        return Err("the signaling listener has no address to listen on".into());
    }
    let limits = options.limits.clone();
    let router = router(options);
    let connections = Arc::new(Semaphore::new(
        limits.max_connections.clamp(1, Semaphore::MAX_PERMITS),
    ));
    let per_address = Arc::new(Connections::default());
    let mut tasks = tokio::task::JoinSet::new();
    for listener in listeners {
        tasks.spawn(accept(
            listener,
            router.clone(),
            Arc::clone(&connections),
            Arc::clone(&per_address),
            limits.clone(),
        ));
    }
    while let Some(result) = tasks.join_next().await {
        result.map_err(|error| format!("a signaling listener failed: {error}"))?;
    }
    Ok(())
}

/// Accepts connections on `listener` while `connections` has a permit for each, so clients
/// beyond [`Limits::max_connections`] wait in the system's queue instead of in memory. A
/// connection beyond [`Limits::connections_per_address`] of its address is closed at once.
async fn accept(
    listener: TcpListener,
    router: Router,
    connections: Arc<Semaphore>,
    per_address: Arc<Connections>,
    limits: Limits,
) {
    loop {
        let Ok(permit) = Arc::clone(&connections).acquire_owned().await else {
            // Only a closed semaphore refuses, and nothing closes it.
            return;
        };
        let (stream, remote) = match listener.accept().await {
            Ok(accepted) => accepted,
            Err(error) => {
                // Out of file descriptors, or a connection reset before it was accepted:
                // the listener itself is fine, so wait a moment and go on, as axum does.
                eprintln!("warning: the signaling listener could not accept a connection: {error}");
                tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                continue;
            }
        };
        let Some(slot) =
            Connections::take(&per_address, remote.ip(), limits.connections_per_address)
        else {
            continue;
        };
        // Shared with a WebSocket the connection upgrades to, which outlives hyper's own
        // connection future: the permit and the slot go back once both are done.
        let permit = ConnectionPermit(Arc::new((permit, slot)));
        let service = TowerToHyperService::new(
            router
                .clone()
                .layer(axum::Extension(ConnectInfo(remote)))
                .layer(axum::Extension(permit.clone())),
        );
        let header_timeout = limits.header_timeout;
        let io = TokioIo::new(stall::WriteDeadline::new(stream, limits.idle));
        tokio::spawn(async move {
            let mut builder = Builder::new(TokioExecutor::new());
            builder
                .http1()
                .timer(TokioTimer::new())
                .header_read_timeout(header_timeout);
            // A failed connection (a reset, slow headers, a stalled reader) concerns that
            // client only.
            let _ = builder.serve_connection_with_upgrades(io, service).await;
        });
    }
}

/// The place of one connection among [`Limits::max_connections`] and among the
/// [`Limits::connections_per_address`] of its address.
#[derive(Clone)]
struct ConnectionPermit(
    #[expect(dead_code, reason = "held, never read")] Arc<(OwnedSemaphorePermit, ConnectionSlot)>,
);

#[derive(Deserialize)]
struct RoomQuery {
    host: Option<String>,
}

async fn room(
    State(app): State<App>,
    ConnectInfo(remote): ConnectInfo<SocketAddr>,
    UrlPath(code): UrlPath<String>,
    query: Result<Query<RoomQuery>, QueryRejection>,
    headers: HeaderMap,
    upgrade: Result<WebSocketUpgrade, WebSocketUpgradeRejection>,
    permit: Option<axum::Extension<ConnectionPermit>>,
) -> Response {
    if let Err(origin) = check_origin(&headers, &app.allowed_origins) {
        let message =
            format!("origin {origin} may not open rooms here: pass --allow-origin {origin}");
        return (StatusCode::FORBIDDEN, message).into_response();
    }
    let code = match RoomCode::parse(&code) {
        Ok(code) => code,
        Err(error) => return (StatusCode::BAD_REQUEST, error.to_string()).into_response(),
    };
    let Query(query) = match query {
        Ok(query) => query,
        Err(rejection) => return rejection.into_response(),
    };
    let size = match query.host.as_deref().map(room_size).transpose() {
        Ok(size) => size,
        Err(message) => return (StatusCode::BAD_REQUEST, message).into_response(),
    };
    let upgrade = match upgrade {
        Ok(upgrade) => upgrade,
        Err(rejection) => return rejection.into_response(),
    };
    #[expect(
        clippy::disallowed_methods,
        reason = "network pacing, never feeds the simulation"
    )]
    let now = Instant::now();
    let reservation = match Reservation::new(&app.rooms, code, size, remote.ip(), now) {
        Ok(reservation) => reservation,
        Err(refusal) => return (refusal.status(), refusal.to_string()).into_response(),
    };
    let limits = app.limits;
    upgrade
        .max_message_size(limits.max_message_bytes)
        .max_frame_size(limits.max_message_bytes)
        .on_upgrade(move |mut socket| async move {
            // The reservation moved here: dropped unused if the upgrade fails. The member
            // holds its connection's permit until it ends.
            let _permit = permit;
            match reservation.arrive() {
                Some(membership) => member(socket, membership, remote, &limits).await,
                None => {
                    let ending = Ending::new(close_code::ERROR, "the room is gone");
                    close(&mut socket, &ending, &limits).await;
                }
            }
        })
}

/// The size `?host=` gives a room.
fn room_size(value: &str) -> Result<usize, String> {
    value
        .parse::<usize>()
        .ok()
        .filter(|size| (2..=MAX_PEERS).contains(size))
        .ok_or_else(|| {
            format!("?host takes the room's size, 2 to {MAX_PEERS} peers, not {value:?}")
        })
}

/// Origins given on the command line (`--allow-origin`) in the form browsers send them:
/// lowercase, without a trailing slash.
pub fn allowed_origins(origins: Vec<String>) -> Vec<String> {
    origins
        .into_iter()
        .map(|origin| origin.trim_end_matches('/').to_ascii_lowercase())
        .collect()
}

/// Whether a browser page may open a WebSocket: a request without `Origin` is not from a
/// browser and passes; one with it must match `Host` or be one of `allowed` (as
/// [`allowed_origins`] writes them). The error is the refused origin.
///
/// # Errors
///
/// The `Origin` of a request from any other page.
pub fn check_origin(headers: &HeaderMap, allowed: &[String]) -> Result<(), String> {
    let Some(origin) = headers.get(header::ORIGIN) else {
        return Ok(());
    };
    let text = String::from_utf8_lossy(origin.as_bytes()).into_owned();
    if allowed.contains(&text.to_ascii_lowercase()) {
        return Ok(());
    }
    let origin_host = text.split_once("://").map(|(_, host)| host);
    let host = headers
        .get(header::HOST)
        .and_then(|host| host.to_str().ok());
    if origin_host.is_some() && origin_host == host {
        Ok(())
    } else {
        Err(text)
    }
}

/// One member's connection: its requests relayed, its queued events sent, until either
/// side ends it or it breaks a rule.
async fn member(
    mut socket: WebSocket,
    mut membership: Membership,
    remote: SocketAddr,
    limits: &Limits,
) {
    let idle = tokio::time::sleep(limits.idle);
    tokio::pin!(idle);
    let ending = loop {
        let step = tokio::select! {
            frame = socket.recv() => {
                idle.set(tokio::time::sleep(limits.idle));
                received(&membership, limits, frame)
            }
            event = membership.events.recv() => forward(&mut socket, limits, event).await,
            () = &mut idle => {
                let reason = format!("no frame for {} seconds", limits.idle.as_secs());
                Step::Close(Ending::new(close_code::AWAY, reason))
            }
        };
        match step {
            Step::Go => {}
            Step::Stop => return,
            Step::Close(ending) => break ending,
        }
    };
    // The member leaves its room before the close lingers.
    drop(membership);
    if ending.code != close_code::AWAY {
        eprintln!(
            "warning: closed the signaling connection of {remote} ({}): {}",
            ending.code, ending.reason
        );
    }
    close(&mut socket, &ending, limits).await;
}

/// Sends the close frame of `ending`, then gives it up to [`LINGER`] to arrive.
async fn close(socket: &mut WebSocket, ending: &Ending, limits: &Limits) {
    if !send_close(socket, ending.code, &ending.reason, limits).await {
        return;
    }
    if ending.code == close_code::SIZE {
        // The rest of the refused message cannot be read past: wait without reading.
        tokio::time::sleep(LINGER).await;
        return;
    }
    // Wait for the client's answer, reading past whatever it still sends, so the connection
    // does not reset before the close frame arrives. No answer in time: it ends anyway.
    let answer = async {
        while let Some(Ok(message)) = socket.recv().await {
            if matches!(message, Message::Close(_)) {
                break;
            }
        }
    };
    let _ = tokio::time::timeout(LINGER, answer).await;
}

/// What the connection of a member does next.
enum Step {
    Go,
    /// The client is gone, or cannot be written to: nothing to say.
    Stop,
    Close(Ending),
}

/// How a member's connection closes.
struct Ending {
    code: u16,
    reason: String,
}

impl Ending {
    fn new(code: u16, reason: impl Into<String>) -> Ending {
        Ending {
            code,
            reason: reason.into(),
        }
    }
}

/// Handles a frame from the member.
fn received(
    membership: &Membership,
    limits: &Limits,
    frame: Option<Result<Message, axum::Error>>,
) -> Step {
    let text = match frame {
        Some(Ok(Message::Text(text))) => text,
        Some(Ok(Message::Ping(_) | Message::Pong(_))) => return Step::Go,
        Some(Ok(Message::Binary(_))) => {
            return Step::Close(Ending::new(
                close_code::POLICY,
                "signaling messages are JSON text",
            ));
        }
        Some(Ok(Message::Close(_))) | None => return Step::Stop,
        Some(Err(error)) => {
            if oversized(error).is_none() {
                return Step::Stop;
            }
            let limit = limits.max_message_bytes;
            let reason = format!("a message over the limit of {limit} bytes");
            return Step::Close(Ending::new(close_code::SIZE, reason));
        }
    };
    match text.as_str().parse::<JsonPeerRequest>() {
        Ok(JsonPeerRequest::Signal { receiver, data }) => {
            membership.relay(receiver, data);
            Step::Go
        }
        Ok(JsonPeerRequest::KeepAlive) => Step::Go,
        Err(_) => Step::Close(Ending::new(close_code::POLICY, "not a signaling request")),
    }
}

/// Sends an event queued for the member. The queue ends when the member fell behind; a
/// send that fails or stays blocked for [`Limits::idle`] stops the connection.
async fn forward(socket: &mut WebSocket, limits: &Limits, event: Option<String>) -> Step {
    match event {
        Some(text) => {
            let send = socket.send(Message::Text(text.into()));
            match tokio::time::timeout(limits.idle, send).await {
                Ok(Ok(())) => Step::Go,
                _ => Step::Stop,
            }
        }
        None => {
            let reason = format!("more than {} messages were waiting", limits.queue);
            Step::Close(Ending::new(close_code::AGAIN, reason))
        }
    }
}

/// Sends a close frame; `false` when it cannot be sent within [`Limits::idle`], which means
/// the client is gone or not reading.
async fn send_close(socket: &mut WebSocket, code: u16, reason: &str, limits: &Limits) -> bool {
    let frame = CloseFrame {
        code,
        reason: reason.into(),
    };
    let send = socket.send(Message::Close(Some(frame)));
    matches!(tokio::time::timeout(limits.idle, send).await, Ok(Ok(())))
}

/// The size and the limit of a message over a WebSocket's size limit, if `error` refuses
/// one: the size received so far when it came in several frames.
pub fn oversized(error: axum::Error) -> Option<(usize, usize)> {
    match *error.into_inner().downcast::<tungstenite::Error>().ok()? {
        tungstenite::Error::Capacity(CapacityError::MessageTooLong { size, max_size }) => {
            Some((size, max_size))
        }
        _ => None,
    }
}

/// A file of the page folder: GET and HEAD only.
async fn page_file(page: Arc<Option<PathBuf>>, method: Method, uri: Uri) -> Response {
    let Some(relative) = files::relative_path(uri.path()) else {
        return StatusCode::BAD_REQUEST.into_response();
    };
    if method != Method::GET && method != Method::HEAD {
        return StatusCode::METHOD_NOT_ALLOWED.into_response();
    }
    match page.as_ref() {
        Some(folder) => files::serve_file(&folder.join(relative)).await,
        None => (
            StatusCode::NOT_FOUND,
            "no page here: pass the folder `ludenea build --web` wrote",
        )
            .into_response(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn headers(host: &str, origin: Option<&str>) -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert(header::HOST, host.parse().unwrap());
        if let Some(origin) = origin {
            headers.insert(header::ORIGIN, origin.parse().unwrap());
        }
        headers
    }

    #[test]
    fn an_allowed_origin_may_differ_from_the_host() {
        let proxied = headers("localhost:7878", Some("http://localhost:5173"));
        assert_eq!(
            check_origin(&proxied, &[]),
            Err("http://localhost:5173".to_string())
        );
        let allowed = allowed_origins(vec!["HTTP://localhost:5173/".to_string()]);
        assert_eq!(check_origin(&proxied, &allowed), Ok(()));
        let other = headers("localhost:7878", Some("http://localhost:5174"));
        assert!(check_origin(&other, &allowed).is_err());
        let own = headers("localhost:7878", Some("http://localhost:7878"));
        assert_eq!(check_origin(&own, &[]), Ok(()));
        assert_eq!(check_origin(&headers("localhost:7878", None), &[]), Ok(()));
    }
}
