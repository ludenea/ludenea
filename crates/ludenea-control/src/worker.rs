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
use web_sys::js_sys::{Array, Object, Reflect, Uint8Array};

use crate::host::Host;
use crate::in_process::text::TextHost;
use crate::protocol::{Event, NetPacket, ProtocolError, ServerMessage, encode_frame};

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
    /// The `render_frames` message of the last `advance`, until taken.
    frames: Option<Object>,
    /// The packets of the online match's `net_send` events, oldest first, until taken.
    net_packets: Vec<NetPacket>,
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
            frames: None,
            net_packets: Vec::new(),
        }
    }

    fn record_tick(&self) {
        TICK.store(self.text.host.tick(), Ordering::Relaxed);
    }

    /// Keeps the packets of the `net_send` events of `messages` for `takeNetPackets`, out of
    /// the JSON: a page sends them as bytes.
    fn keep_net_packets(&mut self, messages: &mut Vec<ServerMessage>) {
        messages.retain_mut(|message| match message {
            ServerMessage::Event {
                event: Event::NetSend { packets },
            } => {
                self.net_packets.append(packets);
                false
            }
            _ => true,
        });
    }
}

#[wasm_bindgen]
impl WorkerHost {
    /// Answers one protocol message; returns a JSON array of `ServerMessage`, after the
    /// events a `renderFrameJson` or a `netReceive` caused since the last call. The
    /// `net_send` events are not among them: `takeNetPackets` hands their packets out.
    pub fn handle(&mut self, message: &str) -> String {
        self.record_tick();
        let mut messages = self.text.handle(message);
        self.keep_net_packets(&mut messages);
        to_json(&messages)
    }

    /// Lets `elapsed_ms` milliseconds of play and resimulation pass; returns a JSON array of
    /// `Event` messages, after the events a `renderFrameJson` or a `netReceive` caused since
    /// the last call. The `render_frames` event is not among them: `takeRenderFrames` hands
    /// it out next. Nor are the `net_send` events: `takeNetPackets` hands out their packets.
    pub fn advance(&mut self, elapsed_ms: f64) -> String {
        self.record_tick();
        let mut messages = self.text.advance(elapsed_ms / 1000.0);
        self.keep_net_packets(&mut messages);
        let mut frames = None;
        messages.retain(|message| match message {
            ServerMessage::Event {
                event:
                    Event::RenderFrames {
                        frames: pushed,
                        progress,
                    },
            } => {
                frames = Some(frames_message(pushed, *progress));
                false
            }
            _ => true,
        });
        self.frames = frames;
        to_json(&messages)
    }

    /// The frames the last `advance` pushed, once, as
    /// `{ type: "render_frames", progress, frames }` where each of `frames` is the
    /// `ArrayBuffer` of a postcard-encoded `RenderFrame`, for the page to transfer; `undefined`
    /// when it pushed none.
    #[wasm_bindgen(js_name = takeRenderFrames)]
    pub fn take_render_frames(&mut self) -> Option<Object> {
        self.frames.take()
    }

    /// The transport's raw encoding of `get_render_frame`: the same command, refused before
    /// the hello as `handle` refuses a request, whose reply is reduced to the `RenderFrame` as
    /// JSON, so a page forwards the frame text to its renderer as is. The events it causes
    /// come first in the next `handle` or `advance`. An error is `<code>: <message>`.
    #[wasm_bindgen(js_name = renderFrameJson)]
    pub fn render_frame_json(&mut self, debug_layers: Vec<String>) -> Result<String, JsValue> {
        self.record_tick();
        self.text
            .render_frame_json(debug_layers)
            .map_err(error_value)
    }

    /// Gives the online match the packet peer `from` sent, as `net_receive` does without its
    /// JSON text: refused before the hello as `handle` refuses a request. The events it
    /// causes come first in the next `handle` or `advance`. An error is `<code>: <message>`.
    #[wasm_bindgen(js_name = netReceive)]
    pub fn net_receive(&mut self, from: u8, packet: &[u8]) -> Result<(), JsValue> {
        self.record_tick();
        self.text
            .net_receive(from, packet.to_vec())
            .map_err(error_value)
    }

    /// The packets the online match sent since the last call, oldest first, once, as
    /// `[{ to, packet }]` where each `packet` is an `ArrayBuffer` of its own, for the page to
    /// transfer; `undefined` when it sent none. They come out of the `net_send` events of
    /// `handle` and `advance`, as the render frames come out of `advance`.
    #[wasm_bindgen(js_name = takeNetPackets)]
    pub fn take_net_packets(&mut self) -> Option<Array> {
        if self.net_packets.is_empty() {
            return None;
        }
        Some(
            std::mem::take(&mut self.net_packets)
                .into_iter()
                .map(|sent| {
                    let buffer = Uint8Array::from(sent.packet.0.as_slice()).buffer();
                    plain_object([("to", JsValue::from(sent.to)), ("packet", buffer.into())])
                })
                .collect(),
        )
    }

    /// The fraction of the next tick that play owes, from 0 to below 1: what the page
    /// interpolates the last two ticks with.
    #[wasm_bindgen(js_name = tickProgress)]
    pub fn tick_progress(&self) -> f64 {
        self.text.host.tick_progress()
    }

    /// The most ticks one advance resimulates in the background, as after an online match
    /// (`RESIMULATION_BUDGET`): what bounds the advances a page waits for a resimulation.
    #[wasm_bindgen(js_name = resimulationBudget)]
    pub fn resimulation_budget(&self) -> u32 {
        crate::host::RESIMULATION_BUDGET
    }
}

/// The `render_frames` message of `frames`, each in a buffer of its own.
fn frames_message(frames: &[ludenea_draw::frame::RenderFrame], progress: f64) -> Object {
    let buffers: Array = frames
        .iter()
        .map(|frame| Uint8Array::from(encode_frame(frame).as_slice()).buffer())
        .collect();
    plain_object([
        ("type", JsValue::from_str("render_frames")),
        ("progress", JsValue::from_f64(progress)),
        ("frames", buffers.into()),
    ])
}

/// A new plain object with `properties`.
fn plain_object<const N: usize>(properties: [(&str, JsValue); N]) -> Object {
    let object = Object::new();
    for (key, value) in properties {
        Reflect::set(&object, &JsValue::from_str(key), &value)
            .expect("a new plain object takes any property");
    }
    object
}

/// `error` as the page reads it: `<code>: <message>`.
fn error_value(error: ProtocolError) -> JsValue {
    let code = serde_json::to_value(error.code).expect("an error code serializes");
    JsValue::from_str(&format!(
        "{}: {}",
        code.as_str().unwrap_or("error"),
        error.message
    ))
}

fn to_json(messages: &[ServerMessage]) -> String {
    serde_json::to_string(messages).expect("protocol messages serialize")
}
