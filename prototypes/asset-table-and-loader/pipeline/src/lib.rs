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
//! manifest-size <bytes>   optional: the size the manifest holds, where it is not the part's length
//! size <bytes>            optional: the length the part is padded to
//! depends <identifier> <stable name of the dependency's type>   zero or more lines
//! form-size <bytes>       optional: the size the prepared form built once of the part is padded to
//! payload <part> <number> optional: the asset has a payload, made for the part named, at this revision
//! presents <identifier>   zero or more lines: a presentation dependency
//! ```
//!
//! The last three keys are read by no canonical action: a canonical import
//! holds nothing of a target. Where a pass is asked for it, a preparing
//! action runs once per part of an asset type that has a prepared form
//! built once, under a platform name, and the domain manifest lists the hash
//! and the size of its output beside those of the part; and a presentation
//! action writes the payload of each asset that has one, the pass giving one
//! presentation manifest per target profile asked, which the root document
//! names beside the domain manifest. Both are the stand-ins a scenario sheet
//! defines. A selection among the files of one run is a loose store of its
//! own: what a dedicated server ships is the domain manifest, the parts and
//! the forms built once, and nothing else.
//!
//! A canonical part lists each dependency with the dependency's own asset
//! type, and so does the domain manifest. When a pass writes a manifest it
//! checks that the type a part lists for a dependency is the type the
//! manifest holds that dependency under: a source that lists another is an
//! import that failed, and its asset keeps its former entry.
//!
//! A pass is the pass of a development host: an action that fails does not
//! hold the revision back. The canonical action of a source that does not
//! read (the word `error` where the importer reads a number, for example)
//! fails with a diagnostic that names the source, every other action of the
//! pass runs, and the revision keeps the former entry of the asset whose
//! action failed; an asset imported for the first time whose action fails
//! has no entry. The two behaviors a scenario's control puts in its place
//! are the fields of `Options`.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use content_store::{
    AssetId, Domain, DomainEntry, PreparedForm, Presentation, PresentationEntry, Variant, build_package, content_hash, decode_domain, decode_presentation, decode_root_whole,
    encode_domain, encode_presentation, encode_root_with, hex,
};
use prototype_core::forms::{PRODUCER, Preparer, stamp};
use prototype_core::table::{AssetType, Dependency, RepulsionTable, part_bytes, part_len};

const IMPORTER: &str = "parameters importer, version 2";
const ENCODER: &str = "payload encoder, version 1";
/// The one variant the stand-in presentation manifest holds for an asset.
pub const VARIANT: &str = "full";
/// The length every payload is padded to.
pub const PAYLOAD_SIZE: usize = 256;
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
    /// The size the domain manifest holds for the asset, where the source
    /// states one that is not the length of its canonical part: what a
    /// prepared form built once would add to it. The pipeline writes it into
    /// the manifest and compares it with nothing.
    pub manifest_size: Option<u64>,
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
        if let Some(size) = self.manifest_size {
            text.push_str(&format!("manifest-size {size}\n"));
        }
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
        let (mut strength, mut radius, mut salt, mut size, mut manifest_size, mut dependencies) = (0.0, 0.0, 0, None, None, Vec::new());
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
                "manifest-size" => manifest_size = Some(value.parse().map_err(|error| format!("manifest-size: {error}"))?),
                "depends" => {
                    let (dependency, listed) = value.split_once(' ').ok_or_else(|| format!("a dependency states its type: {line}"))?;
                    dependencies.push((id(dependency)?, named(listed)?));
                }
                // What a source states for the actions that are not its
                // canonical one: the canonical importer reads none of it.
                "form-size" | "payload" | "presents" => {}
                other => return Err(format!("unknown key {other}")),
            }
        }
        match (asset, asset_type) {
            (Some(asset), Some(asset_type)) if asset != 0 => Ok(Source { asset, asset_type, strength, radius, salt, size, manifest_size, dependencies }),
            _ => Err("a source names its asset and its type".to_string()),
        }
    }

    /// The asset a source names, read from its text alone: what the pass
    /// knows of a source whose canonical action fails, so that the asset
    /// keeps its former entry. None where the text does not name one.
    pub fn asset_of(text: &str) -> Option<AssetId> {
        let named = text.lines().find_map(|line| line.strip_prefix("asset "))?;
        u128::from_str_radix(named.trim(), 16).ok().filter(|asset| *asset != 0)
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
            size: self.manifest_size.unwrap_or(self.part().len() as u64),
            dependencies: self.dependencies.iter().map(|(dependency, asset_type)| (*dependency, asset_type.stable_name().to_string())).collect(),
            // The forms built once are the preparing actions' to list.
            forms: Vec::new(),
        }
    }
}

/// What a source states for the actions that are not its canonical one: the
/// size its prepared form built once is padded to, where a family states
/// one; the payload of its asset, where it has one, as the name of the part
/// the payload is made for and its revision number; and its presentation
/// dependencies. A source that states none of it has the text it had.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Beside {
    pub form_size: Option<usize>,
    pub payload: Option<(String, u32)>,
    pub presents: Vec<AssetId>,
}

impl Beside {
    /// The lines a source holds for it, after those of its canonical part.
    pub fn text(&self) -> String {
        let mut text = String::new();
        if let Some(size) = self.form_size {
            text.push_str(&format!("form-size {size}\n"));
        }
        if let Some((made_for, revision)) = &self.payload {
            text.push_str(&format!("payload {made_for} {revision}\n"));
        }
        for asset in &self.presents {
            text.push_str(&format!("presents {asset:032x}\n"));
        }
        text
    }

    /// Reads those lines from the text of a source, and no other.
    pub fn parse(text: &str) -> Result<Beside, String> {
        let mut beside = Beside::default();
        for line in text.lines() {
            let Some((key, value)) = line.split_once(' ') else { continue };
            match key {
                "form-size" => beside.form_size = Some(value.parse().map_err(|error| format!("form-size: {error}"))?),
                "payload" => {
                    let (made_for, revision) = value.split_once(' ').ok_or_else(|| format!("a payload names its part and its revision: {line}"))?;
                    beside.payload = Some((made_for.to_string(), revision.parse().map_err(|error| format!("payload: {error}"))?));
                }
                "presents" => beside.presents.push(u128::from_str_radix(value, 16).map_err(|error| format!("identifier {value}: {error}"))?),
                _ => {}
            }
        }
        Ok(beside)
    }
}

/// The stand-in payload of an asset: the text `payload`, one space, the name
/// of the part it was made for, one space, and the revision number of the
/// payload, padded with zero bytes to 256 bytes.
pub fn payload(made_for: &str, revision: u32) -> Vec<u8> {
    let mut bytes = format!("payload {made_for} {revision}").into_bytes();
    bytes.resize(bytes.len().max(PAYLOAD_SIZE), 0);
    bytes
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

    /// Writes bytes under a hash that is not theirs. No pipeline does: it
    /// is what one control of a scenario sheet asks of a pass, a form a
    /// machine built for itself held under the hash of the form built once.
    fn put_object_under(&self, hash: u64, bytes: &[u8]) -> Result<(), String> {
        write_atomically(&self.object_path(hash), bytes)
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

    /// The revision in place: the hash of its domain manifest and the
    /// manifest, and the presentation manifests its root names.
    pub fn revision(&self) -> Result<Revision, String> {
        let root = fs::read(self.directory.join("root")).map_err(|error| format!("no revision is in place: {error}"))?;
        let (domain_hash, presentation) = decode_root_whole(&root)?;
        let bytes = self.object(domain_hash).ok_or_else(|| format!("the domain manifest {} is absent", hex(domain_hash)))?;
        Ok(Revision { domain_hash, domain: decode_domain(&bytes)?, presentation })
    }

    /// A presentation manifest the store holds, by its hash.
    pub fn presentation(&self, hash: u64) -> Result<Presentation, String> {
        decode_presentation(&self.object(hash).ok_or_else(|| format!("the presentation manifest {} is absent", hex(hash)))?)
    }
}

/// One consistent domain manifest, put in place atomically, with the
/// presentation manifests of the same pass, each by the name of its target
/// profile and its hash: none where the pass was asked for none.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Revision {
    pub domain_hash: u64,
    pub domain: Domain,
    pub presentation: Vec<(String, u64)>,
}

/// What a pass did.
pub struct Pass {
    /// The revision in place once the pass has ended: the one it wrote, or
    /// the former one where it put none in place.
    pub revision: Revision,
    /// Whether the pass put a revision in place.
    pub in_place: bool,
    /// The assets whose import ran (the others were served by the cache).
    pub ran: Vec<AssetId>,
    /// The actions that failed: the source of each, and a diagnostic that
    /// names it. An asset whose action fails keeps its former entry in the
    /// revision.
    pub failures: Vec<(String, String)>,
}

/// What a pass does where an action fails. The default is the design text's
/// development host: the revision is written without the action that failed,
/// and keeps the former entry of its asset. Each field runs the behavior a
/// scenario's control puts in its place.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Options {
    /// Control: the pass writes a revision without an entry for an asset
    /// whose action failed, in place of its former entry.
    pub failed_action_leaves_no_entry: bool,
    /// Control: an action that fails holds the revision back, the pass
    /// putting no revision in place until every action of a pass has
    /// succeeded.
    pub failed_action_holds_the_revision_back: bool,
    /// Control, of a preparing action and of no failure: the form a machine
    /// builds for itself is written in its manifest and held in its store
    /// under the hash of the form built once, the producer's, with the
    /// producer's size. The default holds a form under the hash of its own
    /// bytes: a machine's own build lists another form than the producer's,
    /// and so writes another domain manifest.
    pub own_forms_under_the_hash_of_the_form_built_once: bool,
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
    pass_under(root, cache, checks, Options::default())
}

pub fn pass_under(root: &Path, cache: &Cache, checks: Checks, options: Options) -> Result<Pass, String> {
    pass_asked(root, cache, checks, options, &Asked::default())
}

/// What a pass is asked beyond the canonical parts: the prepared forms built
/// once that the compositions of the project declare, the platform name
/// they are prepared under, and the target profiles a presentation manifest
/// is asked for. Asked for nothing, a pass writes a domain manifest that
/// lists no form and a root document that names no presentation manifest.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Asked {
    /// Per asset type that has a prepared form built once, what prepares it:
    /// a preparing action runs once per part of the type.
    pub forms: Vec<(AssetType, Preparer)>,
    /// The platform name the forms are prepared under: the producer's for
    /// the content built once, a machine's local name for its own build.
    pub platform: String,
    /// The target profiles: one presentation manifest for each.
    pub profiles: Vec<String>,
}

impl Asked {
    /// The forms built once of one platform, and no presentation manifest.
    pub fn forms(forms: &[(AssetType, Preparer)], platform: &str) -> Asked {
        Asked { forms: forms.to_vec(), platform: platform.to_string(), ..Asked::default() }
    }
}

/// One preparing action: the prepared form built once of one part, under the
/// platform name the pass prepares under, keyed by what it reads. Returns
/// what the domain manifest lists of the form: what prepares it, the hash
/// and the size of its bytes.
fn prepare(cache: &Cache, asked: &Asked, options: Options, preparer: Preparer, part: u64, size: Option<usize>) -> Result<PreparedForm, String> {
    let label = preparer.label();
    let built = |platform: &str| -> Result<Vec<u8>, String> {
        let part = cache.object(part).ok_or_else(|| format!("the part {} to prepare is not in the cache", hex(part)))?;
        match preparer {
            Preparer::Stamp => Ok(stamp(&part, platform, size)),
            Preparer::Grind => Err(format!("{label} is no preparing action: its form is the same computation on every cell, and a machine builds it at load")),
        }
    };
    // Under one control a machine's own form passes for the producer's: it
    // is listed and held under the hash and the size of the form built once.
    if options.own_forms_under_the_hash_of_the_form_built_once && asked.platform != PRODUCER {
        let named = built(PRODUCER)?;
        let hash = content_hash(&named);
        cache.put_object_under(hash, &built(&asked.platform)?)?;
        return Ok(PreparedForm { preparer: label, hash, size: named.len() as u64 });
    }
    let key = content_hash(format!("{label}\n{}\n{}\n{size:?}", asked.platform, hex(part)).as_bytes());
    let hash = match cache.action(key).filter(|hash| cache.object_path(*hash).exists()) {
        Some(hash) => hash,
        None => {
            let hash = cache.put_object(&built(&asked.platform)?)?;
            cache.put_action(key, hash)?;
            hash
        }
    };
    let size = fs::metadata(cache.object_path(hash)).map_err(|error| format!("{}: {error}", hex(hash)))?.len();
    Ok(PreparedForm { preparer: label, hash, size })
}

pub fn pass_asked(root: &Path, cache: &Cache, checks: Checks, options: Options, asked: &Asked) -> Result<Pass, String> {
    let former = cache.revision().map(|revision| revision.domain).unwrap_or_default();
    let mut paths: Vec<PathBuf> = fs::read_dir(root)
        .map_err(|error| format!("{}: {error}", root.display()))?
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .filter(|path| path.extension().is_some_and(|extension| extension == "asset"))
        .collect();
    paths.sort();
    let (mut ran, mut failures) = (Vec::new(), Vec::new());
    // The asset of each action that failed, where its source names one.
    let mut failed: Vec<Option<AssetId>> = Vec::new();
    let mut sources: Vec<(String, Vec<u8>, Source)> = Vec::new();
    for path in paths {
        let label = path.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_default();
        let bytes = fs::read(&path).map_err(|error| format!("{label}: {error}"))?;
        let text = std::str::from_utf8(&bytes).map_err(|error| error.to_string());
        match text.clone().and_then(Source::parse) {
            Ok(source) => sources.push((label, bytes, source)),
            // The canonical action of the source's asset fails, with a
            // diagnostic that names the source; every other action runs.
            Err(reason) => {
                failed.push(text.ok().and_then(Source::asset_of));
                failures.push((label.clone(), format!("{label}: {reason}")));
            }
        }
    }
    // The type each asset of the pass is imported under: what the manifest
    // will hold a dependency under.
    let types: BTreeMap<AssetId, AssetType> = sources.iter().map(|(_, _, source)| (source.asset, source.asset_type)).collect();
    let mut domain = Domain::new();
    let mut presentation = Presentation::new();
    for (label, bytes, source) in sources {
        let contradicted = source.dependencies.iter().find(|(dependency, listed)| types.get(dependency).is_some_and(|held| held != listed));
        if checks.dependency_types
            && let Some((dependency, listed)) = contradicted
        {
            let reason = format!("the part lists the dependency {dependency:032x} under the type {}, which is not the type it is imported under", listed.stable_name());
            failed.push(Some(source.asset));
            failures.push((label.clone(), format!("{label}: {reason}")));
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
        let mut entry = source.entry(hash);
        // What the source states for the actions that are not its canonical
        // one: a text that parsed as a source is UTF-8.
        let beside = Beside::parse(std::str::from_utf8(&bytes).unwrap_or("")).map_err(|reason| format!("{label}: {reason}"))?;
        // One preparing action per form built once that the part's type has.
        for (_, preparer) in asked.forms.iter().filter(|(asset_type, preparer)| *asset_type == source.asset_type && preparer.built_once()) {
            entry.forms.push(prepare(cache, asked, options, *preparer, hash, beside.form_size)?);
        }
        domain.insert(source.asset, entry);
        // One presentation action per asset that has a payload, keyed by
        // what it reads: the part the payload is made for and its revision.
        if let Some((made_for, revision)) = beside.payload.as_ref().filter(|_| !asked.profiles.is_empty()) {
            let key = content_hash(format!("{ENCODER}\n{made_for}\n{revision}").as_bytes());
            let payload_hash = match cache.action(key).filter(|hash| cache.object_path(*hash).exists()) {
                Some(hash) => hash,
                None => {
                    let hash = cache.put_object(&payload(made_for, *revision))?;
                    cache.put_action(key, hash)?;
                    hash
                }
            };
            let variant = Variant { name: VARIANT.to_string(), hash: payload_hash, size: PAYLOAD_SIZE as u64 };
            presentation.insert(source.asset, PresentationEntry { variants: vec![variant], dependencies: beside.presents.clone() });
        }
    }
    // The revision keeps the former entry of an asset whose action failed;
    // an asset imported for the first time has none to keep. Where a source
    // that failed names no asset, the pass cannot tell which entry is its
    // own, and keeps every former entry it did not write again.
    let unnamed = failed.iter().any(Option::is_none);
    for (id, entry) in &former {
        let kept = unnamed || failed.contains(&Some(*id));
        if kept && !options.failed_action_leaves_no_entry && !domain.contains_key(id) {
            domain.insert(*id, entry.clone());
        }
    }
    if options.failed_action_holds_the_revision_back && !failures.is_empty() {
        // Under this control the actions that succeeded stay in the cache,
        // and the revision in place stays the former one.
        let revision = cache.revision().map_err(|error| format!("an action failed and holds back the first revision of the store: {error}"))?;
        return Ok(Pass { revision, in_place: false, ran, failures });
    }
    let domain_hash = cache.put_object(&encode_domain(&domain))?;
    // One presentation manifest per target profile asked, a document of its
    // own: the stand-in has one profile, and its manifests do not differ.
    let mut named = Vec::new();
    for profile in &asked.profiles {
        named.push((profile.clone(), cache.put_object(&encode_presentation(&presentation))?));
    }
    write_atomically(&cache.directory.join("root"), &encode_root_with(domain_hash, &named))?;
    Ok(Pass { revision: Revision { domain_hash, domain, presentation: named }, in_place: true, ran, failures })
}

/// A selection among the files of one run: what a host ships. Every
/// selection holds the domain manifest and the canonical parts.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Selection {
    /// The prepared forms built once that the domain manifest lists.
    pub forms: bool,
    /// The presentation manifests, with their payloads.
    pub presentation: bool,
}

impl Selection {
    /// What a dedicated server ships: the domain manifest, the parts and
    /// the forms built once, and nothing else.
    pub const SERVER: Selection = Selection { forms: true, presentation: false };
}

/// Puts a selection of the revision in place of a cache into a loose store
/// of its own: the artifacts of the selection, each under its hash, then a
/// root document that names the manifests the selection holds, put in place
/// by one rename. An artifact the selection already holds is left as it is.
/// Returns the revision as the selection holds it.
pub fn select(cache: &Cache, to: &Path, selection: Selection) -> Result<Revision, String> {
    let revision = cache.revision()?;
    let selected = Cache::on_disk(to)?;
    let copy = |hash: u64| -> Result<(), String> {
        let (from, into) = (cache.object_path(hash), selected.object_path(hash));
        if from.exists() && !into.exists() {
            let bytes = fs::read(&from).map_err(|error| format!("{}: {error}", hex(hash)))?;
            write_atomically(&into, &bytes)?;
        }
        Ok(())
    };
    copy(revision.domain_hash)?;
    for entry in revision.domain.values() {
        copy(entry.hash)?;
        for form in entry.forms.iter().filter(|_| selection.forms) {
            copy(form.hash)?;
        }
    }
    let mut named = Vec::new();
    for (profile, hash) in revision.presentation.iter().filter(|_| selection.presentation) {
        copy(*hash)?;
        for variant in cache.presentation(*hash)?.values().flat_map(|entry| &entry.variants) {
            copy(variant.hash)?;
        }
        named.push((profile.clone(), *hash));
    }
    write_atomically(&selected.directory.join("root"), &encode_root_with(revision.domain_hash, &named))?;
    Ok(Revision { presentation: named, ..revision })
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
        Source { asset, asset_type: AssetType::Parameters, strength, radius: 8.0, salt: 0, size: None, manifest_size: None, dependencies: dependencies.to_vec() }
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

    /// The text of a source saved with an error: the word `error` where the
    /// importer reads the first parameter.
    fn with_an_error(source: &Source) -> String {
        let lines: Vec<String> = source.text().lines().map(|line| if line.starts_with("strength ") { "strength error".to_string() } else { line.to_string() }).collect();
        lines.join("\n") + "\n"
    }

    #[test]
    fn an_action_that_fails_leaves_the_former_entry_and_the_rest_of_the_pass_is_written() {
        let directory = scratch("error");
        let content = directory.join("content");
        let cache = Cache::on_disk(&directory.join("cache")).unwrap();
        fs::write(content.join("a.asset"), source(1, 64.0, &[]).text()).unwrap();
        fs::write(content.join("b.asset"), source(2, 32.0, &[]).text()).unwrap();
        let first = pass(&content, &cache).unwrap();
        assert!(first.in_place && first.failures.is_empty());
        // One source saved with an error, one saved anew, and one saved for
        // the first time with an error.
        let save = || {
            fs::write(content.join("a.asset"), with_an_error(&source(1, 48.0, &[]))).unwrap();
            fs::write(content.join("b.asset"), source(2, 16.0, &[]).text()).unwrap();
            fs::write(content.join("c.asset"), with_an_error(&source(3, 8.0, &[]))).unwrap();
        };
        save();
        assert_eq!(Source::asset_of(&with_an_error(&source(3, 8.0, &[]))), Some(3));
        let second = pass(&content, &cache).unwrap();
        assert!(second.in_place, "an action that fails does not hold the revision back");
        assert_eq!(second.failures.iter().map(|(source, _)| source.as_str()).collect::<Vec<_>>(), vec!["a.asset", "c.asset"]);
        assert!(second.failures.iter().all(|(source, diagnostic)| diagnostic.starts_with(source.as_str())), "a diagnostic names its source: {:?}", second.failures);
        assert_eq!(second.ran, vec![2], "every other action of the pass runs");
        assert_eq!(second.revision.domain[&1], first.revision.domain[&1], "the asset whose action failed keeps its former entry");
        assert_ne!(second.revision.domain[&2], first.revision.domain[&2]);
        assert!(!second.revision.domain.contains_key(&3), "an asset imported for the first time whose action fails has no entry");
        assert_eq!(cache.revision().unwrap(), second.revision);
        // The two controls: no entry in place of the former one, and a
        // revision held back until every action has succeeded.
        let without = pass_under(&content, &cache, Checks::default(), Options { failed_action_leaves_no_entry: true, ..Options::default() }).unwrap();
        assert!(without.in_place && !without.revision.domain.contains_key(&1) && without.revision.domain.contains_key(&2));
        fs::write(content.join("b.asset"), source(2, 4.0, &[]).text()).unwrap();
        let held = pass_under(&content, &cache, Checks::default(), Options { failed_action_holds_the_revision_back: true, ..Options::default() }).unwrap();
        assert!(!held.in_place && held.failures.len() == 2);
        assert_eq!(held.revision, without.revision, "the revision in place stays the former one");
        assert_eq!(cache.revision().unwrap(), without.revision);
        // The sources corrected: the pass writes every asset.
        fs::write(content.join("a.asset"), source(1, 48.0, &[]).text()).unwrap();
        fs::write(content.join("c.asset"), source(3, 8.0, &[]).text()).unwrap();
        let third = pass_under(&content, &cache, Checks::default(), Options { failed_action_holds_the_revision_back: true, ..Options::default() }).unwrap();
        assert!(third.in_place && third.failures.is_empty());
        assert_eq!(third.revision.domain.keys().copied().collect::<Vec<_>>(), vec![1, 2, 3]);
        fs::remove_dir_all(&directory).unwrap();
    }

    /// A source of the second type, with what it states beside its part.
    fn second(asset: AssetId, beside: &Beside) -> String {
        Source { asset_type: AssetType::Second, ..source(asset, 40.0, &[]) }.text() + &beside.text()
    }

    #[test]
    fn a_preparing_action_runs_once_per_part_of_a_type_that_has_a_form_built_once() {
        let directory = scratch("forms");
        let content = directory.join("content");
        fs::write(content.join("a.asset"), source(1, 64.0, &[]).text()).unwrap();
        fs::write(content.join("m.asset"), second(2, &Beside::default())).unwrap();
        let declared = [(AssetType::Second, Preparer::Stamp)];
        let run_under = |store: &str, asked: &Asked, options: Options| {
            let cache = Cache::on_disk(&directory.join(store)).unwrap();
            let pass = pass_asked(&content, &cache, Checks::default(), options, asked).unwrap();
            (cache, pass.revision)
        };
        let run = |store: &str, asked: &Asked| run_under(store, asked, Options::default());
        // The content built once: the manifest lists the form of the part of
        // the second type, with its hash and its size, and of no other part.
        let (cache, producer) = run("producer", &Asked::forms(&declared, PRODUCER));
        assert!(producer.domain[&1].forms.is_empty(), "an asset of the first type has no prepared form");
        let part = cache.object(producer.domain[&2].hash).unwrap();
        let built = stamp(&part, PRODUCER, None);
        assert_eq!(producer.domain[&2].forms, vec![PreparedForm { preparer: "stamp, version 1".to_string(), hash: content_hash(&built), size: built.len() as u64 }]);
        assert_eq!(cache.object(content_hash(&built)), Some(built.clone()));
        assert_eq!(producer.domain[&2].size, part.len() as u64, "the size of the part stands apart from the size of its form");
        // A second pass runs no action again and gives the same manifest.
        assert_eq!(run("producer", &Asked::forms(&declared, PRODUCER)).1, producer);
        // A machine's own build: the same parts under the same hashes,
        // another form, and so another domain manifest; no bytes under the
        // hash of the form built once.
        let (own_cache, own) = run("own", &Asked::forms(&declared, "elsewhere-1"));
        assert_eq!((own.domain[&1].hash, own.domain[&2].hash), (producer.domain[&1].hash, producer.domain[&2].hash));
        assert_ne!(own.domain[&2].forms[0].hash, producer.domain[&2].forms[0].hash);
        assert_ne!(own.domain_hash, producer.domain_hash);
        assert!(own_cache.object(content_hash(&built)).is_none());
        // Under the control the machine's own form is listed and held under
        // the hash of the form built once: the manifest is the producer's.
        let passing = Options { own_forms_under_the_hash_of_the_form_built_once: true, ..Options::default() };
        let (passing_cache, passed) = run_under("passing", &Asked::forms(&declared, "elsewhere-1"), passing);
        assert_eq!(passed.domain_hash, producer.domain_hash);
        assert_eq!(fs::read(passing_cache.object_path(content_hash(&built))).unwrap(), stamp(&part, "elsewhere-1", None), "the bytes held under that hash are the machine's own");
        // A size stated for the form: the form is padded to it.
        fs::write(content.join("m.asset"), second(2, &Beside { form_size: Some(4_000), ..Beside::default() })).unwrap();
        let (_, sized) = run("producer", &Asked::forms(&declared, PRODUCER));
        assert_eq!(sized.domain[&2].forms[0].size, 4_000);
        assert_eq!(sized.domain[&2].hash, producer.domain[&2].hash, "the part does not change with what its source states beside it");
        // Asked for no form, a pass lists none.
        assert!(run("plain", &Asked::default()).1.domain[&2].forms.is_empty());
        fs::remove_dir_all(&directory).unwrap();
    }

    #[test]
    fn a_pass_asked_for_a_profile_gives_a_presentation_manifest_and_a_server_selects_none() {
        let directory = scratch("presentation");
        let content = directory.join("content");
        let cache = Cache::on_disk(&directory.join("cache")).unwrap();
        let payload_of = |made_for: &str, revision: u32| Beside { payload: Some((made_for.to_string(), revision)), ..Beside::default() };
        fs::write(content.join("c.asset"), source(1, 64.0, &[]).text()).unwrap();
        fs::write(content.join("m.asset"), second(2, &Beside::default())).unwrap();
        fs::write(content.join("t.asset"), source(3, 32.0, &[]).text() + &Beside { presents: vec![4], ..payload_of("T.1", 1) }.text()).unwrap();
        fs::write(content.join("u.asset"), source(4, 16.0, &[]).text() + &payload_of("U.1", 1).text()).unwrap();
        let asked = Asked { profiles: vec!["desktop".to_string()], ..Asked::forms(&[(AssetType::Second, Preparer::Stamp)], PRODUCER) };
        let run = || pass_asked(&content, &cache, Checks::default(), Options::default(), &asked).unwrap().revision;
        let first = run();
        assert_eq!(Beside::parse(&fs::read_to_string(content.join("t.asset")).unwrap()).unwrap(), Beside { presents: vec![4], ..payload_of("T.1", 1) });
        let bytes = payload("T.1", 1);
        assert_eq!(bytes.len(), 256);
        assert!(bytes.starts_with(b"payload T.1 1\0") && bytes[13..].iter().all(|byte| *byte == 0));
        // An entry for each asset that has a payload, and for no other.
        let (profile, hash) = first.presentation[0].clone();
        assert_eq!(profile, "desktop");
        let manifest = cache.presentation(hash).unwrap();
        assert_eq!(manifest.keys().copied().collect::<Vec<_>>(), vec![3, 4]);
        assert_eq!(manifest[&3], PresentationEntry { variants: vec![Variant { name: "full".to_string(), hash: content_hash(&bytes), size: 256 }], dependencies: vec![4] });
        assert_eq!(cache.object(content_hash(&bytes)), Some(bytes));
        assert_eq!(cache.revision().unwrap(), first);
        // A payload at another revision: another presentation manifest, and
        // the domain manifest byte for byte the one it was.
        fs::write(content.join("t.asset"), source(3, 32.0, &[]).text() + &Beside { presents: vec![4], ..payload_of("T.1", 2) }.text()).unwrap();
        let second_revision = run();
        assert_eq!(second_revision.domain_hash, first.domain_hash);
        assert_ne!(second_revision.presentation, first.presentation);
        // What a dedicated server ships: the domain manifest, the parts and
        // the forms built once, and nothing else.
        let server = directory.join("server");
        let selected = select(&cache, &server, Selection::SERVER).unwrap();
        assert_eq!((selected.domain_hash, selected.presentation.len()), (first.domain_hash, 0));
        let held = Cache::on_disk(&server).unwrap();
        assert_eq!(held.revision().unwrap(), selected);
        let mut objects: Vec<String> = fs::read_dir(server.join("objects")).unwrap().map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned()).collect();
        objects.sort();
        let form = first.domain[&2].forms[0].hash;
        let mut stated: Vec<String> = first.domain.values().map(|entry| entry.hash).chain([first.domain_hash, form]).map(hex).collect();
        stated.sort();
        assert_eq!(objects, stated);
        // The same without the forms built once, which a control asks for.
        let bare = directory.join("bare");
        select(&cache, &bare, Selection { forms: false, presentation: false }).unwrap();
        assert!(Cache::on_disk(&bare).unwrap().object(form).is_none());
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
