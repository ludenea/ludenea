//! What the three paths of the spike share: the many draws workload, a
//! future run to completion on the calling thread, the log the process
//! collects, and the shader compiler libraries a process has loaded.
//! Throwaway, never product code.

use std::sync::Mutex;

use render_device_baseline::scene::{Draw, HEIGHT, Scene, TEXTURE_COUNT, Vertex, WIDTH};

/// The draws of the many draws workload.
pub const MANY_DRAWS: usize = 10_000;
/// The workload is a square grid of this many cells a side, one draw a cell.
const GRID: usize = 100;
/// Each timing of the many draws workload is taken this many times.
pub const MANY_DRAWS_REPETITIONS: usize = 5;

/// The device baseline's scene with its vertices and its draws replaced by
/// the many draws workload: 10 000 draws in one pass on the scene's target,
/// one quad of six vertices in each cell of a grid of 100 by 100 cells of
/// 2.56 pixels. Draw `i` names texture `i mod 5` and sampler `i mod 3` as its
/// per-draw constants; the rows alternate between the opaque and the blended
/// pipeline, so the pipeline changes 100 times; no draw is indirect. The
/// four chunks hold 2 500 draws each. Every input is exact in binary
/// floating point, so the bytes are the same on every machine.
pub fn many_draws_scene() -> Scene {
    let mut scene = Scene::build();
    let mut vertices = Vec::with_capacity(MANY_DRAWS * 6);
    let mut draws = Vec::with_capacity(MANY_DRAWS);
    let cell = WIDTH as f32 / GRID as f32;
    for index in 0..MANY_DRAWS {
        let (column, row) = (index % GRID, index / GRID);
        // A quad of two pixels in a cell of 2.56: neighbours never touch.
        let left = (column as f32 * cell).floor() + 0.25;
        let top = (row as f32 * cell).floor() + 0.25;
        let to_clip = |x: f32, y: f32| [x / WIDTH as f32 * 2.0 - 1.0, 1.0 - y / HEIGHT as f32 * 2.0, 0.5, 1.0];
        let shade = 0.25 + 0.75 * ((index % 7) as f32 / 8.0);
        let color = [shade, 1.0 - shade * 0.5, 0.5 + (row % 2) as f32 * 0.25, 0.75];
        let corner = |x: f32, y: f32, u: f32, v: f32| Vertex { position: to_clip(x, y), uv: [u, v], color };
        let (right, bottom) = (left + 2.0, top + 2.0);
        let first = vertices.len() as u32;
        vertices.extend([
            corner(left, top, 0.0, 0.0),
            corner(right, top, 1.0, 0.0),
            corner(left, bottom, 0.0, 1.0),
            corner(left, bottom, 0.0, 1.0),
            corner(right, top, 1.0, 0.0),
            corner(right, bottom, 1.0, 1.0),
        ]);
        draws.push(Draw {
            name: "grid_quad",
            first_vertex: first,
            vertex_count: 6,
            texture: (index % TEXTURE_COUNT) as u32,
            sampler: (index % 3) as u32,
            blended: row % 2 == 1,
            indirect: false,
        });
    }
    let quarter = MANY_DRAWS / 4;
    scene.vertices = vertices;
    scene.draws = draws;
    scene.chunks = [0..quarter, quarter..2 * quarter, 2 * quarter..3 * quarter, 3 * quarter..MANY_DRAWS];
    scene
}

/// Runs a future to completion on the calling thread. The futures of wgpu's
/// native backends are ready at once or after the device was polled, so the
/// loop yields between polls and needs no reactor.
pub fn block_on<F: Future>(future: F) -> F::Output {
    let mut future = std::pin::pin!(future);
    let mut context = std::task::Context::from_waker(std::task::Waker::noop());
    loop {
        if let std::task::Poll::Ready(output) = future.as_mut().poll(&mut context) {
            return output;
        }
        std::thread::yield_now();
    }
}

/// One record of the `log` crate, as the process received it.
#[derive(Clone, Debug)]
pub struct LogRecord {
    pub level: log::Level,
    pub target: String,
    pub message: String,
}

/// The logger of the process: wgpu and the validation layers beneath it
/// write to the `log` crate, and this keeps what they wrote.
pub struct CollectedLog {
    records: Mutex<Vec<LogRecord>>,
}

static COLLECTED: CollectedLog = CollectedLog { records: Mutex::new(Vec::new()) };

impl log::Log for CollectedLog {
    fn enabled(&self, _metadata: &log::Metadata) -> bool {
        true
    }

    fn log(&self, record: &log::Record) {
        if let Ok(mut records) = self.records.lock() {
            records.push(LogRecord { level: record.level(), target: record.target().to_string(), message: record.args().to_string() });
        }
    }

    fn flush(&self) {}
}

/// Installs the collecting logger at the given level; later calls change
/// nothing.
pub fn collect_log(level: log::LevelFilter) {
    if log::set_logger(&COLLECTED).is_ok() {
        log::set_max_level(level);
    }
}

/// The records collected since the last call.
pub fn drain_log() -> Vec<LogRecord> {
    COLLECTED.records.lock().map(|mut records| std::mem::take(&mut *records)).unwrap_or_default()
}

/// The names that mark a shader compiler or translator among the libraries
/// of a process, lowercase.
const COMPILER_MARKS: [&str; 9] = ["d3dcompiler", "dxcompiler", "dxil", "glslang", "shaderc", "spirv", "slang", "metalirconverter", "naga"];

/// The libraries the process has loaded whose name marks a shader compiler,
/// or the reason the list cannot be read.
pub fn loaded_shader_compilers() -> Result<Vec<String>, String> {
    let mut found: Vec<String> = loaded_libraries()?
        .into_iter()
        .filter(|path| {
            let name = path.rsplit(['/', '\\']).next().unwrap_or(path).to_ascii_lowercase();
            COMPILER_MARKS.iter().any(|mark| name.contains(mark))
        })
        .collect();
    found.sort();
    found.dedup();
    Ok(found)
}

#[cfg(target_os = "linux")]
fn loaded_libraries() -> Result<Vec<String>, String> {
    let maps = std::fs::read_to_string("/proc/self/maps").map_err(|reason| format!("/proc/self/maps could not be read: {reason}"))?;
    let mut libraries: Vec<String> = maps.lines().filter_map(|line| line.split_whitespace().nth(5)).filter(|path| path.starts_with('/')).map(str::to_string).collect();
    libraries.sort();
    libraries.dedup();
    Ok(libraries)
}

#[cfg(target_os = "macos")]
fn loaded_libraries() -> Result<Vec<String>, String> {
    unsafe extern "C" {
        fn _dyld_image_count() -> u32;
        fn _dyld_get_image_name(index: u32) -> *const std::ffi::c_char;
    }
    let mut libraries = Vec::new();
    // SAFETY: the two calls are the dynamic loader's own listing of the
    // images of this process; a name is a NUL-terminated string that lives
    // as long as its image, or null for an index that has gone.
    unsafe {
        for index in 0.._dyld_image_count() {
            let name = _dyld_get_image_name(index);
            if !name.is_null() {
                libraries.push(std::ffi::CStr::from_ptr(name).to_string_lossy().into_owned());
            }
        }
    }
    Ok(libraries)
}

#[cfg(windows)]
fn loaded_libraries() -> Result<Vec<String>, String> {
    use std::ffi::c_void;

    const SNAPSHOT_MODULES: u32 = 0x8;
    const INVALID_HANDLE: isize = -1;
    const MAX_PATH: usize = 260;

    #[repr(C)]
    struct ModuleEntry {
        size: u32,
        module_id: u32,
        process_id: u32,
        global_usage: u32,
        process_usage: u32,
        base_address: *mut u8,
        base_size: u32,
        module: *mut c_void,
        name: [u16; 256],
        path: [u16; MAX_PATH],
    }

    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn CreateToolhelp32Snapshot(flags: u32, process_id: u32) -> *mut c_void;
        fn Module32FirstW(snapshot: *mut c_void, entry: *mut ModuleEntry) -> i32;
        fn Module32NextW(snapshot: *mut c_void, entry: *mut ModuleEntry) -> i32;
        fn CloseHandle(handle: *mut c_void) -> i32;
        fn GetCurrentProcessId() -> u32;
    }

    let mut libraries = Vec::new();
    // SAFETY: the snapshot lists the modules of this process; the entry is a
    // MODULEENTRY32W whose first field holds its size, as the calls require,
    // and the handle is closed before returning.
    unsafe {
        let snapshot = CreateToolhelp32Snapshot(SNAPSHOT_MODULES, GetCurrentProcessId());
        if snapshot as isize == INVALID_HANDLE {
            return Err("CreateToolhelp32Snapshot gave no list of the process's modules".to_string());
        }
        let mut entry: ModuleEntry = std::mem::zeroed();
        entry.size = size_of::<ModuleEntry>() as u32;
        let mut more = Module32FirstW(snapshot, &mut entry);
        while more != 0 {
            let length = entry.path.iter().position(|&unit| unit == 0).unwrap_or(MAX_PATH);
            libraries.push(String::from_utf16_lossy(&entry.path[..length]));
            more = Module32NextW(snapshot, &mut entry);
        }
        CloseHandle(snapshot);
    }
    Ok(libraries)
}

#[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
fn loaded_libraries() -> Result<Vec<String>, String> {
    Err("the libraries of a process are not listed on this platform".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_many_draws_workload_has_the_stated_shape() {
        let scene = many_draws_scene();
        assert_eq!(scene.draws.len(), MANY_DRAWS);
        assert_eq!(scene.vertices.len(), MANY_DRAWS * 6);
        assert!(scene.draws.iter().all(|draw| draw.vertex_count == 6 && !draw.indirect && draw.texture < TEXTURE_COUNT as u32 && draw.sampler < 3));
        let pipeline_changes = scene.draws.windows(2).filter(|pair| pair[0].blended != pair[1].blended).count();
        assert_eq!(pipeline_changes, GRID - 1);
        assert_eq!(scene.chunks.iter().map(|chunk| chunk.len()).sum::<usize>(), MANY_DRAWS);
        assert_eq!(scene.chunks[3].end, MANY_DRAWS);
        // Every quad lies inside the target and inside its own cell.
        for vertex in &scene.vertices {
            assert!(vertex.position[0].abs() <= 1.0 && vertex.position[1].abs() <= 1.0);
        }
        // The textures and the other exercises are the baseline's own.
        assert_eq!(scene.textures.len(), Scene::build().textures.len());
    }

    #[test]
    fn a_ready_future_and_a_pending_one_both_complete() {
        assert_eq!(block_on(async { 7 }), 7);
        let mut polls = 0;
        let later = std::future::poll_fn(|_| {
            polls += 1;
            if polls < 3 { std::task::Poll::Pending } else { std::task::Poll::Ready(polls) }
        });
        assert_eq!(block_on(later), 3);
    }

    #[test]
    fn the_process_lists_its_libraries_and_none_is_a_shader_compiler() {
        if cfg!(any(target_os = "linux", target_os = "macos", windows)) {
            assert!(loaded_libraries().is_ok_and(|libraries| !libraries.is_empty()));
            assert_eq!(loaded_shader_compilers().unwrap(), Vec::<String>::new());
        }
    }
}
