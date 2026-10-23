//! Peer connections are released when their peer goes. A member that joins, gets an offer,
//! and leaves, or connects and leaves, again and again, must not grow the link's peer
//! connections (their UDP sockets and tasks) without bound.
//!
//! The count is this process's open file descriptors, so this binary holds one test: nothing
//! else opens or closes descriptors meanwhile. Unix only: it lists them in `/dev/fd`, which
//! Windows lacks, and the peer connections it checks are the same code on every platform.

#![cfg(unix)]

use std::net::{SocketAddr, TcpListener as StdListener, TcpStream};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use ludenea_net::{Link, LinkEvent, RoomCode};
use ludenea_signal::{Options, serve};
use ludenea_webrtc::{LinkSettings, WebRtcLink};
use matchbox_protocol::{JsonPeerEvent, JsonPeerRequest, PeerId};
use serde_json::json;
use tungstenite::stream::MaybeTlsStream;
use tungstenite::{Message, WebSocket};

/// How long each wait may take.
const TIMEOUT: Duration = Duration::from_secs(30);

/// Join-and-leave cycles of each kind.
const CYCLES: usize = 24;

/// Descriptors the process may hold above its count before the cycles, for what the runtimes
/// open lazily (up to 5 seen). A leaked peer connection holds at least one, so a leak shows as
/// at least [`CYCLES`].
const SLACK: usize = 8;

#[expect(
    clippy::disallowed_methods,
    reason = "the tests bound their waits with wall-clock time"
)]
fn now() -> Instant {
    Instant::now()
}

fn open_descriptors() -> usize {
    std::fs::read_dir("/dev/fd")
        .expect("this system lists its descriptors in /dev/fd")
        .count()
}

/// A signaling listener on a port of `127.0.0.1`, in a runtime of its own thread, for the
/// whole test.
fn start_listener() -> (SocketAddr, JoinHandle<()>) {
    let listener = StdListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    listener.set_nonblocking(true).unwrap();
    let thread = std::thread::spawn(move || {
        let runtime = tokio::runtime::Runtime::new().unwrap();
        runtime.block_on(async move {
            let listener = tokio::net::TcpListener::from_std(listener).unwrap();
            serve(vec![listener], Options::default()).await.unwrap();
        });
    });
    (address, thread)
}

/// Polls `link` for `duration`, failing if it closes.
fn pump(link: &mut WebRtcLink, duration: Duration) {
    let start = now();
    while now().duration_since(start) < duration {
        for event in link.poll() {
            assert!(
                !matches!(event, LinkEvent::Closed(_)),
                "the host's link closed: {event:?}"
            );
        }
        std::thread::sleep(Duration::from_millis(5));
    }
}

type Member = WebSocket<MaybeTlsStream<TcpStream>>;

/// A room member speaking matchbox's protocol by hand, once the listener assigned it.
fn join(address: SocketAddr, room: &RoomCode) -> Member {
    let (member, _) =
        tungstenite::connect(format!("ws://{address}/rooms/{room}")).expect("the member joins");
    if let MaybeTlsStream::Plain(stream) = member.get_ref() {
        stream.set_read_timeout(Some(TIMEOUT)).unwrap();
    }
    member
}

/// Reads `member`'s messages, polling `host` meanwhile, until a signal from the host matches
/// `wanted`; returns the host's id and the signal's data.
fn signal(member: &mut Member, host: &mut WebRtcLink, wanted: &str) -> (PeerId, serde_json::Value) {
    if let MaybeTlsStream::Plain(stream) = member.get_ref() {
        stream
            .set_read_timeout(Some(Duration::from_millis(20)))
            .unwrap();
    }
    let start = now();
    loop {
        assert!(
            now().duration_since(start) < TIMEOUT,
            "no {wanted} from the host within 30 s"
        );
        pump(host, Duration::ZERO);
        let text = match member.read() {
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
            Err(error) => panic!("the member lost the listener: {error}"),
        };
        if let Ok(JsonPeerEvent::Signal { sender, data }) = text.parse::<JsonPeerEvent>()
            && data.get(wanted).is_some()
        {
            return (sender, data);
        }
    }
}

fn send(member: &mut Member, receiver: PeerId, data: serde_json::Value) {
    member
        .send(Message::Text(
            JsonPeerRequest::Signal { receiver, data }
                .to_string()
                .into(),
        ))
        .unwrap();
}

/// A well-formed answer to a data channel offer, from a peer that will never connect.
fn answer() -> serde_json::Value {
    let fingerprint = vec!["AB"; 32].join(":");
    let sdp = [
        "v=0",
        "o=- 1 1 IN IP4 127.0.0.1",
        "s=-",
        "t=0 0",
        "a=group:BUNDLE 0",
        "m=application 9 UDP/DTLS/SCTP webrtc-datachannel",
        "c=IN IP4 0.0.0.0",
        "a=ice-ufrag:leaver",
        "a=ice-pwd:leaverleaverleaverleaver",
        &format!("a=fingerprint:sha-256 {fingerprint}"),
        "a=setup:active",
        "a=mid:0",
        "a=sctp-port:5000",
        "",
    ]
    .join("\r\n");
    json!({ "Answer": sdp })
}

/// Waits, polling `host`, until the process holds at most `limit` descriptors, and says how
/// many it holds.
fn settle(host: &mut WebRtcLink, limit: usize) -> usize {
    let start = now();
    loop {
        let count = open_descriptors();
        if count <= limit || now().duration_since(start) > TIMEOUT {
            return count;
        }
        pump(host, Duration::from_millis(50));
    }
}

#[test]
fn abandoned_handshakes_release_their_connections() {
    let (address, _listener) = start_listener();
    let room = RoomCode::parse("MNPQRS").unwrap();
    let signaling = format!("ws://{address}/rooms");
    let settings = LinkSettings::new(&signaling, room.clone(), Some(2), &[]).unwrap();
    let mut host = WebRtcLink::open(settings);
    let start = now();
    while !host
        .poll()
        .iter()
        .any(|event| matches!(event, LinkEvent::Assigned(_)))
    {
        assert!(
            now().duration_since(start) < TIMEOUT,
            "the host was not assigned within 30 s"
        );
        std::thread::sleep(Duration::from_millis(5));
    }
    // One cycle first, so that what the runtime opens once is counted in the baseline.
    let member = join(address, &room);
    drop(member);
    pump(&mut host, Duration::from_secs(1));
    let baseline = open_descriptors();

    // Members that leave before answering the host's offer.
    for _ in 0..CYCLES {
        let mut member = join(address, &room);
        signal(&mut member, &mut host, "Offer");
        drop(member);
        pump(&mut host, Duration::from_millis(100));
    }
    let count = settle(&mut host, baseline + SLACK);
    assert!(
        count <= baseline + SLACK,
        "{CYCLES} members that left before answering left {count} descriptors open, from \
         {baseline}"
    );

    // Members that answer, send a candidate the host refuses, and leave before connecting.
    for _ in 0..CYCLES {
        let mut member = join(address, &room);
        let (host_id, _) = signal(&mut member, &mut host, "Offer");
        send(&mut member, host_id, answer());
        // The host trickles its candidates once it took the answer.
        signal(&mut member, &mut host, "IceCandidate");
        let refused = json!({
            "IceCandidate": "{\"candidate\":\"candidate:refused\",\"sdpMid\":\"0\",\"sdpMLineIndex\":0}"
        });
        send(&mut member, host_id, refused);
        pump(&mut host, Duration::from_millis(100));
        drop(member);
        pump(&mut host, Duration::from_millis(100));
    }
    let count = settle(&mut host, baseline + SLACK);
    assert!(
        count <= baseline + SLACK,
        "{CYCLES} members that left after a refused candidate left {count} descriptors open, \
         from {baseline}"
    );

    // Links that connect to the host, then leave.
    for _ in 0..CYCLES {
        let settings = LinkSettings::new(&signaling, room.clone(), None, &[]).unwrap();
        let mut joiner = WebRtcLink::open(settings);
        let start = now();
        let mut connected = [false, false];
        while connected != [true, true] {
            assert!(
                now().duration_since(start) < TIMEOUT,
                "the joiner and the host did not connect within 30 s: {connected:?}"
            );
            for (index, link) in [&mut host, &mut joiner].into_iter().enumerate() {
                for event in link.poll() {
                    match event {
                        LinkEvent::Connected(_) => connected[index] = true,
                        LinkEvent::Closed(reason) => panic!("a link closed: {reason}"),
                        _ => {}
                    }
                }
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        drop(joiner);
        let start = now();
        while !host
            .poll()
            .iter()
            .any(|event| matches!(event, LinkEvent::Disconnected(_)))
        {
            assert!(
                now().duration_since(start) < TIMEOUT,
                "the host did not see the joiner leave within 30 s"
            );
            std::thread::sleep(Duration::from_millis(5));
        }
    }
    let count = settle(&mut host, baseline + SLACK);
    assert!(
        count <= baseline + SLACK,
        "{CYCLES} links that connected and left left {count} descriptors open, from {baseline}"
    );
}
