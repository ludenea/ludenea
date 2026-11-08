//! The reference workload's motion pass as a plugin. Throwaway, never product
//! code.
//!
//! One implementation of the pass over one chunk's columns, plain data in and
//! out, that the plugin tiers spike reaches through every tier: linked
//! statically as a crate, compiled to WebAssembly by the `motion-module` crate,
//! built as a dynamic library by the `motion-native` crate. The kernels are the
//! reference workload's own (systems 4 to 6: integration, boundary, energy);
//! the wander system is off in that spike, so no transcendental function
//! enters the pass.

use reference_workload::{bounced, integrated, kinetic_energy};

/// The motion pass over one chunk: `x`, `y`, `mass` and `alive` are the state
/// before the tick (`alive` as bytes, 0 or 1); `next_x` and `next_y` receive
/// the integrated and bounced positions; `next_vx` and `next_vy` hold the
/// velocities the repulsion pass wrote and receive the bounced ones. Returns
/// the chunk's kinetic energy, the alive slots added in slot order.
#[allow(clippy::too_many_arguments)]
pub fn motion(
    x: &[f32],
    y: &[f32],
    mass: &[f32],
    alive: &[u8],
    next_x: &mut [f32],
    next_y: &mut [f32],
    next_vx: &mut [f32],
    next_vy: &mut [f32],
) -> f32 {
    let mut sum = 0.0_f32;
    for k in 0..alive.len() {
        if alive[k] == 0 {
            continue;
        }
        let (px, vx) = bounced(integrated(x[k], next_vx[k]), next_vx[k]);
        let (py, vy) = bounced(integrated(y[k], next_vy[k]), next_vy[k]);
        next_x[k] = px;
        next_y[k] = py;
        next_vx[k] = vx;
        next_vy[k] = vy;
        sum += kinetic_energy(vx, vy, mass[k]);
    }
    sum
}

/// The pass through raw pointers, the one signature every tier's boundary
/// carries (a C ABI, a WebAssembly export): `count` slots per column, `tick`
/// unused while the wander system is off.
///
/// # Safety
///
/// Every pointer addresses `count` readable (and, for the `next` ones,
/// writable) elements, and no two `next` columns alias.
#[allow(clippy::too_many_arguments)]
pub unsafe fn motion_raw(
    x: *const f32,
    y: *const f32,
    mass: *const f32,
    alive: *const u8,
    next_x: *mut f32,
    next_y: *mut f32,
    next_vx: *mut f32,
    next_vy: *mut f32,
    count: u32,
    _tick: u32,
) -> f32 {
    let n = count as usize;
    // SAFETY: the caller's contract, stated above.
    unsafe {
        motion(
            std::slice::from_raw_parts(x, n),
            std::slice::from_raw_parts(y, n),
            std::slice::from_raw_parts(mass, n),
            std::slice::from_raw_parts(alive, n),
            std::slice::from_raw_parts_mut(next_x, n),
            std::slice::from_raw_parts_mut(next_y, n),
            std::slice::from_raw_parts_mut(next_vx, n),
            std::slice::from_raw_parts_mut(next_vy, n),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_pass_equals_the_reference_kernels_slot_by_slot() {
        let x = [1.0_f32, 1023.9, 500.0];
        let y = [0.5_f32, 2.0, 1023.99];
        let mass = [1.0_f32, 2.0, 4.0];
        let alive = [1_u8, 0, 1];
        let (mut nx, mut ny) = ([0.0_f32; 3], [0.0_f32; 3]);
        let mut nvx = [-64.0_f32, 10.0, 3.0];
        let mut nvy = [-32.0_f32, 10.0, 64.0];
        let sum = motion(&x, &y, &mass, &alive, &mut nx, &mut ny, &mut nvx, &mut nvy);
        let mut expected = 0.0_f32;
        for k in [0, 2] {
            let (px, vx) = bounced(integrated(x[k], [-64.0, 10.0, 3.0][k]), [-64.0, 10.0, 3.0][k]);
            let (py, vy) = bounced(integrated(y[k], [-32.0, 10.0, 64.0][k]), [-32.0, 10.0, 64.0][k]);
            assert_eq!((nx[k], ny[k], nvx[k], nvy[k]), (px, py, vx, vy));
            expected += kinetic_energy(vx, vy, mass[k]);
        }
        assert_eq!(sum, expected);
        assert_eq!((nx[1], ny[1], nvx[1], nvy[1]), (0.0, 0.0, 10.0, 10.0), "a dead slot is left alone");
    }
}
