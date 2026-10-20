use std::io::{BufReader, Cursor, Read};
use std::thread::sleep;
use std::time::Duration;

use ludenea_control::stdio::serve_stdio;
use ludenea_control::{
    ClientMessage, Command, ErrorCode, Event, Hash64, PROTOCOL_FINGERPRINT, PROTOCOL_VERSION,
    Reply, ServerMessage,
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

#[test]
fn stdio_handshake_and_step_over_pipes() {
    let step = Command::Step {
        ticks: 2,
        record_hashes: true,
    };
    let messages = serve(hello() + &request(7, &step));
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
    let messages = serve(input);
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

/// Delivers its chunks one after another, waiting before each, then ends.
struct Script(std::vec::IntoIter<(Duration, String)>);

impl Read for Script {
    fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        let Some((wait, text)) = self.0.next() else {
            return Ok(0);
        };
        sleep(wait);
        buffer[..text.len()].copy_from_slice(text.as_bytes());
        Ok(text.len())
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
    serve_stdio(
        boxes::app,
        BufReader::new(Script(script.into_iter())),
        &mut output,
    )
    .unwrap();
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
    let messages: Vec<ServerMessage> = String::from_utf8(output)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
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
