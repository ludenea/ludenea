//! Windows only: the gestures the spike produces on a window without a
//! person, the observation of the system's own move and resize loop, the
//! injected key, and the facts of the display. Throwaway, never product code.
//!
//! A move or a resize is started as the window's system menu starts it (the
//! system command posted to the window), which enters the system's own loop;
//! the arrow keys that drive it and the key that ends it are posted to the
//! window's thread, so that nothing is sent to whatever program has the
//! keyboard. A window moved by a call never enters that loop.

use std::ffi::c_void;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use crate::clock::now_ns;
use crate::host::INJECTED_VIRTUAL_KEY;
use crate::json::Json;

#[repr(C)]
struct CallWindowProcedure {
    lparam: isize,
    wparam: usize,
    message: u32,
    window: usize,
}

#[repr(C)]
#[derive(Default, Clone, Copy)]
struct Rect {
    left: i32,
    top: i32,
    right: i32,
    bottom: i32,
}

#[repr(C)]
#[derive(Default)]
struct Point {
    x: i32,
    y: i32,
}

/// `INPUT` with its keyboard member: 40 bytes on a 64-bit Windows.
#[repr(C)]
struct KeyboardInput {
    kind: u32,
    _padding: u32,
    virtual_key: u16,
    scan_code: u16,
    flags: u32,
    time: u32,
    _padding_after_time: u32,
    extra: usize,
    _rest_of_the_union: [u8; 8],
}

type HookProcedure = unsafe extern "system" fn(i32, usize, isize) -> isize;

#[link(name = "user32")]
unsafe extern "system" {
    fn PostMessageW(window: usize, message: u32, wparam: usize, lparam: isize) -> i32;
    fn SetWindowsHookExW(kind: i32, procedure: HookProcedure, module: *mut c_void, thread: u32) -> *mut c_void;
    fn UnhookWindowsHookEx(hook: *mut c_void) -> i32;
    fn CallNextHookEx(hook: *mut c_void, code: i32, wparam: usize, lparam: isize) -> isize;
    fn CreateWindowExW(extended_style: u32, class: *const u16, name: *const u16, style: u32, x: i32, y: i32, width: i32, height: i32, parent: usize, menu: usize, instance: usize, parameter: *mut c_void) -> usize;
    fn DestroyWindow(window: usize) -> i32;
    fn ShowWindowAsync(window: usize, command: i32) -> i32;
    fn SetWindowPos(window: usize, after: isize, x: i32, y: i32, width: i32, height: i32, flags: u32) -> i32;
    fn GetWindowRect(window: usize, rect: *mut Rect) -> i32;
    fn GetForegroundWindow() -> usize;
    fn SendInput(count: u32, inputs: *const KeyboardInput, size: i32) -> u32;
    fn IsIconic(window: usize) -> i32;
    fn IsWindowVisible(window: usize) -> i32;
    fn GetClientRect(window: usize, rect: *mut Rect) -> i32;
    fn GetWindowLongPtrW(window: usize, index: i32) -> isize;
    fn GetCursorPos(point: *mut Point) -> i32;
    fn SetCursorPos(x: i32, y: i32) -> i32;
    fn EnumDisplaySettingsW(device: *const u16, mode: u32, settings: *mut u8) -> i32;
}

#[link(name = "kernel32")]
unsafe extern "system" {
    fn GetCurrentThreadId() -> u32;
    fn GetTickCount() -> u32;
    fn QueryPerformanceFrequency(frequency: *mut i64) -> i32;
}

#[link(name = "dwmapi")]
unsafe extern "system" {
    fn DwmGetCompositionTimingInfo(window: usize, info: *mut u8) -> i32;
}

const WH_CALLWNDPROC: i32 = 4;
const WM_SIZING: u32 = 0x0214;
const WM_MOVING: u32 = 0x0216;
const WM_ENTERSIZEMOVE: u32 = 0x0231;
const WM_EXITSIZEMOVE: u32 = 0x0232;
const WM_SYSCOMMAND: u32 = 0x0112;
const WM_KEYDOWN: u32 = 0x0100;
const WM_KEYUP: u32 = 0x0101;
const WM_CANCELMODE: u32 = 0x001f;
const SC_SIZE: usize = 0xf000;
const SC_MOVE: usize = 0xf010;
const SC_MINIMIZE: usize = 0xf020;
const VK_RETURN: usize = 0x0d;
const VK_ESCAPE: usize = 0x1b;
const VK_RIGHT: usize = 0x27;
const VK_DOWN: usize = 0x28;
const SW_HIDE: i32 = 0;
const SW_SHOWNOACTIVATE: i32 = 4;
const HWND_TOPMOST: isize = -1;
const SWP_NOACTIVATE: u32 = 0x0010;
const SWP_SHOWWINDOW: u32 = 0x0040;
const SWP_ASYNCWINDOWPOS: u32 = 0x4000;

static OBSERVED_WINDOW: AtomicUsize = AtomicUsize::new(0);
static ENTERED: AtomicU64 = AtomicU64::new(0);
static EXITED: AtomicU64 = AtomicU64::new(0);
static ENTERED_NS: AtomicU64 = AtomicU64::new(0);
static EXITED_NS: AtomicU64 = AtomicU64::new(0);
static MOVING_OR_SIZING: AtomicU64 = AtomicU64::new(0);

unsafe extern "system" fn observe(code: i32, wparam: usize, lparam: isize) -> isize {
    if code >= 0 {
        // SAFETY: for this hook the system passes a pointer to the message
        // about to be delivered to a window procedure of the thread.
        let call = unsafe { &*(lparam as *const CallWindowProcedure) };
        if call.window == OBSERVED_WINDOW.load(Ordering::Relaxed) {
            match call.message {
                WM_ENTERSIZEMOVE => {
                    ENTERED_NS.store(now_ns(), Ordering::Relaxed);
                    ENTERED.fetch_add(1, Ordering::Relaxed);
                }
                WM_EXITSIZEMOVE => {
                    EXITED_NS.store(now_ns(), Ordering::Relaxed);
                    EXITED.fetch_add(1, Ordering::Relaxed);
                }
                WM_MOVING | WM_SIZING => {
                    MOVING_OR_SIZING.fetch_add(1, Ordering::Relaxed);
                }
                _ => {}
            }
        }
    }
    // SAFETY: the hook chain is continued with the arguments received.
    unsafe { CallNextHookEx(std::ptr::null_mut(), code, wparam, lparam) }
}

/// Watches the messages the calling thread's window procedures receive for
/// the entry and the exit of the system's move and resize loop on `window`.
/// Called on the platform thread; the hook lives until the thread ends.
pub struct Observer(*mut c_void);

impl Observer {
    pub fn install(window: usize) -> Result<Observer, String> {
        OBSERVED_WINDOW.store(window, Ordering::Relaxed);
        // SAFETY: the procedure is a function of this module; the hook is
        // scoped to the calling thread.
        let hook = unsafe { SetWindowsHookExW(WH_CALLWNDPROC, observe, std::ptr::null_mut(), GetCurrentThreadId()) };
        if hook.is_null() { Err("SetWindowsHookExW installed no hook".to_string()) } else { Ok(Observer(hook)) }
    }
}

impl Drop for Observer {
    fn drop(&mut self) {
        // SAFETY: the hook was installed by `install` and is removed once.
        unsafe { UnhookWindowsHookEx(self.0) };
    }
}

fn window_rect(window: usize) -> [i32; 4] {
    let mut rect = Rect::default();
    // SAFETY: the call fills a plain structure.
    unsafe { GetWindowRect(window, &mut rect) };
    [rect.left, rect.top, rect.right, rect.bottom]
}

fn post_key(window: usize, virtual_key: usize, scan_code: isize, extended: bool) {
    let base = 1 | (scan_code << 16) | if extended { 1 << 24 } else { 0 };
    // SAFETY: posting a message takes no pointer.
    unsafe {
        PostMessageW(window, WM_KEYDOWN, virtual_key, base);
        PostMessageW(window, WM_KEYUP, virtual_key, base | (0xc000_0000_u32 as isize));
    }
}

fn wait_for(counter: &AtomicU64, above: u64, limit: Duration) -> bool {
    let started = Instant::now();
    while counter.load(Ordering::Relaxed) <= above {
        if started.elapsed() > limit {
            return false;
        }
        std::thread::sleep(Duration::from_millis(2));
    }
    true
}

/// What one injected move or resize did.
pub struct ModalGesture {
    pub entered_ns: u64,
    pub exited_ns: u64,
    pub json: Json,
}

/// Starts the system's move or resize loop on `window` as its system menu
/// does, drives it with `steps` arrow keys posted `step` apart, and ends it
/// with the return key. Called from a thread that is not the platform thread,
/// which is held inside the loop meanwhile. The pointer, which the system
/// moves to the window, is put back where it was.
pub fn modal_gesture(window: usize, resize: bool, steps: u32, step: Duration) -> ModalGesture {
    let mut cursor = Point::default();
    // SAFETY: the call fills a plain structure.
    let cursor_known = unsafe { GetCursorPos(&mut cursor) } != 0;
    let (entered_before, exited_before, messages_before) = (ENTERED.load(Ordering::Relaxed), EXITED.load(Ordering::Relaxed), MOVING_OR_SIZING.load(Ordering::Relaxed));
    let rect_before = window_rect(window);
    // SAFETY: posting a message takes no pointer.
    unsafe { PostMessageW(window, WM_SYSCOMMAND, if resize { SC_SIZE } else { SC_MOVE }, 0) };
    let entered = wait_for(&ENTERED, entered_before, Duration::from_secs(2));
    let mut exited = false;
    let mut ended_by = "the loop was not entered";
    if entered {
        for index in 0..steps {
            // The first arrow of a resize chooses the edge; a move alternates
            // right and down so that the window travels on both axes.
            if resize || index % 2 == 0 {
                post_key(window, VK_RIGHT, 0x4d, true);
            } else {
                post_key(window, VK_DOWN, 0x50, true);
            }
            std::thread::sleep(step);
        }
        post_key(window, VK_RETURN, 0x1c, false);
        exited = wait_for(&EXITED, exited_before, Duration::from_secs(2));
        ended_by = "the return key";
        if !exited {
            post_key(window, VK_ESCAPE, 0x01, false);
            exited = wait_for(&EXITED, exited_before, Duration::from_secs(1));
            ended_by = "the escape key";
        }
        if !exited {
            // SAFETY: posting a message takes no pointer.
            unsafe { PostMessageW(window, WM_CANCELMODE, 0, 0) };
            exited = wait_for(&EXITED, exited_before, Duration::from_secs(2));
            ended_by = if exited { "a cancel message" } else { "nothing: the loop was still running" };
        }
    }
    if cursor_known {
        // SAFETY: the call takes two integers.
        unsafe { SetCursorPos(cursor.x, cursor.y) };
    }
    let rect_after = window_rect(window);
    let (entered_ns, exited_ns) = (ENTERED_NS.load(Ordering::Relaxed), EXITED_NS.load(Ordering::Relaxed));
    let list = |rect: [i32; 4]| Json::List(rect.iter().map(|&value| Json::from(value)).collect());
    let json = Json::object()
        .with("gesture", if resize { "resize" } else { "move" })
        .with("entered_the_system_loop", entered)
        .with("left_the_system_loop", exited)
        .with("ended_by", ended_by)
        .with("keys_posted", steps)
        .with("held_ms", if entered && exited { Json::Float(exited_ns.saturating_sub(entered_ns) as f64 / 1e6) } else { Json::Null })
        .with("moving_or_sizing_messages", MOVING_OR_SIZING.load(Ordering::Relaxed) - messages_before)
        .with("window_rect_before", list(rect_before))
        .with("window_rect_after", list(rect_after));
    ModalGesture { entered_ns, exited_ns, json }
}

/// Minimizes the window as its caption button does: the system command.
pub fn minimize(window: usize) {
    // SAFETY: posting a message takes no pointer.
    unsafe { PostMessageW(window, WM_SYSCOMMAND, SC_MINIMIZE, 0) };
}

/// Restores a minimized window without giving it the keyboard focus.
pub fn restore(window: usize) {
    // SAFETY: the call takes a window handle and an integer.
    unsafe { ShowWindowAsync(window, SW_SHOWNOACTIVATE) };
}

pub fn is_minimized(window: usize) -> bool {
    // SAFETY: the call takes a window handle.
    unsafe { IsIconic(window) != 0 }
}

/// A plain window of the spike's own that covers another: how a window is
/// occluded without a person. Created hidden, on the platform thread.
pub struct Cover(pub usize);

impl Cover {
    pub fn create() -> Result<Cover, String> {
        const WS_POPUP: u32 = 0x8000_0000;
        const WS_EX_TOPMOST: u32 = 0x8;
        const WS_EX_TOOLWINDOW: u32 = 0x80;
        const WS_EX_NOACTIVATE: u32 = 0x0800_0000;
        let class: Vec<u16> = "STATIC\0".encode_utf16().collect();
        let name: Vec<u16> = "cover\0".encode_utf16().collect();
        // SAFETY: the class is a system class; the strings are NUL-terminated.
        let window = unsafe { CreateWindowExW(WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE, class.as_ptr(), name.as_ptr(), WS_POPUP, 0, 0, 16, 16, 0, 0, 0, std::ptr::null_mut()) };
        if window == 0 { Err("CreateWindowExW created no covering window".to_string()) } else { Ok(Cover(window)) }
    }
}

impl Drop for Cover {
    fn drop(&mut self) {
        // SAFETY: the window was created by `create` on this thread.
        unsafe { DestroyWindow(self.0) };
    }
}

/// Shows the cover over the whole of `window`, above it, without activating it.
pub fn cover(cover: usize, window: usize) {
    let rect = window_rect(window);
    // SAFETY: the call takes handles and integers; it is queued to the
    // thread that owns the cover.
    unsafe { SetWindowPos(cover, HWND_TOPMOST, rect[0] - 8, rect[1] - 8, rect[2] - rect[0] + 16, rect[3] - rect[1] + 16, SWP_NOACTIVATE | SWP_SHOWWINDOW | SWP_ASYNCWINDOWPOS) };
}

pub fn uncover(cover: usize) {
    // SAFETY: the call takes a window handle and an integer.
    unsafe { ShowWindowAsync(cover, SW_HIDE) };
}

/// Posts the injected key to the window's thread: the platform's message
/// queue, reached without the keyboard focus.
pub fn post_injected_key(window: usize) {
    post_key(window, INJECTED_VIRTUAL_KEY, 0x76, false);
}

/// Injects the key as input of the system, which delivers it to whatever
/// window has the keyboard focus: done only when that window is the spike's.
pub fn send_injected_key(window: usize) -> Result<(), &'static str> {
    // SAFETY: the call reads the foreground window.
    if unsafe { GetForegroundWindow() } != window {
        return Err("the spike's window does not have the keyboard focus");
    }
    const INPUT_KEYBOARD: u32 = 1;
    const KEYEVENTF_KEYUP: u32 = 2;
    let key = |flags: u32| KeyboardInput { kind: INPUT_KEYBOARD, _padding: 0, virtual_key: INJECTED_VIRTUAL_KEY as u16, scan_code: 0x76, flags, time: 0, _padding_after_time: 0, extra: 0, _rest_of_the_union: [0; 8] };
    let inputs = [key(0), key(KEYEVENTF_KEYUP)];
    // SAFETY: the array holds two inputs of the size passed.
    let sent = unsafe { SendInput(2, inputs.as_ptr(), std::mem::size_of::<KeyboardInput>() as i32) };
    if sent == 2 { Ok(()) } else { Err("SendInput injected nothing") }
}

/// The system's tick count, the unit of a message's time.
pub fn tick_count_ms() -> u32 {
    // SAFETY: the call reads a counter.
    unsafe { GetTickCount() }
}

/// The frequency of the performance counter, in ticks per second.
pub fn performance_frequency() -> i64 {
    let mut frequency = 0;
    // SAFETY: the call writes one integer.
    unsafe { QueryPerformanceFrequency(&mut frequency) };
    frequency.max(1)
}


/// The current mode of the primary display and the compositor's refresh, read
/// and never changed.
pub fn display_facts() -> Json {
    let mut facts = Json::object();
    // DEVMODEW is 220 bytes; its size field is at offset 68, the width, the
    // height and the frequency at 172, 176 and 184.
    #[repr(C, align(8))]
    struct Settings([u8; 220]);
    let mut settings = Settings([0; 220]);
    settings.0[68..70].copy_from_slice(&220_u16.to_le_bytes());
    const ENUM_CURRENT_SETTINGS: u32 = 0xffff_ffff;
    let field = |bytes: &[u8; 220], offset: usize| u32::from_le_bytes([bytes[offset], bytes[offset + 1], bytes[offset + 2], bytes[offset + 3]]);
    // SAFETY: the buffer has the size the structure states.
    if unsafe { EnumDisplaySettingsW(std::ptr::null(), ENUM_CURRENT_SETTINGS, settings.0.as_mut_ptr()) } != 0 {
        facts.set("mode", format!("{} by {} at {} hertz", field(&settings.0, 172), field(&settings.0, 176), field(&settings.0, 184)));
        facts.set("mode_hertz", field(&settings.0, 184));
    } else {
        facts.set("mode", "EnumDisplaySettingsW gave no current mode");
    }
    // DWM_TIMING_INFO is packed and 292 bytes: its size, then the refresh
    // rate as two integers at offset 4 and the refresh period in ticks of the
    // performance counter at offset 12.
    let mut timing = [0_u8; 292];
    timing[0..4].copy_from_slice(&292_u32.to_le_bytes());
    // SAFETY: the buffer has the size its first field states.
    let result = unsafe { DwmGetCompositionTimingInfo(0, timing.as_mut_ptr()) };
    if result >= 0 {
        let word = |offset: usize| u32::from_le_bytes([timing[offset], timing[offset + 1], timing[offset + 2], timing[offset + 3]]);
        let period_ticks = u64::from_le_bytes(timing[12..20].try_into().unwrap_or([0; 8]));
        let period_ns = period_ticks as f64 * 1e9 / performance_frequency() as f64;
        facts.set("compositor_refresh_rate", format!("{} / {}", word(4), word(8)));
        facts.set("compositor_refresh_period_us", Json::Float(period_ns / 1000.0));
    } else {
        facts.set("compositor_timing_error", format!("DwmGetCompositionTimingInfo failed with HRESULT {:#010x}", result as u32));
    }
    facts
}

/// The styles, the visibility and the areas of a window, for the report of a
/// swapchain the system refused to create for it.
pub fn describe_window(window: usize) -> String {
    const GWL_STYLE: i32 = -16;
    const GWL_EXSTYLE: i32 = -20;
    let mut client = Rect::default();
    // SAFETY: the calls take a window handle and fill a plain structure.
    let (style, extended, visible, minimized) = unsafe {
        GetClientRect(window, &mut client);
        (GetWindowLongPtrW(window, GWL_STYLE) as u32, GetWindowLongPtrW(window, GWL_EXSTYLE) as u32, IsWindowVisible(window) != 0, IsIconic(window) != 0)
    };
    let rect = window_rect(window);
    format!("style {style:#010x}, extended style {extended:#010x}, visible {visible}, minimized {minimized}, client area {} by {}, window from ({}, {}) to ({}, {})", client.right - client.left, client.bottom - client.top, rect[0], rect[1], rect[2], rect[3])
}
