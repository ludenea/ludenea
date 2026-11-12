//! The pixel file and the numbers of a difference. Throwaway, never product
//! code.
//!
//! A pixel file is a PNG in its plainest lossless form, written by the device
//! baseline spike's writer: 8-bit RGBA, no filter, one stream of stored
//! (uncompressed) blocks. The reader here is strict: it reads only the bytes
//! that writer produces for the pixels they hold, so a file a tool rewrote is
//! refused even where it decodes to the same pixels. The independent decoder
//! is an ordinary tool of the system, started as a process: `pngtopam` of
//! Netpbm, which reads with libpng, on Linux and macOS; the PNG codec of
//! GDI+ through `System.Drawing` in Windows PowerShell on Windows.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use render_device_baseline::scene::Image;

/// Writes a picture as a pixel file.
pub fn write(image: &Image) -> Vec<u8> {
    image.png()
}

/// Reads a pixel file this spike wrote, and nothing else.
pub fn read_strict(file: &[u8]) -> Result<Image, String> {
    let image = Image::from_png(file)?;
    if image.png() != file {
        return Err("not the bytes this writer produces for these pixels".to_string());
    }
    Ok(image)
}

/// A pixel file in the writer's own form whose pixels are not the original's:
/// one channel of the middle pixel is changed.
pub fn with_one_pixel_changed(file: &[u8]) -> Result<Vec<u8>, String> {
    let mut image = read_strict(file)?;
    let middle = (image.height as usize / 2 * image.width as usize + image.width as usize / 2) * 4;
    image.rgba[middle] ^= 0x40;
    Ok(image.png())
}

/// The three numbers a test states for a bounded difference: the largest
/// difference per channel every pixel must meet, a second, smaller one, and
/// the count of pixels allowed to exceed the second.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Bounds {
    pub largest: u8,
    pub second: u8,
    pub allowed_beyond_second: u64,
}

/// What two pictures of one size differ by.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Numbers {
    pub largest_per_channel: [u8; 4],
    pub differing_pixels: u64,
    pub beyond_second: u64,
    pub beyond_largest: u64,
    /// The rectangle that bounds the pixels that differ: left, top, right
    /// and bottom, the last two excluded. All zero when no pixel differs.
    pub rectangle: [u32; 4],
}

impl Numbers {
    pub fn within(&self, bounds: &Bounds) -> bool {
        self.beyond_largest == 0 && self.beyond_second <= bounds.allowed_beyond_second
    }
}

/// The numbers of a difference, or nothing for two pictures of two sizes.
pub fn numbers(a: &Image, b: &Image, bounds: &Bounds) -> Option<Numbers> {
    if a.width != b.width || a.height != b.height || a.rgba.len() != b.rgba.len() {
        return None;
    }
    let mut numbers = Numbers { largest_per_channel: [0; 4], differing_pixels: 0, beyond_second: 0, beyond_largest: 0, rectangle: [u32::MAX, u32::MAX, 0, 0] };
    let width = a.width as usize;
    for (index, (first, second)) in a.rgba.as_chunks::<4>().0.iter().zip(b.rgba.as_chunks::<4>().0).enumerate() {
        if first == second {
            continue;
        }
        let mut largest = 0;
        for channel in 0..4 {
            let distance = first[channel].abs_diff(second[channel]);
            numbers.largest_per_channel[channel] = numbers.largest_per_channel[channel].max(distance);
            largest = largest.max(distance);
        }
        numbers.differing_pixels += 1;
        numbers.beyond_second += u64::from(largest > bounds.second);
        numbers.beyond_largest += u64::from(largest > bounds.largest);
        let (x, y) = ((index % width) as u32, (index / width) as u32);
        numbers.rectangle = [numbers.rectangle[0].min(x), numbers.rectangle[1].min(y), numbers.rectangle[2].max(x + 1), numbers.rectangle[3].max(y + 1)];
    }
    if numbers.differing_pixels == 0 {
        numbers.rectangle = [0; 4];
    }
    Some(numbers)
}

/// The difference picture: each channel's absolute difference, opaque.
pub fn difference_picture(a: &Image, b: &Image) -> Option<Image> {
    if a.width != b.width || a.height != b.height || a.rgba.len() != b.rgba.len() {
        return None;
    }
    let rgba = a.rgba.as_chunks::<4>().0.iter().zip(b.rgba.as_chunks::<4>().0).flat_map(|(first, second)| [first[0].abs_diff(second[0]), first[1].abs_diff(second[1]), first[2].abs_diff(second[2]), 255]).collect();
    Some(Image { width: a.width, height: a.height, rgba })
}

/// The independent decoder of this platform, by the name the report gives it.
pub fn decoder_name() -> &'static str {
    if cfg!(windows) { "the PNG codec of GDI+, through System.Drawing in Windows PowerShell" } else { "pngtopam of Netpbm, which reads with libpng" }
}

/// The tool that writes a picture again without loss, compressed.
pub fn rewriter_name() -> &'static str {
    if cfg!(windows) { "the PNG codec of GDI+, through System.Drawing in Windows PowerShell" } else { "pamtopng of Netpbm, which writes with libpng" }
}

fn run(command: &mut Command) -> Result<Vec<u8>, String> {
    let program = command.get_program().to_string_lossy().into_owned();
    let output = command.stdin(Stdio::null()).output().map_err(|error| format!("{program} could not be started: {error}"))?;
    if !output.status.success() {
        let said: String = String::from_utf8_lossy(&output.stderr).split_whitespace().collect::<Vec<_>>().join(" ").chars().take(300).collect();
        return Err(format!("{program} ended with {}: {said}", output.status));
    }
    Ok(output.stdout)
}

#[cfg(windows)]
fn powershell(mode: &str, input: &Path, output: &Path) -> Result<(), String> {
    let script = Path::new(env!("CARGO_MANIFEST_DIR")).join("tools").join("system-drawing.ps1");
    run(Command::new("powershell.exe").args(["-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass", "-File"]).arg(script).arg(mode).arg(input).arg(output)).map(|_| ())
}

/// Decodes a PNG file with the platform's independent decoder. `work` is a
/// directory for the decoder's output.
#[cfg(windows)]
pub fn decode_with_the_tool(file: &Path, work: &Path) -> Result<Image, String> {
    let raw: PathBuf = work.join("decoded.bgra");
    powershell("decode", file, &raw)?;
    let bytes = fs::read(&raw).map_err(|error| format!("the decoder's output: {error}"))?;
    let header = bytes.get(..8).ok_or("the decoder's output has no header")?;
    let (width, height) = (u32::from_le_bytes(header[..4].try_into().expect("four bytes")), u32::from_le_bytes(header[4..].try_into().expect("four bytes")));
    let pixels = &bytes[8..];
    if pixels.len() != width as usize * height as usize * 4 {
        return Err(format!("{} bytes of pixels for a {width} by {height} picture", pixels.len()));
    }
    // GDI+ hands 32-bit pixels as blue, green, red, alpha.
    let rgba = pixels.as_chunks::<4>().0.iter().flat_map(|pixel| [pixel[2], pixel[1], pixel[0], pixel[3]]).collect();
    Ok(Image { width, height, rgba })
}

/// Writes the picture of a PNG file again, compressed, with the platform's tool.
#[cfg(windows)]
pub fn rewrite_with_the_tool(file: &Path, rewritten: &Path, _work: &Path) -> Result<(), String> {
    powershell("rewrite", file, rewritten)
}

#[cfg(not(windows))]
pub fn decode_with_the_tool(file: &Path, _work: &Path) -> Result<Image, String> {
    parse_pam(&run(Command::new("pngtopam").arg("-alphapam").arg(file))?)
}

#[cfg(not(windows))]
pub fn rewrite_with_the_tool(file: &Path, rewritten: &Path, work: &Path) -> Result<(), String> {
    let pam: PathBuf = work.join("decoded.pam");
    fs::write(&pam, run(Command::new("pngtopam").arg("-alphapam").arg(file))?).map_err(|error| format!("the decoded picture: {error}"))?;
    fs::write(rewritten, run(Command::new("pamtopng").arg(&pam))?).map_err(|error| format!("the rewritten file: {error}"))
}

/// Reads a PAM file of 8-bit RGBA: the header's lines, then the rows.
#[cfg_attr(windows, allow(dead_code, reason = "Netpbm's form is read on Linux and macOS only"))]
fn parse_pam(bytes: &[u8]) -> Result<Image, String> {
    const END: &[u8] = b"ENDHDR\n";
    let end = bytes.windows(END.len()).position(|window| window == END).ok_or("no end of header in the decoder's output")?;
    let header = String::from_utf8_lossy(&bytes[..end]);
    let field = |name: &str| header.lines().find_map(|line| line.strip_prefix(name)).map(str::trim);
    let number = |name: &str| field(name).and_then(|value| value.parse::<u32>().ok()).ok_or_else(|| format!("no {name} in the decoder's output"));
    let (width, height, depth, maxval) = (number("WIDTH")?, number("HEIGHT")?, number("DEPTH")?, number("MAXVAL")?);
    if !header.starts_with("P7") || depth != 4 || maxval != 255 {
        return Err(format!("the decoder's output is not 8-bit RGBA (depth {depth}, largest value {maxval})"));
    }
    let rgba = bytes[end + END.len()..].to_vec();
    if rgba.len() != width as usize * height as usize * 4 {
        return Err(format!("{} bytes of pixels for a {width} by {height} picture", rgba.len()));
    }
    Ok(Image { width, height, rgba })
}

/// The size of blobs in the repository's packed form: the pack `git` writes
/// for them, in bytes, or the reason it could not be measured.
pub fn packed_size(files: &[&Path], work: &Path) -> Result<u64, String> {
    use std::io::Write;
    let repository = work.join(format!("pack-{}", files.len()));
    let _ = fs::remove_dir_all(&repository);
    fs::create_dir_all(&repository).map_err(|error| format!("{}: {error}", repository.display()))?;
    let git = |arguments: &[&str]| {
        let mut command = Command::new("git");
        command.arg("-C").arg(&repository).args(arguments);
        command
    };
    run(git(&["init", "--quiet"]).stdout(Stdio::piped()))?;
    let mut objects = String::new();
    for file in files {
        // Not the canonical form, whose prefix on Windows git does not read.
        let absolute = std::path::absolute(file).map_err(|error| format!("{}: {error}", file.display()))?;
        let identifier = run(git(&["hash-object", "-w", "--no-filters"]).arg(absolute))?;
        // The same path for every revision, as the revisions of one file have.
        objects.push_str(&format!("{} picture.png\n", String::from_utf8_lossy(&identifier).trim()));
    }
    let mut child = git(&["pack-objects", "--quiet", "pack"]).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().map_err(|error| format!("git could not be started: {error}"))?;
    child.stdin.take().ok_or("git's input")?.write_all(objects.as_bytes()).map_err(|error| format!("git's input: {error}"))?;
    let output = child.wait_with_output().map_err(|error| format!("git: {error}"))?;
    if !output.status.success() {
        return Err(format!("git pack-objects ended with {}", output.status));
    }
    let pack = repository.join(format!("pack-{}.pack", String::from_utf8_lossy(&output.stdout).trim()));
    let size = fs::metadata(&pack).map(|metadata| metadata.len()).map_err(|error| format!("{}: {error}", pack.display()));
    let _ = fs::remove_dir_all(&repository);
    size
}

#[cfg(test)]
mod tests {
    use super::*;

    fn picture() -> Image {
        let rgba = (0..16 * 8_u32).flat_map(|pixel| [pixel as u8, (pixel * 3) as u8, (pixel * 7) as u8, 255]).collect();
        Image { width: 16, height: 8, rgba }
    }

    #[test]
    fn the_strict_reader_reads_what_the_writer_wrote() {
        let picture = picture();
        assert_eq!(read_strict(&write(&picture)), Ok(picture));
    }

    #[test]
    fn the_strict_reader_refuses_a_file_with_one_more_chunk() {
        // A text chunk before the end: the same pixels, another form.
        let file = write(&picture());
        let end = file.len() - 12;
        let mut chunk = vec![0, 0, 0, 0];
        chunk.extend_from_slice(b"tEXt");
        chunk.extend_from_slice(&[0, 0, 0, 0]);
        let mut rewritten = file[..end].to_vec();
        rewritten.extend_from_slice(&chunk);
        rewritten.extend_from_slice(&file[end..]);
        assert!(Image::from_png(&rewritten).is_ok(), "the lenient reader decodes it");
        assert!(read_strict(&rewritten).is_err());
    }

    #[test]
    fn a_changed_pixel_keeps_the_form_and_changes_the_hash() {
        let file = write(&picture());
        let changed = with_one_pixel_changed(&file).expect("the file is read");
        let read = read_strict(&changed).expect("the changed file is in the writer's form");
        assert_ne!(read.hash(), picture().hash());
    }

    #[test]
    fn the_numbers_count_the_pixels_beyond_each_bound() {
        let a = picture();
        let mut b = a.clone();
        b.rgba[4 * (16 + 3)] += 2; // (3, 1): within the second bound
        b.rgba[4 * (5 * 16 + 9) + 1] += 9; // (9, 5): beyond the second
        b.rgba[4 * (2 * 16 + 12) + 2] ^= 0x80; // (12, 2): beyond the largest
        let bounds = Bounds { largest: 16, second: 4, allowed_beyond_second: 1 };
        let numbers = numbers(&a, &b, &bounds).expect("one size");
        assert_eq!(numbers, Numbers { largest_per_channel: [2, 9, 128, 0], differing_pixels: 3, beyond_second: 2, beyond_largest: 1, rectangle: [3, 1, 13, 6] });
        assert!(!numbers.within(&bounds));
        assert!(super::numbers(&a, &a, &bounds).expect("one size").within(&bounds));
        assert_eq!(super::numbers(&a, &a, &bounds).expect("one size").rectangle, [0; 4]);
        assert!(super::numbers(&a, &Image { width: 8, height: 16, rgba: a.rgba.clone() }, &bounds).is_none());
    }

    #[test]
    fn a_pam_header_is_read() {
        let mut file = b"P7\nWIDTH 2\nHEIGHT 1\nDEPTH 4\nMAXVAL 255\nTUPLTYPE RGB_ALPHA\nENDHDR\n".to_vec();
        file.extend_from_slice(&[1, 2, 3, 255, 4, 5, 6, 255]);
        assert_eq!(parse_pam(&file), Ok(Image { width: 2, height: 1, rgba: vec![1, 2, 3, 255, 4, 5, 6, 255] }));
        assert!(parse_pam(b"P7\nWIDTH 2\nHEIGHT 1\nDEPTH 3\nMAXVAL 255\nENDHDR\nabcdef").is_err());
    }
}
