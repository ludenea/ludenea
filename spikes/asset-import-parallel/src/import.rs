//! The importer as a hermetic action and its canonical output bytes.
//! Throwaway, never product code.
//!
//! An action's key is the digest of its inputs: the source bytes, the
//! importer's identity and version, the import settings, the target platform
//! name and the content hashes of the outputs of the actions it depends on.
//! Its output is canonical bytes: a versioned header (magic, format version,
//! kind, importer version), little-endian fields in a fixed order, then the
//! body; its content hash is the digest of those bytes. The cache maps action
//! keys to outputs, in memory and mirrored on disk.
//!
//! The plain importer uses integer arithmetic only. The geometric importer
//! uses the IEEE-exact floating-point operations only (addition, subtraction,
//! multiplication, division, `sqrt`, comparisons, conversions) and never
//! asks for a fused multiply-add; the control importer is the geometric one
//! with the platform math library (`ln` and `exp`) in the normalization.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use reference_workload::{Fnv1a, fnv1a};

use crate::sources::{Kind, MeshSource, NAME_MAX_LENGTH};

pub const MAGIC: &[u8; 4] = b"LDNA";
pub const FORMAT_VERSION: u16 = 1;
/// The output never depends on the platform, so the target is one constant name.
pub const TARGET_PLATFORM: &str = "any";

/// The importer's identity, part of the action key.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ImporterIdentity {
    pub name: &'static str,
    pub version: u32,
}

pub fn importer_for(kind: Kind, version: u32) -> ImporterIdentity {
    let name = match kind {
        Kind::Plain => "ludenea-spike-plain-importer",
        Kind::Geometric => "ludenea-spike-geometric-importer",
        Kind::Control => "ludenea-spike-geometric-importer-platform-normalization",
    };
    ImporterIdentity { name, version }
}

/// The import settings, part of the action key.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Settings {
    /// The bits of the quantized positions of a mesh (16 gives `u16` fields).
    pub quantization_bits: u8,
}

impl Settings {
    pub const DEFAULT: Settings = Settings { quantization_bits: 16 };

    fn bytes(&self) -> [u8; 1] {
        [self.quantization_bits]
    }
}

/// Everything an action's key is made of.
#[derive(Clone, Copy)]
pub struct ActionInputs<'a> {
    pub source_digest: u64,
    pub importer: ImporterIdentity,
    pub settings: &'a Settings,
    pub target_platform: &'a str,
    pub dependency_hashes: &'a [u64],
}

/// The digest of the action's inputs, every variable-length part prefixed by
/// its length so that no two input sets share a byte stream.
pub fn action_key(inputs: &ActionInputs) -> u64 {
    let mut hasher = Fnv1a::new();
    hasher.write(&inputs.source_digest.to_le_bytes());
    hasher.write(&(inputs.importer.name.len() as u32).to_le_bytes());
    hasher.write(inputs.importer.name.as_bytes());
    hasher.write(&inputs.importer.version.to_le_bytes());
    let settings = inputs.settings.bytes();
    hasher.write(&(settings.len() as u32).to_le_bytes());
    hasher.write(&settings);
    hasher.write(&(inputs.target_platform.len() as u32).to_le_bytes());
    hasher.write(inputs.target_platform.as_bytes());
    hasher.write(&(inputs.dependency_hashes.len() as u32).to_le_bytes());
    for hash in inputs.dependency_hashes {
        hasher.write(&hash.to_le_bytes());
    }
    hasher.finish()
}

/// The content hash of an output: the digest of its canonical bytes.
pub fn content_hash(bytes: &[u8]) -> u64 {
    fnv1a(bytes)
}

/// The importer: one function of the source bytes and the dependencies' outputs.
pub fn import(kind: Kind, importer_version: u32, settings: &Settings, source: &[u8], dependencies: &[&[u8]]) -> Result<Vec<u8>, String> {
    let header = Header { kind, format_version: FORMAT_VERSION, importer_version };
    match kind {
        Kind::Plain => {
            if !dependencies.is_empty() {
                return Err(format!("the plain importer takes no dependency; {} given", dependencies.len()));
            }
            import_plain(&header, source)
        }
        Kind::Geometric | Kind::Control => {
            if dependencies.len() != 1 {
                return Err(format!("the geometric importer takes exactly one dependency, the transform table; {} given", dependencies.len()));
            }
            let normalization = if kind == Kind::Control { Normalization::PlatformLibrary } else { Normalization::Exact };
            import_geometric(&header, settings, source, dependencies[0], normalization)
        }
    }
}

/// The versioned header every canonical output starts with: magic, format
/// version, kind tag, a reserved byte, importer version.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Header {
    pub kind: Kind,
    pub format_version: u16,
    pub importer_version: u32,
}

impl Header {
    pub const BYTES: usize = 12;

    fn write(&self, out: &mut Vec<u8>) {
        out.extend_from_slice(MAGIC);
        out.extend_from_slice(&self.format_version.to_le_bytes());
        out.push(self.kind.tag());
        out.push(0);
        out.extend_from_slice(&self.importer_version.to_le_bytes());
    }

    pub fn read(bytes: &[u8]) -> Result<Header, String> {
        if bytes.len() < Self::BYTES {
            return Err(format!("header needs {} bytes, {} given", Self::BYTES, bytes.len()));
        }
        if &bytes[..4] != MAGIC {
            return Err(format!("bad magic {:?}", &bytes[..4]));
        }
        let format_version = u16::from_le_bytes([bytes[4], bytes[5]]);
        if format_version != FORMAT_VERSION {
            return Err(format!("format version {format_version} is not {FORMAT_VERSION}"));
        }
        let kind = Kind::from_tag(bytes[6]).ok_or_else(|| format!("unknown kind tag {}", bytes[6]))?;
        let importer_version = u32::from_le_bytes(bytes[8..12].try_into().expect("four bytes"));
        Ok(Header { kind, format_version, importer_version })
    }
}

/// A little-endian reader over canonical bytes that refuses to run past the end.
struct Reader<'a> {
    bytes: &'a [u8],
    cursor: usize,
}

impl<'a> Reader<'a> {
    fn new(bytes: &'a [u8], cursor: usize) -> Reader<'a> {
        Reader { bytes, cursor }
    }

    fn take(&mut self, count: usize, what: &str) -> Result<&'a [u8], String> {
        let end = self.cursor.checked_add(count).filter(|&end| end <= self.bytes.len());
        let end = end.ok_or_else(|| format!("truncated: {what} needs {count} bytes at offset {}, {} left", self.cursor, self.bytes.len() - self.cursor))?;
        let slice = &self.bytes[self.cursor..end];
        self.cursor = end;
        Ok(slice)
    }

    fn u8(&mut self, what: &str) -> Result<u8, String> {
        Ok(self.take(1, what)?[0])
    }

    fn u16(&mut self, what: &str) -> Result<u16, String> {
        Ok(u16::from_le_bytes(self.take(2, what)?.try_into().expect("two bytes")))
    }

    fn u32(&mut self, what: &str) -> Result<u32, String> {
        Ok(u32::from_le_bytes(self.take(4, what)?.try_into().expect("four bytes")))
    }

    fn i32(&mut self, what: &str) -> Result<i32, String> {
        Ok(i32::from_le_bytes(self.take(4, what)?.try_into().expect("four bytes")))
    }

    fn i64(&mut self, what: &str) -> Result<i64, String> {
        Ok(i64::from_le_bytes(self.take(8, what)?.try_into().expect("eight bytes")))
    }

    fn u64(&mut self, what: &str) -> Result<u64, String> {
        Ok(u64::from_le_bytes(self.take(8, what)?.try_into().expect("eight bytes")))
    }

    #[cfg(test)]
    fn f32(&mut self, what: &str) -> Result<f32, String> {
        Ok(f32::from_le_bytes(self.take(4, what)?.try_into().expect("four bytes")))
    }

    fn finish(&self) -> Result<(), String> {
        if self.cursor != self.bytes.len() {
            return Err(format!("{} trailing byte(s)", self.bytes.len() - self.cursor));
        }
        Ok(())
    }
}

// The plain kind: a record table imported by integer arithmetic only.

/// One record of a plain table.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Record {
    pub key: u32,
    pub a: u8,
    pub b: u16,
    pub c: i32,
    pub d: i64,
    pub name: String,
}

impl Record {
    fn write(&self, out: &mut Vec<u8>) {
        out.extend_from_slice(&self.key.to_le_bytes());
        out.push(self.a);
        out.extend_from_slice(&self.b.to_le_bytes());
        out.extend_from_slice(&self.c.to_le_bytes());
        out.extend_from_slice(&self.d.to_le_bytes());
        out.push(self.name.len() as u8);
        out.extend_from_slice(self.name.as_bytes());
    }

    fn read(reader: &mut Reader) -> Result<Record, String> {
        let key = reader.u32("record key")?;
        let a = reader.u8("record field a")?;
        let b = reader.u16("record field b")?;
        let c = reader.i32("record field c")?;
        let d = reader.i64("record field d")?;
        let length = reader.u8("record name length")? as usize;
        let name = reader.take(length, "record name")?;
        let name = validate_name(std::str::from_utf8(name).map_err(|_| "record name is not UTF-8".to_string())?)?;
        Ok(Record { key, a, b, c, d, name })
    }
}

fn validate_name(name: &str) -> Result<String, String> {
    if name.is_empty() || name.len() > NAME_MAX_LENGTH || !name.bytes().all(|byte| byte.is_ascii_lowercase()) {
        return Err(format!("name {name:?} must be 1 to {NAME_MAX_LENGTH} lowercase ascii letters"));
    }
    Ok(name.to_string())
}

/// The plain kind's canonical body: the records sorted by key and
/// deduplicated, the prefix sums of their `c` field, the checksum of the
/// record bytes.
#[derive(Debug, PartialEq, Eq)]
pub struct PlainOutput {
    pub records: Vec<Record>,
    pub prefix_sums: Vec<i64>,
    pub checksum: u64,
}

impl PlainOutput {
    /// The record bytes the checksum covers.
    pub fn record_bytes(&self) -> Vec<u8> {
        let mut bytes = Vec::new();
        for record in &self.records {
            record.write(&mut bytes);
        }
        bytes
    }
}

fn parse_field<T: std::str::FromStr>(line: usize, what: &str, text: &str) -> Result<T, String> {
    text.parse().map_err(|_| format!("line {line}: {what} {text:?} is not a {}", std::any::type_name::<T>()))
}

fn import_plain(header: &Header, source: &[u8]) -> Result<Vec<u8>, String> {
    let text = std::str::from_utf8(source).map_err(|error| format!("plain source is not UTF-8: {error}"))?;
    let mut records = Vec::new();
    for (number, line) in text.lines().enumerate().filter(|(_, line)| !line.trim().is_empty()) {
        let line_number = number + 1;
        let fields: Vec<&str> = line.split(';').collect();
        if fields.len() != 6 {
            return Err(format!("line {line_number}: expected 6 fields, got {}", fields.len()));
        }
        records.push(Record {
            key: parse_field(line_number, "key", fields[0])?,
            a: parse_field(line_number, "field a", fields[1])?,
            b: parse_field(line_number, "field b", fields[2])?,
            c: parse_field(line_number, "field c", fields[3])?,
            d: parse_field(line_number, "field d", fields[4])?,
            name: validate_name(fields[5]).map_err(|error| format!("line {line_number}: {error}"))?,
        });
    }
    if records.is_empty() {
        return Err("plain source has no record".to_string());
    }
    // A stable sort keeps the file order among equal keys; the deduplication
    // keeps the first record of each key.
    records.sort_by_key(|record| record.key);
    records.dedup_by_key(|record| record.key);
    let mut prefix_sums = Vec::with_capacity(records.len());
    let mut running = 0_i64;
    for record in &records {
        running = running.wrapping_add(i64::from(record.c));
        prefix_sums.push(running);
    }
    let output = PlainOutput { records, prefix_sums, checksum: 0 };
    let record_bytes = output.record_bytes();
    let checksum = fnv1a(&record_bytes);
    let mut bytes = Vec::with_capacity(Header::BYTES + 4 + record_bytes.len() + 8 * output.prefix_sums.len() + 8);
    header.write(&mut bytes);
    bytes.extend_from_slice(&(output.records.len() as u32).to_le_bytes());
    bytes.extend_from_slice(&record_bytes);
    for sum in &output.prefix_sums {
        bytes.extend_from_slice(&sum.to_le_bytes());
    }
    bytes.extend_from_slice(&checksum.to_le_bytes());
    Ok(bytes)
}

pub fn decode_plain(bytes: &[u8]) -> Result<(Header, PlainOutput), String> {
    let header = Header::read(bytes)?;
    if header.kind != Kind::Plain {
        return Err(format!("kind {} is not plain", header.kind));
    }
    let mut reader = Reader::new(bytes, Header::BYTES);
    let count = reader.u32("record count")? as usize;
    let records_start = reader.cursor;
    let mut records = Vec::with_capacity(count.min(1 << 16));
    for _ in 0..count {
        records.push(Record::read(&mut reader)?);
    }
    let record_bytes = &bytes[records_start..reader.cursor];
    let mut prefix_sums = Vec::with_capacity(count.min(1 << 16));
    for _ in 0..count {
        prefix_sums.push(reader.i64("prefix sum")?);
    }
    let checksum = reader.u64("checksum")?;
    reader.finish()?;
    let expected = fnv1a(record_bytes);
    if checksum != expected {
        return Err(format!("checksum 0x{checksum:016x} does not match the records' 0x{expected:016x}"));
    }
    Ok((header, PlainOutput { records, prefix_sums, checksum }))
}

// The geometric kind: a mesh imported by IEEE-exact operations only.

/// The scale and offset a mesh's transform table gives, read from the first
/// record of the table's canonical output by exact integer conversions.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Transform {
    pub scale: f32,
    pub offset: [f32; 3],
}

pub fn transform_of(table_output: &[u8]) -> Result<Transform, String> {
    let (_, table) = decode_plain(table_output).map_err(|error| format!("transform table: {error}"))?;
    let first = table.records.first().ok_or("transform table: no record")?;
    Ok(Transform {
        scale: 1.0 + (first.b % 8) as f32 * 0.25,
        offset: [(first.c.rem_euclid(64)) as f32, first.a as f32 * 0.5, (first.d.rem_euclid(128)) as f32 * 0.125],
    })
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Normalization {
    /// `n / sqrt(n . n)`: exact operations only.
    Exact,
    /// `n * exp(-0.5 * ln(n . n))`: the platform math library, the control.
    PlatformLibrary,
}

/// One vertex of the canonical mesh: its position quantized inside the
/// bounding box and its normal.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Vertex {
    pub quantized: [u16; 3],
    pub normal: [f32; 3],
}

/// The geometric kind's canonical body.
#[derive(Debug, PartialEq)]
pub struct GeometricOutput {
    pub bounds_min: [f32; 3],
    pub bounds_max: [f32; 3],
    pub sphere_center: [f32; 3],
    pub sphere_radius: f32,
    pub vertices: Vec<Vertex>,
    pub triangles: Vec<[u32; 3]>,
}

fn sub(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

fn cross(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]]
}

fn dot(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn normalize(n: [f32; 3], normalization: Normalization) -> [f32; 3] {
    let length_squared = dot(n, n);
    if !(length_squared > 0.0) {
        return [0.0; 3];
    }
    match normalization {
        Normalization::Exact => {
            let length = length_squared.sqrt();
            [n[0] / length, n[1] / length, n[2] / length]
        }
        Normalization::PlatformLibrary => {
            let inverse = (-0.5 * length_squared.ln()).exp();
            [n[0] * inverse, n[1] * inverse, n[2] * inverse]
        }
    }
}

fn import_geometric(header: &Header, settings: &Settings, source: &[u8], table_output: &[u8], normalization: Normalization) -> Result<Vec<u8>, String> {
    let mesh = MeshSource::decode(source)?;
    let transform = transform_of(table_output)?;
    if settings.quantization_bits == 0 || settings.quantization_bits > 16 {
        return Err(format!("quantization bits {} not in 1 to 16", settings.quantization_bits));
    }
    // The transform: a multiplication then an addition, two roundings, never contracted.
    let positions: Vec<[f32; 3]> = mesh
        .positions
        .iter()
        .map(|p| [p[0] * transform.scale + transform.offset[0], p[1] * transform.scale + transform.offset[1], p[2] * transform.scale + transform.offset[2]])
        .collect();
    // Vertex normals: the sum of the face normals, in triangle order, normalized.
    let mut normals = vec![[0.0_f32; 3]; positions.len()];
    for triangle in &mesh.triangles {
        let [a, b, c] = [positions[triangle[0] as usize], positions[triangle[1] as usize], positions[triangle[2] as usize]];
        let face = cross(sub(b, a), sub(c, a));
        for &index in triangle {
            let normal = &mut normals[index as usize];
            normal[0] += face[0];
            normal[1] += face[1];
            normal[2] += face[2];
        }
    }
    let normals: Vec<[f32; 3]> = normals.into_iter().map(|n| normalize(n, normalization)).collect();
    // Bounding box and sphere, by comparisons (not `f32::min` and `f32::max`,
    // whose lowering is the target's business).
    let mut bounds_min = positions[0];
    let mut bounds_max = positions[0];
    for p in &positions {
        for axis in 0..3 {
            if p[axis] < bounds_min[axis] {
                bounds_min[axis] = p[axis];
            }
            if p[axis] > bounds_max[axis] {
                bounds_max[axis] = p[axis];
            }
        }
    }
    let sphere_center = [(bounds_min[0] + bounds_max[0]) * 0.5, (bounds_min[1] + bounds_max[1]) * 0.5, (bounds_min[2] + bounds_max[2]) * 0.5];
    let mut radius_squared = 0.0_f32;
    for p in &positions {
        let d = sub(*p, sphere_center);
        let distance_squared = dot(d, d);
        if distance_squared > radius_squared {
            radius_squared = distance_squared;
        }
    }
    let sphere_radius = radius_squared.sqrt();
    // Quantization inside the bounding box: an exact subtraction and division,
    // a multiplication, a rounding by conversion.
    let steps = ((1_u32 << settings.quantization_bits) - 1) as f32;
    let quantize = |p: &[f32; 3]| -> [u16; 3] {
        let mut q = [0_u16; 3];
        for axis in 0..3 {
            let extent = bounds_max[axis] - bounds_min[axis];
            q[axis] = if extent > 0.0 { ((p[axis] - bounds_min[axis]) / extent * steps + 0.5) as u16 } else { 0 };
        }
        q
    };
    let quantized: Vec<[u16; 3]> = positions.iter().map(quantize).collect();
    // The canonical order: by quantized position, then by original index.
    let mut order: Vec<usize> = (0..positions.len()).collect();
    order.sort_unstable_by_key(|&index| (quantized[index], index));
    let mut new_index = vec![0_u32; positions.len()];
    for (new, &old) in order.iter().enumerate() {
        new_index[old] = new as u32;
    }
    let vertices: Vec<Vertex> = order.iter().map(|&old| Vertex { quantized: quantized[old], normal: normals[old] }).collect();
    let triangles: Vec<[u32; 3]> = mesh.triangles.iter().map(|t| [new_index[t[0] as usize], new_index[t[1] as usize], new_index[t[2] as usize]]).collect();
    let output = GeometricOutput { bounds_min, bounds_max, sphere_center, sphere_radius, vertices, triangles };
    Ok(encode_geometric(header, &output))
}

fn encode_geometric(header: &Header, output: &GeometricOutput) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(Header::BYTES + 48 + 18 * output.vertices.len() + 12 * output.triangles.len());
    header.write(&mut bytes);
    bytes.extend_from_slice(&(output.vertices.len() as u32).to_le_bytes());
    bytes.extend_from_slice(&(output.triangles.len() as u32).to_le_bytes());
    for value in output.bounds_min.iter().chain(&output.bounds_max).chain(&output.sphere_center).chain([&output.sphere_radius]) {
        bytes.extend_from_slice(&value.to_le_bytes());
    }
    for vertex in &output.vertices {
        for q in vertex.quantized {
            bytes.extend_from_slice(&q.to_le_bytes());
        }
        for n in vertex.normal {
            bytes.extend_from_slice(&n.to_le_bytes());
        }
    }
    for triangle in &output.triangles {
        for index in triangle {
            bytes.extend_from_slice(&index.to_le_bytes());
        }
    }
    bytes
}

#[cfg(test)]
pub fn decode_geometric(bytes: &[u8]) -> Result<(Header, GeometricOutput), String> {
    let header = Header::read(bytes)?;
    if header.kind == Kind::Plain {
        return Err(format!("kind {} is not a mesh", header.kind));
    }
    let mut reader = Reader::new(bytes, Header::BYTES);
    let vertex_count = reader.u32("vertex count")? as usize;
    let triangle_count = reader.u32("triangle count")? as usize;
    let read3 = |reader: &mut Reader, what: &str| -> Result<[f32; 3], String> { Ok([reader.f32(what)?, reader.f32(what)?, reader.f32(what)?]) };
    let bounds_min = read3(&mut reader, "bounds min")?;
    let bounds_max = read3(&mut reader, "bounds max")?;
    let sphere_center = read3(&mut reader, "sphere center")?;
    let sphere_radius = reader.f32("sphere radius")?;
    let mut vertices = Vec::with_capacity(vertex_count.min(1 << 20));
    for _ in 0..vertex_count {
        let quantized = [reader.u16("quantized x")?, reader.u16("quantized y")?, reader.u16("quantized z")?];
        let normal = read3(&mut reader, "normal")?;
        vertices.push(Vertex { quantized, normal });
    }
    let mut triangles = Vec::with_capacity(triangle_count.min(1 << 20));
    for _ in 0..triangle_count {
        let mut t = [0_u32; 3];
        for index in &mut t {
            *index = reader.u32("triangle index")?;
            if *index as usize >= vertex_count {
                return Err(format!("triangle index {index} out of {vertex_count} vertices"));
            }
        }
        triangles.push(t);
    }
    reader.finish()?;
    Ok((header, GeometricOutput { bounds_min, bounds_max, sphere_center, sphere_radius, vertices, triangles }))
}

// The action cache.

/// What the cache counted: hits served from memory, hits served from the
/// disk mirror, misses.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CacheCounters {
    pub memory_hits: u64,
    pub disk_hits: u64,
    pub misses: u64,
}

/// Action keys to outputs, in memory and, when mirrored, as one file per
/// key under the mirror directory so that another process could hit it.
pub struct Cache {
    memory: Mutex<HashMap<u64, Arc<Vec<u8>>>>,
    mirror: Option<PathBuf>,
    memory_hits: AtomicU64,
    disk_hits: AtomicU64,
    misses: AtomicU64,
}

impl Cache {
    pub fn in_memory() -> Cache {
        Cache { memory: Mutex::new(HashMap::new()), mirror: None, memory_hits: AtomicU64::new(0), disk_hits: AtomicU64::new(0), misses: AtomicU64::new(0) }
    }

    pub fn mirrored(directory: &Path) -> Result<Cache, String> {
        fs::create_dir_all(directory).map_err(|error| format!("creating the cache mirror {}: {error}", directory.display()))?;
        Ok(Cache { mirror: Some(directory.to_path_buf()), ..Cache::in_memory() })
    }

    fn mirror_path(directory: &Path, key: u64) -> PathBuf {
        directory.join(format!("{key:016x}.bin"))
    }

    pub fn get(&self, key: u64) -> Result<Option<Arc<Vec<u8>>>, String> {
        if let Some(bytes) = self.memory.lock().expect("the cache is never poisoned").get(&key) {
            self.memory_hits.fetch_add(1, Ordering::Relaxed);
            return Ok(Some(Arc::clone(bytes)));
        }
        if let Some(directory) = &self.mirror {
            let path = Self::mirror_path(directory, key);
            match fs::read(&path) {
                Ok(bytes) => {
                    let bytes = Arc::new(bytes);
                    self.memory.lock().expect("the cache is never poisoned").insert(key, Arc::clone(&bytes));
                    self.disk_hits.fetch_add(1, Ordering::Relaxed);
                    return Ok(Some(bytes));
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => return Err(format!("reading the cache mirror {}: {error}", path.display())),
            }
        }
        self.misses.fetch_add(1, Ordering::Relaxed);
        Ok(None)
    }

    pub fn put(&self, key: u64, bytes: Arc<Vec<u8>>) -> Result<(), String> {
        if let Some(directory) = &self.mirror {
            let path = Self::mirror_path(directory, key);
            fs::write(&path, bytes.as_slice()).map_err(|error| format!("writing the cache mirror {}: {error}", path.display()))?;
        }
        self.memory.lock().expect("the cache is never poisoned").insert(key, bytes);
        Ok(())
    }

    pub fn counters(&self) -> CacheCounters {
        CacheCounters {
            memory_hits: self.memory_hits.load(Ordering::Relaxed),
            disk_hits: self.disk_hits.load(Ordering::Relaxed),
            misses: self.misses.load(Ordering::Relaxed),
        }
    }


    /// The files of the disk mirror and their total bytes.
    pub fn mirror_files(&self) -> Result<(usize, u64), String> {
        let Some(directory) = &self.mirror else { return Ok((0, 0)) };
        let entries = fs::read_dir(directory).map_err(|error| format!("listing the cache mirror {}: {error}", directory.display()))?;
        let mut files = 0;
        let mut bytes = 0;
        for entry in entries {
            let entry = entry.map_err(|error| format!("listing the cache mirror {}: {error}", directory.display()))?;
            let metadata = entry.metadata().map_err(|error| format!("reading {}: {error}", entry.path().display()))?;
            if metadata.is_file() {
                files += 1;
                bytes += metadata.len();
            }
        }
        Ok((files, bytes))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sources::{MeshSource, SplitMix64};

    const TABLE: &str = "7;1;2;3;4;seven\n3;10;20;-30;40;three\n7;5;6;7;8;duplicate\n1;255;65535;-2147483648;-9223372036854775808;one\n";

    fn plain_bytes(version: u32) -> Vec<u8> {
        import(Kind::Plain, version, &Settings::DEFAULT, TABLE.as_bytes(), &[]).unwrap()
    }

    fn mesh_bytes(vertices: usize, triangles: usize, seed: u64) -> Vec<u8> {
        MeshSource::generate(&mut SplitMix64::new(seed), vertices, triangles).encode()
    }

    #[test]
    fn the_plain_canonical_bytes_round_trip() {
        let bytes = plain_bytes(3);
        let (header, output) = decode_plain(&bytes).unwrap();
        assert_eq!(header, Header { kind: Kind::Plain, format_version: FORMAT_VERSION, importer_version: 3 });
        // Sorted by key, the duplicate key 7 keeps the first record of the file.
        let keys: Vec<u32> = output.records.iter().map(|r| r.key).collect();
        assert_eq!(keys, [1, 3, 7]);
        assert_eq!(output.records[2].name, "seven");
        assert_eq!(output.records[0], Record { key: 1, a: 255, b: 65535, c: i32::MIN, d: i64::MIN, name: "one".to_string() });
        assert_eq!(output.prefix_sums, [i64::from(i32::MIN), i64::from(i32::MIN) - 30, i64::from(i32::MIN) - 27]);
        assert_eq!(output.checksum, reference_workload::fnv1a(&output.record_bytes()));
        assert_eq!(content_hash(&bytes), reference_workload::fnv1a(&bytes));
    }

    #[test]
    fn truncated_or_corrupt_plain_bytes_are_refused_with_a_reason() {
        let bytes = plain_bytes(1);
        assert!(decode_plain(&bytes[..Header::BYTES - 1]).unwrap_err().contains("header"));
        assert!(decode_plain(&bytes[..bytes.len() - 3]).unwrap_err().contains("truncated"));
        let mut magic = bytes.clone();
        magic[0] = b'X';
        assert!(decode_plain(&magic).unwrap_err().contains("magic"));
        let mut kind = bytes.clone();
        kind[6] = Kind::Geometric.tag();
        assert!(decode_plain(&kind).unwrap_err().contains("kind"));
        let mut trailing = bytes.clone();
        trailing.push(0);
        assert!(decode_plain(&trailing).unwrap_err().contains("trailing"));
        let mut checksum = bytes.clone();
        let last = checksum.len() - 1;
        checksum[last] ^= 0xff;
        assert!(decode_plain(&checksum).unwrap_err().contains("checksum"));
    }

    #[test]
    fn the_plain_importer_refuses_bad_sources_with_a_reason() {
        let refuse = |text: &str| import(Kind::Plain, 1, &Settings::DEFAULT, text.as_bytes(), &[]).unwrap_err();
        assert!(refuse("1;2;3;4;5\n").contains("6 fields"));
        assert!(refuse("x;2;3;4;5;name\n").contains("key"));
        assert!(refuse("1;256;3;4;5;name\n").contains("field a"));
        assert!(refuse("1;2;3;4;5;Name\n").contains("name"));
        assert!(refuse("1;2;3;4;5;abcdefghijklmnopq\n").contains("name"));
        assert!(refuse("1;2;3;4;5;\n").contains("name"));
        assert!(refuse("").contains("no record"));
        assert!(import(Kind::Plain, 1, &Settings::DEFAULT, TABLE.as_bytes(), &[&[1, 2, 3]]).unwrap_err().contains("dependenc"));
        assert!(import(Kind::Plain, 1, &Settings::DEFAULT, &[0xff, 0xfe], &[]).unwrap_err().contains("UTF-8"));
    }

    #[test]
    fn the_geometric_canonical_bytes_round_trip() {
        let transform = plain_bytes(1);
        let source = mesh_bytes(50, 80, 11);
        let bytes = import(Kind::Geometric, 2, &Settings::DEFAULT, &source, &[&transform]).unwrap();
        let (header, output) = decode_geometric(&bytes).unwrap();
        assert_eq!(header, Header { kind: Kind::Geometric, format_version: FORMAT_VERSION, importer_version: 2 });
        assert_eq!(output.vertices.len(), 50);
        assert_eq!(output.triangles.len(), 80);
        for axis in 0..3 {
            assert!(output.bounds_min[axis] <= output.bounds_max[axis]);
            assert_eq!(output.sphere_center[axis], (output.bounds_min[axis] + output.bounds_max[axis]) * 0.5);
        }
        assert!(output.sphere_radius > 0.0);
        // Vertices are in canonical order: by quantized position, so the sequence is sorted.
        let quantized: Vec<[u16; 3]> = output.vertices.iter().map(|v| v.quantized).collect();
        assert!(quantized.windows(2).all(|w| w[0] <= w[1]));
        assert!(quantized.iter().any(|q| q.contains(&0)) && quantized.iter().any(|q| q.contains(&u16::MAX)));
        for vertex in &output.vertices {
            let length = vertex.normal.iter().map(|c| c * c).sum::<f32>().sqrt();
            assert!(length == 0.0 || (length - 1.0).abs() < 1e-3, "{length}");
        }
        assert!(output.triangles.iter().all(|t| t.iter().all(|&i| (i as usize) < 50)));
        let control = import(Kind::Control, 2, &Settings::DEFAULT, &source, &[&transform]).unwrap();
        assert_eq!(decode_geometric(&control).unwrap().0.kind, Kind::Control);
        assert_ne!(control[..Header::BYTES], bytes[..Header::BYTES], "the header carries the kind");
    }

    #[test]
    fn truncated_or_corrupt_geometric_bytes_are_refused_with_a_reason() {
        let transform = plain_bytes(1);
        let bytes = import(Kind::Geometric, 1, &Settings::DEFAULT, &mesh_bytes(20, 30, 12), &[&transform]).unwrap();
        assert!(decode_geometric(&bytes[..bytes.len() - 1]).unwrap_err().contains("truncated"));
        assert!(decode_geometric(&bytes[..5]).unwrap_err().contains("header"));
        let mut kind = bytes.clone();
        kind[6] = Kind::Plain.tag();
        assert!(decode_geometric(&kind).unwrap_err().contains("kind"));
        let mut index = bytes.clone();
        let last = index.len() - 1;
        index[last] = 0xff;
        assert!(decode_geometric(&index).unwrap_err().contains("index"));
    }

    #[test]
    fn the_geometric_importer_refuses_bad_sources_and_dependencies_with_a_reason() {
        let transform = plain_bytes(1);
        let source = mesh_bytes(20, 30, 13);
        let refuse = |source: &[u8], dependencies: &[&[u8]]| import(Kind::Geometric, 1, &Settings::DEFAULT, source, dependencies).unwrap_err();
        assert!(refuse(&source, &[]).contains("one dependency"));
        assert!(refuse(&source, &[&transform, &transform]).contains("one dependency"));
        assert!(refuse(&source[..source.len() - 2], &[&transform]).contains("mesh source"));
        assert!(refuse(&source, &[&source]).contains("transform"));
        let two_records = import(Kind::Plain, 1, &Settings::DEFAULT, b"", &[]).unwrap_err();
        assert!(two_records.contains("no record"));
    }

    #[test]
    fn the_importers_are_pure_functions_of_their_inputs() {
        let transform = plain_bytes(1);
        let other_transform = import(Kind::Plain, 1, &Settings::DEFAULT, b"2;9;3;4;5;other\n", &[]).unwrap();
        assert_ne!(transform_of(&transform).unwrap(), transform_of(&other_transform).unwrap());
        let source = mesh_bytes(60, 90, 14);
        for kind in [Kind::Geometric, Kind::Control] {
            let first = import(kind, 1, &Settings::DEFAULT, &source, &[&transform]).unwrap();
            let second = import(kind, 1, &Settings::DEFAULT, &source, &[&transform]).unwrap();
            assert_eq!(first, second, "{kind}");
            let third = import(kind, 1, &Settings::DEFAULT, &source, &[&other_transform]).unwrap();
            assert_ne!(first, third, "{kind}: the dependency's output is an input");
        }
        assert_eq!(plain_bytes(1), plain_bytes(1));
        assert_ne!(plain_bytes(1), plain_bytes(2), "the importer version is in the header");
    }

    #[test]
    fn the_transform_comes_from_the_first_record_of_the_table() {
        let transform = transform_of(&plain_bytes(1)).unwrap();
        // Record with key 1: a 255, b 65535, c i32::MIN, d i64::MIN.
        assert_eq!(transform.scale, 1.0 + (65535 % 8) as f32 * 0.25);
        assert_eq!(transform.offset, [(i32::MIN.rem_euclid(64)) as f32, 255.0 * 0.5, (i64::MIN.rem_euclid(128)) as f32 * 0.125]);
    }

    #[test]
    fn the_action_key_changes_when_any_input_changes_and_only_then() {
        let settings = Settings::DEFAULT;
        let other_settings = Settings { quantization_bits: 12 };
        let base = ActionInputs {
            source_digest: 1,
            importer: ImporterIdentity { name: "plain", version: 1 },
            settings: &settings,
            target_platform: TARGET_PLATFORM,
            dependency_hashes: &[10, 20],
        };
        let key = action_key(&base);
        assert_eq!(key, action_key(&base));
        let variants = [
            ActionInputs { source_digest: 2, ..base },
            ActionInputs { importer: ImporterIdentity { name: "plain2", version: 1 }, ..base },
            ActionInputs { importer: ImporterIdentity { name: "plain", version: 2 }, ..base },
            ActionInputs { settings: &other_settings, ..base },
            ActionInputs { target_platform: "other", ..base },
            ActionInputs { dependency_hashes: &[20, 10], ..base },
            ActionInputs { dependency_hashes: &[10], ..base },
            ActionInputs { dependency_hashes: &[10, 21], ..base },
        ];
        let mut keys: Vec<u64> = variants.iter().map(action_key).collect();
        keys.push(key);
        keys.sort_unstable();
        keys.dedup();
        assert_eq!(keys.len(), variants.len() + 1, "every variant has its own key");
    }

    #[test]
    fn the_cache_serves_memory_then_the_disk_mirror() {
        let directory = std::env::temp_dir().join(format!("asset-import-parallel-cache-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&directory);
        let cache = Cache::mirrored(&directory).unwrap();
        assert!(cache.get(42).unwrap().is_none());
        cache.put(42, std::sync::Arc::new(vec![1, 2, 3])).unwrap();
        assert_eq!(cache.get(42).unwrap().as_deref(), Some(&vec![1, 2, 3]));
        assert_eq!(cache.counters(), CacheCounters { memory_hits: 1, disk_hits: 0, misses: 1 });
        let second = Cache::mirrored(&directory).unwrap();
        assert_eq!(second.get(42).unwrap().as_deref(), Some(&vec![1, 2, 3]), "a second cache on the same mirror hits the disk");
        assert_eq!(second.get(42).unwrap().as_deref(), Some(&vec![1, 2, 3]), "and then memory");
        assert_eq!(second.counters(), CacheCounters { memory_hits: 1, disk_hits: 1, misses: 0 });
        assert_eq!(second.mirror_files().unwrap(), (1, 3));
        let memory_only = Cache::in_memory();
        memory_only.put(7, std::sync::Arc::new(vec![9])).unwrap();
        assert_eq!(memory_only.get(7).unwrap().as_deref(), Some(&vec![9]));
        assert_eq!(memory_only.mirror_files().unwrap(), (0, 0));
        std::fs::remove_dir_all(&directory).unwrap();
    }
}
