//! Runs `ludenea serve` on a free port and talks to it like the browser editor does.

use std::fs;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::time::Duration;

use ludenea_project::protocol::{
    FailureCode, PROJECT_PROTOCOL_FINGERPRINT, PROJECT_PROTOCOL_VERSION, ProjectClientMessage,
    ProjectEvent, ProjectFailure, ProjectReply, ProjectRequest, ProjectServerMessage,
};
use ludenea_project::{AssetKind, FileChange};
use tempfile::TempDir;
use tungstenite::client::IntoClientRequest;
use tungstenite::protocol::frame::coding::{CloseCode, Data, OpCode};
use tungstenite::protocol::frame::{Frame, FrameHeader};
use tungstenite::stream::MaybeTlsStream;
use tungstenite::{Message, WebSocket};

const TIMEOUT: Duration = Duration::from_secs(10);

/// The largest message the project connection takes, as the README documents it.
const MESSAGE_LIMIT: usize = 256 << 20;

type Socket = WebSocket<MaybeTlsStream<TcpStream>>;

struct Server {
    child: Child,
    address: String,
    root: PathBuf,
    /// The lines the server writes to its standard error.
    log: Receiver<String>,
    _dir: TempDir,
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn serve() -> Server {
    serve_with(&[])
}

/// A minimal project named `demo`, served on a port the system picks, with more options.
fn serve_with(options: &[&str]) -> Server {
    let dir = TempDir::new().unwrap();
    let root = dir.path().join("demo");
    let files = [
        ("ludenea.toml", "name = \"demo\"\nengine = \"0.1.0\"\n"),
        (
            "Cargo.toml",
            "[package]\nname = \"demo\"\nversion = \"0.1.0\"\nedition = \"2024\"\n\n[workspace]\n",
        ),
        ("src/lib.rs", ""),
        ("scenes/main.scene.ron", "(entities: [])\n"),
    ];
    for (path, text) in files {
        let file = root.join(path);
        fs::create_dir_all(file.parent().unwrap()).unwrap();
        fs::write(file, text).unwrap();
    }
    let mut child = Command::new(env!("CARGO_BIN_EXE_ludenea"))
        .args(["serve", root.to_str().unwrap(), "--port", "0"])
        .args(options)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let stderr = BufReader::new(child.stderr.take().unwrap());
    let (lines, log) = mpsc::channel();
    std::thread::spawn(move || {
        for line in stderr.lines().map_while(Result::ok) {
            if lines.send(line).is_err() {
                break;
            }
        }
    });
    let mut line = String::new();
    BufReader::new(child.stdout.take().unwrap())
        .read_line(&mut line)
        .unwrap();
    let address = line
        .trim()
        .strip_prefix("listening on http://")
        .unwrap_or_else(|| panic!("unexpected first line {line:?}"))
        .to_string();
    Server {
        child,
        address,
        root: root.canonicalize().unwrap(),
        log,
        _dir: dir,
    }
}

/// The next line the server logs that contains `text`.
fn logged(server: &Server, text: &str) -> String {
    loop {
        let line = server
            .log
            .recv_timeout(TIMEOUT)
            .expect("the server logs it in time");
        if line.contains(text) {
            return line;
        }
    }
}

fn connect(server: &Server, origin: Option<&str>) -> Result<Socket, tungstenite::Error> {
    connect_as(server, &server.address, origin)
}

/// Connects to the server naming `host` in the `Host` header, as a page reaching it by that
/// name would.
fn connect_as(
    server: &Server,
    host: &str,
    origin: Option<&str>,
) -> Result<Socket, tungstenite::Error> {
    let mut request = format!("ws://{}/ws/project", server.address)
        .into_client_request()
        .unwrap();
    request.headers_mut().insert("Host", host.parse().unwrap());
    if let Some(origin) = origin {
        request
            .headers_mut()
            .insert("Origin", origin.parse().unwrap());
    }
    let (socket, _) = tungstenite::connect(request)?;
    if let MaybeTlsStream::Plain(stream) = socket.get_ref() {
        stream.set_read_timeout(Some(TIMEOUT)).unwrap();
    }
    Ok(socket)
}

fn send(socket: &mut Socket, message: &ProjectClientMessage) {
    let text = serde_json::to_string(message).unwrap();
    socket.send(Message::text(text)).unwrap();
}

fn receive(socket: &mut Socket) -> ProjectServerMessage {
    loop {
        if let Message::Text(text) = socket.read().expect("a message arrives in time") {
            return serde_json::from_str(&text).unwrap();
        }
    }
}

fn handshake(socket: &mut Socket) {
    send(
        socket,
        &ProjectClientMessage::Hello {
            protocol_version: PROJECT_PROTOCOL_VERSION,
            protocol_fingerprint: PROJECT_PROTOCOL_FINGERPRINT,
        },
    );
    assert_eq!(
        receive(socket),
        ProjectServerMessage::Welcome {
            protocol_version: PROJECT_PROTOCOL_VERSION,
            protocol_fingerprint: PROJECT_PROTOCOL_FINGERPRINT,
        }
    );
}

/// Sends a request and returns its reply, skipping the events that arrive first.
fn request(socket: &mut Socket, id: u64, request: ProjectRequest) -> ProjectReply {
    send(socket, &ProjectClientMessage::Request { id, request });
    loop {
        if let ProjectServerMessage::Response {
            id: answered,
            result,
        } = receive(socket)
        {
            assert_eq!(answered, id);
            return result.unwrap();
        }
    }
}

/// Status line, `Content-Type`, and body of `GET path`.
fn get(server: &Server, path: &str) -> (String, Option<String>, String) {
    get_as(server, &server.address, path)
}

fn get_as(server: &Server, host: &str, path: &str) -> (String, Option<String>, String) {
    let mut stream = TcpStream::connect(&server.address).unwrap();
    stream.set_read_timeout(Some(TIMEOUT)).unwrap();
    write!(
        stream,
        "GET {path} HTTP/1.1\r\nHost: {host}\r\nConnection: close\r\n\r\n"
    )
    .unwrap();
    let mut response = String::new();
    stream.read_to_string(&mut response).unwrap();
    let (head, body) = response.split_once("\r\n\r\n").unwrap();
    let mut lines = head.lines();
    let status = lines.next().unwrap().to_string();
    let content_type = lines
        .filter_map(|line| line.split_once(": "))
        .find(|(name, _)| name.eq_ignore_ascii_case("content-type"))
        .map(|(_, value)| value.to_string());
    (status, content_type, body.to_string())
}

fn write(root: &Path, path: &str, text: &str) {
    let file = root.join(path);
    fs::create_dir_all(file.parent().unwrap()).unwrap();
    fs::write(file, text).unwrap();
}

#[test]
fn ws_handshake_and_list_files() {
    let server = serve();
    let mut socket = connect(&server, None).unwrap();
    handshake(&mut socket);
    let ProjectReply::Files { entries } = request(&mut socket, 1, ProjectRequest::ListFiles) else {
        panic!("ListFiles answers Files");
    };
    let paths: Vec<_> = entries.iter().map(|entry| entry.path.as_str()).collect();
    assert_eq!(paths, ["scenes/main.scene.ron"]);
    let ProjectReply::Info(info) = request(&mut socket, 2, ProjectRequest::Info) else {
        panic!("Info answers Info");
    };
    assert_eq!(info.name, "demo");
}

#[test]
fn ws_from_another_origin_is_refused() {
    let server = serve();
    assert!(connect(&server, Some("http://elsewhere.example")).is_err());
    let own = format!("http://{}", server.address);
    let mut socket = connect(&server, Some(&own)).unwrap();
    handshake(&mut socket);
}

/// DNS rebinding: a foreign page whose name now resolves to this machine sends a matching
/// `Origin` and `Host`, both naming its own host.
#[test]
fn foreign_hosts_are_refused_even_with_a_matching_origin() {
    let server = serve();
    let port = server.address.rsplit(':').next().unwrap();
    let foreign = format!("evil.example:{port}");
    let origin = format!("http://{foreign}");
    assert!(connect_as(&server, &foreign, Some(&origin)).is_err());
    let (status, _, _) = get_as(&server, &foreign, "/plugins/missing.js");
    assert!(status.contains("421"), "{status}");
    for local in [format!("localhost:{port}"), format!("127.0.0.1:{port}")] {
        let (status, _, _) = get_as(&server, &local, "/plugins/missing.js");
        assert!(status.contains("404"), "{local}: {status}");
    }
}

#[test]
fn allow_host_lets_a_named_host_through() {
    let server = serve_with(&["--allow-host", "studio.example"]);
    let port = server.address.rsplit(':').next().unwrap();
    let named = format!("studio.example:{port}");
    let origin = format!("http://{named}");
    let mut socket = connect_as(&server, &named, Some(&origin)).unwrap();
    handshake(&mut socket);
    let (status, _, _) = get_as(&server, &format!("other.example:{port}"), "/");
    assert!(status.contains("421"), "{status}");
}

/// A development server on another port proxying the WebSocket, named by `--allow-origin`.
#[test]
fn allow_origin_lets_a_dev_proxy_connect() {
    let server = serve_with(&["--allow-origin", "http://localhost:5555"]);
    let mut socket = connect(&server, Some("http://localhost:5555")).unwrap();
    handshake(&mut socket);
    assert!(connect(&server, Some("http://localhost:5556")).is_err());
}

#[test]
fn plugins_and_game_files_are_served_with_js_and_wasm_content_types() {
    let server = serve();
    write(
        &server.root,
        ".ludenea/plugins/demo.0123456789abcdef.js",
        "export {};",
    );
    write(
        &server.root,
        ".ludenea/build/wasm/0123456789abcdef/game.js",
        "// glue",
    );
    write(
        &server.root,
        ".ludenea/build/wasm/0123456789abcdef/game_bg.wasm",
        "\0asm",
    );

    let (status, kind, body) = get(&server, "/plugins/demo.0123456789abcdef.js");
    assert!(status.contains("200"), "{status}");
    assert_eq!(kind.as_deref(), Some("text/javascript"));
    assert_eq!(body, "export {};");
    let (status, kind, _) = get(&server, "/game/0123456789abcdef/game.js");
    assert!(status.contains("200"), "{status}");
    assert_eq!(kind.as_deref(), Some("text/javascript"));
    let (status, kind, _) = get(&server, "/game/0123456789abcdef/game_bg.wasm");
    assert!(status.contains("200"), "{status}");
    assert_eq!(kind.as_deref(), Some("application/wasm"));

    for escape in [
        "/plugins/..%2F..%2Fludenea.toml",
        "/plugins/..",
        "/plugins/%2E%2E",
        "/plugins/..%5C..%5Cludenea.toml",
        "/game/..%2F..%2F..%2F/ludenea.toml",
        "/game/../ludenea.toml",
        "/game/0123456789abcdef/..%2F..%2F..%2F..%2Fludenea.toml",
        "/game/%2E%2E/plugins",
        "/plugins/missing.js",
    ] {
        let (status, _, body) = get(&server, escape);
        assert!(!status.contains("200"), "{escape}: {status}");
        assert!(!body.contains("engine"), "{escape}: {body}");
    }
}

#[test]
fn two_clients_both_receive_file_events() {
    let server = serve();
    let mut sockets = [
        connect(&server, None).unwrap(),
        connect(&server, None).unwrap(),
    ];
    for socket in &mut sockets {
        handshake(socket);
    }
    write(&server.root, "scenes/new.scene.ron", "(entities: [])\n");
    let created = FileChange::Created {
        path: "scenes/new.scene.ron".to_string(),
        kind: AssetKind::Scene,
    };
    for socket in &mut sockets {
        loop {
            if let ProjectServerMessage::Event {
                event: ProjectEvent::FilesChanged { changes },
            } = receive(socket)
                && changes.contains(&created)
            {
                break;
            }
        }
    }
}

/// Reads events until a build of the project finishes, returning whether it succeeded.
fn build_finished(socket: &mut Socket) -> bool {
    loop {
        if let ProjectServerMessage::Event {
            event:
                ProjectEvent::Build {
                    event: ludenea_project::BuildEvent::Finished { ok },
                },
        } = receive(socket)
        {
            return ok;
        }
    }
}

#[test]
fn an_editor_connecting_after_the_build_learns_how_it_ended() {
    let server = serve();
    // The first editor follows the build (or catches up on it).
    let mut first = connect(&server, None).unwrap();
    handshake(&mut first);
    let ok = build_finished(&mut first);
    let mut late = connect(&server, None).unwrap();
    handshake(&mut late);
    assert_eq!(build_finished(&mut late), ok);
}

/// The refusal the service sends before it closes the connection, skipping the events that
/// arrive first, and the close frame that follows it.
fn refusal_then_close(socket: &mut Socket) -> (ProjectFailure, CloseCode) {
    let error = loop {
        match receive(socket) {
            ProjectServerMessage::Refused { error } => break error,
            ProjectServerMessage::Event { .. } => {}
            other => panic!("expected a refusal, got {other:?}"),
        }
    };
    match socket.read() {
        Ok(Message::Close(Some(frame))) => (error, frame.code),
        other => panic!("expected a close frame after the refusal, got {other:?}"),
    }
}

/// A browser may send a message as one frame: a 20 MiB scene write goes through, and a frame
/// over the limit is refused, naming its size and the limit, as soon as its header arrives.
#[test]
fn a_large_write_is_accepted_and_an_oversized_one_is_refused_precisely() {
    let server = serve();
    let mut socket = connect(&server, None).unwrap();
    handshake(&mut socket);
    let mut text = "(entities: [])\n// ".to_string();
    text.push_str(&"x".repeat((20 << 20) - text.len()));
    let path = "scenes/large.scene.ron".to_string();
    let reply = request(&mut socket, 1, ProjectRequest::WriteText { path, text });
    assert_eq!(reply, ProjectReply::Done);
    let written = fs::metadata(server.root.join("scenes/large.scene.ron")).unwrap();
    assert_eq!(written.len(), 20 << 20);

    // Only the header: the service refuses the frame on its announced size.
    let oversized = MESSAGE_LIMIT + 1;
    let header = FrameHeader {
        is_final: true,
        opcode: OpCode::Data(Data::Text),
        mask: Some([0; 4]),
        ..FrameHeader::default()
    };
    let mut bytes = Vec::new();
    header.format(oversized as u64, &mut bytes).unwrap();
    let MaybeTlsStream::Plain(stream) = socket.get_mut() else {
        panic!("the test connects without TLS");
    };
    stream.write_all(&bytes).unwrap();

    let (error, close) = refusal_then_close(&mut socket);
    assert_eq!(error.code, FailureCode::InvalidRequest);
    for text in [&error.message, &logged(&server, "refused")] {
        assert!(
            text.contains(&format!("{oversized} bytes"))
                && text.contains(&format!("{MESSAGE_LIMIT} bytes")),
            "{text}"
        );
    }
    assert_eq!(close, CloseCode::Size);
}

/// Chromium sends a large message as many frames: once their total passes the limit, the
/// message is refused with the size received so far and the limit.
#[test]
fn a_message_over_the_limit_in_many_frames_is_refused_precisely() {
    let server = serve();
    let mut socket = connect(&server, None).unwrap();
    handshake(&mut socket);
    let fragment = tungstenite::Bytes::from(vec![b'x'; 16 << 20]);
    let fragments = MESSAGE_LIMIT / fragment.len() + 1;
    for index in 0..fragments {
        let opcode = OpCode::Data(if index == 0 {
            Data::Text
        } else {
            Data::Continue
        });
        let frame = Frame::message(fragment.clone(), opcode, false);
        socket.send(Message::Frame(frame)).unwrap();
    }

    let (error, close) = refusal_then_close(&mut socket);
    assert_eq!(error.code, FailureCode::InvalidRequest);
    let received = fragments * fragment.len();
    for text in [&error.message, &logged(&server, "refused")] {
        assert!(
            text.contains(&format!("{received} bytes"))
                && text.contains(&format!("{MESSAGE_LIMIT} bytes")),
            "{text}"
        );
    }
    assert_eq!(close, CloseCode::Size);
}
