// WebAssembly aborts on a panic: the executor's recovery from a game panic runs natively only.
#![cfg(not(target_family = "wasm"))]

mod support;

use ludenea_app::SimError;
use ludenea_net::{EndReason, ExecuteError, Executor, PeerId, Session, SessionError, SessionState};
use support::{PANIC_BYTE, handshake, identity, settings, test_simulation};

const NOW: u64 = 1_000;

#[test]
fn panic_during_resimulation_restores_and_ends() {
    let identity = identity(&[0, 1]);
    let spec = test_simulation().player_input_spec().unwrap();
    let sessions = (0..2)
        .map(|local| Session::new(settings(local, &identity), spec.clone()).unwrap())
        .collect();
    let mut sessions = handshake(sessions);
    let mut remote = sessions.pop().unwrap();
    let mut session = sessions.pop().unwrap();
    let mut simulation = test_simulation();
    let mut executor = Executor::new(&mut simulation, session.settings()).unwrap();

    // Peer 1 sends the panic byte for tick 2, the tick its first input applies to with the
    // default input delay, and its packets reach peer 0 late.
    remote.add_local_input(1, vec![PANIC_BYTE]).unwrap();
    remote.advance(NOW).unwrap();
    let late = remote.outgoing(NOW);

    // Peer 0 runs ahead predicting peer 1's default input, which the game accepts.
    let mut hash_at_two = None;
    for _ in 0..4 {
        session.add_local_input(0, vec![1]).unwrap();
        let requests = session.advance(NOW).unwrap();
        executor
            .execute(&mut simulation, &mut session, requests)
            .unwrap();
        if simulation.tick() == 2 {
            hash_at_two = Some(simulation.hash());
        }
        session.outgoing(NOW);
    }
    assert_eq!(simulation.tick(), 4);

    for (to, packet) in late {
        assert_eq!(to, PeerId(0));
        session.receive(PeerId(1), &packet, NOW);
    }
    session.add_local_input(0, vec![1]).unwrap();
    let requests = session.advance(NOW).unwrap();
    let error = executor
        .execute(&mut simulation, &mut session, requests)
        .unwrap_err();
    let ExecuteError::Game {
        tick: 2,
        error: SimError::Panicked(panic),
        recovery: None,
    } = &error
    else {
        panic!("{error:?}");
    };
    assert_eq!(panic.tick, 2);
    assert!(panic.message.contains("panic byte"), "{}", panic.message);

    // The simulation stands at the state of the failed tick, and the session ended without
    // landing: stepping on would run the failing code again.
    assert_eq!(simulation.tick(), 2);
    assert_eq!(Some(simulation.hash()), hash_at_two);
    // The other peer learns which peer failed: the sender of the notice.
    let failure = SessionState::Ended(EndReason::LocalFailure {
        peer: PeerId(0),
        tick: 2,
    });
    assert_eq!(session.state(), failure);
    assert_eq!(session.advance(NOW), Err(SessionError::NotRunning));

    // The end notice ends the other peer for the same reason.
    for (to, packet) in session.outgoing(NOW) {
        assert_eq!(to, PeerId(1));
        remote.receive(PeerId(0), &packet, NOW);
    }
    assert_eq!(remote.state(), failure);
}
