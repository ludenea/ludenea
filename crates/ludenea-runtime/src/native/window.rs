//! `play`: a game in a native window. The window opens on the lobby, where local players claim
//! their devices (unless the player count is given), then runs the match through the
//! `Runner`: each displayed frame reads the keyboard and the gamepads, lets the host run the
//! ticks due, and draws the newest two ticks blended at the display's rate. Online, the lobby
//! readies the room instead, and each frame carries the match over its link
//! (`OnlineMatch::before_frame`, the runner's frame, `OnlineMatch::after_frame`).

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
use ludenea_app::project::{ProjectTexture, ProjectTextures};
use ludenea_control::{ConsoleLevel, MatchEnd, MatchEndKind};
use ludenea_draw::frame::RenderFrame;
use ludenea_net::{END_LINGER_MICROS, RoomView, Timeouts};
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
use crate::online::{
    OnlineMatch, OnlineOptions, OnlinePhase, OnlineRole, entropy, open_link, write_desync_snapshot,
};
use crate::stats::FrameStats;
use crate::{GameFiles, Runner, RuntimeError};

/// How long the loop waits between frames while the window cannot draw (minimized or
/// hidden), when the display no longer paces it: the simulation keeps its pace meanwhile.
const HIDDEN_FRAME: Duration = Duration::from_millis(4);

/// The time given to the frames of a closed window's match beyond the time it takes to end:
/// the silence that disconnects a peer, then the linger. A match that has not ended by then
/// fails the session.
const CLOSE_MARGIN: Duration = Duration::from_secs(1);

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
    /// Ends the match at the first frame that reaches this tick. Online, the window leaves the
    /// match there, and closes once the match ended and the others were told.
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
    /// Hosts or joins an online match instead of playing locally.
    pub online: Option<OnlineOptions>,
    /// Where an online match that desyncs writes this peer's snapshot of the divergent tick.
    pub dump_dir: PathBuf,
    /// The game's command line name, as the desync message names its `diff`.
    pub game: String,
}

/// How a played session ended.
#[derive(Clone, Debug, PartialEq)]
pub struct PlayOutcome {
    /// The tick the match reached.
    pub ticks: u64,
    /// The frame time summary (`FrameStats::summary`), when `PlayOptions::stats` asked.
    pub stats: Option<String>,
    /// How the online match ended, `None` when none was played.
    pub ended: Option<MatchEnd>,
    /// This peer's number in the online match, once the room laid it out.
    pub local: Option<u8>,
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
///
/// Online (`options.online`), the window opens on the room: a host prints its code to `out`
/// once the room exists, Enter (or `options.players`) readies the room, and the match starts
/// once the room agreed. The match goes on while its window shows it; closing the window
/// leaves it, then plays on without the window until it ended and the others were told, so
/// they end it as left and the replay is complete. A desync writes this peer's snapshot of
/// the divergent tick into `options.dump_dir`. The room's dropped messages and the link's
/// warnings go to `out`.
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
    let online = match &options.online {
        Some(online) => {
            let plan = online.plan(entropy())?;
            Some(OnlineMatch::new(open_link(plan.link.clone()), &plan)?)
        }
        None => None,
    };
    // Online, stopping the host would stop its packets too: the window leaves at that tick.
    if let Some(ticks) = options.ticks
        && online.is_none()
    {
        runner.stop_at(ticks);
    }
    let mut session = Session::new(runner, options, online, out);
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

/// The online match of a window, and what the window knows of it.
struct Online {
    matched: OnlineMatch,
    /// This peer hosts the room: it prints the code.
    hosting: bool,
    /// The signaling listener's URL, which a host prints with its code.
    signaling: String,
    /// What the room waits for, until the match starts.
    view: Option<RoomView>,
    /// How the match ended, once it did.
    end: Option<MatchEnd>,
    /// The window left the match at `PlayOptions::ticks`.
    left: bool,
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
    /// `None` when playing locally.
    online: Option<Online>,
    /// The window's title, kept while there is no window too.
    title: String,
    /// The origin of the online match's clock.
    epoch: Instant,
    /// The online match's clock, in microseconds since `epoch`; it never goes back.
    micros: u64,
    /// The window closed during the online match, which plays on without devices.
    closing: bool,
}

impl<'out> Session<'out> {
    fn new(
        runner: Runner,
        options: PlayOptions,
        online: Option<OnlineMatch>,
        out: &'out mut dyn Write,
    ) -> Session<'out> {
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
        let online = online.map(|matched| Online {
            matched,
            hosting: options
                .online
                .as_ref()
                .is_some_and(|online| matches!(online.role, OnlineRole::Host { .. })),
            signaling: options
                .online
                .as_ref()
                .map(|online| online.signaling.clone())
                .unwrap_or_default(),
            view: None,
            end: None,
            left: false,
        });
        let (watch, watch_warning) = match watch {
            Some(Ok(watch)) => (Some(watch), None),
            Some(Err(error)) => (None, Some(format!("{error}; textures are not reloaded"))),
            None => (None, None),
        };
        let mut session = Session {
            out,
            runner,
            lobby: options.players.is_none().then(Lobby::new),
            title: options.title.clone(),
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
            online,
            epoch: now(),
            micros: 0,
            closing: false,
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
        let Some(polled) = watch.poll() else {
            return;
        };
        for warning in reload(polled, |update| display.replace_textures(update)) {
            self.warn(&warning);
        }
    }

    /// Writes the replay, then gives the session's outcome: the first failure, if any. An
    /// online match is first left and played to its end; a desync then writes this peer's
    /// snapshot.
    fn finish(mut self, ran: Result<(), RuntimeError>) -> Result<PlayOutcome, RuntimeError> {
        self.leave_online();
        if let Err(error) = self.pump_online() {
            self.failure.get_or_insert(error);
        }
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
        let end = self.online.as_ref().and_then(|online| online.end.clone());
        let local = self
            .online
            .as_ref()
            .and_then(|online| online.matched.layout())
            .map(|layout| layout.local.0);
        if let (Some(end), Some(local)) = (&end, local)
            && end.kind == MatchEndKind::Desync
        {
            let written =
                write_desync_snapshot(&mut self.runner, end, local, &self.options.dump_dir);
            self.print_messages();
            match (written, &result) {
                (Ok(path), _) => {
                    let _ = writeln!(
                        self.out,
                        "wrote {}: compare it with the other peer's snapshot using `{} diff`",
                        path.display(),
                        self.options.game
                    );
                }
                (Err(error), Ok(())) => result = Err(error),
                (Err(error), Err(_)) => {
                    let _ = writeln!(self.out, "error: {error}");
                }
            }
        }
        result.map(|()| PlayOutcome {
            ticks: self.runner.tick(),
            stats: self.options.stats.then(|| self.stats.summary()),
            ended: end,
            local,
        })
    }

    /// Leaves the online match the window closes on, if one was joined: the window and its
    /// display go, and the match plays on without devices.
    fn leave_online(&mut self) {
        let Some(online) = &mut self.online else {
            return;
        };
        if !self.started {
            return;
        }
        self.closing = true;
        self.keys.clear();
        self.display = None;
        self.window = None;
        online.matched.leave(&mut self.runner);
    }

    /// Plays the frames of a left online match until it ended and the others were told, so
    /// they end it at the same tick and the replay is complete. That takes at most the
    /// silence that disconnects a peer, then the linger; past that and `CLOSE_MARGIN`, it is
    /// an `Online` error.
    fn pump_online(&mut self) -> Result<(), RuntimeError> {
        if self.online.is_none() || !self.started {
            return Ok(());
        }
        let bound =
            Duration::from_micros(Timeouts::default().disconnect_after_micros + END_LINGER_MICROS)
                + CLOSE_MARGIN;
        let deadline = now() + bound;
        while !self.over() {
            if now() >= deadline {
                return Err(RuntimeError::Online(format!(
                    "the match did not end within {} s of leaving it",
                    bound.as_secs_f64()
                )));
            }
            std::thread::sleep(HIDDEN_FRAME);
            self.frame_at(now())?;
        }
        Ok(())
    }

    /// The online match ended and is done telling the others why.
    fn over(&self) -> bool {
        self.online
            .as_ref()
            .is_some_and(|online| online.end.is_some() && !online.matched.lingering(self.micros))
    }

    /// Sets the window's title, when it changes.
    fn set_title(&mut self, title: String) {
        if title == self.title {
            return;
        }
        if let Some(window) = &self.window {
            window.set_title(&title);
        }
        self.title = title;
    }

    /// The title of the online match: the room, the match, or how it ended. `None` while
    /// playing locally, or before the room said what it waits for.
    fn online_title(&self) -> Option<String> {
        let online = self.online.as_ref()?;
        let view = match &online.end {
            Some(end) => lobby::OnlineView::Ended(&end.message),
            None if self.started => {
                let layout = online.matched.layout()?;
                lobby::OnlineView::Playing {
                    peer: layout.local.0,
                    peers: layout.links.len(),
                    silent: self.runner.silent_peers(),
                }
            }
            None => lobby::OnlineView::Room(online.view.as_ref()?),
        };
        Some(lobby::room_title(
            &self.options.title,
            online.matched.code(),
            view,
        ))
    }

    /// Polls the online match before the runner's frame: the room, the runner joining once
    /// it agreed, and the packets received. A host prints its code once the room exists; the
    /// match's warnings go to `out`. A room that fails is an `Online` error.
    fn poll_online(&mut self) -> Result<(), RuntimeError> {
        let Some(online) = &mut self.online else {
            return Ok(());
        };
        let phase = online.matched.before_frame(&mut self.runner, self.micros);
        let messages = online.matched.messages();
        let mut code = None;
        let mut begin = false;
        let mut failed = None;
        match phase {
            OnlinePhase::Room(view) => online.view = Some(view),
            OnlinePhase::Assigned => {
                if online.hosting {
                    code = Some(format!(
                        "room {}: give this code to the other players; signaling at {}",
                        online.matched.code(),
                        online.signaling
                    ));
                }
            }
            OnlinePhase::Playing => begin = !self.started,
            OnlinePhase::Ended(end) => {
                online.end = Some(end);
                begin = !self.started;
            }
            OnlinePhase::Failed(reason) => failed = Some(reason),
        }
        self.print_messages();
        for message in messages {
            self.warn(&message);
        }
        if let Some(code) = code {
            // Best effort, as warnings are; flushed so a script waiting for it sees it.
            let _ = writeln!(self.out, "{code}");
            let _ = self.out.flush();
        }
        if let Some(reason) = failed {
            return Err(RuntimeError::Online(reason));
        }
        if begin {
            self.begin();
        }
        Ok(())
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
            Some(lobby) => {
                let title = lobby::title(&self.options.title, lobby);
                window.set_title(&title);
                self.title = title;
            }
            None => {
                let players = self.options.players.map_or(1, usize::from);
                self.start(automatic_claims(players))?;
            }
        }
        Ok(())
    }

    /// Starts the match with `claims`, or online, readies the room with them: the match
    /// starts once the room agreed.
    fn start(&mut self, claims: Vec<Claim>) -> Result<(), RuntimeError> {
        if let Some(online) = &mut self.online {
            return online.matched.ready(claims);
        }
        let started = self.runner.start(claims);
        self.print_messages();
        started?;
        self.begin();
        self.set_title(self.options.title.clone());
        Ok(())
    }

    /// The match's first frame is fetched: the keys and buttons held stay out of the game
    /// until pressed again, and the display waits for that frame.
    fn begin(&mut self) {
        self.started = true;
        self.held_at_start = std::mem::take(&mut self.keys);
        self.buttons_held_at_start = gamepads::held(&self.gamepads());
        self.fresh = true;
        self.last = None;
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
                let title = lobby::title(&self.options.title, lobby);
                if self.online_title().is_none() {
                    self.set_title(title);
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
    /// whether the window is done: the match reached its last tick, or online, it ended and
    /// the others were told after the window left at that tick or after this peer's failure.
    fn frame(&mut self) -> Result<bool, RuntimeError> {
        self.frame_at(now())
    }

    /// The frame of [`Session::frame`] at `now`.
    fn frame_at(&mut self, now: Instant) -> Result<bool, RuntimeError> {
        let elapsed = self
            .last
            .replace(now)
            .map_or(0.0, |last| now.duration_since(last).as_secs_f64());
        let since = u64::try_from(now.saturating_duration_since(self.epoch).as_micros())
            .unwrap_or(u64::MAX);
        self.micros = self.micros.max(since);
        self.poll_gamepads()?;
        self.reload_textures();
        self.poll_online()?;
        if !self.started {
            if let Some(title) = self.online_title() {
                self.set_title(title);
            }
            return self.show_lobby().map(|()| false);
        }
        let devices = if self.closing {
            LiveDevices::default()
        } else {
            let mut pads = self.gamepads();
            gamepads::mask_held(&mut pads, &mut self.buttons_held_at_start);
            LiveDevices {
                keys: self.keys.clone(),
                gamepads: pads,
            }
        };
        let update = self.runner.frame(elapsed, &devices);
        self.print_messages();
        let update = match update {
            Ok(update) => Some(update),
            // This peer's game failed, which ends the match: the others are told while it
            // lingers, then the window closes.
            Err(error) if self.online.is_some() => {
                self.failure.get_or_insert(error);
                None
            }
            Err(error) => return Err(error),
        };
        let done = match &mut self.online {
            None => update
                .as_ref()
                .is_some_and(|update| self.options.ticks.is_some_and(|ticks| update.tick >= ticks)),
            Some(online) => {
                if let Some(ticks) = self.options.ticks
                    && !online.left
                    && online.end.is_none()
                    && self.runner.tick() >= ticks
                {
                    online.left = true;
                    online.matched.leave(&mut self.runner);
                }
                online.matched.after_frame(&mut self.runner, self.micros);
                (self.options.ticks.is_some() || self.failure.is_some()) && self.over()
            }
        };
        if let Some(title) = self.online_title() {
            self.set_title(title);
        }
        // After the end the window keeps the match's last frame, unblended.
        let ended = self
            .online
            .as_ref()
            .is_some_and(|online| online.end.is_some());
        if let (Some(update), Some(display), (previous, Some(current))) =
            (update, &mut self.display, self.runner.frames())
        {
            let new_frames = update.new_frames || self.fresh;
            let alpha = if ended { 1.0 } else { update.alpha };
            let presented = display.show(previous, current, &self.overlay, new_frames, alpha)?;
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
        Ok(done)
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

/// The warnings of a texture reload: `replace`'s for a reading of the textures, else the
/// reading's failure, the textures drawn staying.
fn reload(
    polled: Result<ProjectTextures, String>,
    replace: impl FnOnce(&ProjectTextures) -> Vec<String>,
) -> Vec<String> {
    match polled {
        Ok(update) => replace(&update),
        Err(error) => vec![format!("{error}; the textures drawn stay")],
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
    use ludenea_app::project::project_assets;
    use ludenea_app::replay::{Replay, ReplayOutcome, play_replay};
    use ludenea_control::pacer::MAX_TICKS_PER_FRAME;
    use ludenea_control::{MatchEndKind, OnlineState};
    use ludenea_net::{LinkConditions, MemoryMesh};

    use super::*;
    use crate::online::{OnlineMatch, OnlineOptions, OnlineRole};

    /// The options of a window on an empty scene of swarm, without a lobby.
    fn test_options() -> PlayOptions {
        PlayOptions {
            files: GameFiles {
                scene: "Scene(version: 2, next_id: 0, resources: {}, entities: [])".to_string(),
                assets: Vec::new(),
                edits: None,
            },
            textures: Vec::new(),
            watch: None,
            players: Some(1),
            ticks: None,
            record: None,
            fullscreen: false,
            vsync: true,
            stats: false,
            threads: None,
            debug_layers: Vec::new(),
            size: (64, 64),
            title: "test".to_string(),
            online: None,
            dump_dir: PathBuf::from("dump"),
            game: "test".to_string(),
        }
    }

    #[test]
    fn a_failed_match_still_writes_its_replay() {
        let folder = tempfile::tempdir().unwrap();
        let record = folder.path().join("nested/match.replay");
        let options = PlayOptions {
            record: Some(record.clone()),
            stats: true,
            ..test_options()
        };
        let runner = Runner::load(swarm::app, &options.files, None, Vec::new()).unwrap();
        let mut out = Vec::new();
        let mut session = Session::new(runner, options, None, &mut out);
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
            players: None,
            ..test_options()
        };
        let runner = Runner::load(swarm::app, &options.files, None, Vec::new()).unwrap();
        let mut out = Vec::new();
        let mut session = Session::new(runner, options, None, &mut out);
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
    fn a_failed_reading_of_the_textures_is_a_warning() {
        let warnings = reload(Err("two textures share 1".to_string()), |_| {
            panic!("nothing to replace")
        });
        assert_eq!(warnings, ["two textures share 1; the textures drawn stay"]);
        let update = ProjectTextures::default();
        let warnings = reload(Ok(update.clone()), |given| {
            assert_eq!(given, &update);
            vec!["replaced".to_string()]
        });
        assert_eq!(warnings, ["replaced"]);
    }

    #[test]
    fn a_watcher_that_cannot_start_is_a_warning() {
        let options = PlayOptions {
            // An empty path has no folder to watch.
            watch: Some((PathBuf::new(), None)),
            ..test_options()
        };
        let runner = Runner::load(swarm::app, &options.files, None, Vec::new()).unwrap();
        let mut out = Vec::new();
        let session = Session::new(runner, options, None, &mut out);
        assert!(session.watch.is_none());
        drop(session);
        let out = String::from_utf8(out).unwrap();
        let warning = out
            .lines()
            .find(|line| line.contains("textures"))
            .unwrap_or_else(|| panic!("no warning in {out}"));
        assert!(
            warning.starts_with("warning: cannot watch the textures: cannot locate "),
            "{warning}"
        );
        assert!(
            warning.ends_with("; textures are not reloaded"),
            "{warning}"
        );
    }

    #[test]
    fn the_display_paces_frames_only_while_the_window_draws() {
        assert_eq!(pace(false, true), Pace::Redraw);
        assert_eq!(pace(true, true), Pace::Timer, "occluded");
        assert_eq!(pace(false, false), Pace::Timer, "no pixels or no texture");
        assert_eq!(pace(true, false), Pace::Timer);
    }

    /// The arena with a spawn point for each of four players.
    const ARENA: &str = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../examples/arena/tests/fixtures/arena-four.scene.ron"
    );
    const SIGNALING: &str = "ws://127.0.0.1:3536/rooms";
    const FRAME: Duration = Duration::from_micros(1_000_000 / 120);
    /// The most frames a test waits for something before it fails.
    const PATIENCE: u32 = 6_000;

    fn arena_files() -> GameFiles {
        let assets = project_assets(std::path::Path::new(ARENA), None).unwrap();
        GameFiles {
            scene: std::fs::read_to_string(ARENA).unwrap(),
            assets: assets.texts.into_iter().collect(),
            edits: None,
        }
    }

    /// A window session of `app` playing `options.files` online over `mesh`, as `role`, with
    /// `options`, and no window.
    fn online_session<'out>(
        out: &'out mut Vec<u8>,
        mesh: &MemoryMesh,
        role: OnlineRole,
        app: fn() -> App,
        options: PlayOptions,
    ) -> Session<'out> {
        let online = OnlineOptions {
            role,
            signaling: SIGNALING.to_string(),
            stun: Vec::new(),
            conditions: LinkConditions {
                latency_micros: 0,
                jitter_micros: 0,
                loss_per_mille: 0,
                duplicate_per_mille: 0,
                reorder_per_mille: 0,
                reorder_extra_micros: 0,
                seed: 0,
            },
        };
        let plan = online
            .plan(0x0123_4567_89ab_cdef_fedc_ba98_7654_3210)
            .unwrap();
        let matched = OnlineMatch::new(Box::new(mesh.join()), &plan).unwrap();
        let options = PlayOptions {
            threads: Some(1),
            online: Some(online),
            ..options
        };
        let runner = Runner::load(app, &options.files, Some(1), Vec::new()).unwrap();
        Session::new(runner, options, Some(matched), out)
    }

    /// A hosting session of `host_app` and a joining one of `join_app` in the same room, with
    /// `host` and `join` options.
    fn pair_of<'out>(
        host_app: fn() -> App,
        join_app: fn() -> App,
        outs: (&'out mut Vec<u8>, &'out mut Vec<u8>),
        host: PlayOptions,
        join: PlayOptions,
    ) -> [Session<'out>; 2] {
        let mesh = MemoryMesh::new();
        let host = online_session(
            outs.0,
            &mesh,
            OnlineRole::Host {
                peers: 2,
                code: None,
            },
            host_app,
            host,
        );
        let code = host.online.as_ref().unwrap().matched.code().clone();
        let joiner = online_session(outs.1, &mesh, OnlineRole::Join { code }, join_app, join);
        [host, joiner]
    }

    /// A hosting session and a joining one of the arena in the same room, with `host` and
    /// `join` options.
    fn pair<'out>(
        outs: (&'out mut Vec<u8>, &'out mut Vec<u8>),
        host: PlayOptions,
        join: PlayOptions,
    ) -> [Session<'out>; 2] {
        let arena = |options| PlayOptions {
            files: arena_files(),
            ..options
        };
        pair_of(arena::app, arena::app, outs, arena(host), arena(join))
    }

    /// Readies both sessions with one player, as a window does at launch with `--players 1`.
    fn ready(sessions: &mut [Session<'_>]) {
        for session in sessions {
            session.start(automatic_claims(1)).unwrap();
        }
    }

    /// Frames every session one frame later than `at`, until `done`, failing after
    /// [`PATIENCE`] frames.
    fn run_until(
        sessions: &mut [Session<'_>],
        at: &mut Instant,
        what: &str,
        done: impl Fn(&[Session<'_>]) -> bool,
    ) {
        for _ in 0..PATIENCE {
            if done(sessions) {
                return;
            }
            *at += FRAME;
            for session in sessions.iter_mut() {
                session.frame_at(*at).unwrap();
            }
        }
        panic!("{what}: still waiting after {PATIENCE} frames");
    }

    fn end(session: &Session<'_>) -> Option<MatchEnd> {
        session
            .online
            .as_ref()
            .and_then(|online| online.end.clone())
    }

    #[test]
    fn the_online_lobby_readies_the_room_then_plays() {
        let (mut host_out, mut join_out) = (Vec::new(), Vec::new());
        let lobby = PlayOptions {
            players: None,
            ..test_options()
        };
        let mut sessions = pair((&mut host_out, &mut join_out), lobby.clone(), lobby);
        let code = sessions[0].online.as_ref().unwrap().matched.code().clone();
        let (pressed, released) = (ElementState::Pressed, ElementState::Released);
        // The joiner claims the left half and readies the room with Enter.
        let joiner = &mut sessions[1];
        joiner.key(KeyCode::KeyA, pressed, false).unwrap();
        joiner.key(KeyCode::Enter, pressed, false).unwrap();
        joiner.key(KeyCode::Enter, released, false).unwrap();
        assert!(!joiner.started, "Enter readies the room, the match waits");
        // The host has not pressed Enter: the room waits.
        let mut at = now();
        for _ in 0..240 {
            at += FRAME;
            for session in &mut sessions {
                session.frame_at(at).unwrap();
            }
        }
        assert!(sessions.iter().all(|session| !session.started));
        assert!(
            sessions[0]
                .title
                .starts_with(&format!("test: room {code}, ")),
            "{}",
            sessions[0].title
        );
        let host = &mut sessions[0];
        host.key(KeyCode::ArrowLeft, pressed, false).unwrap();
        host.key(KeyCode::Enter, pressed, false).unwrap();
        run_until(&mut sessions, &mut at, "both play", |sessions| {
            sessions.iter().all(|session| session.started)
        });
        for session in &sessions {
            assert!(session.runner.frames().1.is_some(), "the first frame");
            assert!(
                session.keys.is_empty(),
                "the lobby's keys stay out of the game"
            );
        }
        run_until(&mut sessions, &mut at, "both reach tick 60", |sessions| {
            sessions.iter().all(|session| session.runner.tick() >= 60)
        });
        for session in &sessions {
            assert_eq!(session.runner.online(), Some(&OnlineState::Running));
        }
        drop(sessions);
        let host_out = String::from_utf8(host_out).unwrap();
        assert_eq!(
            host_out,
            format!("room {code}: give this code to the other players; signaling at {SIGNALING}\n")
        );
        assert_eq!(String::from_utf8(join_out).unwrap(), "");
    }

    #[test]
    fn closing_the_window_leaves_the_match() {
        let folder = tempfile::tempdir().unwrap();
        let records = [
            folder.path().join("host.replay"),
            folder.path().join("join.replay"),
        ];
        let recording = |record: &PathBuf| PlayOptions {
            record: Some(record.clone()),
            ..test_options()
        };
        let (mut host_out, mut join_out) = (Vec::new(), Vec::new());
        let mut sessions = pair(
            (&mut host_out, &mut join_out),
            recording(&records[0]),
            recording(&records[1]),
        );
        ready(&mut sessions);
        let mut at = now();
        run_until(&mut sessions, &mut at, "both reach tick 100", |sessions| {
            sessions.iter().all(|session| session.runner.tick() >= 100)
        });
        // The joiner's window closes: its session leaves, then plays on without devices.
        sessions[1].leave_online();
        assert!(sessions[1].closing);
        run_until(&mut sessions, &mut at, "both end", |sessions| {
            sessions.iter().all(Session::over)
        });
        let ends: Vec<MatchEnd> = sessions
            .iter()
            .map(|session| end(session).unwrap())
            .collect();
        assert_eq!(ends[0], ends[1], "the host ends at the same tick");
        assert_eq!(ends[0].kind, MatchEndKind::Left, "{:?}", ends[0]);
        assert_eq!(ends[0].peer, Some(1));
        let outcomes: Vec<PlayOutcome> = sessions
            .into_iter()
            .map(|session| session.finish(Ok(())).unwrap())
            .collect();
        for outcome in &outcomes {
            assert_eq!(outcome.ended.as_ref(), Some(&ends[0]));
        }
        assert_eq!(outcomes[0].local, Some(0));
        assert_eq!(outcomes[1].local, Some(1));
        let replays = records.map(|record| std::fs::read(record).unwrap());
        assert!(replays[0] == replays[1], "the replays differ");
    }

    #[test]
    fn ticks_online_leaves_at_that_tick() {
        let (mut host_out, mut join_out) = (Vec::new(), Vec::new());
        let mut sessions = pair(
            (&mut host_out, &mut join_out),
            test_options(),
            PlayOptions {
                ticks: Some(120),
                ..test_options()
            },
        );
        ready(&mut sessions);
        let mut at = now();
        let mut left_at = None;
        let mut closed = false;
        for _ in 0..PATIENCE {
            at += FRAME;
            assert!(!sessions[0].frame_at(at).unwrap(), "the host stays open");
            closed = sessions[1].frame_at(at).unwrap();
            let joiner = &sessions[1];
            if left_at.is_none() && joiner.online.as_ref().unwrap().left {
                left_at = Some(joiner.runner.tick());
            }
            assert_eq!(
                closed,
                joiner.over(),
                "closes once ended and done lingering"
            );
            if closed {
                break;
            }
        }
        assert!(closed, "the joiner's window closes");
        let left_at = left_at.expect("the joiner left");
        assert!((120..120 + u64::from(MAX_TICKS_PER_FRAME)).contains(&left_at));
        let ending = end(&sessions[1]).unwrap();
        assert_eq!(ending.kind, MatchEndKind::Left, "{ending:?}");
        assert_eq!(ending.peer, Some(1));
        assert!(ending.tick.unwrap() >= left_at, "{ending:?}");
        assert_eq!(end(&sessions[0]), Some(ending));
    }

    #[test]
    fn the_title_follows_the_room_and_the_match() {
        let (mut host_out, mut join_out) = (Vec::new(), Vec::new());
        let mut sessions = pair(
            (&mut host_out, &mut join_out),
            test_options(),
            test_options(),
        );
        let code = sessions[0].online.as_ref().unwrap().matched.code().clone();
        // Only the host is ready: the room waits for the joiner.
        ready(&mut sessions[..1]);
        let mut at = now();
        run_until(
            &mut sessions,
            &mut at,
            "the host shows the room",
            |sessions| sessions[0].title.contains(" not ready"),
        );
        let view = sessions[0].online.as_ref().unwrap().view.unwrap();
        assert_eq!(
            sessions[0].title,
            format!(
                "test: room {code}, {}, press Enter when your players have joined",
                view.describe()
            )
        );
        ready(&mut sessions[1..]);
        run_until(&mut sessions, &mut at, "both play", |sessions| {
            sessions.iter().all(|session| session.started)
        });
        assert_eq!(sessions[0].title, "test: online, peer 0 of 2");
        assert_eq!(sessions[1].title, "test: online, peer 1 of 2");
        // The joiner stops answering: the host names it silent.
        run_until(
            &mut sessions[..1],
            &mut at,
            "the joiner is silent",
            |sessions| sessions[0].title.contains(", peer 1 silent for "),
        );
        assert!(
            sessions[0]
                .title
                .starts_with("test: online, peer 0 of 2, peer 1 silent for "),
            "{}",
            sessions[0].title
        );
        run_until(&mut sessions, &mut at, "the joiner is back", |sessions| {
            sessions[0].title == "test: online, peer 0 of 2"
        });
        sessions[1].leave_online();
        run_until(&mut sessions, &mut at, "both end", |sessions| {
            sessions.iter().all(Session::over)
        });
        let message = end(&sessions[0]).unwrap().message;
        assert_eq!(sessions[0].title, format!("test: match ended: {message}"));
    }

    #[test]
    fn an_online_match_writes_its_replay_after_the_end() {
        let folder = tempfile::tempdir().unwrap();
        let records = [
            folder.path().join("host.replay"),
            folder.path().join("join.replay"),
        ];
        let (mut host_out, mut join_out) = (Vec::new(), Vec::new());
        let mut sessions = pair(
            (&mut host_out, &mut join_out),
            PlayOptions {
                record: Some(records[0].clone()),
                ..test_options()
            },
            PlayOptions {
                record: Some(records[1].clone()),
                ticks: Some(90),
                ..test_options()
            },
        );
        ready(&mut sessions);
        let mut at = now();
        run_until(&mut sessions, &mut at, "both end", |sessions| {
            sessions.iter().all(Session::over)
        });
        let last = end(&sessions[0]).unwrap().tick.unwrap();
        for session in sessions {
            session.finish(Ok(())).unwrap();
        }
        let replays = records.map(|record| std::fs::read(record).unwrap());
        assert!(replays[0] == replays[1], "the replays differ");
        let replay = Replay::decode(&replays[0]).unwrap();
        let mut sim = arena::app().build().unwrap();
        let report = play_replay(&mut sim, &replay, |_, _| {}).unwrap();
        assert_eq!(report.outcome, ReplayOutcome::Matched);
        assert_eq!(replay.hashes.len() as u64, last + 2, "ticks 0 to the end's");
    }

    /// A component both desync apps register; only their systems differ.
    #[derive(Clone, ludenea::prelude::Component, serde::Serialize, serde::Deserialize)]
    #[component(name = "window::Mover")]
    struct Mover {
        x: i64,
    }

    fn walk(mut movers: ludenea::prelude::Query<&mut Mover>) {
        for mover in movers.iter_mut() {
            mover.x += 1;
        }
    }

    /// From tick 50, moves twice as far.
    fn stride(
        time: ludenea::prelude::Res<ludenea::prelude::Time>,
        mut movers: ludenea::prelude::Query<&mut Mover>,
    ) {
        let step = if time.tick >= 50 { 2 } else { 1 };
        for mover in movers.iter_mut() {
            mover.x += step;
        }
    }

    fn walking_app() -> App {
        let mut app = App::new();
        app.input::<u8>()
            .input_mapping(|_, _| 0u8)
            .component::<Mover>()
            .system(ludenea::prelude::Sim, walk);
        app
    }

    fn striding_app() -> App {
        let mut app = App::new();
        app.input::<u8>()
            .input_mapping(|_, _| 0u8)
            .component::<Mover>()
            .system(ludenea::prelude::Sim, stride);
        app
    }

    #[test]
    fn a_desync_writes_this_peers_snapshot_and_names_the_diff() {
        let dump = tempfile::tempdir().unwrap();
        let options = PlayOptions {
            files: GameFiles {
                scene:
                    r#"Scene(version: 1, resources: {}, entities: [{ "window::Mover": (x: 0) }])"#
                        .to_string(),
                assets: Vec::new(),
                edits: None,
            },
            dump_dir: dump.path().to_path_buf(),
            game: "walker".to_string(),
            ..test_options()
        };
        let (mut host_out, mut join_out) = (Vec::new(), Vec::new());
        let mut sessions = pair_of(
            walking_app,
            striding_app,
            (&mut host_out, &mut join_out),
            options.clone(),
            options,
        );
        ready(&mut sessions);
        let mut at = now();
        run_until(&mut sessions, &mut at, "both end", |sessions| {
            sessions.iter().all(Session::over)
        });
        for session in sessions {
            let outcome = session.finish(Ok(())).unwrap();
            let ended = outcome.ended.unwrap();
            assert_eq!(ended.kind, MatchEndKind::Desync, "{ended:?}");
            assert_eq!(ended.tick, Some(60));
        }
        for (local, out) in [(0, host_out), (1, join_out)] {
            let path = dump
                .path()
                .join(format!("desync-tick-60-peer-{local}.snap"));
            assert!(path.is_file(), "{}", path.display());
            assert_eq!(
                String::from_utf8(out).unwrap().lines().last(),
                Some(
                    format!(
                        "wrote {}: compare it with the other peer's snapshot using `walker diff`",
                        path.display()
                    )
                    .as_str()
                )
            );
        }
    }
}
