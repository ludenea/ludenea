use ludenea::prelude::*;
use serde::{Deserialize, Serialize};

ludenea::worker_entry!(app);

/// What the player does during one tick.
#[derive(Serialize, Deserialize, Schema, Default, Clone)]
pub struct GameInput {
    /// Direction held on the arrow keys.
    pub direction: Vec2,
}

pub struct GamePlugin;

impl Plugin for GamePlugin {
    fn build(&self, app: &mut App) {
        app.input::<GameInput>().input_mapping(map_raw_input);
    }
}

pub fn app() -> App {
    let mut app = App::new();
    app.plugin(PhysicsPlugin).plugin(GamePlugin);
    app
}

fn map_raw_input(raw: &RawInput, _player: usize) -> GameInput {
    let axis = |negative: &str, positive: &str| {
        f32::from(i8::from(raw.keys.contains(positive)) - i8::from(raw.keys.contains(negative)))
    };
    GameInput {
        direction: Vec2::new(
            axis("ArrowLeft", "ArrowRight"),
            axis("ArrowDown", "ArrowUp"),
        ),
    }
}
