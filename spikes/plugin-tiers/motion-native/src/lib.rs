//! The motion pass as a dynamic library with a C ABI. Throwaway, never
//! product code.
//!
//! The one exported symbol the dynamic native tier resolves after loading the
//! library at run time. Built by the engine's own toolchain with the permitted
//! floating-point configuration, like the host.

/// # Safety
///
/// As for [`motion_crate::motion_raw`].
#[unsafe(no_mangle)]
#[allow(clippy::too_many_arguments)]
pub unsafe extern "C" fn motion(
    x: *const f32,
    y: *const f32,
    mass: *const f32,
    alive: *const u8,
    next_x: *mut f32,
    next_y: *mut f32,
    next_vx: *mut f32,
    next_vy: *mut f32,
    count: u32,
    tick: u32,
) -> f32 {
    // SAFETY: forwarded to the caller.
    unsafe { motion_crate::motion_raw(x, y, mass, alive, next_x, next_y, next_vx, next_vy, count, tick) }
}
