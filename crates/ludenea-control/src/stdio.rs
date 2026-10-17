//! Serves a host over line-delimited JSON on stdin and stdout: the transport of a game
//! binary run by the desktop editor. Lines starting with `{"type":` are protocol; anything
//! else a game prints is console output for the reader.

use std::io::{self, BufRead, Write};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::time::{Duration, Instant};

use ludenea_app::App;

use crate::host::Host;
use crate::in_process::answer;
use crate::protocol::{Command, ErrorCode, ProtocolError, ServerMessage};

/// How long the loop waits for a message before it advances the host anyway.
const FRAME: Duration = Duration::from_nanos(1_000_000_000 / 60);

/// Reads one `ClientMessage` per line from `input`, executes it, and writes one
/// `ServerMessage` per line to `output`, advancing play in real time. Returns `Ok` at the
/// end of the input. The game runs on every core. Each batch of messages reaches `output` in
/// one write, so lines the game prints from other threads never land inside a message.
pub fn serve_stdio(
    app: fn() -> App,
    input: impl BufRead + Send + 'static,
    mut output: impl Write,
) -> io::Result<()> {
    let host = Host::new(app()).map_err(io::Error::other);
    let mut host = host?;
    host.handle(Command::SetThreads {
        threads: available_threads(),
    })
    .expect("SetThreads always succeeds");
    let (sender, receiver) = mpsc::channel();
    std::thread::spawn(move || {
        // Bytes, not text: a line that is not UTF-8 gets an error instead of ending input.
        for line in input.split(b'\n') {
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
        let mut batch = Vec::new();
        for line in &lines {
            let reply = match std::str::from_utf8(line) {
                Ok(line) if line.trim().is_empty() => continue,
                Ok(line) => answer(&mut host, &mut connected, line),
                Err(_) => ServerMessage::Refused {
                    error: ProtocolError::new(
                        ErrorCode::InvalidCommand,
                        "this line is not UTF-8 text; send each message as one line of JSON",
                    ),
                },
            };
            write_message(&mut batch, &reply)?;
        }
        let elapsed = now();
        let events = host.advance(elapsed.duration_since(last).as_secs_f64());
        last = elapsed;
        for event in events {
            write_message(&mut batch, &ServerMessage::Event { event })?;
        }
        output.write_all(&batch)?;
        output.flush()?;
        if ended {
            return Ok(());
        }
    }
}

/// Every core: results never depend on the thread count.
#[allow(clippy::disallowed_methods)]
fn available_threads() -> u32 {
    std::thread::available_parallelism().map_or(1, |threads| {
        u32::try_from(threads.get()).unwrap_or(u32::MAX)
    })
}

#[allow(clippy::disallowed_methods)]
fn now() -> Instant {
    Instant::now()
}

fn write_message(output: &mut impl Write, message: &ServerMessage) -> io::Result<()> {
    serde_json::to_writer(&mut *output, message)?;
    output.write_all(b"\n")
}
