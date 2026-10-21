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
