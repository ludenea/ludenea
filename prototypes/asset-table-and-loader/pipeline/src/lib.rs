//! The stand-in pipeline of the prototype. Throwaway, never product code.
//!
//! It is the producer of content and nothing here is measured. From the
//! import graph spike it keeps the artifact cache (a content map and an
//! action map on disk, every file written under another name and put in
//! place by one rename), the action keyed by what it reads, the domain
//! manifest, the revision put in place by one rename of its root document,
//! and the package. It does not keep that spike's sources and importers
//! (tables, sheets, scenes): the world this prototype steps reads the
//! canonical part of the asset table spike, so the one importer here reads a
//! small text source of this prototype's own and writes that part.
//!
//! A source is a text file `<name>.asset`:
//!
//! ```text
//! asset <identifier, 32 hexadecimal digits>
//! type <stable name of the asset type>
//! strength <number>
//! radius <number>
//! salt <number>
//! depends <identifier> <stable name of the dependency's type>   zero or more lines
//! ```
//!
//! A canonical part lists each dependency with the dependency's own asset
//! type, and so does the domain manifest. When a pass writes a manifest it
//! checks that the type a part lists for a dependency is the type the
//! manifest holds that dependency under: a source that lists another is an
//! import that failed, and its asset keeps its former entry.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use content_store::{AssetId, Domain, DomainEntry, build_package, content_hash, decode_domain, decode_root, encode_domain, encode_root, hex};
use prototype_core::table::{AssetType, Dependency, RepulsionTable, part_bytes, part_len};

const IMPORTER: &str = "parameters importer, version 2";
/// The length of the opaque payload every part carries.
pub const PAYLOAD: usize = 64;

/// One synthetic source, as the harness writes it.
#[derive(Clone, Debug, PartialEq)]
pub struct Source {
    pub asset: AssetId,
    pub asset_type: AssetType,
    pub strength: f32,
    pub radius: f32,
    pub salt: u32,
    /// The size of the canonical part in bytes, where the source states one:
    /// the importer pads the part to it. Without one, the part carries a
    /// payload of a fixed length.
    pub size: Option<usize>,
    /// The assets the part lists, each with the type the source states for it.
    pub dependencies: Vec<Dependency>,
}

impl Source {
    pub fn text(&self) -> String {
        let mut text = format!(
            "asset {:032x}\ntype {}\nstrength {}\nradius {}\nsalt {}\n",
            self.asset,
            self.asset_type.stable_name(),
            self.strength,
            self.radius,
            self.salt
        );
        if let Some(size) = self.size {
            text.push_str(&format!("size {size}\n"));
        }
        for (dependency, asset_type) in &self.dependencies {
            text.push_str(&format!("depends {dependency:032x} {}\n", asset_type.stable_name()));
        }
        text
    }

    pub fn parse(text: &str) -> Result<Source, String> {
        let (mut asset, mut asset_type) = (None, None);
        let (mut strength, mut radius, mut salt, mut size, mut dependencies) = (0.0, 0.0, 0, None, Vec::new());
        let id = |value: &str| u128::from_str_radix(value, 16).map_err(|error| format!("identifier {value}: {error}"));
        let named = |value: &str| AssetType::named(value).ok_or_else(|| format!("the schema holds no asset type named {value}"));
        for line in text.lines() {
            let (key, value) = line.split_once(' ').ok_or_else(|| format!("line without a value: {line}"))?;
            match key {
                "asset" => asset = Some(id(value)?),
                "type" => asset_type = Some(named(value)?),
                "strength" => strength = value.parse().map_err(|error| format!("strength: {error}"))?,
                "radius" => radius = value.parse().map_err(|error| format!("radius: {error}"))?,
                "salt" => salt = value.parse().map_err(|error| format!("salt: {error}"))?,
                "size" => size = Some(value.parse().map_err(|error| format!("size: {error}"))?),
                "depends" => {
                    let (dependency, listed) = value.split_once(' ').ok_or_else(|| format!("a dependency states its type: {line}"))?;
                    dependencies.push((id(dependency)?, named(listed)?));
                }
                other => return Err(format!("unknown key {other}")),
            }
        }
        match (asset, asset_type) {
            (Some(asset), Some(asset_type)) if asset != 0 => Ok(Source { asset, asset_type, strength, radius, salt, size, dependencies }),
            _ => Err("a source names its asset and its type".to_string()),
        }
    }

    /// The canonical part the importer writes for this source.
    pub fn part(&self) -> Vec<u8> {
        let payload = self.size.map_or(PAYLOAD, |size| size.saturating_sub(part_len(&self.dependencies, 0)));
        part_bytes(RepulsionTable { strength: self.strength, radius: self.radius }, &self.dependencies, payload, self.salt)
    }

    /// The entry the domain manifest holds for this source's asset.
    fn entry(&self, hash: u64) -> DomainEntry {
        DomainEntry {
            asset_type: self.asset_type.stable_name().to_string(),
            hash,
            size: self.part().len() as u64,
            dependencies: self.dependencies.iter().map(|(dependency, asset_type)| (*dependency, asset_type.stable_name().to_string())).collect(),
        }
    }
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

/// The artifact cache on disk: `objects/<hash>` holds bytes, `actions/<key>`
/// the hash an action wrote, and `root` the root document of the revision in
/// place. Its directory is the loose store a loading capability reads.
pub struct Cache {
    directory: PathBuf,
}

impl Cache {
    pub fn on_disk(directory: &Path) -> Result<Cache, String> {
        for part in ["objects", "actions"] {
            fs::create_dir_all(directory.join(part)).map_err(|error| format!("{}: {error}", directory.display()))?;
        }
        Ok(Cache { directory: directory.to_path_buf() })
    }

    pub fn directory(&self) -> &Path {
        &self.directory
    }

    pub fn object_path(&self, hash: u64) -> PathBuf {
        self.directory.join("objects").join(hex(hash))
    }

    /// The bytes of a hash, verified against the hash that names them.
    pub fn object(&self, hash: u64) -> Option<Vec<u8>> {
        let bytes = fs::read(self.object_path(hash)).ok()?;
        (content_hash(&bytes) == hash).then_some(bytes)
    }

    pub fn put_object(&self, bytes: &[u8]) -> Result<u64, String> {
        let hash = content_hash(bytes);
        let path = self.object_path(hash);
        if !path.exists() {
            write_atomically(&path, bytes)?;
        }
        Ok(hash)
    }

    /// Takes an artifact out of the store, as the cache's collection may.
    pub fn remove_object(&self, hash: u64) -> Result<(), String> {
        fs::remove_file(self.object_path(hash)).map_err(|error| format!("{}: {error}", hex(hash)))
    }

    /// Cuts an artifact short under its name: a damaged file at rest.
    pub fn cut_object(&self, hash: u64) -> Result<(), String> {
        let bytes = fs::read(self.object_path(hash)).map_err(|error| format!("{}: {error}", hex(hash)))?;
        fs::write(self.object_path(hash), &bytes[..bytes.len() / 2]).map_err(|error| format!("{}: {error}", hex(hash)))
    }

    fn action(&self, key: u64) -> Option<u64> {
        let text = fs::read_to_string(self.directory.join("actions").join(hex(key))).ok()?;
        u64::from_str_radix(text.trim(), 16).ok()
    }

    fn put_action(&self, key: u64, output: u64) -> Result<(), String> {
        write_atomically(&self.directory.join("actions").join(hex(key)), hex(output).as_bytes())
    }

    /// The revision in place: the hash of its domain manifest and the manifest.
    pub fn revision(&self) -> Result<Revision, String> {
        let root = fs::read(self.directory.join("root")).map_err(|error| format!("no revision is in place: {error}"))?;
        let domain_hash = decode_root(&root)?;
        let bytes = self.object(domain_hash).ok_or_else(|| format!("the domain manifest {} is absent", hex(domain_hash)))?;
        Ok(Revision { domain_hash, domain: decode_domain(&bytes)? })
    }
}

/// One consistent domain manifest, put in place atomically.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Revision {
    pub domain_hash: u64,
    pub domain: Domain,
}

/// What a pass did.
pub struct Pass {
    pub revision: Revision,
    /// The assets whose import ran (the others were served by the cache).
    pub ran: Vec<AssetId>,
    /// The sources that failed, each with its reason; an asset whose import
    /// fails keeps its former entry in the revision.
    pub failures: Vec<(String, String)>,
}

/// What a pass checks when it writes a manifest.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Checks {
    /// The type a part lists for a dependency is the type the manifest holds
    /// that dependency under. Off, a pass writes content a pipeline never
    /// would: a part that lists a type the manifest contradicts, which is
    /// what the loading capability's own check exists to refuse.
    pub dependency_types: bool,
}

impl Default for Checks {
    fn default() -> Checks {
        Checks { dependency_types: true }
    }
}

/// One pass over a content root: every source is imported by an action keyed
/// by its bytes and its importer, the domain manifest is written, and the
/// revision is put in place by one rename of its root document.
pub fn pass(root: &Path, cache: &Cache) -> Result<Pass, String> {
    pass_with(root, cache, Checks::default())
}

pub fn pass_with(root: &Path, cache: &Cache, checks: Checks) -> Result<Pass, String> {
    let former = cache.revision().map(|revision| revision.domain).unwrap_or_default();
    let mut paths: Vec<PathBuf> = fs::read_dir(root)
        .map_err(|error| format!("{}: {error}", root.display()))?
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .filter(|path| path.extension().is_some_and(|extension| extension == "asset"))
        .collect();
    paths.sort();
    let (mut ran, mut failures) = (Vec::new(), Vec::new());
    let mut sources: Vec<(String, Vec<u8>, Source)> = Vec::new();
    for path in paths {
        let label = path.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_default();
        let bytes = fs::read(&path).map_err(|error| format!("{label}: {error}"))?;
        match std::str::from_utf8(&bytes).map_err(|error| error.to_string()).and_then(Source::parse) {
            Ok(source) => sources.push((label, bytes, source)),
            // The asset of a source that no longer parses is found by its file name in the former revision.
            Err(reason) => failures.push((label, reason)),
        }
    }
    // The type each asset of the pass is imported under: what the manifest
    // will hold a dependency under.
    let types: BTreeMap<AssetId, AssetType> = sources.iter().map(|(_, _, source)| (source.asset, source.asset_type)).collect();
    let mut domain = Domain::new();
    for (label, bytes, source) in sources {
        let contradicted = source.dependencies.iter().find(|(dependency, listed)| types.get(dependency).is_some_and(|held| held != listed));
        if checks.dependency_types
            && let Some((dependency, listed)) = contradicted
        {
            let reason = format!("the part lists the dependency {dependency:032x} under the type {}, which is not the type it is imported under", listed.stable_name());
            failures.push((label, reason));
            continue;
        }
        let key = content_hash(&[IMPORTER.as_bytes(), &bytes].concat());
        let hash = match cache.action(key).filter(|hash| cache.object_path(*hash).exists()) {
            Some(hash) => hash,
            None => {
                let hash = cache.put_object(&source.part())?;
                cache.put_action(key, hash)?;
                ran.push(source.asset);
                hash
            }
        };
        domain.insert(source.asset, source.entry(hash));
    }
    for (id, entry) in &former {
        if !failures.is_empty() && !domain.contains_key(id) {
            domain.insert(*id, entry.clone());
        }
    }
    let domain_hash = cache.put_object(&encode_domain(&domain))?;
    write_atomically(&cache.directory.join("root"), &encode_root(domain_hash))?;
    Ok(Pass { revision: Revision { domain_hash, domain }, ran, failures })
}

/// Builds a package of the given assets from the cache.
pub fn package(cache: &Cache, domain: &Domain, entries: &BTreeSet<AssetId>) -> Result<Vec<u8>, String> {
    build_package(&|hash| cache.object(hash), domain, entries)
}

#[cfg(test)]
mod tests {
    use content_store::contradicted_dependencies;

    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let directory = std::env::temp_dir().join(format!("asset-pipeline-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&directory);
        fs::create_dir_all(directory.join("content")).unwrap();
        directory
    }

    fn source(asset: AssetId, strength: f32, dependencies: &[Dependency]) -> Source {
        Source { asset, asset_type: AssetType::Parameters, strength, radius: 8.0, salt: 0, size: None, dependencies: dependencies.to_vec() }
    }

    #[test]
    fn a_source_that_states_a_size_gives_a_part_padded_to_it() {
        let mut sized = source(5, 1.0, &[(7, AssetType::Parameters)]);
        sized.size = Some(9_000);
        assert_eq!(sized.part().len(), 9_000);
        assert_eq!(sized.entry(1).size, 9_000, "the size the domain manifest gives");
        assert_eq!(Source::parse(&sized.text()).unwrap(), sized);
    }

    #[test]
    fn a_source_round_trips_through_its_text() {
        let one = source(0x1234, 48.5, &[(7, AssetType::Parameters), (9, AssetType::Second)]);
        assert_eq!(Source::parse(&one.text()).unwrap(), one);
        assert!(Source::parse("strength 1\n").is_err());
        assert!(Source::parse("asset 1\ntype prototype/parameters\ndepends 2\n").is_err(), "a dependency states its type");
        assert!(Source::parse("asset 1\ntype no/such\n").is_err());
    }

    #[test]
    fn a_pass_writes_a_revision_and_a_second_pass_runs_only_what_changed() {
        let directory = scratch("pass");
        let content = directory.join("content");
        let cache = Cache::on_disk(&directory.join("cache")).unwrap();
        fs::write(content.join("a.asset"), source(1, 64.0, &[(2, AssetType::Parameters)]).text()).unwrap();
        fs::write(content.join("b.asset"), source(2, 32.0, &[]).text()).unwrap();
        let first = pass(&content, &cache).unwrap();
        assert_eq!(first.ran, vec![1, 2]);
        assert_eq!(cache.revision().unwrap(), first.revision);
        assert_eq!(first.revision.domain[&1].dependencies, vec![(2, AssetType::Parameters.stable_name().to_string())]);
        assert_eq!(first.revision.domain[&2].asset_type, AssetType::Parameters.stable_name());
        assert_eq!(cache.object(first.revision.domain[&2].hash).unwrap(), source(2, 32.0, &[]).part());
        fs::write(content.join("b.asset"), source(2, 16.0, &[]).text()).unwrap();
        let second = pass(&content, &cache).unwrap();
        assert_eq!(second.ran, vec![2]);
        assert_ne!(second.revision.domain_hash, first.revision.domain_hash);
        assert_eq!(second.revision.domain[&1], first.revision.domain[&1]);
        let file = package(&cache, &second.revision.domain, &BTreeSet::from([2])).unwrap();
        assert_eq!(content_store::Package::read(file).unwrap().domain.len(), 1);
        cache.cut_object(second.revision.domain[&2].hash).unwrap();
        assert!(cache.object(second.revision.domain[&2].hash).is_none());
        fs::remove_dir_all(&directory).unwrap();
    }

    #[test]
    fn a_pass_writes_no_part_that_lists_a_type_the_manifest_contradicts() {
        let directory = scratch("types");
        let content = directory.join("content");
        let cache = Cache::on_disk(&directory.join("cache")).unwrap();
        fs::write(content.join("a.asset"), source(1, 64.0, &[(2, AssetType::Parameters)]).text()).unwrap();
        fs::write(content.join("b.asset"), source(2, 32.0, &[]).text()).unwrap();
        let first = pass(&content, &cache).unwrap();
        // The parent now lists its dependency under a type it is not imported under.
        fs::write(content.join("a.asset"), source(1, 64.0, &[(2, AssetType::Second)]).text()).unwrap();
        let second = pass(&content, &cache).unwrap();
        assert_eq!(second.failures.len(), 1);
        assert_eq!(second.failures[0].0, "a.asset");
        assert_eq!(second.revision.domain[&1], first.revision.domain[&1], "an asset whose import fails keeps its former entry");
        assert!(contradicted_dependencies(&second.revision.domain).is_empty());
        // With the check off, the pass writes what a pipeline never would.
        let unchecked = pass_with(&content, &cache, Checks { dependency_types: false }).unwrap();
        assert!(unchecked.failures.is_empty());
        assert_eq!(contradicted_dependencies(&unchecked.revision.domain).len(), 1);
        fs::remove_dir_all(&directory).unwrap();
    }
}
