//! Run-time loading of the platform's graphics libraries. Throwaway, never
//! product code.
//!
//! Every graphics API of the spike is reached through the system loader at run
//! time (`dlopen` on Unix, `LoadLibraryW` on Windows), never linked: the binary
//! imports no graphics library, a device's absence is a reported fact instead
//! of a failure to start, and the import check of the headless research applies
//! to this binary as it did to the headless one.

use std::ffi::{CString, c_void};
#[cfg(windows)]
use std::ffi::c_char;

pub struct Library {
    handle: *mut c_void,
    pub name: String,
}

#[cfg(unix)]
mod sys {
    use std::ffi::{c_char, c_int, c_void};

    #[cfg_attr(target_os = "linux", link(name = "dl"))]
    unsafe extern "C" {
        pub fn dlopen(file: *const c_char, flags: c_int) -> *mut c_void;
        pub fn dlsym(handle: *mut c_void, name: *const c_char) -> *mut c_void;
        pub fn dlclose(handle: *mut c_void) -> c_int;
        pub fn dlerror() -> *const c_char;
    }

    pub const RTLD_NOW: c_int = 2;
}

#[cfg(windows)]
mod sys {
    use std::ffi::{c_char, c_void};

    #[link(name = "kernel32")]
    unsafe extern "system" {
        pub fn LoadLibraryW(name: *const u16) -> *mut c_void;
        pub fn GetProcAddress(module: *mut c_void, name: *const c_char) -> *mut c_void;
        pub fn FreeLibrary(module: *mut c_void) -> i32;
        pub fn GetLastError() -> u32;
    }
}

impl Library {
    /// Loads the first of `names` the loader finds, or reports every attempt.
    pub fn open(names: &[&str]) -> Result<Library, String> {
        let mut errors = Vec::new();
        for name in names {
            match Library::open_one(name) {
                Ok(library) => return Ok(library),
                Err(error) => errors.push(format!("{name}: {error}")),
            }
        }
        Err(format!("no library found; tried {}", errors.join("; ")))
    }

    #[cfg(unix)]
    fn open_one(name: &str) -> Result<Library, String> {
        let c_name = CString::new(name).map_err(|e| e.to_string())?;
        let handle = unsafe { sys::dlopen(c_name.as_ptr(), sys::RTLD_NOW) };
        if handle.is_null() {
            let error = unsafe { sys::dlerror() };
            let text = if error.is_null() { "dlopen failed".to_string() } else { unsafe { std::ffi::CStr::from_ptr(error) }.to_string_lossy().into_owned() };
            // dyld lists every path it tried; the first clause says what matters.
            let short = text.split(", tried:").next().unwrap_or(&text).to_string();
            return Err(short);
        }
        Ok(Library { handle, name: name.to_string() })
    }

    #[cfg(windows)]
    fn open_one(name: &str) -> Result<Library, String> {
        let wide: Vec<u16> = name.encode_utf16().chain(std::iter::once(0)).collect();
        let handle = unsafe { sys::LoadLibraryW(wide.as_ptr()) };
        if handle.is_null() {
            return Err(format!("LoadLibraryW failed with error {}", unsafe { sys::GetLastError() }));
        }
        Ok(Library { handle, name: name.to_string() })
    }

    /// The address of an exported symbol, as a raw pointer.
    pub fn symbol(&self, name: &str) -> Result<*mut c_void, String> {
        let c_name = CString::new(name).map_err(|e| e.to_string())?;
        #[cfg(unix)]
        let address = unsafe { sys::dlsym(self.handle, c_name.as_ptr()) };
        #[cfg(windows)]
        let address = unsafe { sys::GetProcAddress(self.handle, c_name.as_ptr() as *const c_char) };
        if address.is_null() { Err(format!("{name} not exported by {}", self.name)) } else { Ok(address) }
    }

    /// The exported symbol `name` as a function pointer of type `F`.
    ///
    /// # Safety
    ///
    /// `F` must be the function pointer type the library exports under `name`.
    pub unsafe fn function<F: Copy>(&self, name: &str) -> Result<F, String> {
        assert_eq!(std::mem::size_of::<F>(), std::mem::size_of::<*mut c_void>(), "a function pointer type");
        let address = self.symbol(name)?;
        // SAFETY: the caller states that `F` is the exported function's type.
        Ok(unsafe { std::mem::transmute_copy::<*mut c_void, F>(&address) })
    }
}

impl Drop for Library {
    fn drop(&mut self) {
        #[cfg(unix)]
        unsafe {
            sys::dlclose(self.handle);
        }
        #[cfg(windows)]
        unsafe {
            sys::FreeLibrary(self.handle);
        }
    }
}

/// A NUL-terminated copy of a string for the platform APIs.
pub fn c_string(text: &str) -> CString {
    CString::new(text).expect("no interior NUL")
}

/// The text of a NUL-terminated buffer the platform filled.
pub fn text_of(bytes: &[u8]) -> String {
    let end = bytes.iter().position(|&b| b == 0).unwrap_or(bytes.len());
    String::from_utf8_lossy(&bytes[..end]).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_missing_library_is_reported_with_every_name_tried() {
        let error = match Library::open(&["libludenea-does-not-exist.so.1", "ludenea-does-not-exist.dll"]) {
            Ok(_) => panic!("a library that does not exist opened"),
            Err(error) => error,
        };
        assert!(error.contains("does-not-exist.so.1") && error.contains("does-not-exist.dll"), "{error}");
    }

    #[test]
    fn text_of_stops_at_the_first_nul() {
        assert_eq!(text_of(b"llvmpipe\0garbage"), "llvmpipe");
        assert_eq!(text_of(b"no nul"), "no nul");
    }
}
