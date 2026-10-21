//! Local player devices: which keyboard half and which gamepad slot each player holds, and
//! the split of the live devices into one raw input per player. GPU-free and window-free, so
//! the control host, the native runtime and the web page share it.

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

use crate::raw_input::{RawGamepad, RawInput};

/// The most local players a lobby or `--players` accepts.
pub const MAX_LOCAL_PLAYERS: usize = 4;

/// The `KeyboardEvent.code` values of the left half; every other key is the right half.
const LEFT_HALF: &[&str] = &[
    "Backquote",
    "Digit1",
    "Digit2",
    "Digit3",
    "Digit4",
    "Digit5",
    "Tab",
    "KeyQ",
    "KeyW",
    "KeyE",
    "KeyR",
    "KeyT",
    "CapsLock",
    "KeyA",
    "KeyS",
    "KeyD",
    "KeyF",
    "KeyG",
    "ShiftLeft",
    "KeyZ",
    "KeyX",
    "KeyC",
    "KeyV",
    "KeyB",
    "ControlLeft",
    "AltLeft",
    "MetaLeft",
    "Space",
];

/// The part of the keyboard a player holds.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub enum KeyboardPart {
    Whole,
    Left,
    Right,
}

impl KeyboardPart {
    /// The half a key belongs to: `Left` or `Right`, never `Whole`.
    pub fn of(code: &str) -> KeyboardPart {
        if LEFT_HALF.contains(&code) {
            KeyboardPart::Left
        } else {
            KeyboardPart::Right
        }
    }

    pub fn contains(&self, code: &str) -> bool {
        *self == KeyboardPart::Whole || *self == KeyboardPart::of(code)
    }

    fn overlaps(self, other: KeyboardPart) -> bool {
        self == KeyboardPart::Whole || other == KeyboardPart::Whole || self == other
    }
}

/// The devices one player holds: at most one keyboard part and one gamepad slot.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct Claim {
    pub keyboard: Option<KeyboardPart>,
    pub gamepad: Option<u32>,
}

impl Claim {
    /// The devices held, in words, as the runtimes show them: "left keyboard half and
    /// gamepad 1" (gamepad slots counted from 1), or "no device".
    pub fn describe(&self) -> String {
        let keyboard = self.keyboard.map(|part| match part {
            KeyboardPart::Whole => "keyboard".to_string(),
            KeyboardPart::Left => "left keyboard half".to_string(),
            KeyboardPart::Right => "right keyboard half".to_string(),
        });
        let gamepad = self
            .gamepad
            .map(|slot| format!("gamepad {}", u64::from(slot) + 1));
        let parts: Vec<String> = [keyboard, gamepad].into_iter().flatten().collect();
        if parts.is_empty() {
            "no device".to_string()
        } else {
            parts.join(" and ")
        }
    }
}

/// What is held down right now, before it is split between players.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct LiveDevices {
    /// `KeyboardEvent.code` values of the keys held down.
    pub keys: BTreeSet<String>,
    /// Indexed by gamepad slot; `None` is a disconnected slot.
    pub gamepads: Vec<Option<RawGamepad>>,
}

#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum ClaimError {
    #[error("players {} and {} both claim gamepad {gamepad}", .players.0, .players.1)]
    GamepadTwice {
        gamepad: u32,
        players: (usize, usize),
    },
    #[error("players {} and {} claim overlapping parts of the keyboard", .players.0, .players.1)]
    KeyboardOverlap { players: (usize, usize) },
}

/// The claims of `players` local players without a lobby: one player holds the whole keyboard
/// and gamepad 0; otherwise player 0 holds the left half and gamepad 0, player 1 the right
/// half and gamepad 1, and player `k >= 2` gamepad `k`.
pub fn automatic_claims(players: usize) -> Vec<Claim> {
    (0..players)
        .map(|index| Claim {
            keyboard: match (players, index) {
                (1, _) => Some(KeyboardPart::Whole),
                (_, 0) => Some(KeyboardPart::Left),
                (_, 1) => Some(KeyboardPart::Right),
                _ => None,
            },
            gamepad: u32::try_from(index).ok(),
        })
        .collect()
}

/// Refuses a gamepad slot held by two players and keyboard parts that share a key.
pub fn check_claims(claims: &[Claim]) -> Result<(), ClaimError> {
    for (second, later) in claims.iter().enumerate() {
        for (first, earlier) in claims[..second].iter().enumerate() {
            if let (Some(a), Some(b)) = (earlier.gamepad, later.gamepad)
                && a == b
            {
                return Err(ClaimError::GamepadTwice {
                    gamepad: a,
                    players: (first, second),
                });
            }
            if let (Some(a), Some(b)) = (earlier.keyboard, later.keyboard)
                && a.overlaps(b)
            {
                return Err(ClaimError::KeyboardOverlap {
                    players: (first, second),
                });
            }
        }
    }
    Ok(())
}

/// One raw input per player: the keys of its part and its gamepad as `gamepads[0]`. `None`
/// when the player holds no keyboard and its gamepad is disconnected or absent, so the game
/// receives its default input instead of an idle one.
pub fn split(devices: &LiveDevices, claims: &[Claim]) -> Vec<Option<RawInput>> {
    claims
        .iter()
        .map(|claim| {
            let gamepad = claim
                .gamepad
                .and_then(|slot| devices.gamepads.get(usize::try_from(slot).ok()?))
                .and_then(Option::as_ref);
            if claim.keyboard.is_none() && gamepad.is_none() {
                return None;
            }
            let keys = claim.keyboard.map_or_else(BTreeSet::new, |part| {
                devices
                    .keys
                    .iter()
                    .filter(|code| part.contains(code))
                    .cloned()
                    .collect()
            });
            Some(RawInput {
                keys,
                gamepads: gamepad.cloned().into_iter().collect(),
            })
        })
        .collect()
}

/// Gamepad API standard button indices the lobby treats as controls, never as claims.
const SELECT_BUTTON: usize = 8;
const START_BUTTON: usize = 9;

/// A press the lobby reacts to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LobbyPress<'a> {
    /// A `KeyboardEvent.code` value.
    Key(&'a str),
    /// A gamepad button in the Gamepad API standard order: 8 is Select, 9 is Start.
    Button { gamepad: u32, button: usize },
}

/// What a press did to the lobby. Serialized tagged by `type`, in snake case, for the web page.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum LobbyEvent {
    Claimed {
        player: usize,
    },
    /// The slot indices released, as they were before the slots were compacted, ascending.
    Released {
        players: Vec<usize>,
    },
    /// Every slot is taken.
    Full,
    /// The match begins with these claims.
    Started {
        claims: Vec<Claim>,
    },
    Ignored,
}

/// The pre-match lobby: local players claim slots with their devices, then start the match.
/// A key of a keyboard half or a gamepad button (except Start and Select) claims the next
/// free slot for that device; `Escape` releases the keyboard's claims and Select its gamepad;
/// `Enter` or Start begins the match once a slot is claimed.
#[derive(Debug, Default)]
pub struct Lobby {
    claims: Vec<Claim>,
    started: bool,
}

impl Lobby {
    pub fn new() -> Lobby {
        Lobby::default()
    }

    /// The claims in slot order; after `Started`, the final claims of the match.
    pub fn claims(&self) -> &[Claim] {
        &self.claims
    }

    pub fn started(&self) -> bool {
        self.started
    }

    pub fn press(&mut self, press: LobbyPress<'_>) -> LobbyEvent {
        if self.started {
            return LobbyEvent::Ignored;
        }
        match press {
            LobbyPress::Key("Enter") => self.start(),
            LobbyPress::Button {
                button: START_BUTTON,
                ..
            } => self.start(),
            LobbyPress::Key("Escape") => self.release(|claim| claim.keyboard.is_some()),
            LobbyPress::Button {
                gamepad,
                button: SELECT_BUTTON,
            } => self.release(|claim| claim.gamepad == Some(gamepad)),
            LobbyPress::Key(code) => {
                let part = KeyboardPart::of(code);
                self.claim(Claim {
                    keyboard: Some(part),
                    gamepad: None,
                })
            }
            LobbyPress::Button { gamepad, .. } => self.claim(Claim {
                keyboard: None,
                gamepad: Some(gamepad),
            }),
        }
    }

    fn claim(&mut self, claim: Claim) -> LobbyEvent {
        if self.claims.contains(&claim) {
            return LobbyEvent::Ignored;
        }
        if self.claims.len() >= MAX_LOCAL_PLAYERS {
            return LobbyEvent::Full;
        }
        self.claims.push(claim);
        LobbyEvent::Claimed {
            player: self.claims.len() - 1,
        }
    }

    fn release(&mut self, held: impl Fn(&Claim) -> bool) -> LobbyEvent {
        let players: Vec<usize> = (0..self.claims.len())
            .filter(|&index| held(&self.claims[index]))
            .collect();
        if players.is_empty() {
            return LobbyEvent::Ignored;
        }
        let mut index = 0;
        self.claims.retain(|_| {
            index += 1;
            !players.contains(&(index - 1))
        });
        LobbyEvent::Released { players }
    }

    fn start(&mut self) -> LobbyEvent {
        if self.claims.is_empty() {
            return LobbyEvent::Ignored;
        }
        let mut halves = self
            .claims
            .iter_mut()
            .filter(|claim| claim.keyboard.is_some());
        if let (Some(only), None) = (halves.next(), halves.next()) {
            only.keyboard = Some(KeyboardPart::Whole);
        }
        self.started = true;
        LobbyEvent::Started {
            claims: self.claims.clone(),
        }
    }
}
