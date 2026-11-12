//! A probe that links SDL3 and nothing else of the spike: what the library
//! does when asked for a window on this machine, with a display or without
//! one, and, when its first request fails, what a second request for X11
//! does in the same process. Its binary is also the one whose imports say
//! what linking SDL3 brings. Throwaway, never product code.

use std::ffi::CStr;

use sdl3_sys::error::SDL_GetError;
use sdl3_sys::hints::{SDL_HINT_VIDEO_DRIVER, SDL_HINT_WINDOW_ACTIVATE_WHEN_SHOWN, SDL_SetHint};
use sdl3_sys::init::{SDL_INIT_VIDEO, SDL_InitSubSystem, SDL_Quit, SDL_QuitSubSystem};
use sdl3_sys::video::{SDL_CreateWindow, SDL_DestroyWindow, SDL_GetCurrentVideoDriver, SDL_WindowFlags};

fn quoted(text: &str) -> String {
    let mut out = String::from("\"");
    for character in text.chars() {
        match character {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            control if (control as u32) < 0x20 => out.push(' '),
            other => out.push(other),
        }
    }
    out.push('"');
    out
}

fn last_error() -> String {
    // SAFETY: SDL_GetError returns a NUL-terminated string.
    unsafe { CStr::from_ptr(SDL_GetError()) }.to_string_lossy().into_owned()
}

/// One request for the video subsystem and a window, and what SDL answered.
fn request(force_x11: bool) -> (bool, String) {
    let asked = if force_x11 { "X11, named by the host" } else { "the library's own choice" };
    // SAFETY: the hints are NUL-terminated; every call is made on the main
    // thread; the window is destroyed before the subsystem is stopped.
    unsafe {
        if force_x11 {
            SDL_SetHint(SDL_HINT_VIDEO_DRIVER, c"x11".as_ptr());
        }
        if !SDL_InitSubSystem(SDL_INIT_VIDEO) {
            return (false, format!("{{\"asked\":{},\"video\":{},\"window\":null}}", quoted(asked), quoted(&format!("error: {}", last_error()))));
        }
        let driver = SDL_GetCurrentVideoDriver();
        let driver = if driver.is_null() { "none".to_string() } else { CStr::from_ptr(driver).to_string_lossy().into_owned() };
        SDL_SetHint(SDL_HINT_WINDOW_ACTIVATE_WHEN_SHOWN, c"0".as_ptr());
        let window = SDL_CreateWindow(c"windowing probe".as_ptr(), 320, 200, SDL_WindowFlags(0));
        let (opened, answer) = if window.is_null() { (false, format!("error: {}", last_error())) } else { (true, format!("opened through the video driver {driver}")) };
        if !window.is_null() {
            SDL_DestroyWindow(window);
        }
        SDL_QuitSubSystem(SDL_INIT_VIDEO);
        (opened, format!("{{\"asked\":{},\"video\":{},\"window\":{}}}", quoted(asked), quoted(&format!("started with the driver {driver}")), quoted(&answer)))
    }
}

fn main() {
    let force_x11 = std::env::args().any(|argument| argument == "--x11");
    // SAFETY: called once on the main thread before any other SDL call.
    unsafe { sdl3_sys::main::SDL_SetMainReady() };
    let (opened, first) = request(force_x11);
    let mut requests = vec![first];
    if !opened && !force_x11 && cfg!(target_os = "linux") {
        requests.push(request(true).1);
    }
    // SAFETY: the library was used by this thread only.
    unsafe { SDL_Quit() };
    println!("{{\"library\":\"SDL3\",\"requests\":[{}],\"the_process_went_on\":true}}", requests.join(","));
}
