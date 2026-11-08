//! The motion pass as a WebAssembly module. Throwaway, never product code.
//!
//! The export the sandboxed tier calls: the pointers are offsets into the
//! module's linear memory where the host copied the chunk's columns.

/// # Safety
///
/// As for [`motion_crate::motion_raw`]: every offset addresses `count`
/// elements inside the module's memory.
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
