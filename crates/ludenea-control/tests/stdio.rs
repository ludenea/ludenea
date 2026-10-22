use std::io::{BufReader, Cursor, Read};
use std::thread::sleep;
use std::time::Duration;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use ludenea::app::App;
use ludenea::app::assets::asset_file;
use ludenea::draw::frame::RenderFrame;
use ludenea::ecs::AssetId;
use ludenea_control::stdio::serve_stdio;
use ludenea_control::{
    ClientMessage, Command, Document, ErrorCode, Event, Hash64, Host, PROTOCOL_FINGERPRINT,
    PROTOCOL_VERSION, Reply, ServerMessage, Topic,
};

fn line(value: serde_json::Value) -> String {
    format!("{value}\n")
}

fn hello() -> String {
    line(serde_json::json!(ClientMessage::Hello {
        protocol_version: PROTOCOL_VERSION,
        protocol_fingerprint: PROTOCOL_FINGERPRINT,
    }))
}

fn request(id: u64, command: &Command) -> String {
    line(serde_json::json!({ "type": "request", "id": id, "command": command }))
}

fn serve(input: String) -> Vec<ServerMessage> {
    let mut output = Vec::new();
    serve_stdio(boxes::app, BufReader::new(Cursor::new(input)), &mut output).unwrap();
    String::from_utf8(output)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect()
}

/// The messages that answer a line: the events a request causes come before its response.
fn answers(messages: Vec<ServerMessage>) -> Vec<ServerMessage> {
    messages
        .into_iter()
        .filter(|message| !matches!(message, ServerMessage::Event { .. }))
        .collect()
}

#[test]
fn stdio_handshake_and_step_over_pipes() {
    let step = Command::Step {
        ticks: 2,
        record_hashes: true,
    };
    let messages = answers(serve(hello() + &request(7, &step)));
    assert!(matches!(messages[0], ServerMessage::Welcome { .. }));
    match &messages[1] {
        ServerMessage::Response {
            id: 7,
            result: Ok(Reply::Stepped { tick: 2, hashes }),
        } => assert_eq!(hashes.len(), 2),
        other => panic!("{other:?}"),
    }
}

#[test]
fn malformed_line_gets_error_not_panic() {
    let step = Command::Step {
        ticks: 1,
        record_hashes: false,
    };
    let input = "not json\n".to_string()
        + &hello()
        + "{\"type\":\"request\",\"id\":3,\"command\":42}\n"
        + "{\"type\":\"nonsense\"}\n"
        + &request(4, &step);
    let messages = answers(serve(input));
    assert!(matches!(
        &messages[0],
        ServerMessage::Refused { error } if error.code == ErrorCode::NotConnected
    ));
    assert!(matches!(messages[1], ServerMessage::Welcome { .. }));
    assert!(matches!(
        &messages[2],
        ServerMessage::Response { id: 3, result: Err(error) } if error.code == ErrorCode::InvalidCommand
    ));
    assert!(matches!(
        &messages[3],
        ServerMessage::Refused { error } if error.code == ErrorCode::InvalidCommand
    ));
    assert!(matches!(
        messages[4],
        ServerMessage::Response {
            id: 4,
            result: Ok(_)
        }
    ));
}

#[test]
fn a_hello_with_another_fingerprint_is_refused() {
    let other = Hash64(PROTOCOL_FINGERPRINT.0 ^ 1);
    let stale = line(serde_json::json!(ClientMessage::Hello {
        protocol_version: PROTOCOL_VERSION,
        protocol_fingerprint: other,
    }));
    let messages = serve(stale + &hello());
    let ServerMessage::Refused { error } = &messages[0] else {
        panic!("{:?}", messages[0]);
    };
    assert_eq!(error.code, ErrorCode::VersionMismatch);
    assert_eq!(
        error.message,
        format!(
            "client speaks another revision of protocol {PROTOCOL_VERSION} (fingerprint \
             {:016x}, engine {:016x}); rebuild the editor (pnpm -C editor build) and the game \
             from the same checkout",
            other.0, PROTOCOL_FINGERPRINT.0
        )
    );
    assert!(matches!(messages[1], ServerMessage::Welcome { .. }));
}

#[test]
fn a_hello_of_an_earlier_version_is_refused_as_such() {
    // Version 3 had no fingerprint, so its hello is not a message of this version.
    let messages = serve(line(
        serde_json::json!({ "type": "hello", "protocol_version": 3 }),
    ));
    let ServerMessage::Refused { error } = &messages[0] else {
        panic!("{:?}", messages[0]);
    };
    assert_eq!(error.code, ErrorCode::VersionMismatch);
    assert_eq!(
        error.message,
        format!(
            "client speaks protocol 3, engine speaks {PROTOCOL_VERSION}; rebuild the editor \
             (pnpm -C editor build) and the game from the same checkout"
        )
    );
}

#[test]
fn a_hello_that_does_not_decode_says_what_it_needs() {
    let fingerprint = serde_json::json!(PROTOCOL_FINGERPRINT);
    let unreadable = [
        serde_json::json!({ "type": "hello", "protocol_version": PROTOCOL_VERSION }),
        serde_json::json!({ "type": "hello", "protocol_version": PROTOCOL_VERSION, "protocol_fingerprint": 42 }),
        serde_json::json!({ "type": "hello", "protocol_fingerprint": fingerprint }),
    ];
    for hello in unreadable {
        let messages = serve(line(hello.clone()));
        let ServerMessage::Refused { error } = &messages[0] else {
            panic!("{hello}: {:?}", messages[0]);
        };
        assert_eq!(error.code, ErrorCode::InvalidCommand, "{hello}");
        let reason = serde_json::from_str::<ClientMessage>(&hello.to_string()).unwrap_err();
        assert_eq!(
            error.message,
            format!(
                "cannot read this hello: {reason}; a hello needs protocol_version and \
                 protocol_fingerprint"
            ),
            "{hello}"
        );
    }
}

/// Delivers its chunks one after another, waiting before each, then ends. A chunk larger
/// than the reader's buffer arrives over several reads.
struct Script(std::vec::IntoIter<(Duration, String)>, Cursor<Vec<u8>>);

impl Script {
    fn new(chunks: Vec<(Duration, String)>) -> Script {
        Script(chunks.into_iter(), Cursor::new(Vec::new()))
    }
}

impl Read for Script {
    fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        if self.1.position() == self.1.get_ref().len() as u64 {
            let Some((wait, text)) = self.0.next() else {
                return Ok(0);
            };
            sleep(wait);
            self.1 = Cursor::new(text.into_bytes());
        }
        self.1.read(buffer)
    }
}

#[test]
fn playing_emits_ticked_events_in_real_time() {
    let script = vec![
        (Duration::ZERO, hello() + &request(1, &Command::Play)),
        (Duration::from_millis(500), String::new()),
    ];
    // An empty chunk reads as the end of input, which ends the loop after the wait.
    let mut output = Vec::new();
    serve_stdio(boxes::app, BufReader::new(Script::new(script)), &mut output).unwrap();
    let ticks = String::from_utf8(output)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str::<ServerMessage>(line).unwrap())
        .filter(|message| {
            matches!(
                message,
                ServerMessage::Event {
                    event: Event::Ticked { .. }
                }
            )
        })
        .count();
    // 30 ticks are due; a loaded machine may run fewer, but a few prove real-time play.
    assert!((3..=60).contains(&ticks), "{ticks} ticks in 0.5 s");
}

#[test]
fn a_line_that_is_not_utf8_gets_an_error_and_the_session_goes_on() {
    let step = Command::Step {
        ticks: 1,
        record_hashes: false,
    };
    let mut input = hello().into_bytes();
    input.extend_from_slice(b"\xff\xfe\n");
    input.extend_from_slice(request(5, &step).as_bytes());
    let mut output = Vec::new();
    serve_stdio(boxes::app, BufReader::new(Cursor::new(input)), &mut output).unwrap();
    let messages = answers(
        String::from_utf8(output)
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect(),
    );
    assert!(matches!(
        &messages[1],
        ServerMessage::Refused { error } if error.code == ErrorCode::InvalidCommand
    ));
    assert!(matches!(
        messages[2],
        ServerMessage::Response {
            id: 5,
            result: Ok(_)
        }
    ));
}

#[test]
fn a_paint_tiles_without_its_tile_is_refused_naming_the_field() {
    let entity = serde_json::json!({ "index": 0, "generation": 0 });
    let input = hello()
        + &line(serde_json::json!({
            "type": "request",
            "id": 5,
            "command": {
                "type": "paint_tiles",
                "entity": entity,
                "area": { "x": 0, "y": 0, "width": 1, "height": 1 },
            },
        }));
    let messages = answers(serve(input));
    match &messages[1] {
        ServerMessage::Response {
            id: 5,
            result: Err(error),
        } => {
            assert_eq!(error.code, ErrorCode::InvalidCommand);
            assert!(
                error.message.contains("missing field `tile`"),
                "{}",
                error.message
            );
        }
        other => panic!("{other:?}"),
    }
}

/// What `app` prints while it plays `scene` subscribed to the render frames, for `wait`.
fn played(
    app: fn() -> App,
    scene: String,
    assets: Vec<(AssetId, String)>,
    wait: Duration,
) -> String {
    let load = Command::LoadScene {
        scene,
        assets,
        document: Document::Scene,
    };
    let subscribe = Command::Subscribe {
        topic: Topic::RenderFrames,
    };
    let script = vec![
        (
            Duration::ZERO,
            hello() + &request(1, &load) + &request(2, &subscribe) + &request(3, &Command::Play),
        ),
        (wait, String::new()),
    ];
    let mut output = Vec::new();
    serve_stdio(app, BufReader::new(Script::new(script)), &mut output).unwrap();
    String::from_utf8(output).unwrap()
}

const RENDER_FRAMES: &str = r#"{"type":"event","event":{"type":"render_frames","#;

/// Each `render_frames` line of `output`, with its frames decoded from base64 postcard.
fn render_frames(output: &str) -> Vec<(&str, Vec<RenderFrame>)> {
    output
        .lines()
        .filter(|line| line.starts_with(RENDER_FRAMES))
        .map(|line| {
            let message: serde_json::Value = serde_json::from_str(line).unwrap();
            let frames = message["event"]["frames"]
                .as_array()
                .unwrap()
                .iter()
                .map(|frame| {
                    let bytes = STANDARD.decode(frame.as_str().unwrap()).unwrap();
                    postcard::from_bytes(&bytes).unwrap()
                })
                .collect();
            (line, frames)
        })
        .collect()
}

#[test]
fn render_frames_are_base64_postcard_on_one_line() {
    let scene = boxes::generate_scene(30, 7);
    let output = played(
        boxes::app,
        scene.clone(),
        Vec::new(),
        Duration::from_millis(300),
    );
    let pushed = render_frames(&output);
    assert!(!pushed.is_empty(), "{output}");

    // The frames a host of the same scene draws at the same ticks.
    let mut host = Host::new(boxes::app).unwrap();
    let load = Command::LoadScene {
        scene,
        assets: Vec::new(),
        document: Document::Scene,
    };
    host.handle(load).unwrap();
    let last = pushed.last().unwrap().1.last().unwrap().tick;
    host.handle(Command::Step {
        ticks: u32::try_from(last).unwrap(),
        record_hashes: false,
    })
    .unwrap();
    for (line, frames) in &pushed {
        assert!((1..=2).contains(&frames.len()), "{line}");
        let ServerMessage::Event {
            event: Event::RenderFrames { frames: read, .. },
        } = serde_json::from_str(line).unwrap()
        else {
            panic!("{line}")
        };
        assert_eq!(&read, frames, "the message reads back as its frames");
        for frame in frames {
            host.handle(Command::Seek { tick: frame.tick }).unwrap();
            let Reply::RenderFrame { frame: expected } = host
                .handle(Command::GetRenderFrame {
                    debug_layers: Vec::new(),
                })
                .unwrap()
            else {
                panic!()
            };
            assert_eq!(frame, &expected, "tick {}", frame.tick);
        }
    }
}

#[test]
fn a_line_of_ten_thousand_sprites_is_under_a_quarter_of_its_json() {
    let sheet = AssetId(u128::from_str_radix(swarm::BUG_SHEET, 16).unwrap());
    let assets = vec![(sheet, asset_file("ludenea::SpriteSheet", SWARM_SHEET))];
    let output = played(
        swarm::app,
        SWARM_SCENE.into(),
        assets,
        Duration::from_millis(500),
    );
    let (line, frames) = render_frames(&output)
        .into_iter()
        .next()
        .expect("frames were pushed");
    let sprites: usize = frames.iter().map(|frame| frame.sprites.len()).sum();
    assert!(sprites >= 10_000 * frames.len(), "{sprites} sprites");
    let json = serde_json::to_string(&frames).unwrap();
    assert!(
        line.len() * 4 < json.len(),
        "{} bytes on the line, {} bytes of JSON",
        line.len(),
        json.len()
    );
}

const SWARM_SCENE: &str = include_str!("../../../examples/swarm/assets/swarm.scene.ron");
const SWARM_SHEET: &str = include_str!("../../../examples/swarm/assets/bug.sheet.ron");
