use std::io::{self, BufRead, BufReader, Write};
use std::path::Path;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;

use serde::{Deserialize, Serialize};

use crate::{ProjectError, io_error};

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub enum OutputStream {
    Stdout,
    Stderr,
}

/// A line the game wrote that is not a protocol message.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct GameOutput {
    pub stream: OutputStream,
    pub line: String,
}

/// A running game: `<binary> host --stdio`, speaking the control protocol line by line.
pub struct GameProcess {
    child: Child,
    stdin: ChildStdin,
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
                if line.starts_with(r#"{"type":"#) {
                    let _ = message_sender.send(line);
                } else {
                    forward(&stdout_output, OutputStream::Stdout, line);
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
            stdin,
            messages: Some(messages),
            output: Some(output),
        })
    }

    /// Sends one protocol message, without its trailing newline.
    pub fn send(&mut self, line: &str) -> io::Result<()> {
        writeln!(self.stdin, "{line}")?;
        self.stdin.flush()
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

    /// Waits for the game to exit and returns its exit code (`None` when a signal ended it).
    pub fn wait(&mut self) -> Option<i32> {
        self.child.wait().ok().and_then(|status| status.code())
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
