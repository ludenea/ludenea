//! One peer driving its room, then its match, over one link.

mod support;

use std::cell::RefCell;
use std::rc::Rc;

use ludenea_net::{
    Channel, Link, LinkConditions, LinkError, LinkEvent, LinkId, MatchIdentity, MemoryLink,
    MemoryMesh, OnlineError, OnlinePeer, OnlineStatus, PeerId, RoomError, RoomEvent, RoomRole,
    Session, SessionSettings, SessionState, Timeouts,
};
use support::{FRAME_MICROS, Model, identity, model_byte, model_input};

const ONE_SECOND: u64 = 1_000_000;
const STEP: u64 = 1_000;

fn clear() -> LinkConditions {
    LinkConditions {
        latency_micros: 0,
        jitter_micros: 0,
        loss_per_mille: 0,
        duplicate_per_mille: 0,
        reorder_per_mille: 0,
        reorder_extra_micros: 0,
        seed: 3,
    }
}

fn host(peers: u8) -> RoomRole {
    RoomRole::Host { peers, match_id: 7 }
}

/// A [`MemoryLink`] whose owner can close it, as a real link closes.
struct ClosableLink {
    inner: MemoryLink,
    close: Rc<RefCell<Option<String>>>,
}

impl ClosableLink {
    fn new(inner: MemoryLink) -> (Self, Rc<RefCell<Option<String>>>) {
        let close = Rc::new(RefCell::new(None));
        let link = Self {
            inner,
            close: Rc::clone(&close),
        };
        (link, close)
    }
}

impl Link for ClosableLink {
    fn poll(&mut self) -> Vec<LinkEvent> {
        let mut events = self.inner.poll();
        if let Some(reason) = self.close.borrow_mut().take() {
            events.push(LinkEvent::Closed(reason));
        }
        events
    }

    fn send(&mut self, channel: Channel, to: LinkId, bytes: Vec<u8>) -> Result<(), LinkError> {
        self.inner.send(channel, to, bytes)
    }

    fn receive(&mut self, channel: Channel) -> Option<(LinkId, Vec<u8>)> {
        self.inner.receive(channel)
    }
}

/// Polls every peer each millisecond until all are `Ready`, for at most one second; the time
/// it ended at.
fn until_ready<L: Link>(peers: &mut [OnlinePeer<L>]) -> u64 {
    let mut now = 0;
    while now < ONE_SECOND {
        let statuses: Vec<OnlineStatus> = peers.iter_mut().map(|peer| peer.poll(now)).collect();
        if statuses
            .iter()
            .all(|status| matches!(status, OnlineStatus::Ready(_)))
        {
            return now;
        }
        now += STEP;
    }
    panic!("the peers never got ready");
}

/// A room of `count` peers on one mesh, the first hosting, each ready with one player.
fn room(mesh: &MemoryMesh, count: u8, conditions: LinkConditions) -> Vec<OnlinePeer<MemoryLink>> {
    (0..count)
        .map(|index| {
            let role = if index == 0 {
                host(count)
            } else {
                RoomRole::Join
            };
            let mut peer = OnlinePeer::new(mesh.join(), role, conditions).unwrap();
            peer.set_ready(1).unwrap();
            peer
        })
        .collect()
}

struct Player {
    peer: OnlinePeer<MemoryLink>,
    session: Option<Session>,
    model: Option<Model>,
    next_frame: u64,
    frames: u64,
}

#[test]
fn two_three_and_four_peers_reach_running_sessions() {
    for count in 2..=4 {
        let mesh = MemoryMesh::new();
        let mut players: Vec<Player> = room(&mesh, count, clear())
            .into_iter()
            .map(|peer| Player {
                peer,
                session: None,
                model: None,
                next_frame: 0,
                frames: 0,
            })
            .collect();
        let mut now = 0;
        let mut running_at = None;
        while now < 10 * ONE_SECOND {
            for player in &mut players {
                let status = player.peer.poll(now);
                if player.session.is_none()
                    && let OnlineStatus::Ready(layout) = status
                {
                    let settings = SessionSettings {
                        local: layout.local,
                        match_id: layout.match_id,
                        identity: MatchIdentity {
                            players: layout.players.clone(),
                            ..identity(&[])
                        },
                        timeouts: Timeouts::default(),
                    };
                    player.model = Some(Model::new(&settings.identity.rules));
                    player.session = Some(Session::new(settings, model_input()).unwrap());
                }
                let (Some(session), Some(model)) = (&mut player.session, &mut player.model) else {
                    continue;
                };
                for (from, packet) in player.peer.receive(now) {
                    session.receive(from, &packet, now);
                }
                if session.state() == SessionState::Running && now >= player.next_frame {
                    let local = player.peer.layout().unwrap().local_players();
                    for global in local {
                        session
                            .add_local_input(global, vec![model_byte(global, player.frames)])
                            .unwrap();
                    }
                    let requests = session.advance(now).unwrap();
                    model.execute_for(session, &requests);
                    player.frames += 1;
                    player.next_frame = now + FRAME_MICROS;
                }
                for (to, packet) in session.outgoing(now) {
                    player.peer.send(to, packet, now);
                }
            }
            let running = players.iter().all(|player| {
                player
                    .session
                    .as_ref()
                    .is_some_and(|session| session.state() == SessionState::Running)
            });
            if running && running_at.is_none() {
                running_at = Some(now);
            }
            let confirmed = players.iter().all(|player| {
                player
                    .session
                    .as_ref()
                    .is_some_and(|session| session.confirmed_tick() >= 100)
            });
            if confirmed {
                break;
            }
            now += STEP;
        }
        let running_at = running_at.unwrap_or_else(|| panic!("{count} peers never ran"));
        assert!(running_at < ONE_SECOND, "{count} peers ran at {running_at}");
        for player in &players {
            let session = player.session.as_ref().unwrap();
            assert!(
                session.confirmed_tick() >= 100,
                "{count} peers: confirmed {}",
                session.confirmed_tick()
            );
            assert_eq!(player.peer.failed_sends(), 0, "{count} peers");
            assert_eq!(player.peer.strays(), 0, "{count} peers");
        }
    }
}

/// Sends one packet from the host to the joiner at `start` and returns when it arrived.
fn arrival(conditions: LinkConditions) -> u64 {
    let mesh = MemoryMesh::new();
    let mut peers = room(&mesh, 2, conditions);
    let start = until_ready(&mut peers);
    peers[0].send(PeerId(1), vec![1, 2, 3], start);
    let mut now = start;
    while now < start + ONE_SECOND {
        for peer in &mut peers {
            peer.poll(now);
        }
        assert!(peers[0].receive(now).is_empty());
        let received = peers[1].receive(now);
        if !received.is_empty() {
            assert_eq!(received, vec![(PeerId(0), vec![1, 2, 3])]);
            return now - start;
        }
        now += STEP;
    }
    panic!("the packet never arrived");
}

#[test]
fn conditions_delay_the_sends() {
    let delayed = LinkConditions {
        latency_micros: 50_000,
        ..clear()
    };
    assert_eq!(arrival(delayed), 50_000);
}

#[test]
fn clear_conditions_add_nothing() {
    let mesh = MemoryMesh::new();
    let mut peers = room(&mesh, 2, clear());
    let now = until_ready(&mut peers);
    // The sender never receives, so nothing but the send itself can release the packet.
    peers[0].send(PeerId(1), vec![9], now);
    assert_eq!(peers[1].receive(now), vec![(PeerId(0), vec![9])]);
}

#[test]
fn conditions_out_of_range_are_refused() {
    let mesh = MemoryMesh::new();
    let lossy = LinkConditions {
        loss_per_mille: 1001,
        ..clear()
    };
    let error = OnlinePeer::new(mesh.join(), RoomRole::Join, lossy).err();
    assert!(matches!(error, Some(OnlineError::Conditions(_))));
    let error = OnlinePeer::new(mesh.join(), host(9), clear()).err();
    assert!(matches!(
        error,
        Some(OnlineError::Room(RoomError::Peers { peers: 9 }))
    ));
}

#[test]
fn a_closed_link_fails_the_room() {
    let mesh = MemoryMesh::new();
    let (link, close) = ClosableLink::new(mesh.join());
    let mut peer = OnlinePeer::new(link, RoomRole::Join, clear()).unwrap();
    assert!(matches!(peer.poll(0), OnlineStatus::Room(_)));
    *close.borrow_mut() = Some("the signaling server went away".to_owned());
    let failed = OnlineStatus::Failed(
        "the connection to the room closed: the signaling server went away".to_owned(),
    );
    assert_eq!(peer.poll(STEP), failed);
    // A host that shows up afterwards changes nothing.
    let mut late_host = OnlinePeer::new(mesh.join(), host(2), clear()).unwrap();
    late_host.set_ready(1).unwrap();
    peer.set_ready(1).unwrap();
    for now in 2..20 {
        late_host.poll(now * STEP);
        assert_eq!(peer.poll(now * STEP), failed);
    }
    assert_eq!(peer.closed(), None);
    assert!(peer.layout().is_none());
}

#[test]
fn a_link_closed_after_the_layout_is_kept_for_the_message() {
    let mesh = MemoryMesh::new();
    let (link, close) = ClosableLink::new(mesh.join());
    let mut host_peer = OnlinePeer::new(link, host(2), clear()).unwrap();
    host_peer.set_ready(1).unwrap();
    let mut joiner = OnlinePeer::new(mesh.join(), RoomRole::Join, clear()).unwrap();
    joiner.set_ready(1).unwrap();
    let mut now = 0;
    while !matches!(host_peer.poll(now), OnlineStatus::Ready(_)) {
        joiner.poll(now);
        now += STEP;
        assert!(now < ONE_SECOND, "the host never got ready");
    }
    assert_eq!(host_peer.closed(), None);
    *close.borrow_mut() = Some("the peer connection failed".to_owned());
    let layout = host_peer.layout().unwrap().clone();
    assert_eq!(host_peer.poll(now), OnlineStatus::Ready(layout));
    assert_eq!(host_peer.closed(), Some("the peer connection failed"));
}

/// The id of `link` and the members it connected to, from its first poll.
fn identify(link: &mut MemoryLink) -> (LinkId, Vec<LinkId>) {
    let events = link.poll();
    let LinkEvent::Assigned(me) = events[0] else {
        panic!("the first event is the assignment: {events:?}");
    };
    let members = events[1..]
        .iter()
        .map(|event| match event {
            LinkEvent::Connected(member) => *member,
            other => panic!("only connections follow: {other:?}"),
        })
        .collect();
    (me, members)
}

#[test]
fn match_packets_before_the_layout_are_dropped() {
    let mesh = MemoryMesh::new();
    let mut peers = room(&mesh, 2, clear());
    let mut intruder = mesh.join();
    let (_, members) = identify(&mut intruder);
    assert_eq!(members.len(), 2);
    for member in members {
        for _ in 0..5 {
            intruder.send(Channel::Match, member, vec![0xAB]).unwrap();
        }
    }
    // A send before the layout goes nowhere.
    peers[1].send(PeerId(0), vec![0xCD], 0);
    assert!(peers[1].receive(0).is_empty());
    let now = until_ready(&mut peers);
    for peer in &mut peers {
        assert!(peer.receive(now).is_empty());
        assert_eq!(peer.strays(), 5);
    }
    assert_eq!(peers[0].failed_sends(), 0);
    assert_eq!(peers[1].failed_sends(), 1);
}

#[test]
fn a_room_flood_after_the_layout_is_drained() {
    let mesh = MemoryMesh::new();
    let mut peers = room(&mesh, 2, clear());
    let now = until_ready(&mut peers);
    let target = peers[1].layout().unwrap().links[1];
    let mut flooder = mesh.join();
    let (flooder_id, _) = identify(&mut flooder);
    peers[1].poll(now);
    assert!(peers[1].room_drops().is_empty());
    for _ in 0..10_000 {
        flooder.send(Channel::Room, target, vec![0xFF; 64]).unwrap();
    }
    assert!(matches!(peers[1].poll(now), OnlineStatus::Ready(_)));
    let drops = peers[1].room_drops();
    assert_eq!(drops.len(), 10_000);
    assert!(
        drops
            .iter()
            .all(|drop| matches!(drop, RoomEvent::Dropped { link, .. } if *link == flooder_id))
    );
    assert!(peers[1].room_drops().is_empty());
    peers[1].poll(now + STEP);
    assert!(peers[1].room_drops().is_empty(), "nothing was kept");
}

#[test]
fn a_newcomer_after_the_layout_is_refused_as_full() {
    let mesh = MemoryMesh::new();
    let mut peers = room(&mesh, 2, clear());
    let now = until_ready(&mut peers);
    let mut newcomer = OnlinePeer::new(mesh.join(), RoomRole::Join, clear()).unwrap();
    newcomer.set_ready(1).unwrap();
    let mut status = newcomer.poll(now);
    for step in 1..10 {
        for peer in &mut peers {
            assert!(matches!(
                peer.poll(now + step * STEP),
                OnlineStatus::Ready(_)
            ));
        }
        status = newcomer.poll(now + step * STEP);
    }
    assert_eq!(status, OnlineStatus::Failed(RoomError::Full.to_string()));
    assert!(newcomer.layout().is_none());
}
