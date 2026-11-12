//! The windowed presentation spike of the rendering research: a window
//! through each of winit and SDL3, and a swapchain presented by a thread that
//! is not the platform thread, natively on Direct3D 12, Vulkan and Metal and
//! through wgpu's surface. Throwaway, never product code.
//!
//! `render-windowed-presentation <cell> [--with-display] [--frames <count>]
//! [--only <section>]...` runs the plan of the machine and prints one JSON
//! document; each measurement runs in a child process (`--child <scenario>
//! key=value...`), so that a deadlock, a crash or a library's threads stay in
//! their own process.

mod clock;
#[cfg(windows)]
mod d3d12;
mod events;
mod gestures;
mod host;
mod host_sdl;
mod host_winit;
mod imports;
mod json;
mod loader;
#[cfg(target_os = "macos")]
mod metal;
mod modal;
mod pace;
mod plan;
mod presenter;
mod scenario;
mod soak;
mod thread_steps;
mod threads;
#[cfg(any(windows, target_os = "linux"))]
mod vulkan;
mod wgpu_surface;
#[cfg(windows)]
mod win32;

use scenario::Arguments;

fn main() {
    let words: Vec<String> = std::env::args().skip(1).collect();
    if words.first().map(String::as_str) == Some("--child") {
        let scenario = words.get(1).cloned().unwrap_or_default();
        let arguments = Arguments::parse(&words[2.min(words.len())..]);
        let result = match scenario.as_str() {
            "soak" => soak::run(&arguments),
            "pace" => pace::run(&arguments),
            "threads" => thread_steps::run(&arguments),
            "events" => events::run(&arguments),
            "modal" => modal::run(&arguments),
            other => json::Json::object().with("completed", false).with("error", format!("unknown scenario: {other}")),
        };
        println!("{}", result.print());
        return;
    }
    let Some(cell) = words.first().filter(|word| !word.starts_with("--")).cloned() else {
        eprintln!("usage: render-windowed-presentation <cell> [--with-display] [--frames <count>] [--only <section>]...");
        std::process::exit(2);
    };
    let mut options = plan::Options { cell, with_display: false, only: Vec::new(), frames: 10_000 };
    let mut rest = words[1..].iter();
    while let Some(word) = rest.next() {
        match word.as_str() {
            "--with-display" => options.with_display = true,
            "--only" => options.only.extend(rest.next().cloned()),
            "--frames" => options.frames = rest.next().and_then(|count| count.parse().ok()).unwrap_or(options.frames),
            other => {
                eprintln!("unknown argument: {other}");
                std::process::exit(2);
            }
        }
    }
    println!("{}", plan::run_plan(&options).print());
}
