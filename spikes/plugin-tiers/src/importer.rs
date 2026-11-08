//! The out-of-process tier measured on what fits it: an importer-like client,
//! a separate process that turns a source file into canonical bytes the host
//! records as an input before the first tick. Throwaway, never product code.

use std::io::{Read, Write};
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use reference_workload::fnv1a;

/// The source the spike writes: the specification's repulsion constants.
pub const SOURCE: &str = "strength=64\nradius=8\n";

/// The import: two `f32` fields as little-endian bytes, then the FNV-1a 64
/// content hash of those bytes as eight little-endian bytes.
pub fn import(source: &str) -> Result<Vec<u8>, String> {
    let mut strength = None;
    let mut radius = None;
    for line in source.lines().map(str::trim).filter(|line| !line.is_empty()) {
        let (key, value) = line.split_once('=').ok_or_else(|| format!("line without '=': {line:?}"))?;
        let value: f32 = value.trim().parse().map_err(|_| format!("not a number: {value:?}"))?;
        match key.trim() {
            "strength" => strength = Some(value),
            "radius" => radius = Some(value),
            other => return Err(format!("unknown field {other:?}")),
        }
    }
    let (strength, radius) = (strength.ok_or("strength missing")?, radius.ok_or("radius missing")?);
    let mut bytes = Vec::with_capacity(16);
    bytes.extend_from_slice(&strength.to_le_bytes());
    bytes.extend_from_slice(&radius.to_le_bytes());
    let hash = fnv1a(&bytes);
    bytes.extend_from_slice(&hash.to_le_bytes());
    Ok(bytes)
}

/// The content hash carried at the end of an import's bytes.
pub fn content_hash(bytes: &[u8]) -> Option<u64> {
    bytes.get(bytes.len().checked_sub(8)?..).map(|tail| u64::from_le_bytes(tail.try_into().expect("eight bytes")))
}

/// The importer process: reads one line naming the source file on its
/// standard input, writes the import's bytes on its standard output.
pub fn serve() -> Result<(), String> {
    let mut line = String::new();
    std::io::stdin().read_line(&mut line).map_err(|error| error.to_string())?;
    let source = std::fs::read_to_string(line.trim()).map_err(|error| format!("{}: {error}", line.trim()))?;
    let bytes = import(&source)?;
    let mut out = std::io::stdout().lock();
    out.write_all(&bytes).map_err(|error| error.to_string())?;
    out.flush().map_err(|error| error.to_string())
}

/// Spawns `exe --importer`, sends it the source path, reads the bytes back
/// and waits for it; returns the bytes and the spawn plus round trip time.
pub fn round_trip(exe: &Path, source: &Path) -> Result<(Vec<u8>, Duration), String> {
    let started = Instant::now();
    let mut child = Command::new(exe)
        .arg("--importer")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| format!("spawning the importer: {error}"))?;
    {
        let mut stdin = child.stdin.take().ok_or("no stdin")?;
        writeln!(stdin, "{}", source.display()).map_err(|error| error.to_string())?;
    }
    let mut bytes = Vec::new();
    child.stdout.take().ok_or("no stdout")?.read_to_end(&mut bytes).map_err(|error| error.to_string())?;
    let status = child.wait().map_err(|error| error.to_string())?;
    let elapsed = started.elapsed();
    if !status.success() {
        let mut stderr = String::new();
        if let Some(mut pipe) = child.stderr.take() {
            let _ = pipe.read_to_string(&mut stderr);
        }
        return Err(format!("the importer exited with {status}: {}", stderr.trim()));
    }
    Ok((bytes, elapsed))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_import_is_canonical_and_carries_its_hash() {
        let bytes = import(SOURCE).unwrap();
        assert_eq!(bytes.len(), 16);
        assert_eq!(&bytes[..4], &64.0_f32.to_le_bytes());
        assert_eq!(&bytes[4..8], &8.0_f32.to_le_bytes());
        assert_eq!(content_hash(&bytes), Some(fnv1a(&bytes[..8])));
        assert_eq!(import("radius = 8\n\nstrength = 64").unwrap(), bytes, "field order and spacing do not matter");
        assert!(import("strength=64").unwrap_err().contains("radius missing"));
        assert!(import("strength=x\nradius=8").unwrap_err().contains("not a number"));
        assert!(import("mass=1").unwrap_err().contains("unknown field"));
    }
}
