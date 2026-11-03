//! Control surface toy spike, for the control surface research. Throwaway,
//! never product code.
//!
//! One command, one query and one event subscription over the reference
//! workload, driven by a scripted agent that discovers the contract through
//! introspection, served in process (typed, and as messages) and over local
//! sockets, with a reconnection, an idempotent retry, a paced session, one
//! compatible schema change and the structured rejection of what the
//! description lacks. The spike measures, instead of assuming, whether the
//! agent's hashes equal the sequential oracle's and the world's stepped
//! directly with the same kicks, whether the input log replays to them, what
//! the surface costs in process, and what a step costs over a local socket.

mod agent;
mod contract;
mod surface;
mod transport;
mod world;

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use agent::{AgentReport, Client, MessageClient, Observed, Script, TypedClient, decide, run_paced, run_stepped};
use contract::Handle;
use reference_workload::{DEFAULT_TICK_COUNT, Grid, Json, Parameters, SEQUENTIAL_PHASES, State, Timing, first_divergent_tick, run_hash, write_platform_and_build};
use surface::{InputLog, Server, replay};
use transport::{Endpoint, InProcess, connect_tcp, serve_tcp};
use world::{Kick, Size, World, WorldConfig, kicked_velocity, validate_kicks};

const SCHEMA_VERSION: u64 = 1;
const SPIKE: &str = "control-surface-toy";
const BATCH: u32 = 10;
const DISCONNECT_AT: u32 = 500;
const ROUND_TRIP_TICKS: u32 = 200;
const EMPTY_ROUND_TRIPS: u32 = 2_000;
const PACE: Duration = Duration::from_millis(1);
const PACED_DISCONNECT_AT: u32 = 300;
const PACED_DISCONNECT_FOR: Duration = Duration::from_millis(30);
const SCALE: f32 = 0.5;

fn worker_counts() -> Vec<usize> {
    let available = thread::available_parallelism().map_or(1, |n| n.get());
    let mut counts = vec![1, 4];
    if !counts.contains(&available) {
        counts.push(available);
        counts.sort_unstable();
    }
    counts
}

#[derive(Clone, Copy)]
struct Scenario {
    size: Size,
    ticks: u32,
    disconnect_at: u32,
    paced_disconnect_at: u32,
}

const MEASURED: Scenario = Scenario { size: Size::Default, ticks: DEFAULT_TICK_COUNT, disconnect_at: DISCONNECT_AT, paced_disconnect_at: PACED_DISCONNECT_AT };

// ---------------------------------------------------------------------------
// The scripted oracle
// ---------------------------------------------------------------------------

fn observe(state: &State, watched: &[u32]) -> Vec<Observed> {
    watched
        .iter()
        .map(|&index| {
            let slot = index as usize;
            let present = slot < state.len() && state.alive[slot];
            Observed {
                handle: Handle { index, generation: if slot < state.len() { state.generation[slot] } else { 0 } },
                present,
                x: if present { state.x[slot] } else { 0.0 },
                y: if present { state.y[slot] } else { 0.0 },
            }
        })
        .collect()
}

/// The sequential reference stepped with the kicks the script decides from
/// its own observations at every kick boundary: the ground truth. Returns the
/// tick hashes from tick 0 and the records of kicks per tick.
fn scripted_oracle(config: &WorldConfig, script: &Script) -> (Vec<u64>, Vec<Vec<Kick>>) {
    let parameters = Parameters { seed: config.seed, size: config.size, tick_count: script.total_ticks, wander: config.wander };
    let mut state = State::initialize(&parameters);
    let mut grid = Grid::new();
    let mut timing = Timing::new(&SEQUENTIAL_PHASES);
    let mut hashes = vec![state.hash()];
    let mut records = Vec::with_capacity(script.total_ticks as usize);
    let scale = script.scale.unwrap_or(1.0);
    for tick in 0..script.total_ticks {
        let mut record = Vec::new();
        if tick % script.kick_every == 0 {
            let rows = observe(&state, &script.watched);
            if let Some((dvx, dvy)) = decide(&rows) {
                record = rows
                    .iter()
                    .filter(|r| r.present)
                    .map(|r| Kick { index: r.handle.index, generation: r.handle.generation, delta_vx: dvx * scale, delta_vy: dvy * scale })
                    .collect();
            }
        }
        let (_, applied) = validate_kicks(&state, &record);
        for (slot, dvx, dvy) in applied {
            state.vx[slot] = kicked_velocity(state.vx[slot], dvx);
            state.vy[slot] = kicked_velocity(state.vy[slot], dvy);
        }
        records.push(record);
        reference_workload::tick(&mut state, &mut grid, tick + 1, config.wander, &mut timing);
        hashes.push(state.hash());
    }
    (hashes, records)
}

// ---------------------------------------------------------------------------
// Paths
// ---------------------------------------------------------------------------

struct PathOutcome {
    path: &'static str,
    transport: &'static str,
    report: AgentReport,
    wall: Duration,
    world_stepping: Duration,
    log: InputLog,
    log_bytes: usize,
    replay_hashes: Option<Vec<u64>>,
    replay_error: Option<String>,
}

fn hashes_of(report: &AgentReport, initial: u64) -> Vec<u64> {
    let mut hashes = vec![initial];
    let mut sorted = report.tick_hashes.clone();
    sorted.sort_unstable_by_key(|(tick, _)| *tick);
    sorted.dedup_by_key(|(tick, _)| *tick);
    for (tick, hash) in sorted {
        if tick as usize == hashes.len() {
            hashes.push(hash);
        }
    }
    hashes
}

fn run_path(path: &'static str, transport: &'static str, server: Arc<Mutex<Server>>, client: &mut dyn Client, script: &Script, config: WorldConfig, schema_version: u32) -> PathOutcome {
    let started = Instant::now();
    let report = run_stepped(client, script);
    let wall = started.elapsed();
    let server = server.lock().unwrap();
    let log = server.input_log.clone();
    let log_bytes = log.bytes().len();
    let (replay_hashes, replay_error) = match replay(&log, config, schema_version) {
        Ok(hashes) => (Some(hashes), None),
        Err(error) => (None, Some(error.to_string())),
    };
    PathOutcome { path, transport, report, wall, world_stepping: server.world.stepping(), log, log_bytes, replay_hashes, replay_error }
}

fn write_path(out: &mut Json, outcome: &PathOutcome, oracle: &[u64], ticks: u32) {
    let r = &outcome.report;
    let hashes = hashes_of(r, oracle[0]);
    let divergence = first_divergent_tick(&hashes, oracle);
    out.object_start();
    out.string("path", outcome.path);
    out.string("transport", outcome.transport);
    out.u64("ticks_announced", hashes.len().saturating_sub(1) as u64);
    out.hex("run_hash", run_hash(&hashes));
    out.bool("matches_oracle", divergence.is_none() && hashes.len() == oracle.len());
    out.i64("first_divergent_tick", divergence.map_or(-1, |t| t as i64));
    out.decimal("wall_ms", outcome.wall.as_secs_f64() * 1_000.0);
    out.decimal("world_stepping_ms", outcome.world_stepping.as_secs_f64() * 1_000.0);
    out.decimal("ticks_per_second", f64::from(ticks) / outcome.wall.as_secs_f64().max(1e-9));
    out.u64("step_calls", r.step_calls);
    out.decimal("step_us_per_call", r.step_time.as_secs_f64() * 1e6 / r.step_calls.max(1) as f64);
    out.u64("query_calls", r.query_calls);
    out.u64("commands_sent", r.commands_sent);
    out.u64("events_received", r.events_received);
    out.bool("events_contiguous", r.events_contiguous);
    out.u64("duplicate_events", r.duplicate_events);
    out.bool("step_hashes_match_events", r.step_hashes_match_events);
    out.u64("outcomes_applied", r.outcomes_applied);
    out.u64("outcomes_stale", r.outcomes_stale);
    out.u64("outcomes_unknown", r.outcomes_unknown);
    out.bool("reconnected", r.reconnected);
    match r.retried_key_answered_alike {
        Some(alike) => out.bool("retried_key_answered_alike", alike),
        None => out.string("retried_key_answered_alike", "not retried"),
    }
    out.u64("description_bytes", r.description_bytes as u64);
    out.u64("bytes_sent", r.bytes_sent);
    out.u64("bytes_received", r.bytes_received);
    out.u64("log_bytes", outcome.log_bytes as u64);
    out.decimal("log_bytes_per_tick", outcome.log_bytes as f64 / f64::from(ticks));
    out.u64("log_commands", outcome.log.command_count() as u64);
    match &outcome.replay_hashes {
        Some(replayed) => {
            let divergence = first_divergent_tick(replayed, &hashes);
            out.bool("replay_matches", divergence.is_none() && replayed.len() == hashes.len());
            out.i64("replay_first_divergent_tick", divergence.map_or(-1, |t| t as i64));
        }
        None => out.bool("replay_matches", false),
    }
    if let Some(error) = &outcome.replay_error {
        out.string("replay_error", error);
    }
    out.u64("final_tick", u64::from(r.final_tick));
    if let Some(error) = &r.error {
        out.string("error", error);
    }
    out.object_end();
}

fn tcp_connector(address: String) -> Box<dyn FnMut() -> std::io::Result<Box<dyn Endpoint>> + Send> {
    Box::new(move || Ok(Box::new(connect_tcp(&address)?) as Box<dyn Endpoint>))
}

#[cfg(unix)]
fn unix_connector(path: String) -> Box<dyn FnMut() -> std::io::Result<Box<dyn Endpoint>> + Send> {
    Box::new(move || Ok(Box::new(transport::unix::connect_unix(&path)?) as Box<dyn Endpoint>))
}

fn in_process_connector(server: Arc<Mutex<Server>>) -> Box<dyn FnMut() -> std::io::Result<Box<dyn Endpoint>> + Send> {
    Box::new(move || Ok(Box::new(InProcess::new(server.clone())) as Box<dyn Endpoint>))
}

/// The paths of one worker count: the world stepped directly with the
/// oracle's records, then the agent through the typed client, the in-process
/// message endpoint, the loopback socket (with a disconnection, a resume and
/// a retried key) and the Unix domain socket where the platform has one.
fn run_paths(out: &mut Json, scenario: &Scenario, workers: usize) {
    let config = WorldConfig::new(scenario.size, workers);
    let script = Script::version_1(scenario.ticks, BATCH);
    let (oracle_hashes, records) = scripted_oracle(&config, &script);
    out.u64("workers", workers as u64);
    out.hex("oracle_run_hash", run_hash(&oracle_hashes));
    out.u64("oracle_kicks", records.iter().map(|r| r.len() as u64).sum());

    // The world alone, with the same records, in the same batches, after a
    // warm-up instance of ten ticks (the paths run on a warm machine).
    World::new(config).step(vec![Vec::new(); 10]).expect("the warm-up steps");
    let mut world = World::new(config);
    let started = Instant::now();
    let mut outcomes = 0_u64;
    while world.tick() < scenario.ticks {
        let to = (world.tick() + BATCH).min(scenario.ticks);
        outcomes += world.step(records[world.tick() as usize..to as usize].to_vec()).expect("the world steps").iter().map(|o| o.len() as u64).sum::<u64>();
    }
    let wall = started.elapsed();
    let direct: Vec<u64> = (0..=world.tick()).map(|t| world.tick_hash(t).expect("in the history")).collect();
    out.key("direct");
    out.object_start();
    out.hex("run_hash", run_hash(&direct));
    out.bool("matches_oracle", first_divergent_tick(&direct, &oracle_hashes).is_none());
    out.i64("first_divergent_tick", first_divergent_tick(&direct, &oracle_hashes).map_or(-1, |t| t as i64));
    out.u64("kicks_decided", outcomes);
    out.decimal("wall_ms", wall.as_secs_f64() * 1_000.0);
    out.decimal("ticks_per_second", f64::from(scenario.ticks) / wall.as_secs_f64());
    out.object_end();

    out.key("paths");
    out.array_start();
    {
        let server = Arc::new(Mutex::new(Server::new(config, 1)));
        let mut client = TypedClient::new(server.clone());
        let outcome = run_path("typed", "in-process-typed", server, &mut client, &script, config, 1);
        write_path(out, &outcome, &oracle_hashes, scenario.ticks);
    }
    {
        let server = Arc::new(Mutex::new(Server::new(config, 1)));
        let mut client = MessageClient::new(in_process_connector(server.clone())).expect("in process connects");
        let outcome = run_path("messages", "in-process-messages", server, &mut client, &script, config, 1);
        write_path(out, &outcome, &oracle_hashes, scenario.ticks);
    }
    {
        let server = Arc::new(Mutex::new(Server::new(config, 1)));
        let listening = serve_tcp(server.clone()).expect("a loopback port");
        let mut client = MessageClient::new(tcp_connector(listening.address.clone())).expect("tcp connects");
        let disconnecting = Script { disconnect_at: Some(scenario.disconnect_at), ..script.clone() };
        let outcome = run_path("messages", listening.transport, server, &mut client, &disconnecting, config, 1);
        write_path(out, &outcome, &oracle_hashes, scenario.ticks);
        drop(client);
        listening.stop();
    }
    #[cfg(unix)]
    {
        let server = Arc::new(Mutex::new(Server::new(config, 1)));
        let listening = transport::unix::serve_unix(server.clone(), &format!("paths-{workers}")).expect("a unix socket");
        let mut client = MessageClient::new(unix_connector(listening.address.clone())).expect("unix connects");
        let outcome = run_path("messages", listening.transport, server, &mut client, &script, config, 1);
        write_path(out, &outcome, &oracle_hashes, scenario.ticks);
        drop(client);
        listening.stop();
    }
    out.array_end();

    // Round trips: one-tick steps with a query, and empty polls.
    out.key("round_trips");
    out.array_start();
    let short = Script { total_ticks: ROUND_TRIP_TICKS.min(scenario.ticks), batch: 1, ..script.clone() };
    let mut transports: Vec<(&'static str, Box<dyn FnOnce(Arc<Mutex<Server>>) -> (Box<dyn Client>, Option<transport::Listening>)>)> = vec![
        ("in-process-typed", Box::new(|server| (Box::new(TypedClient::new(server)) as Box<dyn Client>, None))),
        ("in-process-messages", Box::new(|server| (Box::new(MessageClient::new(in_process_connector(server)).expect("connects")) as Box<dyn Client>, None))),
        ("tcp-loopback", Box::new(|server| {
            let listening = serve_tcp(server).expect("a loopback port");
            (Box::new(MessageClient::new(tcp_connector(listening.address.clone())).expect("connects")) as Box<dyn Client>, Some(listening))
        })),
    ];
    #[cfg(unix)]
    transports.push((
        "unix-domain-socket",
        Box::new(move |server| {
            let listening = transport::unix::serve_unix(server, &format!("round-trips-{workers}")).expect("a unix socket");
            (Box::new(MessageClient::new(unix_connector(listening.address.clone())).expect("connects")) as Box<dyn Client>, Some(listening))
        }),
    ));
    for (name, connect) in transports {
        let server = Arc::new(Mutex::new(Server::new(config, 1)));
        let (mut client, listening) = connect(server.clone());
        let report = run_stepped(client.as_mut(), &short);
        let world_stepping = server.lock().unwrap().world.stepping();
        let (session, _) = client.open(surface::Profile::Stepped, &["tick_completed"]).expect("a session opens");
        let polling = Instant::now();
        let mut polled = 0_u64;
        for _ in 0..EMPTY_ROUND_TRIPS {
            let (_, events) = client.poll(session, u64::MAX / 2).expect("a poll answers");
            polled += events.len() as u64;
        }
        let polling = polling.elapsed();
        out.object_start();
        out.string("transport", name);
        out.u64("ticks", u64::from(short.total_ticks));
        out.u64("step_calls", report.step_calls);
        out.decimal("step_us_per_call", report.step_time.as_secs_f64() * 1e6 / report.step_calls.max(1) as f64);
        out.decimal("world_step_us_per_call", world_stepping.as_secs_f64() * 1e6 / report.step_calls.max(1) as f64);
        out.decimal("surface_and_transport_us_per_step", (report.step_time.saturating_sub(world_stepping)).as_secs_f64() * 1e6 / report.step_calls.max(1) as f64);
        out.u64("empty_round_trips", u64::from(EMPTY_ROUND_TRIPS));
        out.decimal("empty_round_trip_us", polling.as_secs_f64() * 1e6 / f64::from(EMPTY_ROUND_TRIPS));
        out.u64("events_in_empty_polls", polled);
        out.bool("matches_oracle", first_divergent_tick(&hashes_of(&report, oracle_hashes[0]), &oracle_hashes[..=short.total_ticks as usize]).is_none());
        if let Some(error) = &report.error {
            out.string("error", error);
        }
        out.object_end();
        drop(client);
        if let Some(listening) = listening {
            listening.stop();
        }
    }
    out.array_end();
}

// ---------------------------------------------------------------------------
// The paced session
// ---------------------------------------------------------------------------

fn run_paced_scenario(out: &mut Json, scenario: &Scenario, workers: usize) {
    let config = WorldConfig::new(scenario.size, workers);
    let server = Arc::new(Mutex::new(Server::new(config, 1)));
    let listening = serve_tcp(server.clone()).expect("a loopback port");
    let stop = Arc::new(AtomicBool::new(false));
    let host = {
        let server = server.clone();
        let stop = stop.clone();
        let ticks = scenario.ticks;
        thread::spawn(move || {
            let started = Instant::now();
            let mut steps = 0_u64;
            while !stop.load(Ordering::SeqCst) {
                {
                    let mut s = server.lock().unwrap();
                    if s.tick() >= ticks {
                        break;
                    }
                    s.tick_once().expect("the paced host steps");
                    steps += 1;
                }
                thread::sleep(PACE);
            }
            (steps, started.elapsed())
        })
    };
    let script = Script { disconnect_at: Some(scenario.paced_disconnect_at), ..Script::version_1(scenario.ticks, 1) };
    let mut client = MessageClient::new(tcp_connector(listening.address.clone())).expect("tcp connects");
    let started = Instant::now();
    let report = run_paced(&mut client, &script, PACED_DISCONNECT_FOR);
    let wall = started.elapsed();
    stop.store(true, Ordering::SeqCst);
    let (host_steps, host_elapsed) = host.join().expect("the host joins");
    let (hashes, log, replayed) = {
        let s = server.lock().unwrap();
        let hashes: Vec<u64> = (0..=s.tick()).map(|t| s.world.tick_hash(t).expect("in the history")).collect();
        (hashes, s.input_log.clone(), replay(&s.input_log, config, 1))
    };
    drop(client);
    listening.stop();
    let announced = hashes_of(&report, hashes[0]);
    out.key("paced");
    out.object_start();
    out.string("transport", "tcp-loopback");
    out.u64("workers", workers as u64);
    out.decimal("pace_ms", PACE.as_secs_f64() * 1_000.0);
    out.u64("host_steps", host_steps);
    out.decimal("host_ticks_per_second", host_steps as f64 / host_elapsed.as_secs_f64());
    out.decimal("agent_wall_ms", wall.as_secs_f64() * 1_000.0);
    out.u64("final_tick", u64::from(report.final_tick));
    out.u64("events_received", report.events_received);
    out.bool("events_contiguous", report.events_contiguous);
    out.u64("duplicate_events", report.duplicate_events);
    out.bool("every_tick_announced_once", announced.len() == hashes.len() && report.tick_hashes.len() as usize == hashes.len() - 1);
    out.bool("announced_hashes_match_the_world", first_divergent_tick(&announced, &hashes).is_none() && announced.len() == hashes.len());
    out.u64("commands_sent", report.commands_sent);
    out.u64("outcomes_applied", report.outcomes_applied);
    out.u64("outcomes_stale", report.outcomes_stale);
    out.bool("reconnected", report.reconnected);
    out.decimal("disconnected_ms", PACED_DISCONNECT_FOR.as_secs_f64() * 1_000.0);
    out.u64("log_commands", log.command_count() as u64);
    out.u64("log_bytes", log.bytes().len() as u64);
    match replayed {
        Ok(replayed) => {
            out.bool("replay_matches", first_divergent_tick(&replayed, &hashes).is_none() && replayed.len() == hashes.len());
            out.i64("replay_first_divergent_tick", first_divergent_tick(&replayed, &hashes).map_or(-1, |t| t as i64));
        }
        Err(error) => {
            out.bool("replay_matches", false);
            out.string("replay_error", &error.to_string());
        }
    }
    if let Some(error) = &report.error {
        out.string("error", error);
    }
    out.object_end();
}

// ---------------------------------------------------------------------------
// The compatible change
// ---------------------------------------------------------------------------

fn run_compatibility(out: &mut Json, scenario: &Scenario, workers: usize) {
    let config = WorldConfig::new(scenario.size, workers);
    let v1 = Script::version_1(scenario.ticks, BATCH);
    let v2 = Script { scale: Some(SCALE), ..v1.clone() };
    let (oracle_v1, _) = scripted_oracle(&config, &v1);
    let (oracle_v2, _) = scripted_oracle(&config, &v2);
    out.key("compatibility");
    out.object_start();
    out.hex("oracle_run_hash_unscaled", run_hash(&oracle_v1));
    out.hex("oracle_run_hash_scaled", run_hash(&oracle_v2));
    out.bool("scale_changes_the_hashes", oracle_v1 != oracle_v2);
    out.key("runs");
    out.array_start();
    // The old script against the new server, in process: the default fills the field.
    {
        let server = Arc::new(Mutex::new(Server::new(config, 2)));
        let mut client = MessageClient::new(in_process_connector(server.clone())).expect("connects");
        let outcome = run_path("old-script-new-server", "in-process-messages", server, &mut client, &v1, config, 2);
        write_path(out, &outcome, &oracle_v1, scenario.ticks);
    }
    // The new script against the new server, in process and over the socket.
    {
        let server = Arc::new(Mutex::new(Server::new(config, 2)));
        let mut client = MessageClient::new(in_process_connector(server.clone())).expect("connects");
        let outcome = run_path("new-script-new-server", "in-process-messages", server, &mut client, &v2, config, 2);
        write_path(out, &outcome, &oracle_v2, scenario.ticks);
    }
    {
        let server = Arc::new(Mutex::new(Server::new(config, 2)));
        let listening = serve_tcp(server.clone()).expect("a loopback port");
        let mut client = MessageClient::new(tcp_connector(listening.address.clone())).expect("connects");
        let outcome = run_path("new-script-new-server", listening.transport, server, &mut client, &v2, config, 2);
        write_path(out, &outcome, &oracle_v2, scenario.ticks);
        drop(client);
        listening.stop();
    }
    out.array_end();
    // The new script against the old server: the breaking direction, named.
    {
        let server = Arc::new(Mutex::new(Server::new(config, 1)));
        let mut client = MessageClient::new(in_process_connector(server.clone())).expect("connects");
        let short = Script { total_ticks: BATCH, ..v2.clone() };
        let report = run_stepped(&mut client, &short);
        out.key("new_script_old_server");
        out.object_start();
        out.bool("rejected", report.error.is_some());
        out.string("error", report.error.as_deref().unwrap_or(""));
        out.object_end();
    }
    // The probes: an unknown command and an unknown field, over the socket.
    {
        let server = Arc::new(Mutex::new(Server::new(config, 2)));
        let listening = serve_tcp(server.clone()).expect("a loopback port");
        let mut client = MessageClient::new(tcp_connector(listening.address.clone())).expect("connects");
        let probing = Script { total_ticks: BATCH, probe_errors: true, ..v1.clone() };
        let report = run_stepped(&mut client, &probing);
        drop(client);
        listening.stop();
        out.key("probes");
        out.array_start();
        for error in &report.probed_errors {
            out.object_start();
            out.string("code", &error.code);
            out.string("message", &error.message);
            for (key, value) in &error.context {
                out.string(key, value);
            }
            out.object_end();
        }
        out.array_end();
        out.bool("probes_left_the_run_intact", report.error.is_none() && report.final_tick == BATCH);
    }
    out.object_end();
}

fn write_description(out: &mut Json) {
    out.key("description");
    out.object_start();
    for version in [1, 2] {
        let d = surface::describe(version);
        out.key(&format!("version_{version}"));
        out.object_start();
        out.u64("bytes", d.encode().len() as u64);
        out.u64("structures", d.structs.len() as u64);
        out.string("commands", &d.commands.iter().map(|c| format!("{} v{}", c.name, c.version)).collect::<Vec<_>>().join(", "));
        out.string("queries", &d.queries.iter().map(|q| format!("{} v{}", q.name, q.version)).collect::<Vec<_>>().join(", "));
        out.string("events", &d.events.iter().map(|e| format!("{} v{}", e.name, e.version)).collect::<Vec<_>>().join(", "));
        out.string(
            "kick_fields",
            &d.structure("kick_parameters").expect("described").fields.iter().map(|f| format!("{}{}", f.name, if f.default.is_some() { " (default)" } else { "" })).collect::<Vec<_>>().join(", "),
        );
        out.object_end();
    }
    out.object_end();
}

fn report(out: &mut Json, scenario: &Scenario, worker_counts: &[usize]) {
    out.u64("workload_version", u64::from(reference_workload::VERSION));
    out.string("size", scenario.size.name());
    out.u64("ticks", u64::from(scenario.ticks));
    out.u64("batch", u64::from(BATCH));
    out.u64("kick_every", 10);
    out.u64("watched_entities", 16);
    write_description(out);
    out.key("runs");
    out.array_start();
    for &workers in worker_counts {
        out.object_start();
        run_paths(out, scenario, workers);
        out.object_end();
    }
    out.array_end();
    let largest = worker_counts.iter().copied().filter(|&w| w <= 4).max().unwrap_or(1);
    run_paced_scenario(out, scenario, largest);
    run_compatibility(out, scenario, largest);
}

fn main() {
    let cell = std::env::args().nth(1).unwrap_or_else(|| "unspecified".to_string());
    let mut out = Json::new();
    out.object_start();
    out.u64("schema_version", SCHEMA_VERSION);
    out.string("spike", SPIKE);
    out.string("cell", &cell);
    write_platform_and_build(&mut out);
    let counts = worker_counts();
    out.key("worker_counts");
    out.array_start();
    for &w in &counts {
        out.object_start();
        out.u64("workers", w as u64);
        out.object_end();
    }
    out.array_end();
    report(&mut out, &MEASURED, &counts);
    out.object_end();
    println!("{}", out.finish());
}

#[cfg(test)]
mod tests {
    use super::*;

    const SMALL: Scenario = Scenario { size: Size::Small, ticks: 40, disconnect_at: 20, paced_disconnect_at: 10 };

    #[test]
    fn the_oracle_and_the_world_agree_with_kicks_decided_from_observations() {
        let config = WorldConfig::new(Size::Small, 3);
        let script = Script::version_1(40, 7);
        let (hashes, records) = scripted_oracle(&config, &script);
        assert_eq!(records.iter().filter(|r| !r.is_empty()).count(), 4, "kicks at ticks 1, 11, 21 and 31");
        assert_eq!(records[0].len(), 16);
        let mut world = World::new(config);
        world.step(records.clone()).unwrap();
        let ours: Vec<u64> = (0..=40).map(|t| world.tick_hash(t).unwrap()).collect();
        assert_eq!(ours, hashes);
        let scaled = Script { scale: Some(0.5), ..script };
        let (scaled_hashes, _) = scripted_oracle(&config, &scaled);
        assert_ne!(scaled_hashes, hashes);
    }

    #[test]
    fn every_path_matches_the_oracle_and_replays_on_the_small_size() {
        let mut out = Json::new();
        out.object_start();
        report(&mut out, &SMALL, &[1, 3]);
        out.object_end();
        let json = out.finish();
        for check in ["matches_oracle", "replay_matches", "events_contiguous", "step_hashes_match_events", "retried_key_answered_alike", "every_tick_announced_once", "announced_hashes_match_the_world", "probes_left_the_run_intact", "scale_changes_the_hashes"] {
            assert!(!json.contains(&format!("\"{check}\":false")), "{check} fails: {json}");
        }
        assert!(json.contains("\"reconnected\":true"), "{json}");
        assert!(json.contains("\"duplicate_events\":0"));
        assert!(!json.contains("\"duplicate_events\":1"));
        assert!(json.contains("\"code\":\"unknown_command\"") && json.contains("\"code\":\"unknown_field\""), "{json}");
        assert!(json.contains("\"rejected\":true"), "{json}");
        assert!(json.contains("`scale`"), "the breaking direction names the field: {json}");
        assert!(!json.contains("\"error\":\"transport"), "{json}");
    }
}
