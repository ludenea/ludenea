//! Loading a dynamic library and resolving one symbol, through the platform's
//! loader reached directly (no loading crate). Throwaway, never product code.

use std::ffi::CString;
use std::path::Path;

#[cfg(unix)]
mod sys {
    use std::ffi::{CStr, c_char, c_int, c_void};
    use std::path::Path;

    #[cfg_attr(target_os = "linux", link(name = "dl"))]
    unsafe extern "C" {
        fn dlopen(file: *const c_char, flags: c_int) -> *mut c_void;
        fn dlsym(handle: *mut c_void, name: *const c_char) -> *mut c_void;
        fn dlclose(handle: *mut c_void) -> c_int;
        fn dlerror() -> *const c_char;
    }

    const RTLD_NOW: c_int = 2;

    fn last_error() -> String {
        let text = unsafe { dlerror() };
        if text.is_null() { "unknown loader error".to_string() } else { unsafe { CStr::from_ptr(text) }.to_string_lossy().into_owned() }
    }

    pub fn open(path: &Path) -> Result<usize, String> {
        let path = super::c_string(&path.to_string_lossy())?;
        let handle = unsafe { dlopen(path.as_ptr(), RTLD_NOW) };
        if handle.is_null() { Err(last_error()) } else { Ok(handle as usize) }
    }

    pub fn symbol(handle: usize, name: &str) -> Result<usize, String> {
        let name = super::c_string(name)?;
        let address = unsafe { dlsym(handle as *mut c_void, name.as_ptr()) };
        if address.is_null() { Err(last_error()) } else { Ok(address as usize) }
    }

    pub fn close(handle: usize) -> Result<(), String> {
        if unsafe { dlclose(handle as *mut c_void) } == 0 { Ok(()) } else { Err(last_error()) }
    }
}

#[cfg(windows)]
mod sys {
    use std::ffi::{c_char, c_void};
    use std::path::Path;

    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn LoadLibraryW(name: *const u16) -> *mut c_void;
        fn GetProcAddress(module: *mut c_void, name: *const c_char) -> *mut c_void;
        fn FreeLibrary(module: *mut c_void) -> i32;
        fn GetLastError() -> u32;
    }

    fn last_error() -> String {
        format!("error {}", unsafe { GetLastError() })
    }

    pub fn open(path: &Path) -> Result<usize, String> {
        let wide: Vec<u16> = path.to_string_lossy().encode_utf16().chain(std::iter::once(0)).collect();
        let handle = unsafe { LoadLibraryW(wide.as_ptr()) };
        if handle.is_null() { Err(last_error()) } else { Ok(handle as usize) }
    }

    pub fn symbol(handle: usize, name: &str) -> Result<usize, String> {
        let name = super::c_string(name)?;
        let address = unsafe { GetProcAddress(handle as *mut c_void, name.as_ptr()) };
        if address.is_null() { Err(last_error()) } else { Ok(address as usize) }
    }

    pub fn close(handle: usize) -> Result<(), String> {
        if unsafe { FreeLibrary(handle as *mut c_void) } != 0 { Ok(()) } else { Err(last_error()) }
    }
}

fn c_string(text: &str) -> Result<CString, String> {
    CString::new(text).map_err(|_| format!("{text:?} holds an interior nul"))
}

/// Loads the library at `path` and returns the loader's handle.
pub fn open(path: &Path) -> Result<usize, String> {
    sys::open(path).map_err(|error| format!("loading {}: {error}", path.display()))
}

/// The address of `name` in the loaded library.
pub fn symbol(handle: usize, name: &str) -> Result<usize, String> {
    sys::symbol(handle, name).map_err(|error| format!("resolving {name}: {error}"))
}

/// Unloads the library (the loader may keep it mapped while other references exist).
pub fn close(handle: usize) -> Result<(), String> {
    sys::close(handle)
}
