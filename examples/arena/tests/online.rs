//! Two `arena peer` processes play a match over real WebRTC, through a signaling listener of
//! this test process and without STUN: the peers reach each other on this machine's network
//! address (webrtc-rs leaves loopback out, so the machine needs another interface).

use std::io::{BufRead, BufReader};
use std::net::{SocketAddr, TcpListener as StdListener};
use std::path::Path;
use std::process::{Child, ChildStdout, Command, ExitStatus, Stdio};
use std::sync::mpsc::{Receiver, channel};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use ludenea_signal::{Options, serve};

const SCENE: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/assets/arena.scene.ron");
/// How long a process may take to print a line or to exit.
const TIMEOUT: Duration = Duration::from_secs(60);

#[expect(
    clippy::disallowed_methods,
    reason = "the tests bound their waits with wall-clock time"
)]
fn now() -> Instant {
    Instant::now()
}

/// A signaling listener on a port of `127.0.0.1` the system picks, in a runtime of its own
/// thread, stopped when dropped.
struct Listener {
    address: SocketAddr,
    stop: Option<tokio::sync::oneshot::Sender<()>>,
    thread: Option<JoinHandle<()>>,
}

impl Listener {
    fn start() -> Listener {
        let listener = StdListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        listener.set_nonblocking(true).unwrap();
        let (stop, stopped) = tokio::sync::oneshot::channel::<()>();
        let thread = std::thread::spawn(move || {
            let runtime = tokio::runtime::Runtime::new().unwrap();
            runtime.block_on(async move {
                let listener = tokio::net::TcpListener::from_std(listener).unwrap();
                tokio::select! {
                    result = serve(vec![listener], Options::default()) => result.unwrap(),
                    _ = stopped => {}
                }
            });
        });
        Listener {
            address,
            stop: Some(stop),
            thread: Some(thread),
        }
    }

    fn signaling(&self) -> String {
        format!("ws://{}/rooms", self.address)
    }
}

impl Drop for Listener {
    fn drop(&mut self) {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// An `arena` process and the lines it printed, read on a thread of their own.
struct Peer {
    child: Child,
    lines: Receiver<String>,
    seen: Vec<String>,
}

impl Peer {
    fn spawn(args: &[&str]) -> Peer {
        let mut child = Command::new(env!("CARGO_BIN_EXE_arena"))
            .args(args)
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .unwrap();
        let stdout: ChildStdout = child.stdout.take().unwrap();
        let (send, lines) = channel();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                if send.send(line).is_err() {
                    break;
                }
            }
        });
        Peer {
            child,
            lines,
            seen: Vec::new(),
        }
    }

    /// Waits for a line starting with `prefix`, and returns it.
    fn line(&mut self, prefix: &str) -> String {
        let deadline = now() + TIMEOUT;
        loop {
            let left = deadline.saturating_duration_since(now());
            let line = self.lines.recv_timeout(left).unwrap_or_else(|_| {
                let _ = self.child.kill();
                panic!("no line `{prefix}` in time; printed {:?}", self.seen)
            });
            self.seen.push(line.clone());
            if line.starts_with(prefix) {
                return line;
            }
        }
    }

    /// Waits for the process to exit; returns its status and every line it printed.
    fn wait(mut self) -> (ExitStatus, Vec<String>) {
        let deadline = now() + TIMEOUT;
        let status = loop {
            if let Some(status) = self.child.try_wait().unwrap() {
                break status;
            }
            if now() > deadline {
                let _ = self.child.kill();
                panic!("the peer did not exit in time; printed {:?}", self.seen);
            }
            std::thread::sleep(Duration::from_millis(20));
        };
        // The reader thread ends with the process's output.
        while let Ok(line) = self.lines.recv_timeout(Duration::from_secs(5)) {
            self.seen.push(line);
        }
        (status, self.seen)
    }
}

fn path(dir: &Path, name: &str) -> String {
    dir.join(name).display().to_string()
}

/// `arena verify FILE` exits 0.
fn verified(replay: &str) {
    let output = Command::new(env!("CARGO_BIN_EXE_arena"))
        .args(["verify", replay])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stdout)
    );
}

#[test]
fn two_arena_peers_play_and_verify() {
    let listener = Listener::start();
    let signaling = listener.signaling();
    let dir = tempfile::tempdir().unwrap();
    let (host_replay, join_replay) = (
        path(dir.path(), "host.replay"),
        path(dir.path(), "join.replay"),
    );
    let common = ["--signaling", &signaling, "--no-stun", "--ticks", "300"];
    let mut host = Peer::spawn(
        &[
            &["peer", SCENE, "--host", "--room", "23456A"][..],
            &common,
            &[
                "--random-inputs",
                "1",
                "--threads",
                "1",
                "--record",
                &host_replay,
            ],
        ]
        .concat(),
    );
    assert_eq!(host.line("room "), "room 23456A");
    let joiner = Peer::spawn(
        &[
            &["peer", SCENE, "--join", "23456A"][..],
            &common,
            &[
                "--random-inputs",
                "2",
                "--threads",
                "8",
                "--record",
                &join_replay,
            ],
        ]
        .concat(),
    );
    let (host_status, host_lines) = host.wait();
    let (join_status, join_lines) = joiner.wait();
    assert_eq!(host_status.code(), Some(0), "{host_lines:?}");
    assert_eq!(join_status.code(), Some(0), "{join_lines:?}");
    for (lines, peer) in [(&host_lines, 0), (&join_lines, 1)] {
        assert!(
            lines.contains(&format!(
                "match started: peer {peer} of 2, local players [{peer}]"
            )),
            "{lines:?}"
        );
        assert!(
            lines
                .iter()
                .any(|line| line.starts_with("match ended at tick ")),
            "{lines:?}"
        );
        assert!(
            lines
                .iter()
                .any(|line| line.starts_with("peer ") && line.contains("ping")),
            "{lines:?}"
        );
    }
    assert!(host_lines.contains(&format!("wrote {host_replay}")));
    let (host_bytes, join_bytes) = (
        std::fs::read(&host_replay).unwrap(),
        std::fs::read(&join_replay).unwrap(),
    );
    assert!(host_bytes == join_bytes, "the replays differ");
    verified(&host_replay);
}

#[test]
fn a_peer_that_disappears_ends_the_other_lost() {
    let listener = Listener::start();
    let signaling = listener.signaling();
    let dir = tempfile::tempdir().unwrap();
    let host_replay = path(dir.path(), "host.replay");
    let common = [
        "--signaling",
        &signaling,
        "--no-stun",
        "--random-inputs",
        "3",
    ];
    let mut host = Peer::spawn(
        &[
            &[
                "peer",
                SCENE,
                "--host",
                "--room",
                "23456B",
                "--record",
                &host_replay,
            ][..],
            &common,
        ]
        .concat(),
    );
    host.line("room ");
    let mut joiner = Peer::spawn(&[&["peer", SCENE, "--join", "23456B"][..], &common].concat());
    joiner.line("match started");
    host.line("match started");
    std::thread::sleep(Duration::from_secs(1));
    joiner.child.kill().unwrap();
    let _ = joiner.child.wait();
    let (status, lines) = host.wait();
    assert_eq!(status.code(), Some(1), "{lines:?}");
    assert!(
        lines
            .iter()
            .any(|line| line.starts_with("peer 1 silent for ")),
        "{lines:?}"
    );
    assert!(
        lines
            .iter()
            // The words of a `lost` ending, and of no other kind.
            .any(|line| line.starts_with("match ended at tick ")
                && line.ends_with(": peer 1 stopped answering, so the match ended")),
        "{lines:?}"
    );
    verified(&host_replay);
}
