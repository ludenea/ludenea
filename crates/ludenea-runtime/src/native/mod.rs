//! The native window: `play` runs a game in a winit window, drawn with the wgpu renderer, with
//! winit keys and gilrs gamepads translated to the names and the layout the browser reports,
//! so a game's input mapping reads the same values everywhere.

pub mod gamepads;
pub mod keys;
pub mod lobby;
pub mod surface;
mod window;

pub use window::{PlayOptions, PlayOutcome, play};
