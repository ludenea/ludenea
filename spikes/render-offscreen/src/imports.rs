//! The dynamic imports of an executable, read from the file: a copy of the
//! headless modes spike's reader. Throwaway, never product code.
//!
//! Applied here to the rendering binary itself: since every graphics API is
//! loaded at run time, the binary that renders imports no windowing, graphics
//! or audio library either, which is the fact the check records. Three
//! formats, the fields the question needs and nothing else: ELF64 (the
//! `DT_NEEDED` entries of the dynamic section), Mach-O 64 (the dylib load
//! commands) and PE32+ (the import and delay-load directories).

use std::fs;
use std::path::Path;

/// The libraries an executable imports, by name as the file states them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Imports {
    pub format: &'static str,
    pub libraries: Vec<String>,
}

/// Windowing, graphics and audio libraries per format, matched as
/// case-insensitive substrings of the imported names.
const DENIED_ELF: &[&str] = &[
    "libx11", "libxcb", "libwayland", "libgl", "libegl", "libvulkan", "libasound", "libpulse", "libpipewire", "libjack",
    "libdrm", "libgbm", "libsdl",
];
const DENIED_MACHO: &[&str] = &[
    "appkit", "cocoa", "metal", "quartzcore", "coregraphics", "coreaudio", "audiotoolbox", "opengl", "iosurface", "corevideo",
    "gamecontroller",
];
const DENIED_PE: &[&str] =
    &["user32", "gdi32", "d3d", "dxgi", "opengl32", "winmm", "xaudio", "dsound", "mmdevapi", "xinput", "dinput"];

impl Imports {
    pub fn of_file(path: &Path) -> Result<Imports, String> {
        let bytes = fs::read(path).map_err(|error| format!("{}: {error}", path.display()))?;
        Imports::parse(&bytes)
    }

    pub fn parse(bytes: &[u8]) -> Result<Imports, String> {
        match bytes.get(0..4) {
            Some([0x7f, b'E', b'L', b'F']) => elf64(bytes),
            Some([0xcf, 0xfa, 0xed, 0xfe]) => macho64(bytes),
            Some([0xca, 0xfe, 0xba, 0xbe]) | Some([0xbe, 0xba, 0xfe, 0xca]) => Err("fat binary: one architecture per file expected".to_string()),
            Some([b'M', b'Z', _, _]) => pe32plus(bytes),
            _ => Err("not an ELF, Mach-O or PE file".to_string()),
        }
    }

    /// The imported names that name a windowing, graphics or audio library.
    pub fn denied(&self) -> Vec<String> {
        let list = match self.format {
            "elf64" => DENIED_ELF,
            "macho64" => DENIED_MACHO,
            _ => DENIED_PE,
        };
        self.libraries
            .iter()
            .filter(|name| {
                let lower = name.to_ascii_lowercase();
                list.iter().any(|denied| lower.contains(denied))
            })
            .cloned()
            .collect()
    }
}

struct Bytes<'a>(&'a [u8]);

impl Bytes<'_> {
    fn u16(&self, at: usize) -> Result<u16, String> {
        Ok(u16::from_le_bytes(self.slice(at, 2)?.try_into().expect("two bytes")))
    }

    fn u32(&self, at: usize) -> Result<u32, String> {
        Ok(u32::from_le_bytes(self.slice(at, 4)?.try_into().expect("four bytes")))
    }

    fn u64(&self, at: usize) -> Result<u64, String> {
        Ok(u64::from_le_bytes(self.slice(at, 8)?.try_into().expect("eight bytes")))
    }

    fn slice(&self, at: usize, len: usize) -> Result<&[u8], String> {
        at.checked_add(len).and_then(|end| self.0.get(at..end)).ok_or_else(|| format!("truncated at byte {at}"))
    }

    /// The NUL-terminated string at `at`.
    fn c_string(&self, at: usize) -> Result<String, String> {
        let rest = self.0.get(at..).ok_or_else(|| format!("string offset {at} out of the file"))?;
        let end = rest.iter().position(|&b| b == 0).ok_or_else(|| format!("unterminated string at {at}"))?;
        Ok(String::from_utf8_lossy(&rest[..end]).into_owned())
    }
}

fn elf64(bytes: &[u8]) -> Result<Imports, String> {
    let file = Bytes(bytes);
    if bytes.get(4) != Some(&2) || bytes.get(5) != Some(&1) {
        return Err("ELF: not a little-endian 64-bit file".to_string());
    }
    let section_offset = file.u64(0x28)? as usize;
    let section_size = file.u16(0x3a)? as usize;
    let section_count = file.u16(0x3c)? as usize;
    let mut libraries = Vec::new();
    for index in 0..section_count {
        let header = section_offset + index * section_size;
        if file.u32(header + 4)? != 6 {
            continue; // SHT_DYNAMIC
        }
        let dynamic_offset = file.u64(header + 0x18)? as usize;
        let dynamic_size = file.u64(header + 0x20)? as usize;
        let strings_index = file.u32(header + 0x28)? as usize;
        let strings_offset = file.u64(section_offset + strings_index * section_size + 0x18)? as usize;
        let mut at = dynamic_offset;
        while at + 16 <= dynamic_offset + dynamic_size {
            let tag = file.u64(at)?;
            let value = file.u64(at + 8)? as usize;
            match tag {
                0 => break, // DT_NULL
                1 => libraries.push(file.c_string(strings_offset + value)?), // DT_NEEDED
                _ => {}
            }
            at += 16;
        }
    }
    Ok(Imports { format: "elf64", libraries })
}

fn macho64(bytes: &[u8]) -> Result<Imports, String> {
    let file = Bytes(bytes);
    let command_count = file.u32(16)? as usize;
    let mut at = 32;
    let mut libraries = Vec::new();
    for _ in 0..command_count {
        let command = file.u32(at)?;
        let size = file.u32(at + 4)? as usize;
        if size < 8 {
            return Err(format!("Mach-O: load command of {size} bytes at {at}"));
        }
        // LC_LOAD_DYLIB, LC_LAZY_LOAD_DYLIB, LC_LOAD_WEAK_DYLIB, LC_REEXPORT_DYLIB, LC_LOAD_UPWARD_DYLIB
        if matches!(command, 0xc | 0x20 | 0x8000_0018 | 0x8000_001f | 0x8000_0023) {
            let name_offset = file.u32(at + 8)? as usize;
            libraries.push(file.c_string(at + name_offset)?);
        }
        at += size;
    }
    Ok(Imports { format: "macho64", libraries })
}

fn pe32plus(bytes: &[u8]) -> Result<Imports, String> {
    let file = Bytes(bytes);
    let pe = file.u32(0x3c)? as usize;
    if file.slice(pe, 4)? != b"PE\0\0" {
        return Err("PE: signature not found".to_string());
    }
    let section_count = file.u16(pe + 6)? as usize;
    let optional_size = file.u16(pe + 20)? as usize;
    let optional = pe + 24;
    if file.u16(optional)? != 0x20b {
        return Err("PE: not a PE32+ file".to_string());
    }
    let directory_count = file.u32(optional + 108)? as usize;
    let directory = |index: usize| -> Result<(usize, usize), String> {
        if index >= directory_count {
            return Ok((0, 0));
        }
        let at = optional + 112 + index * 8;
        Ok((file.u32(at)? as usize, file.u32(at + 4)? as usize))
    };
    let sections = optional + optional_size;
    let to_offset = |rva: usize| -> Result<usize, String> {
        for index in 0..section_count {
            let header = sections + index * 40;
            let virtual_size = file.u32(header + 8)? as usize;
            let virtual_address = file.u32(header + 12)? as usize;
            let raw_size = file.u32(header + 16)? as usize;
            let raw_offset = file.u32(header + 20)? as usize;
            if rva >= virtual_address && rva < virtual_address + virtual_size.max(raw_size) {
                return Ok(rva - virtual_address + raw_offset);
            }
        }
        Err(format!("PE: address {rva:#x} in no section"))
    };
    let mut libraries = Vec::new();
    let (imports, _) = directory(1)?;
    if imports != 0 {
        let mut at = to_offset(imports)?;
        loop {
            let name = file.u32(at + 12)? as usize;
            if name == 0 && file.u32(at + 16)? == 0 {
                break;
            }
            libraries.push(file.c_string(to_offset(name)?)?);
            at += 20;
        }
    }
    let (delayed, _) = directory(13)?;
    if delayed != 0 {
        let mut at = to_offset(delayed)?;
        loop {
            let name = file.u32(at + 4)? as usize;
            if name == 0 {
                break;
            }
            libraries.push(format!("{} (delay-load)", file.c_string(to_offset(name)?)?));
            at += 32;
        }
    }
    Ok(Imports { format: "pe32+", libraries })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_running_test_binary_is_parsed_and_imports_no_graphics_library() {
        let imports = Imports::of_file(&std::env::current_exe().unwrap()).unwrap();
        assert!(!imports.libraries.is_empty(), "{imports:?}");
        assert!(imports.denied().is_empty(), "every graphics library is loaded at run time: {imports:?}");
    }

    #[test]
    fn the_matcher_ignores_case_and_matches_substrings() {
        let imports = Imports { format: "pe32+", libraries: vec!["KERNEL32.dll".into(), "USER32.dll".into(), "d3d12.dll (delay-load)".into()] };
        assert_eq!(imports.denied(), vec!["USER32.dll".to_string(), "d3d12.dll (delay-load)".to_string()]);
        let imports = Imports { format: "macho64", libraries: vec!["/usr/lib/libSystem.B.dylib".into()] };
        assert!(imports.denied().is_empty());
    }

    #[test]
    fn garbage_and_fat_files_are_refused_with_a_reason() {
        assert!(Imports::parse(b"hello world").unwrap_err().contains("not an ELF"));
        assert!(Imports::parse(&[0xca, 0xfe, 0xba, 0xbe, 0, 0, 0, 2]).unwrap_err().contains("fat"));
        assert!(Imports::parse(&[0x7f, b'E', b'L', b'F', 2, 1]).unwrap_err().contains("truncated"));
    }
}
