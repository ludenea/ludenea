//! The pipeline of the import graph spike: the cache, the change journal, a
//! pass over a content root, the manifests and the revision. Throwaway, never
//! product code.
//!
//! An import is one canonical action per asset and one presentation action
//! per asset and variant. The graph is built in two passes on the pool: the
//! describe actions first (one per source: its facts, its digest, its
//! identity, its description), then the import actions on the dependencies
//! the descriptions and the cache's traces give.
//!
//! The cache is two maps. The content map goes from a hash to bytes. The
//! action map goes from an action key to a trace: the hashes of the outputs
//! and the reads the action made through its context, each with the hash it
//! read. A lookup is in two steps: the key matches, then every recorded read
//! still has its hash. A key names the hashes of what the action read and
//! never the keys of the actions that produced them, so that an action whose
//! inputs changed and whose output did not leaves its dependents alone.
//!
//! The key of a canonical action is the digest of the source's bytes, the
//! importer's name and version, the settings, the source's identifier and the
//! sub-asset's key (so that a copied source never takes the original's
//! parts), the schema entry of the asset type and the version of the
//! canonical form. No target enters it. The key of a presentation action
//! adds the hash of the canonical part, the variant key and the platform.
//!
//! A pass ends by writing a revision: one root document that names the
//! hashes of the domain manifest and of each presentation manifest, put in
//! place by one rename.

use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant, UNIX_EPOCH};

use crate::content::sidecar_of;
use crate::formats::{
    AssetId, AssetKind, Built, Context, Described, FORM_VERSION, Placement, Read, Reader, Schema, SourceKind, Stop, decode_description,
    derived_id, describe, digest, encode_description, encode_presentation, id_text, identity, import, resolve_siblings, settings, validate,
};
use crate::pool::{self, Outcome};

// ---------------------------------------------------------------------------
// The cache
// ---------------------------------------------------------------------------

/// What the action map holds under a key.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Trace {
    pub reads: Vec<Read>,
    pub outputs: Vec<u64>,
    /// What the action keeps beside its outputs: a description, or the
    /// dependencies a manifest lists, or a payload's length.
    pub blob: Vec<u8>,
}

impl Trace {
    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(&(self.reads.len() as u32).to_le_bytes());
        for read in &self.reads {
            match read {
                Read::Artifact(id, hash) => {
                    out.push(0);
                    out.extend_from_slice(&id.to_le_bytes());
                    out.extend_from_slice(&hash.to_le_bytes());
                }
                Read::Schema(name, hash) => {
                    out.push(1);
                    out.extend_from_slice(&(name.len() as u32).to_le_bytes());
                    out.extend_from_slice(name.as_bytes());
                    out.extend_from_slice(&hash.to_le_bytes());
                }
            }
        }
        out.extend_from_slice(&(self.outputs.len() as u32).to_le_bytes());
        for output in &self.outputs {
            out.extend_from_slice(&output.to_le_bytes());
        }
        out.extend_from_slice(&(self.blob.len() as u32).to_le_bytes());
        out.extend_from_slice(&self.blob);
        out
    }

    pub fn decode(bytes: &[u8]) -> Result<Trace, String> {
        let mut reader = Reader::new(bytes);
        let mut trace = Trace::default();
        for _ in 0..reader.u32()? {
            trace.reads.push(match reader.u8()? {
                0 => Read::Artifact(reader.id()?, reader.u64()?),
                1 => Read::Schema(reader.string()?, reader.u64()?),
                other => return Err(format!("read tag {other}")),
            });
        }
        for _ in 0..reader.u32()? {
            trace.outputs.push(reader.u64()?);
        }
        let blob = reader.u32()? as usize;
        trace.blob = reader.take(blob)?.to_vec();
        reader.end()?;
        Ok(trace)
    }
}

/// What the change journal holds per source: the facts seen at its last
/// digest (the sizes and the modification times of the source and of its
/// sidecar), with what was read from them.
#[derive(Clone, Debug, PartialEq, Eq)]
struct JournalEntry {
    facts: [u128; 4],
    digest: u64,
    id: AssetId,
    settings: Vec<u8>,
}

static TEMPORARY: AtomicU64 = AtomicU64::new(0);

/// Writes a file in one piece: under another name, then by one rename.
pub fn write_atomically(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let mut name = path.as_os_str().to_owned();
    name.push(format!(".{}-{}.tmp", std::process::id(), TEMPORARY.fetch_add(1, Ordering::Relaxed)));
    let temporary = PathBuf::from(name);
    fs::write(&temporary, bytes).map_err(|error| format!("{}: {error}", temporary.display()))?;
    fs::rename(&temporary, path).map_err(|error| format!("{}: {error}", path.display()))
}

fn hex(hash: u64) -> String {
    format!("{hash:016x}")
}

/// The hash of an artifact: the digest of its bytes.
pub fn content_hash(bytes: &[u8]) -> u64 {
    digest(&[bytes])
}

/// The artifact cache: the content map and the action map, in memory and,
/// where a directory is given, mirrored on disk entry by entry.
pub struct Cache {
    objects: Mutex<HashMap<u64, Arc<Vec<u8>>>>,
    actions: Mutex<HashMap<u64, Arc<Trace>>>,
    journal: Mutex<HashMap<String, JournalEntry>>,
    root: Mutex<Option<Vec<u8>>>,
    disk: Option<PathBuf>,
}

impl Cache {
    pub fn in_memory() -> Cache {
        Cache { objects: Mutex::default(), actions: Mutex::default(), journal: Mutex::default(), root: Mutex::default(), disk: None }
    }

    pub fn on_disk(directory: &Path) -> Result<Cache, String> {
        for part in ["objects", "actions"] {
            fs::create_dir_all(directory.join(part)).map_err(|error| format!("{}: {error}", directory.display()))?;
        }
        Ok(Cache { disk: Some(directory.to_path_buf()), ..Cache::in_memory() })
    }

    /// The bytes of a hash: from memory, or from disk where they are
    /// verified against the hash that names them.
    pub fn object(&self, hash: u64) -> Option<Arc<Vec<u8>>> {
        if let Some(bytes) = self.objects.lock().expect("never poisoned").get(&hash) {
            return Some(bytes.clone());
        }
        let bytes = fs::read(self.disk.as_ref()?.join("objects").join(hex(hash))).ok()?;
        if content_hash(&bytes) != hash {
            return None;
        }
        let bytes = Arc::new(bytes);
        self.objects.lock().expect("never poisoned").insert(hash, bytes.clone());
        Some(bytes)
    }

    pub fn put_object(&self, bytes: Vec<u8>) -> Result<u64, String> {
        let hash = content_hash(&bytes);
        if let Some(directory) = &self.disk {
            let path = directory.join("objects").join(hex(hash));
            if !path.exists() {
                write_atomically(&path, &bytes)?;
            }
        }
        self.objects.lock().expect("never poisoned").entry(hash).or_insert_with(|| Arc::new(bytes));
        Ok(hash)
    }

    pub fn action(&self, key: u64) -> Option<Arc<Trace>> {
        if let Some(trace) = self.actions.lock().expect("never poisoned").get(&key) {
            return Some(trace.clone());
        }
        let bytes = fs::read(self.disk.as_ref()?.join("actions").join(hex(key))).ok()?;
        let trace = Arc::new(Trace::decode(&bytes).ok()?);
        self.actions.lock().expect("never poisoned").insert(key, trace.clone());
        Some(trace)
    }

    /// An entry of the action map is written as its action ends, after its outputs.
    pub fn put_action(&self, key: u64, trace: Trace) -> Result<(), String> {
        if let Some(directory) = &self.disk {
            write_atomically(&directory.join("actions").join(hex(key)), &trace.encode())?;
        }
        self.actions.lock().expect("never poisoned").insert(key, Arc::new(trace));
        Ok(())
    }

    /// Puts a revision in place: one rename of its root document.
    pub fn put_root(&self, bytes: Vec<u8>) -> Result<(), String> {
        if let Some(directory) = &self.disk {
            write_atomically(&directory.join("root"), &bytes)?;
        }
        *self.root.lock().expect("never poisoned") = Some(bytes);
        Ok(())
    }

    pub fn root(&self) -> Option<Vec<u8>> {
        if let Some(root) = self.root.lock().expect("never poisoned").clone() {
            return Some(root);
        }
        fs::read(self.disk.as_ref()?.join("root")).ok()
    }
}

/// What a walk of a cache directory finds: every entry present must be valid
/// under its key.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct CacheCheck {
    pub objects: usize,
    pub invalid_objects: usize,
    pub actions: usize,
    pub invalid_actions: usize,
    /// Files a writer was ended before renaming: no entry, under no key.
    pub unfinished: usize,
}

pub fn check_cache(directory: &Path) -> Result<CacheCheck, String> {
    let mut check = CacheCheck::default();
    let list = |part: &str| -> Result<Vec<PathBuf>, String> {
        let entries = fs::read_dir(directory.join(part)).map_err(|error| error.to_string())?;
        Ok(entries.filter_map(|entry| entry.ok().map(|entry| entry.path())).collect())
    };
    let named = |path: &Path| path.file_name().and_then(|name| name.to_str()).and_then(|name| u64::from_str_radix(name, 16).ok().filter(|_| name.len() == 16));
    let mut present = Vec::new();
    for path in list("objects")? {
        let Some(hash) = named(&path) else {
            check.unfinished += 1;
            continue;
        };
        check.objects += 1;
        match fs::read(&path) {
            Ok(bytes) if content_hash(&bytes) == hash => present.push(hash),
            _ => check.invalid_objects += 1,
        }
    }
    for path in list("actions")? {
        if named(&path).is_none() {
            check.unfinished += 1;
            continue;
        }
        check.actions += 1;
        let trace = fs::read(&path).map_err(|error| error.to_string()).and_then(|bytes| Trace::decode(&bytes));
        if !trace.is_ok_and(|trace| trace.outputs.iter().all(|output| present.contains(output))) {
            check.invalid_actions += 1;
        }
    }
    Ok(check)
}

// ---------------------------------------------------------------------------
// The manifests and the revision
// ---------------------------------------------------------------------------

/// A target profile: its platform and the variant keys its payloads depend on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Profile {
    pub name: &'static str,
    pub platform: &'static str,
    pub variants: [&'static str; 2],
}

pub const DESKTOP: Profile = Profile { name: "desktop", platform: "desktop", variants: ["bc/high", "bc/low"] };
pub const MOBILE: Profile = Profile { name: "mobile", platform: "mobile", variants: ["astc/high", "astc/low"] };

/// An entry of the domain manifest: nothing of a target.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DomainEntry {
    pub kind: AssetKind,
    pub hash: u64,
    pub dependencies: Vec<AssetId>,
}

pub type Domain = BTreeMap<AssetId, DomainEntry>;
/// A presentation manifest: per asset, its variants with each payload's key, hash and size.
pub type Presentation = BTreeMap<AssetId, Vec<(String, u64, u64)>>;

pub fn encode_domain(domain: &Domain) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(&(domain.len() as u32).to_le_bytes());
    for (id, entry) in domain {
        out.extend_from_slice(&id.to_le_bytes());
        out.push(entry.kind.tag());
        out.extend_from_slice(&entry.hash.to_le_bytes());
        out.extend_from_slice(&(entry.dependencies.len() as u32).to_le_bytes());
        for dependency in &entry.dependencies {
            out.extend_from_slice(&dependency.to_le_bytes());
        }
    }
    out
}

pub fn decode_domain(bytes: &[u8]) -> Result<Domain, String> {
    let mut reader = Reader::new(bytes);
    let mut domain = Domain::new();
    for _ in 0..reader.u32()? {
        let id = reader.id()?;
        let (kind, hash) = (AssetKind::from_tag(reader.u8()?)?, reader.u64()?);
        let dependencies = (0..reader.u32()?).map(|_| reader.id()).collect::<Result<_, _>>()?;
        domain.insert(id, DomainEntry { kind, hash, dependencies });
    }
    reader.end()?;
    Ok(domain)
}

pub fn encode_presentation_manifest(presentation: &Presentation) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(&(presentation.len() as u32).to_le_bytes());
    for (id, variants) in presentation {
        out.extend_from_slice(&id.to_le_bytes());
        out.extend_from_slice(&(variants.len() as u32).to_le_bytes());
        for (key, hash, len) in variants {
            out.extend_from_slice(&(key.len() as u32).to_le_bytes());
            out.extend_from_slice(key.as_bytes());
            out.extend_from_slice(&hash.to_le_bytes());
            out.extend_from_slice(&len.to_le_bytes());
        }
    }
    out
}

pub fn decode_presentation_manifest(bytes: &[u8]) -> Result<Presentation, String> {
    let mut reader = Reader::new(bytes);
    let mut presentation = Presentation::new();
    for _ in 0..reader.u32()? {
        let id = reader.id()?;
        let variants = (0..reader.u32()?).map(|_| Ok((reader.string()?, reader.u64()?, reader.u64()?))).collect::<Result<_, String>>()?;
        presentation.insert(id, variants);
    }
    reader.end()?;
    Ok(presentation)
}

const ROOT_MAGIC: &[u8; 4] = b"LGRV";

/// A revision as a reader obtains it from a cache: the root document, the
/// manifests it names and every artifact they name, each verified by its hash.
pub struct Revision {
    pub domain_hash: u64,
    pub domain: Domain,
    pub presentation: Vec<(String, u64, Presentation)>,
}

pub fn read_revision(cache: &Cache) -> Result<Revision, String> {
    let root = cache.root().ok_or("no revision is in place")?;
    let mut reader = Reader::new(&root);
    if reader.take(4)? != ROOT_MAGIC {
        return Err("the root document is not one".to_string());
    }
    let domain_hash = reader.u64()?;
    let object = |hash: u64, what: &str| cache.object(hash).ok_or_else(|| format!("{what} {} is absent or does not hash to its name", hex(hash)));
    let domain = decode_domain(&object(domain_hash, "the domain manifest")?)?;
    for (id, entry) in &domain {
        object(entry.hash, &format!("the canonical part of {}", id_text(*id)))?;
    }
    let mut presentation = Vec::new();
    for _ in 0..reader.u32()? {
        let (name, hash) = (reader.string()?, reader.u64()?);
        let manifest = decode_presentation_manifest(&object(hash, "a presentation manifest")?)?;
        for (key, payload, _) in manifest.values().flatten() {
            object(*payload, &format!("the payload {key}"))?;
        }
        presentation.push((name, hash, manifest));
    }
    reader.end()?;
    Ok(Revision { domain_hash, domain, presentation })
}

// ---------------------------------------------------------------------------
// A pass
// ---------------------------------------------------------------------------

pub struct Options<'a> {
    pub workers: usize,
    /// The target profiles asked; none runs the canonical actions alone.
    pub profiles: &'a [Profile],
    /// A development host trusts its change journal; a hermetic build digests everything.
    pub journal: bool,
    /// The facts of the files are taken from the directory listing, not asked file by file.
    pub facts_from_the_listing: bool,
    /// The describe functions report the dependencies an import would discover.
    pub everything_declared: bool,
    pub placement: Placement,
    pub schema: &'a Schema,
}

/// An action that ran in a pass (the others were served by the cache).
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Ran {
    Describe(String),
    Canonical(AssetId),
    Presentation(AssetId, String),
}

pub struct PassReport {
    pub wall: Duration,
    pub sources: usize,
    /// The sources whose bytes were read to digest them.
    pub sources_digested: usize,
    pub assets: usize,
    pub ran: Vec<Ran>,
    /// The imports that ended with a request and were run again, and the time they had spent.
    pub requests: usize,
    pub run_twice: Duration,
    pub waiting_share: f64,
    pub failures: Vec<(String, String)>,
    pub domain_hash: u64,
    pub domain: Domain,
    pub presentation_hashes: Vec<(&'static str, u64)>,
    /// The runs the pool started again after a request.
    pub restarts: usize,
    /// Where the pass spent its time: the scan of the sources, the graph, the import actions, the revision.
    pub phases: [Duration; 4],
}

impl PassReport {
    pub fn count(&self, kind: fn(&Ran) -> bool) -> usize {
        self.ran.iter().filter(|ran| kind(ran)).count()
    }

    pub fn described(&self) -> usize {
        self.count(|ran| matches!(ran, Ran::Describe(_)))
    }

    pub fn canonical(&self) -> Vec<AssetId> {
        self.ran.iter().filter_map(|ran| if let Ran::Canonical(id) = ran { Some(*id) } else { None }).collect()
    }

    pub fn presentation(&self) -> usize {
        self.count(|ran| matches!(ran, Ran::Presentation(..)))
    }
}

struct Source {
    path: PathBuf,
    label: String,
    kind: SourceKind,
    id: AssetId,
    digest: u64,
    settings: Vec<u8>,
    bytes: OnceLock<Arc<Vec<u8>>>,
    described: Vec<Described>,
}

impl Source {
    /// The bytes of the source, read once; a source the journal vouched for
    /// is checked against the digest the journal held.
    fn bytes(&self) -> Result<Arc<Vec<u8>>, String> {
        if let Some(bytes) = self.bytes.get() {
            return Ok(bytes.clone());
        }
        let bytes = fs::read(&self.path).map_err(|error| format!("{}: {error}", self.label))?;
        if content_hash(&bytes) != self.digest {
            return Err(format!("{} changed since the journal saw it", self.label));
        }
        Ok(self.bytes.get_or_init(|| Arc::new(bytes)).clone())
    }
}

struct Asset {
    id: AssetId,
    kind: AssetKind,
    source: usize,
    key: String,
    canonical_key: u64,
}

fn modified(metadata: &fs::Metadata) -> u128 {
    metadata.modified().ok().and_then(|time| time.duration_since(UNIX_EPOCH).ok()).map_or(0, |since| since.as_nanos())
}

struct Listed {
    path: PathBuf,
    label: String,
    kind: SourceKind,
    /// The sizes and the modification times of the source and of its
    /// sidecar as the directory listing gave them, where it was asked to.
    facts: Option<Result<[u128; 4], String>>,
}

fn facts_of(metadata: &fs::Metadata) -> [u128; 2] {
    [u128::from(metadata.len()), modified(metadata)]
}

/// Lists the sources of a content root, in the order of their paths. With
/// `facts`, the listing carries the facts of every file, so that no file is
/// asked for them one by one.
fn list_sources(root: &Path, facts: bool) -> Result<Vec<Listed>, String> {
    let mut sources = Vec::new();
    for directory in ["tables", "sheets", "scenes"] {
        let Ok(entries) = fs::read_dir(root.join(directory)) else { continue };
        let mut listed = Vec::new();
        let mut seen: HashMap<String, [u128; 2]> = HashMap::new();
        for entry in entries {
            let entry = entry.map_err(|error| error.to_string())?;
            let Ok(name) = entry.file_name().into_string() else { continue };
            if facts {
                seen.insert(name.clone(), facts_of(&entry.metadata().map_err(|error| format!("{directory}/{name}: {error}"))?));
            }
            let kind = name.rsplit_once('.').and_then(|(_, extension)| SourceKind::of_extension(extension));
            if let Some(kind) = kind {
                listed.push((entry.path(), name, kind));
            }
        }
        for (path, name, kind) in listed {
            let label = format!("{directory}/{name}");
            let both = || -> Result<[u128; 4], String> {
                let source = seen[&name];
                let sidecar = if kind.has_sidecar() { *seen.get(&format!("{name}.meta")).ok_or_else(|| format!("{label} has no sidecar: it has no identity"))? } else { [0, 0] };
                Ok([source[0], source[1], sidecar[0], sidecar[1]])
            };
            sources.push(Listed { facts: facts.then(both), path, label, kind });
        }
    }
    sources.sort_by(|a, b| a.label.cmp(&b.label));
    Ok(sources)
}

/// The describe action of one source: its facts, its digest and identity
/// (from the journal where the facts are unchanged and the journal is
/// trusted), and its description (from the cache where the digest is known).
fn scan(listed: &Listed, cache: &Cache, options: &Options, counters: &Counters) -> Result<Source, String> {
    let (path, label, kind) = (listed.path.as_path(), listed.label.as_str(), listed.kind);
    let sidecar_path = sidecar_of(path);
    let facts = match &listed.facts {
        Some(facts) => facts.clone()?,
        None => {
            let facts = |path: &Path| fs::metadata(path).map(|metadata| facts_of(&metadata)).map_err(|error| format!("{label}: {error}"));
            let source = facts(path)?;
            let sidecar = if kind.has_sidecar() { facts(&sidecar_path).map_err(|_| format!("{label} has no sidecar: it has no identity"))? } else { [0, 0] };
            [source[0], source[1], sidecar[0], sidecar[1]]
        }
    };
    let journaled = if options.journal { cache.journal.lock().expect("never poisoned").get(label).filter(|entry| entry.facts == facts).cloned() } else { None };
    let bytes = OnceLock::new();
    let entry = match journaled {
        Some(entry) => entry,
        None => {
            let read = Arc::new(fs::read(path).map_err(|error| format!("{label}: {error}"))?);
            let sidecar = if kind.has_sidecar() { Some(fs::read(&sidecar_path).map_err(|error| format!("{label}: {error}"))?) } else { None };
            counters.digested.fetch_add(1, Ordering::Relaxed);
            let entry = JournalEntry {
                facts,
                digest: content_hash(&read),
                id: identity(kind, &read, sidecar.as_deref()).map_err(|reason| format!("{label}: {reason}"))?,
                settings: settings(sidecar.as_deref()),
            };
            let _ = bytes.set(read);
            if options.journal {
                cache.journal.lock().expect("never poisoned").insert(label.to_string(), entry.clone());
            }
            entry
        }
    };
    let (name, version) = kind.importer();
    let key = digest(&[b"describe", name.as_bytes(), &version.to_le_bytes(), &entry.digest.to_le_bytes(), &[u8::from(options.everything_declared)]]);
    let mut source = Source { path: path.to_path_buf(), label: label.to_string(), kind, id: entry.id, digest: entry.digest, settings: entry.settings, bytes, described: Vec::new() };
    source.described = match cache.action(key) {
        Some(trace) => decode_description(&trace.blob)?,
        None => {
            let described = describe(kind, &source.bytes()?, options.everything_declared).map_err(|reason| format!("{label}: {reason}"))?;
            cache.put_action(key, Trace { blob: encode_description(&described), ..Trace::default() })?;
            counters.ran.lock().expect("never poisoned").push(Ran::Describe(label.to_string()));
            described
        }
    };
    Ok(source)
}

#[derive(Default)]
struct Counters {
    digested: AtomicUsize,
    requests: AtomicUsize,
    run_twice_nanos: AtomicU64,
    ran: Mutex<Vec<Ran>>,
}

/// The second half of a pass: the import actions and what they have built.
struct Imports<'a> {
    cache: &'a Cache,
    options: &'a Options<'a>,
    sources: &'a [Source],
    assets: Vec<Asset>,
    index_of: HashMap<AssetId, usize>,
    variants: Vec<(&'static str, &'static str)>,
    /// The hash of each asset's canonical part once built; zero before.
    hashes: Vec<AtomicU64>,
    dependencies: Vec<Mutex<Vec<AssetId>>>,
    payloads: Vec<Mutex<(u64, u64)>>,
    counters: &'a Counters,
}

impl Built for Imports<'_> {
    fn artifact(&self, id: AssetId) -> Option<(u64, Arc<Vec<u8>>)> {
        let hash = self.hashes[*self.index_of.get(&id)?].load(Ordering::Acquire);
        if hash == 0 {
            return None;
        }
        Some((hash, self.cache.object(hash)?))
    }
}

fn encode_ids(ids: &[AssetId]) -> Vec<u8> {
    ids.iter().flat_map(|id| id.to_le_bytes()).collect()
}

fn decode_ids(bytes: &[u8]) -> Vec<AssetId> {
    bytes.chunks_exact(16).map(|chunk| u128::from_le_bytes(chunk.try_into().expect("sixteen bytes"))).collect()
}

impl Imports<'_> {
    fn built(&self, asset: usize, hash: u64, dependencies: Vec<AssetId>) -> Outcome {
        *self.dependencies[asset].lock().expect("never poisoned") = dependencies;
        self.hashes[asset].store(hash, Ordering::Release);
        Outcome::Done
    }

    fn canonical(&self, index: usize) -> Result<Outcome, String> {
        let asset = &self.assets[index];
        let source = &self.sources[asset.source];
        // The lookup in two steps: the key matches, then every recorded read still has its hash.
        if let Some(trace) = self.cache.action(asset.canonical_key) {
            let mut valid = trace.outputs.len() == 1;
            for read in &trace.reads {
                match read {
                    Read::Artifact(id, hash) => match self.index_of.get(id).map(|other| (*other, self.hashes[*other].load(Ordering::Acquire))) {
                        Some((other, 0)) => return Ok(Outcome::Request(other)),
                        Some((_, built)) => valid &= built == *hash,
                        None => valid = false,
                    },
                    Read::Schema(name, hash) => valid &= self.options.schema.entry_digest(name) == Some(*hash),
                }
            }
            if valid && self.cache.object(trace.outputs[0]).is_some() {
                return Ok(self.built(index, trace.outputs[0], decode_ids(&trace.blob)));
            }
        }
        let bytes = source.bytes()?;
        let mut context = Context::new(self, self.options.schema);
        let started = Instant::now();
        let imported = match import(&mut context, source.kind, &bytes, &asset.key, self.options.placement) {
            Ok(imported) => imported,
            Err(Stop::Failed(reason)) => return Err(reason),
            Err(Stop::Request(id)) => {
                // The import ends with a request and is run again from its start.
                self.counters.requests.fetch_add(1, Ordering::Relaxed);
                self.counters.run_twice_nanos.fetch_add(started.elapsed().as_nanos() as u64, Ordering::Relaxed);
                let other = self.index_of.get(&id).ok_or_else(|| format!("it asks for {}, which no source holds", id_text(id)))?;
                return Ok(Outcome::Request(*other));
            }
        };
        let mut dependencies: Vec<AssetId> = imported.siblings.iter().map(|key| derived_id(source.id, key)).collect();
        dependencies.extend(context.reads.iter().filter_map(|read| if let Read::Artifact(id, _) = read { Some(*id) } else { None }));
        dependencies.sort_unstable();
        dependencies.dedup();
        let part = resolve_siblings(source.id, imported);
        // An importer is trusted for what it computes, never for the well-formedness of what it hands back.
        validate(asset.kind, &part)?;
        let hash = self.cache.put_object(part)?;
        self.cache.put_action(asset.canonical_key, Trace { reads: context.reads, outputs: vec![hash], blob: encode_ids(&dependencies) })?;
        self.counters.ran.lock().expect("never poisoned").push(Ran::Canonical(asset.id));
        Ok(self.built(index, hash, dependencies))
    }

    fn presentation(&self, index: usize, variant: usize, worker: usize) -> Result<Outcome, String> {
        let asset = &self.assets[index];
        let source = &self.sources[asset.source];
        let canonical = self.hashes[index].load(Ordering::Acquire);
        let (key, platform) = self.variants[variant];
        let (name, version) = source.kind.importer();
        let action = digest(&[
            b"presentation",
            name.as_bytes(),
            &version.to_le_bytes(),
            &source.digest.to_le_bytes(),
            asset.key.as_bytes(),
            &canonical.to_le_bytes(),
            key.as_bytes(),
            platform.as_bytes(),
        ]);
        let slot = &self.payloads[index * self.variants.len() + variant];
        if let Some(trace) = self.cache.action(action)
            && trace.outputs.len() == 1
            && let Some(payload) = self.cache.object(trace.outputs[0])
        {
            *slot.lock().expect("never poisoned") = (trace.outputs[0], payload.len() as u64);
            return Ok(Outcome::Done);
        }
        let part = self.cache.object(canonical).ok_or("the canonical part is not in the cache")?;
        let payload = encode_presentation(source.kind, &source.bytes()?, &asset.key, &part, key, worker)?;
        let len = payload.len() as u64;
        let hash = self.cache.put_object(payload)?;
        self.cache.put_action(action, Trace { outputs: vec![hash], ..Trace::default() })?;
        self.counters.ran.lock().expect("never poisoned").push(Ran::Presentation(asset.id, key.to_string()));
        *slot.lock().expect("never poisoned") = (hash, len);
        Ok(Outcome::Done)
    }

    fn execute(&self, action: usize, worker: usize) -> Outcome {
        let count = self.assets.len();
        let outcome = if action < count {
            self.canonical(action)
        } else {
            self.presentation((action - count) / self.variants.len(), (action - count) % self.variants.len(), worker)
        };
        outcome.unwrap_or_else(Outcome::Failed)
    }

    fn name(&self, action: usize) -> String {
        let count = self.assets.len();
        let asset = &self.assets[if action < count { action } else { (action - count) / self.variants.len() }];
        let source = &self.sources[asset.source].label;
        let of = if asset.key.is_empty() { source.clone() } else { format!("{} of {source}", asset.key) };
        if action < count { format!("the canonical part of {of} ({})", id_text(asset.id)) } else { format!("a payload of {of}") }
    }
}

/// One pass over a content root: the describe actions, the import actions,
/// then the manifests and the revision. A pass that has a failed action
/// puts no revision in place.
pub fn pass(root: &Path, cache: &Cache, options: &Options) -> Result<PassReport, String> {
    let started = Instant::now();
    let listed = list_sources(root, options.facts_from_the_listing)?;
    let counters = Counters::default();
    let scanned: Vec<OnceLock<Result<Source, String>>> = listed.iter().map(|_| OnceLock::new()).collect();
    let first = pool::run(options.workers, &vec![Vec::new(); listed.len()], &|index, _| {
        let _ = scanned[index].set(scan(&listed[index], cache, options, &counters));
        Outcome::Done
    });
    let scanned_at = started.elapsed();
    let mut sources = Vec::with_capacity(listed.len());
    for source in scanned {
        sources.push(source.into_inner().expect("every source was scanned")?);
    }
    // A duplicate is an error of the scan, naming both files.
    let mut owners: HashMap<AssetId, &str> = HashMap::new();
    for source in &sources {
        if let Some(other) = owners.insert(source.id, &source.label) {
            return Err(format!("{other} and {} hold one identifier, {}", source.label, id_text(source.id)));
        }
    }

    let mut assets = Vec::new();
    let mut known: Vec<Vec<AssetId>> = Vec::new();
    for (index, source) in sources.iter().enumerate() {
        let (name, version) = source.kind.importer();
        for described in &source.described {
            let entry = options.schema.entry_digest(described.kind.entry()).ok_or_else(|| format!("no schema entry {}", described.kind.entry()))?;
            let canonical_key = digest(&[
                b"canonical",
                name.as_bytes(),
                &version.to_le_bytes(),
                &source.digest.to_le_bytes(),
                &source.settings,
                &source.id.to_le_bytes(),
                described.key.as_bytes(),
                &entry.to_le_bytes(),
                &FORM_VERSION.to_le_bytes(),
            ]);
            let id = if described.key.is_empty() { source.id } else { derived_id(source.id, &described.key) };
            // The dependencies known in advance: the described ones, and the
            // ones the cache's trace recorded the last time the action ran.
            let mut dependencies = described.dependencies.clone();
            if let Some(trace) = cache.action(canonical_key) {
                dependencies.extend(trace.reads.iter().filter_map(|read| if let Read::Artifact(id, _) = read { Some(*id) } else { None }));
            }
            known.push(dependencies);
            assets.push(Asset { id, kind: described.kind, source: index, key: described.key.clone(), canonical_key });
        }
    }
    let index_of: HashMap<AssetId, usize> = assets.iter().enumerate().map(|(index, asset)| (asset.id, index)).collect();
    let variants: Vec<(&'static str, &'static str)> = options.profiles.iter().flat_map(|profile| profile.variants.map(|variant| (variant, profile.platform))).collect();
    let count = assets.len();
    let mut graph: Vec<Vec<usize>> = known.iter().map(|ids| ids.iter().filter_map(|id| index_of.get(id).copied()).collect()).collect();
    for asset in 0..count {
        graph.extend(variants.iter().map(|_| vec![asset]));
    }
    let imports = Imports {
        cache,
        options,
        sources: &sources,
        hashes: (0..count).map(|_| AtomicU64::new(0)).collect(),
        dependencies: (0..count).map(|_| Mutex::default()).collect(),
        payloads: (0..count * variants.len()).map(|_| Mutex::default()).collect(),
        assets,
        index_of,
        variants,
        counters: &counters,
    };
    let graphed_at = started.elapsed();
    let second = pool::run(options.workers, &graph, &|action, worker| imports.execute(action, worker));
    let imported_at = started.elapsed();
    let failures: Vec<(String, String)> = second.failures.iter().map(|(action, reason)| (imports.name(*action), reason.clone())).collect();

    let mut domain = Domain::new();
    let (mut domain_hash, mut presentation_hashes) = (0, Vec::new());
    if failures.is_empty() {
        for (index, asset) in imports.assets.iter().enumerate() {
            let dependencies = imports.dependencies[index].lock().expect("never poisoned").clone();
            domain.insert(asset.id, DomainEntry { kind: asset.kind, hash: imports.hashes[index].load(Ordering::Acquire), dependencies });
        }
        domain_hash = cache.put_object(encode_domain(&domain))?;
        let mut root = ROOT_MAGIC.to_vec();
        root.extend_from_slice(&domain_hash.to_le_bytes());
        root.extend_from_slice(&(options.profiles.len() as u32).to_le_bytes());
        for (number, profile) in options.profiles.iter().enumerate() {
            let mut manifest = Presentation::new();
            for (index, asset) in imports.assets.iter().enumerate() {
                let payload = |variant: usize| {
                    let at = number * 2 + variant;
                    let (hash, len) = *imports.payloads[index * imports.variants.len() + at].lock().expect("never poisoned");
                    (imports.variants[at].0.to_string(), hash, len)
                };
                manifest.insert(asset.id, vec![payload(0), payload(1)]);
            }
            let hash = cache.put_object(encode_presentation_manifest(&manifest))?;
            root.extend_from_slice(&(profile.name.len() as u32).to_le_bytes());
            root.extend_from_slice(profile.name.as_bytes());
            root.extend_from_slice(&hash.to_le_bytes());
            presentation_hashes.push((profile.name, hash));
        }
        // A revision is put in place once every action of the pass has ended.
        cache.put_root(root)?;
    }
    let mut ran = std::mem::take(&mut *counters.ran.lock().expect("never poisoned"));
    ran.sort();
    Ok(PassReport {
        wall: started.elapsed(),
        sources: sources.len(),
        sources_digested: counters.digested.load(Ordering::Relaxed),
        assets: count,
        ran,
        requests: counters.requests.load(Ordering::Relaxed),
        run_twice: Duration::from_nanos(counters.run_twice_nanos.load(Ordering::Relaxed)),
        waiting_share: (first.waiting_share * first.wall.as_secs_f64() + second.waiting_share * second.wall.as_secs_f64())
            / (first.wall + second.wall).as_secs_f64().max(f64::MIN_POSITIVE),
        failures,
        domain_hash,
        domain,
        presentation_hashes,
        restarts: second.restarts,
        phases: [scanned_at, graphed_at - scanned_at, imported_at - graphed_at, started.elapsed() - imported_at],
    })
}
