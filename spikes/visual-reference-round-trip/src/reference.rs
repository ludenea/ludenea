//! A visual reference as reviewed files: a set of parts under one name.
//! Throwaway, never product code.
//!
//! A set is a directory. It holds one content part (`content.txt`), what is
//! the same on every device; one byte part per device key
//! (`bytes-<hash of the key>.txt`); and for a byte part, a pixel file named by
//! the hash of its bytes (`<hash>.png`). Parts are text, one `name = value`
//! per line, written in one order so that a part is a pure function of what it
//! records. A pending part is the same file with `.pending` after its name,
//! written into the directory a run is given, never over an accepted part.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use reference_workload::fnv1a;

use crate::frame::{Content, Target};

/// The form of the parts this spike writes and reads.
const FORM: &str = "1";
pub const CONTENT_FILE: &str = "content.txt";
pub const PENDING_SUFFIX: &str = ".pending";

fn hex(value: u64) -> String {
    format!("{value:016x}")
}

fn parse_hex(text: &str) -> Result<u64, String> {
    u64::from_str_radix(text, 16).map_err(|error| format!("{text:?} is not a hash: {error}"))
}

/// What is the same on every device.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ContentPart {
    pub engine_version: String,
    pub declared_level: String,
    pub fixture_hash: u64,
    pub tick: u64,
    pub target: String,
    pub presentation_settings_hash: u64,
    pub content: Content,
}

/// What one device drew, under its key.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BytePart {
    pub key: String,
    pub frame_bytes_hash: u64,
    /// The hash of each presentation payload this key's device bound.
    pub payloads: Vec<(String, u64)>,
    /// The name of the pixel file, absent where none is kept.
    pub pixel_file: Option<String>,
    /// Whether a bounded difference is made against this part.
    pub base: bool,
    /// Recorded beside the key and not part of it: the processor of the
    /// machine that drew the frame.
    pub host_processor: String,
    pub note: Option<String>,
}

fn lines(text: &str) -> Result<BTreeMap<String, String>, String> {
    let mut values = BTreeMap::new();
    for line in text.lines().filter(|line| !line.trim().is_empty()) {
        let (name, value) = line.split_once(" = ").ok_or_else(|| format!("a line with no ` = `: {line:?}"))?;
        if values.insert(name.to_string(), value.to_string()).is_some() {
            return Err(format!("{name} is stated twice"));
        }
    }
    Ok(values)
}

fn take(values: &mut BTreeMap<String, String>, name: &str) -> Result<String, String> {
    values.remove(name).ok_or_else(|| format!("no {name}"))
}

/// The entries whose name starts with `prefix`, in the order of a listed line.
fn listed(values: &mut BTreeMap<String, String>, order: &str, prefix: &str) -> Result<Vec<(String, u64)>, String> {
    let mut entries = Vec::new();
    for name in take(values, order)?.split(' ').filter(|name| !name.is_empty()) {
        entries.push((name.to_string(), parse_hex(&take(values, &format!("{prefix}{name}"))?)?));
    }
    Ok(entries)
}

impl ContentPart {
    pub fn of(content: &Content, target: &Target, fixture_hash: u64, tick: u64) -> ContentPart {
        ContentPart {
            engine_version: "visual-reference-round-trip spike, form 1".to_string(),
            declared_level: "application".to_string(),
            fixture_hash,
            tick,
            target: target.text(),
            // The spike has no presentation settings: the hash of nothing.
            presentation_settings_hash: fnv1a(&[]),
            content: content.clone(),
        }
    }

    pub fn text(&self) -> String {
        let names = |entries: &[(String, u64)]| entries.iter().map(|(name, _)| name.as_str()).collect::<Vec<_>>().join(" ");
        let mut text = String::new();
        let mut line = |name: &str, value: &str| text.push_str(&format!("{name} = {value}\n"));
        line("part", "content");
        line("form", FORM);
        line("engine_version", &self.engine_version);
        line("declared_level", &self.declared_level);
        line("fixture_state_hash", &hex(self.fixture_hash));
        line("tick", &self.tick.to_string());
        line("target", &self.target);
        line("presentation_settings_hash", &hex(self.presentation_settings_hash));
        line("content_hash", &hex(self.content.hash));
        line("view", &self.content.view.0);
        line(&format!("view.{}", self.content.view.0), &hex(self.content.view.1));
        line("passes", &names(&self.content.passes));
        for (name, hash) in &self.content.passes {
            line(&format!("pass.{name}"), &hex(*hash));
        }
        line("assets", &names(&self.content.assets));
        for (name, hash) in &self.content.assets {
            line(&format!("asset.{name}"), &hex(*hash));
        }
        text
    }

    pub fn parse(text: &str) -> Result<ContentPart, String> {
        let mut values = lines(text)?;
        if take(&mut values, "part")? != "content" || take(&mut values, "form")? != FORM {
            return Err("not a content part of this form".to_string());
        }
        let view = take(&mut values, "view")?;
        let view_hash = parse_hex(&take(&mut values, &format!("view.{view}"))?)?;
        let part = ContentPart {
            engine_version: take(&mut values, "engine_version")?,
            declared_level: take(&mut values, "declared_level")?,
            fixture_hash: parse_hex(&take(&mut values, "fixture_state_hash")?)?,
            tick: take(&mut values, "tick")?.parse().map_err(|error| format!("the tick: {error}"))?,
            target: take(&mut values, "target")?,
            presentation_settings_hash: parse_hex(&take(&mut values, "presentation_settings_hash")?)?,
            content: Content {
                hash: parse_hex(&take(&mut values, "content_hash")?)?,
                view: (view, view_hash),
                passes: listed(&mut values, "passes", "pass.")?,
                assets: listed(&mut values, "assets", "asset.")?,
            },
        };
        match values.keys().next() {
            Some(name) => Err(format!("a line this form does not hold: {name}")),
            None => Ok(part),
        }
    }
}

impl BytePart {
    /// The name of the part's file: the key is long and holds characters a
    /// file name may not, so the file is named by the key's hash.
    pub fn file_name(key: &str) -> String {
        format!("bytes-{}.txt", hex(fnv1a(key.as_bytes())))
    }

    pub fn text(&self) -> String {
        let mut text = String::new();
        let mut line = |name: &str, value: &str| text.push_str(&format!("{name} = {value}\n"));
        line("part", "bytes");
        line("form", FORM);
        line("key", &self.key);
        line("frame_bytes_hash", &hex(self.frame_bytes_hash));
        line("payloads", &self.payloads.iter().map(|(name, _)| name.as_str()).collect::<Vec<_>>().join(" "));
        for (name, hash) in &self.payloads {
            line(&format!("payload.{name}"), &hex(*hash));
        }
        if let Some(file) = &self.pixel_file {
            line("pixel_file", file);
        }
        line("base", if self.base { "yes" } else { "no" });
        line("host_processor", &self.host_processor);
        if let Some(note) = &self.note {
            line("note", note);
        }
        text
    }

    pub fn parse(text: &str) -> Result<BytePart, String> {
        let mut values = lines(text)?;
        if take(&mut values, "part")? != "bytes" || take(&mut values, "form")? != FORM {
            return Err("not a byte part of this form".to_string());
        }
        let part = BytePart {
            key: take(&mut values, "key")?,
            frame_bytes_hash: parse_hex(&take(&mut values, "frame_bytes_hash")?)?,
            payloads: listed(&mut values, "payloads", "payload.")?,
            pixel_file: values.remove("pixel_file"),
            base: take(&mut values, "base")? == "yes",
            host_processor: take(&mut values, "host_processor")?,
            note: values.remove("note"),
        };
        match values.keys().next() {
            Some(name) => Err(format!("a line this form does not hold: {name}")),
            None => Ok(part),
        }
    }
}

/// The name of a pixel file: the hash of its bytes, so that the files of
/// several revisions and branches never collide in one directory.
pub fn pixel_file_name(file_bytes: &[u8]) -> String {
    format!("{}.png", hex(fnv1a(file_bytes)))
}

/// An accepted set, as read from its directory.
#[derive(Clone, Debug, Default)]
pub struct ReferenceSet {
    pub directory: PathBuf,
    pub content: Option<ContentPart>,
    pub bytes: Vec<BytePart>,
}

impl ReferenceSet {
    /// Reads the accepted parts of a directory; pending parts are not read. A
    /// directory that does not exist is a set with no part.
    pub fn read(directory: &Path) -> Result<ReferenceSet, String> {
        let mut set = ReferenceSet { directory: directory.to_path_buf(), content: None, bytes: Vec::new() };
        let Ok(entries) = fs::read_dir(directory) else { return Ok(set) };
        let mut names: Vec<String> = entries.flatten().filter_map(|entry| entry.file_name().into_string().ok()).collect();
        names.sort();
        for name in names {
            let read = || fs::read_to_string(directory.join(&name)).map_err(|error| format!("{name}: {error}"));
            if name == CONTENT_FILE {
                set.content = Some(ContentPart::parse(&read()?).map_err(|error| format!("{name}: {error}"))?);
            } else if name.starts_with("bytes-") && name.ends_with(".txt") {
                let part = BytePart::parse(&read()?).map_err(|error| format!("{name}: {error}"))?;
                if BytePart::file_name(&part.key) != name {
                    return Err(format!("{name} is not named by the hash of the key it holds"));
                }
                set.bytes.push(part);
            }
        }
        Ok(set)
    }

    pub fn byte_part(&self, key: &str) -> Option<&BytePart> {
        self.bytes.iter().find(|part| part.key == key)
    }

    /// The part a bounded difference is made against.
    pub fn base(&self) -> Option<&BytePart> {
        self.bytes.iter().find(|part| part.base)
    }

    /// The bytes of a part's pixel file, where the set keeps it.
    pub fn pixel_file(&self, part: &BytePart) -> Option<Vec<u8>> {
        fs::read(self.directory.join(part.pixel_file.as_ref()?)).ok()
    }
}

/// Writes an accepted set: the review's accept, done by the spike for the
/// references its own cases compare against.
pub fn write_accepted(directory: &Path, content: &ContentPart, bytes: &BytePart, pixel_file: &[u8]) -> Result<(), String> {
    let write = |name: &str, data: &[u8]| fs::write(directory.join(name), data).map_err(|error| format!("{name}: {error}"));
    fs::create_dir_all(directory).map_err(|error| format!("{}: {error}", directory.display()))?;
    write(CONTENT_FILE, content.text().as_bytes())?;
    write(&BytePart::file_name(&bytes.key), bytes.text().as_bytes())?;
    if let Some(name) = &bytes.pixel_file {
        write(name, pixel_file)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::frame::{Change, Frame};

    fn parts() -> (ContentPart, BytePart) {
        let frame = Frame::build(Change::None);
        let content = ContentPart::of(&frame.content(), &frame.target, Frame::fixture_hash(), frame.tick);
        let bytes = BytePart {
            key: "interface | device (version 1, 256 bits) | driver 1.2.3 | settings; with = signs | x86_64".to_string(),
            frame_bytes_hash: 0x0123_4567_89ab_cdef,
            payloads: frame.payloads(),
            pixel_file: Some("00000000000000aa.png".to_string()),
            base: true,
            host_processor: "a processor".to_string(),
            note: Some("accepted with a known defect of its device".to_string()),
        };
        (content, bytes)
    }

    #[test]
    fn the_parts_read_back_as_written() {
        let (content, bytes) = parts();
        assert_eq!(ContentPart::parse(&content.text()), Ok(content.clone()));
        assert_eq!(BytePart::parse(&bytes.text()), Ok(bytes.clone()));
        let bare = BytePart { pixel_file: None, note: None, base: false, ..bytes };
        assert_eq!(BytePart::parse(&bare.text()), Ok(bare));
    }

    #[test]
    fn a_part_of_another_kind_or_with_an_unknown_line_is_refused() {
        let (content, bytes) = parts();
        assert!(ContentPart::parse(&bytes.text()).is_err());
        assert!(BytePart::parse(&content.text()).is_err());
        assert!(ContentPart::parse(&format!("{}surprise = 1\n", content.text())).is_err());
        assert!(BytePart::parse(&bytes.text().replace("frame_bytes_hash = ", "frame_bytes_hash = z")).is_err());
    }

    #[test]
    fn a_set_reads_its_accepted_parts_and_no_pending_one() {
        let (content, bytes) = parts();
        let directory = std::env::temp_dir().join(format!("visual-reference-set-{}", std::process::id()));
        let _ = fs::remove_dir_all(&directory);
        write_accepted(&directory, &content, &bytes, b"pixels").expect("the set is written");
        fs::write(directory.join(format!("{CONTENT_FILE}{PENDING_SUFFIX}")), "not a part").expect("a pending file");
        let set = ReferenceSet::read(&directory).expect("the set is read");
        assert_eq!(set.content, Some(content));
        assert_eq!(set.byte_part(&bytes.key), Some(&bytes));
        assert_eq!(set.base(), Some(&bytes));
        assert_eq!(set.pixel_file(&bytes), Some(b"pixels".to_vec()));
        assert!(set.byte_part("another key").is_none());
        let _ = fs::remove_dir_all(&directory);
        assert!(ReferenceSet::read(&directory).expect("an absent set").content.is_none());
    }
}
