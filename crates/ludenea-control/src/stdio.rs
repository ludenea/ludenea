//! Serves a host over line-delimited JSON on stdin and stdout: the transport of a game
//! binary run by the desktop editor. Lines starting with `{"type":` are protocol; anything
//! else a game prints is console output for the reader.

use std::io::{self, BufRead, Write};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::time::{Duration, Instant};

use ludenea_app::App;

use crate::host::Host;
use crate::in_process::handshake;
use crate::protocol::{ClientMessage, ErrorCode, ProtocolError, ServerMessage};

/// How long the loop waits for a message before it advances the host anyway.
const FRAME: Duration = Duration::from_nanos(1_000_000_000 / 60);

/// Reads one `ClientMessage` per line from `input`, executes it, and writes one
/// `ServerMessage` per line to `output`, advancing play in real time. Returns `Ok` at the
/// end of the input.
pub fn serve_stdio(
    app: fn() -> App,
    input: impl BufRead + Send + 'static,
    mut output: impl Write,
) -> io::Result<()> {
    let host = Host::new(app()).map_err(io::Error::other);
    let mut host = host?;
    let (sender, receiver) = mpsc::channel();
    std::thread::spawn(move || {
        for line in input.lines() {
            let Ok(line) = line else { break };
            if sender.send(line).is_err() {
                break;
            }
        }
    });
    let mut connected = false;
    let mut last = now();
    loop {
        let mut lines = Vec::new();
        let mut ended = false;
        match receiver.recv_timeout(FRAME) {
            Ok(line) => lines.push(line),
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => ended = true,
        }
        lines.extend(receiver.try_iter());
        for line in lines.iter().filter(|line| !line.trim().is_empty()) {
            let reply = answer(&mut host, &mut connected, line);
            write_message(&mut output, &reply)?;
        }
        let elapsed = now();
        let events = host.advance(elapsed.duration_since(last).as_secs_f64());
        last = elapsed;
        for event in events {
            write_message(&mut output, &ServerMessage::Event { event })?;
        }
        output.flush()?;
        if ended {
            return Ok(());
        }
    }
}

#[allow(clippy::disallowed_methods)]
fn now() -> Instant {
    Instant::now()
}

fn answer(host: &mut Host, connected: &mut bool, line: &str) -> ServerMessage {
    match serde_json::from_str::<ClientMessage>(line) {
        Ok(ClientMessage::Request { id, command }) if *connected => ServerMessage::Response {
            id,
            result: host.handle(command),
        },
        Ok(message) => {
            let reply = handshake(host, message);
            *connected = matches!(reply, ServerMessage::Welcome { .. });
            reply
        }
        Err(error) => malformed(*connected, line, &error),
    }
}

fn malformed(connected: bool, line: &str, error: &serde_json::Error) -> ServerMessage {
    let id = serde_json::from_str::<serde_json::Value>(line)
        .ok()
        .and_then(|value| value.get("id")?.as_u64());
    match (connected, id) {
        (false, _) => ServerMessage::Refused {
            error: ProtocolError::new(ErrorCode::NotConnected, "send Hello before any request"),
        },
        (true, Some(id)) => ServerMessage::Response {
            id,
            result: Err(ProtocolError::new(
                ErrorCode::InvalidCommand,
                format!("cannot read this request: {error}"),
            )),
        },
        (true, None) => ServerMessage::Refused {
            error: ProtocolError::new(
                ErrorCode::InvalidCommand,
                format!("cannot read this message: {error}"),
            ),
        },
    }
}

fn write_message(output: &mut impl Write, message: &ServerMessage) -> io::Result<()> {
    serde_json::to_writer(&mut *output, message)?;
    output.write_all(b"\n")
}
