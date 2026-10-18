//! `host --stdio` on the real standard output: this test binary runs itself as the game.

use std::io::{BufRead, BufReader, Write};
use std::process::{Command as Process, Stdio};
use std::time::{Duration, Instant};

use ludenea_app::{App, Parallel, Sim};
use ludenea_control::{Command, Document, PROTOCOL_VERSION};
use ludenea_ecs::{Component, Query, Runtime};
use serde::{Deserialize, Serialize};

/// Set in the child process, which serves the game instead of testing.
const CHILD: &str = "LUDENEA_HOST_TEST_CHILD";

#[derive(Component, Serialize, Deserialize)]
struct Counter(u32);

/// Prints from the pool's threads, as game code may.
fn shout(counters: Query<&Counter>, parallel: Runtime<Parallel>) {
    let threads = parallel.threads();
    let items: Vec<u32> = counters.iter().map(|counter| counter.0).collect();
    parallel.map(&items, 1, |item| println!("threads={threads} item={item}"));
}

fn app() -> App {
    let mut app = App::new();
    app.component::<Counter>().system(Sim, shout);
    app
}

#[test]
fn child_serves_the_game() {
    if std::env::var_os(CHILD).is_some() {
        ludenea_cli::run(app, &["game", "host", "--stdio"], &mut std::io::sink());
    }
}

fn request(id: u64, command: &Command) -> String {
    serde_json::json!({ "type": "request", "id": id, "command": command }).to_string()
}

#[test]
#[allow(clippy::disallowed_methods)]
fn printing_from_parallel_systems_never_blocks_the_host() {
    let entities: Vec<String> = (0..64)
        .map(|i| format!(r#"{{ "host::Counter": ({i}) }}"#))
        .collect();
    let scene = format!(
        "Scene(version: 1, resources: {{}}, entities: [{}])",
        entities.join(", ")
    );
    let step = Command::Step {
        ticks: 3,
        record_hashes: false,
    };
    let input = [
        serde_json::json!({ "type": "hello", "protocol_version": PROTOCOL_VERSION }).to_string(),
        request(
            1,
            &Command::LoadScene {
                scene,
                assets: Vec::new(),
                document: Document::Scene,
            },
        ),
        request(2, &step),
        request(3, &Command::SetThreads { threads: 4 }),
        request(4, &step),
    ];
    let mut child = Process::new(std::env::current_exe().unwrap())
        .args([
            "child_serves_the_game",
            "--exact",
            "--nocapture",
            "--test-threads=1",
        ])
        .env(CHILD, "1")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    // Closing the input ends the session once every request is answered.
    child
        .stdin
        .take()
        .unwrap()
        .write_all((input.join("\n") + "\n").as_bytes())
        .unwrap();
    let stdout = child.stdout.take().unwrap();
    let reader = std::thread::spawn(move || {
        BufReader::new(stdout)
            .lines()
            .map_while(Result::ok)
            .collect::<Vec<_>>()
    });
    let started = Instant::now();
    while child.try_wait().unwrap().is_none() {
        if started.elapsed() > Duration::from_secs(60) {
            child.kill().unwrap();
            panic!("the host is stuck");
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    let lines = reader.join().unwrap();
    let answered = lines
        .iter()
        .filter(|line| line.starts_with(r#"{"type":"response""#))
        .count();
    assert_eq!(answered, 4, "{lines:?}");
    let available = std::thread::available_parallelism().unwrap().get();
    let first = lines
        .iter()
        .find(|line| line.starts_with("threads="))
        .unwrap();
    assert!(
        first.starts_with(&format!("threads={available} ")),
        "uses every core: {first}"
    );
    assert!(
        lines.iter().any(|line| line.starts_with("threads=4 ")),
        "{lines:?}"
    );
}
