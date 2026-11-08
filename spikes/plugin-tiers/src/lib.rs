//! The plugin tiers spike, for the plugin architecture research. Throwaway,
//! never product code.
//!
//! The reference workload's motion pass implemented once and reached through
//! each candidate plugin tier, run inside the world's motion node through one
//! plain-data extension point ([`world::MotionPass`]), compared with the
//! sequential reference hashes at every worker count, with the processor time
//! of the motion node per tier against the kernel inlined in the world.

#![allow(clippy::too_many_arguments)]

pub mod builds;
pub mod dynamic;
pub mod importer;
pub mod module;
pub mod tiers;
pub mod world;
