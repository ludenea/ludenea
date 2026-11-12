//! Runs the check of `render_device_baseline::frames` on every device of
//! every graphics interface of this machine and prints what it found. With
//! `--validation` each device is opened under its interface's validation
//! layer and the layer's messages are printed. Throwaway, never product code.

use std::path::PathBuf;

use render_device_baseline::backend::Api;
use render_device_baseline::frames;

fn main() {
    let validation = std::env::args().any(|argument| argument == "--validation");
    let shaders = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("shaders");
    let work = std::env::temp_dir();
    #[allow(unused_mut, reason = "the platform interfaces below are pushed on their platform only")]
    let mut apis: Vec<(&str, Result<Box<dyn Api>, String>)> = vec![("vulkan", render_device_baseline::vulkan::open(&shaders, &work))];
    #[cfg(windows)]
    apis.push(("direct3d12", render_device_baseline::d3d12::open(&shaders, &work)));
    #[cfg(target_os = "macos")]
    apis.push(("metal", render_device_baseline::metal::open(&shaders, &work)));

    let mut failures = 0;
    for (name, api) in apis {
        let api = match api {
            Ok(api) => api,
            Err(reason) => {
                println!("{name}: unavailable: {reason}");
                continue;
            }
        };
        for info in api.devices() {
            println!("{name}, device {} ({}, {}):", info.index, info.name, info.kind.name());
            let device = match api.open_frames(info.index, validation) {
                Ok(device) => device,
                Err(reason) => {
                    println!("  not opened: {reason}");
                    failures += 1;
                    continue;
                }
            };
            for line in frames::check(device.as_ref()) {
                match line.outcome {
                    Ok(found) => println!("  holds: {} ({found})", line.name),
                    Err(found) => {
                        println!("  FAILS: {} ({found})", line.name);
                        failures += 1;
                    }
                }
            }
            if validation {
                let messages = device.validation_messages();
                println!("  validation ({}): {} messages", device.validation_delivery(), messages.len());
                for message in messages.iter().take(10) {
                    println!("    {message}");
                }
            }
        }
    }
    if failures > 0 {
        println!("{failures} failures");
        std::process::exit(1);
    }
}
