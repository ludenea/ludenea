//! Runs a game on its own, outside the editor: the runner that drives the control host (frame
//! pacing, live devices, render frames, replays), the online match it plays over a link
//! (`online`), and the frame statistics, plus, with the
//! `native` feature on native targets, the native window (`native::play`), whose keys and
//! gamepads are translated to the names and the layout browsers report, and, with the `web`
//! feature in the browser, the standalone web page's exports (`web`).

#[cfg(all(feature = "native", not(target_family = "wasm")))]
pub mod native;
pub mod online;
pub mod runner;
pub mod stats;
#[cfg(all(feature = "web", target_arch = "wasm32", target_os = "unknown"))]
pub mod web;

pub use runner::{FrameUpdate, GameFiles, Runner, RuntimeError};
