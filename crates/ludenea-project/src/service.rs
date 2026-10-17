//! The project service: answers the project protocol for one open project, builds the game in
//! the background, and, on desktop, runs it. Shared by `ludenea serve` and the desktop backend.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread;
use std::time::Duration;

use tokio::sync::broadcast;

use crate::protocol::{
    FailureCode, ProjectClientMessage, ProjectEvent, ProjectFailure, ProjectInfo, ProjectReply,
    ProjectRequest, ProjectServerMessage, ServiceMode, handshake,
};
use crate::{
    BuildArtifact, BuildError, BuildTarget, GameProcess, PluginEvent, Project, ProjectError,
    ProjectWarning,
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
    /// Set once an editor said hello: from then on a desktop rebuild waits for `RestartGame`.
    editor_connected: Arc<AtomicBool>,
}

/// The open project and what its background threads share.
struct Opened {
    project: Arc<Mutex<Project>>,
    game: Arc<Mutex<Game>>,
    /// Builds share a staging directory, so they run one at a time.
    builds: Arc<Mutex<()>>,
    /// Cleared when the project is closed; background threads then stop.
    alive: Arc<AtomicBool>,
}

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

    /// Desktop: what an editor connecting now (a reloaded window) missed and needs to catch
    /// up with the games: `GameReady` when one runs, then `GameRebuilt` when a rebuilt one
    /// waits for `RestartGame`.
    pub fn announcements(&self) -> Vec<ProjectEvent> {
        let Some(opened) = &self.opened else {
            return Vec::new();
        };
        let game = lock(&opened.game);
        let mut events = Vec::new();
        if game.process.is_some() {
            events.push(ProjectEvent::GameReady);
        }
        if game.rebuilt.is_some() {
            events.push(ProjectEvent::GameRebuilt);
        }
        events
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
            Ok(ProjectClientMessage::Hello { protocol_version }) => {
                let reply = handshake(protocol_version);
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
                    (false, _) => not_connected(),
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
            ProjectRequest::ListPlugins => {
                let plugins = self.project()?.bundle_plugins().map_err(|diagnostics| {
                    let messages: Vec<_> = diagnostics.into_iter().map(|d| d.message).collect();
                    ProjectFailure::new(FailureCode::PluginFailed, messages.join("\n"))
                })?;
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
        if self.mode == ServiceMode::Desktop {
            return Ok(ProjectReply::Native);
        }
        let game = lock(&opened.game);
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
        };
        let (alive, events) = (Arc::clone(&opened.alive), self.events.clone());
        thread::spawn(move || {
            while let Ok(change) = changes.recv() {
                if !alive.load(Ordering::SeqCst) {
                    return;
                }
                let mut batch = vec![change];
                batch.extend(changes.try_iter());
                let _ = events.send(ProjectEvent::FilesChanged { changes: batch });
            }
        });
        let (alive, events) = (Arc::clone(&opened.alive), self.events.clone());
        thread::spawn(move || {
            for event in plugins {
                if !alive.load(Ordering::SeqCst) {
                    return;
                }
                let _ = events.send(match event {
                    PluginEvent::Rebuilt(bundle) => ProjectEvent::PluginRebuilt(bundle),
                    PluginEvent::Failed { name, diagnostics } => {
                        ProjectEvent::PluginFailed { name, diagnostics }
                    }
                });
            }
        });
        self.build(&opened);
        self.opened = Some(opened);
        Ok(ProjectReply::Info(info))
    }

    /// Builds in the background for this service's mode. Success replaces the game and
    /// announces `GameReady`; failure keeps the previous game.
    fn build(&self, opened: &Opened) {
        let project = Arc::clone(&opened.project);
        let game = Arc::clone(&opened.game);
        let builds = Arc::clone(&opened.builds);
        let alive = Arc::clone(&opened.alive);
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
            let (sender, receiver) = mpsc::channel();
            let handle = lock(&project).build(target, sender);
            for event in receiver {
                let _ = events.send(ProjectEvent::Build { event });
            }
            let artifact = match handle.join() {
                Ok(Ok(artifact)) => artifact,
                // The diagnostics already tell why.
                Ok(Err(BuildError::Failed)) => return,
                Ok(Err(error)) => return warn(&events, format!("the build failed: {error}")),
                Err(_) => return warn(&events, "the build thread panicked".to_string()),
            };
            let mut game = lock(&game);
            // Checked under the lock: closing the project clears the flag before taking it.
            if !alive.load(Ordering::SeqCst) {
                return;
            }
            let hold = editor_connected.load(Ordering::SeqCst);
            finish_build(&mut game, artifact, hold, &events);
        });
    }
}

/// Puts a successful build in place and announces it. On desktop with an editor connected
/// (`hold`), a running game keeps running until `RestartGame`, so the editor can export its
/// session first.
fn finish_build(
    game: &mut Game,
    artifact: BuildArtifact,
    hold: bool,
    events: &broadcast::Sender<ProjectEvent>,
) {
    match artifact {
        BuildArtifact::Wasm { dir } => game.wasm = Some(dir),
        BuildArtifact::Native { binary } if hold && game.process.is_some() => {
            game.rebuilt = Some(binary);
            let _ = events.send(ProjectEvent::GameRebuilt);
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

/// Stops the running game, if any, and starts `binary` in its place.
fn start_game(
    game: &mut Game,
    binary: &Path,
    events: &broadcast::Sender<ProjectEvent>,
) -> Result<(), String> {
    if let Some(previous) = game.process.take() {
        lock(&previous).take();
    }
    game.control = None;
    game.rebuilt = None;
    let process =
        GameProcess::start(binary).map_err(|error| format!("cannot start the game: {error}"))?;
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
    let messages = process.take_messages();
    let output = process.take_output();
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
            if let Some(code) = process.wait() {
                let _ = events.send(ProjectEvent::GameExited { code: Some(code) });
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
    use std::os::unix::fs::PermissionsExt;
    use std::time::Duration;

    use tempfile::TempDir;

    use super::*;

    /// A stand-in game binary answering each line with `{"type":"<name>"}`.
    fn fake_game(dir: &Path, name: &str) -> PathBuf {
        let path = dir.join(name);
        let script = format!(
            "#!/bin/sh\nwhile IFS= read -r line; do echo '{{\"type\":\"{name}\"}}'; done\n"
        );
        fs::write(&path, script).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
        path
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

    fn received(events: &mut broadcast::Receiver<ProjectEvent>) -> Vec<ProjectEvent> {
        std::iter::from_fn(|| events.try_recv().ok())
            .filter(|event| matches!(event, ProjectEvent::GameReady | ProjectEvent::GameRebuilt))
            .collect()
    }

    #[test]
    fn hello_makes_desktop_rebuilds_wait() {
        let mut service = ProjectService::new(ServiceMode::Desktop, None);
        assert!(!service.editor_connected.load(Ordering::SeqCst));
        service.answer(&mut false, r#"{"type":"hello","protocol_version":1}"#);
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
            service.announcements(),
            [ProjectEvent::GameReady, ProjectEvent::GameRebuilt]
        );

        assert_eq!(
            service.handle(ProjectRequest::RestartGame),
            Ok(ProjectReply::Done)
        );
        assert_eq!(received(&mut events), [ProjectEvent::GameReady]);
        assert_eq!(ask(&mut service), r#"{"type":"second"}"#);
        assert_eq!(service.announcements(), [ProjectEvent::GameReady]);
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
}
