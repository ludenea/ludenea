//! `ludenea rooms`: the signaling listener with the read-only page beside it.

use std::fs;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpStream;
use std::process::{Child, Command, Output, Stdio};
use std::time::Duration;

use tempfile::TempDir;
use tungstenite::Message;

const TIMEOUT: Duration = Duration::from_secs(10);

fn ludenea(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_ludenea"))
        .args(args)
        .output()
        .expect("ludenea runs")
}

struct Rooms(Child);

impl Drop for Rooms {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

#[test]
fn rooms_prints_its_urls_and_serves_the_page() {
    let page = TempDir::new().unwrap();
    fs::write(page.path().join("game.json"), "{}").unwrap();
    fs::write(page.path().join("index.html"), "<p>game</p>").unwrap();
    let mut child = Command::new(env!("CARGO_BIN_EXE_ludenea"))
        .args(["rooms", "--page", page.path().to_str().unwrap()])
        .args(["--listen", "127.0.0.1", "--port", "0"])
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let mut stdout = BufReader::new(child.stdout.take().unwrap());
    let rooms = Rooms(child);
    let mut lines = [String::new(), String::new()];
    for line in &mut lines {
        stdout.read_line(line).unwrap();
    }
    let signaling = lines[0]
        .trim()
        .strip_prefix("signaling ws://")
        .and_then(|rest| rest.strip_suffix("/rooms"))
        .unwrap_or_else(|| panic!("unexpected first line {:?}", lines[0]))
        .to_string();
    assert!(signaling.starts_with("127.0.0.1:"), "{signaling}");
    assert_eq!(lines[1].trim(), format!("page http://{signaling}/"));

    let mut stream = TcpStream::connect(&signaling).unwrap();
    stream.set_read_timeout(Some(TIMEOUT)).unwrap();
    write!(
        stream,
        "GET / HTTP/1.1\r\nHost: {signaling}\r\nConnection: close\r\n\r\n"
    )
    .unwrap();
    let mut response = String::new();
    stream.read_to_string(&mut response).unwrap();
    assert!(response.starts_with("HTTP/1.1 200"), "{response}");
    assert!(response.ends_with("<p>game</p>"), "{response}");

    let (mut host, _) =
        tungstenite::connect(format!("ws://{signaling}/rooms/ABCDEF?host=2")).unwrap();
    match host.read().unwrap() {
        Message::Text(text) => assert!(text.contains("IdAssigned"), "{text}"),
        other => panic!("expected the id, got {other:?}"),
    }
    drop(rooms);
}

#[test]
fn port_zero_needs_listen() {
    let output = ludenea(&["rooms", "--port", "0"]);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!output.status.success(), "{stderr}");
    assert!(
        stderr.contains(
            "error: --port 0 needs --listen: the default listens on two addresses, which \
             share one port"
        ),
        "{stderr}"
    );
}

#[test]
fn a_folder_without_a_web_build_is_refused() {
    let page = TempDir::new().unwrap();
    let dir = page.path().display().to_string();
    let output = ludenea(&[
        "rooms",
        "--page",
        &dir,
        "--listen",
        "127.0.0.1",
        "--port",
        "0",
    ]);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!output.status.success(), "{stderr}");
    assert!(
        stderr.contains(&format!(
            "error: {dir} holds no web build: write one with `ludenea build <project> --web \
             --out {dir}`"
        )),
        "{stderr}"
    );
}
