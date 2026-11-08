//! The tiers' adapters behind the world's extension point: the compile-time
//! crate called directly, and a dynamic library resolved at run time, reloadable
//! between two ticks. Throwaway, never product code.

use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use crate::dynamic;
use crate::world::MotionPass;

/// The pass through raw pointers: the one signature a C ABI carries.
pub type RawMotion = unsafe extern "C" fn(
    *const f32,
    *const f32,
    *const f32,
    *const u8,
    *mut f32,
    *mut f32,
    *mut f32,
    *mut f32,
    u32,
    u32,
) -> f32;

/// The symbol every library exports.
pub const SYMBOL: &str = "motion";

/// `bool` is one byte holding 0 or 1: the same bytes, read as such.
pub fn alive_bytes(alive: &[bool]) -> &[u8] {
    // SAFETY: a bool is a byte that is 0 or 1; reading it as u8 is always valid.
    unsafe { std::slice::from_raw_parts(alive.as_ptr().cast::<u8>(), alive.len()) }
}

/// The compile-time tier: the crate linked statically, called through the
/// extension point (one trait object call per chunk, then a direct call).
pub struct CompileTime;

impl MotionPass for CompileTime {
    fn motion(
        &self,
        x: &[f32],
        y: &[f32],
        mass: &[f32],
        alive: &[bool],
        next_x: &mut [f32],
        next_y: &mut [f32],
        next_vx: &mut [f32],
        next_vy: &mut [f32],
        _tick: u32,
    ) -> f32 {
        motion_crate::motion(x, y, mass, alive_bytes(alive), next_x, next_y, next_vx, next_vy)
    }
}

/// The dynamic native tier: a library loaded at run time whose `motion` symbol
/// the chunks call through a function pointer. `reload` unloads and loads it
/// again; the host calls it between two steps, when no chunk runs, so the
/// pointer is read relaxed and the handle guarded by a lock nobody contends.
pub struct DynamicLibrary {
    path: PathBuf,
    handle: Mutex<usize>,
    symbol: AtomicUsize,
    reloads: AtomicU64,
}

impl DynamicLibrary {
    pub fn load(path: &Path) -> Result<DynamicLibrary, String> {
        let (handle, symbol) = Self::open(path)?;
        Ok(DynamicLibrary {
            path: path.to_path_buf(),
            handle: Mutex::new(handle),
            symbol: AtomicUsize::new(symbol),
            reloads: AtomicU64::new(0),
        })
    }

    fn open(path: &Path) -> Result<(usize, usize), String> {
        let handle = dynamic::open(path)?;
        match dynamic::symbol(handle, SYMBOL) {
            Ok(symbol) => Ok((handle, symbol)),
            Err(error) => {
                let _ = dynamic::close(handle);
                Err(error)
            }
        }
    }

    /// Unloads the library and loads it again; returns the time it took.
    pub fn reload(&self) -> Result<Duration, String> {
        let mut handle = self.handle.lock().unwrap();
        let started = Instant::now();
        dynamic::close(*handle)?;
        let (new_handle, symbol) = Self::open(&self.path)?;
        *handle = new_handle;
        self.symbol.store(symbol, Ordering::SeqCst);
        self.reloads.fetch_add(1, Ordering::Relaxed);
        Ok(started.elapsed())
    }

    pub fn reloads(&self) -> u64 {
        self.reloads.load(Ordering::Relaxed)
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for DynamicLibrary {
    fn drop(&mut self) {
        let _ = dynamic::close(*self.handle.lock().unwrap());
    }
}

impl MotionPass for DynamicLibrary {
    fn motion(
        &self,
        x: &[f32],
        y: &[f32],
        mass: &[f32],
        alive: &[bool],
        next_x: &mut [f32],
        next_y: &mut [f32],
        next_vx: &mut [f32],
        next_vy: &mut [f32],
        tick: u32,
    ) -> f32 {
        let count = alive.len();
        debug_assert!([x.len(), y.len(), mass.len(), next_x.len(), next_y.len(), next_vx.len(), next_vy.len()].iter().all(|&n| n == count));
        // SAFETY: the symbol was resolved from a library exporting the pass with
        // this signature, and every slice holds `count` elements.
        let call: RawMotion = unsafe { std::mem::transmute::<usize, RawMotion>(self.symbol.load(Ordering::Relaxed)) };
        unsafe {
            call(
                x.as_ptr(),
                y.as_ptr(),
                mass.as_ptr(),
                alive_bytes(alive).as_ptr(),
                next_x.as_mut_ptr(),
                next_y.as_mut_ptr(),
                next_vx.as_mut_ptr(),
                next_vy.as_mut_ptr(),
                count as u32,
                tick,
            )
        }
    }
}
