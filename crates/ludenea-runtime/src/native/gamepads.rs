//! gilrs gamepads in the Gamepad API standard layout, the layout browsers report, and the
//! gamepad slots players claim: a pad keeps its slot while it is unplugged, and a new pad takes
//! the lowest orphaned slot, so a pad that comes back under another identifier reclaims its
//! player.

use std::collections::{BTreeMap, BTreeSet};

use gilrs::{Axis, Button};
use ludenea_app::RawGamepad;

/// Buttons of the standard layout: 16 plus the home button.
const BUTTONS: usize = 17;
/// Axes of the standard layout: both sticks, horizontal then vertical.
const AXES: usize = 4;
const DPAD_UP: usize = 12;
const DPAD_DOWN: usize = 13;
const DPAD_LEFT: usize = 14;
const DPAD_RIGHT: usize = 15;

/// The standard index of `button`, or `None` for a button the layout does not have.
pub fn button_index(button: Button) -> Option<usize> {
    Some(match button {
        Button::South => 0,
        Button::East => 1,
        Button::West => 2,
        Button::North => 3,
        Button::LeftTrigger => 4,
        Button::RightTrigger => 5,
        Button::LeftTrigger2 => 6,
        Button::RightTrigger2 => 7,
        Button::Select => 8,
        Button::Start => 9,
        Button::LeftThumb => 10,
        Button::RightThumb => 11,
        Button::DPadUp => DPAD_UP,
        Button::DPadDown => DPAD_DOWN,
        Button::DPadLeft => DPAD_LEFT,
        Button::DPadRight => DPAD_RIGHT,
        Button::Mode => 16,
        Button::C | Button::Z | Button::Unknown => return None,
    })
}

/// A gamepad in the standard layout from gilrs's button values (0 to 1) and axis values (-1
/// to 1, up positive): the vertical stick axes are negated (down is positive, as in
/// browsers), and the d-pad axes some pads report press the d-pad buttons.
pub fn standard(
    buttons: impl IntoIterator<Item = (Button, f32)>,
    axes: impl IntoIterator<Item = (Axis, f32)>,
) -> RawGamepad {
    let mut pad = RawGamepad {
        buttons: vec![0.0; BUTTONS],
        axes: vec![0.0; AXES],
    };
    for (button, value) in buttons {
        if let Some(index) = button_index(button) {
            press(&mut pad, index, value);
        }
    }
    for (axis, value) in axes {
        match axis {
            Axis::LeftStickX => pad.axes[0] = value,
            Axis::LeftStickY => pad.axes[1] = -value,
            Axis::RightStickX => pad.axes[2] = value,
            Axis::RightStickY => pad.axes[3] = -value,
            Axis::DPadX if value > 0.0 => press(&mut pad, DPAD_RIGHT, value),
            Axis::DPadX => press(&mut pad, DPAD_LEFT, -value),
            Axis::DPadY if value > 0.0 => press(&mut pad, DPAD_UP, value),
            Axis::DPadY => press(&mut pad, DPAD_DOWN, -value),
            Axis::LeftZ | Axis::RightZ | Axis::Unknown => {}
        }
    }
    pad
}

/// Every button and axis gilrs names, whose values make a pad's state.
const GILRS_BUTTONS: [Button; 17] = [
    Button::South,
    Button::East,
    Button::West,
    Button::North,
    Button::LeftTrigger,
    Button::RightTrigger,
    Button::LeftTrigger2,
    Button::RightTrigger2,
    Button::Select,
    Button::Start,
    Button::LeftThumb,
    Button::RightThumb,
    Button::DPadUp,
    Button::DPadDown,
    Button::DPadLeft,
    Button::DPadRight,
    Button::Mode,
];
const GILRS_AXES: [Axis; 6] = [
    Axis::LeftStickX,
    Axis::LeftStickY,
    Axis::RightStickX,
    Axis::RightStickY,
    Axis::DPadX,
    Axis::DPadY,
];

/// The state of a connected `gamepad` in the standard layout (see `standard`).
pub fn state(gamepad: gilrs::Gamepad<'_>) -> RawGamepad {
    standard(
        GILRS_BUTTONS.into_iter().filter_map(|button| {
            gamepad
                .button_data(button)
                .map(|data| (button, data.value()))
        }),
        GILRS_AXES
            .into_iter()
            .filter_map(|axis| gamepad.axis_data(axis).map(|data| (axis, data.value()))),
    )
}

/// Presses button `index` to `value` unless something pressed it further already: a d-pad
/// reported both as buttons and as axes counts once.
fn press(pad: &mut RawGamepad, index: usize, value: f32) {
    if value > pad.buttons[index] {
        pad.buttons[index] = value;
    }
}

/// Per slot, the buttons pressed on `devices`, such as the Start that started the match.
pub fn held(devices: &[Option<RawGamepad>]) -> Vec<BTreeSet<usize>> {
    devices
        .iter()
        .map(|pad| {
            pad.iter()
                .flat_map(|pad| pad.buttons.iter().enumerate())
                .filter(|&(_, &value)| value > 0.0)
                .map(|(index, _)| index)
                .collect()
        })
        .collect()
}

/// Releases on `devices` the buttons still held since `held` was taken: a button reaches the
/// game only once released and pressed again. A button released, or a pad unplugged, leaves
/// `held`.
pub fn mask_held(devices: &mut [Option<RawGamepad>], held: &mut [BTreeSet<usize>]) {
    for (slot, buttons) in held.iter_mut().enumerate() {
        match devices.get_mut(slot) {
            Some(Some(pad)) => buttons.retain(|&index| match pad.buttons.get_mut(index) {
                Some(value) if *value > 0.0 => {
                    *value = 0.0;
                    true
                }
                _ => false,
            }),
            _ => buttons.clear(),
        }
    }
}

/// The gamepad slots players claim, in connection order.
#[derive(Clone, Debug, Default)]
pub struct Slots {
    /// Per slot, the gamepad identifier that holds it and whether it is connected.
    slots: Vec<(usize, bool)>,
}

impl Slots {
    /// The slot of gamepad `id`, which just connected: its previous slot if it had one, else
    /// the lowest slot whose pad is unplugged (the slot changes hands), else a new slot.
    pub fn connect(&mut self, id: usize) -> u32 {
        let index = match self.slots.iter().position(|&(holder, _)| holder == id) {
            Some(index) => index,
            None => match self.slots.iter().position(|&(_, connected)| !connected) {
                Some(orphaned) => orphaned,
                None => {
                    self.slots.push((id, true));
                    self.slots.len() - 1
                }
            },
        };
        self.slots[index] = (id, true);
        u32::try_from(index).expect("fewer than 2^32 gamepads")
    }

    /// Gamepad `id` was unplugged: its slot stays its own until another pad takes it.
    pub fn disconnect(&mut self, id: usize) {
        if let Some(slot) = self.slots.iter_mut().find(|(holder, _)| *holder == id) {
            slot.1 = false;
        }
    }

    /// The gamepads by slot, from each connected pad's state in `states` (by identifier);
    /// `None` for an unplugged slot.
    pub fn devices(&self, states: &BTreeMap<usize, RawGamepad>) -> Vec<Option<RawGamepad>> {
        self.slots
            .iter()
            .map(|&(id, connected)| connected.then(|| states.get(&id).cloned()).flatten())
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use gilrs::{Axis, Button};

    use super::*;

    #[test]
    fn south_is_button_zero() {
        let pad = standard([(Button::South, 1.0), (Button::Mode, 1.0)], []);
        assert_eq!(pad.buttons.len(), 17);
        assert_eq!(pad.axes, [0.0; 4]);
        assert_eq!(pad.buttons[0], 1.0);
        assert_eq!(pad.buttons[16], 1.0);
        assert_eq!(pad.buttons[1..16], [0.0; 15]);
        let order = [
            Button::South,
            Button::East,
            Button::West,
            Button::North,
            Button::LeftTrigger,
            Button::RightTrigger,
            Button::LeftTrigger2,
            Button::RightTrigger2,
            Button::Select,
            Button::Start,
            Button::LeftThumb,
            Button::RightThumb,
            Button::DPadUp,
            Button::DPadDown,
            Button::DPadLeft,
            Button::DPadRight,
            Button::Mode,
        ];
        for (index, button) in order.into_iter().enumerate() {
            let pad = standard([(button, 0.75)], []);
            assert_eq!(pad.buttons[index], 0.75, "{button:?}");
        }
        let unknown = standard([(Button::C, 1.0), (Button::Unknown, 1.0)], []);
        assert_eq!(unknown.buttons, [0.0; 17]);
    }

    #[test]
    fn stick_y_is_negated() {
        let pad = standard(
            [],
            [
                (Axis::LeftStickX, 0.5),
                (Axis::LeftStickY, 1.0),
                (Axis::RightStickX, -0.25),
                (Axis::RightStickY, -0.5),
            ],
        );
        assert_eq!(
            pad.axes,
            [0.5, -1.0, -0.25, 0.5],
            "up is negative, as in browsers"
        );
    }

    #[test]
    fn dpad_axes_press_dpad_buttons() {
        let pad = standard([], [(Axis::DPadX, -1.0), (Axis::DPadY, 1.0)]);
        assert_eq!(pad.buttons[12], 1.0, "up");
        assert_eq!(pad.buttons[13], 0.0, "down");
        assert_eq!(pad.buttons[14], 1.0, "left");
        assert_eq!(pad.buttons[15], 0.0, "right");
        assert_eq!(pad.axes, [0.0; 4], "the d-pad is not a stick");
        let pad = standard(
            [(Button::DPadRight, 1.0)],
            [(Axis::DPadX, 0.0), (Axis::DPadY, -1.0)],
        );
        assert_eq!(pad.buttons[13], 1.0, "down");
        assert_eq!(
            pad.buttons[15], 1.0,
            "a released axis keeps the pressed button"
        );
    }

    fn pad(axis: f32) -> RawGamepad {
        RawGamepad {
            buttons: vec![0.0; 17],
            axes: vec![axis, 0.0, 0.0, 0.0],
        }
    }

    #[test]
    fn reconnected_pad_keeps_its_slot() {
        let mut slots = Slots::default();
        assert_eq!(slots.connect(10), 0);
        assert_eq!(slots.connect(11), 1);
        let states = BTreeMap::from([(10, pad(0.1)), (11, pad(0.2))]);
        assert_eq!(slots.devices(&states), [Some(pad(0.1)), Some(pad(0.2))]);
        slots.disconnect(10);
        let states = BTreeMap::from([(11, pad(0.2))]);
        assert_eq!(slots.devices(&states), [None, Some(pad(0.2))]);
        assert_eq!(slots.connect(10), 0);
        assert_eq!(slots.connect(10), 0, "connecting twice keeps the slot");
        slots.disconnect(99);
    }

    #[test]
    fn new_pad_takes_the_orphaned_slot() {
        let mut slots = Slots::default();
        assert_eq!(slots.connect(10), 0);
        assert_eq!(slots.connect(11), 1);
        assert_eq!(slots.connect(12), 2);
        slots.disconnect(12);
        slots.disconnect(10);
        assert_eq!(slots.connect(20), 0, "the lowest orphaned slot");
        let states = BTreeMap::from([(20, pad(0.3)), (11, pad(0.2))]);
        assert_eq!(
            slots.devices(&states),
            [Some(pad(0.3)), Some(pad(0.2)), None]
        );
        assert_eq!(slots.connect(10), 2, "its slot went to another pad");
        assert_eq!(slots.connect(30), 3);
    }

    #[test]
    fn buttons_held_at_start_reach_the_game_only_once_pressed_again() {
        let mut start = pad(0.5);
        start.buttons[9] = 1.0;
        start.buttons[6] = 0.25;
        let mut held = held(&[Some(start.clone()), None]);
        assert_eq!(held, [BTreeSet::from([6, 9]), BTreeSet::new()]);
        // Still down: masked, the stick passes.
        let mut devices = [Some(start.clone()), None];
        mask_held(&mut devices, &mut held);
        assert_eq!(devices, [Some(pad(0.5)), None]);
        // Start released, the trigger still down.
        let mut trigger = pad(0.5);
        trigger.buttons[6] = 0.25;
        let mut devices = [Some(trigger), None];
        mask_held(&mut devices, &mut held);
        assert_eq!(devices, [Some(pad(0.5)), None]);
        assert_eq!(held[0], BTreeSet::from([6]));
        // Start pressed again reaches the game.
        let mut devices = [Some(start.clone()), None];
        mask_held(&mut devices, &mut held);
        assert_eq!(devices[0].as_ref().unwrap().buttons[9], 1.0);
        assert_eq!(devices[0].as_ref().unwrap().buttons[6], 0.0);
        // An unplugged pad forgets its held buttons.
        mask_held(&mut [None, None], &mut held);
        assert_eq!(held, [BTreeSet::new(), BTreeSet::new()]);
    }
}
