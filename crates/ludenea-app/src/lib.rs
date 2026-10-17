//! The simulation loop: an `App` collects registrations, plugins, and systems, then builds
//! a `Simulation` that steps the world one fixed tick at a time from per-player inputs.

pub mod app;
pub mod input;
pub mod parallel;
pub mod rng;
pub mod simulation;
pub mod time;

pub use app::{App, AppError, Plugin, Sim};
pub use input::{Input, Inputs};
pub use parallel::Parallel;
pub use rng::Rng;
pub use simulation::{SimError, Simulation};
pub use time::Time;
