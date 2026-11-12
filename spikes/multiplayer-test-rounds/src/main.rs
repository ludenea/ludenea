//! Multiplayer rounds spike, for the game testing research. Throwaway, never
//! product code.
//!
//! Several peers of an agreed game are hosted as worlds of one process and
//! driven by rounds, through a relay, over seeded links. The spike measures,
//! instead of assuming, whether the order of the peers in a round changes
//! nothing when a link delivers only at its advance and shares nothing
//! between its two directions; whether a schedule of stalls, suspensions,
//! losses and clock rates repeats its history; whether the fixture's oracle
//! reports an injected desync at its first divergent tick; whether each
//! world's recording replays alone and the round table places the replayed
//! worlds; and what the fixture's preparation, a round and the link log cost.
//!
//! `link.rs` holds the link, `protocol.rs` the peers and the relay,
//! `fixture.rs` the rounds, the schedule and the oracle, `recording.rs` the
//! recordings and the round table. Run with a cell label, the spike prints
//! one JSON document and writes the recordings of one run under
//! `out/<cell>/`.

mod fixture;
mod link;
mod protocol;
mod recording;

use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use reference_workload::{Json, write_platform_and_build};
use workload_lockstep::lockstep::LinkSettings;
use workload_lockstep::world::{Size, WorldConfig};

use fixture::{Event, Fixture, Order, Outcome, Rates, Schedule, Scheduled, Settings, Until};
use link::{DOWN, LOG_ENTRY_BYTES, LOG_RING, Sharing, UP};

const SCHEMA_VERSION: u64 = 1;
const SPIKE: &str = "multiplayer-test-rounds";
const DELAY: u32 = 2;
const LINK_SEED: u64 = 0x4c69_6e6b_5365_6564;
/// A link that draws: a latency of one frame, a jitter of up to two, and one
/// message in twenty lost.
const DRAWING: LinkSettings = LinkSettings { latency: 1, jitter_max: 2, loss: 0.05 };
/// The lockstep spike's setting with no latency, which here means one frame.
const NO_LATENCY: LinkSettings = LinkSettings { latency: 0, jitter_max: 0, loss: 0.0 };
const ORDER_PEERS: [usize; 3] = [2, 4, 16];
const ORDER_TICKS: u32 = 60;
const PERMUTATIONS: u64 = 20;
const WORKER_BUDGETS: [usize; 2] = [1, 2];
const SCHEDULE_PEERS: usize = 4;
const SCHEDULE_TICKS: u32 = 90;
const SCHEDULE_RUNS: usize = 3;
const DRAWN_SEED: u64 = 0x5363_6865_6475_6c65;
const DRAWN_RATES: Rates = Rates { stall: 12, suspension: 8, loss_burst: 16, total_loss: 8, clock_rate: 8 };
const ORACLE_PEERS: usize = 4;
const ORACLE_TICKS: u32 = 60;
const ORACLE_INJECTION_TICK: u32 = 23;
const ROLLBACK_WINDOW: u32 = 8;
const PREPARATION_PEERS: [usize; 3] = [2, 4, 16];
const PREPARATION_RUNS: usize = 5;
const PACE_PEERS: [usize; 4] = [2, 4, 16, 64];
const PACE_TICKS: u32 = 60;
const PACE_RUNS: usize = 3;
const LOG_COST_RUNS: usize = 5;
const LOG_ALONE_MESSAGES: u64 = 200_000;

fn profile() -> &'static str {
    if cfg!(debug_assertions) { "dev" } else { "release" }
}

fn output_dir(cell: &str) -> PathBuf {
    let candidate = Path::new("out").join(cell);
    if fs::create_dir_all(&candidate).is_ok() {
        return candidate;
    }
    let fallback = env::temp_dir().join(format!("{SPIKE}-{cell}"));
    let _ = fs::create_dir_all(&fallback);
    fallback
}

fn settings(peers: usize, workers: usize, ticks: u32, link: LinkSettings) -> Settings {
    Settings {
        peers,
        config: WorldConfig::new(Size::Small, workers),
        ticks,
        delay: DELAY,
        link,
        link_seed: LINK_SEED,
        sharing: Sharing::PerDirection,
        keep_log: true,
        rollback: None,
        injection: None,
        schedule: Schedule::default(),
        order: Order::Fixed,
        until: Until::Finished,
    }
}

fn run(settings: Settings) -> Outcome {
    Fixture::new(settings).run()
}

fn ms(duration: Duration) -> f64 {
    duration.as_secs_f64() * 1_000.0
}

fn link_text(link: &LinkSettings) -> String {
    format!("latency {} frame(s), jitter up to {}, loss {}", link.latency, link.jitter_max, link.loss)
}

fn write_history(out: &mut Json, outcome: &Outcome) {
    out.hex("tick_hashes", outcome.tick_hashes);
    out.hex("round_table_hash", outcome.round_table_hash);
    out.hex("link_log_hash", outcome.link_log_hash);
    out.u64("rounds", outcome.rounds);
    out.bool("finished", outcome.finished);
    out.bool("every_peer_one_run_hash", outcome.peers.iter().all(|peer| peer.run_hash == outcome.peers[0].run_hash));
    out.hex("run_hash", outcome.peers[0].run_hash);
    out.string("last_confirmed_ticks", &outcome.peers.iter().map(|peer| peer.confirmed.to_string()).collect::<Vec<_>>().join(" "));
    out.u64("messages", outcome.link.sent);
    out.u64("messages_lost", outcome.link.lost);
}

/// The order of the peers: under the two conditions, twenty shuffles and a
/// round run at once against the fixed order; with one random state for both
/// directions of a link, the same twenty shuffles. Returns whether the order
/// changed nothing under the conditions, and whether it changed something
/// without the second.
fn write_order(out: &mut Json) -> (bool, bool) {
    let (mut free, mut needed) = (true, true);
    out.key("order_of_the_peers");
    out.array_start();
    for peers in ORDER_PEERS {
        for workers in WORKER_BUDGETS {
            let base = settings(peers, workers, ORDER_TICKS, DRAWING);
            let fixed = run(base.clone());
            let shuffled: Vec<Outcome> = (1..=PERMUTATIONS).map(|seed| run(Settings { order: Order::Shuffled(seed), ..base.clone() })).collect();
            let at_once = run(Settings { order: Order::AtOnce, ..base.clone() });
            let same = shuffled.iter().filter(|outcome| outcome.history() == fixed.history()).count() as u64;
            let shared = Settings { sharing: Sharing::OneRandomState, ..base.clone() };
            let shared_fixed = run(shared.clone());
            let shared_shuffled: Vec<Outcome> = (1..=PERMUTATIONS).map(|seed| run(Settings { order: Order::Shuffled(seed), ..shared.clone() })).collect();
            let changed = |differs: &dyn Fn(&Outcome) -> bool| shared_shuffled.iter().filter(|outcome| differs(outcome)).count() as u64;
            let (log_changed, table_changed, ticks_changed) = (
                changed(&|outcome| outcome.link_log_hash != shared_fixed.link_log_hash),
                changed(&|outcome| outcome.round_table_hash != shared_fixed.round_table_hash),
                changed(&|outcome| outcome.tick_hashes != shared_fixed.tick_hashes),
            );
            free &= fixed.finished && same == PERMUTATIONS && at_once.history() == fixed.history();
            needed &= log_changed > 0;
            out.object_start();
            out.u64("peers", peers as u64);
            out.u64("workers_per_world", workers as u64);
            out.string("link", &link_text(&DRAWING));
            out.u64("ticks", u64::from(ORDER_TICKS));
            write_history(out, &fixed);
            out.u64("permutations", PERMUTATIONS);
            out.u64("permutations_with_the_fixed_order_history", same);
            out.bool("round_run_at_once_gives_the_fixed_order_history", at_once.history() == fixed.history());
            out.u64("threads_of_the_round_run_at_once", at_once.threads as u64);
            out.key("one_random_state_for_both_directions");
            out.object_start();
            write_history(out, &shared_fixed);
            out.u64("permutations_that_change_the_link_log", log_changed);
            out.u64("permutations_that_change_the_round_table", table_changed);
            out.u64("permutations_that_change_the_tick_hashes", ticks_changed);
            out.object_end();
            out.object_end();
        }
    }
    out.array_end();
    (free, needed)
}

/// A schedule that holds one event of each kind.
fn written_schedule() -> Schedule {
    Schedule(vec![
        Scheduled { round: 10, event: Event::Stall { peer: 0, rounds: 6 } },
        Scheduled { round: 20, event: Event::LossBurst { link: 1, direction: DOWN, rounds: 8, per_mille: 600 } },
        Scheduled { round: 20, event: Event::LossBurst { link: 2, direction: UP, rounds: 8, per_mille: 600 } },
        Scheduled { round: 32, event: Event::ClockRate { peer: 2, per_mille: 900 } },
        Scheduled { round: 44, event: Event::Suspension { peer: 1, rounds: 5 } },
        Scheduled { round: 58, event: Event::TotalLoss { link: 3, rounds: 8 } },
        Scheduled { round: 70, event: Event::ClockRate { peer: 2, per_mille: 1_100 } },
    ])
}

/// The schedule: a written one and a drawn one, each run several times, the
/// drawn one also from its recorded events. Returns whether every run of
/// each gave one history.
fn write_schedules(out: &mut Json, output: &Path) -> bool {
    let mut repeats = true;
    let base = settings(SCHEDULE_PEERS, 1, SCHEDULE_TICKS, DRAWING);
    let unscheduled = run(base.clone());
    let drawn = Schedule::drawn(DRAWN_SEED, &DRAWN_RATES, SCHEDULE_PEERS, u64::from(SCHEDULE_TICKS) - 10);
    out.key("schedules");
    out.array_start();
    for (name, schedule) in [("written", written_schedule()), ("drawn", drawn)] {
        let text = schedule.text();
        let _ = fs::write(output.join(format!("schedule-{name}.txt")), &text);
        let runs: Vec<Outcome> = (0..SCHEDULE_RUNS).map(|_| run(Settings { schedule: schedule.clone(), ..base.clone() })).collect();
        let one_history = runs.iter().all(|outcome| outcome.finished && outcome.desync.is_none() && outcome.history() == runs[0].history());
        let recorded = Schedule::parse(&text);
        let replayed = recorded.as_ref().ok().map(|recorded| run(Settings { schedule: recorded.clone(), ..base.clone() }));
        let from_record = recorded.as_ref() == Ok(&schedule) && replayed.as_ref().is_some_and(|outcome| outcome.history() == runs[0].history());
        repeats &= one_history && from_record;
        out.object_start();
        out.string("schedule", name);
        out.u64("peers", SCHEDULE_PEERS as u64);
        out.u64("events", schedule.0.len() as u64);
        out.string("events_as_recorded", &text.lines().collect::<Vec<_>>().join("; "));
        if name == "drawn" {
            out.hex("seed", DRAWN_SEED);
            out.string("rates_per_thousand_rounds", &format!("stall {}, suspension {}, loss burst {}, total loss {}, clock rate {}", DRAWN_RATES.stall, DRAWN_RATES.suspension, DRAWN_RATES.loss_burst, DRAWN_RATES.total_loss, DRAWN_RATES.clock_rate));
        }
        write_history(out, &runs[0]);
        out.u64("runs", SCHEDULE_RUNS as u64);
        out.bool("every_run_one_history", one_history);
        out.bool("replayed_from_its_recorded_events_gives_that_history", from_record);
        out.bool("history_differs_from_the_run_with_no_schedule", runs[0].history() != unscheduled.history());
        out.bool("tick_hashes_are_those_of_the_run_with_no_schedule", runs[0].tick_hashes == unscheduled.tick_hashes);
        out.u64("rounds_with_no_schedule", unscheduled.rounds);
        out.string("stalled_rounds_per_peer", &runs[0].peers.iter().map(|peer| peer.stalled_rounds.to_string()).collect::<Vec<_>>().join(" "));
        out.object_end();
    }
    out.array_end();
    repeats
}

/// The oracle: a desync injected into one peer, in every order. Returns
/// whether it was reported at the injected tick in every case and order, and
/// nothing was reported where nothing was injected.
fn write_oracle(out: &mut Json) -> bool {
    let mut holds = true;
    let orders: Vec<Order> = [Order::Fixed].into_iter().chain((1..=5).map(Order::Shuffled)).chain([Order::AtOnce]).collect();
    let cases: [(&str, Option<(usize, u32)>, Option<usize>); 4] = [
        ("no desync injected, one peer rolls back", Some((1, ROLLBACK_WINDOW)), None),
        ("injected into a lockstep peer, every peer in lockstep", None, Some(2)),
        ("injected into a lockstep peer, another peer rolls back", Some((1, ROLLBACK_WINDOW)), Some(2)),
        ("injected into the peer that rolls back", Some((1, ROLLBACK_WINDOW)), Some(1)),
    ];
    out.key("oracle");
    out.array_start();
    for (name, rollback, injected) in cases {
        let outcomes: Vec<Outcome> =
            orders.iter().map(|order| run(Settings { rollback, injection: injected.map(|peer| (peer, ORACLE_INJECTION_TICK)), order: *order, ..settings(ORACLE_PEERS, 1, ORACLE_TICKS, DRAWING) })).collect();
        let reported: Vec<Option<u32>> = outcomes.iter().map(|outcome| outcome.desync.map(|desync| desync.tick)).collect();
        let expected = injected.map(|_| ORACLE_INJECTION_TICK);
        let as_expected = reported.iter().all(|tick| *tick == expected) && outcomes.iter().all(|outcome| outcome.desync.is_none_or(|desync| Some(desync.peers.0) == injected || Some(desync.peers.1) == injected));
        holds &= as_expected;
        out.object_start();
        out.string("case", name);
        out.u64("peers", ORACLE_PEERS as u64);
        out.string("injected_at_tick", &expected.map_or("none".to_string(), |tick| tick.to_string()));
        out.string("injected_into_peer", &injected.map_or("none".to_string(), |peer| peer.to_string()));
        out.u64("orders", orders.len() as u64);
        out.string("reported_at_tick_per_order", &reported.iter().map(|tick| tick.map_or("none".to_string(), |tick| tick.to_string())).collect::<Vec<_>>().join(" "));
        out.string("reported_at_round_per_order", &outcomes.iter().map(|outcome| outcome.desync.map_or("none".to_string(), |desync| desync.round.to_string())).collect::<Vec<_>>().join(" "));
        out.string("rollbacks_per_order", &outcomes.iter().map(|outcome| outcome.peers.iter().map(|peer| peer.rollback.rollbacks).sum::<u64>().to_string()).collect::<Vec<_>>().join(" "));
        out.bool("as_expected_in_every_order", as_expected);
        out.object_end();
    }
    out.array_end();
    holds
}

/// The recordings: each peer's recording of one run, the peer that rolls
/// back among them, replayed alone, and the round table. Returns whether
/// every recording replayed alone to its hashes and the round table placed
/// every replayed world at its recorded hash.
fn write_recordings(out: &mut Json, cell: &str, output: &Path) -> bool {
    let mut fixture = Fixture::new(Settings { rollback: Some((1, ROLLBACK_WINDOW)), schedule: written_schedule(), ..settings(SCHEDULE_PEERS, 1, SCHEDULE_TICKS, DRAWING) });
    let outcome = fixture.run();
    let directory = output.join("recordings");
    let _ = fs::create_dir_all(&directory);
    let _ = fs::write(directory.join("round-table.txt"), recording::round_table_text(&outcome.round_table));
    let link_log = fixture.link_log_text();
    let _ = fs::write(directory.join("link-log.txt"), &link_log);
    let files: Vec<Vec<u8>> = (0..SCHEDULE_PEERS).map(|id| fixture.peer(id, |peer| recording::of_peer(peer, "multiplayer rounds spike, one peer rolls back", cell, profile())).encode()).collect();
    let mut holds = outcome.finished;
    out.key("recordings");
    out.object_start();
    out.u64("peers", SCHEDULE_PEERS as u64);
    out.u64("peer_that_rolls_back", 1);
    out.u64("rollbacks", outcome.peers[1].rollback.rollbacks);
    out.u64("ticks", u64::from(SCHEDULE_TICKS));
    out.u64("rounds", outcome.rounds);
    out.key("each_alone");
    out.array_start();
    for (id, file) in files.iter().enumerate() {
        let _ = fs::write(directory.join(format!("peer-{id}.recording")), file);
        let replayed = recording::replay_alone(file, None);
        let expected = (SCHEDULE_TICKS, fixture.peer(id, |peer| peer.hashes()[SCHEDULE_TICKS as usize]));
        let verified = replayed.as_ref().is_ok_and(|replayed| replayed.last == expected && replayed.compared == SCHEDULE_TICKS);
        holds &= verified;
        out.object_start();
        out.u64("peer", id as u64);
        out.u64("file_bytes", file.len() as u64);
        out.bool("replays_alone_to_its_recorded_hashes", verified);
        if let Err(reason) = &replayed {
            out.string("error", reason);
        }
        out.object_end();
    }
    out.array_end();
    // Every tenth round and the last: each world replayed alone to the
    // confirmed tick the table gives it, against the hash recorded for it.
    let rounds: Vec<usize> = (9..outcome.round_table.len()).step_by(10).chain([outcome.round_table.len() - 1]).collect();
    let (mut placed, mut ahead) = (0_u64, 0_u64);
    let mut misplaced = 0_u64;
    for round in &rounds {
        for (id, (tick, confirmed)) in outcome.round_table[*round].iter().enumerate() {
            let expected = (*confirmed, fixture.peer(id, |peer| peer.hashes()[*confirmed as usize]));
            match recording::replay_alone(&files[id], Some(*confirmed)) {
                Ok(replayed) if replayed.last == expected => placed += 1,
                _ => misplaced += 1,
            }
            ahead += u64::from(tick > confirmed);
        }
    }
    holds &= misplaced == 0;
    out.u64("round_table_rounds_checked", rounds.len() as u64);
    out.u64("worlds_placed_at_their_recorded_hash", placed);
    out.u64("worlds_not_placed", misplaced);
    out.u64("of_which_stood_ahead_of_their_confirmed_tick", ahead);
    out.u64("round_table_bytes", recording::round_table_text(&outcome.round_table).len() as u64);
    out.u64("link_log_artifact_entries", link_log.lines().count() as u64);
    out.u64("link_log_artifact_bytes", link_log.len() as u64);
    out.object_end();
    holds
}

fn spread(values: &mut [f64]) -> (f64, f64, f64) {
    values.sort_by(f64::total_cmp);
    (values[values.len() / 2], values[0], values[values.len() - 1])
}

/// The fixture's preparation: from the creations to a starting point after
/// the first agreed tick, with peers of 1 000 entities.
fn write_preparation(out: &mut Json) {
    out.key("fixture_preparation");
    out.array_start();
    for peers in PREPARATION_PEERS {
        let (mut creations, mut rounds, mut totals) = (Vec::new(), Vec::new(), Vec::new());
        let mut round_count = 0;
        for _ in 0..PREPARATION_RUNS {
            let started = Instant::now();
            let mut fixture = Fixture::new(Settings { until: Until::FirstAgreedTick, ..settings(peers, 1, ORDER_TICKS, NO_LATENCY) });
            let created = started.elapsed();
            let outcome = fixture.run();
            round_count = outcome.rounds;
            creations.push(ms(created));
            rounds.push(ms(outcome.elapsed));
            totals.push(ms(started.elapsed()));
        }
        let (creation, run, total) = (spread(&mut creations), spread(&mut rounds), spread(&mut totals));
        out.object_start();
        out.u64("peers", peers as u64);
        out.u64("entities_per_peer", Size::Small.entity_count() as u64);
        out.string("link", &link_text(&NO_LATENCY));
        out.u64("rounds_to_the_first_agreed_tick", round_count);
        out.decimal("creations_median_ms", creation.0);
        out.decimal("rounds_median_ms", run.0);
        out.decimal("total_median_ms", total.0);
        out.decimal("total_fastest_ms", total.1);
        out.decimal("total_slowest_ms", total.2);
        out.object_end();
    }
    out.array_end();
}

fn fastest(runs: usize, settings: &Settings) -> Outcome {
    (0..runs).map(|_| run(settings.clone())).min_by_key(|outcome| outcome.elapsed).expect("at least one run")
}

/// The pace: rounds per second with the peers of a round run in turn and at
/// once.
fn write_pace(out: &mut Json) {
    out.key("pace");
    out.array_start();
    for peers in PACE_PEERS {
        for (name, order) in [("in turn", Order::Fixed), ("at once", Order::AtOnce)] {
            let outcome = fastest(PACE_RUNS, &Settings { order, ..settings(peers, 1, PACE_TICKS, NO_LATENCY) });
            out.object_start();
            out.u64("peers", peers as u64);
            out.string("the_peers_of_a_round_run", name);
            out.u64("threads", outcome.threads as u64);
            out.u64("rounds", outcome.rounds);
            out.decimal("fastest_ms", ms(outcome.elapsed));
            out.decimal("rounds_per_second", outcome.rounds as f64 / outcome.elapsed.as_secs_f64());
            out.decimal("microseconds_per_peer_and_round", outcome.elapsed.as_secs_f64() * 1_000_000.0 / (outcome.rounds as f64 * peers as f64));
            out.object_end();
        }
    }
    out.array_end();
}

/// The link log: what an entry holds, and what keeping it costs a run.
fn write_link_log(out: &mut Json) {
    let base = settings(16, 1, PACE_TICKS, DRAWING);
    let kept = fastest(LOG_COST_RUNS, &base);
    let not_kept = fastest(LOG_COST_RUNS, &Settings { keep_log: false, ..base });
    out.key("link_log");
    out.object_start();
    out.u64("peers", 16);
    out.u64("entry_bytes", LOG_ENTRY_BYTES as u64);
    out.u64("ring_entries_per_link", LOG_RING as u64);
    out.u64("ring_bytes_per_link", (LOG_RING * LOG_ENTRY_BYTES) as u64);
    out.u64("messages", kept.link.sent);
    out.u64("entries_logged", kept.logged);
    out.decimal("bytes_carried_per_message", kept.link.bytes as f64 / kept.link.sent as f64);
    out.decimal("fastest_ms_with_the_log", ms(kept.elapsed));
    out.decimal("fastest_ms_without_the_log", ms(not_kept.elapsed));
    out.decimal("nanoseconds_per_message_for_the_log", (kept.elapsed.as_secs_f64() - not_kept.elapsed.as_secs_f64()) * 1_000_000_000.0 / kept.link.sent as f64);
    out.bool("same_ticks_and_round_table_without_the_log", (kept.tick_hashes, kept.round_table_hash) == (not_kept.tick_hashes, not_kept.round_table_hash));
    // A run does not resolve the log: a link alone, carrying small messages.
    let alone = |keep_log: bool| {
        (0..LOG_COST_RUNS)
            .map(|_| {
                let mut link = link::Link::new(NO_LATENCY, LINK_SEED, Sharing::PerDirection, keep_log);
                let started = Instant::now();
                for frame in 0..LOG_ALONE_MESSAGES {
                    link.send(UP, frame, vec![0; 16]);
                    link.advance(frame);
                    std::hint::black_box(link.take(UP));
                }
                started.elapsed()
            })
            .min()
            .expect("at least one run")
    };
    let (with, without) = (alone(true), alone(false));
    out.u64("messages_on_a_link_alone", LOG_ALONE_MESSAGES);
    out.decimal("nanoseconds_per_message_on_a_link_alone_with_the_log", with.as_secs_f64() * 1_000_000_000.0 / LOG_ALONE_MESSAGES as f64);
    out.decimal("nanoseconds_per_message_on_a_link_alone_without_the_log", without.as_secs_f64() * 1_000_000_000.0 / LOG_ALONE_MESSAGES as f64);
    out.object_end();
}

fn main() {
    let cell = env::args().nth(1).unwrap_or_else(|| "local".to_string());
    let output = output_dir(&cell);
    let mut out = Json::new();
    out.object_start();
    out.u64("schema_version", SCHEMA_VERSION);
    out.string("spike", SPIKE);
    out.string("cell", &cell);
    out.string("profile", profile());
    write_platform_and_build(&mut out);
    out.key("parameters");
    out.object_start();
    out.u64("entities_per_peer", Size::Small.entity_count() as u64);
    out.u64("input_delay_ticks", u64::from(DELAY));
    out.u64("tick_budget_per_round", u64::from(protocol::TICK_BUDGET));
    out.u64("kicks_per_peer_and_tick", protocol::KICKS_PER_TICK as u64);
    out.u64("rollback_window_ticks", u64::from(ROLLBACK_WINDOW));
    out.hex("link_seed", LINK_SEED);
    out.object_end();
    let (order_free, second_condition_needed) = write_order(&mut out);
    let schedule_repeats = write_schedules(&mut out, &output);
    let oracle_holds = write_oracle(&mut out);
    let recordings_hold = write_recordings(&mut out, &cell, &output);
    write_preparation(&mut out);
    write_pace(&mut out);
    write_link_log(&mut out);
    out.key("hypotheses");
    out.object_start();
    out.bool("order_of_the_peers_changes_nothing_under_the_two_conditions", order_free);
    out.bool("one_random_state_for_both_directions_lets_the_order_in", second_condition_needed);
    out.bool("schedules_repeat_their_history", schedule_repeats);
    out.bool("oracle_reports_the_desync_at_its_first_divergent_tick", oracle_holds);
    out.bool("recordings_replay_alone_and_the_round_table_places_them", recordings_hold);
    out.object_end();
    out.object_end();
    println!("{}", out.finish());
}
