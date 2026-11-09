//! The runs the spike measures. Throwaway, never product code.
//!
//! Every run hosts a fresh world behind the local transports, drives it with
//! the scripted agent in batches of ten ticks (or lets a paced host step it),
//! and returns what the agent observed: the hashes the steps announced, what
//! its reader thread received, the commands it sent with their results. The
//! sequential oracle is then replayed from the agent's own record of what it
//! sent and of the tick boundary each command was applied at.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use reference_workload::fnv1a;

use crate::agent::{CallError, Client, Fallible, ReaderControl, ReaderReport, StreamView, spawn_reader};
use crate::commands::{Digests, Element, Handle, Oracle, Outcome, Scratch, Transaction, Verdict, read_canonical, transact, write_canonical};
use crate::data_plane::{BUDGET_EXCEEDED, FRAME_STREAM, PlaneConfig, Policy, STATE_STREAM};
use crate::script::{LabelBook, MixKind, SIZES, WATCHED, build, first_labels, restores_over_a_reused_slot, slots_for_spawns, with_one_refused, with_several_refused};
use crate::surface::{CommandReply, Profile, Server, ServerConfig, Statistics};
use crate::transport::{Address, Listening, serve_all};
use crate::world::{Size, World, WorldConfig};

pub const BATCH: u32 = 10;
pub const PACE: Duration = Duration::from_millis(1);

#[derive(Clone, Copy, Debug)]
pub struct Scale {
    pub size: Size,
    pub ticks: u32,
    pub plane: PlaneConfig,
    pub log_retention: u32,
    /// The tick a stalled client stops reading at.
    pub stall_at: u32,
    pub socket_reads: u32,
    pub repetitions: usize,
    /// Whether the stepped client of a lossless ring waits for its reader
    /// between two steps, as the client of a lossless second stream always
    /// does. Off at full scale, where the reader keeps up with the world; on
    /// where ticks are shorter than the reader's polling period, since a
    /// client that outruns its stream is ended at the budget by design.
    pub ring_clients_wait: bool,
}

impl Scale {
    fn paced(&self, spec: PlaneSpec) -> PlaneSpec {
        PlaneSpec { follow: spec.follow || (self.ring_clients_wait && !spec.second_stream && spec.state == Policy::Lossless), ..spec }
    }
}

struct Hosted {
    server: Arc<Mutex<Server>>,
    listening: Vec<Listening>,
}

fn host(scale: &Scale, workers: usize) -> Fallible<Hosted> {
    let config = ServerConfig { world: WorldConfig::new(scale.size, workers), plane: scale.plane, log_retention: scale.log_retention };
    let server = Arc::new(Mutex::new(Server::new(config)));
    let listening = serve_all(&server)?;
    Ok(Hosted { server, listening })
}

impl Hosted {
    /// The transport that can hand a ring over.
    fn primary(&self) -> &Address {
        &self.listening[0].address
    }
}

/// The first tick whose announced hash differs from the oracle's, if any.
pub fn divergence(announced: &[u64], oracle: &[u64]) -> Option<u32> {
    let expected = &oracle[1..];
    (0..announced.len().max(expected.len())).find(|&at| announced.get(at) != expected.get(at)).map(|at| at as u32 + 1)
}

// ---------------------------------------------------------------------------
// The client's reader and its stall
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stall {
    Never,
    /// From this tick on, for the rest of the run.
    Forever(u32),
    /// From this tick until the streams have ended, then resume by cursor.
    UntilEnded(u32),
}

#[derive(Clone, Copy, Debug)]
pub struct PlaneSpec {
    pub second_stream: bool,
    pub state: Policy,
    pub frame: Policy,
    /// Hash every state frame's payload on the client.
    pub verify: bool,
    pub stall: Stall,
    /// Wait, between two steps, for the reader to hold the ticks stepped.
    pub follow: bool,
}

impl PlaneSpec {
    pub fn mode(&self) -> &'static str {
        if self.second_stream { "second stream" } else { "ring" }
    }

    fn policy(&self, stream: u32) -> Policy {
        if stream == STATE_STREAM { self.state } else { self.frame }
    }
}

#[derive(Debug, Default)]
pub struct Reception {
    pub report: ReaderReport,
    pub streams: Vec<StreamView>,
    pub stalled_at: Option<u32>,
    pub ended_at: Option<u32>,
    pub ended_codes: Vec<String>,
    pub resumed_at: Option<u32>,
    /// Per resumed stream: its identifier, the client's cursor, the server's
    /// cursor before the resume and the result.
    pub resumes: Vec<(u32, u32, u32, String)>,
    /// Frames whose content disagrees with the hash the step announced for their tick.
    pub content_mismatches: u64,
    pub frames_checked: u64,
}

impl Reception {
    /// Every tick of a lossless stream once, in order, from the first to `ticks`.
    pub fn lossless_complete(&self, spec: &PlaneSpec, ticks: u32) -> bool {
        [STATE_STREAM, FRAME_STREAM].into_iter().filter(|stream| spec.policy(*stream) == Policy::Lossless).all(|stream| self.report.streams[stream as usize].complete(1, u64::from(ticks)))
    }

    pub fn ended_with_the_budget_error(&self) -> bool {
        !self.ended_codes.is_empty() && self.ended_codes.iter().all(|code| code == BUDGET_EXCEEDED)
    }

    pub fn resumed(&self) -> bool {
        !self.resumes.is_empty() && self.resumes.iter().all(|resume| resume.3 == "resumed")
    }
}

struct Reading {
    spec: PlaneSpec,
    control: Arc<ReaderControl>,
    thread: thread::JoinHandle<ReaderReport>,
    reception: Reception,
}

fn wait_until(limit: Duration, mut reached: impl FnMut() -> bool) -> bool {
    let deadline = Instant::now() + limit;
    while !reached() {
        if Instant::now() >= deadline {
            return false;
        }
        thread::sleep(Duration::from_micros(500));
    }
    true
}

impl Reading {
    fn open(client: &mut Client, spec: PlaneSpec) -> Fallible<Reading> {
        let source = if spec.second_stream { client.open_second_stream(spec.state, spec.frame)? } else { client.open_ring(spec.state, spec.frame)? };
        let control = Arc::new(ReaderControl::default());
        let thread = spawn_reader(source, control.clone(), spec.verify);
        Ok(Reading { spec, control, thread, reception: Reception::default() })
    }

    /// What the client does about its reader at a tick it observes.
    fn at_tick(&mut self, client: &mut Client, tick: u32, ended_streams: u32) -> Fallible<()> {
        self.follow(tick, ended_streams);
        let at = match self.spec.stall {
            Stall::Never => return Ok(()),
            Stall::Forever(at) | Stall::UntilEnded(at) => at,
        };
        if self.reception.stalled_at.is_none() && tick >= at {
            self.control.stall.store(true, Ordering::SeqCst);
            self.reception.stalled_at = Some(tick);
        }
        if self.spec.stall == Stall::Forever(at) {
            return Ok(());
        }
        if self.reception.ended_at.is_none() && ended_streams != 0 {
            self.reception.ended_at = Some(tick);
        }
        let Some(ended_at) = self.reception.ended_at else { return Ok(()) };
        if self.reception.resumed_at.is_some() || tick < ended_at + BATCH {
            return Ok(());
        }
        // The client reads again: first what the sink still holds, then it
        // resumes each ended stream at the sequence after the last it received.
        let ended: Vec<StreamView> = client.stream_status()?.into_iter().filter(|stream| !stream.ended.is_empty()).collect();
        self.reception.ended_codes = ended.iter().map(|stream| stream.ended.clone()).collect();
        self.control.stall.store(false, Ordering::SeqCst);
        let seen = self.control.empty_polls.load(Ordering::SeqCst);
        wait_until(Duration::from_secs(20), || self.control.empty_polls.load(Ordering::SeqCst) >= seen + 3);
        for stream in ended {
            let cursor = self.control.last_sequence[stream.id as usize].load(Ordering::SeqCst) as u32 + 1;
            let result = match client.resume_stream(stream.id, cursor) {
                Ok(()) => "resumed".to_string(),
                Err(error) => error.code().to_string(),
            };
            self.reception.resumes.push((stream.id, cursor, stream.cursor, result));
        }
        self.reception.resumed_at = Some(tick);
        Ok(())
    }

    /// A client that follows its lossless streams waits, before it steps
    /// again, for its reader to hold the ticks it stepped: the stepped
    /// client's own pacing, where the transport is slower than the world.
    fn follow(&self, tick: u32, ended_streams: u32) {
        if !self.spec.follow || ended_streams != 0 || self.control.stall.load(Ordering::SeqCst) {
            return;
        }
        let lossless = [STATE_STREAM, FRAME_STREAM].into_iter().filter(|stream| self.spec.policy(*stream) == Policy::Lossless);
        for stream in lossless {
            wait_until(Duration::from_secs(20), || self.control.last_sequence[stream as usize].load(Ordering::SeqCst) >= u64::from(tick));
        }
    }

    /// Waits for the last frames, stops the reader and checks what it
    /// received against the hashes the steps announced.
    fn finish(mut self, client: &mut Client, announced: &[u64]) -> Fallible<Reception> {
        let ticks = announced.len() as u64;
        if !matches!(self.spec.stall, Stall::Forever(_)) {
            // The last frames of the streams still running; an ended stream sends no more.
            let deadline = Instant::now() + Duration::from_secs(20);
            loop {
                let running = client.stream_status()?.into_iter().filter(|stream| stream.ended.is_empty());
                let behind = running.filter(|stream| self.control.last_sequence[stream.id as usize].load(Ordering::SeqCst) < ticks).count();
                if behind == 0 || Instant::now() >= deadline {
                    break;
                }
                thread::sleep(Duration::from_millis(1));
            }
        }
        self.reception.streams = client.stream_status()?;
        self.control.stop.store(true, Ordering::SeqCst);
        let report = self.thread.join().map_err(|_| CallError::Malformed("the reader thread, which panicked"))?;
        for (tick, hash) in report.state_hashes.iter().chain(&report.frame_stamps) {
            self.reception.frames_checked += 1;
            if announced.get((*tick as usize).wrapping_sub(1)) != Some(hash) {
                self.reception.content_mismatches += 1;
            }
        }
        self.reception.report = report;
        Ok(self.reception)
    }
}

// ---------------------------------------------------------------------------
// Stepping without commands: throughput and flow control
// ---------------------------------------------------------------------------

/// The world stepped directly, with no surface and no extraction.
pub fn run_direct(scale: &Scale, workers: usize) -> (Vec<u64>, Duration) {
    let mut world = World::new(WorldConfig::new(scale.size, workers));
    while world.tick() < scale.ticks {
        world.step(BATCH.min(scale.ticks - world.tick())).expect("the world steps");
    }
    ((1..=scale.ticks).map(|tick| world.tick_hash(tick).expect("a completed tick")).collect(), world.stepping())
}

#[derive(Debug)]
pub struct PlainRun {
    pub transport: &'static str,
    pub hashes: Vec<u64>,
    pub wall: Duration,
    pub statistics: Statistics,
    pub reception: Option<Reception>,
}

/// A stepped session that only steps, with or without a data plane.
pub fn run_plain(scale: &Scale, workers: usize, plane: Option<PlaneSpec>) -> Fallible<PlainRun> {
    let hosted = host(scale, workers)?;
    let mut client = Client::connect(hosted.primary(), Profile::Stepped)?;
    let mut reading = plane.map(|spec| Reading::open(&mut client, scale.paced(spec))).transpose()?;
    let mut hashes = Vec::with_capacity(scale.ticks as usize);
    let started = Instant::now();
    let mut tick = 0;
    while tick < scale.ticks {
        let progress = client.step(BATCH.min(scale.ticks - tick))?;
        tick = progress.tick;
        hashes.extend(progress.hashes);
        if let Some(reading) = &mut reading {
            reading.at_tick(&mut client, tick, progress.ended_streams)?;
        }
    }
    let wall = started.elapsed();
    let statistics = client.statistics()?;
    let reception = reading.map(|reading| reading.finish(&mut client, &hashes)).transpose()?;
    Ok(PlainRun { transport: hosted.primary().transport(), hashes, wall, statistics, reception })
}

#[derive(Clone, Debug)]
pub struct SocketRead {
    pub transport: &'static str,
    pub reads: u32,
    pub state_bytes: usize,
    pub median_micros: f64,
    pub mean_micros: f64,
    /// The client's own hash of the bytes it read, for scale.
    pub hash_micros: f64,
    pub every_read_hashes_to_the_tick: bool,
}

/// The full state read through the socket, per transport: the baseline the
/// ring's client cost is compared with.
pub fn run_socket_reads(scale: &Scale, workers: usize) -> Fallible<Vec<SocketRead>> {
    let hosted = host(scale, workers)?;
    let mut stepper = Client::connect(hosted.primary(), Profile::Stepped)?;
    let expected = *stepper.step(BATCH * 5)?.hashes.last().expect("a step announces its ticks");
    let mut reads = Vec::new();
    for listening in &hosted.listening {
        let mut client = Client::connect(&listening.address, Profile::Stepped)?;
        let mut micros = Vec::with_capacity(scale.socket_reads as usize);
        let (mut state_bytes, mut hashing, mut every_read_hashes_to_the_tick) = (0, Duration::ZERO, true);
        for _ in 0..scale.socket_reads {
            let started = Instant::now();
            let (_, bytes) = client.full_state()?;
            micros.push(started.elapsed().as_secs_f64() * 1e6);
            let started = Instant::now();
            every_read_hashes_to_the_tick &= fnv1a(&bytes) == expected;
            hashing += started.elapsed();
            state_bytes = bytes.len();
        }
        let mean_micros = micros.iter().sum::<f64>() / micros.len() as f64;
        micros.sort_by(f64::total_cmp);
        reads.push(SocketRead {
            transport: listening.address.transport(),
            reads: scale.socket_reads,
            state_bytes,
            median_micros: micros[micros.len() / 2],
            mean_micros,
            hash_micros: hashing.as_secs_f64() * 1e6 / f64::from(scale.socket_reads),
            every_read_hashes_to_the_tick,
        });
    }
    Ok(reads)
}

// ---------------------------------------------------------------------------
// The commands: kicks, transactions, inverses and the sequence
// ---------------------------------------------------------------------------

/// What the agent keeps of what it sent, for the oracle to replay.
#[derive(Clone, Debug)]
pub enum Recorded {
    Command { tick: u32, sequence: u64, transaction: Transaction, outcome: Vec<u8> },
    Digests { tick: u32, digests: Digests },
}

#[derive(Clone, Debug)]
pub struct MixReport {
    pub kind: MixKind,
    pub size: usize,
    pub tick: u32,
    /// The valid transaction: applied, every element accepted, the state changed.
    pub applied_whole: bool,
    pub changed_the_state: bool,
    /// One refused element: the elements before it accepted, it refused, the
    /// rest not evaluated, nothing applied.
    pub one_refused_verdicts: (usize, usize, usize),
    /// The code the refused element was rejected with.
    pub refused_with: &'static str,
    pub one_refused_as_expected: bool,
    pub one_refused_left_the_state: bool,
    /// Full evaluation: every planted refusal listed, nothing applied.
    pub planted: usize,
    pub listed: usize,
    pub full_evaluation_left_the_state: bool,
    pub inverse_elements: usize,
    pub inverse_applied: bool,
    pub restored_full_hash: bool,
    pub restored_component_data: bool,
    pub restored_labels: bool,
    pub spawns: usize,
    pub restores_over_a_reused_slot: usize,
    /// The original sent again after the inverse: applied, with the component data and labels of the first time.
    pub reapplied: bool,
}

impl MixReport {
    pub fn atomic(&self) -> bool {
        self.applied_whole && self.changed_the_state && self.one_refused_as_expected && self.one_refused_left_the_state && self.planted == self.listed && self.full_evaluation_left_the_state
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct Probes {
    pub retries_sent: u32,
    pub retries_answered_alike: u32,
    pub gaps_sent: u32,
    pub gaps_refused_naming_the_expected: u32,
    pub altered_sent: u32,
    pub altered_refused_by_name: u32,
    pub probes_left_the_state: u32,
    pub rounds: u32,
    pub outside_retention_refused: bool,
}

impl Probes {
    pub fn hold(&self) -> bool {
        self.rounds > 0
            && self.retries_answered_alike == self.retries_sent
            && self.gaps_refused_naming_the_expected == self.gaps_sent
            && self.altered_refused_by_name == self.altered_sent
            && self.probes_left_the_state == self.rounds
            && self.outside_retention_refused
    }
}

#[derive(Debug)]
pub struct CommandsRun {
    pub transport: &'static str,
    pub hashes: Vec<u64>,
    pub record: Vec<Recorded>,
    pub mixes: Vec<MixReport>,
    pub probes: Probes,
    pub kicks_applied: u32,
    pub kicks_stale: u32,
    pub commands_sent: u64,
    pub statistics: Statistics,
    pub reception: Option<Reception>,
}

struct Scripted<'a> {
    client: &'a mut Client,
    record: Vec<Recorded>,
    last: Option<(u64, Vec<u8>, CommandReply)>,
}

impl Scripted<'_> {
    /// Sends a transaction as the next command and records it with its result.
    fn send(&mut self, transaction: Transaction) -> Fallible<Outcome> {
        let (sequence, bytes, reply) = self.client.send(&transaction)?;
        let (Some(tick), Some(outcome)) = (reply.tick, reply.outcome.clone()) else {
            return Err(CallError::Malformed("a command of a stepped session, which carries its result"));
        };
        let decoded = Outcome::decode(&outcome).ok_or(CallError::Malformed("a command's outcome"))?;
        self.record.push(Recorded::Command { tick, sequence, transaction, outcome });
        self.last = Some((sequence, bytes, reply));
        Ok(decoded)
    }

    fn digests(&mut self, tick: u32) -> Fallible<Digests> {
        let digests = self.client.digests()?;
        self.record.push(Recorded::Digests { tick, digests });
        Ok(digests)
    }

    fn view(&mut self) -> Fallible<Vec<crate::commands::SlotView>> {
        read_canonical(&self.client.full_state()?.1).ok_or(CallError::Malformed("a full state read"))
    }

    fn kicks(&mut self, watched: &[Handle]) -> Fallible<(u32, u32)> {
        let bodies = self.client.entity_state(watched)?;
        let alive: Vec<_> = bodies.iter().flatten().collect();
        let count = alive.len().max(1) as f32;
        let (cx, cy) = (alive.iter().map(|body| body.x).sum::<f32>() / count, alive.iter().map(|body| body.y).sum::<f32>() / count);
        let (mut applied, mut stale) = (0, 0);
        for (entity, body) in watched.iter().zip(&bodies) {
            let (delta_vx, delta_vy) = body.map_or((0.0, 0.0), |body| ((cx - body.x) * 0.01, (cy - body.y) * 0.01));
            let outcome = self.send(Transaction::of(vec![Element::Kick { entity: *entity, delta_vx, delta_vy }]))?;
            if outcome.applied { applied += 1 } else { stale += 1 }
        }
        Ok((applied, stale))
    }

    /// One kind at one size: the two refused variants, the valid transaction
    /// with its inverse, the inverse, and the original again.
    fn mix(&mut self, kind: MixKind, size: usize, index: usize, tick: u32, book: &mut LabelBook) -> Fallible<MixReport> {
        let mut view = self.view()?;
        let start = 100 + (index * 577) % (view.len() / 2);
        if kind == MixKind::Spawns {
            let freed = slots_for_spawns(size, &view, start + view.len() / 4);
            book.follow(&freed);
            self.send(Transaction::of(freed))?;
            view = self.view()?;
        }
        let elements = build(kind, size, &view, book, start);
        let before = self.digests(tick)?;

        let (one, position) = with_one_refused(&elements, &view);
        let refused = self.send(Transaction::of(one))?;
        let after_one = self.digests(tick)?;
        let (several, planted) = with_several_refused(&elements, &view);
        let evaluated = self.send(Transaction { full_evaluation: true, want_inverse: false, elements: several })?;
        let after_several = self.digests(tick)?;

        let applied = self.send(Transaction { full_evaluation: false, want_inverse: true, elements: elements.clone() })?;
        let changed = self.digests(tick)?;
        let inverse = applied.inverse.clone().unwrap_or_default();
        let undone = self.send(Transaction::of(inverse.clone()))?;
        let restored = self.digests(tick)?;
        let again = self.send(Transaction::of(elements.clone()))?;
        let redone = self.digests(tick)?;
        if again.applied {
            book.follow(&elements);
        }
        let one_refused_verdicts = (refused.accepted(), refused.rejected(), refused.not_evaluated());
        let refused_with = refused.verdicts.iter().find_map(|verdict| if let Verdict::Rejected(rejection) = verdict { Some(rejection.code()) } else { None }).unwrap_or("");
        Ok(MixReport {
            kind,
            size,
            tick,
            applied_whole: applied.applied && applied.accepted() == size,
            changed_the_state: changed != before,
            one_refused_verdicts,
            refused_with,
            one_refused_as_expected: !refused.applied && one_refused_verdicts == (position, 1, size - position - 1),
            one_refused_left_the_state: after_one == before,
            planted,
            listed: evaluated.rejected(),
            full_evaluation_left_the_state: !evaluated.applied && evaluated.not_evaluated() == 0 && after_several == before,
            inverse_elements: inverse.len(),
            inverse_applied: undone.applied,
            restored_full_hash: restored.full == before.full,
            restored_component_data: restored.components == before.components,
            restored_labels: restored.labels == before.labels,
            spawns: elements.iter().filter(|element| matches!(element, Element::Spawn { .. })).count(),
            restores_over_a_reused_slot: restores_over_a_reused_slot(&inverse),
            reapplied: again.applied && (redone.components, redone.labels) == (changed.components, changed.labels),
        })
    }

    /// The sequence's three probes: the last command retried, a sequence
    /// that skips ahead, the last sequence retried with other bytes.
    fn probe(&mut self, probes: &mut Probes, watched: Handle) -> Fallible<()> {
        let Some((sequence, bytes, first)) = self.last.clone() else { return Ok(()) };
        let before = self.client.digests()?;
        probes.rounds += 1;
        probes.retries_sent += 1;
        let again = self.client.command(sequence, &bytes)?;
        if again.retry && again.tick == first.tick && again.outcome == first.outcome {
            probes.retries_answered_alike += 1;
        }
        let other = Transaction::of(vec![Element::Kick { entity: watched, delta_vx: 1.0, delta_vy: 1.0 }]).encode();
        probes.gaps_sent += 1;
        let expected = self.client.next_sequence;
        if let Err(CallError::Surface { code, context, .. }) = self.client.command(expected + 1, &other) {
            probes.gaps_refused_naming_the_expected += u32::from(code == "sequence_gap" && context == expected);
        }
        probes.altered_sent += 1;
        if let Err(error) = self.client.command(sequence, &other) {
            probes.altered_refused_by_name += u32::from(error.code() == "altered_retry");
        }
        probes.probes_left_the_state += u32::from(self.client.digests()? == before);
        Ok(())
    }
}

/// The scripted agent's full run through a stepped session: kicks every ten
/// ticks, every kind of transaction at every size, the sequence's probes,
/// with a data plane read and verified beside it.
pub fn run_commands(scale: &Scale, workers: usize, plane: Option<PlaneSpec>) -> Fallible<CommandsRun> {
    let hosted = host(scale, workers)?;
    let mut client = Client::connect(hosted.primary(), Profile::Stepped)?;
    let mut reading = plane.map(|spec| Reading::open(&mut client, scale.paced(spec))).transpose()?;
    let watched: Vec<Handle> = (0..WATCHED as u32).map(|index| Handle { index, generation: 0 }).collect();
    let mixes: Vec<(usize, MixKind)> = SIZES.into_iter().flat_map(|size| MixKind::ALL.into_iter().map(move |kind| (size, kind))).collect();
    let stride = ((scale.ticks / BATCH).saturating_sub(4) / mixes.len() as u32).max(1);
    let probe_ticks = [3, 5, 7].map(|tenths| scale.ticks * tenths / 10 / BATCH * BATCH);

    let mut scripted = Scripted { client: &mut client, record: Vec::new(), last: None };
    let mut book = LabelBook::default();
    let mut reports = Vec::new();
    let mut probes = Probes::default();
    let (mut kicks_applied, mut kicks_stale) = (0, 0);
    let mut hashes = Vec::with_capacity(scale.ticks as usize);

    let view = scripted.view()?;
    let labels = first_labels(&view, view.len() / 3, (view.len() / 12).min(80));
    if scripted.send(Transaction::of(labels.clone()))?.applied {
        book.follow(&labels);
    }
    let first_command = scripted.last.clone().expect("a command was sent");

    let mut tick = 0;
    while tick < scale.ticks {
        let (applied, stale) = scripted.kicks(&watched)?;
        kicks_applied += applied;
        kicks_stale += stale;
        let due = (tick / BATCH).checked_sub(2).filter(|boundary| boundary % stride == 0).map(|boundary| (boundary / stride) as usize);
        if let Some(index) = due.filter(|index| *index < mixes.len()) {
            let (size, kind) = mixes[index];
            reports.push(scripted.mix(kind, size, index, tick, &mut book)?);
        }
        if probe_ticks.contains(&tick) {
            scripted.probe(&mut probes, watched[0])?;
        }
        let progress = scripted.client.step(BATCH.min(scale.ticks - tick))?;
        tick = progress.tick;
        hashes.extend(progress.hashes);
        if let Some(reading) = &mut reading {
            reading.at_tick(scripted.client, tick, progress.ended_streams)?;
        }
    }
    scripted.digests(tick)?;
    // The first command is older than the retained log by now.
    probes.outside_retention_refused = matches!(scripted.client.command(first_command.0, &first_command.1), Err(error) if error.code() == "outside_retention");
    let record = scripted.record;
    let commands_sent = client.next_sequence - 1;
    let statistics = client.statistics()?;
    let reception = reading.map(|reading| reading.finish(&mut client, &hashes)).transpose()?;
    Ok(CommandsRun { transport: hosted.primary().transport(), hashes, record, mixes: reports, probes, kicks_applied, kicks_stale, commands_sent, statistics, reception })
}

#[derive(Debug)]
pub struct Replay {
    pub oracle_hashes: Vec<u64>,
    pub commands: u64,
    pub outcome_mismatches: u64,
    pub digests: u64,
    pub digest_mismatches: u64,
}

/// The sequential oracle given the agent's record: the same transactions at
/// the same tick boundaries, its outcomes and digests compared on the way.
pub fn replay(size: Size, ticks: u32, record: &[Recorded]) -> Replay {
    let mut oracle = Oracle::new(size);
    let mut replay = Replay { oracle_hashes: Vec::new(), commands: 0, outcome_mismatches: 0, digests: 0, digest_mismatches: 0 };
    for entry in record {
        match entry {
            Recorded::Command { tick, transaction, outcome, .. } => {
                oracle.step_to(*tick);
                replay.commands += 1;
                replay.outcome_mismatches += u64::from(oracle.tick() != *tick || oracle.transact(transaction).encode() != *outcome);
            }
            Recorded::Digests { tick, digests } => {
                oracle.step_to(*tick);
                replay.digests += 1;
                replay.digest_mismatches += u64::from(oracle.tick() != *tick || oracle.digests() != *digests);
            }
        }
    }
    oracle.step_to(ticks);
    replay.oracle_hashes = oracle.hashes;
    replay
}

// ---------------------------------------------------------------------------
// The paced session
// ---------------------------------------------------------------------------

#[derive(Debug)]
pub struct PacedRun {
    pub transport: &'static str,
    pub hashes: Vec<u64>,
    pub record: Vec<Recorded>,
    pub host_steps: u32,
    pub host_ticks_per_second: f64,
    pub commands_sent: u64,
    pub kicks_applied: u32,
    pub kicks_stale: u32,
    pub commands_never_applied: usize,
    pub statistics: Statistics,
    pub reception: Reception,
}

/// A host steps the world one tick per millisecond; the agent polls, kicks
/// when the tick crosses a multiple of ten, asks for each command's result by
/// retrying its sequence, and its reader stalls once and resumes by cursor.
pub fn run_paced(scale: &Scale, workers: usize, spec: PlaneSpec) -> Fallible<PacedRun> {
    let hosted = host(scale, workers)?;
    let mut client = Client::connect(hosted.primary(), Profile::Paced)?;
    let mut reading = Reading::open(&mut client, spec)?;
    let stop = Arc::new(AtomicBool::new(false));
    let (server, stop_flag, ticks) = (hosted.server.clone(), stop.clone(), scale.ticks);
    let host_thread = thread::Builder::new().name("paced-host".to_string()).spawn(move || {
        let started = Instant::now();
        let mut steps = 0;
        while !stop_flag.load(Ordering::SeqCst) {
            {
                let mut server = server.lock().unwrap();
                if server.tick() >= ticks || server.host_tick().is_err() {
                    break;
                }
            }
            steps += 1;
            thread::sleep(PACE);
        }
        (steps, started.elapsed())
    })?;

    let watched: Vec<Handle> = (0..WATCHED as u32).map(|index| Handle { index, generation: 0 }).collect();
    let mut hashes = Vec::with_capacity(scale.ticks as usize);
    let mut record = Vec::new();
    let mut waiting: Vec<(u64, Vec<u8>, Transaction)> = Vec::new();
    let (mut kicks_applied, mut kicks_stale) = (0, 0);
    let mut next_kicks_at = 0;
    let outcome = (|| -> Fallible<()> {
        loop {
            let progress = client.poll(hashes.len() as u32)?;
            hashes.extend(progress.hashes);
            let tick = progress.tick;
            reading.at_tick(&mut client, tick, progress.ended_streams)?;
            let mut still_waiting = Vec::new();
            for (sequence, bytes, transaction) in waiting.drain(..) {
                let reply = client.command(sequence, &bytes)?;
                match (reply.tick, reply.outcome) {
                    (Some(tick), Some(outcome)) => {
                        let applied = Outcome::decode(&outcome).is_some_and(|outcome| outcome.applied);
                        if applied { kicks_applied += 1 } else { kicks_stale += 1 }
                        record.push(Recorded::Command { tick, sequence, transaction, outcome });
                    }
                    _ => still_waiting.push((sequence, bytes, transaction)),
                }
            }
            waiting = still_waiting;
            if tick >= scale.ticks {
                return Ok(());
            }
            if tick >= next_kicks_at && tick + 2 * BATCH < scale.ticks {
                let bodies = client.entity_state(&watched)?;
                for (entity, body) in watched.iter().zip(&bodies) {
                    let (delta_vx, delta_vy) = body.map_or((0.0, 0.0), |body| ((512.0 - body.x) * 0.01, (512.0 - body.y) * 0.01));
                    let transaction = Transaction::of(vec![Element::Kick { entity: *entity, delta_vx, delta_vy }]);
                    let (sequence, bytes, _) = client.send(&transaction)?;
                    waiting.push((sequence, bytes, transaction));
                }
                next_kicks_at = (tick / BATCH + 1) * BATCH;
            }
            thread::sleep(Duration::from_micros(300));
        }
    })();
    stop.store(true, Ordering::SeqCst);
    let (host_steps, hosting) = host_thread.join().map_err(|_| CallError::Malformed("the host thread, which panicked"))?;
    outcome?;
    record.sort_by_key(|entry| match entry {
        Recorded::Command { tick, sequence, .. } => (*tick, *sequence),
        Recorded::Digests { tick, .. } => (*tick, u64::MAX),
    });
    let statistics = client.statistics()?;
    let reception = reading.finish(&mut client, &hashes)?;
    Ok(PacedRun {
        transport: hosted.primary().transport(),
        hashes,
        record,
        host_steps,
        host_ticks_per_second: f64::from(host_steps) / hosting.as_secs_f64(),
        commands_sent: client.next_sequence - 1,
        kicks_applied,
        kicks_stale,
        commands_never_applied: waiting.len(),
        statistics,
        reception,
    })
}

// ---------------------------------------------------------------------------
// The cost of computing the inverse
// ---------------------------------------------------------------------------

#[derive(Clone, Debug)]
pub struct InverseCost {
    pub kind: MixKind,
    pub size: usize,
    pub transactions: usize,
    /// Median nanoseconds per transaction over the rounds.
    pub apply_nanos: f64,
    pub apply_with_inverse_nanos: f64,
}

impl InverseCost {
    /// The inverse's share of applying the transaction without it.
    pub fn ratio(&self) -> f64 {
        (self.apply_with_inverse_nanos - self.apply_nanos) / self.apply_nanos
    }
}

/// Applies the same transactions to copies of one state with and without
/// asking for the inverse, in alternating rounds, in process.
pub fn inverse_cost(size: Size, rounds: usize) -> Vec<InverseCost> {
    let mut oracle = Oracle::new(size);
    oracle.step_to(50);
    let mut costs = Vec::new();
    for tx_size in SIZES {
        for kind in MixKind::ALL {
            let (state, labels, dead) = oracle.parts();
            let (mut state, mut labels, mut dead) = (state.clone(), labels.clone(), dead.to_vec());
            let n = state.len();
            let mut scratch = Scratch::new(n);
            let mut touched = Vec::new();
            let spacing = tx_size * 3 + 3;
            let count = [256, 64, 12][SIZES.iter().position(|size| *size == tx_size).expect("a listed size")].min((n / 2).saturating_sub(20) / spacing).max(1);
            let view = |state: &crate::world::State| {
                let mut bytes = Vec::new();
                write_canonical(state, &mut bytes);
                read_canonical(&bytes).expect("canonical bytes read back")
            };
            // Labels for the removes, in slots no transaction's other elements reach.
            let per_transaction = tx_size / 2 + tx_size / 10;
            let seeds = first_labels(&view(&state), n * 6 / 10, (count * per_transaction).max(1));
            let mut book = LabelBook::default();
            assert!(transact(&mut state, &mut labels, &mut dead, &mut touched, &mut scratch, &Transaction::of(seeds.clone())).applied);
            book.follow(&seeds);
            if kind == MixKind::Spawns {
                let freed = slots_for_spawns(count * tx_size, &view(&state), n * 3 / 4);
                assert!(transact(&mut state, &mut labels, &mut dead, &mut touched, &mut scratch, &Transaction::of(freed)).applied);
            }
            let seen = view(&state);
            let transactions: Vec<Vec<Element>> = (0..count).map(|j| build(kind, tx_size, &seen, &book.part(&seen, j * per_transaction, per_transaction), 20 + j * spacing)).collect();
            let mut samples = [Vec::with_capacity(rounds), Vec::with_capacity(rounds)];
            for round in 0..rounds * 2 {
                let want_inverse = round % 2 == 1;
                let batch: Vec<Transaction> = transactions.iter().map(|elements| Transaction { full_evaluation: false, want_inverse, elements: elements.clone() }).collect();
                let (mut state, mut labels, mut dead) = (state.clone(), labels.clone(), dead.clone());
                touched.clear();
                let started = Instant::now();
                let mut applied = 0;
                for transaction in &batch {
                    let outcome = transact(&mut state, &mut labels, &mut dead, &mut touched, &mut scratch, transaction);
                    applied += usize::from(outcome.applied && outcome.inverse.is_some() == want_inverse);
                    std::hint::black_box(&outcome);
                }
                let nanos = started.elapsed().as_nanos() as f64 / count as f64;
                assert_eq!(applied, count, "every measured transaction applies: {} of {tx_size}", kind.name());
                samples[usize::from(want_inverse)].push(nanos);
            }
            let median = |values: &mut Vec<f64>| {
                values.sort_by(f64::total_cmp);
                values[values.len() / 2]
            };
            let [without, with] = &mut samples;
            costs.push(InverseCost { kind, size: tx_size, transactions: count, apply_nanos: median(without), apply_with_inverse_nanos: median(with) });
        }
    }
    costs
}
