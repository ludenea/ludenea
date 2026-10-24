//! The signaling listener on a free port, in a background runtime, driven by blocking
//! WebSocket clients as matchbox_socket would drive it.

use std::fs;
use std::io::{ErrorKind, Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::sync::{Arc, Barrier};
use std::time::{Duration, Instant};

use ludenea_signal::{Limits, Options, serve};
use matchbox_protocol::{JsonPeerEvent, JsonPeerRequest, PeerId};
use serde_json::json;
use tempfile::TempDir;
use tungstenite::client::IntoClientRequest;
use tungstenite::protocol::frame::coding::CloseCode;
use tungstenite::stream::MaybeTlsStream;
use tungstenite::{Message, WebSocket};

const TIMEOUT: Duration = Duration::from_secs(10);

type Socket = WebSocket<MaybeTlsStream<TcpStream>>;

#[expect(
    clippy::disallowed_methods,
    reason = "the tests bound the listener's timeouts with wall-clock time"
)]
fn now() -> Instant {
    Instant::now()
}

/// Starts a listener with `options` on a port the system picks, and returns its address.
fn start(options: Options) -> SocketAddr {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    listener.set_nonblocking(true).unwrap();
    std::thread::spawn(move || {
        let runtime = tokio::runtime::Runtime::new().unwrap();
        runtime.block_on(async move {
            let listener = tokio::net::TcpListener::from_std(listener).unwrap();
            serve(vec![listener], options).await.unwrap();
        });
    });
    address
}

fn start_with(limits: Limits) -> SocketAddr {
    start(Options {
        limits,
        ..Options::default()
    })
}

fn url(address: SocketAddr, path: &str) -> String {
    format!("ws://{address}{path}")
}

fn connect_with(
    address: SocketAddr,
    path: &str,
    origin: Option<&str>,
) -> Result<Socket, (u16, String)> {
    let mut request = url(address, path).into_client_request().unwrap();
    if let Some(origin) = origin {
        request
            .headers_mut()
            .insert("Origin", origin.parse().unwrap());
    }
    match tungstenite::connect(request) {
        Ok((socket, _)) => {
            if let MaybeTlsStream::Plain(stream) = socket.get_ref() {
                stream.set_read_timeout(Some(TIMEOUT)).unwrap();
            }
            Ok(socket)
        }
        Err(tungstenite::Error::Http(response)) => {
            let body = response.body().clone().unwrap_or_default();
            Err((response.status().as_u16(), String::from_utf8(body).unwrap()))
        }
        Err(error) => panic!("cannot connect to {path}: {error}"),
    }
}

fn connect(address: SocketAddr, path: &str) -> Socket {
    connect_with(address, path, None)
        .unwrap_or_else(|(status, body)| panic!("{path} refused: {status} {body}"))
}

fn refused(address: SocketAddr, path: &str) -> (u16, String) {
    match connect_with(address, path, None) {
        Ok(_) => panic!("{path} was admitted"),
        Err(refusal) => refusal,
    }
}

fn event(socket: &mut Socket) -> JsonPeerEvent {
    loop {
        match socket.read().expect("an event in time") {
            Message::Text(text) => return text.parse().expect("a peer event"),
            Message::Ping(_) | Message::Pong(_) => continue,
            other => panic!("expected an event, got {other:?}"),
        }
    }
}

/// Connects to `path` and returns the socket and the id the listener assigned.
fn join(address: SocketAddr, path: &str) -> (Socket, PeerId) {
    let mut socket = connect(address, path);
    match event(&mut socket) {
        JsonPeerEvent::IdAssigned(id) => (socket, id),
        other => panic!("expected IdAssigned first, got {other:?}"),
    }
}

fn send(socket: &mut Socket, request: &JsonPeerRequest) {
    socket
        .send(Message::Text(request.to_string().into()))
        .unwrap();
}

fn signal(socket: &mut Socket, receiver: PeerId, data: serde_json::Value) {
    send(socket, &JsonPeerRequest::Signal { receiver, data });
}

/// The close code the listener ends `socket` with, reading past any events.
fn closed_with(socket: &mut Socket) -> CloseCode {
    loop {
        match socket.read() {
            Ok(Message::Close(Some(frame))) => return frame.code,
            Ok(Message::Close(None)) => panic!("closed without a code"),
            Ok(_) => continue,
            Err(error) => panic!("the connection ended without a close frame: {error}"),
        }
    }
}

/// Whether `socket` gets a message within `wait`, which must be below [`TIMEOUT`].
fn quiet_for(socket: &mut Socket, wait: Duration) -> bool {
    set_read_timeout(socket, Some(wait));
    let result = match socket.read() {
        Err(tungstenite::Error::Io(error))
            if matches!(error.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) =>
        {
            true
        }
        Ok(message) => panic!("expected nothing, got {message:?}"),
        Err(error) => panic!("expected nothing, got {error}"),
    };
    set_read_timeout(socket, Some(TIMEOUT));
    result
}

fn set_read_timeout(socket: &Socket, timeout: Option<Duration>) {
    if let MaybeTlsStream::Plain(stream) = socket.get_ref() {
        stream.set_read_timeout(timeout).unwrap();
    }
}

#[test]
fn a_host_and_a_joiner_meet() {
    let address = start(Options::default());
    let (mut host, host_id) = join(address, "/rooms/ABCDEF?host=2");
    let (mut joiner, joiner_id) = join(address, "/rooms/abcdef");
    assert_ne!(host_id, joiner_id);
    assert_eq!(event(&mut host), JsonPeerEvent::NewPeer(joiner_id));

    signal(&mut joiner, host_id, json!({ "offer": "sdp" }));
    assert_eq!(
        event(&mut host),
        JsonPeerEvent::Signal {
            sender: joiner_id,
            data: json!({ "offer": "sdp" })
        }
    );
    signal(&mut host, joiner_id, json!({ "answer": "sdp" }));
    assert_eq!(
        event(&mut joiner),
        JsonPeerEvent::Signal {
            sender: host_id,
            data: json!({ "answer": "sdp" })
        }
    );
}

#[test]
fn a_leaving_peer_is_announced() {
    let address = start(Options::default());
    let (mut host, _) = join(address, "/rooms/ABCDEF?host=3");
    let (mut joiner, joiner_id) = join(address, "/rooms/ABCDEF");
    assert_eq!(event(&mut host), JsonPeerEvent::NewPeer(joiner_id));
    joiner.close(None).unwrap();
    assert_eq!(event(&mut host), JsonPeerEvent::PeerLeft(joiner_id));
}

#[test]
fn joining_a_missing_room_is_404() {
    let address = start(Options::default());
    assert_eq!(
        refused(address, "/rooms/ABCDEF"),
        (404, "no room ABCDEF".to_string())
    );
}

#[test]
fn a_second_host_is_409() {
    let address = start(Options::default());
    let _host = join(address, "/rooms/ABCDEF?host=2");
    assert_eq!(
        refused(address, "/rooms/ABCDEF?host=2"),
        (409, "room ABCDEF is in use".to_string())
    );
}

#[test]
fn a_full_room_is_409() {
    let address = start(Options::default());
    let _host = join(address, "/rooms/ABCDEF?host=2");
    let _joiner = join(address, "/rooms/ABCDEF");
    assert_eq!(
        refused(address, "/rooms/ABCDEF"),
        (409, "room ABCDEF is full".to_string())
    );
}

#[test]
fn a_room_whose_host_left_is_410() {
    let address = start(Options::default());
    let (mut host, host_id) = join(address, "/rooms/ABCDEF?host=3");
    let (mut joiner, joiner_id) = join(address, "/rooms/ABCDEF");
    assert_eq!(event(&mut host), JsonPeerEvent::NewPeer(joiner_id));
    host.close(None).unwrap();
    assert_eq!(event(&mut joiner), JsonPeerEvent::PeerLeft(host_id));
    assert_eq!(
        refused(address, "/rooms/ABCDEF"),
        (410, "the host of room ABCDEF left".to_string())
    );
}

#[test]
fn an_invalid_code_or_size_is_400() {
    let address = start(Options::default());
    assert_eq!(
        refused(address, "/rooms/ABC"),
        (
            400,
            "a room code has 6 characters, this one has 3".to_string()
        )
    );
    let (status, body) = refused(address, "/rooms/ABCDE1");
    assert_eq!(status, 400);
    assert!(
        body.contains("'1' (character 6) is not in a room code"),
        "{body}"
    );
    for size in ["1", "5", "two", ""] {
        assert_eq!(
            refused(address, &format!("/rooms/ABCDEF?host={size}")),
            (
                400,
                format!("?host takes the room's size, 2 to 4 peers, not \"{size}\"")
            ),
            "{size}"
        );
    }
    // None of them made a room.
    assert_eq!(refused(address, "/rooms/ABCDEF").0, 404);
}

#[test]
fn rooms_are_bounded() {
    let address = start_with(Limits {
        max_rooms: 2,
        ..Limits::default()
    });
    let _first = join(address, "/rooms/AAAAAA?host=2");
    let _second = join(address, "/rooms/BBBBBB?host=2");
    assert_eq!(
        refused(address, "/rooms/CCCCCC?host=2"),
        (503, "this server holds its maximum of 2 rooms".to_string())
    );
}

#[test]
fn an_address_hosts_a_bounded_number_of_rooms() {
    let address = start_with(Limits {
        rooms_per_address: 2,
        ..Limits::default()
    });
    let _first = join(address, "/rooms/AAAAAA?host=2");
    let _second = join(address, "/rooms/BBBBBB?host=2");
    assert_eq!(
        refused(address, "/rooms/CCCCCC?host=2"),
        (429, "this address hosts 2 rooms already".to_string())
    );
    // Joining is still allowed.
    let _joiner = join(address, "/rooms/AAAAAA");
}

#[test]
fn refused_joins_are_rate_limited() {
    let address = start_with(Limits {
        refused_joins_per_minute: 3,
        ..Limits::default()
    });
    let _host = join(address, "/rooms/AAAAAA?host=2");
    assert_eq!(refused(address, "/rooms/BBBBBB").0, 404);
    assert_eq!(refused(address, "/rooms/AAAAAA?host=2").0, 409);
    let _joiner = join(address, "/rooms/AAAAAA");
    assert_eq!(refused(address, "/rooms/AAAAAA").0, 409);
    let limited = (
        429,
        "too many refused joins from this address; wait a minute".to_string(),
    );
    assert_eq!(refused(address, "/rooms/CCCCCC"), limited);
    assert_eq!(refused(address, "/rooms/CCCCCC?host=2"), limited);
}

#[test]
fn concurrent_joins_never_overfill_a_room() {
    let address = start(Options::default());
    let _host = join(address, "/rooms/ABCDEF?host=2");
    let barrier = Arc::new(Barrier::new(8));
    let joiners: Vec<_> = (0..8)
        .map(|_| {
            let barrier = Arc::clone(&barrier);
            std::thread::spawn(move || {
                barrier.wait();
                connect_with(address, "/rooms/ABCDEF", None)
            })
        })
        .collect();
    let results: Vec<_> = joiners.into_iter().map(|j| j.join().unwrap()).collect();
    let admitted = results.iter().filter(|result| result.is_ok()).count();
    assert_eq!(admitted, 1);
    for result in &results {
        if let Err(refusal) = result {
            assert_eq!(refusal, &(409, "room ABCDEF is full".to_string()));
        }
    }
}

#[test]
fn a_foreign_origin_is_403() {
    let address = start(Options {
        allowed_origins: vec!["HTTP://Allowed.Example/".to_string()],
        ..Options::default()
    });
    assert_eq!(
        connect_with(address, "/rooms/ABCDEF?host=2", Some("http://evil.example")).err(),
        Some((
            403,
            "origin http://evil.example may not open rooms here: pass --allow-origin \
             http://evil.example"
                .to_string()
        ))
    );
    let own = format!("http://{address}");
    assert!(connect_with(address, "/rooms/ABCDEF?host=2", Some(&own)).is_ok());
    assert!(
        connect_with(
            address,
            "/rooms/BBBBBB?host=2",
            Some("http://allowed.example")
        )
        .is_ok()
    );
}

#[test]
fn an_oversized_message_closes() {
    let address = start(Options::default());
    let (mut host, _) = join(address, "/rooms/ABCDEF?host=2");
    let text = "x".repeat(16 * 1024 + 1);
    host.send(Message::Text(text.into())).unwrap();
    assert_eq!(closed_with(&mut host), CloseCode::Size);
}

#[test]
fn a_malformed_message_closes() {
    let address = start(Options::default());
    for text in ["not json", r#"{"Signal":{"receiver":"nope","data":1}}"#] {
        let (mut host, _) = join(address, "/rooms/ABCDEF?host=2");
        host.send(Message::Text(text.into())).unwrap();
        assert_eq!(closed_with(&mut host), CloseCode::Policy, "{text}");
        // The room is gone with its only peer: the next host may take it.
        let deadline = now() + TIMEOUT;
        while connect_with(address, "/rooms/ABCDEF?host=2", None).is_err() {
            assert!(now() < deadline, "the room was never forgotten");
            std::thread::sleep(Duration::from_millis(20));
        }
        // That probe hosted it; wait for its own leaving.
        let deadline = now() + TIMEOUT;
        while refused(address, "/rooms/ABCDEF").0 != 404 {
            assert!(now() < deadline, "the probe's room was never forgotten");
            std::thread::sleep(Duration::from_millis(20));
        }
    }
}

#[test]
fn an_idle_connection_closes() {
    let address = start_with(Limits {
        idle: Duration::from_secs(1),
        ..Limits::default()
    });
    let (mut host, _) = join(address, "/rooms/ABCDEF?host=2");
    let started = now();
    assert_eq!(closed_with(&mut host), CloseCode::Away);
    assert!(started.elapsed() >= Duration::from_millis(900));
}

#[test]
fn keep_alives_keep_a_connection_open() {
    let address = start_with(Limits {
        idle: Duration::from_secs(1),
        ..Limits::default()
    });
    let (mut host, _) = join(address, "/rooms/ABCDEF?host=2");
    let started = now();
    while started.elapsed() < Duration::from_secs(3) {
        assert!(quiet_for(&mut host, Duration::from_millis(300)));
        send(&mut host, &JsonPeerRequest::KeepAlive);
    }
    let (_joiner, joiner_id) = join(address, "/rooms/ABCDEF");
    assert_eq!(event(&mut host), JsonPeerEvent::NewPeer(joiner_id));
}

#[test]
fn a_member_that_never_reads_is_closed() {
    let address = start(Options::default());
    let (mut host, host_id) = join(address, "/rooms/ABCDEF?host=2");
    let (mut joiner, _) = join(address, "/rooms/ABCDEF");
    let data = json!("x".repeat(15 * 1024));
    let deadline = now() + TIMEOUT;
    // The host reads nothing while the joiner floods it, until the listener drops it.
    let left = loop {
        assert!(now() < deadline, "the listener never dropped the host");
        for _ in 0..32 {
            signal(&mut joiner, host_id, data.clone());
        }
        set_read_timeout(&joiner, Some(Duration::from_millis(1)));
        match joiner.read() {
            Ok(Message::Text(text)) => break text.parse::<JsonPeerEvent>().unwrap(),
            Ok(other) => panic!("unexpected {other:?}"),
            Err(tungstenite::Error::Io(error))
                if matches!(error.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) => {}
            Err(error) => panic!("the joiner lost its connection: {error}"),
        }
    };
    assert_eq!(left, JsonPeerEvent::PeerLeft(host_id));
    // What was queued for the host before still arrives, then the close.
    set_read_timeout(&host, Some(TIMEOUT));
    assert_eq!(closed_with(&mut host), CloseCode::Again);
}

#[test]
fn signals_to_another_room_are_dropped() {
    let address = start(Options::default());
    let (mut a, a_id) = join(address, "/rooms/AAAAAA?host=2");
    let (mut a2, a2_id) = join(address, "/rooms/AAAAAA");
    assert_eq!(event(&mut a), JsonPeerEvent::NewPeer(a2_id));
    let (mut b, _) = join(address, "/rooms/BBBBBB?host=2");
    let (mut b2, b2_id) = join(address, "/rooms/BBBBBB");
    assert_eq!(event(&mut b), JsonPeerEvent::NewPeer(b2_id));

    signal(&mut b, a_id, json!("from another room"));
    signal(&mut b, b2_id, json!("in the room"));
    // b's signals are handled in order: once b2 has the second, the first was dropped.
    assert!(matches!(event(&mut b2), JsonPeerEvent::Signal { .. }));
    signal(&mut a2, a_id, json!("from a2"));
    assert_eq!(
        event(&mut a),
        JsonPeerEvent::Signal {
            sender: a2_id,
            data: json!("from a2")
        }
    );
}

#[test]
fn an_empty_room_is_forgotten() {
    let address = start_with(Limits {
        max_rooms: 1,
        ..Limits::default()
    });
    let (mut host, _) = join(address, "/rooms/ABCDEF?host=2");
    host.close(None).unwrap();
    // The listener holds one room at most: another host gets in once the first is forgotten.
    let deadline = now() + TIMEOUT;
    loop {
        match connect_with(address, "/rooms/BBBBBB?host=2", None) {
            Ok(_) => break,
            Err((503, _)) => {
                assert!(now() < deadline, "the room was never forgotten");
                std::thread::sleep(Duration::from_millis(20));
            }
            Err(refusal) => panic!("unexpected {refusal:?}"),
        }
    }
}

#[test]
fn a_slow_request_header_closes() {
    let address = start_with(Limits {
        header_timeout: Duration::from_secs(1),
        ..Limits::default()
    });
    let mut stream = TcpStream::connect(address).unwrap();
    stream.set_read_timeout(Some(TIMEOUT)).unwrap();
    stream.write_all(b"GET / HTTP/1.1\r\nHost: x\r\n").unwrap();
    let started = now();
    let mut response = Vec::new();
    // The listener closes the connection, answering 408 or nothing.
    stream.read_to_end(&mut response).unwrap();
    assert!(started.elapsed() >= Duration::from_millis(900));
    assert!(started.elapsed() < TIMEOUT);
}

/// The status, the headers, and the body of a raw HTTP/1.1 request.
fn http(address: SocketAddr, method: &str, path: &str) -> (u16, String, String) {
    let mut stream = TcpStream::connect(address).unwrap();
    stream.set_read_timeout(Some(TIMEOUT)).unwrap();
    write!(
        stream,
        "{method} {path} HTTP/1.1\r\nHost: {address}\r\nConnection: close\r\n\r\n"
    )
    .unwrap();
    let mut response = String::new();
    stream.read_to_string(&mut response).unwrap();
    let (head, body) = response.split_once("\r\n\r\n").unwrap();
    let status = head.split(' ').nth(1).unwrap().parse().unwrap();
    (status, head.to_ascii_lowercase(), body.to_string())
}

#[test]
fn page_files_are_served_read_only() {
    let page = TempDir::new().unwrap();
    fs::write(page.path().join("index.html"), "<p>game</p>").unwrap();
    fs::create_dir(page.path().join("pkg")).unwrap();
    fs::write(page.path().join("pkg/game.js"), "export {}").unwrap();
    let address = start(Options {
        page: Some(page.path().to_path_buf()),
        ..Options::default()
    });

    let (status, head, body) = http(address, "GET", "/");
    assert_eq!((status, body.as_str()), (200, "<p>game</p>"));
    assert!(head.contains("content-type: text/html"), "{head}");
    let (status, head, body) = http(address, "GET", "/pkg/game.js");
    assert_eq!((status, body.as_str()), (200, "export {}"));
    assert!(head.contains("content-type: text/javascript"), "{head}");
    let (status, _, body) = http(address, "HEAD", "/index.html");
    assert_eq!((status, body.as_str()), (200, ""));
    assert_eq!(http(address, "GET", "/missing.js").0, 404);
    assert_eq!(http(address, "GET", "/pkg/../index.html").0, 400);
    assert_eq!(http(address, "POST", "/index.html").0, 405);
    assert_eq!(http(address, "PUT", "/").0, 405);

    let bare = start(Options::default());
    let (status, _, body) = http(bare, "GET", "/");
    assert_eq!(status, 404);
    assert_eq!(
        body,
        "no page here: pass the folder `ludenea build --web` wrote"
    );
}

/// Whether `stream` gets nothing within `wait`.
fn unanswered(stream: &mut TcpStream, wait: Duration) -> bool {
    stream.set_read_timeout(Some(wait)).unwrap();
    let mut byte = [0];
    let quiet = matches!(
        stream.read(&mut byte),
        Err(error) if matches!(error.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut)
    );
    stream.set_read_timeout(Some(TIMEOUT)).unwrap();
    quiet
}

#[test]
fn held_open_connections_stay_bounded() {
    let page = TempDir::new().unwrap();
    fs::write(page.path().join("index.html"), "<p>game</p>").unwrap();
    let address = start(Options {
        page: Some(page.path().to_path_buf()),
        limits: Limits {
            max_connections: 2,
            header_timeout: Duration::from_secs(30),
            ..Limits::default()
        },
        ..Options::default()
    });
    // Two clients hold every connection the listener serves, sending nothing.
    let held: Vec<TcpStream> = (0..2)
        .map(|_| TcpStream::connect(address).unwrap())
        .collect();
    std::thread::sleep(Duration::from_millis(200));
    let mut waiting = TcpStream::connect(address).unwrap();
    write!(
        waiting,
        "GET / HTTP/1.1\r\nHost: {address}\r\nConnection: close\r\n\r\n"
    )
    .unwrap();
    assert!(unanswered(&mut waiting, Duration::from_millis(500)));
    // Once they go, the waiting client is served.
    drop(held);
    let mut response = String::new();
    waiting.read_to_string(&mut response).unwrap();
    assert!(response.starts_with("HTTP/1.1 200"), "{response}");
    assert!(response.ends_with("<p>game</p>"), "{response}");
}

#[test]
fn a_client_that_stops_reading_a_file_is_cut_off() {
    const SIZE: usize = 48 << 20;
    let page = TempDir::new().unwrap();
    fs::write(page.path().join("index.html"), "<p>game</p>").unwrap();
    fs::write(page.path().join("game.wasm"), vec![7u8; SIZE]).unwrap();
    let address = start(Options {
        page: Some(page.path().to_path_buf()),
        limits: Limits {
            max_connections: 1,
            idle: Duration::from_secs(1),
            ..Limits::default()
        },
        ..Options::default()
    });
    let mut stalled = TcpStream::connect(address).unwrap();
    write!(
        stalled,
        "GET /game.wasm HTTP/1.1\r\nHost: {address}\r\nConnection: close\r\n\r\n"
    )
    .unwrap();
    // It reads nothing; once its writes stay blocked for `idle`, the listener drops it and
    // serves the next client.
    let (status, _, body) = http(address, "GET", "/");
    assert_eq!((status, body.as_str()), (200, "<p>game</p>"));
    let mut received = Vec::new();
    let _ = stalled.read_to_end(&mut received);
    assert!(received.len() < SIZE, "the whole file arrived");
}

#[test]
fn a_close_frame_is_given_time_to_arrive() {
    let address = start_with(Limits {
        idle: Duration::from_secs(1),
        ..Limits::default()
    });
    let (host, _) = join(address, "/rooms/ABCDEF?host=2");
    let MaybeTlsStream::Plain(mut stream) = host.into_inner() else {
        panic!("a plain connection");
    };
    // Read the close frame's bytes past the WebSocket client, which would answer it.
    let mut frame = [0; 4];
    stream.read_exact(&mut frame).unwrap();
    assert_eq!(frame[0], 0x88, "a close frame");
    assert_eq!(u16::from_be_bytes([frame[2], frame[3]]), 1001);
    let mut rest = vec![0; usize::from(frame[1]) - 2];
    stream.read_exact(&mut rest).unwrap();
    // The listener keeps the connection open a moment for the client's answer.
    assert!(unanswered(&mut stream, Duration::from_millis(300)));
}

#[test]
fn room_members_hold_their_connections() {
    let page = TempDir::new().unwrap();
    fs::write(page.path().join("index.html"), "<p>game</p>").unwrap();
    let address = start(Options {
        page: Some(page.path().to_path_buf()),
        limits: Limits {
            max_connections: 2,
            ..Limits::default()
        },
        ..Options::default()
    });
    let (mut host, _) = join(address, "/rooms/ABCDEF?host=2");
    let (joiner, joiner_id) = join(address, "/rooms/ABCDEF");
    assert_eq!(event(&mut host), JsonPeerEvent::NewPeer(joiner_id));
    let mut waiting = TcpStream::connect(address).unwrap();
    write!(
        waiting,
        "GET / HTTP/1.1\r\nHost: {address}\r\nConnection: close\r\n\r\n"
    )
    .unwrap();
    assert!(unanswered(&mut waiting, Duration::from_millis(500)));
    // Once a member leaves, the waiting client is served.
    drop(joiner);
    let mut response = String::new();
    waiting.read_to_string(&mut response).unwrap();
    assert!(response.starts_with("HTTP/1.1 200"), "{response}");
}

/// Whether the listener closed `stream` without answering.
fn closed_at_once(stream: &mut TcpStream) -> bool {
    stream.set_read_timeout(Some(TIMEOUT)).unwrap();
    let mut byte = [0];
    match stream.read(&mut byte) {
        Ok(0) => true,
        Err(error) => !matches!(error.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut),
        Ok(_) => false,
    }
}

#[test]
fn an_address_holds_a_bounded_number_of_connections() {
    let page = TempDir::new().unwrap();
    fs::write(page.path().join("index.html"), "<p>game</p>").unwrap();
    let address = start(Options {
        page: Some(page.path().to_path_buf()),
        limits: Limits {
            connections_per_address: 3,
            header_timeout: Duration::from_secs(30),
            ..Limits::default()
        },
        ..Options::default()
    });
    // Two room members and an idle client hold every connection of this address, the
    // members for as long as they stay in their room.
    let (mut host, _) = join(address, "/rooms/ABCDEF?host=2");
    let (joiner, joiner_id) = join(address, "/rooms/ABCDEF");
    assert_eq!(event(&mut host), JsonPeerEvent::NewPeer(joiner_id));
    let idle = TcpStream::connect(address).unwrap();
    std::thread::sleep(Duration::from_millis(200));
    let mut refused = TcpStream::connect(address).unwrap();
    assert!(
        closed_at_once(&mut refused),
        "a fourth connection is closed"
    );
    // Once a member leaves, and once the idle client goes, the address may connect again.
    drop(joiner);
    assert_eq!(event(&mut host), JsonPeerEvent::PeerLeft(joiner_id));
    std::thread::sleep(Duration::from_millis(200));
    let (status, _, body) = http(address, "GET", "/");
    assert_eq!((status, body.as_str()), (200, "<p>game</p>"));
    drop(idle);
    std::thread::sleep(Duration::from_millis(200));
    let _again = join(address, "/rooms/ABCDEF");
}
