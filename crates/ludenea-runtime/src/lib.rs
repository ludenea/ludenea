//! Runs a game on its own, outside the editor: the runner that drives the control host (frame
//! pacing, live devices, render frames, replays) and the frame statistics, plus, with the
//! `native` feature on native targets, the translation of window keys and gamepads to the
//! names and the layout browsers report.

#[cfg(all(feature = "native", not(target_family = "wasm")))]
pub mod native;
pub mod runner;
pub mod stats;

pub use runner::{FrameUpdate, GameFiles, Runner, RuntimeError};
