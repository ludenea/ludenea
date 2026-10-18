//! The desktop editor: the project service in process, the game as a native process over
//! stdio, and the editor front end in a webview. The front end talks to both through two
//! channels, `project` and `control`, with the `send`, `subscribe`, and `unsubscribe` commands.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, Sender};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread;

use ludenea_project::protocol::{ProjectEvent, ProjectRequest, ProjectServerMessage, ServiceMode};
use ludenea_project::service::ProjectService;
use tauri::http::{Response, StatusCode, header};
use tauri::ipc::Channel;
use tauri::{Manager, State};
use tokio::sync::broadcast;
use tokio::sync::broadcast::error::{RecvError, TryRecvError};

/// The engine repository new projects depend on when the front end names none (its setting
/// `ludenea.engine-path` travels in `Create`).
const ENGINE_PATH_VARIABLE: &str = "LUDENEA_ENGINE_PATH";

struct Backend {
    service: Mutex<ProjectService>,
    delivery: Mutex<Delivery>,
    project: Mutex<Option<Channel<String>>>,
    control: Mutex<Option<Channel<String>>>,
    /// The running game's input, and which game it is: a replaced game's last lines are dropped.
    to_game: Mutex<Option<Sender<String>>>,
    game: AtomicU64,
}

/// What reaches the front end on the project channel. Locked after the service, never before.
#[derive(Default)]
struct Delivery {
    /// Whether the front end said hello: events wait for it.
    connected: bool,
    /// The subscription that follows the replay sent at the last hello, for the event relay
    /// to switch to: the events it had already received are in the replay or predate the hello.
    caught_up: Option<broadcast::Receiver<ProjectEvent>>,
}

impl Backend {
    /// Answers one project message and sends the reply on the project channel. After a
    /// hello, also what the editor missed (a fast build, a reloaded window), each event once.
    fn answer(&self, text: &str) {
        let mut service = lock(&self.service);
        let was_connected = lock(&self.delivery).connected;
        let mut connected = was_connected;
        let reply = service.answer(&mut connected, text);
        let mut delivery = lock(&self.delivery);
        deliver(&self.project, encode(&reply));
        if matches!(reply, ProjectServerMessage::Welcome { .. }) {
            let (missed, events) = service.subscribe_caught_up();
            if !missed.is_empty() {
                deliver(&self.project, events_batch(missed));
            }
            delivery.caught_up = Some(events);
        }
        // Only a hello changes it; a window that reloaded meanwhile stays disconnected.
        if connected != was_connected {
            delivery.connected = connected;
        }
    }

    /// A front end (re)subscribed to the project channel: the previous one is gone, so until
    /// this one says hello, events wait and rebuilds swap the game at once.
    fn replace_editor(&self, channel: Channel<String>) {
        let service = lock(&self.service);
        service.editor_disconnected();
        lock(&self.delivery).connected = false;
        *lock(&self.project) = Some(channel);
    }

    /// The front end closed its project channel `id`: like a replaced editor, until another
    /// subscribes and says hello, events wait. A newer channel stays.
    fn drop_editor(&self, id: u32) {
        let service = lock(&self.service);
        let mut delivery = lock(&self.delivery);
        let mut project = lock(&self.project);
        if project.as_ref().is_some_and(|channel| channel.id() == id) {
            *project = None;
            delivery.connected = false;
            service.editor_disconnected();
        }
    }

    /// Takes the control channel of the game just started and relays what it prints.
    fn attach_game(self: &Arc<Self>) {
        let Some((to_game, from_game)) = lock(&self.service).take_control_channel() else {
            return;
        };
        *lock(&self.to_game) = Some(to_game);
        let game = self.game.fetch_add(1, Ordering::SeqCst) + 1;
        let backend = Arc::clone(self);
        thread::spawn(move || backend.relay_game(game, &from_game));
    }

    /// One channel message per batch the game printed together (one `Host::advance`).
    fn relay_game(&self, game: u64, from_game: &Receiver<String>) {
        while let Ok(first) = from_game.recv() {
            let mut lines = vec![first];
            lines.extend(from_game.try_iter());
            if self.game.load(Ordering::SeqCst) != game {
                return;
            }
            deliver(&self.control, lines.join("\n"));
        }
    }

    /// Forwards the project's events in batches, attaching each new game before announcing it.
    fn relay_events(self: &Arc<Self>, mut events: broadcast::Receiver<ProjectEvent>) {
        loop {
            let mut batch = match events.blocking_recv() {
                Ok(event) => vec![event],
                Err(RecvError::Lagged(missed)) => vec![missed_events(missed)],
                Err(RecvError::Closed) => return,
            };
            loop {
                match events.try_recv() {
                    Ok(event) => batch.push(event),
                    Err(TryRecvError::Lagged(missed)) => batch.push(missed_events(missed)),
                    Err(TryRecvError::Empty | TryRecvError::Closed) => break,
                }
            }
            if batch
                .iter()
                .any(|event| matches!(event, ProjectEvent::GameReady))
            {
                // Taking the control channel twice (a repeated `GameReady`) finds nothing.
                self.attach_game();
            }
            let mut delivery = lock(&self.delivery);
            if let Some(caught_up) = delivery.caught_up.take() {
                events = caught_up;
                continue;
            }
            if delivery.connected {
                deliver(&self.project, events_batch(batch));
            }
        }
    }
}

#[tauri::command]
async fn subscribe(
    channel: String,
    on_message: Channel<String>,
    backend: State<'_, Arc<Backend>>,
) -> Result<(), String> {
    match channel.as_str() {
        // Waits for a long request to finish: off the main thread.
        "project" => {
            let backend = Arc::clone(&backend);
            tauri::async_runtime::spawn_blocking(move || backend.replace_editor(on_message))
                .await
                .map_err(|error| error.to_string())
        }
        "control" => {
            *lock(&backend.control) = Some(on_message);
            Ok(())
        }
        _ => Err(format!("no channel named {channel:?}")),
    }
}

/// Detaches channel `id` from `channel`, unless a newer subscription replaced it.
#[tauri::command]
async fn unsubscribe(
    channel: String,
    id: u32,
    backend: State<'_, Arc<Backend>>,
) -> Result<(), String> {
    match channel.as_str() {
        "project" => {
            let backend = Arc::clone(&backend);
            tauri::async_runtime::spawn_blocking(move || backend.drop_editor(id))
                .await
                .map_err(|error| error.to_string())
        }
        "control" => {
            let mut control = lock(&backend.control);
            if control.as_ref().is_some_and(|channel| channel.id() == id) {
                *control = None;
            }
            Ok(())
        }
        _ => Err(format!("no channel named {channel:?}")),
    }
}

#[tauri::command]
async fn send(
    channel: String,
    message: String,
    backend: State<'_, Arc<Backend>>,
) -> Result<(), String> {
    match channel.as_str() {
        // Requests can take long (bundling plugins): off the main thread.
        "project" => {
            let backend = Arc::clone(&backend);
            tauri::async_runtime::spawn_blocking(move || backend.answer(&message))
                .await
                .map_err(|error| error.to_string())
        }
        "control" => lock(&backend.to_game)
            .as_ref()
            .ok_or("no game is running")?
            .send(message)
            .map_err(|_| "the game has exited".to_string()),
        _ => Err(format!("no channel named {channel:?}")),
    }
}

/// `ludenea-plugin://localhost/plugins/<file>`: the open project's plugin bundles.
fn plugin_bundle(backend: &Backend, path: &str) -> Response<Vec<u8>> {
    let respond = |status: StatusCode, body: Vec<u8>| {
        Response::builder()
            .status(status)
            .header(header::CONTENT_TYPE, "text/javascript")
            .header(header::ACCESS_CONTROL_ALLOW_ORIGIN, "*")
            .body(body)
            .expect("a valid response")
    };
    let Some(file) = path
        .strip_prefix("/plugins/")
        .filter(|file| plain_name(file))
    else {
        return respond(StatusCode::BAD_REQUEST, Vec::new());
    };
    let Some(root) = lock(&backend.service).root() else {
        return respond(StatusCode::NOT_FOUND, Vec::new());
    };
    match std::fs::read(root.join(".ludenea/plugins").join(file)) {
        Ok(code) => respond(StatusCode::OK, code),
        Err(_) => respond(StatusCode::NOT_FOUND, Vec::new()),
    }
}

/// A single file name: nothing that could lead out of the plugins directory.
fn plain_name(name: &str) -> bool {
    !name.is_empty() && name != "." && name != ".." && !name.contains(['/', '\\', ':', '\0'])
}

fn missed_events(missed: u64) -> ProjectEvent {
    ProjectEvent::Warning {
        message: format!("the editor missed {missed} project events; reload it to catch up"),
    }
}

fn deliver(slot: &Mutex<Option<Channel<String>>>, batch: String) {
    if let Some(channel) = lock(slot).as_ref() {
        let _ = channel.send(batch);
    }
}

fn events_batch(events: Vec<ProjectEvent>) -> String {
    let lines: Vec<String> = events
        .into_iter()
        .map(|event| encode(&ProjectServerMessage::Event { event }))
        .collect();
    lines.join("\n")
}

fn encode(message: &ProjectServerMessage) -> String {
    serde_json::to_string(message).expect("protocol messages serialize")
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

// `generate_context!` expands to a `HashMap`; nothing here is part of the simulation.
#[allow(clippy::disallowed_types)]
fn main() {
    let engine_path = std::env::var_os(ENGINE_PATH_VARIABLE).map(PathBuf::from);
    let mut service = ProjectService::new(ServiceMode::Desktop, engine_path);
    let events = service.subscribe();
    // `ludenea-desktop <project>` opens the project at once.
    if let Some(path) = std::env::args().nth(1) {
        let request = ProjectRequest::Open { path: path.clone() };
        if let Err(failure) = service.handle(request) {
            eprintln!("cannot open {path}: {}", failure.message);
        }
    }
    let backend = Arc::new(Backend {
        service: Mutex::new(service),
        delivery: Mutex::default(),
        project: Mutex::new(None),
        control: Mutex::new(None),
        to_game: Mutex::new(None),
        game: AtomicU64::new(0),
    });
    let relay = Arc::clone(&backend);
    thread::spawn(move || relay.relay_events(events));

    tauri::Builder::default()
        .manage(backend)
        .invoke_handler(tauri::generate_handler![subscribe, unsubscribe, send])
        .register_asynchronous_uri_scheme_protocol(
            "ludenea-plugin",
            |context, request, responder| {
                let backend = Arc::clone(context.app_handle().state::<Arc<Backend>>().inner());
                let path = request.uri().path().to_string();
                tauri::async_runtime::spawn_blocking(move || {
                    responder.respond(plugin_bundle(&backend, &path));
                });
            },
        )
        .run(tauri::generate_context!())
        .expect("the editor window failed");
}
