//! The game of the tilemap painting test: a renderer with one layer, `world`, and no systems.
//! Its scene holds one empty 16 by 9 tilemap whose tileset is the swarm example's bug.

use ludenea::prelude::*;

ludenea::worker_entry!(app);

pub fn app() -> App {
    let mut app = App::new();
    app.plugin(RenderPlugin).render_layer("world", 1.0);
    app
}
