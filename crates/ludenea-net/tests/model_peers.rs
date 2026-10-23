//! The property test of the session on the model game: two to four peers over seeded degraded
//! links play a match, one leaves, and every peer ends holding the same confirmed inputs, whose
//! offline replay gives every peer's state hash at every confirmed tick.

mod support;

use ludenea_app::Rng;
use ludenea_net::{EndReason, LinkConditions, PeerId, SessionState, Timeouts};
use support::{Harness, offline_hashes, sessions};

const ONE_SECOND: u64 = 1_000_000;
const SEEDS: u64 = 32;
/// The ticks the leaver runs before it leaves.
const TICKS: u64 = 300;

/// The link conditions of `seed`: latency 0 to 150 ms, jitter 0 to 30 ms, loss 0 to 100 per
/// mille, duplication 0 to 20 per mille, reordering 0 to 50 per mille by 0 to 60 ms more.
fn conditions(seed: u64) -> LinkConditions {
    let mut rng = Rng::new(seed, 1);
    let mut draw = |low: u32, high: u32| rng.range_u32(low, high + 1);
    LinkConditions {
        latency_micros: draw(0, 150_000),
        jitter_micros: draw(0, 30_000),
        loss_per_mille: u16::try_from(draw(0, 100)).unwrap(),
        duplicate_per_mille: u16::try_from(draw(0, 20)).unwrap(),
        reorder_per_mille: u16::try_from(draw(0, 50)).unwrap(),
        reorder_extra_micros: draw(0, 60_000),
        seed,
    }
}

/// Plays one match and checks it. Returns the rollbacks and the prediction misses its
/// sessions counted.
fn play(peers: u8, seed: u64) -> (u64, u64) {
    let conditions = conditions(seed);
    let case = format!("{peers} peers, seed {seed}, {conditions:?}");
    let mut harness = Harness::conditioned(sessions(peers), conditions);
    assert!(harness.run_until_running(2 * ONE_SECOND), "{case}");
    let leaver = PeerId(u8::try_from(seed % u64::from(peers)).unwrap());
    assert!(
        harness.run_until(20 * ONE_SECOND, |harness| harness.model(leaver).tick()
            >= TICKS),
        "{case}: the leaver never reached tick {TICKS}"
    );
    harness.leave(leaver);
    let ended = harness.run_until(2 * Timeouts::default().disconnect_after_micros, |harness| {
        harness
            .peers()
            .all(|peer| matches!(harness.state(peer), SessionState::Ended(_)))
    });
    assert!(
        ended,
        "{case}: {:?}",
        harness
            .peers()
            .map(|peer| harness.state(peer))
            .collect::<Vec<_>>()
    );
    // The last landing runs right after the last ending.
    harness.step();

    let SessionState::Ended(EndReason::Left { last_tick, .. }) = harness.state(leaver) else {
        panic!("{case}: the leaver ended {:?}", harness.state(leaver));
    };
    let reference = harness.confirmed(leaver);
    let offline = offline_hashes(&reference);
    assert_eq!(
        u64::try_from(reference.len()).unwrap(),
        last_tick + 1,
        "{case}"
    );
    let mut rollbacks = 0;
    let mut misses = 0;
    for peer in harness.peers() {
        assert!(
            matches!(
                harness.state(peer),
                SessionState::Ended(EndReason::Left { last_tick: tick, .. }) if tick == last_tick
            ),
            "{case}: peer {peer} ended {:?}",
            harness.state(peer)
        );
        assert_eq!(
            harness.confirmed(peer),
            reference,
            "{case}: peer {peer} confirmed other inputs"
        );
        let model = harness.model(peer);
        for (tick, expected) in (1..).zip(&offline) {
            assert_eq!(
                model.hash_at(tick),
                Some(*expected),
                "{case}: peer {peer} at tick {tick}"
            );
        }
        for stats in harness.session(peer).stats().peers.values() {
            rollbacks += stats.rollbacks;
            misses += stats.prediction_misses;
        }
    }
    (rollbacks, misses)
}

#[test]
fn model_peers_match_an_offline_run() {
    let mut rollbacks = 0;
    let mut misses = 0;
    for peers in 2..=4 {
        for seed in 0..SEEDS {
            let (case_rollbacks, case_misses) = play(peers, seed);
            rollbacks += case_rollbacks;
            misses += case_misses;
        }
    }
    // Not vacuous: the matches rolled back on prediction misses.
    assert!(rollbacks > 0, "no rollback in any match");
    assert!(misses > 0, "no prediction miss in any match");
}
