//! The simulation loop: an `App` collects registrations, plugins, and systems, then builds
//! a `Simulation` that steps the world one fixed tick at a time from per-player inputs.

pub mod app;
pub mod edit;
pub mod input;
pub mod input_log;
pub mod parallel;
pub mod raw_input;
pub mod rng;
pub mod scene;
pub mod simulation;
pub mod time;

pub use app::{App, AppError, Plugin, Sim};
pub use edit::{Edit, EditError, EditLog, TimedEdit, ValueKind};
pub use input::{Input, Inputs};
pub use input_log::{INPUT_LOG_VERSION, InputLog};
pub use parallel::Parallel;
pub use raw_input::{RawGamepad, RawInput};
pub use rng::Rng;
pub use scene::SceneError;
pub use simulation::{SimError, Simulation};
pub use time::Time;
