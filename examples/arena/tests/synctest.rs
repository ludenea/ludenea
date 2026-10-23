//! SyncTest on the golden replay `golden/arena.replay`: a run that mispredicts and rolls back
//! every tick matches the straight run at every tick, at 1 and 8 threads.

use ludenea::app::replay::{Replay, load_replay};
use ludenea::net::sync_test;

const GOLDEN: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../golden/arena.replay");
const WINDOW: u32 = 8;

#[test]
fn arena_passes_synctest() {
    let replay = Replay::decode(&std::fs::read(GOLDEN).unwrap()).unwrap();
    let ticks = u64::try_from(replay.hashes.len() - 1).unwrap();
    for threads in [1, 8] {
        let [mut sim, mut other] = [(), ()].map(|()| {
            let mut sim = arena::app().build().unwrap();
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

#[test]
fn synctest_command_passes_on_the_arena_replay() {
    let dump = tempfile::tempdir().unwrap();
    let dump_dir = dump.path().to_str().unwrap();
    let arguments = ["arena", "synctest", GOLDEN, "--dump-dir", dump_dir];
    let mut said = Vec::new();
    let code = ludenea::cli::run(arena::app, &arguments, &mut said);
    let said = String::from_utf8_lossy(&said);
    assert_eq!(
        (code, said.as_ref()),
        (0, "synctest passed: 600 ticks, 600 rollbacks\n")
    );
    assert_eq!(std::fs::read_dir(dump.path()).unwrap().count(), 0);
}
