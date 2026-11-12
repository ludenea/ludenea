//! A recording of one peer's world, and the round table. Throwaway, never
//! product code.
//!
//! The recording is the recording round trip spike's, in its container and
//! with its description, in its reference form: the world's creation values
//! and no state. What is extended is the inputs: that spike's recording holds
//! one input record per tick, from one source; a peer's world receives the
//! inputs of every peer, so a tick holds one input record per source, in the
//! order they were applied in. A kick the peer applied and never sent is a
//! record of its own, under a source no peer has.
//!
//! A replay creates the world from the file alone, steps each tick with the
//! tick's records in file order, and compares each recorded hash with the
//! replayed one. It knows nothing of the other peers, of the relay or of the
//! links.

use reference_workload::VERSION;
use test_recording_round_trip::recording::{InputRecord, Recording, Start};
use test_recording_round_trip::world::{Kick as RecordedKick, SNAPSHOT_FORMAT};
use workload_lockstep::world::{Kick, Size, World, WorldConfig};

use crate::protocol::{LOCAL_SOURCE, Peer};

/// The recording of a peer's world up to its confirmed tick: what the world
/// received and the hash of each confirmed tick. The ticks a peer that rolls
/// back stepped on predictions leave no trace in it.
pub fn of_peer(peer: &Peer, test: &str, cell: &str, profile: &str) -> Recording {
    let config = peer.config();
    let recorded = |kicks: &[Kick]| kicks.iter().map(|kick| RecordedKick { index: kick.index, generation: kick.generation, delta_vx: kick.delta_vx, delta_vy: kick.delta_vy }).collect();
    let mut inputs = Vec::new();
    for tick in 1..=peer.confirmed() {
        for (source, kicks) in peer.record(tick).iter().enumerate() {
            inputs.push(InputRecord { tick, source: source as u32, kicks: recorded(kicks) });
        }
        if let Some((_, kick)) = peer.injection().filter(|(at, _)| *at == tick) {
            inputs.push(InputRecord { tick, source: LOCAL_SOURCE, kicks: recorded(&[kick]) });
        }
    }
    Recording {
        workload_version: VERSION,
        snapshot_format: SNAPSHOT_FORMAT,
        seed: config.seed,
        entity_count: config.size.entity_count() as u32,
        wander: config.wander,
        start_tick: 0,
        start_hash: peer.hashes()[0],
        end_tick: peer.confirmed(),
        test: format!("{test}, peer {}", peer.id),
        writer_cell: cell.to_string(),
        writer_profile: profile.to_string(),
        start: Start::Recipe(Vec::new()),
        inputs,
        hashes: (1..=peer.confirmed()).map(|tick| (tick, peer.hashes()[tick as usize])).collect(),
    }
}

/// What a replay alone found.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Replayed {
    /// The last tick stepped and its hash.
    pub last: (u32, u64),
    /// The recorded hashes compared, all equal to the replayed ones.
    pub compared: u32,
}

/// Replays a recording's file alone, to its last tick or to `through`, and
/// compares every recorded hash on the way. An error names the first tick
/// whose replayed hash is not the recorded one, or why the file was refused.
pub fn replay_alone(file: &[u8], through: Option<u32>) -> Result<Replayed, String> {
    let recording = Recording::decode(file).map_err(|error| error.0)?;
    if recording.workload_version != VERSION {
        return Err(format!("workload version {}, this build runs {VERSION}", recording.workload_version));
    }
    if !matches!(&recording.start, Start::Recipe(preparation) if preparation.is_empty()) || recording.start_tick != 0 {
        return Err("not a recording in the reference form from the world's creation".to_string());
    }
    let size = Size::ALL.iter().copied().find(|size| size.entity_count() == recording.entity_count as usize).ok_or_else(|| format!("{} entities is not a workload size", recording.entity_count))?;
    let mut world = World::new(WorldConfig { seed: recording.seed, wander: recording.wander, ..WorldConfig::new(size, 1) });
    if world.hash() != recording.start_hash {
        return Err("the created world does not hold the recorded starting hash".to_string());
    }
    let last_tick = through.unwrap_or(recording.end_tick).min(recording.end_tick);
    let mut records = recording.inputs.iter().peekable();
    let mut hashes = recording.hashes.iter().peekable();
    let mut compared = 0;
    for tick in 1..=last_tick {
        let mut kicks = Vec::new();
        while let Some(record) = records.next_if(|record| record.tick == tick) {
            kicks.extend(record.kicks.iter().map(|kick| Kick { index: kick.index, generation: kick.generation, delta_vx: kick.delta_vx, delta_vy: kick.delta_vy }));
        }
        if records.peek().is_some_and(|record| record.tick < tick) {
            return Err(format!("an input record of tick {} follows the records of tick {tick}", records.peek().map_or(0, |record| record.tick)));
        }
        world.step(vec![kicks]).map_err(|error| error.to_string())?;
        while let Some((_, recorded)) = hashes.next_if(|(at, _)| *at == tick) {
            if *recorded != world.hash() {
                return Err(format!("tick {tick} replays to {:016x} where {recorded:016x} was recorded", world.hash()));
            }
            compared += 1;
        }
    }
    Ok(Replayed { last: (world.tick(), world.hash()), compared })
}

/// The round table as a record keeps it: one line per round, each peer's
/// present tick and confirmed tick.
pub fn round_table_text(table: &[Vec<(u32, u32)>]) -> String {
    let mut text = String::new();
    for (index, row) in table.iter().enumerate() {
        text.push_str(&format!("round {}", index + 1));
        for (tick, confirmed) in row {
            text.push_str(&format!(" {tick}/{confirmed}"));
        }
        text.push('\n');
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fixture::{Fixture, Order, Schedule, Settings, Until};
    use crate::link::Sharing;
    use workload_lockstep::lockstep::LinkSettings;

    fn fixture(rollback: Option<(usize, u32)>, injection: Option<(usize, u32)>) -> Fixture {
        Fixture::new(Settings {
            peers: 3,
            config: WorldConfig::new(Size::Small, 1),
            ticks: 20,
            delay: 2,
            link: LinkSettings { latency: 1, jitter_max: 1, loss: 0.05 },
            link_seed: 3,
            sharing: Sharing::PerDirection,
            keep_log: true,
            rollback,
            injection,
            schedule: Schedule::default(),
            order: Order::Fixed,
            until: Until::Finished,
        })
    }

    #[test]
    fn each_peers_recording_replays_alone_the_peer_that_rolls_back_among_them() {
        let mut fixture = fixture(Some((2, 6)), None);
        let outcome = fixture.run();
        assert!(outcome.finished);
        for id in 0..3 {
            let file = fixture.peer(id, |peer| of_peer(peer, "a test", "a cell", "a profile")).encode();
            let replayed = replay_alone(&file, None).expect("the recording replays alone");
            assert_eq!(replayed.compared, 20);
            assert_eq!(replayed.last, (20, fixture.peer(id, |peer| peer.hashes()[20])));
        }
    }

    #[test]
    fn the_round_table_places_each_world_at_its_confirmed_tick() {
        let mut fixture = fixture(Some((2, 6)), None);
        let outcome = fixture.run();
        let row = &outcome.round_table[outcome.round_table.len() / 2];
        assert!(row[2].0 > row[2].1, "the peer that rolls back stands ahead of its confirmed tick");
        for (id, (_, confirmed)) in row.iter().enumerate() {
            let file = fixture.peer(id, |peer| of_peer(peer, "a test", "a cell", "a profile")).encode();
            let replayed = replay_alone(&file, Some(*confirmed)).expect("the recording replays alone");
            assert_eq!(replayed.last, (*confirmed, fixture.peer(id, |peer| peer.hashes()[*confirmed as usize])));
        }
        assert!(round_table_text(&outcome.round_table).starts_with("round 1 0/0 0/0 "));
    }

    #[test]
    fn a_recording_holds_the_kick_its_peer_never_sent_and_a_changed_one_is_caught() {
        let mut fixture = fixture(None, Some((1, 5)));
        let outcome = fixture.run();
        assert_eq!(outcome.desync.map(|desync| desync.tick), Some(5));
        let recording = fixture.peer(1, |peer| of_peer(peer, "a test", "a cell", "a profile"));
        assert!(recording.inputs.iter().any(|record| record.source == LOCAL_SOURCE && record.tick == 5));
        assert!(replay_alone(&recording.encode(), None).is_ok(), "the peer's own recording replays to its own hashes");
        let mut changed = recording.clone();
        changed.inputs.retain(|record| record.source != LOCAL_SOURCE);
        assert!(replay_alone(&changed.encode(), None).is_err_and(|reason| reason.starts_with("tick 5 ")));
    }
}
