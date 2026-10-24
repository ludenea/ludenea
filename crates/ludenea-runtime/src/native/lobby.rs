//! What the window shows before the match: the lobby's slots as a debug draw (the renderer
//! draws no text), and the window title, which names each claim and how to start, or online,
//! the room, then the match.

use std::collections::BTreeMap;

use ludenea_app::players::{KeyboardPart, Lobby, MAX_LOCAL_PLAYERS};
use ludenea_draw::frame::RenderFrame;
use ludenea_draw::{Color, DebugDrawFrame, Primitive};
use ludenea_math::Vec2;
use ludenea_net::{RoomCode, RoomView};

/// The debug draw layer the lobby is drawn in.
pub const LAYER: &str = "runtime.lobby";

const OUTLINE: Color = Color(0x8A8A93FF);
const DEVICE: Color = Color(0xEDEDEFFF);
/// One colour per slot, faded to fill a claimed slot.
const PLAYERS: [Color; MAX_LOCAL_PLAYERS] = [
    Color(0xE5484D60),
    Color(0x3E9BFF60),
    Color(0x46A75860),
    Color(0xFFB22460),
];

/// The width and height of a slot, and the gap between slots, in world units: the four
/// slots fit the default camera's 20 by 11.25 view.
const SLOT: (f32, f32) = (4.0, 6.0);
const GAP: f32 = 0.8;

/// The lobby as a frame without a camera (the default view), on layer `LAYER`: four slot
/// outlines, filled when claimed; in a claimed slot, a wide rectangle for a keyboard part
/// (its half, or all of it, filled) and a circle for a gamepad, filled while its slot is
/// among the `connected` gamepad slots.
pub fn frame(lobby: &Lobby, connected: &[u32]) -> RenderFrame {
    let mut debug = DebugDrawFrame::default();
    let mut draw = |primitive| debug.push(LAYER, None, primitive);
    for (slot, color) in PLAYERS.iter().enumerate() {
        let center_x = (slot as f32 - 1.5) * (SLOT.0 + GAP);
        let claim = lobby.claims().get(slot);
        if claim.is_some() {
            draw(rectangle(center_x, 0.0, SLOT.0, SLOT.1, *color, true));
        }
        draw(rectangle(center_x, 0.0, SLOT.0, SLOT.1, OUTLINE, false));
        let Some(claim) = claim else {
            continue;
        };
        if let Some(part) = claim.keyboard {
            let (width, height, y) = (3.0, 1.0, 1.2);
            let quarter = width / 4.0;
            let (x, filled) = match part {
                KeyboardPart::Whole => (center_x, width),
                KeyboardPart::Left => (center_x - quarter, width / 2.0),
                KeyboardPart::Right => (center_x + quarter, width / 2.0),
            };
            draw(rectangle(x, y, filled, height, DEVICE, true));
            draw(rectangle(center_x, y, width, height, DEVICE, false));
        }
        if let Some(gamepad) = claim.gamepad {
            draw(Primitive::Circle {
                center: Vec2::new(center_x, -1.2),
                radius: 0.8,
                color: DEVICE,
                fill: connected.contains(&gamepad),
            });
        }
    }
    RenderFrame {
        tick: 0,
        camera: None,
        layers: Vec::new(),
        sprites: Vec::new(),
        tilemaps: Vec::new(),
        teleported: Vec::new(),
        debug,
        warnings: Vec::new(),
    }
}

/// An axis-aligned rectangle of `width` by `height` centred on (`x`, `y`).
fn rectangle(x: f32, y: f32, width: f32, height: f32, color: Color, fill: bool) -> Primitive {
    let (half_width, half_height) = (width / 2.0, height / 2.0);
    Primitive::Polygon {
        points: vec![
            Vec2::new(x - half_width, y - half_height),
            Vec2::new(x + half_width, y - half_height),
            Vec2::new(x + half_width, y + half_height),
            Vec2::new(x - half_width, y + half_height),
        ],
        color,
        fill,
    }
}

/// The window title during the lobby: `game`, then each claim (players and gamepads counted
/// from 1) and how to start, or how to join while no slot is claimed.
pub fn title(game: &str, lobby: &Lobby) -> String {
    if lobby.claims().is_empty() {
        return format!("{game}: press a key or a gamepad button to join");
    }
    let claims: Vec<String> = lobby
        .claims()
        .iter()
        .enumerate()
        .map(|(player, claim)| format!("player {} {}", player + 1, claim.describe()))
        .collect();
    format!(
        "{game}: {}; Enter or Start to play, Escape or Select to leave",
        claims.join(", ")
    )
}

/// Where an online window stands, for its title.
#[derive(Clone, Copy, Debug)]
pub enum OnlineView<'a> {
    /// In the room, before the match: what the room waits for.
    Room(&'a RoomView),
    /// During the match: this peer's number, the match's peer count, and the peers silent,
    /// each with its silence in milliseconds.
    Playing {
        peer: u8,
        peers: usize,
        silent: &'a BTreeMap<u8, u64>,
    },
    /// After the end: why it ended.
    Ended(&'a str),
}

/// The window title of an online window: `game`, then the room `code` and what it waits for,
/// the match and its silent peers, or why the match ended.
pub fn room_title(game: &str, code: &RoomCode, view: OnlineView<'_>) -> String {
    match view {
        OnlineView::Room(room) => format!(
            "{game}: room {code}, {}, press Enter when your players have joined",
            room.describe()
        ),
        OnlineView::Playing {
            peer,
            peers,
            silent,
        } => {
            let mut title = format!("{game}: online, peer {peer} of {peers}");
            for (silent, for_ms) in silent {
                title.push_str(&format!(
                    ", peer {silent} silent for {} s",
                    *for_ms as f64 / 1000.0
                ));
            }
            title
        }
        OnlineView::Ended(message) => format!("{game}: match ended: {message}"),
    }
}

#[cfg(test)]
mod tests {
    use ludenea_app::players::LobbyPress;

    use super::*;

    fn polygons(frame: &RenderFrame) -> Vec<(bool, usize)> {
        frame.debug.layers[LAYER]
            .iter()
            .filter_map(|item| match &item.primitive {
                Primitive::Polygon { fill, points, .. } => Some((*fill, points.len())),
                _ => None,
            })
            .collect()
    }

    fn circles(frame: &RenderFrame) -> Vec<bool> {
        frame.debug.layers[LAYER]
            .iter()
            .filter_map(|item| match &item.primitive {
                Primitive::Circle { fill, .. } => Some(*fill),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn lobby_frame_fills_claimed_slots() {
        let mut lobby = Lobby::new();
        let empty = frame(&lobby, &[]);
        assert_eq!(empty.debug.layers.keys().collect::<Vec<_>>(), [LAYER]);
        assert_eq!(
            polygons(&empty),
            [(false, 4); 4],
            "four empty slot outlines"
        );
        assert!(circles(&empty).is_empty());

        lobby.press(LobbyPress::Key("KeyA"));
        lobby.press(LobbyPress::Button {
            gamepad: 1,
            button: 0,
        });
        let claimed = frame(&lobby, &[1]);
        let count = |filled: bool| {
            polygons(&claimed)
                .iter()
                .filter(|(fill, _)| *fill == filled)
                .count()
        };
        // Four slot outlines and the keyboard's outline; two filled slots and the filled
        // left half of the keyboard.
        assert_eq!(
            (count(false), count(true)),
            (5, 3),
            "{:?}",
            polygons(&claimed)
        );
        assert_eq!(circles(&claimed), [true], "the connected pad is filled");
        assert_eq!(
            circles(&frame(&lobby, &[])),
            [false],
            "an unplugged pad is not"
        );
    }

    #[test]
    fn title_names_claims_and_how_to_start() {
        let mut lobby = Lobby::new();
        assert_eq!(
            title("swarm", &lobby),
            "swarm: press a key or a gamepad button to join"
        );
        lobby.press(LobbyPress::Key("KeyA"));
        lobby.press(LobbyPress::Key("ArrowLeft"));
        lobby.press(LobbyPress::Button {
            gamepad: 0,
            button: 0,
        });
        assert_eq!(
            title("swarm", &lobby),
            "swarm: player 1 left keyboard half, player 2 right keyboard half, \
             player 3 gamepad 1; Enter or Start to play, Escape or Select to leave"
        );
    }

    #[test]
    fn room_title_names_the_room_then_the_match_then_its_end() {
        let code = RoomCode::parse("23456A").unwrap();
        let room = RoomView {
            host: true,
            peers: Some(2),
            connected: 1,
            ready: 1,
            unmeshed: 0,
            unfit: 0,
            local_ready: true,
        };
        assert_eq!(
            room_title("arena", &code, OnlineView::Room(&room)),
            format!(
                "arena: room 23456A, {}, press Enter when your players have joined",
                room.describe()
            )
        );
        fn playing(silent: &BTreeMap<u8, u64>) -> OnlineView<'_> {
            OnlineView::Playing {
                peer: 0,
                peers: 2,
                silent,
            }
        }
        let mut silent = BTreeMap::new();
        assert_eq!(
            room_title("arena", &code, playing(&silent)),
            "arena: online, peer 0 of 2"
        );
        silent.insert(1, 1500);
        assert_eq!(
            room_title("arena", &code, playing(&silent)),
            "arena: online, peer 0 of 2, peer 1 silent for 1.5 s"
        );
        assert_eq!(
            room_title("arena", &code, OnlineView::Ended("peer 1 left at tick 90")),
            "arena: match ended: peer 1 left at tick 90"
        );
    }
}
