//! Facade of the runtime skeleton spike. Throwaway, never product code.
//!
//! The one crate a host depends on. It re-exports the core and, when its
//! `presentation` feature is on, the presentation capability crate. Feature
//! flags live here and nowhere else: a capability crate has no flag for its
//! own presence, and the core knows no capability by name.

pub use skeleton_core::*;

#[cfg(feature = "presentation")]
pub use skeleton_presentation as presentation;

/// Whether this build of the facade includes the presentation capability.
pub const PRESENTATION: bool = cfg!(feature = "presentation");
