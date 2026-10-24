//! The `peer` command: the options it refuses, and the rooms it gives up on, through a
//! signaling listener of this test process.

use std::fs;
use std::net::{SocketAddr, TcpListener as StdListener};
use std::path::PathBuf;
use std::thread::JoinHandle;

use ludenea_app::{App, Sim};
use ludenea_cli::run;
use ludenea_ecs::{Component, Query};
use ludenea_signal::{Options, serve};
use serde::{Deserialize, Serialize};

#[derive(Clone, Component, Serialize, Deserialize)]
#[component(name = "peer::Counter")]
struct Counter(u32);

fn count(mut counters: Query<&mut Counter>) {
    for counter in counters.iter_mut() {
        counter.0 += 1;
    }
}

fn app() -> App {
    let mut app = App::new();
    app.input::<u8>()
        .input_mapping(|raw, _| u8::from(!raw.keys.is_empty()))
        .component::<Counter>()
        .system(Sim, count);
    app
}

/// A fresh directory holding a scene.
fn scene(test: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("ludenea-peer-{test}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    let scene = dir.join("scene.ron");
    fs::write(
        &scene,
        r#"Scene(version: 1, resources: {}, entities: [{ "peer::Counter": (0) }])"#,
    )
    .unwrap();
    scene
}

/// Runs the command line with `args` after the program name; returns the exit code and the
/// output.
fn cli(args: &[&str]) -> (i32, String) {
    let mut out = Vec::new();
    let all: Vec<&str> = ["game"].iter().chain(args).copied().collect();
    let code = run(app, &all, &mut out);
    (code, String::from_utf8(out).unwrap())
}

/// A signaling listener on a port of `127.0.0.1` the system picks, stopped when dropped.
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

#[test]
fn peer_refuses_bad_options() {
    let scene = scene("options");
    let scene = scene.to_str().unwrap();
    let refusals: [(&[&str], &str); 6] = [
        (&[], "--host"),
        (&["--host", "--join", "23456A"], "cannot be used with"),
        (&["--host", "--peers", "5"], "5 is not in 2..=4"),
        (
            &["--join", "23456A", "--peers", "3"],
            "the argument '--join <CODE>' cannot be used with '--peers <PEERS>'",
        ),
        (
            &["--join", "0O0O0O"],
            "'0' (character 1) is not in a room code",
        ),
        (
            &["--host", "--stun", "http://x"],
            "the STUN URL 'http://x' must start with stun: or stuns:",
        ),
    ];
    for (options, problem) in refusals {
        let args: Vec<&str> = ["peer", scene].iter().chain(options).copied().collect();
        let (code, out) = cli(&args);
        assert_eq!(code, 2, "{options:?}: {out}");
        assert!(out.contains(problem), "{options:?}: {out}");
    }
}

#[test]
fn peer_reports_an_unknown_room() {
    let listener = Listener::start();
    let signaling = listener.signaling();
    let scene = scene("unknown");
    let (code, out) = cli(&[
        "peer",
        scene.to_str().unwrap(),
        "--join",
        "FGHJKM",
        "--signaling",
        &signaling,
        "--no-stun",
    ]);
    assert_eq!(code, 2, "{out}");
    assert!(
        out.contains(&format!(
            "error: the connection to the room closed: no room FGHJKM at {signaling}: check \
             the code, or there is no Ludenea listener there"
        )),
        "{out}"
    );
}

#[test]
fn peer_gives_up_on_an_empty_room() {
    let listener = Listener::start();
    let signaling = listener.signaling();
    let scene = scene("empty");
    let (code, out) = cli(&[
        "peer",
        scene.to_str().unwrap(),
        "--host",
        "--room",
        "23456C",
        "--room-timeout",
        "1",
        "--signaling",
        &signaling,
        "--no-stun",
    ]);
    assert_eq!(code, 2, "{out}");
    assert!(out.lines().any(|line| line == "room 23456C"), "{out}");
    assert!(
        out.contains("error: room 23456C was not ready within 1 s: 1 of 2 peers here"),
        "{out}"
    );
}
