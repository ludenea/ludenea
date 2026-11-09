//! A shared memory mapping and its hand-over between two endpoints.
//! Throwaway, never product code.
//!
//! The platform calls are declared by hand, the standard library having none:
//!
//! - Linux: an anonymous memory file (`memfd_create`), mapped with `mmap`;
//!   its descriptor travels over a Unix domain socket as ancillary data
//!   (`sendmsg` with `SCM_RIGHTS`).
//! - macOS: a POSIX shared memory object (`shm_open`) unlinked as soon as it
//!   is created, so that only descriptors name it; mapped and handed over as
//!   on Linux, with that system's structure layouts and constants.
//! - Windows: a named file mapping section backed by the paging file
//!   (`CreateFileMappingW`, `MapViewOfFile`); its name travels as text and the
//!   peer opens it with `OpenFileMappingW`.
//!
//! Every layout below is the 64-bit one; the spike builds for x86_64 and
//! ARM64 only.

use std::io;

/// One view of a shared memory object. Unmapped, and its descriptor or handle
/// closed, when dropped; the object lives as long as one view of it does.
pub struct Mapping {
    pointer: *mut u8,
    length: usize,
    owner: platform::Owner,
}

// SAFETY: the mapping is plain memory; what is read and written through it,
// and from which thread, is the ring's contract.
unsafe impl Send for Mapping {}
unsafe impl Sync for Mapping {}

impl Mapping {
    /// Creates a zero-filled shared memory object of `length` bytes and maps it.
    pub fn create(length: usize) -> io::Result<Mapping> {
        platform::create(length)
    }

    /// A second view of the same object, as the peer gets one: by name on
    /// Windows, through a duplicate of the descriptor elsewhere.
    #[cfg(test)]
    pub fn second_view(&self) -> io::Result<Mapping> {
        platform::second_view(self)
    }

    pub fn pointer(&self) -> *mut u8 {
        self.pointer
    }

    pub fn length(&self) -> usize {
        self.length
    }
}

impl Drop for Mapping {
    fn drop(&mut self) {
        platform::unmap(self);
    }
}

#[cfg(windows)]
pub use platform::open_named;
#[cfg(unix)]
pub use platform::{duplicate, from_descriptor, receive_descriptor, send_descriptor};

impl Mapping {
    /// The name the peer opens the object by; empty where a descriptor is
    /// handed over instead.
    pub fn name(&self) -> &str {
        platform::name(self)
    }
}

#[cfg(windows)]
mod platform {
    use super::Mapping;
    use std::ffi::c_void;
    use std::io;
    use std::sync::atomic::{AtomicU64, Ordering};

    pub struct Owner {
        handle: *mut c_void,
        name: String,
    }

    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn CreateFileMappingW(file: *mut c_void, attributes: *const c_void, protection: u32, size_high: u32, size_low: u32, name: *const u16) -> *mut c_void;
        fn OpenFileMappingW(access: u32, inherit: i32, name: *const u16) -> *mut c_void;
        fn MapViewOfFile(mapping: *mut c_void, access: u32, offset_high: u32, offset_low: u32, bytes: usize) -> *mut c_void;
        fn UnmapViewOfFile(address: *const c_void) -> i32;
        fn CloseHandle(handle: *mut c_void) -> i32;
    }

    const INVALID_HANDLE_VALUE: *mut c_void = -1_isize as *mut c_void;
    const PAGE_READWRITE: u32 = 0x04;
    const FILE_MAP_READ_WRITE: u32 = 0x0002 | 0x0004;

    static NEXT: AtomicU64 = AtomicU64::new(0);

    fn wide(text: &str) -> Vec<u16> {
        text.encode_utf16().chain([0]).collect()
    }

    fn view(handle: *mut c_void, length: usize, name: String) -> io::Result<Mapping> {
        // SAFETY: a valid section handle; the view covers `length` bytes from its start.
        let pointer = unsafe { MapViewOfFile(handle, FILE_MAP_READ_WRITE, 0, 0, length) };
        if pointer.is_null() {
            let error = io::Error::last_os_error();
            // SAFETY: the handle is ours and not used again.
            unsafe { CloseHandle(handle) };
            return Err(error);
        }
        Ok(Mapping { pointer: pointer.cast(), length, owner: Owner { handle, name } })
    }

    pub fn create(length: usize) -> io::Result<Mapping> {
        let name = format!("Local\\ludenea-data-plane-{}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed));
        let wide_name = wide(&name);
        let size = length as u64;
        // SAFETY: the name is a null-terminated UTF-16 string that outlives the
        // call; the invalid file handle asks for a section backed by the paging file.
        let handle = unsafe { CreateFileMappingW(INVALID_HANDLE_VALUE, std::ptr::null(), PAGE_READWRITE, (size >> 32) as u32, size as u32, wide_name.as_ptr()) };
        if handle.is_null() {
            return Err(io::Error::last_os_error());
        }
        view(handle, length, name)
    }

    /// Opens the section a peer created, by the name it sent.
    pub fn open_named(name: &str, length: usize) -> io::Result<Mapping> {
        let wide_name = wide(name);
        // SAFETY: the name is a null-terminated UTF-16 string that outlives the call.
        let handle = unsafe { OpenFileMappingW(FILE_MAP_READ_WRITE, 0, wide_name.as_ptr()) };
        if handle.is_null() {
            return Err(io::Error::last_os_error());
        }
        view(handle, length, name.to_string())
    }

    #[cfg(test)]
    pub fn second_view(mapping: &Mapping) -> io::Result<Mapping> {
        open_named(&mapping.owner.name, mapping.length)
    }

    pub fn name(mapping: &Mapping) -> &str {
        &mapping.owner.name
    }

    pub fn unmap(mapping: &mut Mapping) {
        // SAFETY: the view and the handle are ours and not used again.
        unsafe {
            UnmapViewOfFile(mapping.pointer.cast());
            CloseHandle(mapping.owner.handle);
        }
    }
}

#[cfg(unix)]
mod platform {
    use super::Mapping;
    use std::ffi::{c_char, c_int, c_void};
    use std::fs::File;
    use std::io;
    use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
    use std::os::unix::net::UnixStream;

    pub struct Owner {
        file: File,
    }

    unsafe extern "C" {
        fn mmap(address: *mut c_void, length: usize, protection: c_int, flags: c_int, descriptor: c_int, offset: i64) -> *mut c_void;
        fn munmap(address: *mut c_void, length: usize) -> c_int;
        fn sendmsg(socket: c_int, message: *const MessageHeader, flags: c_int) -> isize;
        fn recvmsg(socket: c_int, message: *mut MessageHeader, flags: c_int) -> isize;
    }

    // The same values on Linux and macOS.
    const PROT_READ: c_int = 1;
    const PROT_WRITE: c_int = 2;
    const MAP_SHARED: c_int = 1;
    const MAP_FAILED: *mut c_void = !0_usize as *mut c_void;
    const SCM_RIGHTS: c_int = 1;

    #[cfg(target_os = "linux")]
    const SOL_SOCKET: c_int = 1;
    #[cfg(not(target_os = "linux"))]
    const SOL_SOCKET: c_int = 0xffff;

    /// `MSG_NOSIGNAL` where it exists; elsewhere the runtime ignores the
    /// broken pipe signal and the socket reports the error.
    #[cfg(target_os = "linux")]
    const SEND_FLAGS: c_int = 0x4000;
    #[cfg(not(target_os = "linux"))]
    const SEND_FLAGS: c_int = 0;

    #[repr(C)]
    struct IoVector {
        base: *mut c_void,
        length: usize,
    }

    /// `struct msghdr`, 64-bit Linux (glibc and musl agree on little-endian
    /// targets): the two lengths are `size_t`.
    #[cfg(target_os = "linux")]
    #[repr(C)]
    struct MessageHeader {
        name: *mut c_void,
        name_length: u32,
        vectors: *mut IoVector,
        vector_count: usize,
        control: *mut c_void,
        control_length: usize,
        flags: c_int,
    }

    /// `struct msghdr`, 64-bit macOS and the BSDs: the vector count is an
    /// `int` and the control length a `socklen_t`.
    #[cfg(not(target_os = "linux"))]
    #[repr(C)]
    struct MessageHeader {
        name: *mut c_void,
        name_length: u32,
        vectors: *mut IoVector,
        vector_count: c_int,
        control: *mut c_void,
        control_length: u32,
        flags: c_int,
    }

    /// One control message carrying one descriptor: `struct cmsghdr` followed
    /// by an `int`. On 64-bit Linux the length is a `size_t` and the data is
    /// aligned to eight bytes: 16 bytes of header, 20 of message, 24 of space.
    #[cfg(target_os = "linux")]
    #[repr(C)]
    struct DescriptorMessage {
        length: usize,
        level: c_int,
        kind: c_int,
        descriptor: c_int,
        padding: c_int,
    }

    #[cfg(target_os = "linux")]
    const DESCRIPTOR_MESSAGE_LENGTH: usize = 20;

    /// On macOS and the BSDs the length is a `socklen_t` and the data is
    /// aligned to four bytes: 12 bytes of header, 16 of message and of space.
    #[cfg(not(target_os = "linux"))]
    #[repr(C)]
    struct DescriptorMessage {
        length: u32,
        level: c_int,
        kind: c_int,
        descriptor: c_int,
    }

    #[cfg(not(target_os = "linux"))]
    const DESCRIPTOR_MESSAGE_LENGTH: u32 = 16;

    fn map(file: File, length: usize) -> io::Result<Mapping> {
        // SAFETY: a fresh shared mapping of `length` bytes of the descriptor,
        // at an address the system chooses.
        let pointer = unsafe { mmap(std::ptr::null_mut(), length, PROT_READ | PROT_WRITE, MAP_SHARED, file.as_raw_fd(), 0) };
        if pointer == MAP_FAILED {
            return Err(io::Error::last_os_error());
        }
        Ok(Mapping { pointer: pointer.cast(), length, owner: Owner { file } })
    }

    #[cfg(target_os = "linux")]
    fn create_object() -> io::Result<File> {
        unsafe extern "C" {
            fn memfd_create(name: *const c_char, flags: u32) -> c_int;
        }
        const MFD_CLOEXEC: u32 = 1;
        // SAFETY: the name is a null-terminated string that outlives the call.
        let descriptor = unsafe { memfd_create(c"ludenea-data-plane".as_ptr(), MFD_CLOEXEC) };
        if descriptor < 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: a descriptor the call just returned, owned by nothing else.
        Ok(unsafe { File::from_raw_fd(descriptor) })
    }

    #[cfg(target_os = "macos")]
    fn create_object() -> io::Result<File> {
        use std::sync::atomic::{AtomicU32, Ordering};
        unsafe extern "C" {
            // Variadic in the system's header: the mode must be passed as a
            // variadic argument, which ARM64 macOS passes on the stack.
            fn shm_open(name: *const c_char, flags: c_int, ...) -> c_int;
            fn shm_unlink(name: *const c_char) -> c_int;
        }
        const O_RDWR: c_int = 0x0002;
        const O_CREAT: c_int = 0x0200;
        const O_EXCL: c_int = 0x0800;
        static NEXT: AtomicU32 = AtomicU32::new(0);
        // At most 31 bytes: the system's limit on a shared memory name.
        let name = format!("/ludenea-{}-{}\0", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed));
        let name: *const c_char = name.as_ptr().cast();
        // SAFETY: the name is a null-terminated string that outlives both calls.
        let descriptor = unsafe { shm_open(name, O_RDWR | O_CREAT | O_EXCL, 0o600 as c_int) };
        if descriptor < 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: as above; the object stays alive through its descriptor.
        unsafe { shm_unlink(name) };
        // SAFETY: a descriptor the call just returned, owned by nothing else.
        Ok(unsafe { File::from_raw_fd(descriptor) })
    }

    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    fn create_object() -> io::Result<File> {
        Err(io::Error::new(io::ErrorKind::Unsupported, "no shared memory object on this system in this spike"))
    }

    pub fn create(length: usize) -> io::Result<Mapping> {
        let file = create_object()?;
        file.set_len(length as u64)?;
        map(file, length)
    }

    /// Maps the object a peer created, from the descriptor it handed over.
    pub fn from_descriptor(descriptor: OwnedFd, length: usize) -> io::Result<Mapping> {
        map(File::from(descriptor), length)
    }

    #[cfg(test)]
    pub fn second_view(mapping: &Mapping) -> io::Result<Mapping> {
        map(mapping.owner.file.try_clone()?, mapping.length)
    }

    pub fn name(_mapping: &Mapping) -> &str {
        ""
    }

    /// A descriptor of the object of its own, to hand over to a peer.
    pub fn duplicate(mapping: &Mapping) -> io::Result<OwnedFd> {
        Ok(OwnedFd::from(mapping.owner.file.try_clone()?))
    }

    pub fn unmap(mapping: &mut Mapping) {
        // SAFETY: the mapping is ours and not used again; the file closes itself.
        unsafe { munmap(mapping.pointer.cast(), mapping.length) };
    }

    /// Sends one byte with `descriptor` as ancillary data: the receiver gets
    /// its own descriptor of the same object.
    pub fn send_descriptor(socket: &UnixStream, descriptor: &OwnedFd) -> io::Result<()> {
        let mut byte = [0x5a_u8];
        let mut vector = IoVector { base: byte.as_mut_ptr().cast(), length: 1 };
        let mut control = DescriptorMessage {
            length: DESCRIPTOR_MESSAGE_LENGTH,
            level: SOL_SOCKET,
            kind: SCM_RIGHTS,
            descriptor: descriptor.as_raw_fd(),
            #[cfg(target_os = "linux")]
            padding: 0,
        };
        // SAFETY: an all-zero message header is valid (null pointers, zero lengths).
        let mut header: MessageHeader = unsafe { std::mem::zeroed() };
        header.vectors = &mut vector;
        header.vector_count = 1;
        header.control = (&raw mut control).cast();
        header.control_length = size_of::<DescriptorMessage>() as _;
        loop {
            // SAFETY: the header points at one vector of one byte and one
            // control message, all alive for the call.
            let sent = unsafe { sendmsg(socket.as_raw_fd(), &header, SEND_FLAGS) };
            if sent == 1 {
                return Ok(());
            }
            let error = if sent < 0 { io::Error::last_os_error() } else { io::Error::new(io::ErrorKind::WriteZero, "the descriptor's byte was not sent") };
            if error.kind() != io::ErrorKind::Interrupted {
                return Err(error);
            }
        }
    }

    /// Receives the byte and the descriptor [`send_descriptor`] sent.
    pub fn receive_descriptor(socket: &UnixStream) -> io::Result<OwnedFd> {
        let mut byte = [0_u8];
        let mut vector = IoVector { base: byte.as_mut_ptr().cast(), length: 1 };
        // SAFETY: an all-zero control message and message header are valid.
        let mut control: DescriptorMessage = unsafe { std::mem::zeroed() };
        let mut header: MessageHeader = unsafe { std::mem::zeroed() };
        header.vectors = &mut vector;
        header.vector_count = 1;
        header.control = (&raw mut control).cast();
        header.control_length = size_of::<DescriptorMessage>() as _;
        loop {
            // SAFETY: the header points at one vector of one byte and room for
            // one control message, all alive for the call.
            let received = unsafe { recvmsg(socket.as_raw_fd(), &mut header, 0) };
            if received == 1 {
                break;
            }
            let error = if received < 0 { io::Error::last_os_error() } else { io::Error::new(io::ErrorKind::UnexpectedEof, "the connection closed before the descriptor") };
            if error.kind() != io::ErrorKind::Interrupted {
                return Err(error);
            }
        }
        let carried = (header.control_length as usize) >= DESCRIPTOR_MESSAGE_LENGTH as usize
            && control.length as usize == DESCRIPTOR_MESSAGE_LENGTH as usize
            && control.level == SOL_SOCKET
            && control.kind == SCM_RIGHTS;
        if !carried || byte[0] != 0x5a {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "the message carried no descriptor"));
        }
        // SAFETY: the system installed this descriptor for us; nothing else owns it.
        Ok(unsafe { OwnedFd::from_raw_fd(control.descriptor) })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn two_views_of_one_object_share_their_bytes() {
        let first = Mapping::create(8192).unwrap();
        let second = first.second_view().unwrap();
        assert_eq!((first.length(), second.length()), (8192, 8192));
        assert_ne!(first.pointer(), second.pointer());
        // SAFETY: both views cover 8192 bytes; nothing else uses them.
        unsafe {
            assert_eq!(*second.pointer().add(4097), 0, "a new object is zero-filled");
            *first.pointer().add(4097) = 0xab;
            assert_eq!(*second.pointer().add(4097), 0xab);
            *second.pointer().add(8191) = 0xcd;
            assert_eq!(*first.pointer().add(8191), 0xcd);
        }
        drop(first);
        // SAFETY: the object lives while one view does.
        unsafe { assert_eq!(*second.pointer().add(4097), 0xab) };
    }

    #[cfg(unix)]
    #[test]
    fn a_descriptor_travels_over_a_unix_domain_socket() {
        use std::os::unix::net::UnixStream;
        let (left, right) = UnixStream::pair().unwrap();
        let created = Mapping::create(4096).unwrap();
        // SAFETY: inside the mapping.
        unsafe { *created.pointer().add(100) = 0x77 };
        send_descriptor(&left, &duplicate(&created).unwrap()).unwrap();
        let received = from_descriptor(receive_descriptor(&right).unwrap(), 4096).unwrap();
        // SAFETY: inside both mappings of the same object.
        unsafe {
            assert_eq!(*received.pointer().add(100), 0x77);
            *received.pointer().add(101) = 0x78;
            assert_eq!(*created.pointer().add(101), 0x78);
        }
    }
}
