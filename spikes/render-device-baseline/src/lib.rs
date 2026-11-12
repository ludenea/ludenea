//! The device baseline spike as a library, so that a later spike of the
//! rendering research can stand on its native path: the contract of
//! `backend`, the deterministic inputs of `scene` and the three interfaces
//! reached through the system's loader. Throwaway, never product code.

pub mod backend;
#[cfg(windows)]
pub mod d3d12;
pub mod imports;
pub mod loader;
#[cfg(target_os = "macos")]
pub mod metal;
pub mod platform;
pub mod scene;
pub mod vulkan;
