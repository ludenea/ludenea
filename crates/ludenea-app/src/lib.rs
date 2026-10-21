//! The simulation loop: an `App` collects registrations, plugins, and systems, then builds
//! a `Simulation` that steps the world one fixed tick at a time from per-player inputs.

pub mod app;
pub mod assets;
pub mod edit;
pub mod input;
pub mod input_log;
pub mod meta;
pub mod panic;
pub mod parallel;
pub mod path;
pub mod players;
pub mod pose;
pub mod prefab;
pub mod raw_input;
pub mod replay;
pub mod rng;
pub mod scene;
pub mod simulation;
pub mod time;
pub mod trace;

pub use app::{App, AppError, Plugin, Sim};
pub use assets::{AssetSource, AssetTexts, NamedAssetTexts, NoAssets};
pub use edit::{EDIT_LOG_VERSION, Edit, EditError, EditLog, EntityRef, TimedEdit, ValueKind};
pub use input::{Input, Inputs};
pub use input_log::{INPUT_LOG_VERSION, InputLog};
pub use panic::{GamePanic, catch_panic};
pub use parallel::{Parallel, ParallelError};
pub use prefab::{InstanceInfo, PREFAB_VERSION, Resolved, ResolvedEntity, resolve};
pub use raw_input::{RawGamepad, RawInput};
pub use rng::Rng;
pub use scene::{LoadAddress, SceneError, SceneLoad};
pub use simulation::{SimError, Simulation};
pub use time::Time;
