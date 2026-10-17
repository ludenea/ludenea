use std::io::{self, BufRead, BufReader, Write};
use std::path::Path;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;

use serde::{Deserialize, Serialize};

use crate::{ProjectError, io_error};

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
            for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                let (printed, message) = split_stdout_line(&line);
                if let Some(printed) = printed {
                    forward(&stdout_output, OutputStream::Stdout, printed.to_string());
                }
                if let Some(message) = message {
                    let _ = message_sender.send(message.to_string());
                }
            }
        });
        thread::spawn(move || {
            for line in BufReader::new(stderr).lines().map_while(Result::ok) {
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

    /// The protocol messages the game prints. The receiver disconnects when the game exits.
    /// A second call returns a receiver that is already disconnected.
    pub fn take_messages(&mut self) -> Receiver<String> {
        self.messages.take().unwrap_or_else(|| mpsc::channel().1)
    }

    /// Everything else the game prints, on both streams.
    pub fn take_output(&mut self) -> Receiver<GameOutput> {
        self.output.take().unwrap_or_else(|| mpsc::channel().1)
    }

    /// The exit code once the game has exited; `None` while it runs (and when a signal ended
    /// it). Never blocks: detect the exit by the message receiver disconnecting, then call this.
    pub fn wait(&mut self) -> Option<i32> {
        self.child
            .try_wait()
            .ok()
            .flatten()
            .and_then(|status| status.code())
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

/// Splits a stdout line into what the game printed and a protocol message, which starts
/// with `{"type":`. Text the game printed without a trailing newline ends up in front of
/// the next protocol line on the same line; it is split off as the game's own output.
fn split_stdout_line(line: &str) -> (Option<&str>, Option<&str>) {
    match line.find(r#"{"type":"#) {
        Some(0) => (None, Some(line)),
        Some(at) => (Some(&line[..at]), Some(&line[at..])),
        None => (Some(line), None),
    }
}

#[cfg(test)]
mod tests {
    use super::split_stdout_line;

    #[test]
    fn printed_text_without_a_newline_is_split_from_the_next_message() {
        let message = r#"{"type":"event"}"#;
        assert_eq!(split_stdout_line(message), (None, Some(message)));
        assert_eq!(
            split_stdout_line(&format!("loading...{message}")),
            (Some("loading..."), Some(message))
        );
        assert_eq!(split_stdout_line("hello"), (Some("hello"), None));
    }
}
