use ludenea_net::{
    Channel, Link, LinkEvent, LinkId, MAX_PEERS, MatchLayout, MatchRules, MemoryLink, MemoryMesh,
    PeerId, Room, RoomDrop, RoomError, RoomEvent, RoomMessage, RoomRefusal, RoomRole, RoomState,
};

const MATCH_ID: u64 = 0x5eed;

/// One room member over its link of the mesh.
struct Member {
    link: MemoryLink,
    room: Room,
    id: LinkId,
}

impl Member {
    fn new(link: MemoryLink, role: RoomRole) -> Self {
        Self::with_rules(link, role, MatchRules::default())
    }

    fn with_rules(mut link: MemoryLink, role: RoomRole, rules: MatchRules) -> Self {
        let room = Room::new(role, rules).unwrap();
        // The first event of a memory link is its assignment, left for `pump` to deliver.
        let events = link.poll();
        let LinkEvent::Assigned(id) = events[0] else {
            panic!("assigned first: {events:?}");
        };
        let mut member = Self { link, room, id };
        member.apply(events);
        member
    }

    fn apply(&mut self, events: Vec<LinkEvent>) {
        for event in events {
            match event {
                LinkEvent::Assigned(me) => self.room.assigned(me),
                LinkEvent::Connected(link) => self.room.connected(link),
                LinkEvent::Disconnected(link) => self.room.disconnected(link),
                LinkEvent::Closed(_) => {}
            }
        }
    }

    /// Moves everything one step; true when anything moved.
    fn step(&mut self) -> bool {
        let events = self.link.poll();
        let mut moved = !events.is_empty();
        self.apply(events);
        while let Some((from, bytes)) = self.link.receive(Channel::Room) {
            moved = true;
            self.room.receive(from, &bytes);
        }
        for (to, bytes) in self.room.outgoing() {
            moved = true;
            // A peer that left in the meantime misses it, as on a network.
            let _ = self.link.send(Channel::Room, to, bytes);
        }
        moved
    }

    fn layout(&self) -> &MatchLayout {
        match self.room.state() {
            RoomState::Ready(layout) => layout,
            state => panic!("expected a layout, the room is {state:?}"),
        }
    }

    fn failure(&self) -> &RoomError {
        match self.room.state() {
            RoomState::Failed(error) => error,
            state => panic!("expected a failure, the room is {state:?}"),
        }
    }

    fn waiting(&self) -> bool {
        matches!(self.room.state(), RoomState::Waiting)
    }
}

fn pump(members: &mut [&mut Member]) {
    for _ in 0..100 {
        let mut moved = false;
        for member in members.iter_mut() {
            moved |= member.step();
        }
        if !moved {
            return;
        }
    }
    panic!("the room never settles");
}

fn host(mesh: &MemoryMesh, peers: u8) -> Member {
    Member::new(
        mesh.join(),
        RoomRole::Host {
            peers,
            match_id: MATCH_ID,
        },
    )
}

fn joiner(mesh: &MemoryMesh) -> Member {
    Member::new(mesh.join(), RoomRole::Join)
}

fn encode(message: RoomMessage) -> Vec<u8> {
    message.encode()
}

fn link(n: u8) -> LinkId {
    LinkId([n; 16])
}

fn dropped(link: LinkId, reason: RoomDrop) -> RoomEvent {
    RoomEvent::Dropped { link, reason }
}

#[test]
fn a_room_needs_two_to_four_peers() {
    for peers in [0, 1, 5] {
        assert_eq!(
            Room::new(RoomRole::Host { peers, match_id: 1 }, MatchRules::default()).err(),
            Some(RoomError::Peers { peers })
        );
    }
    for peers in 2..=4 {
        assert!(Room::new(RoomRole::Host { peers, match_id: 1 }, MatchRules::default()).is_ok());
    }
    assert_eq!(
        RoomError::Peers { peers: 5 }.to_string(),
        format!("a room holds 2 to {MAX_PEERS} peers, not 5")
    );
}

#[test]
fn two_three_and_four_peers_agree_on_one_layout() {
    for peers in 2..=4u8 {
        let mesh = MemoryMesh::new();
        let mut host = host(&mesh, peers);
        let mut joiners: Vec<Member> = (1..peers).map(|_| joiner(&mesh)).collect();
        host.room.set_ready(1).unwrap();
        for joiner in &mut joiners {
            joiner.room.set_ready(1).unwrap();
        }
        let mut all: Vec<&mut Member> = std::iter::once(&mut host)
            .chain(joiners.iter_mut())
            .collect();
        pump(&mut all);

        let links: Vec<LinkId> = all.iter().map(|member| member.id).collect();
        let expected_players: Vec<PeerId> = (0..peers).map(PeerId).collect();
        for (index, member) in all.iter_mut().enumerate() {
            let layout = member.layout();
            assert_eq!(layout.match_id, MATCH_ID);
            assert_eq!(layout.links, links, "the host first, joiners in order");
            assert_eq!(layout.players, expected_players);
            assert_eq!(layout.local, PeerId(index as u8));
            assert_eq!(layout.local_players(), vec![index as u8]);
            assert!(member.room.events().is_empty());
        }
    }
}

#[test]
fn players_follow_each_declaration() {
    for (host_players, joiner_players, players) in [
        (2, 1, vec![PeerId(0), PeerId(0), PeerId(1)]),
        (1, 2, vec![PeerId(0), PeerId(1), PeerId(1)]),
    ] {
        let mesh = MemoryMesh::new();
        let mut host = host(&mesh, 2);
        let mut joiner = joiner(&mesh);
        host.room.set_ready(host_players).unwrap();
        joiner.room.set_ready(joiner_players).unwrap();
        pump(&mut [&mut host, &mut joiner]);
        assert_eq!(host.layout().players, players);
        assert_eq!(joiner.layout().players, players);
        let host_indices: Vec<u8> = (0..host_players).collect();
        let joiner_indices: Vec<u8> = (host_players..host_players + joiner_players).collect();
        assert_eq!(host.layout().local_players(), host_indices);
        assert_eq!(joiner.layout().local_players(), joiner_indices);
    }
}

#[test]
fn the_host_waits_for_a_complete_mesh() {
    let mesh = MemoryMesh::new();
    let mut host = host(&mesh, 3);
    let mut first = Member::new(mesh.join_with(&[host.id]), RoomRole::Join);
    let mut second = Member::new(mesh.join_with(&[host.id]), RoomRole::Join);
    host.room.set_ready(1).unwrap();
    first.room.set_ready(1).unwrap();
    second.room.set_ready(1).unwrap();
    pump(&mut [&mut host, &mut first, &mut second]);
    assert!(host.waiting() && first.waiting() && second.waiting());
    let view = host.room.view();
    assert_eq!((view.connected, view.ready, view.unmeshed), (3, 3, 1));
    assert_eq!(
        view.describe(),
        "3 of 3 peers here, 1 pair of peers not connected to each other"
    );
    assert_eq!(
        first.room.view().describe(),
        "connected to 2 of 3 peers, waiting for the host to start"
    );

    mesh.connect(first.id, second.id);
    pump(&mut [&mut host, &mut first, &mut second]);
    let links = vec![host.id, first.id, second.id];
    for member in [&host, &first, &second] {
        assert_eq!(member.layout().links, links);
    }
}

#[test]
fn the_host_waits_for_its_own_ready() {
    let mesh = MemoryMesh::new();
    let mut host = host(&mesh, 2);
    let mut joiner = joiner(&mesh);
    joiner.room.set_ready(1).unwrap();
    pump(&mut [&mut host, &mut joiner]);
    assert!(host.waiting() && joiner.waiting());
    let view = host.room.view();
    assert!(view.host && !view.local_ready);
    assert_eq!((view.peers, view.connected, view.ready), (Some(2), 2, 1));
    assert_eq!(view.describe(), "2 of 2 peers here, 1 not ready");

    host.room.set_ready(1).unwrap();
    pump(&mut [&mut host, &mut joiner]);
    assert_eq!(host.layout().links, joiner.layout().links);
}

#[test]
fn a_joiner_that_never_readies_is_passed_over() {
    let mesh = MemoryMesh::new();
    let mut host = host(&mesh, 3);
    let mut idle = joiner(&mesh);
    let mut second = joiner(&mesh);
    let mut third = joiner(&mesh);
    pump(&mut [&mut host, &mut idle, &mut second, &mut third]);
    assert!(idle.waiting() && second.waiting() && third.waiting());
    assert_eq!(
        host.room.view().describe(),
        "4 of 3 peers here, 4 not ready"
    );

    second.room.set_ready(1).unwrap();
    third.room.set_ready(1).unwrap();
    host.room.set_ready(1).unwrap();
    pump(&mut [&mut host, &mut idle, &mut second, &mut third]);
    let links = vec![host.id, second.id, third.id];
    for member in [&host, &second, &third] {
        assert_eq!(member.layout().links, links);
    }
    assert_eq!(idle.failure(), &RoomError::Full);
}

#[test]
fn an_extra_peer_is_refused_as_full() {
    // Before the layout: the only joiner a two-peer room holds is ready, the host is not.
    let mesh = MemoryMesh::new();
    let mut host = host(&mesh, 2);
    let mut first = joiner(&mesh);
    first.room.set_ready(1).unwrap();
    pump(&mut [&mut host, &mut first]);
    let mut extra = joiner(&mesh);
    pump(&mut [&mut host, &mut first, &mut extra]);
    assert_eq!(extra.failure(), &RoomError::Full);
    assert_eq!(
        extra.room.view().peers,
        Some(2),
        "Host came before the refusal"
    );
    assert!(host.waiting() && first.waiting());

    // After the layout.
    host.room.set_ready(1).unwrap();
    pump(&mut [&mut host, &mut first, &mut extra]);
    assert!(matches!(host.room.state(), RoomState::Ready(_)));
    let mut late = joiner(&mesh);
    pump(&mut [&mut host, &mut first, &mut late]);
    assert_eq!(late.failure(), &RoomError::Full);

    // After the end, anything else is dropped and counted.
    let ready = encode(RoomMessage::Ready {
        players: 1,
        connected: vec![],
    });
    host.room.receive(first.id, &ready);
    assert_eq!(
        host.room.events(),
        vec![dropped(first.id, RoomDrop::Unexpected("Ready"))]
    );
    assert_eq!(host.layout().links, vec![host.id, first.id]);
}

#[test]
fn a_joiner_that_cannot_fit_is_refused() {
    let mesh = MemoryMesh::new();
    let mut host = host(&mesh, 3);
    let mut greedy = joiner(&mesh);
    host.room.set_ready(2).unwrap();
    // Two of its own, two of the host's, one at least for the third peer: 5.
    greedy.room.set_ready(2).unwrap();
    pump(&mut [&mut host, &mut greedy]);
    assert_eq!(greedy.failure(), &RoomError::TooManyPlayers { total: 5 });
    assert!(host.waiting());
    assert_eq!(
        host.room.view().connected,
        1,
        "the refused joiner left the room"
    );
}

#[test]
fn the_host_picks_joiners_that_fit() {
    let mesh = MemoryMesh::new();
    let mut host = host(&mesh, 3);
    let mut first = joiner(&mesh);
    let mut second = joiner(&mesh);
    host.room.set_ready(1).unwrap();
    first.room.set_ready(2).unwrap();
    second.room.set_ready(2).unwrap();
    pump(&mut [&mut host, &mut first, &mut second]);
    assert!(host.waiting());
    let view = host.room.view();
    assert_eq!(view.unfit, 1);
    assert_eq!(
        view.describe(),
        "3 of 3 peers here, 1 peer with too many players"
    );

    let mut third = joiner(&mesh);
    third.room.set_ready(1).unwrap();
    pump(&mut [&mut host, &mut first, &mut second, &mut third]);
    let links = vec![host.id, first.id, third.id];
    assert_eq!(host.layout().links, links);
    assert_eq!(third.layout().players, host.layout().players);
    assert_eq!(
        host.layout().players,
        vec![PeerId(0), PeerId(1), PeerId(1), PeerId(2)]
    );
    assert_eq!(second.failure(), &RoomError::Full);
}

#[test]
fn an_unplayable_layout_fails_every_peer() {
    let rules = MatchRules {
        rollback_window: 24,
        ..MatchRules::default()
    };
    let mesh = MemoryMesh::new();
    let mut host = Member::with_rules(
        mesh.join(),
        RoomRole::Host {
            peers: 2,
            match_id: MATCH_ID,
        },
        rules,
    );
    let mut joiner = Member::with_rules(mesh.join(), RoomRole::Join, rules);
    host.room.set_ready(1).unwrap();
    joiner.room.set_ready(3).unwrap();
    pump(&mut [&mut host, &mut joiner]);
    let RoomError::Unplayable { reason } = host.failure() else {
        panic!("unplayable: {:?}", host.failure());
    };
    assert!(
        reason.starts_with("the worst-case packet of peer 1 takes"),
        "{reason}"
    );
    assert!(host.failure().to_string().contains(reason.as_str()));
    assert!(matches!(joiner.failure(), RoomError::Unplayable { .. }));
}

#[test]
fn a_joiners_host_message_is_dropped_by_the_host() {
    let mut room = Room::new(
        RoomRole::Host {
            peers: 2,
            match_id: 1,
        },
        MatchRules::default(),
    )
    .unwrap();
    room.assigned(link(1));
    room.connected(link(2));
    room.receive(link(2), &encode(RoomMessage::Host { peers: 2 }));
    assert_eq!(
        room.events(),
        vec![dropped(link(2), RoomDrop::Unexpected("Host"))]
    );
    assert_eq!(room.state(), &RoomState::Waiting);
}

#[test]
fn two_hosts_fail_their_joiners() {
    let mesh = MemoryMesh::new();
    let mut first = host(&mesh, 2);
    let mut second = host(&mesh, 2);
    pump(&mut [&mut first, &mut second]);
    assert_eq!(
        first.room.events(),
        vec![dropped(second.id, RoomDrop::Unexpected("Host"))]
    );
    assert!(first.waiting() && second.waiting());

    let mut joiner = joiner(&mesh);
    pump(&mut [&mut first, &mut second, &mut joiner]);
    assert_eq!(
        joiner.failure(),
        &RoomError::TwoHosts {
            first: first.id,
            second: second.id
        }
    );
}

#[test]
fn a_repeated_host_message_is_ignored() {
    let mut room = Room::new(RoomRole::Join, MatchRules::default()).unwrap();
    room.assigned(link(2));
    room.connected(link(1));
    room.set_ready(1).unwrap();
    room.receive(link(1), &encode(RoomMessage::Host { peers: 2 }));
    let ready = encode(RoomMessage::Ready {
        players: 1,
        connected: vec![],
    });
    assert_eq!(room.outgoing(), vec![(link(1), ready)]);
    room.receive(link(1), &encode(RoomMessage::Host { peers: 3 }));
    assert!(room.events().is_empty());
    assert!(room.outgoing().is_empty());
    assert_eq!(room.state(), &RoomState::Waiting);
    assert_eq!(room.view().peers, Some(2));
}

#[test]
fn a_leaving_host_fails_the_joiners() {
    let mesh = MemoryMesh::new();
    let mut host = host(&mesh, 3);
    let mut first = joiner(&mesh);
    let mut second = joiner(&mesh);
    first.room.set_ready(1).unwrap();
    pump(&mut [&mut host, &mut first, &mut second]);
    assert!(first.waiting() && second.waiting());
    drop(host);
    pump(&mut [&mut first, &mut second]);
    assert_eq!(first.failure(), &RoomError::HostLeft);
    assert_eq!(second.failure(), &RoomError::HostLeft);
}

#[test]
fn a_joiner_leaving_before_the_layout_is_forgotten() {
    let mesh = MemoryMesh::new();
    let mut host = host(&mesh, 2);
    let mut first = joiner(&mesh);
    first.room.set_ready(1).unwrap();
    pump(&mut [&mut host, &mut first]);
    drop(first);
    let mut second = joiner(&mesh);
    second.room.set_ready(1).unwrap();
    pump(&mut [&mut host, &mut second]);
    assert!(second.waiting(), "the place is free again");
    host.room.set_ready(1).unwrap();
    pump(&mut [&mut host, &mut second]);
    assert_eq!(host.layout().links, vec![host.id, second.id]);
    assert_eq!(second.layout().links, vec![host.id, second.id]);
}

/// A joiner as `link(2)`, ready with one player, connected to its host `link(1)` and to
/// `link(3)`, in a room of three.
fn ready_joiner(assigned: bool) -> Room {
    let mut room = Room::new(RoomRole::Join, MatchRules::default()).unwrap();
    if assigned {
        room.assigned(link(2));
    }
    room.connected(link(1));
    room.connected(link(3));
    room.receive(link(1), &encode(RoomMessage::Host { peers: 3 }));
    room.set_ready(1).unwrap();
    assert_eq!(
        room.outgoing(),
        vec![(
            link(1),
            encode(RoomMessage::Ready {
                players: 1,
                connected: vec![link(3)]
            })
        )]
    );
    room
}

#[test]
fn invalid_layouts_fail_the_joiner() {
    let layout = |links: Vec<LinkId>, counts: Vec<u8>| RoomMessage::Layout {
        match_id: 7,
        links,
        counts,
    };
    let cases = [
        (
            true,
            layout(vec![link(1), link(2), link(3)], vec![1, 1]),
            "the layout lists 3 links and 2 counts, the host announced 3 peers",
        ),
        (
            true,
            layout(vec![link(1), link(2)], vec![1, 1]),
            "the layout lists 2 links and 2 counts, the host announced 3 peers",
        ),
        (
            true,
            layout(vec![link(1), link(2), link(2)], vec![1, 1, 1]),
            "the layout lists a link twice",
        ),
        (
            true,
            layout(vec![link(3), link(2), link(1)], vec![1, 1, 1]),
            "the layout does not start with the host",
        ),
        (
            false,
            layout(vec![link(1), link(2), link(3)], vec![1, 1, 1]),
            "own link unknown",
        ),
        (
            true,
            layout(vec![link(1), link(3), link(4)], vec![1, 1, 1]),
            "the layout leaves this peer out",
        ),
        (
            true,
            layout(vec![link(1), link(2), link(3)], vec![1, 2, 1]),
            "the layout gives this peer 2 players, it declared 1",
        ),
        (
            true,
            layout(vec![link(1), link(2), link(4)], vec![1, 1, 1]),
            "the layout names a link this peer is not connected to",
        ),
        (
            true,
            layout(vec![link(1), link(2), link(3)], vec![0, 1, 1]),
            "the layout gives a peer 0 players, expected 1 to 4",
        ),
        (
            true,
            layout(vec![link(1), link(2), link(3)], vec![1, 1, 5]),
            "the layout gives a peer 5 players, expected 1 to 4",
        ),
        (
            true,
            layout(vec![link(1), link(2), link(3)], vec![3, 1, 1]),
            "the layout holds 5 players, at most 4 allowed",
        ),
    ];
    for (assigned, message, rule) in cases {
        let mut room = ready_joiner(assigned);
        room.receive(link(1), &encode(message.clone()));
        assert_eq!(
            room.state(),
            &RoomState::Failed(RoomError::InvalidLayout(rule.to_owned())),
            "{message:?}"
        );
        assert!(room.outgoing().is_empty());
    }

    // A layout before this peer declared its players.
    let mut room = Room::new(RoomRole::Join, MatchRules::default()).unwrap();
    room.assigned(link(2));
    room.connected(link(1));
    room.receive(link(1), &encode(RoomMessage::Host { peers: 2 }));
    room.receive(link(1), &encode(layout(vec![link(1), link(2)], vec![1, 1])));
    assert_eq!(
        room.state(),
        &RoomState::Failed(RoomError::InvalidLayout(
            "the layout came before this peer declared its players".to_owned()
        ))
    );

    let mut room = ready_joiner(true);
    room.receive(
        link(1),
        &encode(layout(vec![link(1), link(2), link(3)], vec![2, 1, 1])),
    );
    assert_eq!(
        room.state(),
        &RoomState::Ready(MatchLayout {
            local: PeerId(1),
            match_id: 7,
            players: vec![PeerId(0), PeerId(0), PeerId(1), PeerId(2)],
            links: vec![link(1), link(2), link(3)],
        })
    );
}

#[test]
fn layouts_and_refusals_from_another_link_are_dropped() {
    let mut room = ready_joiner(true);
    room.receive(
        link(3),
        &encode(RoomMessage::Layout {
            match_id: 7,
            links: vec![link(3), link(2), link(1)],
            counts: vec![1, 1, 1],
        }),
    );
    room.receive(
        link(3),
        &encode(RoomMessage::Refuse {
            reason: RoomRefusal::Full,
        }),
    );
    room.receive(
        link(3),
        &encode(RoomMessage::Ready {
            players: 1,
            connected: vec![],
        }),
    );
    assert_eq!(
        room.events(),
        vec![
            dropped(link(3), RoomDrop::Unexpected("Layout")),
            dropped(link(3), RoomDrop::Unexpected("Refuse")),
            dropped(link(3), RoomDrop::Unexpected("Ready")),
        ]
    );
    assert_eq!(room.state(), &RoomState::Waiting);

    // Before any host is known, nobody may send them.
    let mut room = Room::new(RoomRole::Join, MatchRules::default()).unwrap();
    room.connected(link(1));
    room.receive(
        link(1),
        &encode(RoomMessage::Refuse {
            reason: RoomRefusal::Full,
        }),
    );
    assert_eq!(
        room.events(),
        vec![dropped(link(1), RoomDrop::Unexpected("Refuse"))]
    );
    assert_eq!(room.state(), &RoomState::Waiting);
}

#[test]
fn invalid_ready_messages_are_dropped() {
    let mut room = Room::new(
        RoomRole::Host {
            peers: 2,
            match_id: 1,
        },
        MatchRules::default(),
    )
    .unwrap();
    room.assigned(link(1));
    room.connected(link(2));
    room.set_ready(1).unwrap();
    let _ = room.outgoing();
    let ready = |players, connected| encode(RoomMessage::Ready { players, connected });
    room.receive(link(2), &ready(0, vec![]));
    room.receive(link(2), &ready(5, vec![]));
    room.receive(link(2), &ready(1, vec![link(3), link(4), link(5), link(6)]));
    // From a link that is not in the room.
    room.receive(link(9), &ready(1, vec![]));
    assert_eq!(
        room.events(),
        vec![
            dropped(link(2), RoomDrop::Invalid("players")),
            dropped(link(2), RoomDrop::Invalid("players")),
            dropped(link(2), RoomDrop::Invalid("connected")),
            dropped(link(9), RoomDrop::Unexpected("Ready")),
        ]
    );
    assert_eq!(room.state(), &RoomState::Waiting);
    assert!(room.outgoing().is_empty());
    assert_eq!(room.view().ready, 1);

    // A joiner drops a host announcing a room it cannot be.
    let mut room = Room::new(RoomRole::Join, MatchRules::default()).unwrap();
    room.connected(link(1));
    room.receive(link(1), &encode(RoomMessage::Host { peers: 1 }));
    room.receive(link(1), &encode(RoomMessage::Host { peers: 5 }));
    assert_eq!(
        room.events(),
        vec![
            dropped(link(1), RoomDrop::Invalid("peers")),
            dropped(link(1), RoomDrop::Invalid("peers")),
        ]
    );
    assert_eq!(room.view().peers, None);
    assert_eq!(room.view().describe(), "waiting for the host");
}

#[test]
fn malformed_messages_are_dropped() {
    let host = || {
        Room::new(
            RoomRole::Host {
                peers: 2,
                match_id: 1,
            },
            MatchRules::default(),
        )
        .unwrap()
    };
    let joiner = || Room::new(RoomRole::Join, MatchRules::default()).unwrap();
    let mut wrong_version = encode(RoomMessage::Host { peers: 2 });
    wrong_version[0] = 9;
    let mut trailing = encode(RoomMessage::Host { peers: 2 });
    trailing.push(0);
    let mut too_large = encode(RoomMessage::Host { peers: 2 });
    too_large.resize(600, 0);
    for mut room in [host(), joiner()] {
        room.connected(link(2));
        room.receive(link(2), &[]);
        room.receive(link(2), &wrong_version);
        room.receive(link(2), &trailing);
        room.receive(link(2), &too_large);
        assert_eq!(
            room.events(),
            vec![
                dropped(link(2), RoomDrop::Wire("Malformed")),
                dropped(link(2), RoomDrop::Wire("Version")),
                dropped(link(2), RoomDrop::Wire("Malformed")),
                dropped(link(2), RoomDrop::Wire("TooLarge")),
            ]
        );
        assert_eq!(room.state(), &RoomState::Waiting);
    }
}

#[test]
fn set_ready_refuses() {
    let mut room = Room::new(RoomRole::Join, MatchRules::default()).unwrap();
    assert_eq!(
        room.set_ready(0),
        Err(RoomError::LocalPlayers { players: 0 })
    );
    assert_eq!(
        room.set_ready(5),
        Err(RoomError::LocalPlayers { players: 5 })
    );
    assert!(!room.view().local_ready);
    assert_eq!(room.set_ready(4), Ok(()));
    assert_eq!(room.set_ready(1), Err(RoomError::AlreadyReady));
    assert!(room.view().local_ready);
    assert_eq!(
        RoomError::LocalPlayers { players: 5 }.to_string(),
        "a peer brings 1 to 4 local players, not 5"
    );
}
