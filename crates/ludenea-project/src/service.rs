//! The project service: answers the project protocol for one open project, builds the game in
//! the background, and, on desktop, runs it. Shared by `ludenea serve` and the desktop backend.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread;
use std::time::Duration;

use ludenea_control::Bytes;
use ludenea_control::protocol::unreadable_hello;
use tokio::sync::broadcast;

use crate::protocol::{
    AssetTexts, FailureCode, PROJECT_PROTOCOL_VERSION, ProjectClientMessage, ProjectEvent,
    ProjectFailure, ProjectInfo, ProjectReply, ProjectRequest, ProjectServerMessage, ServiceMode,
    handshake, version_mismatch,
};
use crate::{
    BuildArtifact, BuildError, BuildEvent, BuildTarget, GameProcess, PluginEvent, Project,
    ProjectError, ProjectWarning, WATCH_POLL, Watch,
};

/// Events a slow client may fall behind by before it misses some.
const EVENT_CAPACITY: usize = 1024;
/// How long an exited game gets to report its exit code.
const EXIT_POLLS: u32 = 100;
const EXIT_POLL_INTERVAL: Duration = Duration::from_millis(10);

/// A game process shared with the threads relaying it. Emptied when the game is replaced, so
/// those threads stop and report nothing.
type GameSlot = Arc<Mutex<Option<GameProcess>>>;
type ControlChannel = (Sender<String>, Receiver<String>);

pub struct ProjectService {
    mode: ServiceMode,
    engine_path: Option<PathBuf>,
    events: broadcast::Sender<ProjectEvent>,
    opened: Option<Opened>,
    /// Set while an editor is connected (it said hello): a desktop rebuild then waits for
    /// `RestartGame`.
    editor_connected: Arc<AtomicBool>,
}

/// The open project and what its background threads share.
struct Opened {
    project: Arc<Mutex<Project>>,
    game: Arc<Mutex<Game>>,
    /// Builds run one at a time, so the last to finish is the newest.
    builds: Arc<Mutex<()>>,
    /// Cleared when the project is closed; background threads then stop.
    alive: Arc<AtomicBool>,
    /// The events of the current or last build, for editors connecting later.
    last_build: BuildLog,
}

/// The events of one build, with only its latest progress.
type BuildLog = Arc<Mutex<Vec<ProjectEvent>>>;

#[derive(Default)]
struct Game {
    /// Desktop: the running game.
    process: Option<GameSlot>,
    /// Desktop: the relay to the running game, until the backend takes it.
    control: Option<ControlChannel>,
    /// Browser: the directory of the last good WebAssembly build.
    wasm: Option<PathBuf>,
    /// Desktop: a rebuilt game waiting for `RestartGame`.
    rebuilt: Option<PathBuf>,
}

impl Drop for Opened {
    fn drop(&mut self) {
        self.alive.store(false, Ordering::SeqCst);
        if let Some(process) = lock(&self.game).process.take() {
            lock(&process).take();
        }
    }
}

impl ProjectService {
    /// `engine_path` is the engine repository new projects depend on when `Create` names none.
    pub fn new(mode: ServiceMode, engine_path: Option<PathBuf>) -> Self {
        ProjectService {
            mode,
            engine_path,
            events: broadcast::channel(EVENT_CAPACITY).0,
            opened: None,
            editor_connected: Arc::default(),
        }
    }

    /// Desktop: the editor went away (its window reloaded or closed), so rebuilds swap the game
    /// at once until the next hello.
    pub fn editor_disconnected(&self) {
        self.editor_connected.store(false, Ordering::SeqCst);
    }

    /// The events of every open project from now on, for one client.
    pub fn subscribe(&self) -> broadcast::Receiver<ProjectEvent> {
        self.events.subscribe()
    }

    /// The root of the open project.
    pub fn root(&self) -> Option<PathBuf> {
        let opened = self.opened.as_ref()?;
        Some(lock(&opened.project).root().to_path_buf())
    }

    /// Desktop: the lines to send to the running game and the protocol lines it prints. Each
    /// game started (announced by `GameReady`) has its own channel, taken once.
    pub fn take_control_channel(&mut self) -> Option<ControlChannel> {
        lock(&self.opened.as_ref()?.game).control.take()
    }

    /// What an editor connecting now (a reloaded window, a build started before it) missed:
    /// the events of the current or last build, then, on desktop, `GameReady` when a game
    /// runs and `GameRebuilt` when a rebuilt one waits for `RestartGame`.
    pub fn announcements(&self) -> Vec<ProjectEvent> {
        self.subscribe_caught_up().0
    }

    /// `announcements` and a subscription to the events after them, taken together so no
    /// event is missed or repeated.
    pub fn subscribe_caught_up(&self) -> (Vec<ProjectEvent>, broadcast::Receiver<ProjectEvent>) {
        let Some(opened) = &self.opened else {
            return (Vec::new(), self.subscribe());
        };
        // Game events are sent under the game lock and build events under the log lock.
        let game = lock(&opened.game);
        let mut events = lock(&opened.last_build).clone();
        let subscription = self.subscribe();
        if game.process.is_some() {
            events.push(ProjectEvent::GameReady);
        }
        if game.rebuilt.is_some() {
            events.push(ProjectEvent::GameRebuilt);
        }
        (events, subscription)
    }

    pub fn handle(&mut self, request: ProjectRequest) -> Result<ProjectReply, ProjectFailure> {
        let reply = self.dispatch(request);
        if let Some(opened) = &self.opened {
            for warning in lock(&opened.project).take_warnings() {
                let message = warning_message(&warning);
                let _ = self.events.send(ProjectEvent::Warning { message });
            }
        }
        reply
    }

    /// Answers one message of a text transport: the handshake until `connected`, then
    /// requests. A message that cannot be read gets a `Refused` or an error response.
    pub fn answer(&mut self, connected: &mut bool, text: &str) -> ProjectServerMessage {
        match serde_json::from_str::<ProjectClientMessage>(text) {
            Ok(ProjectClientMessage::Hello {
                protocol_version,
                protocol_fingerprint,
            }) => {
                let reply = handshake(protocol_version, protocol_fingerprint);
                *connected = matches!(reply, ProjectServerMessage::Welcome { .. });
                if *connected {
                    self.editor_connected.store(true, Ordering::SeqCst);
                }
                reply
            }
            Ok(ProjectClientMessage::Request { id, request }) if *connected => {
                ProjectServerMessage::Response {
                    id,
                    result: self.handle(request),
                }
            }
            Ok(ProjectClientMessage::Request { .. }) => not_connected(),
            Err(error) => {
                let id = serde_json::from_str::<serde_json::Value>(text)
                    .ok()
                    .and_then(|value| value.get("id")?.as_u64());
                match (*connected, id) {
                    (false, _) => match unreadable_hello(text) {
                        Some(Some(version)) if version != PROJECT_PROTOCOL_VERSION => {
                            ProjectServerMessage::Refused {
                                error: version_mismatch(version),
                            }
                        }
                        Some(_) => ProjectServerMessage::Refused {
                            error: ProjectFailure::new(
                                FailureCode::InvalidRequest,
                                format!(
                                    "cannot read this hello: {error}; a hello needs \
                                     protocol_version and protocol_fingerprint"
                                ),
                            ),
                        },
                        None => not_connected(),
                    },
                    (true, Some(id)) => ProjectServerMessage::Response {
                        id,
                        result: Err(ProjectFailure::new(
                            FailureCode::InvalidRequest,
                            format!("cannot read this request: {error}"),
                        )),
                    },
                    (true, None) => ProjectServerMessage::Refused {
                        error: ProjectFailure::new(
                            FailureCode::InvalidRequest,
                            format!("cannot read this message: {error}"),
                        ),
                    },
                }
            }
        }
    }

    fn dispatch(&mut self, request: ProjectRequest) -> Result<ProjectReply, ProjectFailure> {
        match request {
            ProjectRequest::Create {
                parent,
                name,
                engine_path,
            } => {
                let engine_path = engine_path
                    .map(PathBuf::from)
                    .or_else(|| self.engine_path.clone())
                    .ok_or(ProjectError::NoEnginePath)?;
                let project = Project::create(Path::new(&parent), &name, &engine_path)?;
                self.install(project)
            }
            ProjectRequest::Open { path } => {
                let project = Project::open(Path::new(&path))?;
                self.install(project)
            }
            ProjectRequest::Info => Ok(ProjectReply::Info(self.info(&*self.project()?))),
            ProjectRequest::ListFiles => Ok(ProjectReply::Files {
                entries: self.project()?.files(),
            }),
            ProjectRequest::ReadText { path } => Ok(ProjectReply::Text {
                text: self.project()?.read_text(&path)?,
            }),
            ProjectRequest::WriteText { path, text } => {
                self.project()?.write_text(&path, &text)?;
                Ok(ProjectReply::Done)
            }
            ProjectRequest::ReadBinary { path } => Ok(ProjectReply::Bytes {
                bytes: Bytes(self.project()?.read_binary(&path)?),
            }),
            ProjectRequest::WriteBinary { path, bytes } => {
                self.project()?.write_bytes(&path, &bytes.0)?;
                Ok(ProjectReply::Done)
            }
            ProjectRequest::CreateAsset { path, kind, text } => Ok(ProjectReply::File(
                self.project()?.create_asset(&path, kind, &text)?,
            )),
            ProjectRequest::ReadAssetTexts => {
                let found = self.project()?.asset_texts()?;
                let paths = found.iter().map(|asset| (asset.id, asset.path.clone()));
                Ok(ProjectReply::AssetTexts(AssetTexts {
                    paths: paths.collect(),
                    assets: found
                        .into_iter()
                        .map(|asset| (asset.id, asset.text))
                        .collect(),
                }))
            }
            ProjectRequest::StoreSession { document, bytes } => {
                self.project()?.store_session(document, &bytes.0)?;
                Ok(ProjectReply::Done)
            }
            ProjectRequest::LoadStoredSession { document } => Ok(ProjectReply::StoredSession {
                bytes: self.project()?.load_stored_session(document)?.map(Bytes),
            }),
            ProjectRequest::SetAsideStoredSession { document } => Ok(ProjectReply::SetAside {
                path: self.project()?.set_aside_stored_session(document)?,
            }),
            ProjectRequest::Rename { from, to } => {
                self.project()?.rename(&from, &to)?;
                Ok(ProjectReply::Done)
            }
            ProjectRequest::Delete { path } => {
                self.project()?.delete(&path)?;
                Ok(ProjectReply::Done)
            }
            ProjectRequest::Build => {
                self.build(self.opened()?);
                Ok(ProjectReply::Done)
            }
            // The plugins that bundled; each failing one is reported as `PluginFailed`.
            ProjectRequest::ListPlugins => {
                let mut plugins = Vec::new();
                for event in self.project()?.bundle_plugins() {
                    match event {
                        PluginEvent::Rebuilt(bundle) => plugins.push(bundle),
                        other => {
                            for event in plugin_events(other) {
                                let _ = self.events.send(event);
                            }
                        }
                    }
                }
                Ok(ProjectReply::Plugins { plugins })
            }
            ProjectRequest::GameEndpoint => self.game_endpoint(),
            ProjectRequest::WriteGameTypes { info } => {
                self.project()?.write_game_types(&info)?;
                Ok(ProjectReply::Done)
            }
            ProjectRequest::RestartGame => {
                let mut game = lock(&self.opened()?.game);
                let binary = game.rebuilt.take().ok_or_else(|| {
                    ProjectFailure::new(FailureCode::NoGame, "no rebuilt game waits to start")
                })?;
                // A game that cannot start leaves the previous one running.
                start_game(&mut game, &binary, &self.events)
                    .map_err(|message| ProjectFailure::new(FailureCode::Io, message))?;
                let _ = self.events.send(ProjectEvent::GameReady);
                Ok(ProjectReply::Done)
            }
        }
    }

    fn opened(&self) -> Result<&Opened, ProjectFailure> {
        self.opened
            .as_ref()
            .ok_or_else(|| ProjectFailure::new(FailureCode::NoProject, "no project is open"))
    }

    fn project(&self) -> Result<MutexGuard<'_, Project>, ProjectFailure> {
        Ok(lock(&self.opened()?.project))
    }

    fn info(&self, project: &Project) -> ProjectInfo {
        ProjectInfo {
            name: project.manifest().name.clone(),
            root: project.root().to_string_lossy().into_owned(),
            default_scene: project.manifest().default_scene.clone(),
            mode: self.mode,
        }
    }

    fn game_endpoint(&self) -> Result<ProjectReply, ProjectFailure> {
        let opened = self.opened()?;
        let game = lock(&opened.game);
        if self.mode == ServiceMode::Desktop {
            return match game.process {
                Some(_) => Ok(ProjectReply::Native),
                None => Err(ProjectFailure::new(
                    FailureCode::NoGame,
                    "the game has not started yet",
                )),
            };
        }
        let hash = game
            .wasm
            .as_deref()
            .and_then(Path::file_name)
            .ok_or_else(|| {
                ProjectFailure::new(FailureCode::NoGame, "the game has not been built yet")
            })?;
        Ok(ProjectReply::Wasm {
            glue_url: format!("/game/{}/game.js", hash.to_string_lossy()),
        })
    }

    /// Makes `project` the open one, closing the previous one, and starts building it.
    fn install(&mut self, project: Project) -> Result<ProjectReply, ProjectFailure> {
        let changes = project.watch()?;
        let plugins = project.watch_plugins();
        let info = self.info(&project);
        // Close the previous project first: its game stops and its threads go quiet.
        self.opened = None;
        let opened = Opened {
            project: Arc::new(Mutex::new(project)),
            game: Arc::default(),
            builds: Arc::default(),
            alive: Arc::new(AtomicBool::new(true)),
            last_build: Arc::default(),
        };
        let events = self.events.clone();
        forward_while_open(&opened.alive, changes, move |changes| {
            let _ = events.send(ProjectEvent::FilesChanged { changes });
        });
        let events = self.events.clone();
        forward_while_open(&opened.alive, plugins, move |batch| {
            for event in batch.into_iter().flat_map(plugin_events) {
                let _ = events.send(event);
            }
        });
        self.build(&opened);
        self.opened = Some(opened);
        let _ = self
            .events
            .send(ProjectEvent::ProjectOpened { info: info.clone() });
        Ok(ProjectReply::Info(info))
    }

    /// Builds in the background for this service's mode. Success replaces the game and
    /// announces `GameReady`; failure keeps the previous game.
    fn build(&self, opened: &Opened) {
        let project = Arc::clone(&opened.project);
        let game = Arc::clone(&opened.game);
        let builds = Arc::clone(&opened.builds);
        let alive = Arc::clone(&opened.alive);
        let last_build = Arc::clone(&opened.last_build);
        let editor_connected = Arc::clone(&self.editor_connected);
        let events = self.events.clone();
        let target = match self.mode {
            ServiceMode::Desktop => BuildTarget::Native,
            ServiceMode::Browser => BuildTarget::Wasm,
        };
        thread::spawn(move || {
            let _one_at_a_time = lock(&builds);
            if !alive.load(Ordering::SeqCst) {
                return;
            }
            lock(&last_build).clear();
            let (sender, receiver) = mpsc::channel();
            let handle = lock(&project).build(target, sender);
            for event in receiver {
                record(&last_build, &events, ProjectEvent::Build { event });
            }
            let failure = match handle.join() {
                Ok(Ok(artifact)) => {
                    let mut game = lock(&game);
                    // Checked under the lock: closing the project clears the flag before taking it.
                    if !alive.load(Ordering::SeqCst) {
                        return;
                    }
                    let hold = editor_connected.load(Ordering::SeqCst);
                    return finish_build(&mut game, artifact, hold, &events);
                }
                // The diagnostics already tell why.
                Ok(Err(BuildError::Failed)) => return,
                Ok(Err(error)) => format!("the build failed: {error}"),
                Err(_) => "the build thread panicked".to_string(),
            };
            record(
                &last_build,
                &events,
                ProjectEvent::Warning { message: failure },
            );
        });
    }
}

/// Hands each batch `watch` reports to `forward` on a thread of its own until the project
/// closes, then drops `watch`, which stops its watcher.
fn forward_while_open<T: Send + 'static>(
    alive: &Arc<AtomicBool>,
    watch: Watch<T>,
    mut forward: impl FnMut(Vec<T>) + Send + 'static,
) {
    let alive = Arc::clone(alive);
    thread::spawn(move || {
        while alive.load(Ordering::SeqCst) {
            match watch.recv_timeout(WATCH_POLL) {
                Ok(first) => {
                    let mut batch = vec![first];
                    batch.extend(watch.try_iter());
                    if alive.load(Ordering::SeqCst) {
                        forward(batch);
                    }
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(mpsc::RecvTimeoutError::Disconnected) => return,
            }
        }
    });
}

/// A plugin event for the editor: its warnings become one console warning each.
fn plugin_events(event: PluginEvent) -> Vec<ProjectEvent> {
    match event {
        PluginEvent::Rebuilt(bundle) => vec![ProjectEvent::PluginRebuilt(bundle)],
        PluginEvent::Failed { name, diagnostics } => {
            vec![ProjectEvent::PluginFailed { name, diagnostics }]
        }
        PluginEvent::Warnings { name, diagnostics } => diagnostics
            .into_iter()
            .map(|diagnostic| {
                let place = match (&diagnostic.file, diagnostic.line) {
                    (Some(file), Some(line)) => format!(" ({file}:{line})"),
                    (Some(file), None) => format!(" ({file})"),
                    _ => String::new(),
                };
                ProjectEvent::Warning {
                    message: format!("plugin {name}{place}: {}", diagnostic.message),
                }
            })
            .collect(),
    }
}

/// Sends a build's event and keeps it for `announcements`, a progress replacing the last one.
fn record(log: &BuildLog, events: &broadcast::Sender<ProjectEvent>, event: ProjectEvent) {
    let mut log = lock(log);
    let progress = |event: &ProjectEvent| {
        matches!(
            event,
            ProjectEvent::Build {
                event: BuildEvent::Progress { .. }
            }
        )
    };
    if progress(&event) {
        log.retain(|kept| !progress(kept));
    }
    log.push(event.clone());
    // Sent under the lock, so `subscribe_caught_up` sees each event exactly once.
    let _ = events.send(event);
}

/// Puts a successful build in place and announces it. On desktop with an editor connected
/// (`hold`), a running game keeps running until `RestartGame`, so the editor can export its
/// session first; a newer build replaces the waiting one without a second announcement, so
/// the editor exports once.
fn finish_build(
    game: &mut Game,
    artifact: BuildArtifact,
    hold: bool,
    events: &broadcast::Sender<ProjectEvent>,
) {
    match artifact {
        BuildArtifact::Wasm { dir } => game.wasm = Some(dir),
        BuildArtifact::Native { binary } if hold && game.process.is_some() => {
            if game.rebuilt.replace(binary).is_none() {
                let _ = events.send(ProjectEvent::GameRebuilt);
            }
            return;
        }
        BuildArtifact::Native { binary } => {
            if let Err(message) = start_game(game, &binary, events) {
                return warn(events, message);
            }
        }
    }
    let _ = events.send(ProjectEvent::GameReady);
}

/// Starts `binary`, then stops the running game, if any, and puts the new one in its place.
/// When `binary` cannot start, the running game stays.
fn start_game(
    game: &mut Game,
    binary: &Path,
    events: &broadcast::Sender<ProjectEvent>,
) -> Result<(), String> {
    let process =
        GameProcess::start(binary).map_err(|error| format!("cannot start the game: {error}"))?;
    if let Some(previous) = game.process.take() {
        lock(&previous).take();
    }
    game.rebuilt = None;
    let (slot, control) = relay(process, events);
    game.process = Some(slot);
    game.control = Some(control);
    Ok(())
}

/// Relays a started game: its protocol lines to the returned channel, its other output and its
/// exit to the events.
fn relay(
    mut process: GameProcess,
    events: &broadcast::Sender<ProjectEvent>,
) -> (GameSlot, ControlChannel) {
    let messages = process.take_messages().expect("a new game's messages");
    let output = process.take_output().expect("a new game's output");
    let slot: GameSlot = Arc::new(Mutex::new(Some(process)));
    let (to_game, input) = mpsc::channel::<String>();
    let (from_game, relayed) = mpsc::channel();

    let writer = Arc::clone(&slot);
    thread::spawn(move || {
        for line in input {
            let mut slot = lock(&writer);
            let Some(process) = slot.as_mut() else { return };
            if process.send(&line).is_err() {
                return;
            }
        }
    });
    let output_events = events.clone();
    thread::spawn(move || {
        for line in output {
            let _ = output_events.send(ProjectEvent::GameOutput(line));
        }
    });
    let (watcher, events) = (Arc::clone(&slot), events.clone());
    thread::spawn(move || {
        for line in messages {
            let _ = from_game.send(line);
        }
        // The game closed its output, so it is exiting: give it a moment to report a code.
        for _ in 0..EXIT_POLLS {
            let mut slot = lock(&watcher);
            // A replaced game is not news.
            let Some(process) = slot.as_mut() else { return };
            if let Some(status) = process.wait() {
                let code = status.code();
                let _ = events.send(ProjectEvent::GameExited { code });
                return;
            }
            drop(slot);
            thread::sleep(EXIT_POLL_INTERVAL);
        }
        if lock(&watcher).is_some() {
            let _ = events.send(ProjectEvent::GameExited { code: None });
        }
    });
    (slot, (to_game, relayed))
}

fn warn(events: &broadcast::Sender<ProjectEvent>, message: String) {
    let _ = events.send(ProjectEvent::Warning { message });
}

fn warning_message(warning: &ProjectWarning) -> String {
    match warning {
        ProjectWarning::OrphanMeta { id, path } => format!(
            "{path}.meta (asset {id}) has no file, probably because {path} was moved outside \
             the editor; the meta was kept"
        ),
        ProjectWarning::InvalidMeta { path, reason } => {
            format!("cannot use {path}: {reason}; its file has no identifier until it is repaired")
        }
        ProjectWarning::DuplicateId { id, paths } => {
            let files = match paths.split_last() {
                Some((last, rest)) if !rest.is_empty() => format!("{} and {last}", rest.join(", ")),
                _ => paths.join(", "),
            };
            format!(
                "{files} have the same identifier {id}, as files copied with their metas do; \
                 delete the .meta of each copy to give it an identifier of its own"
            )
        }
    }
}

fn not_connected() -> ProjectServerMessage {
    ProjectServerMessage::Refused {
        error: ProjectFailure::new(FailureCode::NotConnected, "send Hello before any request"),
    }
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

#[cfg(all(test, unix))]
mod tests {
    use std::fs;
    use std::time::Duration;

    use tempfile::TempDir;

    use super::*;
    use crate::game::tests::script;
    use crate::protocol::PROJECT_PROTOCOL_FINGERPRINT;

    /// A stand-in game binary answering each line with `{"type":"<name>"}`.
    fn fake_game(dir: &Path, name: &str) -> PathBuf {
        script(
            dir,
            name,
            &format!("while IFS= read -r line; do echo '{{\"type\":\"{name}\"}}'; done"),
        )
    }

    /// Waits for the first `GameExited` and returns its code with how long it took.
    #[expect(
        clippy::disallowed_methods,
        reason = "the test bounds a wait with wall-clock time"
    )]
    fn game_exited(events: &mut broadcast::Receiver<ProjectEvent>) -> (Option<i32>, Duration) {
        let start = std::time::Instant::now();
        while start.elapsed() < Duration::from_secs(10) {
            match events.try_recv() {
                Ok(ProjectEvent::GameExited { code }) => return (code, start.elapsed()),
                Ok(_) => {}
                Err(_) => thread::sleep(Duration::from_millis(5)),
            }
        }
        panic!("the game never exited");
    }

    /// A desktop service with a project open (its own build fails fast: no engine).
    fn desktop_service(dir: &TempDir) -> ProjectService {
        let root = dir.path().join("demo");
        fs::create_dir_all(root.join("src")).unwrap();
        fs::write(
            root.join("ludenea.toml"),
            "name = \"demo\"\nengine = \"0.1.0\"\n",
        )
        .unwrap();
        fs::write(
            root.join("Cargo.toml"),
            "[package]\nname = \"demo\"\nversion = \"0.1.0\"\nedition = \"2024\"\n\n[workspace]\n",
        )
        .unwrap();
        fs::write(root.join("src/lib.rs"), "").unwrap();
        let mut service = ProjectService::new(ServiceMode::Desktop, None);
        let path = root.to_string_lossy().into_owned();
        service.handle(ProjectRequest::Open { path }).unwrap();
        service
    }

    /// Asks the running game over its control channel and returns who answered.
    fn ask(service: &mut ProjectService) -> String {
        let (to_game, from_game) = service.take_control_channel().expect("a game runs");
        to_game.send("{}".to_string()).unwrap();
        from_game.recv_timeout(Duration::from_secs(10)).unwrap()
    }

    fn finish(service: &ProjectService, binary: PathBuf, hold: bool) {
        let opened = service.opened.as_ref().unwrap();
        finish_build(
            &mut lock(&opened.game),
            BuildArtifact::Native { binary },
            hold,
            &service.events,
        );
    }

    /// The announcements about games, without those of the project's own (failing) build.
    fn game_announcements(service: &ProjectService) -> Vec<ProjectEvent> {
        let mut events = service.announcements();
        events.retain(|event| {
            !matches!(
                event,
                ProjectEvent::Build { .. } | ProjectEvent::Warning { .. }
            )
        });
        events
    }

    fn received(events: &mut broadcast::Receiver<ProjectEvent>) -> Vec<ProjectEvent> {
        std::iter::from_fn(|| events.try_recv().ok())
            .filter(|event| matches!(event, ProjectEvent::GameReady | ProjectEvent::GameRebuilt))
            .collect()
    }

    #[test]
    fn an_exit_code_is_announced() {
        let dir = TempDir::new().unwrap();
        let mut service = desktop_service(&dir);
        let mut events = service.subscribe();
        finish(
            &service,
            script(dir.path(), "exits", "read -r line; exit 3"),
            false,
        );
        let (to_game, _from_game) = service.take_control_channel().unwrap();
        to_game.send("{}".to_string()).unwrap();
        assert_eq!(game_exited(&mut events).0, Some(3));
    }

    /// A game a signal ended has no code to wait for.
    #[test]
    fn a_signal_exit_is_announced_at_once() {
        let dir = TempDir::new().unwrap();
        let mut service = desktop_service(&dir);
        let mut events = service.subscribe();
        finish(
            &service,
            script(dir.path(), "killed", "read -r line; kill -9 $$"),
            false,
        );
        let (to_game, _from_game) = service.take_control_channel().unwrap();
        to_game.send("{}".to_string()).unwrap();
        let (code, waited) = game_exited(&mut events);
        assert_eq!(code, None);
        assert!(waited < EXIT_POLL_INTERVAL * EXIT_POLLS / 2, "{waited:?}");
    }

    #[test]
    fn a_game_that_cannot_start_keeps_the_previous_one() {
        let dir = TempDir::new().unwrap();
        let mut service = desktop_service(&dir);
        finish(&service, fake_game(dir.path(), "first"), false);
        finish(&service, dir.path().join("missing"), false);
        assert_eq!(ask(&mut service), r#"{"type":"first"}"#);

        finish(&service, dir.path().join("missing"), true);
        let failure = service.handle(ProjectRequest::RestartGame).unwrap_err();
        assert_eq!(failure.code, FailureCode::Io);
        assert_eq!(game_announcements(&service), [ProjectEvent::GameReady]);
    }

    /// The editor exports its session once for two rebuilds; the newest one starts.
    #[test]
    fn rebuilds_waiting_for_a_restart_are_announced_once() {
        let dir = TempDir::new().unwrap();
        let mut service = desktop_service(&dir);
        let mut events = service.subscribe();
        finish(&service, fake_game(dir.path(), "first"), true);
        finish(&service, fake_game(dir.path(), "second"), true);
        finish(&service, fake_game(dir.path(), "third"), true);
        assert_eq!(
            received(&mut events),
            [ProjectEvent::GameReady, ProjectEvent::GameRebuilt]
        );
        service.take_control_channel();
        service.handle(ProjectRequest::RestartGame).unwrap();
        assert_eq!(ask(&mut service), r#"{"type":"third"}"#);
    }

    #[test]
    fn desktop_game_endpoint_waits_for_a_game() {
        let dir = TempDir::new().unwrap();
        let mut service = desktop_service(&dir);
        let failure = service.handle(ProjectRequest::GameEndpoint).unwrap_err();
        assert_eq!(failure.code, FailureCode::NoGame);
        finish(&service, fake_game(dir.path(), "first"), false);
        assert_eq!(
            service.handle(ProjectRequest::GameEndpoint),
            Ok(ProjectReply::Native)
        );
    }

    /// Every thread of a closed project ends, watchers included, even if nothing changes.
    #[test]
    #[expect(
        clippy::disallowed_methods,
        reason = "the test bounds a wait with wall-clock time"
    )]
    fn closing_a_project_stops_its_threads() {
        let dir = TempDir::new().unwrap();
        let mut service = desktop_service(&dir);
        let alive = Arc::clone(&service.opened.as_ref().unwrap().alive);
        service.opened = None;
        let start = std::time::Instant::now();
        while Arc::strong_count(&alive) > 1 {
            assert!(
                start.elapsed() < Duration::from_secs(60),
                "a thread still runs"
            );
            thread::sleep(Duration::from_millis(20));
        }
    }

    #[test]
    fn hello_makes_desktop_rebuilds_wait() {
        let mut service = ProjectService::new(ServiceMode::Desktop, None);
        assert!(!service.editor_connected.load(Ordering::SeqCst));
        let hello = serde_json::to_string(&ProjectClientMessage::Hello {
            protocol_version: PROJECT_PROTOCOL_VERSION,
            protocol_fingerprint: PROJECT_PROTOCOL_FINGERPRINT,
        })
        .unwrap();
        let mut connected = false;
        assert_eq!(
            service.answer(&mut connected, &hello),
            ProjectServerMessage::Welcome {
                protocol_version: PROJECT_PROTOCOL_VERSION,
                protocol_fingerprint: PROJECT_PROTOCOL_FINGERPRINT,
            }
        );
        assert!(connected);
        assert!(service.editor_connected.load(Ordering::SeqCst));
    }

    #[test]
    fn rebuild_keeps_old_game_until_restart_game() {
        let dir = TempDir::new().unwrap();
        let mut service = desktop_service(&dir);
        let mut events = service.subscribe();
        finish(&service, fake_game(dir.path(), "first"), true);
        assert_eq!(received(&mut events), [ProjectEvent::GameReady]);
        assert_eq!(ask(&mut service), r#"{"type":"first"}"#);
        finish(&service, fake_game(dir.path(), "second"), true);
        assert_eq!(received(&mut events), [ProjectEvent::GameRebuilt]);
        // A reloaded editor learns both.
        assert_eq!(
            game_announcements(&service),
            [ProjectEvent::GameReady, ProjectEvent::GameRebuilt]
        );

        assert_eq!(
            service.handle(ProjectRequest::RestartGame),
            Ok(ProjectReply::Done)
        );
        assert_eq!(received(&mut events), [ProjectEvent::GameReady]);
        assert_eq!(ask(&mut service), r#"{"type":"second"}"#);
        assert_eq!(game_announcements(&service), [ProjectEvent::GameReady]);
        let again = service.handle(ProjectRequest::RestartGame).unwrap_err();
        assert_eq!(again.code, FailureCode::NoGame);
    }

    #[test]
    fn rebuild_without_editor_swaps_at_once() {
        let dir = TempDir::new().unwrap();
        let mut service = desktop_service(&dir);
        let mut events = service.subscribe();
        finish(&service, fake_game(dir.path(), "first"), false);
        finish(&service, fake_game(dir.path(), "second"), false);
        assert_eq!(
            received(&mut events),
            [ProjectEvent::GameReady, ProjectEvent::GameReady]
        );
        assert_eq!(ask(&mut service), r#"{"type":"second"}"#);
    }

    #[test]
    fn a_build_finished_before_the_editor_connected_is_announced() {
        let dir = TempDir::new().unwrap();
        // Its build fails fast: the project has no binary.
        let service = desktop_service(&dir);
        let failed = ProjectEvent::Build {
            event: BuildEvent::Finished { ok: false },
        };
        let mut announced = Vec::new();
        for _ in 0..3_000 {
            announced = service.announcements();
            if announced.contains(&failed) {
                break;
            }
            thread::sleep(Duration::from_millis(20));
        }
        assert!(announced.contains(&failed), "{announced:?}");
        let error = announced.iter().any(|event| {
            matches!(event, ProjectEvent::Build { event: BuildEvent::Line { text } } if text.starts_with("error"))
        });
        assert!(error, "the reason is announced too: {announced:?}");
        let (caught_up, _events) = service.subscribe_caught_up();
        assert_eq!(caught_up, announced);
    }
}
