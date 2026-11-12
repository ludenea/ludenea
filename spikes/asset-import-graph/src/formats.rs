//! The source formats, the importers and the canonical parts of the import
//! graph spike. Throwaway, never product code.
//!
//! Three kinds of source, as a content root holds them:
//!
//! - a table: text records in a format the pipeline does not own, with a
//!   sidecar file that holds its identifier; one asset;
//! - a sheet: a header of text lines and raw pixels, with a sidecar; it holds
//!   several assets (sprites, and clips that name sprites of the same sheet by
//!   their key), each with an identifier derived from the sheet's and its key;
//! - a scene: text in a format the pipeline owns, with its identifier in its
//!   first line and no sidecar; entity prototypes that name component types
//!   of the schema and may use other assets.
//!
//! An importer is two functions in plain data. The describe function gives,
//! from the source bytes, the assets the source holds and the dependencies
//! known without doing the work. The import function gives the canonical part
//! of one asset; it reads nothing but its source and what it asks of its
//! context by identifier or by name, which records every such read; where it
//! asks for an artifact that is not built yet, it ends with a request. The
//! context of a canonical import holds no target. A presentation payload is
//! made by an encoder that mixes the index of the worker that ran it into its
//! output: it is deliberately not reproducible.

use std::collections::BTreeMap;
use std::sync::Arc;

use reference_workload::Fnv1a;

pub type AssetId = u128;

/// The digest of a list of byte strings, each prefixed by its length.
pub fn digest(parts: &[&[u8]]) -> u64 {
    let mut hasher = Fnv1a::new();
    for part in parts {
        hasher.write(&(part.len() as u64).to_le_bytes());
        hasher.write(part);
    }
    hasher.finish()
}

/// The top bit of an identifier separates the derived ones from the drawn ones.
const DERIVED: u128 = 1 << 127;

/// A drawn identifier, from two words of a random stream.
pub fn drawn_id(high: u64, low: u64) -> AssetId {
    ((u128::from(high) << 64) | u128::from(low | 1)) & !DERIVED
}

/// The identifier of a sub-asset: derived from its source's identifier and
/// from its key, a stable string the importer takes from the source.
pub fn derived_id(source: AssetId, key: &str) -> AssetId {
    let source = source.to_le_bytes();
    let high = digest(&[b"derived, high word", &source, key.as_bytes()]);
    let low = digest(&[b"derived, low word", &source, key.as_bytes()]);
    ((u128::from(high) << 64) | u128::from(low)) | DERIVED
}

pub fn id_text(id: AssetId) -> String {
    format!("{id:032x}")
}

pub fn parse_id(text: &str) -> Result<AssetId, String> {
    u128::from_str_radix(text, 16).map_err(|_| format!("`{text}` is no identifier"))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SourceKind {
    Table,
    Sheet,
    Scene,
}

impl SourceKind {
    pub fn of_extension(extension: &str) -> Option<SourceKind> {
        match extension {
            "tbl" => Some(SourceKind::Table),
            "sheet" => Some(SourceKind::Sheet),
            "scene" => Some(SourceKind::Scene),
            _ => None,
        }
    }

    /// The importer's stable name and its version: inputs of every action key.
    pub fn importer(self) -> (&'static str, u32) {
        match self {
            SourceKind::Table => ("spike.table", 1),
            SourceKind::Sheet => ("spike.sheet", 1),
            SourceKind::Scene => ("spike.scene", 1),
        }
    }

    /// Whether the identifier is beside the source, in a sidecar file.
    pub fn has_sidecar(self) -> bool {
        self != SourceKind::Scene
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum AssetKind {
    Table,
    Sprite,
    Clip,
    Scene,
}

impl AssetKind {
    pub fn tag(self) -> u8 {
        match self {
            AssetKind::Table => 1,
            AssetKind::Sprite => 2,
            AssetKind::Clip => 3,
            AssetKind::Scene => 4,
        }
    }

    pub fn from_tag(tag: u8) -> Result<AssetKind, String> {
        [AssetKind::Table, AssetKind::Sprite, AssetKind::Clip, AssetKind::Scene]
            .into_iter()
            .find(|kind| kind.tag() == tag)
            .ok_or_else(|| format!("asset kind tag {tag}"))
    }

    /// The schema entry of the asset type: an input of the canonical action's key.
    pub fn entry(self) -> &'static str {
        match self {
            AssetKind::Table => "asset/table",
            AssetKind::Sprite => "asset/sprite",
            AssetKind::Clip => "asset/clip",
            AssetKind::Scene => "asset/scene",
        }
    }
}

/// The version of the canonical form: an input of every canonical action's key.
pub const FORM_VERSION: u32 = 1;

/// The schema: per entry, its fields in order. An asset type's entry lists
/// the fields of its canonical part; a component type's, the fields a scene
/// may set.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Schema {
    pub entries: BTreeMap<String, Vec<String>>,
}

impl Schema {
    pub fn standard() -> Schema {
        let entries = [
            ("asset/table", vec!["count", "records"]),
            ("asset/sprite", vec!["width", "height", "histogram", "checksum"]),
            ("asset/clip", vec!["rate", "frames"]),
            ("asset/scene", vec!["prototypes"]),
            ("component/position", vec!["x", "y"]),
            ("component/velocity", vec!["dx", "dy"]),
            ("component/health", vec!["points"]),
            ("component/sprite", vec!["frame"]),
        ];
        let owned = |(name, fields): (&str, Vec<&str>)| (name.to_string(), fields.into_iter().map(str::to_string).collect());
        Schema { entries: entries.into_iter().map(owned).collect() }
    }

    /// The same schema with one more field in an entry.
    pub fn with_field(&self, entry: &str, field: &str) -> Schema {
        let mut schema = self.clone();
        schema.entries.get_mut(entry).expect("an entry of the schema").push(field.to_string());
        schema
    }

    pub fn entry_digest(&self, name: &str) -> Option<u64> {
        let fields = self.entries.get(name)?;
        let mut parts: Vec<&[u8]> = vec![name.as_bytes()];
        parts.extend(fields.iter().map(String::as_bytes));
        Some(digest(&parts))
    }
}

// ---------------------------------------------------------------------------
// The context of a canonical import
// ---------------------------------------------------------------------------

/// A read an import made through its context, with the hash it read.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Read {
    Artifact(AssetId, u64),
    Schema(String, u64),
}

/// How an import ends when it does not give a part.
#[derive(Debug, PartialEq, Eq)]
pub enum Stop {
    /// It asked for the canonical part of an asset that is not built yet: it
    /// is run again from its start once the part exists.
    Request(AssetId),
    Failed(String),
}

impl From<String> for Stop {
    fn from(reason: String) -> Stop {
        Stop::Failed(reason)
    }
}

/// What a pass has built so far.
pub trait Built {
    fn artifact(&self, id: AssetId) -> Option<(u64, Arc<Vec<u8>>)>;
}

/// What a canonical import may ask for beyond its source: the canonical part
/// of another asset by identifier, an entry of the schema by name. No target.
pub struct Context<'a> {
    built: &'a dyn Built,
    schema: &'a Schema,
    pub reads: Vec<Read>,
}

impl<'a> Context<'a> {
    pub fn new(built: &'a dyn Built, schema: &'a Schema) -> Context<'a> {
        Context { built, schema, reads: Vec::new() }
    }

    pub fn artifact(&mut self, id: AssetId) -> Result<Arc<Vec<u8>>, Stop> {
        let (hash, bytes) = self.built.artifact(id).ok_or(Stop::Request(id))?;
        let read = Read::Artifact(id, hash);
        if !self.reads.contains(&read) {
            self.reads.push(read);
        }
        Ok(bytes)
    }

    pub fn entry(&mut self, name: &str) -> Result<&'a [String], Stop> {
        let fields = self.schema.entries.get(name).ok_or_else(|| format!("no schema entry `{name}`"))?;
        let read = Read::Schema(name.to_string(), self.schema.entry_digest(name).expect("an entry"));
        if !self.reads.contains(&read) {
            self.reads.push(read);
        }
        Ok(fields)
    }
}

/// Where an importer places a request for an artifact: before it computes,
/// as the interface asks, or after, where the work done is run twice.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Placement {
    Before,
    After,
}

// ---------------------------------------------------------------------------
// Identity and description
// ---------------------------------------------------------------------------

fn text(bytes: &[u8]) -> Result<&str, String> {
    std::str::from_utf8(bytes).map_err(|_| "the source is not text".to_string())
}

/// The text of a sidecar file: the identifier, then the importer's name.
pub fn sidecar_text(id: AssetId, kind: SourceKind) -> String {
    format!("identifier = {}\nimporter = {}\n", id_text(id), kind.importer().0)
}

/// The identifier of a source: in its sidecar, or in its own first line for
/// a format the pipeline owns.
pub fn identity(kind: SourceKind, source: &[u8], sidecar: Option<&[u8]>) -> Result<AssetId, String> {
    if kind.has_sidecar() {
        let sidecar = text(sidecar.ok_or("the source has no sidecar: it has no identity")?)?;
        let line = sidecar.lines().find_map(|line| line.strip_prefix("identifier = "));
        parse_id(line.ok_or("the sidecar holds no identifier")?)
    } else {
        let first = source.split(|byte| *byte == b'\n').next().unwrap_or_default();
        parse_id(text(first)?.strip_prefix("scene ").ok_or("the scene does not start with its identifier")?)
    }
}

/// The settings a sidecar holds: every line but the identifier's.
pub fn settings(sidecar: Option<&[u8]>) -> Vec<u8> {
    let Some(Ok(sidecar)) = sidecar.map(text) else { return Vec::new() };
    sidecar.lines().filter(|line| !line.starts_with("identifier = ")).collect::<Vec<_>>().join("\n").into_bytes()
}

/// One asset a source holds, as the describe function gives it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Described {
    /// The key of a sub-asset; empty where the source is one asset.
    pub key: String,
    pub kind: AssetKind,
    /// The dependencies known without doing the work.
    pub dependencies: Vec<AssetId>,
}

pub fn encode_description(assets: &[Described]) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(&(assets.len() as u32).to_le_bytes());
    for asset in assets {
        out.push(asset.kind.tag());
        out.extend_from_slice(&(asset.key.len() as u32).to_le_bytes());
        out.extend_from_slice(asset.key.as_bytes());
        out.extend_from_slice(&(asset.dependencies.len() as u32).to_le_bytes());
        for dependency in &asset.dependencies {
            out.extend_from_slice(&dependency.to_le_bytes());
        }
    }
    out
}

pub fn decode_description(bytes: &[u8]) -> Result<Vec<Described>, String> {
    let mut reader = Reader::new(bytes);
    let mut assets = Vec::new();
    for _ in 0..reader.u32()? {
        let kind = AssetKind::from_tag(reader.u8()?)?;
        let key = reader.string()?;
        let mut dependencies = Vec::new();
        for _ in 0..reader.u32()? {
            dependencies.push(reader.id()?);
        }
        assets.push(Described { key, kind, dependencies });
    }
    reader.end()?;
    Ok(assets)
}

pub struct Reader<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl<'a> Reader<'a> {
    pub fn new(bytes: &'a [u8]) -> Reader<'a> {
        Reader { bytes, at: 0 }
    }

    pub fn take(&mut self, len: usize) -> Result<&'a [u8], String> {
        let end = self.at.checked_add(len).filter(|end| *end <= self.bytes.len());
        let end = end.ok_or_else(|| format!("truncated at byte {}", self.at))?;
        let taken = &self.bytes[self.at..end];
        self.at = end;
        Ok(taken)
    }

    pub fn u8(&mut self) -> Result<u8, String> {
        Ok(self.take(1)?[0])
    }

    pub fn u32(&mut self) -> Result<u32, String> {
        Ok(u32::from_le_bytes(self.take(4)?.try_into().expect("four bytes")))
    }

    pub fn u64(&mut self) -> Result<u64, String> {
        Ok(u64::from_le_bytes(self.take(8)?.try_into().expect("eight bytes")))
    }

    pub fn id(&mut self) -> Result<AssetId, String> {
        Ok(u128::from_le_bytes(self.take(16)?.try_into().expect("sixteen bytes")))
    }

    pub fn string(&mut self) -> Result<String, String> {
        let len = self.u32()? as usize;
        Ok(text(self.take(len)?)?.to_string())
    }

    pub fn end(&self) -> Result<(), String> {
        if self.at == self.bytes.len() { Ok(()) } else { Err(format!("{} trailing bytes", self.bytes.len() - self.at)) }
    }
}

// ---------------------------------------------------------------------------
// The canonical parts
// ---------------------------------------------------------------------------

const MAGIC: &[u8; 4] = b"LGCP";
/// Every canonical part starts with the magic, its kind and a count.
const COUNT_AT: usize = 8;

fn header(kind: AssetKind, count: u32) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(MAGIC);
    out.extend_from_slice(&[kind.tag(), 0, 0, 0]);
    out.extend_from_slice(&count.to_le_bytes());
    out
}

/// The count every canonical part carries after its header.
pub fn part_count(part: &[u8]) -> Result<u32, String> {
    let mut reader = Reader::new(part);
    if reader.take(4)? != MAGIC {
        return Err("not a canonical part".to_string());
    }
    reader.take(COUNT_AT - 4)?;
    reader.u32()
}

/// The validator of a canonical part: the pipeline runs it on every part an
/// importer returns, before the part is hashed and stored.
pub fn validate(kind: AssetKind, part: &[u8]) -> Result<(), String> {
    let mut reader = Reader::new(part);
    if reader.take(4)? != MAGIC || reader.u8()? != kind.tag() {
        return Err(format!("not a canonical part of {}", kind.entry()));
    }
    reader.take(3)?;
    let count = reader.u32()?;
    match kind {
        AssetKind::Table => {
            for _ in 0..count {
                reader.take(12)?;
                let name = reader.u8()? as usize;
                reader.take(name)?;
            }
        }
        AssetKind::Sprite => {
            reader.take(4 + 16 * 4 + 8)?;
        }
        AssetKind::Clip => {
            reader.take(4)?;
            if reader.u32()? != count {
                return Err("a clip names as many siblings as it has frames".to_string());
            }
            reader.take(count as usize * 16)?;
        }
        AssetKind::Scene => {
            for _ in 0..count {
                reader.string()?;
                reader.take(8)?;
                let fields = reader.u32()? as usize;
                reader.take(fields * 8)?;
                if reader.u8()? == 1 {
                    reader.take(20)?;
                }
            }
        }
    }
    reader.end()
}

fn table_values(part: &[u8]) -> Result<Vec<i64>, String> {
    let count = part_count(part)?;
    let mut reader = Reader::new(part);
    reader.take(COUNT_AT + 4)?;
    let mut values = Vec::with_capacity(count as usize);
    for _ in 0..count {
        reader.take(4)?;
        values.push(reader.u64()? as i64);
        let name = reader.u8()? as usize;
        reader.take(name)?;
    }
    Ok(values)
}

// ---------------------------------------------------------------------------
// The table importer
// ---------------------------------------------------------------------------

/// A table: one record per line, `key;value;name`; a line that starts with
/// `#` is a comment. The canonical part holds the records sorted by key, the
/// first of each key kept.
fn import_table(source: &[u8]) -> Result<Vec<u8>, String> {
    let mut records: Vec<(u32, i64, &str)> = Vec::new();
    for (number, line) in text(source)?.lines().enumerate() {
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let mut fields = line.split(';');
        let (Some(key), Some(value), Some(name), None) = (fields.next(), fields.next(), fields.next(), fields.next()) else {
            return Err(format!("line {}: three fields expected", number + 1));
        };
        let key = key.parse().map_err(|_| format!("line {}: `{key}` is no key", number + 1))?;
        let value = value.parse().map_err(|_| format!("line {}: `{value}` is no value", number + 1))?;
        if name.len() > 255 {
            return Err(format!("line {}: the name is too long", number + 1));
        }
        records.push((key, value, name));
    }
    records.sort_by_key(|record| record.0);
    records.dedup_by_key(|record| record.0);
    let mut out = header(AssetKind::Table, records.len() as u32);
    for (key, value, name) in records {
        out.extend_from_slice(&key.to_le_bytes());
        out.extend_from_slice(&value.to_le_bytes());
        out.push(name.len() as u8);
        out.extend_from_slice(name.as_bytes());
    }
    Ok(out)
}

// ---------------------------------------------------------------------------
// The sheet importer
// ---------------------------------------------------------------------------

struct SpriteLine<'a> {
    key: &'a str,
    width: u32,
    height: u32,
    offset: usize,
    palette: AssetId,
    /// The describe function does not report the palette of a late sprite:
    /// the import discovers it.
    late: bool,
}

struct ClipLine<'a> {
    key: &'a str,
    rate: u32,
    frames: Vec<&'a str>,
}

struct Sheet<'a> {
    sprites: Vec<SpriteLine<'a>>,
    clips: Vec<ClipLine<'a>>,
    pixels: &'a [u8],
}

const PIXELS_MARK: &[u8] = b"\npixels\n";

/// A sheet: a header of lines (`sprite key width height offset palette
/// declared|late`, `clip key rate frame...`), a line `pixels`, then the raw
/// pixels of every sprite, four bytes per pixel.
fn parse_sheet(source: &[u8]) -> Result<Sheet<'_>, String> {
    let mark = source.windows(PIXELS_MARK.len()).position(|window| window == PIXELS_MARK).ok_or("the sheet has no pixels")?;
    let mut sheet = Sheet { sprites: Vec::new(), clips: Vec::new(), pixels: &source[mark + PIXELS_MARK.len()..] };
    for line in text(&source[..mark])?.lines().skip(1) {
        let words: Vec<&str> = line.split(' ').collect();
        let number = |word: &str| word.parse::<u32>().map_err(|_| format!("`{word}` is no number in `{line}`"));
        match words.as_slice() {
            ["sprite", key, width, height, offset, palette, mode] => sheet.sprites.push(SpriteLine {
                key,
                width: number(width)?,
                height: number(height)?,
                offset: number(offset)? as usize,
                palette: parse_id(palette)?,
                late: *mode == "late",
            }),
            ["clip", key, rate, frames @ ..] => sheet.clips.push(ClipLine { key, rate: number(rate)?, frames: frames.to_vec() }),
            _ => return Err(format!("`{line}` is no line of a sheet")),
        }
    }
    Ok(sheet)
}

fn sprite_pixels<'a>(sheet: &Sheet<'a>, sprite: &SpriteLine) -> Result<&'a [u8], String> {
    let len = sprite.width as usize * sprite.height as usize * 4;
    sheet.pixels.get(sprite.offset..sprite.offset + len).ok_or_else(|| format!("the pixels of `{}` are out of the sheet", sprite.key))
}

/// The histogram of a sprite's pixels: the half of the work that needs no palette.
fn histogram(pixels: &[u8]) -> [u32; 16] {
    let mut buckets = [0_u32; 16];
    for byte in pixels {
        buckets[(byte >> 4) as usize] += 1;
    }
    buckets
}

fn import_sprite(context: &mut Context, sheet: &Sheet, key: &str, placement: Placement) -> Result<Vec<u8>, Stop> {
    let sprite = sheet.sprites.iter().find(|sprite| sprite.key == key).ok_or_else(|| format!("no sprite `{key}` in the sheet"))?;
    let pixels = sprite_pixels(sheet, sprite)?;
    let (palette, buckets) = match placement {
        Placement::Before => {
            let palette = context.artifact(sprite.palette)?;
            (palette, histogram(pixels))
        }
        Placement::After => {
            let buckets = histogram(pixels);
            (context.artifact(sprite.palette)?, buckets)
        }
    };
    let values = table_values(&palette)?;
    if values.is_empty() {
        return Err(Stop::Failed(format!("the palette of `{key}` is empty")));
    }
    let mut checksum = 0_u64;
    for byte in pixels {
        checksum = checksum.wrapping_mul(31).wrapping_add(values[*byte as usize % values.len()] as u64);
    }
    let mut out = header(AssetKind::Sprite, sprite.width);
    out.extend_from_slice(&sprite.height.to_le_bytes());
    for bucket in buckets {
        out.extend_from_slice(&bucket.to_le_bytes());
    }
    out.extend_from_slice(&checksum.to_le_bytes());
    Ok(out)
}

// ---------------------------------------------------------------------------
// The scene importer
// ---------------------------------------------------------------------------

struct Prototype<'a> {
    name: &'a str,
    component: &'a str,
    fields: Vec<(&'a str, i64)>,
    uses: Option<(AssetId, bool)>,
}

/// A scene: its identifier, then one prototype per line: `prototype name
/// component field=value... [uses identifier | late-uses identifier]`.
fn parse_scene(source: &[u8]) -> Result<Vec<Prototype<'_>>, String> {
    let mut prototypes = Vec::new();
    for line in text(source)?.lines().skip(1) {
        let mut words = line.split(' ');
        let (Some("prototype"), Some(name), Some(component)) = (words.next(), words.next(), words.next()) else {
            return Err(format!("`{line}` is no prototype"));
        };
        let mut prototype = Prototype { name, component, fields: Vec::new(), uses: None };
        while let Some(word) = words.next() {
            if word == "uses" || word == "late-uses" {
                let id = parse_id(words.next().ok_or_else(|| format!("`{line}` names no asset"))?)?;
                prototype.uses = Some((id, word == "late-uses"));
            } else {
                let (field, value) = word.split_once('=').ok_or_else(|| format!("`{word}` is no field of `{name}`"))?;
                prototype.fields.push((field, value.parse().map_err(|_| format!("`{value}` is no value of `{name}`"))?));
            }
        }
        prototypes.push(prototype);
    }
    Ok(prototypes)
}

fn import_scene(context: &mut Context, source: &[u8], placement: Placement) -> Result<Vec<u8>, Stop> {
    let prototypes = parse_scene(source)?;
    if placement == Placement::Before {
        // What the import needs, it asks for before it computes.
        for (id, _) in prototypes.iter().filter_map(|prototype| prototype.uses) {
            context.artifact(id)?;
        }
    }
    let mut out = header(AssetKind::Scene, prototypes.len() as u32);
    for prototype in &prototypes {
        let entry = context.entry(&format!("component/{}", prototype.component))?;
        let mut values = vec![0_i64; entry.len()];
        for (field, value) in &prototype.fields {
            let at = entry.iter().position(|name| name == field);
            values[at.ok_or_else(|| format!("`{}` has no field `{field}` in `{}`", prototype.component, prototype.name))?] = *value;
        }
        out.extend_from_slice(&(prototype.name.len() as u32).to_le_bytes());
        out.extend_from_slice(prototype.name.as_bytes());
        out.extend_from_slice(&digest(&[prototype.component.as_bytes()]).to_le_bytes());
        out.extend_from_slice(&(values.len() as u32).to_le_bytes());
        for value in values {
            out.extend_from_slice(&value.to_le_bytes());
        }
        match prototype.uses {
            None => out.push(0),
            Some((id, _)) => {
                let count = part_count(&context.artifact(id)?)?;
                out.push(1);
                out.extend_from_slice(&id.to_le_bytes());
                out.extend_from_slice(&count.to_le_bytes());
            }
        }
    }
    Ok(out)
}

// ---------------------------------------------------------------------------
// The two functions of an importer, and the presentation encoder
// ---------------------------------------------------------------------------

/// The describe function: the assets a source holds and the dependencies
/// known without doing the work. With `everything`, the dependencies an
/// import would otherwise discover are declared too.
pub fn describe(kind: SourceKind, source: &[u8], everything: bool) -> Result<Vec<Described>, String> {
    match kind {
        SourceKind::Table => Ok(vec![Described { key: String::new(), kind: AssetKind::Table, dependencies: Vec::new() }]),
        SourceKind::Sheet => {
            let sheet = parse_sheet(source)?;
            let sprite = |sprite: &SpriteLine| Described {
                key: sprite.key.to_string(),
                kind: AssetKind::Sprite,
                dependencies: if sprite.late && !everything { Vec::new() } else { vec![sprite.palette] },
            };
            let clip = |clip: &ClipLine| Described { key: clip.key.to_string(), kind: AssetKind::Clip, dependencies: Vec::new() };
            Ok(sheet.sprites.iter().map(sprite).chain(sheet.clips.iter().map(clip)).collect())
        }
        SourceKind::Scene => {
            let mut dependencies: Vec<AssetId> =
                parse_scene(source)?.iter().filter_map(|prototype| prototype.uses).filter(|(_, late)| everything || !late).map(|(id, _)| id).collect();
            dependencies.sort_unstable();
            dependencies.dedup();
            Ok(vec![Described { key: String::new(), kind: AssetKind::Scene, dependencies }])
        }
    }
}

/// What the import function gives: the canonical part, and the siblings of
/// the same source it references by their key, which the pipeline resolves
/// to identifiers and appends before the part is hashed.
pub struct Imported {
    pub part: Vec<u8>,
    pub siblings: Vec<String>,
}

/// The import function: the canonical part of one asset of a source.
pub fn import(context: &mut Context, kind: SourceKind, source: &[u8], key: &str, placement: Placement) -> Result<Imported, Stop> {
    let part = |part: Vec<u8>| Imported { part, siblings: Vec::new() };
    match kind {
        SourceKind::Table => Ok(part(import_table(source)?)),
        SourceKind::Scene => Ok(part(import_scene(context, source, placement)?)),
        SourceKind::Sheet => {
            let sheet = parse_sheet(source)?;
            if let Some(clip) = sheet.clips.iter().find(|clip| clip.key == key) {
                if let Some(missing) = clip.frames.iter().find(|frame| !sheet.sprites.iter().any(|sprite| sprite.key == **frame)) {
                    return Err(Stop::Failed(format!("the clip `{key}` names `{missing}`, which the sheet does not hold")));
                }
                let mut out = header(AssetKind::Clip, clip.frames.len() as u32);
                out.extend_from_slice(&clip.rate.to_le_bytes());
                return Ok(Imported { part: out, siblings: clip.frames.iter().map(|frame| (*frame).to_string()).collect() });
            }
            Ok(part(import_sprite(context, &sheet, key, placement)?))
        }
    }
}

/// The canonical part with its sibling references resolved to identifiers.
pub fn resolve_siblings(source: AssetId, imported: Imported) -> Vec<u8> {
    let mut part = imported.part;
    if !imported.siblings.is_empty() {
        part.extend_from_slice(&(imported.siblings.len() as u32).to_le_bytes());
        for key in &imported.siblings {
            part.extend_from_slice(&derived_id(source, key).to_le_bytes());
        }
    }
    part
}

/// The identifiers a canonical part of a clip names.
pub fn clip_siblings(part: &[u8]) -> Result<Vec<AssetId>, String> {
    let mut reader = Reader::new(part);
    reader.take(COUNT_AT + 4 + 4)?;
    (0..reader.u32()?).map(|_| reader.id()).collect()
}

/// The presentation encoder: a payload for one variant, from the pixels of a
/// sprite or from the canonical part of any other asset. It mixes the index
/// of the worker that runs it into its output, so that two runs need not
/// give the same bytes: a payload is identified by its hash and nothing else.
pub fn encode_presentation(kind: SourceKind, source: &[u8], key: &str, canonical: &[u8], variant: &str, worker: usize) -> Result<Vec<u8>, String> {
    let sheet;
    let data = match kind {
        SourceKind::Sheet => {
            sheet = parse_sheet(source)?;
            match sheet.sprites.iter().find(|sprite| sprite.key == key) {
                Some(sprite) => sprite_pixels(&sheet, sprite)?,
                None => canonical,
            }
        }
        _ => canonical,
    };
    let mut out = Vec::with_capacity(data.len() + variant.len() + 16);
    out.extend_from_slice(b"LGPP");
    out.extend_from_slice(&(variant.len() as u32).to_le_bytes());
    out.extend_from_slice(variant.as_bytes());
    out.extend_from_slice(&(worker as u32).to_le_bytes());
    if variant.ends_with("high") {
        out.extend(data.iter().map(|byte| byte.rotate_left(3) ^ 0x5a));
    } else {
        out.extend(data.iter().step_by(2).map(|byte| byte.rotate_left(5) ^ 0xa5));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    struct Parts(HashMap<AssetId, Arc<Vec<u8>>>);

    impl Built for Parts {
        fn artifact(&self, id: AssetId) -> Option<(u64, Arc<Vec<u8>>)> {
            self.0.get(&id).map(|bytes| (digest(&[bytes]), bytes.clone()))
        }
    }

    const PALETTE: AssetId = 7;

    fn sheet() -> Vec<u8> {
        let mut source = format!("sheet\nsprite a 2 2 0 {} declared\nsprite b 2 2 16 {} late\nclip walk 12 a b", id_text(PALETTE), id_text(PALETTE)).into_bytes();
        source.extend_from_slice(PIXELS_MARK);
        source.extend((0..32_u8).map(|byte| byte.wrapping_mul(9)));
        source
    }

    #[test]
    fn a_table_is_sorted_and_a_comment_changes_nothing() {
        let part = import_table(b"3;30;c\n1;10;a\n# a comment\n2;20;b\n1;99;again\n").expect("a table");
        assert_eq!(part, import_table(b"# moved\n1;10;a\n2;20;b\n3;30;c\n").expect("a table"));
        assert_eq!(part_count(&part), Ok(3));
        assert_eq!(table_values(&part), Ok(vec![10, 20, 30]));
        assert!(validate(AssetKind::Table, &part).is_ok() && validate(AssetKind::Table, &part[..part.len() - 1]).is_err());
        assert!(import_table(b"1;x;a\n").is_err() && import_table(b"1;2\n").is_err());
    }

    #[test]
    fn a_sheet_describes_its_assets_and_a_late_palette_is_discovered() {
        let source = sheet();
        let described = describe(SourceKind::Sheet, &source, false).expect("a sheet");
        assert_eq!(described.iter().map(|asset| asset.key.as_str()).collect::<Vec<_>>(), ["a", "b", "walk"]);
        assert_eq!((described[0].dependencies.clone(), described[1].dependencies.clone()), (vec![PALETTE], vec![]));
        assert_eq!(describe(SourceKind::Sheet, &source, true).expect("a sheet")[1].dependencies, vec![PALETTE]);
        assert_eq!(decode_description(&encode_description(&described)), Ok(described));
        let schema = Schema::standard();
        let nothing = Parts(HashMap::new());
        for placement in [Placement::Before, Placement::After] {
            let mut context = Context::new(&nothing, &schema);
            assert_eq!(import(&mut context, SourceKind::Sheet, &source, "b", placement).err(), Some(Stop::Request(PALETTE)));
        }
        let palette = Arc::new(import_table(b"1;5;a\n2;7;b\n").expect("a table"));
        let built = Parts(HashMap::from([(PALETTE, palette.clone())]));
        let mut context = Context::new(&built, &schema);
        let before = import(&mut context, SourceKind::Sheet, &source, "b", Placement::Before).ok().expect("a sprite").part;
        assert_eq!(context.reads, vec![Read::Artifact(PALETTE, digest(&[&palette]))]);
        let after = import(&mut Context::new(&built, &schema), SourceKind::Sheet, &source, "b", Placement::After).ok().expect("a sprite").part;
        assert_eq!(before, after);
        assert!(validate(AssetKind::Sprite, &before).is_ok());
    }

    #[test]
    fn a_clip_names_the_siblings_of_its_own_source() {
        let schema = Schema::standard();
        let nothing = Parts(HashMap::new());
        let clip = |source: AssetId| {
            let imported = import(&mut Context::new(&nothing, &schema), SourceKind::Sheet, &sheet(), "walk", Placement::Before).ok().expect("a clip");
            resolve_siblings(source, imported)
        };
        let (one, other) = (clip(100), clip(200));
        assert!(validate(AssetKind::Clip, &one).is_ok());
        assert_eq!(clip_siblings(&one), Ok(vec![derived_id(100, "a"), derived_id(100, "b")]));
        assert_eq!(clip_siblings(&other), Ok(vec![derived_id(200, "a"), derived_id(200, "b")]));
        assert_ne!(one, other);
        assert!(derived_id(100, "a") & DERIVED != 0 && drawn_id(u64::MAX, 0) & DERIVED == 0);
    }

    #[test]
    fn a_scene_reads_the_schema_and_the_assets_it_uses() {
        let schema = Schema::standard();
        let table = Arc::new(import_table(b"1;5;a\n2;7;b\n").expect("a table"));
        let built = Parts(HashMap::from([(PALETTE, table)]));
        let source = format!("scene {}\nprototype crate position x=3 y=-4\nprototype bird velocity dx=1 late-uses {}\n", id_text(9), id_text(PALETTE));
        assert_eq!(identity(SourceKind::Scene, source.as_bytes(), None), Ok(9));
        assert_eq!(describe(SourceKind::Scene, source.as_bytes(), false).expect("a scene")[0].dependencies, Vec::<AssetId>::new());
        assert_eq!(describe(SourceKind::Scene, source.as_bytes(), true).expect("a scene")[0].dependencies, vec![PALETTE]);
        let mut context = Context::new(&built, &schema);
        let part = import(&mut context, SourceKind::Scene, source.as_bytes(), "", Placement::After).ok().expect("a scene").part;
        assert!(validate(AssetKind::Scene, &part).is_ok());
        assert_eq!(context.reads.len(), 3, "two schema entries and one artifact: {:?}", context.reads);
        // Another entry for a component type changes the part of a scene that reads it.
        let wider = schema.with_field("component/velocity", "damping");
        let other = import(&mut Context::new(&built, &wider), SourceKind::Scene, source.as_bytes(), "", Placement::After).ok().expect("a scene").part;
        assert_ne!(part, other);
        let bad = format!("scene {}\nprototype crate position z=1\n", id_text(9));
        assert!(matches!(import(&mut Context::new(&built, &schema), SourceKind::Scene, bad.as_bytes(), "", Placement::Before), Err(Stop::Failed(_))));
    }

    #[test]
    fn an_identity_is_read_beside_a_foreign_source_and_the_encoder_is_not_reproducible() {
        let sidecar = sidecar_text(77, SourceKind::Table);
        assert_eq!(identity(SourceKind::Table, b"", Some(sidecar.as_bytes())), Ok(77));
        assert!(identity(SourceKind::Table, b"", None).is_err());
        assert_eq!(settings(Some(sidecar.as_bytes())), b"importer = spike.table");
        let part = import_table(b"1;5;a\n").expect("a table");
        let payload = |worker| encode_presentation(SourceKind::Table, b"", "", &part, "bc/high", worker).expect("a payload");
        assert_eq!(payload(0), payload(0));
        assert_ne!(payload(0), payload(1));
        let source = sheet();
        let sprite = encode_presentation(SourceKind::Sheet, &source, "a", &part, "bc/low", 0).expect("a payload");
        assert_eq!(sprite.len(), 4 + 4 + 6 + 4 + 8, "half of the sprite's sixteen bytes");
    }
}
