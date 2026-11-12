//! Metal's validation layer as a program can observe it. The layer wraps the
//! devices of a process started with `MTL_DEBUG_LAYER=1` and has no callback:
//! told to log instead of stopping the process, it writes its messages to the
//! standard error. This module points the standard error at a file, reads
//! what was appended since the last read, and commits one misuse the layer
//! reports. Throwaway, never product code.

use std::ffi::c_int;
use std::fs::{self, File};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::os::fd::{AsRawFd, FromRawFd};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use super::gpu::{self, Gpu};
use super::objc::{Id, present};

unsafe extern "C" {
    fn dup(descriptor: c_int) -> c_int;
    fn dup2(descriptor: c_int, replaced: c_int) -> c_int;
}

const STANDARD_ERROR: c_int = 2;
/// The length of the buffer the misuse binds, and the offset it binds it at.
const MISUSED_BUFFER_LENGTH: usize = 16;
const MISUSED_OFFSET: usize = 64;

/// The standard error of the process, pointed at a file for as long as the
/// value lives. What is read from the file is also written to the original
/// standard error, so that nothing is lost to whoever started the process.
pub struct StandardErrorCapture {
    path: PathBuf,
    original: File,
    consumed: u64,
}

impl StandardErrorCapture {
    pub fn start(work: &Path) -> Result<StandardErrorCapture, String> {
        fs::create_dir_all(work).map_err(|error| format!("{}: {error}", work.display()))?;
        let moment = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |since| since.as_nanos());
        let path = work.join(format!("metal-validation-{}-{moment}.log", std::process::id()));
        let file = File::create(&path).map_err(|error| format!("{}: {error}", path.display()))?;
        // SAFETY: `dup` takes any descriptor number and returns a new one or -1.
        let saved = unsafe { dup(STANDARD_ERROR) };
        if saved < 0 {
            return Err(format!("dup of the standard error: {}", io::Error::last_os_error()));
        }
        // SAFETY: the descriptor was just returned by `dup` and nothing else owns it.
        let original = unsafe { File::from_raw_fd(saved) };
        // SAFETY: both descriptors are open; `dup2` makes the second one refer to the file.
        if unsafe { dup2(file.as_raw_fd(), STANDARD_ERROR) } < 0 {
            return Err(format!("dup2 of {} over the standard error: {}", path.display(), io::Error::last_os_error()));
        }
        Ok(StandardErrorCapture { path, original, consumed: 0 })
    }

    /// The messages written to the standard error since the last call.
    pub fn drain(&mut self) -> Vec<String> {
        let mut appended = Vec::new();
        let read = File::open(&self.path).and_then(|mut file| {
            file.seek(SeekFrom::Start(self.consumed))?;
            file.read_to_end(&mut appended)
        });
        match read {
            Ok(count) => {
                self.consumed += count as u64;
                let _ = self.original.write_all(&appended);
                split_messages(&String::from_utf8_lossy(&appended))
            }
            Err(error) => {
                let _ = writeln!(self.original, "the captured standard error {} could not be read: {error}", self.path.display());
                Vec::new()
            }
        }
    }
}

impl Drop for StandardErrorCapture {
    fn drop(&mut self) {
        let _ = self.drain();
        // SAFETY: both descriptors are open; the standard error refers to
        // what it referred to before the capture again.
        unsafe { dup2(self.original.as_raw_fd(), STANDARD_ERROR) };
        let _ = fs::remove_file(&self.path);
    }
}

/// Whether a line starts as the system's log lines do: `2026-11-12 09:41:07`.
fn starts_with_a_date(line: &str) -> bool {
    let bytes = line.as_bytes();
    bytes.len() >= 19 && bytes[..4].iter().all(u8::is_ascii_digit) && bytes[4] == b'-' && bytes[7] == b'-' && bytes[10] == b' ' && bytes[13] == b':' && bytes[16] == b':'
}

/// The messages of a captured text: a message starts on a line that begins
/// with a date and takes the lines that follow it, since the layer's
/// messages run over several lines.
pub fn split_messages(text: &str) -> Vec<String> {
    let mut messages: Vec<String> = Vec::new();
    for line in text.lines().map(str::trim).filter(|line| !line.is_empty()) {
        match messages.last_mut() {
            Some(message) if !starts_with_a_date(line) => {
                message.push(' ');
                message.push_str(line);
            }
            _ => messages.push(line.to_string()),
        }
    }
    messages
}

/// How the layer's messages reach the program, with the class of the device
/// object as the evidence of what stands behind it.
pub fn delivery(device_class: &str, capturing: bool) -> String {
    if capturing {
        format!(
            "Metal's validation layer has no callback. Started with MTL_DEBUG_LAYER=1 and told by MTL_DEBUG_LAYER_ERROR_MODE=nslog and MTL_DEBUG_LAYER_WARNING_MODE=nslog to log, it writes each message to the standard error of the process; the spike points the standard error at a file with dup2 and reads what was appended since its last read. The device object is of class {device_class}."
        )
    } else {
        format!("The device was opened without validation: nothing is collected. The device object is of class {device_class}.")
    }
}

/// Binds a buffer to a compute encoder at an offset past its end, with no
/// dispatch after it: the layer reports the offset, and the device is left
/// with a binding nothing reads.
pub fn commit_misuse(gpu: &Gpu) -> Result<String, String> {
    let buffer = gpu.empty_buffer("the misused buffer", MISUSED_BUFFER_LENGTH)?;
    let command_buffer = gpu.command_buffer("the misuse")?;
    let encoder = present(send!(Id; command_buffer, "computeCommandEncoder")?, "computeCommandEncoder for the misuse")?;
    let bound = send!((); encoder, "setBuffer:offset:atIndex:", Id: buffer.id(), usize: MISUSED_OFFSET, usize: 0);
    gpu::end_encoding(encoder, bound)?;
    gpu::complete(command_buffer, "the misuse")?;
    Ok(format!(
        "setBuffer:offset:atIndex: of a compute command encoder was given the offset {MISUSED_OFFSET} into a buffer of {MISUSED_BUFFER_LENGTH} bytes, with no dispatch after it; the encoder was ended and its command buffer completed"
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_message_starts_at_a_dated_line_and_takes_the_lines_after_it() {
        let text = "2026-11-12 09:41:07.123 spike[1:2] -[MTLDebugComputeCommandEncoder setBuffer:offset:atIndex:]:1: failed assertion `Set Buffer Validation\noffset(64) must be < [buffer length](16).\n'\n\n2026-11-12 09:41:07.200 spike[1:2] second\n";
        let messages = split_messages(text);
        assert_eq!(messages.len(), 2, "{messages:?}");
        assert!(messages[0].ends_with("offset(64) must be < [buffer length](16). '"), "{}", messages[0]);
        assert!(messages[1].ends_with("second"));
        assert_eq!(split_messages("no date here\nnor here\n"), vec!["no date here nor here".to_string()]);
        assert!(split_messages("\n  \n").is_empty());
    }

    #[test]
    fn the_delivery_names_the_class_of_the_device_object() {
        assert!(delivery("MTLDebugDevice", true).contains("class MTLDebugDevice"));
        assert!(delivery("AGXG13GDevice", false).contains("without validation"));
    }
}
