use std::borrow::Cow;
use std::io::{self, Write};
use std::path::Path;
use std::process::{Child, ChildStdin, Command, ExitStatus, Stdio};
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;

use ludenea_control::ServerMessage;
use serde::{Deserialize, Serialize};

use crate::{ProjectError, io_error, output_lines};

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export_to = "project/"))]
pub enum OutputStream {
    Stdout,
    Stderr,
}

/// A line the game wrote that is not a protocol message.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export_to = "project/"))]
pub struct GameOutput {
    pub stream: OutputStream,
    pub line: String,
}

/// A running game: `<binary> host --stdio`, speaking the control protocol line by line.
pub struct GameProcess {
    child: Child,
    stdin: Option<ChildStdin>,
    messages: Option<Receiver<String>>,
    output: Option<Receiver<GameOutput>>,
}

impl GameProcess {
    pub fn start(binary: &Path) -> Result<GameProcess, ProjectError> {
        let mut child = Command::new(binary)
            .args(["host", "--stdio"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|source| io_error(binary, source))?;
        let stdin = child.stdin.take().expect("stdin is piped");
        let stdout = child.stdout.take().expect("stdout is piped");
        let stderr = child.stderr.take().expect("stderr is piped");
        let (message_sender, messages) = mpsc::channel();
        let (output_sender, output) = mpsc::channel();
        let stdout_output = output_sender.clone();
        thread::spawn(move || {
            for line in output_lines(stdout) {
                let (printed, message) = split_stdout_line(&line);
                if let Some(printed) = printed {
                    forward(&stdout_output, OutputStream::Stdout, printed.into_owned());
                }
                if let Some(message) = message {
                    let _ = message_sender.send(message.to_string());
                }
            }
        });
        thread::spawn(move || {
            for line in output_lines(stderr) {
                let line = String::from_utf8_lossy(&line).into_owned();
                forward(&output_sender, OutputStream::Stderr, line);
            }
        });
        Ok(GameProcess {
            child,
            stdin: Some(stdin),
            messages: Some(messages),
            output: Some(output),
        })
    }

    /// Sends one protocol message, without its trailing newline.
    pub fn send(&mut self, line: &str) -> io::Result<()> {
        let stdin = self
            .stdin
            .as_mut()
            .ok_or_else(|| io::Error::from(io::ErrorKind::BrokenPipe))?;
        writeln!(stdin, "{line}")?;
        stdin.flush()
    }

    /// Closes the game's input, which makes the host exit. Later sends fail.
    pub fn close_input(&mut self) {
        self.stdin = None;
    }

    /// The protocol messages the game prints, once: `None` when already taken. The receiver
    /// disconnects when the game closes its output, which it does when it exits.
    pub fn take_messages(&mut self) -> Option<Receiver<String>> {
        self.messages.take()
    }

    /// Everything else the game prints, on both streams, once: `None` when already taken.
    pub fn take_output(&mut self) -> Option<Receiver<GameOutput>> {
        self.output.take()
    }

    /// How the game ended (`code()` is `None` when a signal ended it), or `None` while it runs.
    /// Never blocks. The message receiver disconnecting comes first: the status can follow a
    /// moment later, so poll for a while.
    pub fn wait(&mut self) -> Option<ExitStatus> {
        self.child.try_wait().ok().flatten()
    }

    pub fn stop(self) {}
}

impl Drop for GameProcess {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn forward(sender: &Sender<GameOutput>, stream: OutputStream, line: String) {
    let _ = sender.send(GameOutput { stream, line });
}

/// Splits a stdout line into what the game printed and a protocol message. A line starting
/// with `{"type":` is a protocol message: the host writes nothing else that way, and a game
/// printing such a line itself confuses the editor. Text the game printed without a trailing
/// newline ends up in front of the next protocol line; it is split off only where the rest of
/// the line is a whole protocol message, so a game printing `{"type":` mid-line keeps its line.
/// The host writes JSON, so a message is UTF-8: bytes that are not UTF-8 are printed text,
/// shown with replacement characters, and a message is looked for only after the last of them.
fn split_stdout_line(line: &[u8]) -> (Option<Cow<'_, str>>, Option<&str>) {
    const START: &str = r#"{"type":"#;
    // What follows the last bytes that are not UTF-8: the only place a message can be.
    let tail = match line.utf8_chunks().last() {
        Some(chunk) if chunk.invalid().is_empty() => chunk.valid(),
        _ => "",
    };
    if tail.len() == line.len() && tail.starts_with(START) {
        return (None, Some(tail));
    }
    let message = tail
        .match_indices(START)
        .map(|(at, _)| at)
        .find(|&at| serde_json::from_str::<ServerMessage>(&tail[at..]).is_ok());
    match message {
        Some(at) => {
            let printed = &line[..line.len() - tail.len() + at];
            (Some(String::from_utf8_lossy(printed)), Some(&tail[at..]))
        }
        None => (Some(String::from_utf8_lossy(line)), None),
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// A stand-in game binary running the shell `body`.
    #[cfg(unix)]
    pub(crate) fn script(dir: &Path, name: &str, body: &str) -> std::path::PathBuf {
        use std::os::unix::fs::PermissionsExt;

        let path = dir.join(name);
        std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        path
    }

    /// Bytes that are not UTF-8 are output with replacement characters, never a protocol
    /// message, and reading goes on, on both streams.
    #[cfg(unix)]
    #[test]
    fn a_line_that_is_not_utf8_is_output_and_protocol_lines_still_arrive() {
        use std::time::Duration;

        const TIMEOUT: Duration = Duration::from_secs(10);
        let dir = tempfile::TempDir::new().unwrap();
        let message = r#"{"type":"event","event":{"type":"resimulated","up_to":3}}"#;
        for (stream, redirect) in [(OutputStream::Stdout, ""), (OutputStream::Stderr, " >&2")] {
            let body = format!(
                "printf 'caf\\351\\n'{redirect}\n\
                 printf '{{\"type\":\"pong\",\"note\":\"\\377\"}}\\n'{redirect}\n\
                 printf 'then\\r\\n'{redirect}\n\
                 echo '{message}'\n\
                 read -r line"
            );
            let binary = script(dir.path(), &format!("{stream:?}"), &body);
            let mut game = GameProcess::start(&binary).unwrap();
            let messages = game.take_messages().unwrap();
            let output = game.take_output().unwrap();
            assert_eq!(messages.recv_timeout(TIMEOUT).as_deref(), Ok(message));
            let printed: Vec<_> = (0..3)
                .map(|_| {
                    output
                        .recv_timeout(TIMEOUT)
                        .expect("the game's output arrives")
                })
                .collect();
            let lines = [
                "caf\u{FFFD}",
                "{\"type\":\"pong\",\"note\":\"\u{FFFD}\"}",
                "then",
            ];
            let expected = lines.map(|line| GameOutput {
                stream,
                line: line.to_string(),
            });
            assert_eq!(printed, expected, "{stream:?}");
            assert_eq!(game.wait(), None, "the game still runs");
            assert_eq!(
                output.try_recv(),
                Err(mpsc::TryRecvError::Empty),
                "{stream:?}: more output arrived"
            );
            assert_eq!(
                messages.try_recv(),
                Err(mpsc::TryRecvError::Empty),
                "{stream:?}: messages still arrive"
            );
        }
    }

    #[test]
    fn printed_text_without_a_newline_is_split_from_the_next_message() {
        let message = r#"{"type":"event","event":{"type":"resimulated","up_to":3}}"#;
        assert_eq!(split_stdout_line(message.as_bytes()), (None, Some(message)));
        assert_eq!(
            split_stdout_line(format!("loading...{message}").as_bytes()),
            (Some("loading...".into()), Some(message))
        );
        assert_eq!(
            split_stdout_line(format!(r#"saw {{"type":"jump"}} then {message}"#).as_bytes()),
            (Some(r#"saw {"type":"jump"} then "#.into()), Some(message))
        );
        assert_eq!(split_stdout_line(b"hello"), (Some("hello".into()), None));
        let mut line = b"caf\xe9".to_vec();
        line.extend_from_slice(message.as_bytes());
        assert_eq!(
            split_stdout_line(&line),
            (Some("caf\u{FFFD}".into()), Some(message))
        );
    }

    #[test]
    fn printed_json_mid_line_stays_the_games_output() {
        let line = r#"state: {"type":"jump"}"#;
        assert_eq!(
            split_stdout_line(line.as_bytes()),
            (Some(line.into()), None)
        );
    }

    #[test]
    fn a_line_that_is_not_utf8_is_never_a_message() {
        let cases: [(&[u8], &str); 3] = [
            (
                b"{\"type\":\"pong\",\"note\":\"\xff\"}",
                "{\"type\":\"pong\",\"note\":\"\u{FFFD}\"}",
            ),
            (b"\xff{\"type\":\"jump\"}", "\u{FFFD}{\"type\":\"jump\"}"),
            (b"{\"type\":\xff", "{\"type\":\u{FFFD}"),
        ];
        for (line, printed) in cases {
            assert_eq!(split_stdout_line(line), (Some(printed.into()), None));
        }
    }
}
