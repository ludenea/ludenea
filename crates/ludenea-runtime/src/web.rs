//! The standalone web page's exports (feature `web`, browser only): the pre-match lobby and the
//! automatic claims, so the page runs the same device assignment as the native window instead
//! of rules of its own. The browser renderer (`WebRenderer`) is linked in beside them, so one
//! module gives the page everything it draws and claims with. Runtime code: nothing here
//! writes simulation state; the claims reach the game through the host's `set_players`.

use ludenea_app::players::{
    Claim, Lobby, LobbyEvent, LobbyPress, MAX_LOCAL_PLAYERS, automatic_claims,
};
use wasm_bindgen::prelude::*;

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
