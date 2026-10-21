//! Serves a host inside a browser Web Worker: the page passes protocol JSON text in and
//! posts the JSON it gets back. There is no clock here; the worker passes the elapsed time.
//!
//! A panic aborts the WebAssembly instance, so instead of recovering the panic hook posts
//! `{"type":"crashed","tick":<tick>,"message":"... (<file>:<line>:<column>)"}` to the worker
//! scope before it dies: the location as a native host names it.
//!
//! No automated test covers the hook: it runs only inside a browser worker, and a test
//! would need a game built to WebAssembly that panics on demand, plus the browser end to
//! end harness to observe the dying worker. The supervisor's handling of the posted message
//! is tested in the control client with a fake worker.

use std::sync::atomic::{AtomicU64, Ordering};

use ludenea_app::App;
use wasm_bindgen::prelude::*;

use crate::host::Host;
use crate::in_process::text::answer;
use crate::protocol::ServerMessage;

/// The tick the host was at when the current call began, for the panic hook.
// ponytail: a panic in the middle of a multi-tick call reports the tick the call started
// from; read the tick from the session's step if crash reports need the exact tick.
static TICK: AtomicU64 = AtomicU64::new(0);

#[wasm_bindgen]
extern "C" {
    #[wasm_bindgen(js_namespace = JSON, js_name = parse)]
    fn parse_json(text: &str) -> JsValue;

    #[wasm_bindgen(js_name = postMessage)]
    fn post_message(message: &JsValue);
}

#[wasm_bindgen]
pub struct WorkerHost {
    host: Host,
    connected: bool,
}

impl WorkerHost {
    /// Installs the crash-reporting panic hook, then builds the game.
    pub fn new(app: fn() -> App) -> WorkerHost {
        std::panic::set_hook(Box::new(|info| {
            let message = info.payload_as_str().unwrap_or("a panic without a message");
            let message = match info.location() {
                Some(location) => format!("{message} ({location})"),
                None => message.to_string(),
            };
            let crash = serde_json::json!({
                "type": "crashed",
                "tick": TICK.load(Ordering::Relaxed),
                "message": message,
            });
            post_message(&parse_json(&crash.to_string()));
        }));
        let host = Host::new(app).unwrap_or_else(|error| panic!("cannot build the game: {error}"));
        WorkerHost {
            host,
            connected: false,
        }
    }

    fn record_tick(&self) {
        TICK.store(self.host.tick(), Ordering::Relaxed);
    }
}

#[wasm_bindgen]
impl WorkerHost {
    /// Answers one protocol message; returns a JSON array of `ServerMessage`.
    pub fn handle(&mut self, message: &str) -> String {
        self.record_tick();
        let replies: Vec<ServerMessage> = if message.trim().is_empty() {
            Vec::new()
        } else {
            answer(&mut self.host, &mut self.connected, message)
        };
        to_json(&replies)
    }

    /// Lets `elapsed_ms` milliseconds of play and resimulation pass; returns a JSON array of
    /// `Event` messages.
    pub fn advance(&mut self, elapsed_ms: f64) -> String {
        self.record_tick();
        let events: Vec<ServerMessage> = self
            .host
            .advance(elapsed_ms / 1000.0)
            .into_iter()
            .map(|event| ServerMessage::Event { event })
            .collect();
        to_json(&events)
    }

    /// The fraction of the next tick that play owes, from 0 to below 1: what the page
    /// interpolates the last two ticks with.
    #[wasm_bindgen(js_name = tickProgress)]
    pub fn tick_progress(&self) -> f64 {
        self.host.tick_progress()
    }
}

fn to_json(messages: &[ServerMessage]) -> String {
    serde_json::to_string(messages).expect("protocol messages serialize")
}
