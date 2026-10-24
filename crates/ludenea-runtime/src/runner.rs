//! The runner: drives the in-process control host for a game that plays on its own in the
//! native window. It loads the game, starts the match with its players' claims, and per
//! displayed frame sends the live devices, lets the host's pacer run the ticks due, and fetches
//! the render frame of the newest tick. Every step goes through host commands, so a played
//! match records the same session as the editor. An online match goes through the host's
//! commands too: `join_match`, `net_receive`, and the packets each frame keeps for
//! `take_net_packets` (`online::OnlineMatch` carries them over a link). The web page does not
//! use it: its worker sends the same commands to the browser's `WorkerHost`.

use std::collections::BTreeMap;

use ludenea_app::App;
use ludenea_app::players::{Claim, LiveDevices};
use ludenea_control::host::RESIMULATION_BUDGET;
use ludenea_control::pacer::MAX_TICKS_PER_FRAME;
use ludenea_control::{
    Bytes, Command, ConsoleLevel, Document, Event, Host, NetStats, OnlineSettings, OnlineState,
    PauseReason, ProtocolError, Reply,
};
use ludenea_draw::frame::RenderFrame;
use ludenea_ecs::AssetId;

/// The files a game runs from, in file form.
#[derive(Clone, Debug, PartialEq)]
pub struct GameFiles {
    /// The scene text.
    pub scene: String,
    /// The prefab and data asset texts the scene may reach.
    pub assets: Vec<(AssetId, String)>,
    /// An encoded edit log, if any: the game starts from the edited state. Edits at tick 0
    /// are part of the starting state. An edit recorded at a later tick applies only while
    /// the match follows the recorded future: live input that differs from it (any held
    /// device, while no inputs are recorded) at a tick before the edit takes over and forks a
    /// branch there, as in the editor, and the new branch holds none of the edits after that
    /// tick. `Runner::export_replay` exports the branch that was played, so its replay then
    /// leaves those edits out.
    pub edits: Option<Vec<u8>>,
}

/// What one displayed frame did.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FrameUpdate {
    /// Whether ticks ran: `Runner::frames` holds new frames to upload.
    pub new_frames: bool,
    /// The fraction of the next tick that passed, from 0 to 1 (the host's progress is below
    /// 1, but it can round up to exactly 1 as an `f32`): what the renderer interpolates the
    /// last two ticks with.
    pub alpha: f32,
    /// The tick of the current state.
    pub tick: u64,
}

/// Why a game cannot go on running.
#[derive(Clone, Debug, PartialEq, thiserror::Error)]
pub enum RuntimeError {
    /// The game cannot be built, its files do not load, or its match cannot start (such as
    /// claims that hold a device twice).
    #[error("cannot load the game: {0}")]
    Load(String),
    /// The game failed or panicked while it ran `tick`.
    #[error("the game failed at tick {tick}: {message}")]
    Game { tick: u64, message: String },
    /// The GPU cannot draw, with its cause.
    #[error("the GPU failed: {0}")]
    Gpu(String),
    /// The window cannot open or keep running, with its cause.
    #[error("the window failed: {0}")]
    Window(String),
    /// The replay of the match cannot be exported or written, with its cause.
    #[error("cannot record the replay: {0}")]
    Record(String),
    /// The project's textures cannot be watched for changes, with the cause.
    #[error("cannot watch the textures: {0}")]
    Watch(String),
    /// The online match cannot start or go on: its settings, its room, or a command the host
    /// refused, with the cause.
    #[error("the online match cannot go on: {0}")]
    Online(String),
}

/// A game played through its control host.
pub struct Runner {
    host: Host,
    debug_layers: Vec<String>,
    /// The devices last sent with `SetLiveInput`, `None` before the first frame.
    sent: Option<LiveDevices>,
    previous: Option<RenderFrame>,
    current: Option<RenderFrame>,
    /// What the host wrote to its console since the last `take_messages`.
    messages: Vec<(ConsoleLevel, String)>,
    /// The tick play stops at, if any.
    stop_at: Option<u64>,
    /// The game's ticks per second.
    tick_rate: u32,
    /// The latest state of the online match, `None` before one is joined.
    online: Option<OnlineState>,
    /// The peers announced silent and not back yet, with the silence announced, in ms.
    silent: BTreeMap<u8, u64>,
    /// The packets the host sent since the last `take_net_packets`, with their peer.
    net_packets: Vec<(u8, Vec<u8>)>,
}

impl Runner {
    /// Builds the game, loads its scene and edits, and sets its thread count (the host's
    /// default when `None`). `debug_layers` are drawn in every render frame.
    pub fn load(
        app: fn() -> App,
        files: &GameFiles,
        threads: Option<usize>,
        debug_layers: Vec<String>,
    ) -> Result<Runner, RuntimeError> {
        let host = Host::new(app).map_err(|error| RuntimeError::Load(error.to_string()))?;
        let tick_rate = host.game_info().tick_rate;
        let mut runner = Runner {
            host,
            debug_layers,
            sent: None,
            previous: None,
            current: None,
            messages: Vec::new(),
            stop_at: None,
            tick_rate,
            online: None,
            silent: BTreeMap::new(),
            net_packets: Vec::new(),
        };
        runner.setup(Command::LoadScene {
            scene: files.scene.clone(),
            assets: files.assets.clone(),
            document: Document::Scene,
        })?;
        if let Some(edits) = &files.edits {
            runner.setup(Command::LoadEdits {
                log: Bytes(edits.clone()),
            })?;
        }
        if let Some(threads) = threads {
            let threads = u32::try_from(threads).map_err(|_| {
                RuntimeError::Load(format!("{threads} threads is more than the game can run"))
            })?;
            runner.setup(Command::SetThreads { threads })?;
        }
        let events = runner.host.drain_events();
        runner.keep_messages(events);
        Ok(runner)
    }

    /// Starts the match with one player per claim, and fetches the frame of its first tick.
    pub fn start(&mut self, claims: Vec<Claim>) -> Result<(), RuntimeError> {
        let players = u16::try_from(claims.len()).map_err(|_| {
            RuntimeError::Load(format!(
                "{} players is more than a game holds",
                claims.len()
            ))
        })?;
        self.setup(Command::SetPlayers {
            players,
            claims: Some(claims),
        })?;
        self.setup(Command::Play)?;
        let frame = self.render_frame().map_err(|error| {
            RuntimeError::Load(format!("cannot draw the first tick: {}", error.message))
        })?;
        self.current = Some(frame);
        self.previous = None;
        let events = self.host.drain_events();
        self.keep_messages(events);
        Ok(())
    }

    /// Joins the online match `settings` describe, `claims` holding the devices of each local
    /// player, and fetches the frame of its first tick. The match then plays as frames pass
    /// time; its packets wait for `take_net_packets`.
    pub fn join_match(
        &mut self,
        settings: OnlineSettings,
        claims: Vec<Claim>,
    ) -> Result<(), RuntimeError> {
        self.host
            .handle(Command::JoinMatch { settings, claims })
            .map_err(|error| RuntimeError::Online(error.message))?;
        self.sent = None;
        let frame = self.render_frame().map_err(|error| {
            RuntimeError::Online(format!("cannot draw the first tick: {}", error.message))
        })?;
        self.current = Some(frame);
        self.previous = None;
        let events = self.host.drain_events();
        self.keep_online(&events);
        self.keep_messages(events);
        Ok(())
    }

    /// Gives the host a packet peer `from` sent. A packet the host cannot take is dropped
    /// there, and counted in its statistics; a refusal goes to the messages.
    pub fn net_receive(&mut self, from: u8, packet: Vec<u8>) {
        let received = self.host.handle(Command::NetReceive {
            from,
            packet: Bytes(packet),
        });
        if let Err(error) = received {
            self.messages.push((
                ConsoleLevel::Error,
                format!("a packet from peer {from} was refused: {}", error.message),
            ));
        }
    }

    /// The packets to send since the last call, oldest first, each with the peer it goes to.
    pub fn take_net_packets(&mut self) -> Vec<(u8, Vec<u8>)> {
        std::mem::take(&mut self.net_packets)
    }

    /// Leaves the online match: every peer ends at the last tick this one has inputs for.
    pub fn leave_match(&mut self) -> Result<(), RuntimeError> {
        self.host
            .handle(Command::LeaveMatch)
            .map(|_| ())
            .map_err(|error| RuntimeError::Online(error.message))?;
        let events = self.host.drain_events();
        self.keep_online(&events);
        self.keep_messages(events);
        Ok(())
    }

    /// The latest state of the online match, `None` before one is joined.
    pub fn online(&self) -> Option<&OnlineState> {
        self.online.as_ref()
    }

    /// The peers announced silent and not back yet, each with the silence the announcement
    /// gave, in milliseconds. Empty once the match ended.
    pub fn silent_peers(&self) -> &BTreeMap<u8, u64> {
        &self.silent
    }

    /// The statistics of the online match, readable after its end too.
    pub fn net_stats(&mut self) -> Result<NetStats, RuntimeError> {
        match self.host.handle(Command::GetNetStats) {
            Ok(Reply::NetStats(stats)) => Ok(stats),
            Ok(other) => unreachable!("GetNetStats answers NetStats, not {other:?}"),
            Err(error) => Err(RuntimeError::Online(error.message)),
        }
    }

    /// Whether the online match ended.
    fn match_ended(&self) -> bool {
        matches!(self.online, Some(OnlineState::Ended { .. }))
    }

    /// Lets `elapsed_seconds` of play pass with `devices` held: the host's pacer runs the
    /// ticks due, at most its cap per frame however long the frame took. A game that fails
    /// or panics is a `Game` error naming the tick. During an online match it keeps the
    /// packets to send and the match's state; the end of the match is not an error. Once the
    /// match ended, `devices` are no longer sent, and the frames stay the last ones fetched
    /// while the match ran, as the session resimulates the match in the background from
    /// tick 0 (which `FrameUpdate::tick` then reports).
    pub fn frame(
        &mut self,
        elapsed_seconds: f64,
        devices: &LiveDevices,
    ) -> Result<FrameUpdate, RuntimeError> {
        if !self.match_ended()
            && let Some(command) = self.live_input(devices)
        {
            self.host
                .handle(command)
                .map_err(|error| self.game_error(error))?;
        }
        let events = self.advance(elapsed_seconds);
        self.keep_online(&events);
        let ticked = events
            .iter()
            .any(|event| matches!(event, Event::Ticked { .. }));
        let failure = events.iter().find_map(|event| match event {
            Event::Paused {
                reason:
                    PauseReason::Failed { tick, message } | PauseReason::Panicked { tick, message },
            } => Some(RuntimeError::Game {
                tick: *tick,
                message: message.clone(),
            }),
            _ => None,
        });
        self.keep_messages(events);
        if let Some(failure) = failure {
            // The frames are not fetched for the ticks that ran before the failure: they are
            // stale by those ticks, which is harmless as the match ends here.
            return Err(failure);
        }
        // Once the match ended, the session shows its resimulation, from tick 0: the frames
        // stay those of the match.
        let ticked = ticked && !self.match_ended();
        if ticked {
            let frame = self
                .render_frame()
                .map_err(|error| self.game_error(error))?;
            self.previous = self.current.replace(frame);
        }
        Ok(FrameUpdate {
            new_frames: ticked,
            alpha: self.host.tick_progress() as f32,
            tick: self.host.tick(),
        })
    }

    /// Stops play at `tick`: no frame runs past it, so a replay exported once it is reached
    /// holds exactly the ticks up to it.
    pub fn stop_at(&mut self, tick: u64) {
        self.stop_at = Some(tick);
    }

    /// Lets the host's pacer run the ticks due after `elapsed_seconds`, never past `stop_at`.
    fn advance(&mut self, elapsed_seconds: f64) -> Vec<Event> {
        let cap = u64::from(MAX_TICKS_PER_FRAME);
        let Some(limit) = self.stop_at else {
            return self.host.advance(elapsed_seconds);
        };
        if self.host.tick() + cap <= limit {
            return self.host.advance(elapsed_seconds);
        }
        // Within a frame's cap of the limit: half a tick of time at a time, so the pacer runs
        // at most one tick per call and stops exactly at `limit`. Time past the cap's worth
        // of slices is dropped, as the pacer drops a stall's debt.
        let half_tick = 0.5 / f64::from(self.tick_rate.max(1));
        let mut events = Vec::new();
        let mut left = elapsed_seconds;
        for _ in 0..2 * cap {
            if self.host.tick() >= limit || left <= 0.0 {
                break;
            }
            let slice = if left < half_tick { left } else { half_tick };
            events.extend(self.host.advance(slice));
            left -= slice;
        }
        events
    }

    /// The frame fetched before the current one, and the current one. After a displayed frame
    /// that ran several ticks the first is more than a tick older: the renderer then draws
    /// the current frame as is (`Renderer::set_frames` blends only consecutive ticks).
    pub fn frames(&self) -> (Option<&RenderFrame>, Option<&RenderFrame>) {
        (self.previous.as_ref(), self.current.as_ref())
    }

    /// The encoded replay of the match up to the current tick, after a failure too. After an
    /// online match, it first waits for the session to resimulate the match.
    pub fn export_replay(&mut self) -> Result<Vec<u8>, RuntimeError> {
        self.settle()?;
        let exported = self.host.handle(Command::ExportReplay { branch: None });
        let events = self.host.drain_events();
        self.keep_online(&events);
        self.keep_messages(events);
        match exported {
            Ok(Reply::Replay { replay }) => Ok(replay.0),
            Ok(other) => unreachable!("ExportReplay answers Replay, not {other:?}"),
            Err(error) => Err(self.game_error(error)),
        }
    }

    /// The snapshot of the session's state at `tick`, which the session seeks to. After an
    /// online match, it first waits for the session to resimulate the match.
    pub fn snapshot_at(&mut self, tick: u64) -> Result<Vec<u8>, RuntimeError> {
        self.settle()?;
        self.host.handle(Command::Seek { tick }).map_err(|error| {
            RuntimeError::Record(format!("cannot seek to tick {tick}: {}", error.message))
        })?;
        let taken = self.host.handle(Command::TakeSnapshot);
        let events = self.host.drain_events();
        self.keep_online(&events);
        self.keep_messages(events);
        match taken {
            Ok(Reply::Snapshot { snapshot, .. }) => Ok(snapshot.0),
            Ok(other) => unreachable!("TakeSnapshot answers Snapshot, not {other:?}"),
            Err(error) => Err(RuntimeError::Record(format!(
                "cannot take the snapshot of tick {tick}: {}",
                error.message
            ))),
        }
    }

    /// Advances by zero while the session resimulates, as after an online match, at most
    /// `ticks / RESIMULATION_BUDGET + 2` times, `ticks` being the session's length: each
    /// advance resimulates up to `RESIMULATION_BUDGET` ticks. Past that bound it is a `Record`
    /// error naming the tick reached. While a match runs the host refuses the timeline, and
    /// the command that follows reports why.
    fn settle(&mut self) -> Result<(), RuntimeError> {
        let mut advances = 0;
        loop {
            let Ok(Reply::Timeline(timeline)) = self.host.handle(Command::GetTimeline) else {
                return Ok(());
            };
            if !timeline.resimulating {
                return Ok(());
            }
            if advances >= timeline.head / u64::from(RESIMULATION_BUDGET) + 2 {
                return Err(RuntimeError::Record(format!(
                    "the session is still resimulating the match at tick {} of {}",
                    timeline.simulated, timeline.head
                )));
            }
            let events = self.host.advance(0.0);
            self.keep_online(&events);
            self.keep_messages(events);
            advances += 1;
        }
    }

    /// The tick of the current state.
    pub fn tick(&self) -> u64 {
        self.host.tick()
    }

    /// What the host wrote to its console since the last call, oldest first: warnings such as
    /// an edit of the loaded log that no longer applies or a tick that diverged from the
    /// recorded session (the host writes one for each), and errors such as a game panic.
    pub fn take_messages(&mut self) -> Vec<(ConsoleLevel, String)> {
        std::mem::take(&mut self.messages)
    }

    /// Keeps the console messages of `events`. The host writes every warning it reports
    /// with an event of its own (`EditDropped`, `Diverged`) to the console too, so those
    /// events add nothing.
    fn keep_messages(&mut self, events: Vec<Event>) {
        self.messages
            .extend(events.into_iter().filter_map(|event| match event {
                Event::Console { level, text } => Some((level, text)),
                _ => None,
            }));
    }

    /// Keeps the online match's packets, state and silences of `events`.
    fn keep_online(&mut self, events: &[Event]) {
        for event in events {
            match event {
                Event::NetSend { packets } => self.net_packets.extend(
                    packets
                        .iter()
                        .map(|packet| (packet.to, packet.packet.0.clone())),
                ),
                Event::Online { state } => {
                    if matches!(state, OnlineState::Ended { .. }) {
                        self.silent.clear();
                    }
                    self.online = Some(state.clone());
                }
                Event::PeerSilent { peer, for_ms } if !self.match_ended() => {
                    self.silent.insert(*peer, *for_ms);
                }
                Event::PeerBack { peer } => {
                    self.silent.remove(peer);
                }
                _ => {}
            }
        }
    }

    /// The `SetLiveInput` command for `devices`, or `None` when they are the ones last sent.
    fn live_input(&mut self, devices: &LiveDevices) -> Option<Command> {
        if self.sent.as_ref() == Some(devices) {
            return None;
        }
        self.sent = Some(devices.clone());
        Some(Command::SetLiveInput {
            devices: devices.clone(),
        })
    }

    fn render_frame(&mut self) -> Result<RenderFrame, ProtocolError> {
        let command = Command::GetRenderFrame {
            debug_layers: self.debug_layers.clone(),
        };
        match self.host.handle(command)? {
            Reply::RenderFrame { frame } => Ok(frame),
            other => unreachable!("GetRenderFrame answers RenderFrame, not {other:?}"),
        }
    }

    /// Sends a command that loads or starts the game; a refusal is a `Load` error.
    fn setup(&mut self, command: Command) -> Result<(), RuntimeError> {
        self.host
            .handle(command)
            .map(|_| ())
            .map_err(|error| RuntimeError::Load(error.message))
    }

    /// A refused command while the game runs, at the current tick.
    fn game_error(&self, error: ProtocolError) -> RuntimeError {
        RuntimeError::Game {
            tick: self.host.tick(),
            message: error.message,
        }
    }
}

#[cfg(test)]
mod tests {
    use ludenea_app::players::{LiveDevices, automatic_claims};

    use super::*;

    #[test]
    fn devices_unchanged_send_no_command() {
        let files = GameFiles {
            scene: "Scene(version: 2, next_id: 0, resources: {}, entities: [])".to_string(),
            assets: Vec::new(),
            edits: None,
        };
        let mut runner = Runner::load(swarm::app, &files, None, Vec::new()).unwrap();
        runner.start(automatic_claims(1)).unwrap();
        let held = LiveDevices {
            keys: ["KeyA".to_string()].into(),
            gamepads: Vec::new(),
        };
        assert!(
            runner.live_input(&LiveDevices::default()).is_some(),
            "the first frame sends"
        );
        assert!(runner.live_input(&LiveDevices::default()).is_none());
        assert!(runner.live_input(&held).is_some());
        assert!(runner.live_input(&held).is_none());
        runner.frame(1.0 / 60.0, &held).unwrap();
        assert!(
            runner.live_input(&held).is_none(),
            "a frame remembers what it sent"
        );
    }
}
