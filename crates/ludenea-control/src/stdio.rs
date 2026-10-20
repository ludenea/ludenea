//! Serves a host over line-delimited JSON on stdin and stdout: the transport of a game
//! binary run by the desktop editor. Lines starting with `{"type":` are protocol; anything
//! else a game prints is console output for the reader.

use std::io::{self, BufRead, Write};
use std::num::NonZeroUsize;
use std::sync::mpsc::{self, RecvTimeoutError};
use std::time::{Duration, Instant};

use ludenea_app::{App, Parallel};

use crate::host::Host;
use crate::in_process::text::answer;
use crate::protocol::{Command, ConsoleLevel, ErrorCode, Event, ProtocolError, ServerMessage};

/// How long the loop waits for a message before it advances the host anyway.
const FRAME: Duration = Duration::from_nanos(1_000_000_000 / 60);

/// Reads one `ClientMessage` per line from `input`, executes it, and writes one
/// `ServerMessage` per line to `output`, advancing play in real time. Returns `Ok` at the
/// end of the input. The game runs on every core up to [`Parallel::MAX_THREADS`], or on one
/// thread, with a console warning after the welcome, when the system refuses them. Each batch
/// of messages reaches `output` in one write, so lines the game prints from other threads
/// never land inside a message.
pub fn serve_stdio(
    app: fn() -> App,
    input: impl BufRead + Send + 'static,
    output: impl Write,
) -> io::Result<()> {
    serve(app, available_threads(), input, output)
}

/// [`serve_stdio`] on `threads` threads.
fn serve(
    app: fn() -> App,
    threads: u32,
    input: impl BufRead + Send + 'static,
    mut output: impl Write,
) -> io::Result<()> {
    let host = Host::new(app).map_err(io::Error::other);
    let mut host = host?;
    // A refused count leaves the one thread the app is built with.
    let mut warning = host
        .handle(Command::SetThreads { threads })
        .err()
        .map(|error| format!("the game runs on one thread: {}", error.message));
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
            let replies = match std::str::from_utf8(line) {
                Ok(line) if line.trim().is_empty() => continue,
                Ok(line) => answer(&mut host, &mut connected, line),
                Err(_) => vec![ServerMessage::Refused {
                    error: ProtocolError::new(
                        ErrorCode::InvalidCommand,
                        "this line is not UTF-8 text; send each message as one line of JSON",
                    ),
                }],
            };
            for reply in &replies {
                write_message(&mut batch, reply)?;
            }
            // Right after the welcome: a client ignores what comes before it.
            if connected && let Some(text) = warning.take() {
                let event = Event::Console {
                    level: ConsoleLevel::Warning,
                    text,
                };
                write_message(&mut batch, &ServerMessage::Event { event })?;
            }
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

/// Every core, up to [`Parallel::MAX_THREADS`]: results never depend on the thread count.
#[expect(
    clippy::disallowed_methods,
    reason = "the thread count never changes results"
)]
fn available_threads() -> u32 {
    let cores = std::thread::available_parallelism().map_or(1, NonZeroUsize::get);
    u32::try_from(cores.min(Parallel::MAX_THREADS)).unwrap_or(1)
}

#[allow(clippy::disallowed_methods)]
fn now() -> Instant {
    Instant::now()
}

fn write_message(output: &mut impl Write, message: &ServerMessage) -> io::Result<()> {
    serde_json::to_writer(&mut *output, message)?;
    output.write_all(b"\n")
}

#[cfg(test)]
mod tests {
    use std::io::{BufReader, Cursor};

    use super::serve;
    use crate::protocol::{
        ClientMessage, Command, ConsoleLevel, Event, PROTOCOL_FINGERPRINT, PROTOCOL_VERSION, Reply,
        ServerMessage,
    };

    #[test]
    fn threads_that_cannot_start_leave_one_thread_and_a_warning() {
        let hello = ClientMessage::Hello {
            protocol_version: PROTOCOL_VERSION,
            protocol_fingerprint: PROTOCOL_FINGERPRINT,
        };
        let step = serde_json::json!({
            "type": "request",
            "id": 1,
            "command": Command::Step { ticks: 2, record_hashes: false },
        });
        let input = format!("{}\n{step}\n", serde_json::json!(hello));
        let mut output = Vec::new();
        serve(
            boxes::app,
            u32::MAX,
            BufReader::new(Cursor::new(input)),
            &mut output,
        )
        .unwrap();
        let messages: Vec<ServerMessage> = String::from_utf8(output)
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        assert!(matches!(messages[0], ServerMessage::Welcome { .. }));
        // Right after the welcome: a client ignores what comes before it.
        assert_eq!(
            messages[1],
            ServerMessage::Event {
                event: Event::Console {
                    level: ConsoleLevel::Warning,
                    text: "the game runs on one thread: cannot start 4294967295 threads: outside \
                           the supported range; use 1 to 256"
                        .into(),
                },
            }
        );
        // The step's events come before its response.
        assert!(messages[2..].iter().any(|message| matches!(
            message,
            ServerMessage::Response {
                id: 1,
                result: Ok(Reply::Stepped { tick: 2, .. })
            }
        )));
    }
}
