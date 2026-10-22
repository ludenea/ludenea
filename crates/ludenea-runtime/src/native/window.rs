//! `play`: a game in a native window. The window opens on the lobby, where local players claim
//! their devices (unless the player count is given), then runs the match through the
//! `Runner`: each displayed frame reads the keyboard and the gamepads, lets the host run the
//! ticks due, and draws the newest two ticks blended at the display's rate.

use std::collections::{BTreeMap, BTreeSet};
use std::io::Write;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use gilrs::{EventType, Gilrs};
use ludenea_app::App;
use ludenea_app::players::{
    Claim, LiveDevices, Lobby, LobbyEvent, LobbyPress, MAX_LOCAL_PLAYERS, automatic_claims,
};
use ludenea_app::project::ProjectTexture;
use ludenea_control::ConsoleLevel;
use ludenea_draw::frame::RenderFrame;
use ludenea_render::gpu::{Overlay, Renderer, for_surface};
use winit::application::ApplicationHandler;
use winit::dpi::LogicalSize;
use winit::event::{ElementState, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::keyboard::{KeyCode, PhysicalKey};
use winit::window::{Fullscreen, Window, WindowId};

use super::surface::{Display, OpenScreen, Presenter, now};
use super::watch::TextureWatch;
use super::{gamepads, keys, lobby};
use crate::stats::FrameStats;
use crate::{GameFiles, Runner, RuntimeError};

/// How long the loop waits between frames while the window cannot draw (minimized or
/// hidden), when the display no longer paces it: the simulation keeps its pace meanwhile.
const HIDDEN_FRAME: Duration = Duration::from_millis(4);

/// What to play and how.
#[derive(Clone, Debug)]
pub struct PlayOptions {
    pub files: GameFiles,
    /// The project's textures, packed into the atlas at launch.
    pub textures: Vec<ProjectTexture>,
    /// The scene and the project folder whose textures are watched while the window plays
    /// (`TextureWatch::start`), each change repacked into the atlas; `None` to never reload
    /// them.
    pub watch: Option<(PathBuf, Option<PathBuf>)>,
    /// The local players, from 1 to `MAX_LOCAL_PLAYERS`, with the automatic claims: no lobby.
    pub players: Option<u8>,
    /// Ends the match at the first frame that reaches this tick.
    pub ticks: Option<u64>,
    /// Where to write the replay of the match when the window closes, after a failure too.
    pub record: Option<PathBuf>,
    /// Opens the window borderless fullscreen (F11 toggles it).
    pub fullscreen: bool,
    /// Presents in step with the display; off, as fast as the GPU allows.
    pub vsync: bool,
    /// Measures each displayed frame for `PlayOutcome::stats`.
    pub stats: bool,
    /// The game's thread count; the host's default when `None`.
    pub threads: Option<usize>,
    /// Debug draw layers drawn over the game.
    pub debug_layers: Vec<String>,
    /// The window's inner size in logical pixels.
    pub size: (u32, u32),
    pub title: String,
}

/// How a played session ended.
#[derive(Clone, Debug, PartialEq)]
pub struct PlayOutcome {
    /// The tick the match reached.
    pub ticks: u64,
    /// The frame time summary (`FrameStats::summary`), when `PlayOptions::stats` asked.
    pub stats: Option<String>,
}

/// Loads the game, then opens its window and plays until the window closes or the match
/// reaches `options.ticks`. A game that cannot load fails before any window opens; a game that
/// fails while it runs closes the window and fails naming the tick. The replay is written to
/// `options.record` in every case; failing to write it fails the session, unless it failed
/// already (the replay's error is then printed). With `options.watch`, the textures that
/// change on disk are repacked while the window plays, a broken one keeping its previous
/// version; the simulation and its hashes never see it. The window's warnings (textures left
/// out of the atlas or not reloaded, a lost GPU device, gamepads that cannot be read) are
/// written to `out`, as the command line tool's `render` writes its own; the game's console
/// goes to the standard error.
pub fn play(
    app: fn() -> App,
    options: PlayOptions,
    out: &mut dyn Write,
) -> Result<PlayOutcome, RuntimeError> {
    if let Some(players) = options.players
        && !(1..=MAX_LOCAL_PLAYERS).contains(&usize::from(players))
    {
        return Err(RuntimeError::Load(format!(
            "{players} players: a window holds 1 to {MAX_LOCAL_PLAYERS} local players"
        )));
    }
    let mut runner = Runner::load(
        app,
        &options.files,
        options.threads,
        options.debug_layers.clone(),
    )?;
    if let Some(ticks) = options.ticks {
        runner.stop_at(ticks);
    }
    let mut session = Session::new(runner, options, out);
    session.print_messages();
    let ran = EventLoop::new()
        .map_err(|error| RuntimeError::Window(format!("cannot open a window: {error}")))
        .and_then(|event_loop| {
            event_loop.run_app(&mut session).map_err(|error| {
                RuntimeError::Window(format!("the window's event loop failed: {error}"))
            })
        });
    session.finish(ran)
}

/// The state of a window being played.
struct Session<'out> {
    /// Where the window's warnings go.
    out: &'out mut dyn Write,
    runner: Runner,
    options: PlayOptions,
    /// The lobby, until the match starts; `None` when the player count was given.
    lobby: Option<Lobby>,
    /// The lobby frame last shown, so a new one is uploaded only when it changes.
    lobby_shown: Option<RenderFrame>,
    started: bool,
    window: Option<Arc<Window>>,
    display: Option<Display>,
    /// `None` when the textures are not reloaded, or the watcher could not start (a warning
    /// says why).
    watch: Option<TextureWatch>,
    occluded: bool,
    /// `None` when the platform's gamepads cannot be read (a warning says why).
    gilrs: Option<Gilrs>,
    slots: gamepads::Slots,
    /// The `KeyboardEvent.code` names of the keys held down.
    keys: BTreeSet<String>,
    /// The keys held when the match started, such as the Enter that started it: they reach
    /// the game only once pressed again, not through the keyboard's repeats.
    held_at_start: BTreeSet<String>,
    /// Per gamepad slot, the buttons held when the match started, such as the Start that
    /// started it: they reach the game only once released and pressed again.
    buttons_held_at_start: Vec<BTreeSet<usize>>,
    overlay: Overlay,
    lobby_overlay: Overlay,
    /// The display holds no frame of the match yet.
    fresh: bool,
    /// When the last frame started, `None` before the match's first frame.
    last: Option<Instant>,
    stats: FrameStats,
    failure: Option<RuntimeError>,
}

impl<'out> Session<'out> {
    fn new(runner: Runner, options: PlayOptions, out: &'out mut dyn Write) -> Session<'out> {
        let (gilrs, warning) = match Gilrs::new() {
            Ok(gilrs) => (Some(gilrs), None),
            Err(gilrs::Error::NotImplemented(dummy)) => (
                Some(dummy),
                Some("gamepads are not supported on this platform".to_string()),
            ),
            Err(error) => (None, Some(format!("gamepads cannot be read: {error}"))),
        };
        let mut slots = gamepads::Slots::default();
        if let Some(gilrs) = &gilrs {
            for (id, _) in gilrs.gamepads() {
                slots.connect(usize::from(id));
            }
        }
        let overlay = Overlay {
            debug_layers: options.debug_layers.iter().cloned().collect(),
            ..Overlay::default()
        };
        let lobby_overlay = Overlay {
            debug_layers: BTreeSet::from([lobby::LAYER.to_string()]),
            ..Overlay::default()
        };
        let watch = options
            .watch
            .as_ref()
            .map(|(scene, project)| TextureWatch::start(scene, project.as_deref()));
        let (watch, watch_warning) = match watch {
            Some(Ok(watch)) => (Some(watch), None),
            Some(Err(error)) => (None, Some(format!("{error}; textures are not reloaded"))),
            None => (None, None),
        };
        let mut session = Session {
            out,
            runner,
            lobby: options.players.is_none().then(Lobby::new),
            options,
            lobby_shown: None,
            started: false,
            window: None,
            display: None,
            watch,
            occluded: false,
            gilrs,
            slots,
            keys: BTreeSet::new(),
            held_at_start: BTreeSet::new(),
            buttons_held_at_start: Vec::new(),
            overlay,
            lobby_overlay,
            fresh: true,
            last: None,
            stats: FrameStats::default(),
            failure: None,
        };
        for warning in [warning, watch_warning].into_iter().flatten() {
            session.warn(&warning);
        }
        session
    }

    /// Writes a warning to `out`. Best effort: a session does not end because its output
    /// cannot be written to, and nothing else would report it.
    fn warn(&mut self, warning: &str) {
        let _ = writeln!(self.out, "warning: {warning}");
    }

    /// Writes the display's warnings.
    fn print_warnings(&mut self) {
        let warnings = self
            .display
            .as_mut()
            .map(Display::take_warnings)
            .unwrap_or_default();
        for warning in warnings {
            self.warn(&warning);
        }
    }

    /// Repacks the textures when the watcher reports a change, writing its warnings. Only the
    /// display changes: the match never hears of it.
    fn reload_textures(&mut self) {
        let (Some(watch), Some(display)) = (&mut self.watch, &mut self.display) else {
            return;
        };
        let warnings = match watch.poll() {
            None => return,
            Some(Ok(update)) => display.replace_textures(&update),
            Some(Err(error)) => vec![format!("{error}; the textures drawn stay")],
        };
        for warning in warnings {
            self.warn(&warning);
        }
    }

    /// Writes the replay, then gives the session's outcome: the first failure, if any.
    fn finish(mut self, ran: Result<(), RuntimeError>) -> Result<PlayOutcome, RuntimeError> {
        let mut result = match self.failure.take() {
            Some(failure) => Err(failure),
            None => ran,
        };
        if let Some(path) = &self.options.record {
            let recorded = self
                .runner
                .export_replay()
                .map_err(|error| RuntimeError::Record(error.to_string()))
                .and_then(|replay| write(path, &replay));
            self.print_messages();
            match (recorded, &result) {
                (Err(error), Ok(())) => result = Err(error),
                (Err(error), Err(_)) => {
                    let _ = writeln!(self.out, "error: {error}");
                }
                (Ok(()), _) => {}
            }
        }
        result.map(|()| PlayOutcome {
            ticks: self.runner.tick(),
            stats: self.options.stats.then(|| self.stats.summary()),
        })
    }

    /// Prints what the host wrote to its console to the standard error.
    fn print_messages(&mut self) {
        for (level, text) in self.runner.take_messages() {
            match level {
                ConsoleLevel::Info => eprintln!("{text}"),
                ConsoleLevel::Warning => eprintln!("warning: {text}"),
                ConsoleLevel::Error => eprintln!("error: {text}"),
            }
        }
    }

    /// Ends the loop with `error`, keeping the first failure.
    fn fail(&mut self, event_loop: &ActiveEventLoop, error: RuntimeError) {
        self.failure.get_or_insert(error);
        event_loop.exit();
    }

    /// Opens the window and its display, then starts the match unless a lobby comes first.
    fn open(&mut self, event_loop: &ActiveEventLoop) -> Result<(), RuntimeError> {
        let (width, height) = self.options.size;
        let attributes = Window::default_attributes()
            .with_title(self.options.title.clone())
            .with_inner_size(LogicalSize::new(width, height))
            .with_fullscreen(
                self.options
                    .fullscreen
                    .then_some(Fullscreen::Borderless(None)),
            );
        let window = Arc::new(
            event_loop
                .create_window(attributes)
                .map_err(|error| RuntimeError::Window(format!("cannot open a window: {error}")))?,
        );
        let size = window.inner_size();
        let display = Display::new(
            open_screen(Arc::clone(&window), self.options.vsync),
            &self.options.textures,
            (size.width, size.height),
        )?;
        self.window = Some(Arc::clone(&window));
        self.display = Some(display);
        self.print_warnings();
        match &self.lobby {
            Some(lobby) => window.set_title(&lobby::title(&self.options.title, lobby)),
            None => {
                let players = self.options.players.map_or(1, usize::from);
                self.start(automatic_claims(players))?;
            }
        }
        Ok(())
    }

    /// Starts the match with `claims`.
    fn start(&mut self, claims: Vec<Claim>) -> Result<(), RuntimeError> {
        let started = self.runner.start(claims);
        self.print_messages();
        started?;
        self.started = true;
        self.held_at_start = std::mem::take(&mut self.keys);
        self.buttons_held_at_start = gamepads::held(&self.gamepads());
        self.fresh = true;
        self.last = None;
        if let Some(window) = &self.window {
            window.set_title(&self.options.title);
        }
        Ok(())
    }

    /// A lobby press, from a key or a gamepad button.
    fn press(&mut self, press: LobbyPress<'_>) -> Result<(), RuntimeError> {
        let Some(lobby) = &mut self.lobby else {
            return Ok(());
        };
        if lobby.started() {
            return Ok(());
        }
        match lobby.press(press) {
            LobbyEvent::Started { claims } => self.start(claims),
            LobbyEvent::Ignored => Ok(()),
            LobbyEvent::Claimed { .. } | LobbyEvent::Released { .. } | LobbyEvent::Full => {
                if let Some(window) = &self.window {
                    window.set_title(&lobby::title(&self.options.title, lobby));
                }
                Ok(())
            }
        }
    }

    /// Reads the gamepads' events: connections, and in the lobby, button presses.
    fn poll_gamepads(&mut self) -> Result<(), RuntimeError> {
        let Some(gilrs) = &mut self.gilrs else {
            return Ok(());
        };
        let mut presses = Vec::new();
        while let Some(event) = gilrs.next_event() {
            let id = usize::from(event.id);
            match event.event {
                EventType::Connected => {
                    self.slots.connect(id);
                }
                EventType::Disconnected => self.slots.disconnect(id),
                EventType::ButtonPressed(button, _) if !self.started => {
                    if let Some(index) = gamepads::button_index(button) {
                        // The pad is connected: this is its slot, which it keeps.
                        presses.push((self.slots.connect(id), index));
                    }
                }
                _ => {}
            }
        }
        for (gamepad, button) in presses {
            self.press(LobbyPress::Button { gamepad, button })?;
        }
        Ok(())
    }

    /// The connected gamepads' states by slot, `None` for an unplugged slot.
    fn gamepads(&self) -> Vec<Option<ludenea_app::RawGamepad>> {
        let Some(gilrs) = &self.gilrs else {
            return Vec::new();
        };
        let states: BTreeMap<usize, ludenea_app::RawGamepad> = gilrs
            .gamepads()
            .map(|(id, gamepad)| (usize::from(id), gamepads::state(gamepad)))
            .collect();
        self.slots.devices(&states)
    }

    /// One displayed frame: the lobby, or a step of the match and its drawing. Returns
    /// whether the match reached its last tick.
    fn frame(&mut self) -> Result<bool, RuntimeError> {
        let now = now();
        let elapsed = self
            .last
            .replace(now)
            .map_or(0.0, |last| now.duration_since(last).as_secs_f64());
        self.poll_gamepads()?;
        self.reload_textures();
        if !self.started {
            return self.show_lobby().map(|()| false);
        }
        let mut pads = self.gamepads();
        gamepads::mask_held(&mut pads, &mut self.buttons_held_at_start);
        let devices = LiveDevices {
            keys: self.keys.clone(),
            gamepads: pads,
        };
        let update = self.runner.frame(elapsed, &devices);
        self.print_messages();
        let update = update?;
        if let (Some(display), (previous, Some(current))) =
            (&mut self.display, self.runner.frames())
        {
            let new_frames = update.new_frames || self.fresh;
            let presented =
                display.show(previous, current, &self.overlay, new_frames, update.alpha)?;
            self.fresh = false;
            // Only displayed frames count: a hidden window's frames are paced by a timer. A
            // frame's time is its work, without the wait for the screen's texture, which only
            // says the frame came faster than the display refreshes.
            if presented && self.options.stats && elapsed > 0.0 {
                let work = super::surface::now().duration_since(now);
                self.stats
                    .record(work.saturating_sub(display.waited()).as_secs_f64());
            }
        }
        Ok(self.options.ticks.is_some_and(|ticks| update.tick >= ticks))
    }

    fn show_lobby(&mut self) -> Result<(), RuntimeError> {
        let connected: Vec<u32> = self
            .gamepads()
            .iter()
            .zip(0..)
            .filter_map(|(pad, slot)| pad.as_ref().map(|_| slot))
            .collect();
        let (Some(lobby), Some(display)) = (&self.lobby, &mut self.display) else {
            return Ok(());
        };
        let frame = lobby::frame(lobby, &connected);
        let changed = self.lobby_shown.as_ref() != Some(&frame);
        display.show(None, &frame, &self.lobby_overlay, changed, 1.0)?;
        self.lobby_shown = Some(frame);
        Ok(())
    }

    fn run_frame(&mut self, event_loop: &ActiveEventLoop) {
        let ran = self.frame();
        self.print_warnings();
        match ran {
            Ok(true) => event_loop.exit(),
            Ok(false) => {}
            Err(error) => self.fail(event_loop, error),
        }
    }

    fn key(
        &mut self,
        code: KeyCode,
        state: ElementState,
        repeat: bool,
    ) -> Result<(), RuntimeError> {
        if code == KeyCode::F11 {
            // The window's control, not the game's.
            if state == ElementState::Pressed
                && !repeat
                && let Some(window) = &self.window
            {
                let fullscreen = window.fullscreen().is_none();
                window.set_fullscreen(fullscreen.then_some(Fullscreen::Borderless(None)));
            }
            return Ok(());
        }
        let Some(name) = keys::code_name(code) else {
            return Ok(());
        };
        match state {
            ElementState::Pressed => {
                if repeat && self.held_at_start.contains(name) {
                    return Ok(());
                }
                self.held_at_start.remove(name);
                self.keys.insert(name.to_string());
                if !repeat && !self.started {
                    self.press(LobbyPress::Key(name))?;
                }
            }
            ElementState::Released => {
                self.held_at_start.remove(name);
                self.keys.remove(name);
            }
        }
        Ok(())
    }

    fn pace(&self) -> Pace {
        pace(
            self.occluded,
            self.display.as_ref().is_some_and(Display::can_draw),
        )
    }
}

/// How the loop gets its next frame.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Pace {
    /// A redraw request, which the display paces.
    Redraw,
    /// A frame run by the loop itself after `HIDDEN_FRAME`: the display paces nothing.
    Timer,
}

/// Redraw requests while the window draws; the timer while it is occluded, has no pixels or
/// no display, or its surface gives no texture (hidden, lost or outdated), so the simulation
/// keeps its pace without spinning.
fn pace(occluded: bool, can_draw: bool) -> Pace {
    if !occluded && can_draw {
        Pace::Redraw
    } else {
        Pace::Timer
    }
}

impl ApplicationHandler for Session<'_> {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_some() {
            return;
        }
        if let Err(error) = self.open(event_loop) {
            self.fail(event_loop, error);
        }
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _: WindowId, event: WindowEvent) {
        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::Resized(size) => {
                if let Some(display) = &mut self.display {
                    display.resize(size.width, size.height);
                }
            }
            WindowEvent::Occluded(occluded) => self.occluded = occluded,
            // Keys released while another window has the focus never come back.
            WindowEvent::Focused(false) => self.keys.clear(),
            WindowEvent::KeyboardInput { event, .. } => {
                if let PhysicalKey::Code(code) = event.physical_key
                    && let Err(error) = self.key(code, event.state, event.repeat)
                {
                    self.fail(event_loop, error);
                }
            }
            WindowEvent::RedrawRequested => self.run_frame(event_loop),
            _ => {}
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        if event_loop.exiting() {
            return;
        }
        // Drawn frames come from redraw requests, paced by the display; while nothing draws
        // the loop runs the frames itself, so the simulation keeps its pace.
        match self.pace() {
            Pace::Redraw => {
                if let Some(window) = &self.window {
                    window.request_redraw();
                }
                event_loop.set_control_flow(ControlFlow::Wait);
            }
            Pace::Timer => {
                self.run_frame(event_loop);
                event_loop.set_control_flow(ControlFlow::WaitUntil(now() + HIDDEN_FRAME));
            }
        }
    }
}

/// Opens a screen on `window`: a surface, a device that presents to it, and a renderer.
fn open_screen(window: Arc<Window>, vsync: bool) -> OpenScreen {
    Box::new(move || {
        let (surface, adapter, device, queue) =
            pollster::block_on(for_surface(Arc::clone(&window)))
                .map_err(|error| RuntimeError::Gpu(error.to_string()))?;
        let presenter = Presenter::new(surface, &adapter, device.clone(), queue.clone(), vsync)?;
        let renderer = Renderer::new(device, queue, presenter.format());
        Ok((presenter, renderer))
    })
}

/// Writes `bytes` to `path`, creating its folder.
fn write(path: &std::path::Path, bytes: &[u8]) -> Result<(), RuntimeError> {
    let fail = |error: std::io::Error| {
        RuntimeError::Record(format!("cannot write {}: {error}", path.display()))
    };
    if let Some(parent) = path.parent()
        && !parent.as_os_str().is_empty()
    {
        std::fs::create_dir_all(parent).map_err(fail)?;
    }
    std::fs::write(path, bytes).map_err(fail)
}

#[cfg(test)]
mod tests {
    use ludenea_app::replay::Replay;

    use super::*;

    #[test]
    fn a_failed_match_still_writes_its_replay() {
        let folder = tempfile::tempdir().unwrap();
        let record = folder.path().join("nested/match.replay");
        let options = PlayOptions {
            files: GameFiles {
                scene: "Scene(version: 2, next_id: 0, resources: {}, entities: [])".to_string(),
                assets: Vec::new(),
                edits: None,
            },
            textures: Vec::new(),
            watch: None,
            players: Some(1),
            ticks: None,
            record: Some(record.clone()),
            fullscreen: false,
            vsync: true,
            stats: true,
            threads: None,
            debug_layers: Vec::new(),
            size: (64, 64),
            title: "test".to_string(),
        };
        let runner = Runner::load(swarm::app, &options.files, None, Vec::new()).unwrap();
        let mut out = Vec::new();
        let mut session = Session::new(runner, options, &mut out);
        session.start(automatic_claims(1)).unwrap();
        for _ in 0..5 {
            session
                .runner
                .frame(1.0 / 60.0 + 1e-9, &LiveDevices::default())
                .unwrap();
        }
        let failure = RuntimeError::Game {
            tick: 5,
            message: "boom".to_string(),
        };
        session.failure = Some(failure.clone());
        assert_eq!(session.finish(Ok(())), Err(failure));
        let replay = Replay::decode(&std::fs::read(&record).unwrap()).unwrap();
        assert_eq!(replay.hashes.len(), 6, "ticks 0 to 5");
    }

    #[test]
    fn the_keys_that_start_the_match_reach_the_game_only_once_pressed_again() {
        let options = PlayOptions {
            files: GameFiles {
                scene: "Scene(version: 2, next_id: 0, resources: {}, entities: [])".to_string(),
                assets: Vec::new(),
                edits: None,
            },
            textures: Vec::new(),
            watch: None,
            players: None,
            ticks: None,
            record: None,
            fullscreen: false,
            vsync: true,
            stats: false,
            threads: None,
            debug_layers: Vec::new(),
            size: (64, 64),
            title: "test".to_string(),
        };
        let runner = Runner::load(swarm::app, &options.files, None, Vec::new()).unwrap();
        let mut out = Vec::new();
        let mut session = Session::new(runner, options, &mut out);
        let (pressed, released) = (ElementState::Pressed, ElementState::Released);
        // A claims the left half, Enter starts the match: both are still held.
        session.key(KeyCode::KeyA, pressed, false).unwrap();
        session.key(KeyCode::Enter, pressed, false).unwrap();
        assert!(session.started);
        assert!(session.keys.is_empty(), "{:?}", session.keys);
        // The keyboard repeats them while they stay down: they stay out of the game.
        session.key(KeyCode::KeyA, pressed, true).unwrap();
        session.key(KeyCode::Enter, pressed, true).unwrap();
        assert!(session.keys.is_empty(), "{:?}", session.keys);
        // Released and pressed again, a key reaches the game.
        session.key(KeyCode::Enter, released, false).unwrap();
        session.key(KeyCode::Enter, pressed, false).unwrap();
        session.key(KeyCode::KeyD, pressed, false).unwrap();
        let held: Vec<&str> = session.keys.iter().map(String::as_str).collect();
        assert_eq!(held, ["Enter", "KeyD"]);
    }

    #[test]
    fn the_display_paces_frames_only_while_the_window_draws() {
        assert_eq!(pace(false, true), Pace::Redraw);
        assert_eq!(pace(true, true), Pace::Timer, "occluded");
        assert_eq!(pace(false, false), Pace::Timer, "no pixels or no texture");
        assert_eq!(pace(true, false), Pace::Timer);
    }
}
