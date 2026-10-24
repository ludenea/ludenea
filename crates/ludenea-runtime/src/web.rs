//! The standalone web page's exports (feature `web`, browser only): the pre-match lobby and the
//! automatic claims, so the page runs the same device assignment as the native window instead
//! of rules of its own, and the online room (`WebOnline`), so the page meets the others as
//! every runtime does. The browser renderer (`WebRenderer`) is linked in beside them, so one
//! module gives the page everything it draws, claims and meets with. Runtime code: nothing
//! here writes simulation state; the claims reach the game through the host's `set_players`,
//! and the match through its `join_match` and `net_receive`, in the page's worker.

use ludenea_app::players::{
    Claim, Lobby, LobbyEvent, LobbyPress, MAX_LOCAL_PLAYERS, automatic_claims,
};
use ludenea_net::{Link, OnlinePeer, PeerId, RoomCode};
use wasm_bindgen::prelude::*;
use web_sys::js_sys::{Array, Object, Reflect, Uint8Array};

use crate::online::{OnlineOptions, PeerWarnings, open_link, status_json};

pub use ludenea_render::web::WebRenderer;

fn event_json(event: &LobbyEvent) -> String {
    serde_json::to_string(event).expect("a lobby event serializes")
}

fn claims_json(claims: &[Claim]) -> String {
    serde_json::to_string(claims).expect("claims serialize")
}

/// The lobby of `ludenea_app::players::Lobby`, driven by the page's key and button presses.
#[wasm_bindgen]
#[derive(Default)]
pub struct WebLobby {
    lobby: Lobby,
}

#[wasm_bindgen]
impl WebLobby {
    #[wasm_bindgen(constructor)]
    pub fn new() -> WebLobby {
        WebLobby::default()
    }

    /// Presses the key `code` (a `KeyboardEvent.code`); returns the `LobbyEvent` as JSON,
    /// `{ "type": "claimed" | "released" | "full" | "started" | "ignored", ... }`.
    #[wasm_bindgen(js_name = pressKey)]
    pub fn press_key(&mut self, code: &str) -> String {
        event_json(&self.lobby.press(LobbyPress::Key(code)))
    }

    /// Presses `button` (Gamepad API standard order) of gamepad slot `gamepad`; returns the
    /// `LobbyEvent` as JSON.
    #[wasm_bindgen(js_name = pressButton)]
    pub fn press_button(&mut self, gamepad: u32, button: u32) -> String {
        let button = usize::try_from(button).unwrap_or(usize::MAX);
        event_json(&self.lobby.press(LobbyPress::Button { gamepad, button }))
    }

    /// The claims in slot order as JSON, `[{ keyboard, gamepad }]`; once started, the final
    /// claims of the match.
    #[wasm_bindgen(js_name = claimsJson)]
    pub fn claims_json(&self) -> String {
        claims_json(self.lobby.claims())
    }

    /// The devices of each claim in slot order, in words, as JSON (`["left keyboard half",
    /// ...]`): what the page shows in the slots, as the native window's title names them.
    #[wasm_bindgen(js_name = describeClaims)]
    pub fn describe_claims(&self) -> String {
        let described: Vec<String> = self.lobby.claims().iter().map(Claim::describe).collect();
        serde_json::to_string(&described).expect("strings serialize")
    }

    #[wasm_bindgen(getter)]
    pub fn started(&self) -> bool {
        self.lobby.started()
    }
}

/// The automatic claims of `players` local players as JSON, as `--players` gives them; a count
/// outside 1 to `MAX_LOCAL_PLAYERS` is refused, naming the range.
#[wasm_bindgen(js_name = automaticClaimsJson)]
pub fn automatic_claims_json(players: u32) -> Result<String, JsValue> {
    let count = usize::try_from(players).unwrap_or(usize::MAX);
    if !(1..=MAX_LOCAL_PLAYERS).contains(&count) {
        return Err(JsValue::from_str(&format!(
            "{players} players cannot play: a game holds 1 to {MAX_LOCAL_PLAYERS} local players"
        )));
    }
    Ok(claims_json(&automatic_claims(count)))
}

/// The STUN server a page uses unless told otherwise (`ludenea_webrtc::DEFAULT_STUN`).
#[wasm_bindgen(js_name = defaultStun)]
pub fn default_stun() -> String {
    ludenea_webrtc::DEFAULT_STUN.to_owned()
}

/// One peer's room, then its match's packets, over the browser's WebRTC: the page side of an
/// online match, whose game runs in the page's worker. A thin binding over
/// `OnlineOptions::plan`, `online::open_link` and `ludenea_net::OnlinePeer`. The page polls
/// it often (every few milliseconds, and whenever the worker posts), with the time of its
/// monotonic clock in milliseconds.
#[wasm_bindgen]
pub struct WebOnline {
    peer: OnlinePeer<Box<dyn Link>>,
    code: RoomCode,
    warnings: PeerWarnings,
    /// The latest time given, in microseconds: the peer's clock never goes back.
    now_micros: u64,
}

#[wasm_bindgen]
impl WebOnline {
    /// Opens the link of `options` (JSON, as `OnlineOptions::from_json` reads it) and its
    /// room, with 128 bits of `entropy` (four 32-bit words, the lowest first) for the plan.
    /// Bad options throw a message that starts with the path of the field they name.
    #[wasm_bindgen(constructor)]
    pub fn new(options: &str, entropy: &[u32]) -> Result<WebOnline, JsValue> {
        let options =
            OnlineOptions::from_json(options).map_err(|error| JsValue::from_str(&error))?;
        let words: [u32; 4] = entropy.try_into().map_err(|_| {
            JsValue::from_str(&format!(
                "entropy: 4 words of 32 bits are needed, not {}",
                entropy.len()
            ))
        })?;
        let entropy = words
            .iter()
            .rev()
            .fold(0u128, |high, &word| (high << 32) | u128::from(word));
        let plan = options
            .plan(entropy)
            .map_err(|error| JsValue::from_str(&error.to_string()))?;
        let peer = OnlinePeer::new(open_link(plan.link.clone()), plan.role, plan.conditions)
            .map_err(|error| JsValue::from_str(&error.to_string()))?;
        Ok(WebOnline {
            peer,
            code: plan.code,
            warnings: PeerWarnings::default(),
            now_micros: 0,
        })
    }

    /// The room's code.
    #[wasm_bindgen(getter)]
    pub fn code(&self) -> String {
        self.code.to_string()
    }

    /// Polls the room at `now_ms`; returns where it stands as JSON:
    /// `{ "state": "room", "assigned", "describe", "view" }` until it agreed (`assigned` once
    /// the room exists on the listener), then `{ "state": "ready", "settings", "local" }`,
    /// the `OnlineSettings` the worker joins with and this peer's number, on every call; or
    /// `{ "state": "failed", "message" }`. Poll it on every pump, before and after the match
    /// started: the room keeps answering and its messages never pile up.
    pub fn poll(&mut self, now_ms: f64) -> String {
        let now = self.micros(now_ms);
        let status = self.peer.poll(now);
        self.warnings.take_room_drops(&mut self.peer);
        status_json(&status, self.peer.id().is_some())
    }

    /// This peer is ready with `players` local players.
    ///
    /// # Errors
    ///
    /// A message for 0 or more than 4 players, or a second call.
    #[wasm_bindgen(js_name = setReady)]
    pub fn set_ready(&mut self, players: u32) -> Result<(), JsValue> {
        let players = u8::try_from(players).unwrap_or(u8::MAX);
        self.peer
            .set_ready(players)
            .map_err(|error| JsValue::from_str(&error.to_string()))
    }

    /// The match packets arrived by `now_ms`, as `[{ from, packet }]`, each `packet` an
    /// `ArrayBuffer` of its own for the page to transfer to the worker; none before the room
    /// agreed. It also releases this peer's delayed sends that are due.
    pub fn receive(&mut self, now_ms: f64) -> Array {
        let now = self.micros(now_ms);
        self.peer
            .receive(now)
            .into_iter()
            .map(|(from, packet)| {
                let packet = Uint8Array::from(packet.as_slice()).buffer();
                let received = Object::new();
                for (key, value) in [("from", JsValue::from(from.0)), ("packet", packet.into())] {
                    Reflect::set(&received, &JsValue::from_str(key), &value)
                        .expect("a new plain object takes any property");
                }
                received
            })
            .collect()
    }

    /// Sends one match packet the worker made to peer `to`, through this peer's conditions.
    /// Before the room agreed, or when the link refuses it, it is lost: the session's
    /// redundancy sends its inputs again.
    pub fn send(&mut self, to: u8, packet: &[u8], now_ms: f64) {
        let now = self.micros(now_ms);
        self.peer.send(PeerId(to), packet.to_vec(), now);
    }

    /// The warnings since the last call: one per link and reason of the room's dropped
    /// messages, for the first 64 of them, and the reason the link closed.
    pub fn messages(&mut self) -> Vec<String> {
        self.warnings.take(&self.peer)
    }

    /// `now_ms` in microseconds, never before the latest time given.
    fn micros(&mut self, now_ms: f64) -> u64 {
        // Saturating: a negative or not-a-number time counts as 0.
        let micros = (now_ms * 1_000.0) as u64;
        self.now_micros = self.now_micros.max(micros);
        self.now_micros
    }
}
