//! SyncTest on the golden replay `golden/spawner.replay`: a run that mispredicts and rolls back
//! every tick matches the straight run at every tick, at 1 and 8 threads.

use ludenea::app::replay::{Replay, load_replay};
use ludenea::net::sync_test;

const GOLDEN: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../golden/spawner.replay");
const WINDOW: u32 = 8;

#[test]
fn boxes_passes_synctest() {
    let replay = Replay::decode(&std::fs::read(GOLDEN).unwrap()).unwrap();
    let ticks = u64::try_from(replay.hashes.len() - 1).unwrap();
    for threads in [1, 8] {
        let [mut sim, mut other] = [(), ()].map(|()| {
            let mut sim = boxes::app().build().unwrap();
            sim.set_threads(threads).unwrap();
            sim
        });
        let (inputs, edits) = load_replay(&mut sim, &replay).unwrap();
        load_replay(&mut other, &replay).unwrap();
        let report = sync_test(&mut sim, &mut other, &inputs, &edits, ticks, WINDOW)
            .unwrap_or_else(|error| panic!("{threads} threads: {error}"));
        assert_eq!((report.ticks, report.rollbacks), (ticks, ticks));
        assert_eq!(
            other.hash(),
            *replay.hashes.last().unwrap(),
            "{threads} threads"
        );
    }
}
