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
use crate::in_process::text::TextHost;
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
    text: TextHost,
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
            text: TextHost::new(host),
        }
    }

    fn record_tick(&self) {
        TICK.store(self.text.host.tick(), Ordering::Relaxed);
    }
}

#[wasm_bindgen]
impl WorkerHost {
    /// Answers one protocol message; returns a JSON array of `ServerMessage`, after the
    /// events a `renderFrameJson` caused since the last call.
    pub fn handle(&mut self, message: &str) -> String {
        self.record_tick();
        to_json(&self.text.handle(message))
    }

    /// Lets `elapsed_ms` milliseconds of play and resimulation pass; returns a JSON array of
    /// `Event` messages, after the events a `renderFrameJson` caused since the last call.
    pub fn advance(&mut self, elapsed_ms: f64) -> String {
        self.record_tick();
        to_json(&self.text.advance(elapsed_ms / 1000.0))
    }

    /// The transport's raw encoding of `get_render_frame`: the same command, refused before
    /// the hello as `handle` refuses a request, whose reply is reduced to the `RenderFrame` as
    /// JSON, so a page forwards the frame text to its renderer as is. The events it causes
    /// come first in the next `handle` or `advance`. An error is `<code>: <message>`.
    #[wasm_bindgen(js_name = renderFrameJson)]
    pub fn render_frame_json(&mut self, debug_layers: Vec<String>) -> Result<String, JsValue> {
        self.record_tick();
        self.text.render_frame_json(debug_layers).map_err(|error| {
            let code = serde_json::to_value(error.code).expect("an error code serializes");
            JsValue::from_str(&format!(
                "{}: {}",
                code.as_str().unwrap_or("error"),
                error.message
            ))
        })
    }

    /// The fraction of the next tick that play owes, from 0 to below 1: what the page
    /// interpolates the last two ticks with.
    #[wasm_bindgen(js_name = tickProgress)]
    pub fn tick_progress(&self) -> f64 {
        self.text.host.tick_progress()
    }
}

fn to_json(messages: &[ServerMessage]) -> String {
    serde_json::to_string(messages).expect("protocol messages serialize")
}
