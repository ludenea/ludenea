//! Real WebRTC between links of this process, through an in-process signaling listener and
//! without STUN: the peers reach each other on this machine's network address (webrtc-rs
//! leaves loopback out, so the machine needs another interface).

use std::collections::BTreeSet;
use std::net::{SocketAddr, TcpListener as StdListener, TcpStream};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use ludenea_net::{
    Channel, Link, LinkConditions, LinkError, LinkEvent, LinkId, MAX_PACKET_BYTES,
    MAX_ROOM_MESSAGE_BYTES, OnlinePeer, OnlineStatus, RoomCode, RoomRole,
};
use ludenea_signal::{Options, serve};
use ludenea_webrtc::{LinkSettings, WebRtcLink};
use matchbox_protocol::{JsonPeerEvent, JsonPeerRequest};
use serde_json::json;
use tungstenite::stream::MaybeTlsStream;
use tungstenite::{Message, WebSocket};

/// How long each wait may take.
const TIMEOUT: Duration = Duration::from_secs(30);

#[expect(
    clippy::disallowed_methods,
    reason = "the tests bound their waits with wall-clock time"
)]
fn now() -> Instant {
    Instant::now()
}

/// A signaling listener on a port of `127.0.0.1` the system picks, in a runtime of its own
/// thread; [`Listener::stop`] ends it and every connection it serves.
struct Listener {
    address: SocketAddr,
    stop: Option<tokio::sync::oneshot::Sender<()>>,
    thread: Option<JoinHandle<()>>,
}

impl Listener {
    fn start() -> Listener {
        let listener = StdListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        listener.set_nonblocking(true).unwrap();
        let (stop, stopped) = tokio::sync::oneshot::channel::<()>();
        let thread = std::thread::spawn(move || {
            let runtime = tokio::runtime::Runtime::new().unwrap();
            runtime.block_on(async move {
                let listener = tokio::net::TcpListener::from_std(listener).unwrap();
                tokio::select! {
                    result = serve(vec![listener], Options::default()) => result.unwrap(),
                    _ = stopped => {}
                }
            });
            // Dropping the runtime ends every connection it serves.
        });
        Listener {
            address,
            stop: Some(stop),
            thread: Some(thread),
        }
    }

    fn signaling(&self) -> String {
        format!("ws://{}/rooms", self.address)
    }

    fn stop(&mut self) {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
        if let Some(thread) = self.thread.take() {
            thread.join().unwrap();
        }
    }
}

impl Drop for Listener {
    fn drop(&mut self) {
        self.stop();
    }
}

fn code(text: &str) -> RoomCode {
    RoomCode::parse(text).unwrap()
}

fn open(signaling: &str, code: &RoomCode, host: Option<u8>) -> WebRtcLink {
    WebRtcLink::open(LinkSettings::new(signaling, code.clone(), host, &[]).unwrap())
}

/// A link and what its events said so far.
struct Peer {
    link: WebRtcLink,
    events: Vec<LinkEvent>,
    id: Option<LinkId>,
    connected: BTreeSet<LinkId>,
    closed: Option<String>,
}

impl Peer {
    fn new(link: WebRtcLink) -> Peer {
        Peer {
            link,
            events: Vec::new(),
            id: None,
            connected: BTreeSet::new(),
            closed: None,
        }
    }

    fn pump(&mut self) {
        for event in self.link.poll() {
            match &event {
                LinkEvent::Assigned(id) => self.id = Some(*id),
                LinkEvent::Connected(other) => {
                    self.connected.insert(*other);
                }
                LinkEvent::Disconnected(other) => {
                    self.connected.remove(other);
                }
                LinkEvent::Closed(reason) => self.closed = Some(reason.clone()),
            }
            self.events.push(event);
        }
    }

    fn id(&self) -> LinkId {
        self.id.expect("an assigned link")
    }
}

/// Polls `peers` until `done` holds, and says whether it did within [`TIMEOUT`].
fn wait_until(peers: &mut [Peer], mut done: impl FnMut(&mut [Peer]) -> bool) -> bool {
    let start = now();
    loop {
        for peer in peers.iter_mut() {
            peer.pump();
        }
        if done(peers) {
            return true;
        }
        if now().duration_since(start) > TIMEOUT {
            return false;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
}

/// Polls `peers` until each is assigned and connected to every other, or panics.
fn mesh(peers: &mut [Peer]) {
    let meshed = wait_until(peers, |peers| {
        let ids: Option<BTreeSet<LinkId>> = peers.iter().map(|peer| peer.id).collect();
        ids.is_some_and(|ids| {
            peers.iter().all(|peer| {
                ids.iter()
                    .all(|id| Some(*id) == peer.id || peer.connected.contains(id))
            })
        })
    });
    let report: Vec<_> = peers.iter().map(|peer| &peer.events).collect();
    assert!(
        meshed,
        "the links were not all assigned and connected to each other within 30 s: {report:?}"
    );
}

/// The host of a new room of `size` peers, once the listener assigned it: the room exists
/// before anyone joins it.
fn hosted(listener: &Listener, room: &RoomCode, size: u8) -> Peer {
    let mut peers = vec![Peer::new(open(&listener.signaling(), room, Some(size)))];
    let assigned = wait_until(&mut peers, |peers| peers[0].id.is_some());
    assert!(
        assigned,
        "the host was not assigned within 30 s: {:?}",
        peers[0].events
    );
    peers.remove(0)
}

/// A host and a joiner of a new room, connected.
fn pair(listener: &Listener, room: &str) -> Vec<Peer> {
    let room = code(room);
    let host = hosted(listener, &room, 2);
    let joiner = Peer::new(open(&listener.signaling(), &room, None));
    let mut peers = vec![host, joiner];
    mesh(&mut peers);
    peers
}

/// A link whose first events were polled already: it reports them again, first.
struct Replayed {
    link: WebRtcLink,
    events: Vec<LinkEvent>,
}

impl Link for Replayed {
    fn poll(&mut self) -> Vec<LinkEvent> {
        let mut events = std::mem::take(&mut self.events);
        events.extend(self.link.poll());
        events
    }

    fn send(&mut self, channel: Channel, to: LinkId, bytes: Vec<u8>) -> Result<(), LinkError> {
        self.link.send(channel, to, bytes)
    }

    fn receive(&mut self, channel: Channel) -> Option<(LinkId, Vec<u8>)> {
        self.link.receive(channel)
    }
}

/// The reason the link closes with, within [`TIMEOUT`].
fn closed(link: WebRtcLink) -> String {
    let mut peers = [Peer::new(link)];
    let done = wait_until(&mut peers, |peers| peers[0].closed.is_some());
    let [peer] = peers;
    assert!(
        done,
        "the link did not close within 30 s: {:?}",
        peer.events
    );
    peer.closed.unwrap()
}

#[test]
fn two_links_connect_and_carry_both_channels() {
    let listener = Listener::start();
    let mut peers = pair(&listener, "ABCDEF");
    let (host, joiner) = (peers[0].id(), peers[1].id());
    assert_eq!(
        peers[0].events,
        vec![LinkEvent::Assigned(host), LinkEvent::Connected(joiner)]
    );
    assert_eq!(
        peers[1].events,
        vec![LinkEvent::Assigned(joiner), LinkEvent::Connected(host)]
    );

    for index in 0..100u8 {
        peers[0]
            .link
            .send(Channel::Room, joiner, vec![index; 300])
            .unwrap();
        peers[1]
            .link
            .send(Channel::Match, host, vec![index; 1000])
            .unwrap();
    }
    let mut room = Vec::new();
    let mut matches = Vec::new();
    let done = wait_until(&mut peers, |peers| {
        while let Some(message) = peers[1].link.receive(Channel::Room) {
            room.push(message);
        }
        while let Some(packet) = peers[0].link.receive(Channel::Match) {
            matches.push(packet);
        }
        room.len() == 100 && matches.len() == 100
    });
    assert!(
        done,
        "within 30 s, {} of 100 room messages and {} of 100 match packets arrived",
        room.len(),
        matches.len()
    );
    let expected: Vec<_> = (0..100u8).map(|index| (host, vec![index; 300])).collect();
    assert_eq!(
        room, expected,
        "the room messages arrive whole and in order"
    );
    let mut received: Vec<_> = matches
        .iter()
        .map(|(from, packet)| (*from, packet[0]))
        .collect();
    received.sort();
    let expected: Vec<_> = (0..100u8).map(|index| (joiner, index)).collect();
    assert_eq!(received, expected, "every match packet arrives once");
    assert!(matches.iter().all(|(_, packet)| packet.len() == 1000));
    assert!(peers[0].link.receive(Channel::Room).is_none());
    assert!(peers[1].link.receive(Channel::Match).is_none());
}

#[test]
fn three_online_peers_agree_on_a_layout() {
    let listener = Listener::start();
    let room = code("KMNPQR");
    let conditions = LinkConditions {
        latency_micros: 0,
        jitter_micros: 0,
        loss_per_mille: 0,
        duplicate_per_mille: 0,
        reorder_per_mille: 0,
        reorder_extra_micros: 0,
        seed: 1,
    };
    let host = hosted(&listener, &room, 3);
    let host_id = host.id();
    let host: Box<dyn Link> = Box::new(Replayed {
        link: host.link,
        events: host.events,
    });
    let mut online = vec![
        OnlinePeer::new(
            host,
            RoomRole::Host {
                peers: 3,
                match_id: 7,
            },
            conditions,
        )
        .unwrap(),
    ];
    for _ in 0..2 {
        let joiner: Box<dyn Link> = Box::new(open(&listener.signaling(), &room, None));
        online.push(OnlinePeer::new(joiner, RoomRole::Join, conditions).unwrap());
    }
    for peer in &mut online {
        peer.set_ready(1).unwrap();
    }
    let start = now();
    let layouts = loop {
        let micros = u64::try_from(now().duration_since(start).as_micros()).unwrap();
        let statuses: Vec<_> = online.iter_mut().map(|peer| peer.poll(micros)).collect();
        let layouts: Vec<_> = statuses
            .iter()
            .filter_map(|status| match status {
                OnlineStatus::Ready(layout) => Some(layout.clone()),
                _ => None,
            })
            .collect();
        if layouts.len() == 3 {
            break layouts;
        }
        if let Some(failed) = statuses
            .iter()
            .find(|status| matches!(status, OnlineStatus::Failed(_)))
        {
            panic!("a peer failed: {failed:?}");
        }
        assert!(
            now().duration_since(start) < TIMEOUT,
            "the three peers did not agree on a layout within 30 s: {statuses:?}"
        );
        std::thread::sleep(Duration::from_millis(5));
    };
    // The host is peer 0; the joiners take 1 and 2 in the order the host met them.
    assert_eq!(layouts[0].local.0, 0, "the host first");
    let locals: BTreeSet<u8> = layouts.iter().map(|layout| layout.local.0).collect();
    assert_eq!(locals, BTreeSet::from([0, 1, 2]), "{layouts:?}");
    for layout in &layouts {
        assert_eq!(layout.links, layouts[0].links);
        assert_eq!(layout.players, layouts[0].players);
        assert_eq!(layout.match_id, 7);
    }
    assert_eq!(layouts[0].links.len(), 3);
    assert_eq!(layouts[0].links[0], host_id);
}

#[test]
fn a_leaving_peer_disconnects() {
    let listener = Listener::start();
    let mut peers = pair(&listener, "BCDEFG");
    let joiner = peers.pop().unwrap();
    let joiner_id = joiner.id();
    drop(joiner);
    let done = wait_until(&mut peers, |peers| peers[0].connected.is_empty());
    assert!(
        done,
        "the host did not see the joiner leave within 30 s: {:?}",
        peers[0].events
    );
    assert_eq!(
        peers[0].events.last(),
        Some(&LinkEvent::Disconnected(joiner_id))
    );
    assert_eq!(peers[0].closed, None, "the host's link lives on");
    assert_eq!(
        peers[0].link.send(Channel::Room, joiner_id, vec![1]),
        Err(LinkError::UnknownLink { link: joiner_id })
    );
}

#[test]
fn a_stopped_listener_disconnects_and_closes() {
    let mut listener = Listener::start();
    let mut peers = pair(&listener, "CDEFGH");
    let ids = [peers[0].id(), peers[1].id()];
    listener.stop();
    let done = wait_until(&mut peers, |peers| {
        peers.iter().all(|peer| peer.closed.is_some())
    });
    assert!(
        done,
        "the links did not close within 30 s of the listener stopping: {:?} {:?}",
        peers[0].events, peers[1].events
    );
    for (index, peer) in peers.iter().enumerate() {
        let other = ids[1 - index];
        let tail = &peer.events[peer.events.len() - 2..];
        assert_eq!(tail[0], LinkEvent::Disconnected(other), "{:?}", peer.events);
        let LinkEvent::Closed(reason) = &tail[1] else {
            panic!("the last event is not Closed: {:?}", peer.events);
        };
        assert!(
            reason.starts_with(&format!(
                "lost the signaling server at {}",
                listener.signaling()
            )),
            "{reason}"
        );
    }
    let mut host = peers.swap_remove(0);
    assert_eq!(
        host.link.send(Channel::Room, ids[1], vec![1]),
        Err(LinkError::Closed)
    );
    host.pump();
    assert!(
        host.link.poll().is_empty(),
        "a closed link reports nothing more"
    );
}

#[test]
fn a_send_to_a_peer_never_connected_is_refused_and_the_link_lives_on() {
    let listener = Listener::start();
    let room = code("DEFGHJ");
    let mut peers = vec![hosted(&listener, &room, 2)];
    let stranger = LinkId([7; 16]);
    assert_eq!(
        peers[0].link.send(Channel::Match, stranger, vec![1]),
        Err(LinkError::UnknownLink { link: stranger })
    );
    assert_eq!(
        peers[0].link.send(Channel::Room, stranger, vec![1]),
        Err(LinkError::UnknownLink { link: stranger })
    );

    peers.push(Peer::new(open(&listener.signaling(), &room, None)));
    mesh(&mut peers);
    let joiner = peers[1].id();
    assert_eq!(
        peers[0].link.send(Channel::Room, joiner, vec![0; 513]),
        Err(LinkError::TooLarge { len: 513, max: 512 })
    );
    peers[0].link.send(Channel::Room, joiner, vec![9]).unwrap();
    let mut message = None;
    let done = wait_until(&mut peers, |peers| {
        message = peers[1].link.receive(Channel::Room);
        message.is_some()
    });
    assert!(done, "the room message did not arrive within 30 s");
    assert_eq!(message, Some((peers[0].id(), vec![9])));
}

/// A room member speaking matchbox's protocol by hand: every peer it meets gets a malformed
/// offer, every offer it gets a malformed answer, and every signal a malformed candidate. It
/// returns once the listener assigned it, and answers until `stop`.
fn hostile_member(address: SocketAddr, room: &RoomCode, stop: Arc<AtomicBool>) -> JoinHandle<()> {
    let url = format!("ws://{address}/rooms/{room}");
    let (mut socket, _) = tungstenite::connect(url).expect("the hostile member joins");
    if let MaybeTlsStream::Plain(stream) = socket.get_ref() {
        stream.set_read_timeout(Some(TIMEOUT)).unwrap();
    }
    match socket.read().expect("the hostile member's id within 30 s") {
        Message::Text(text) => assert!(
            matches!(
                text.parse::<JsonPeerEvent>(),
                Ok(JsonPeerEvent::IdAssigned(_))
            ),
            "expected IdAssigned first, got {text}"
        ),
        other => panic!("expected IdAssigned first, got {other:?}"),
    }
    if let MaybeTlsStream::Plain(stream) = socket.get_ref() {
        stream
            .set_read_timeout(Some(Duration::from_millis(50)))
            .unwrap();
    }
    std::thread::spawn(move || {
        let send = |socket: &mut WebSocket<MaybeTlsStream<TcpStream>>, request: JsonPeerRequest| {
            socket
                .send(Message::Text(request.to_string().into()))
                .unwrap();
        };
        while !stop.load(Ordering::Relaxed) {
            let text = match socket.read() {
                Ok(Message::Text(text)) => text,
                Ok(_) => continue,
                Err(tungstenite::Error::Io(error))
                    if matches!(
                        error.kind(),
                        std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                    ) =>
                {
                    continue;
                }
                Err(_) => return,
            };
            let reply = match text.parse::<JsonPeerEvent>() {
                Ok(JsonPeerEvent::NewPeer(peer)) => {
                    Some((peer, json!({ "Offer": "not an offer" })))
                }
                Ok(JsonPeerEvent::Signal { sender, data }) if data.get("Offer").is_some() => {
                    Some((sender, json!({ "Answer": "not an answer" })))
                }
                _ => None,
            };
            if let Some((receiver, data)) = reply {
                send(&mut socket, JsonPeerRequest::Signal { receiver, data });
                let candidate = json!({ "IceCandidate": "{\"candidate\":\"not a candidate\"}" });
                send(
                    &mut socket,
                    JsonPeerRequest::Signal {
                        receiver,
                        data: candidate,
                    },
                );
            }
        }
    })
}

#[test]
fn a_malformed_offer_does_not_kill_the_link() {
    let listener = Listener::start();
    let room = code("EFGHJK");
    let mut peers = vec![hosted(&listener, &room, 3)];
    let stop = Arc::new(AtomicBool::new(false));
    // In the room before the joiner, so it is the one that offers to the joiner, and the host
    // offers to it.
    let hostile = hostile_member(listener.address, &room, Arc::clone(&stop));
    peers.push(Peer::new(open(&listener.signaling(), &room, None)));
    mesh(&mut peers);
    let (host, joiner) = (peers[0].id(), peers[1].id());

    peers[0].link.send(Channel::Room, joiner, vec![1]).unwrap();
    peers[1].link.send(Channel::Room, host, vec![2]).unwrap();
    let mut received = [None, None];
    let done = wait_until(&mut peers, |peers| {
        for (index, peer) in peers.iter_mut().enumerate() {
            if let Some(message) = peer.link.receive(Channel::Room) {
                received[index] = Some(message);
            }
        }
        received.iter().all(Option::is_some)
    });
    assert!(
        done,
        "the room messages did not arrive within 30 s: {received:?}"
    );
    assert_eq!(received, [Some((joiner, vec![2])), Some((host, vec![1]))]);
    for peer in &peers {
        assert_eq!(peer.closed, None, "{:?}", peer.events);
        assert_eq!(
            peer.connected.len(),
            1,
            "only the honest peer connects: {:?}",
            peer.events
        );
    }
    stop.store(true, Ordering::Relaxed);
    hostile.join().unwrap();
}

#[test]
fn a_peer_leaving_mid_handshake_does_not_kill_the_link() {
    let listener = Listener::start();
    let room = code("PQRSTU");
    let mut peers = vec![hosted(&listener, &room, 2)];
    let url = format!("ws://{}/rooms/{room}", listener.address);
    let (mut member, _) = tungstenite::connect(url).expect("the member joins");
    if let MaybeTlsStream::Plain(stream) = member.get_ref() {
        stream.set_read_timeout(Some(TIMEOUT)).unwrap();
    }
    // The host offers to the member, which leaves instead of answering.
    let offered = std::iter::from_fn(|| match member.read() {
        Ok(Message::Text(text)) => Some(text.parse::<JsonPeerEvent>().ok()),
        Ok(_) => Some(None),
        Err(_) => None,
    })
    .flatten()
    .any(
        |event| matches!(event, JsonPeerEvent::Signal { data, .. } if data.get("Offer").is_some()),
    );
    assert!(offered, "the host did not offer to the member within 30 s");
    drop(member);

    peers.push(Peer::new(open(&listener.signaling(), &room, None)));
    mesh(&mut peers);
    assert_eq!(peers[0].closed, None, "{:?}", peers[0].events);
}

#[test]
fn an_unknown_room_closes_naming_it() {
    let listener = Listener::start();
    let signaling = listener.signaling();
    let reason = closed(open(&signaling, &code("FGHJKM"), None));
    assert_eq!(
        reason,
        format!(
            "no room FGHJKM at {signaling}: check the code, or there is no Ludenea listener there"
        )
    );
}

#[test]
fn a_room_in_use_closes_naming_it() {
    let listener = Listener::start();
    let signaling = listener.signaling();
    let room = code("GHJKMN");
    let _host = hosted(&listener, &room, 2);
    let reason = closed(open(&signaling, &room, Some(2)));
    assert_eq!(
        reason,
        format!("room GHJKMN is in use at {signaling}: host again for a new code")
    );
}

#[test]
fn an_unreachable_server_closes_naming_it() {
    // A socket bound and never listening holds the port, so nothing else can take it, and
    // refuses every connection.
    let bound = tokio::net::TcpSocket::new_v4().unwrap();
    bound.bind("127.0.0.1:0".parse().unwrap()).unwrap();
    let address = bound.local_addr().unwrap();
    let signaling = format!("ws://{address}/rooms");
    let reason = closed(open(&signaling, &code("HJKMNP"), None));
    let prefix = format!("cannot reach the signaling server at {signaling}: ");
    assert!(
        reason.starts_with(&prefix) && reason.len() > prefix.len(),
        "{reason}"
    );
    drop(bound);
}

#[test]
fn a_silent_server_closes_after_10_s() {
    // Connections queue in the backlog and are never answered.
    let silent = StdListener::bind("127.0.0.1:0").unwrap();
    let signaling = format!("ws://{}/rooms", silent.local_addr().unwrap());
    let start = now();
    let reason = closed(open(&signaling, &code("JKMNPQ"), None));
    let waited = now().duration_since(start);
    assert_eq!(
        reason,
        format!("no answer from the signaling server at {signaling} within 10 s")
    );
    // Not before 10 s; the upper bound only catches a wait that never ends, on a loaded machine.
    assert!(
        (Duration::from_secs(10)..Duration::from_secs(25)).contains(&waited),
        "closed after {waited:?}"
    );
    drop(silent);
}

#[test]
fn an_oversized_message_is_dropped_and_counted() {
    let listener = Listener::start();
    let room = code("QRSTUV");
    let mut peers = vec![hosted(&listener, &room, 2)];
    let host = peers[0].id();
    // A peer that does not run Ludenea's link, so nothing bounds what it sends.
    let (mut socket, message_loop) =
        matchbox_socket::WebRtcSocketBuilder::new(format!("{}/{room}", listener.signaling()))
            .ice_server(matchbox_socket::RtcIceServerConfig {
                urls: Vec::new(),
                username: None,
                credential: None,
            })
            .add_channel(matchbox_socket::ChannelConfig::unreliable())
            .add_channel(matchbox_socket::ChannelConfig::reliable())
            .build();
    let raw_loop = std::thread::spawn(move || futures::executor::block_on(message_loop));
    let host_peer = matchbox_socket::PeerId(uuid::Uuid::from_bytes(host.0));
    let connected = wait_until(&mut peers, |_| {
        socket.update_peers();
        socket.connected_peers().any(|peer| peer == host_peer)
    });
    assert!(connected, "the raw peer did not connect within 30 s");

    let sends = [
        (1, vec![1; MAX_ROOM_MESSAGE_BYTES + 1]),
        (1, vec![2; MAX_ROOM_MESSAGE_BYTES]),
        (0, vec![3; MAX_PACKET_BYTES + 1]),
        (0, vec![4; MAX_PACKET_BYTES]),
    ];
    for (channel, bytes) in sends {
        socket
            .channel_mut(channel)
            .send(bytes.into_boxed_slice(), host_peer);
    }
    let mut room_messages = Vec::new();
    let mut match_packets = Vec::new();
    let done = wait_until(&mut peers, |peers| {
        while let Some((_, message)) = peers[0].link.receive(Channel::Room) {
            room_messages.push(message[0]);
        }
        while let Some((_, packet)) = peers[0].link.receive(Channel::Match) {
            match_packets.push(packet[0]);
        }
        peers[0].link.oversized() == 2 && room_messages.len() == 1 && match_packets.len() == 1
    });
    assert!(
        done,
        "within 30 s, {} oversized, room messages {room_messages:?}, match packets \
         {match_packets:?}",
        peers[0].link.oversized()
    );
    assert_eq!((room_messages, match_packets), (vec![2], vec![4]));

    socket.close();
    let ended = raw_loop.join().expect("the raw peer's loop does not panic");
    assert!(ended.is_ok(), "{ended:?}");
}
