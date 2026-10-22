use ludenea_net::{
    Conditioned, ConditionsError, LinkConditions, MAX_PACKET_BYTES, MemoryNetwork, PeerId,
    Transport, TransportError,
};

fn pair() -> (impl Transport, impl Transport) {
    let mut net = MemoryNetwork::new(2).into_iter();
    (net.next().unwrap(), net.next().unwrap())
}

fn conditions() -> LinkConditions {
    LinkConditions {
        latency_micros: 0,
        jitter_micros: 0,
        loss_per_mille: 0,
        duplicate_per_mille: 0,
        reorder_per_mille: 0,
        reorder_extra_micros: 0,
        seed: 42,
    }
}

fn drain(t: &mut impl Transport, now: u64) -> Vec<(PeerId, Vec<u8>)> {
    std::iter::from_fn(|| t.receive(now)).collect()
}

#[test]
fn memory_delivers_in_order() {
    let (mut a, mut b) = pair();
    for i in 0..5u8 {
        a.send(PeerId(1), vec![i], 0).unwrap();
    }
    let got = drain(&mut b, 0);
    let expected: Vec<_> = (0..5u8).map(|i| (PeerId(0), vec![i])).collect();
    assert_eq!(got, expected);
    assert_eq!(a.receive(0), None);
}

#[test]
fn memory_refuses_an_unknown_peer_itself_and_a_large_packet() {
    let (mut a, _b) = pair();
    assert_eq!(
        a.send(PeerId(2), vec![1], 0),
        Err(TransportError::UnknownPeer { peer: PeerId(2) })
    );
    assert_eq!(
        a.send(PeerId(0), vec![1], 0),
        Err(TransportError::UnknownPeer { peer: PeerId(0) })
    );
    let len = MAX_PACKET_BYTES + 1;
    assert_eq!(
        a.send(PeerId(1), vec![0; len], 0),
        Err(TransportError::TooLarge { len })
    );
    assert_eq!(a.send(PeerId(1), vec![0; MAX_PACKET_BYTES], 0), Ok(()));
}

#[test]
fn conditioned_refuses_an_oversized_packet_at_send() {
    let (a, _b) = pair();
    let mut a = Conditioned::new(a, conditions()).unwrap();
    let len = MAX_PACKET_BYTES + 1;
    assert_eq!(
        a.send(PeerId(1), vec![0; len], 0),
        Err(TransportError::TooLarge { len })
    );
}

#[test]
fn conditioned_counts_a_release_failure() {
    let (a, _b) = pair();
    let mut a = Conditioned::new(
        a,
        LinkConditions {
            latency_micros: 100,
            ..conditions()
        },
    )
    .unwrap();
    assert_eq!(a.send(PeerId(5), vec![1], 0), Ok(()));
    a.receive(99);
    assert_eq!(a.failed_sends(), 0);
    a.receive(100);
    assert_eq!(a.failed_sends(), 1);
    assert_eq!(
        a.last_send_error(),
        Some(&TransportError::UnknownPeer { peer: PeerId(5) })
    );
}

#[test]
fn conditions_are_validated() {
    let (a, _b) = pair();
    let bad = LinkConditions {
        loss_per_mille: 1001,
        ..conditions()
    };
    assert!(matches!(
        Conditioned::new(a, bad),
        Err(ConditionsError::PerMilleTooLarge { .. })
    ));
    let (a, _b) = pair();
    let bad = LinkConditions {
        jitter_micros: u32::MAX,
        ..conditions()
    };
    assert_eq!(
        Conditioned::new(a, bad).err(),
        Some(ConditionsError::JitterTooLarge)
    );
}

fn run(conditions: LinkConditions) -> Vec<(u64, PeerId, Vec<u8>)> {
    let (a, mut b) = pair();
    let mut a = Conditioned::new(a, conditions).unwrap();
    let mut out = Vec::new();
    for tick in 0..300u64 {
        a.send(PeerId(1), vec![(tick % 256) as u8], tick * 1000)
            .unwrap();
        while let Some((from, p)) = b.receive(tick * 1000) {
            out.push((tick, from, p));
        }
        // The conditioned side pumps its own pending packets on receive.
        a.receive(tick * 1000);
    }
    a.receive(u64::MAX);
    out
}

fn lossy() -> LinkConditions {
    LinkConditions {
        latency_micros: 5_000,
        jitter_micros: 20_000,
        loss_per_mille: 100,
        duplicate_per_mille: 100,
        reorder_per_mille: 100,
        reorder_extra_micros: 30_000,
        seed: 7,
    }
}

#[test]
fn conditioned_is_deterministic_for_a_seed() {
    let first = run(lossy());
    assert!(!first.is_empty());
    assert_eq!(first, run(lossy()));
    let other = run(LinkConditions { seed: 8, ..lossy() });
    assert_ne!(first, other);
}

#[test]
fn conditioned_loss_rate_is_close() {
    let (a, mut b) = pair();
    let mut a = Conditioned::new(
        a,
        LinkConditions {
            loss_per_mille: 50,
            ..conditions()
        },
    )
    .unwrap();
    let total = 100_000usize;
    for _ in 0..total {
        a.send(PeerId(1), vec![0], 0).unwrap();
    }
    a.receive(0);
    let delivered = drain(&mut b, 0).len();
    let lost = total - delivered;
    assert!(
        lost > total * 45 / 1000 && lost < total * 55 / 1000,
        "lost {lost}"
    );
}

#[test]
fn conditioned_delays_by_latency() {
    let (a, mut b) = pair();
    let mut a = Conditioned::new(
        a,
        LinkConditions {
            latency_micros: 1_000,
            ..conditions()
        },
    )
    .unwrap();
    a.send(PeerId(1), vec![9], 500).unwrap();
    a.receive(1_499);
    assert_eq!(b.receive(1_499), None);
    a.receive(1_500);
    assert_eq!(b.receive(1_500), Some((PeerId(0), vec![9])));
}

fn order_after(conditions: LinkConditions) -> Vec<u8> {
    let (a, mut b) = pair();
    let mut a = Conditioned::new(a, conditions).unwrap();
    for i in 0..100u8 {
        a.send(PeerId(1), vec![i], u64::from(i)).unwrap();
    }
    a.receive(u64::MAX);
    drain(&mut b, u64::MAX)
        .into_iter()
        .map(|(_, p)| p[0])
        .collect()
}

fn is_sorted(v: &[u8]) -> bool {
    v.windows(2).all(|w| w[0] <= w[1])
}

#[test]
fn jitter_reorders() {
    let got = order_after(LinkConditions {
        jitter_micros: 1_000,
        ..conditions()
    });
    assert_eq!(got.len(), 100);
    assert!(!is_sorted(&got));
}

#[test]
fn reorder_reorders_without_jitter() {
    let got = order_after(LinkConditions {
        reorder_per_mille: 300,
        reorder_extra_micros: 50,
        ..conditions()
    });
    assert_eq!(got.len(), 100);
    assert!(!is_sorted(&got));
    assert!(is_sorted(&order_after(conditions())));
}

#[test]
fn duplicates_arrive_twice() {
    let got = order_after(LinkConditions {
        duplicate_per_mille: 1000,
        ..conditions()
    });
    assert_eq!(got.len(), 200);
    let mut sorted = got;
    sorted.sort_unstable();
    assert!(sorted.chunks(2).all(|c| c[0] == c[1]));
}

#[test]
fn links_are_independent() {
    let mut net = MemoryNetwork::new(3).into_iter();
    let mut a = Conditioned::new(
        net.next().unwrap(),
        LinkConditions {
            loss_per_mille: 500,
            ..conditions()
        },
    )
    .unwrap();
    let (mut b, mut c) = (net.next().unwrap(), net.next().unwrap());
    for i in 0..200u8 {
        a.send(PeerId(1), vec![i], 0).unwrap();
        a.send(PeerId(2), vec![i], 0).unwrap();
    }
    a.receive(0);
    let to_b = drain(&mut b, 0);
    let to_c = drain(&mut c, 0);
    for got in [&to_b, &to_c] {
        assert!(
            !got.is_empty() && got.len() < 200,
            "{} delivered",
            got.len()
        );
    }
    let ids = |v: &[(PeerId, Vec<u8>)]| v.iter().map(|(_, p)| p[0]).collect::<Vec<_>>();
    assert_ne!(ids(&to_b), ids(&to_c));
}
