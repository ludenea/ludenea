//! The session facts of a process, the discovery of a display or window
//! server session by the presentation capability, and the self-description of
//! a host's capabilities. Throwaway, never product code.
//!
//! The description says what forms the four capability axes take in this
//! host: it is what a client discovers through the contract. It carries no
//! reason: whether a capability is absent because the build has no crate for
//! it, because startup discovery found no display, or because the host was
//! asked to run headless is a fact of the environment, reported beside the
//! description, never inside it.

use std::env;
use std::process::Command;

use reference_workload::Json;

/// The forms of the four capability axes of this host.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Description {
    pub presentation: &'static str,
    pub audio_output: &'static str,
    pub input_devices: &'static str,
    pub platform_session: &'static str,
}

impl Description {
    /// `windowed` and `interactive` only when the capability is built in, was
    /// discovered and was not requested absent; the audio and input axes have
    /// no capability in this spike and are always null.
    pub fn new(built: bool, discovered: bool, headless: bool) -> Description {
        let present = built && discovered && !headless;
        Description {
            presentation: if present { "windowed" } else { "null" },
            audio_output: "null",
            input_devices: "null",
            platform_session: if present { "interactive" } else { "none" },
        }
    }

    /// A stable encoding: the same fields in the same order, so that two
    /// descriptions compare as strings.
    pub fn json(&self) -> String {
        format!(
            r#"{{"presentation":"{}","audio_output":"{}","input_devices":"{}","platform_session":"{}"}}"#,
            self.presentation, self.audio_output, self.input_devices, self.platform_session
        )
    }
}

/// What the presentation capability found at startup, and how.
#[derive(Clone, Debug)]
pub struct Discovery {
    pub built: bool,
    pub found: bool,
    pub method: &'static str,
    pub detail: String,
}

impl Discovery {
    pub fn write(&self, out: &mut Json) {
        out.key("discovery");
        out.object_start();
        out.bool("capability_built", self.built);
        out.bool("found", self.found);
        out.string("method", self.method);
        out.string("detail", &self.detail);
        out.object_end();
    }
}

#[cfg(not(feature = "presentation"))]
pub fn discover() -> Discovery {
    Discovery { built: false, found: false, method: "none", detail: "no presentation capability in this build".to_string() }
}

#[cfg(all(feature = "presentation", target_os = "linux"))]
pub fn discover() -> Discovery {
    use std::ffi::c_void;
    use std::os::raw::{c_char, c_int};
    #[link(name = "libX11.so.6", kind = "dylib", modifiers = "+verbatim")]
    unsafe extern "C" {
        fn XOpenDisplay(name: *const c_char) -> *mut c_void;
        fn XCloseDisplay(display: *mut c_void) -> c_int;
    }
    // SAFETY: a null name asks for the DISPLAY variable's server; the display
    // is closed when opened.
    let display = unsafe { XOpenDisplay(std::ptr::null()) };
    let found = !display.is_null();
    if found {
        unsafe { XCloseDisplay(display) };
    }
    Discovery {
        built: true,
        found,
        method: "XOpenDisplay",
        detail: format!("DISPLAY {}", env::var("DISPLAY").map_or("unset".to_string(), |d| format!("is {d}"))),
    }
}

#[cfg(all(feature = "presentation", target_os = "macos"))]
pub fn discover() -> Discovery {
    use std::ffi::c_void;
    #[link(name = "CoreGraphics", kind = "framework")]
    unsafe extern "C" {
        fn CGSessionCopyCurrentDictionary() -> *const c_void;
    }
    #[link(name = "CoreFoundation", kind = "framework")]
    unsafe extern "C" {
        fn CFRelease(cf: *const c_void);
    }
    // SAFETY: the dictionary is owned by the caller and released when non-null.
    let session = unsafe { CGSessionCopyCurrentDictionary() };
    let found = !session.is_null();
    if found {
        unsafe { CFRelease(session) };
    }
    Discovery {
        built: true,
        found,
        method: "CGSessionCopyCurrentDictionary",
        detail: if found { "a window server session" } else { "no window server session" }.to_string(),
    }
}

#[cfg(all(feature = "presentation", target_os = "windows"))]
pub fn discover() -> Discovery {
    use std::ffi::c_void;
    #[repr(C)]
    struct UserObjectFlags {
        inherit: i32,
        reserved: i32,
        flags: u32,
    }
    #[link(name = "user32")]
    unsafe extern "system" {
        fn GetProcessWindowStation() -> *mut c_void;
        fn GetUserObjectInformationW(handle: *mut c_void, index: i32, info: *mut c_void, length: u32, needed: *mut u32) -> i32;
    }
    const UOI_FLAGS: i32 = 1;
    const WSF_VISIBLE: u32 = 1;
    let mut flags = UserObjectFlags { inherit: 0, reserved: 0, flags: 0 };
    let mut needed = 0_u32;
    // SAFETY: the window station handle needs no closing; the structure is
    // the one the index documents, with its size.
    let ok = unsafe {
        let station = GetProcessWindowStation();
        !station.is_null()
            && GetUserObjectInformationW(
                station,
                UOI_FLAGS,
                (&mut flags as *mut UserObjectFlags).cast(),
                std::mem::size_of::<UserObjectFlags>() as u32,
                &mut needed,
            ) != 0
    };
    let found = ok && flags.flags & WSF_VISIBLE != 0;
    Discovery {
        built: true,
        found,
        method: "GetUserObjectInformationW",
        detail: if !ok {
            "the window station flags could not be read".to_string()
        } else if found {
            "the process window station is visible (interactive)".to_string()
        } else {
            "the process window station is not visible".to_string()
        },
    }
}

#[cfg(all(feature = "presentation", not(any(target_os = "linux", target_os = "macos", target_os = "windows"))))]
pub fn discover() -> Discovery {
    Discovery { built: true, found: false, method: "none", detail: "no discovery on this platform".to_string() }
}

fn is_set(name: &str) -> bool {
    env::var_os(name).is_some_and(|value| !value.is_empty())
}

fn write_set(out: &mut Json, names: &[&str]) {
    out.key("variables_set");
    out.object_start();
    for name in names {
        out.bool(name, is_set(name));
    }
    out.object_end();
}

fn write_entries(out: &mut Json, key: &str, directory: &str) {
    out.key(key);
    out.array_start();
    if let Ok(entries) = std::fs::read_dir(directory) {
        let mut names: Vec<String> = entries.flatten().map(|entry| entry.file_name().to_string_lossy().into_owned()).collect();
        names.sort();
        for name in names {
            out.object_start();
            out.string("name", &name);
            out.object_end();
        }
    }
    out.array_end();
}

/// The facts of this process's environment that say whether it runs inside a
/// desktop session, a container or neither.
pub fn write_session(out: &mut Json) {
    out.key("session");
    out.object_start();
    out.string("os", env::consts::OS);
    if cfg!(target_os = "linux") {
        write_set(out, &["DISPLAY", "WAYLAND_DISPLAY", "XDG_SESSION_TYPE", "XDG_RUNTIME_DIR", "DBUS_SESSION_BUS_ADDRESS"]);
        write_entries(out, "dev_dri", "/dev/dri");
        write_entries(out, "dev_snd", "/dev/snd");
        out.key("container");
        out.object_start();
        out.bool("dockerenv_present", std::path::Path::new("/.dockerenv").exists());
        out.bool("containerenv_present", std::path::Path::new("/run/.containerenv").exists());
        let mentions = |path: &str| {
            std::fs::read_to_string(path)
                .is_ok_and(|text| ["docker", "containerd", "kubepods"].iter().any(|marker| text.contains(marker)))
        };
        out.bool("cgroup_mentions_a_container_runtime", mentions("/proc/1/cgroup"));
        out.bool("mountinfo_mentions_a_container_runtime", mentions("/proc/self/mountinfo"));
        out.object_end();
        out.key("system_libraries");
        match Command::new("ldconfig").arg("-p").output().or_else(|_| Command::new("/sbin/ldconfig").arg("-p").output()) {
            Ok(output) if output.status.success() => {
                let listing = String::from_utf8_lossy(&output.stdout);
                out.object_start();
                for library in ["libX11", "libwayland-client", "libGL", "libEGL", "libvulkan", "libasound", "libpulse"] {
                    out.bool(library, listing.lines().any(|line| line.trim_start().starts_with(&format!("{library}."))));
                }
                out.object_end();
            }
            _ => out.string("system_libraries", "unavailable: ldconfig -p did not run"),
        }
    } else if cfg!(target_os = "macos") {
        write_set(out, &["SECURITYSESSIONID", "XPC_FLAGS", "XPC_SERVICE_NAME", "TERM_SESSION_ID", "SSH_CONNECTION", "SSH_TTY", "DISPLAY"]);
    } else if cfg!(target_os = "windows") {
        write_set(out, &["SESSIONNAME", "USERNAME", "USERPROFILE"]);
        out.bool("console_window_null", console_window_is_null());
    }
    out.object_end();
}

#[cfg(target_os = "windows")]
fn console_window_is_null() -> bool {
    use std::ffi::c_void;
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetConsoleWindow() -> *mut c_void;
    }
    // SAFETY: a plain query with no arguments.
    unsafe { GetConsoleWindow().is_null() }
}

#[cfg(not(target_os = "windows"))]
fn console_window_is_null() -> bool {
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_three_absences_describe_themselves_identically() {
        let not_built = Description::new(false, true, false);
        let discovered_absent = Description::new(true, false, false);
        let requested_absent = Description::new(true, true, true);
        assert_eq!(not_built.json(), discovered_absent.json());
        assert_eq!(not_built.json(), requested_absent.json());
        assert_eq!(not_built.json(), r#"{"presentation":"null","audio_output":"null","input_devices":"null","platform_session":"none"}"#);
        let present = Description::new(true, true, false);
        assert_ne!(present.json(), not_built.json());
        assert_eq!(present.presentation, "windowed");
        assert_eq!(present.platform_session, "interactive");
    }

    #[test]
    fn discovery_reports_whether_the_capability_is_built() {
        let discovery = discover();
        assert_eq!(discovery.built, cfg!(feature = "presentation"));
        if !discovery.built {
            assert!(!discovery.found);
        }
        let mut out = Json::new();
        out.object_start();
        discovery.write(&mut out);
        write_session(&mut out);
        out.object_end();
        let text = out.finish();
        assert!(text.contains("\"session\":{\"os\":\""), "{text}");
    }
}
