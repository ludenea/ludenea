//! The container of a recording, and the client that reads one from its
//! description alone. Throwaway, never product code.
//!
//! This module knows nothing of the world, of a recording's meaning or of the
//! names of its sections. A file is a header, a table of sections and their
//! payloads; its first section is a description of record types in a small
//! type system (unsigned integers, a float, a string, bytes, a sequence of
//! records of a named type), and every other section is a run of records of
//! one described type. A client lists a file, checks its integrity and reads
//! its values with what is here and the description the file carries.
//!
//! Layout, every integer little-endian:
//!
//! ```text
//! magic            8 bytes  "LUDENREC"
//! container        u32      the container's version
//! file length      u64      the whole file, so that a cut file is refused
//! section count    u32
//! per section      tag (4 bytes), type index (u32), record count (u32),
//!                  offset (u64), length (u64), digest (u64, FNV-1a of the payload)
//! table digest     u64      FNV-1a of every byte before it
//! payloads
//! ```
//!
//! A file is untrusted input: every length is checked against the bytes that
//! remain before anything is allocated or read, and nothing here panics on a
//! malformed file.

use std::fmt;

use reference_workload::fnv1a;

pub const MAGIC: &[u8; 8] = b"LUDENREC";
pub const CONTAINER_VERSION: u32 = 1;
pub const DESCRIPTION_TAG: [u8; 4] = *b"desc";
/// The type index of the description section, which describes itself.
const SELF_DESCRIBED: u32 = u32::MAX;
const SECTION_ENTRY_LEN: usize = 4 + 4 + 4 + 8 + 8 + 8;
const PREAMBLE_LEN: usize = 8 + 4 + 8 + 4;
/// A bound on the sections of one file, far above what a recording holds.
const MAX_SECTIONS: u32 = 64;

/// The kind of one field of a record type.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Kind {
    U8,
    U32,
    U64,
    F32,
    Text,
    Bytes,
    /// A counted run of records of the type at this index of the description.
    Sequence(u32),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Field {
    pub name: String,
    pub kind: Kind,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RecordType {
    pub name: String,
    pub fields: Vec<Field>,
}

/// A record type from its name and its fields, for a writer's description.
pub fn record_type(name: &str, fields: &[(&str, Kind)]) -> RecordType {
    RecordType { name: name.to_string(), fields: fields.iter().map(|(n, k)| Field { name: (*n).to_string(), kind: k.clone() }).collect() }
}

#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    U8(u8),
    U32(u32),
    U64(u64),
    F32(f32),
    Text(String),
    Bytes(Vec<u8>),
    Sequence(Vec<Record>),
}

pub type Record = Vec<Value>;

/// One section as a writer gives it and a reader returns it.
#[derive(Clone, Debug, PartialEq)]
pub struct Section {
    pub tag: [u8; 4],
    pub type_index: u32,
    pub records: Vec<Record>,
}

/// A file refused, with the reason.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ContainerError(pub String);

impl fmt::Display for ContainerError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "recording refused: {}", self.0)
    }
}

impl std::error::Error for ContainerError {}

fn refused<T>(reason: impl Into<String>) -> Result<T, ContainerError> {
    Err(ContainerError(reason.into()))
}

pub fn tag_text(tag: [u8; 4]) -> String {
    String::from_utf8_lossy(&tag).into_owned()
}

// ---------------------------------------------------------------------------
// Writing
// ---------------------------------------------------------------------------

fn put_text(out: &mut Vec<u8>, text: &str) {
    out.extend_from_slice(&(text.len() as u32).to_le_bytes());
    out.extend_from_slice(text.as_bytes());
}

fn encode_description(types: &[RecordType]) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(&(types.len() as u32).to_le_bytes());
    for record_type in types {
        put_text(&mut out, &record_type.name);
        out.extend_from_slice(&(record_type.fields.len() as u32).to_le_bytes());
        for field in &record_type.fields {
            put_text(&mut out, &field.name);
            let (code, target) = match field.kind {
                Kind::U8 => (0_u8, None),
                Kind::U32 => (1, None),
                Kind::U64 => (2, None),
                Kind::F32 => (3, None),
                Kind::Text => (4, None),
                Kind::Bytes => (5, None),
                Kind::Sequence(index) => (6, Some(index)),
            };
            out.push(code);
            if let Some(index) = target {
                out.extend_from_slice(&index.to_le_bytes());
            }
        }
    }
    out
}

fn encode_record(out: &mut Vec<u8>, types: &[RecordType], type_index: u32, record: &Record) {
    let record_type = &types[type_index as usize];
    assert_eq!(record.len(), record_type.fields.len(), "a record of {} holds one value per field", record_type.name);
    for (field, value) in record_type.fields.iter().zip(record) {
        match (&field.kind, value) {
            (Kind::U8, Value::U8(v)) => out.push(*v),
            (Kind::U32, Value::U32(v)) => out.extend_from_slice(&v.to_le_bytes()),
            (Kind::U64, Value::U64(v)) => out.extend_from_slice(&v.to_le_bytes()),
            (Kind::F32, Value::F32(v)) => out.extend_from_slice(&v.to_le_bytes()),
            (Kind::Text, Value::Text(v)) => put_text(out, v),
            (Kind::Bytes, Value::Bytes(v)) => {
                out.extend_from_slice(&(v.len() as u32).to_le_bytes());
                out.extend_from_slice(v);
            }
            (Kind::Sequence(index), Value::Sequence(records)) => {
                out.extend_from_slice(&(records.len() as u32).to_le_bytes());
                for inner in records {
                    encode_record(out, types, *index, inner);
                }
            }
            (kind, value) => panic!("field {} of {} is {kind:?}, given {value:?}", field.name, record_type.name),
        }
    }
}

/// The bytes of a file holding the description and the sections, in order.
/// A writer's mistake (a value of another kind than its field) panics: the
/// writer is this spike's own code, never a file.
pub fn write(types: &[RecordType], sections: &[Section]) -> Vec<u8> {
    let mut payloads = vec![(DESCRIPTION_TAG, SELF_DESCRIBED, types.len() as u32, encode_description(types))];
    for section in sections {
        let mut payload = Vec::new();
        for record in &section.records {
            encode_record(&mut payload, types, section.type_index, record);
        }
        payloads.push((section.tag, section.type_index, section.records.len() as u32, payload));
    }
    let table_len = PREAMBLE_LEN + payloads.len() * SECTION_ENTRY_LEN + 8;
    let file_len = table_len + payloads.iter().map(|(_, _, _, payload)| payload.len()).sum::<usize>();
    let mut out = Vec::with_capacity(file_len);
    out.extend_from_slice(MAGIC);
    out.extend_from_slice(&CONTAINER_VERSION.to_le_bytes());
    out.extend_from_slice(&(file_len as u64).to_le_bytes());
    out.extend_from_slice(&(payloads.len() as u32).to_le_bytes());
    let mut offset = table_len;
    for (tag, type_index, count, payload) in &payloads {
        out.extend_from_slice(tag);
        out.extend_from_slice(&type_index.to_le_bytes());
        out.extend_from_slice(&count.to_le_bytes());
        out.extend_from_slice(&(offset as u64).to_le_bytes());
        out.extend_from_slice(&(payload.len() as u64).to_le_bytes());
        out.extend_from_slice(&fnv1a(payload).to_le_bytes());
        offset += payload.len();
    }
    let table_digest = fnv1a(&out);
    out.extend_from_slice(&table_digest.to_le_bytes());
    for (_, _, _, payload) in &payloads {
        out.extend_from_slice(payload);
    }
    out
}

// ---------------------------------------------------------------------------
// Reading
// ---------------------------------------------------------------------------

struct Reader<'a> {
    bytes: &'a [u8],
    at: usize,
    what: &'a str,
}

impl<'a> Reader<'a> {
    fn remaining(&self) -> usize {
        self.bytes.len() - self.at
    }

    fn take(&mut self, len: usize) -> Result<&'a [u8], ContainerError> {
        if len > self.remaining() {
            return refused(format!("{} ends at byte {} where {len} more bytes are stated", self.what, self.at));
        }
        let slice = &self.bytes[self.at..self.at + len];
        self.at += len;
        Ok(slice)
    }

    fn u8(&mut self) -> Result<u8, ContainerError> {
        Ok(self.take(1)?[0])
    }

    fn u32(&mut self) -> Result<u32, ContainerError> {
        Ok(u32::from_le_bytes(self.take(4)?.try_into().expect("four bytes")))
    }

    fn u64(&mut self) -> Result<u64, ContainerError> {
        Ok(u64::from_le_bytes(self.take(8)?.try_into().expect("eight bytes")))
    }

    fn text(&mut self) -> Result<String, ContainerError> {
        let len = self.u32()? as usize;
        match std::str::from_utf8(self.take(len)?) {
            Ok(text) => Ok(text.to_string()),
            Err(_) => refused(format!("{} holds a text that is not UTF-8", self.what)),
        }
    }

    /// A count of items of at least `min_len` bytes each, refused when the
    /// bytes that remain cannot hold them: nothing is allocated on a lie.
    fn count(&mut self, min_len: usize) -> Result<usize, ContainerError> {
        let count = self.u32()? as usize;
        if count.saturating_mul(min_len.max(1)) > self.remaining() {
            return refused(format!("{} states {count} items where {} bytes remain", self.what, self.remaining()));
        }
        Ok(count)
    }
}

struct Entry {
    tag: [u8; 4],
    type_index: u32,
    count: u32,
    offset: usize,
    len: usize,
    digest: u64,
}

/// The table of a file, checked: the magic, the version, the stated length
/// against the bytes given, the table's digest, and every section inside the
/// file, in order and without overlap.
fn table(bytes: &[u8]) -> Result<Vec<Entry>, ContainerError> {
    let mut reader = Reader { bytes, at: 0, what: "the header" };
    if reader.take(8)? != MAGIC {
        return refused("not a recording: the magic differs");
    }
    let version = reader.u32()?;
    if version != CONTAINER_VERSION {
        return refused(format!("container version {version}, this reader reads {CONTAINER_VERSION}"));
    }
    let file_len = reader.u64()?;
    if file_len != bytes.len() as u64 {
        return refused(format!("the header states {file_len} bytes and the file holds {}", bytes.len()));
    }
    let count = reader.u32()?;
    if count == 0 || count > MAX_SECTIONS {
        return refused(format!("{count} sections, between 1 and {MAX_SECTIONS} are read"));
    }
    let mut entries = Vec::with_capacity(count as usize);
    for _ in 0..count {
        let tag: [u8; 4] = reader.take(4)?.try_into().expect("four bytes");
        let type_index = reader.u32()?;
        let records = reader.u32()?;
        let offset = reader.u64()?;
        let len = reader.u64()?;
        let digest = reader.u64()?;
        entries.push((tag, type_index, records, offset, len, digest));
    }
    let digest_at = reader.at;
    let stated = reader.u64()?;
    if stated != fnv1a(&bytes[..digest_at]) {
        return refused("the header's digest differs from its bytes");
    }
    let mut expected = reader.at as u64;
    let mut checked = Vec::with_capacity(entries.len());
    for (tag, type_index, count, offset, len, digest) in entries {
        if offset != expected || len > file_len - offset {
            return refused(format!("section {} is not where the sections before it end, or runs past the file", tag_text(tag)));
        }
        expected = offset + len;
        checked.push(Entry { tag, type_index, count, offset: offset as usize, len: len as usize, digest });
    }
    if expected != file_len {
        return refused("bytes follow the last section");
    }
    if checked[0].tag != DESCRIPTION_TAG || checked[0].type_index != SELF_DESCRIBED {
        return refused("the first section is not the description");
    }
    Ok(checked)
}

fn decode_description(bytes: &[u8]) -> Result<Vec<RecordType>, ContainerError> {
    let mut reader = Reader { bytes, at: 0, what: "the description" };
    let type_count = reader.count(8)?;
    let mut types = Vec::with_capacity(type_count);
    for _ in 0..type_count {
        let name = reader.text()?;
        let field_count = reader.count(5)?;
        let mut fields = Vec::with_capacity(field_count);
        for _ in 0..field_count {
            let field_name = reader.text()?;
            let kind = match reader.u8()? {
                0 => Kind::U8,
                1 => Kind::U32,
                2 => Kind::U64,
                3 => Kind::F32,
                4 => Kind::Text,
                5 => Kind::Bytes,
                6 => Kind::Sequence(reader.u32()?),
                other => return refused(format!("field {field_name} of {name} has the unknown kind {other}")),
            };
            fields.push(Field { name: field_name, kind });
        }
        types.push(RecordType { name, fields });
    }
    if reader.remaining() != 0 {
        return refused("bytes follow the description's last type");
    }
    for record_type in &types {
        for field in &record_type.fields {
            if let Kind::Sequence(index) = field.kind
                && index as usize >= types.len()
            {
                return refused(format!("field {} of {} names a type the description lacks", field.name, record_type.name));
            }
        }
    }
    // A type that contains itself, directly or not, would let a few bytes ask
    // for an unbounded depth: a sequence may only name a type declared before
    // the one that holds it.
    for (position, record_type) in types.iter().enumerate() {
        for field in &record_type.fields {
            if let Kind::Sequence(index) = field.kind
                && index as usize >= position
            {
                return refused(format!("field {} of {} names a type declared at or after its own", field.name, record_type.name));
            }
        }
    }
    Ok(types)
}

/// The least bytes a record of the type occupies, for the bound on a count.
fn min_len(types: &[RecordType], type_index: u32) -> usize {
    types[type_index as usize]
        .fields
        .iter()
        .map(|field| match field.kind {
            Kind::U8 => 1,
            Kind::U32 | Kind::F32 | Kind::Text | Kind::Bytes | Kind::Sequence(_) => 4,
            Kind::U64 => 8,
        })
        .sum()
}

fn decode_record(reader: &mut Reader<'_>, types: &[RecordType], type_index: u32) -> Result<Record, ContainerError> {
    let record_type = &types[type_index as usize];
    let mut record = Vec::with_capacity(record_type.fields.len());
    for field in &record_type.fields {
        record.push(match field.kind {
            Kind::U8 => Value::U8(reader.u8()?),
            Kind::U32 => Value::U32(reader.u32()?),
            Kind::U64 => Value::U64(reader.u64()?),
            Kind::F32 => Value::F32(f32::from_le_bytes(reader.take(4)?.try_into().expect("four bytes"))),
            Kind::Text => Value::Text(reader.text()?),
            Kind::Bytes => {
                let len = reader.u32()? as usize;
                Value::Bytes(reader.take(len)?.to_vec())
            }
            Kind::Sequence(index) => {
                let count = reader.count(min_len(types, index))?;
                let mut records = Vec::with_capacity(count);
                for _ in 0..count {
                    records.push(decode_record(reader, types, index)?);
                }
                Value::Sequence(records)
            }
        });
    }
    Ok(record)
}

/// A file read whole: its description and its sections' values.
#[derive(Clone, Debug, PartialEq)]
pub struct Container {
    pub types: Vec<RecordType>,
    pub sections: Vec<Section>,
}

impl Container {
    pub fn section(&self, tag: [u8; 4]) -> Option<&Section> {
        self.sections.iter().find(|section| section.tag == tag)
    }

    /// The position of a field in a record type, by name.
    pub fn field(&self, type_index: u32, name: &str) -> Option<usize> {
        self.types.get(type_index as usize)?.fields.iter().position(|field| field.name == name)
    }
}

/// Reads a file from its description alone. Refuses a cut file, a header or a
/// section whose digest differs from its bytes, and any value that does not
/// fit the description, before returning anything.
pub fn read(bytes: &[u8]) -> Result<Container, ContainerError> {
    let entries = table(bytes)?;
    for entry in &entries {
        if fnv1a(&bytes[entry.offset..entry.offset + entry.len]) != entry.digest {
            return refused(format!("section {} differs from its digest", tag_text(entry.tag)));
        }
    }
    let types = decode_description(&bytes[entries[0].offset..entries[0].offset + entries[0].len])?;
    let mut sections = Vec::with_capacity(entries.len() - 1);
    for entry in &entries[1..] {
        let name = tag_text(entry.tag);
        if entry.type_index as usize >= types.len() {
            return refused(format!("section {name} names a type the description lacks"));
        }
        let what = format!("section {name}");
        let mut reader = Reader { bytes: &bytes[entry.offset..entry.offset + entry.len], at: 0, what: &what };
        if (entry.count as usize).saturating_mul(min_len(&types, entry.type_index).max(1)) > reader.remaining() {
            return refused(format!("section {name} states {} records where {} bytes are given", entry.count, entry.len));
        }
        let mut records = Vec::with_capacity(entry.count as usize);
        for _ in 0..entry.count {
            records.push(decode_record(&mut reader, &types, entry.type_index)?);
        }
        if reader.remaining() != 0 {
            return refused(format!("bytes follow the last record of section {name}"));
        }
        sections.push(Section { tag: entry.tag, type_index: entry.type_index, records });
    }
    Ok(Container { types, sections })
}

/// What a tool lists of a file without a world: per section its tag, the name
/// of its record type, its records, its bytes and its digest.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Listed {
    pub tag: String,
    pub type_name: String,
    pub records: u32,
    pub offset: usize,
    pub bytes: usize,
    pub digest: u64,
}

/// Lists a file and checks it: the listing is given only for a file that
/// [`read`] accepts, so a listed file is a checked one.
pub fn list(bytes: &[u8]) -> Result<Vec<Listed>, ContainerError> {
    let container = read(bytes)?;
    let entries = table(bytes)?;
    Ok(entries
        .iter()
        .map(|entry| Listed {
            tag: tag_text(entry.tag),
            type_name: if entry.type_index == SELF_DESCRIBED {
                "description".to_string()
            } else {
                container.types[entry.type_index as usize].name.clone()
            },
            records: entry.count,
            offset: entry.offset,
            bytes: entry.len,
            digest: entry.digest,
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> (Vec<RecordType>, Vec<Section>) {
        let types = vec![
            record_type("point", &[("x", Kind::F32), ("label", Kind::Text)]),
            record_type("row", &[("tick", Kind::U32), ("flag", Kind::U8), ("big", Kind::U64), ("blob", Kind::Bytes), ("points", Kind::Sequence(0))]),
        ];
        let row = |tick: u32, points: Vec<Record>| {
            vec![Value::U32(tick), Value::U8(1), Value::U64(u64::MAX - u64::from(tick)), Value::Bytes(vec![7; tick as usize]), Value::Sequence(points)]
        };
        let point = |x: f32| vec![Value::F32(x), Value::Text(format!("p{x}"))];
        let sections = vec![
            Section { tag: *b"rows", type_index: 1, records: vec![row(1, vec![]), row(2, vec![point(0.5), point(-1.25)])] },
            Section { tag: *b"pnts", type_index: 0, records: vec![point(3.0)] },
        ];
        (types, sections)
    }

    #[test]
    fn a_file_reads_back_from_its_description_and_lists_its_sections() {
        let (types, sections) = sample();
        let bytes = write(&types, &sections);
        let container = read(&bytes).unwrap();
        assert_eq!(container.types, types);
        assert_eq!(container.sections, sections);
        assert_eq!(container.field(1, "big"), Some(2));
        let listed = list(&bytes).unwrap();
        assert_eq!(listed.iter().map(|s| (s.tag.as_str(), s.type_name.as_str(), s.records)).collect::<Vec<_>>(), [
            ("desc", "description", 2),
            ("rows", "row", 2),
            ("pnts", "point", 1)
        ]);
    }

    #[test]
    fn every_cut_and_every_changed_byte_is_refused() {
        let (types, sections) = sample();
        let bytes = write(&types, &sections);
        for len in 0..bytes.len() {
            assert!(read(&bytes[..len]).is_err(), "a file cut at {len} bytes is read");
        }
        for at in 0..bytes.len() {
            let mut changed = bytes.clone();
            changed[at] ^= 0x40;
            assert!(read(&changed).is_err(), "a file with byte {at} changed is read");
        }
        let mut longer = bytes.clone();
        longer.push(0);
        assert!(read(&longer).is_err());
    }

    #[test]
    fn a_description_that_contains_itself_or_lies_about_a_count_is_refused() {
        let recursive = vec![record_type("node", &[("children", Kind::Sequence(0))])];
        let bytes = write(&recursive, &[]);
        assert!(read(&bytes).unwrap_err().0.contains("declared at or after"));
        let types = vec![record_type("row", &[("tick", Kind::U32)])];
        let mut bytes = write(&types, &[Section { tag: *b"rows", type_index: 0, records: vec![vec![Value::U32(1)]] }]);
        // The record count of the second section, then the table's digest again.
        let count_at = PREAMBLE_LEN + SECTION_ENTRY_LEN + 8;
        bytes[count_at..count_at + 4].copy_from_slice(&u32::MAX.to_le_bytes());
        let digest_at = PREAMBLE_LEN + 2 * SECTION_ENTRY_LEN;
        let digest = fnv1a(&bytes[..digest_at]);
        bytes[digest_at..digest_at + 8].copy_from_slice(&digest.to_le_bytes());
        assert!(read(&bytes).unwrap_err().0.contains("states"));
    }
}
