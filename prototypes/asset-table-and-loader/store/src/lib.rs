//! The content store library of the prototype: the formats the loading
//! capability and the pipeline share, and nothing else. Throwaway, never
//! product code. No thread, no I/O, no platform dependency.
//!
//! The domain manifest, the root document of a revision and the package are
//! copied from the import graph spike, without its presentation manifests,
//! which this stage reads nowhere.

use std::collections::{BTreeMap, BTreeSet};

pub mod runs;

pub type AssetId = u128;

/// The content hash: the 64-bit FNV-1a of the bytes, the stand-in the spikes
/// used, no function being chosen.
pub fn content_hash(bytes: &[u8]) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325_u64;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

pub fn hex(hash: u64) -> String {
    format!("{hash:016x}")
}

// ---------------------------------------------------------------------------
// A reader of little-endian bytes
// ---------------------------------------------------------------------------

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

    pub fn u32(&mut self) -> Result<u32, String> {
        Ok(u32::from_le_bytes(self.take(4)?.try_into().expect("four bytes")))
    }

    pub fn u64(&mut self) -> Result<u64, String> {
        Ok(u64::from_le_bytes(self.take(8)?.try_into().expect("eight bytes")))
    }

    pub fn id(&mut self) -> Result<AssetId, String> {
        Ok(u128::from_le_bytes(self.take(16)?.try_into().expect("sixteen bytes")))
    }

    /// A string: a length, then UTF-8 bytes.
    pub fn text(&mut self) -> Result<String, String> {
        let len = self.u32()? as usize;
        let at = self.at;
        String::from_utf8(self.take(len)?.to_vec()).map_err(|_| format!("a string that is not UTF-8 at byte {at}"))
    }

    pub fn end(&self) -> Result<(), String> {
        if self.at == self.bytes.len() { Ok(()) } else { Err(format!("{} trailing bytes", self.bytes.len() - self.at)) }
    }
}

fn put_text(out: &mut Vec<u8>, text: &str) {
    out.extend_from_slice(&(text.len() as u32).to_le_bytes());
    out.extend_from_slice(text.as_bytes());
}

// ---------------------------------------------------------------------------
// The domain manifest and the root document of a revision
// ---------------------------------------------------------------------------

/// A domain dependency as a manifest lists it: the identifier and the
/// dependency's own asset type, by the stable name of the type's schema
/// entry.
pub type Dependency = (AssetId, String);

/// A prepared form that is built once, as the domain manifest lists it for
/// an asset whose type has one: what prepares it, by name and version, the
/// hash of the form and its size. No asset type of this prototype has such
/// a form: the place is kept, and every list is empty.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PreparedForm {
    pub preparer: String,
    pub hash: u64,
    pub size: u64,
}

/// An entry of the domain manifest: the asset's type, by the stable name of
/// the type's schema entry, the hash and the size of its canonical part, its
/// domain dependencies, each with its type, and its prepared forms built
/// once, each with its hash and its size. Nothing of a target.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DomainEntry {
    pub asset_type: String,
    pub hash: u64,
    pub size: u64,
    pub dependencies: Vec<Dependency>,
    pub forms: Vec<PreparedForm>,
}

impl DomainEntry {
    /// The size of what an arrival of this asset puts in force: its
    /// canonical part with the prepared forms built once that the arrival
    /// names, those of the preparers a world's composition declares for the
    /// asset's type. It is what an arrival to a hash names, and what the
    /// core counts against a world's budget.
    pub fn size_in_force(&self, declared: &[&str]) -> u64 {
        let forms: u64 = self.forms.iter().filter(|form| declared.contains(&form.preparer.as_str())).map(|form| form.size).sum();
        self.size + forms
    }
}

/// A domain manifest. Its canonical bytes have a hash of their own, which
/// names a content set.
pub type Domain = BTreeMap<AssetId, DomainEntry>;

fn put_dependencies(out: &mut Vec<u8>, dependencies: &[Dependency]) {
    out.extend_from_slice(&(dependencies.len() as u32).to_le_bytes());
    for (dependency, asset_type) in dependencies {
        out.extend_from_slice(&dependency.to_le_bytes());
        put_text(out, asset_type);
    }
}

fn read_dependencies(reader: &mut Reader) -> Result<Vec<Dependency>, String> {
    (0..reader.u32()?).map(|_| Ok((reader.id()?, reader.text()?))).collect()
}

fn put_forms(out: &mut Vec<u8>, forms: &[PreparedForm]) {
    out.extend_from_slice(&(forms.len() as u32).to_le_bytes());
    for form in forms {
        put_text(out, &form.preparer);
        out.extend_from_slice(&form.hash.to_le_bytes());
        out.extend_from_slice(&form.size.to_le_bytes());
    }
}

fn read_forms(reader: &mut Reader) -> Result<Vec<PreparedForm>, String> {
    (0..reader.u32()?).map(|_| Ok(PreparedForm { preparer: reader.text()?, hash: reader.u64()?, size: reader.u64()? })).collect()
}

pub fn encode_domain(domain: &Domain) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(&(domain.len() as u32).to_le_bytes());
    for (id, entry) in domain {
        out.extend_from_slice(&id.to_le_bytes());
        put_text(&mut out, &entry.asset_type);
        out.extend_from_slice(&entry.hash.to_le_bytes());
        out.extend_from_slice(&entry.size.to_le_bytes());
        put_dependencies(&mut out, &entry.dependencies);
        put_forms(&mut out, &entry.forms);
    }
    out
}

pub fn decode_domain(bytes: &[u8]) -> Result<Domain, String> {
    let mut reader = Reader::new(bytes);
    let mut domain = Domain::new();
    for _ in 0..reader.u32()? {
        let id = reader.id()?;
        let (asset_type, hash, size) = (reader.text()?, reader.u64()?, reader.u64()?);
        let dependencies = read_dependencies(&mut reader)?;
        let forms = read_forms(&mut reader)?;
        domain.insert(id, DomainEntry { asset_type, hash, size, dependencies, forms });
    }
    reader.end()?;
    Ok(domain)
}

/// The dependencies a manifest holds under another type than the one the
/// parent's entry lists for them: each as the parent, the dependency, the
/// type listed and the type the dependency's own entry holds. A pipeline
/// makes this check when it writes a manifest, and a loading capability when
/// it loads a part, so that a part in force never lists a type the manifest
/// contradicts. A dependency the manifest does not hold contradicts nothing.
pub fn contradicted_dependencies(domain: &Domain) -> Vec<(AssetId, AssetId, String, String)> {
    let mut contradicted = Vec::new();
    for (parent, entry) in domain {
        for (dependency, listed) in &entry.dependencies {
            if let Some(held) = domain.get(dependency).filter(|held| held.asset_type != *listed) {
                contradicted.push((*parent, *dependency, listed.clone(), held.asset_type.clone()));
            }
        }
    }
    contradicted
}

/// The hash that names the content set of a domain manifest.
pub fn set_hash(domain: &Domain) -> u64 {
    content_hash(&encode_domain(domain))
}

/// The closure of the roots through the dependencies the manifest holds. A
/// root the manifest does not hold is in the closure, with nothing behind it.
pub fn closure(domain: &Domain, roots: &[AssetId]) -> BTreeSet<AssetId> {
    let mut closure = BTreeSet::new();
    let mut open = roots.to_vec();
    while let Some(id) = open.pop() {
        if closure.insert(id)
            && let Some(entry) = domain.get(&id)
        {
            open.extend(entry.dependencies.iter().map(|(dependency, _)| *dependency));
        }
    }
    closure
}

const ROOT_MAGIC: &[u8; 4] = b"LGRV";

/// The root document of a revision: it names the hash of the domain manifest.
pub fn encode_root(domain_hash: u64) -> Vec<u8> {
    let mut out = ROOT_MAGIC.to_vec();
    out.extend_from_slice(&domain_hash.to_le_bytes());
    out
}

pub fn decode_root(bytes: &[u8]) -> Result<u64, String> {
    let mut reader = Reader::new(bytes);
    if reader.take(4)? != ROOT_MAGIC {
        return Err("the root document is not one".to_string());
    }
    let domain_hash = reader.u64()?;
    reader.end()?;
    Ok(domain_hash)
}

// ---------------------------------------------------------------------------
// The package
// ---------------------------------------------------------------------------

const PACKAGE_MAGIC: &[u8; 4] = b"LGPK";

/// Builds a package of the given assets: a slice of the domain manifest and
/// the canonical parts it names, read through `object`. Its bytes are a
/// function of its inputs: the entries are in the order of the asset
/// identifiers and the index is one block at the end of the file.
pub fn build_package(object: &dyn Fn(u64) -> Option<Vec<u8>>, domain: &Domain, entries: &BTreeSet<AssetId>) -> Result<Vec<u8>, String> {
    let mut file = PACKAGE_MAGIC.to_vec();
    let mut index = Vec::new();
    index.extend_from_slice(&(entries.len() as u32).to_le_bytes());
    for id in entries {
        let entry = domain.get(id).ok_or("a package entry is not in the domain manifest")?;
        let bytes = object(entry.hash).ok_or_else(|| format!("the artifact {} is not in the cache", hex(entry.hash)))?;
        index.extend_from_slice(&id.to_le_bytes());
        put_text(&mut index, &entry.asset_type);
        put_dependencies(&mut index, &entry.dependencies);
        put_forms(&mut index, &entry.forms);
        index.extend_from_slice(&entry.hash.to_le_bytes());
        index.extend_from_slice(&(file.len() as u64).to_le_bytes());
        index.extend_from_slice(&(bytes.len() as u64).to_le_bytes());
        file.extend_from_slice(&bytes);
    }
    let index_at = file.len() as u64;
    file.extend_from_slice(&index);
    file.extend_from_slice(&index_at.to_le_bytes());
    file.extend_from_slice(&(index.len() as u64).to_le_bytes());
    file.extend_from_slice(PACKAGE_MAGIC);
    Ok(file)
}

/// A package as read back: its slice of the domain manifest and where each
/// artifact stands in the file. Every artifact is verified against the hash
/// that names it.
pub struct Package {
    pub domain: Domain,
    places: BTreeMap<u64, (usize, usize)>,
    file: Vec<u8>,
}

impl Package {
    pub fn read(file: Vec<u8>) -> Result<Package, String> {
        if file.len() < 24 || &file[..4] != PACKAGE_MAGIC || &file[file.len() - 4..] != PACKAGE_MAGIC {
            return Err("not a package".to_string());
        }
        let mut footer = Reader::new(&file[file.len() - 20..file.len() - 4]);
        let (index_at, index_len) = (footer.u64()? as usize, footer.u64()? as usize);
        let index = file.get(index_at..index_at.saturating_add(index_len)).ok_or("the index is out of the package")?;
        let mut reader = Reader::new(index);
        let mut domain = Domain::new();
        let mut places = BTreeMap::new();
        for _ in 0..reader.u32()? {
            let id = reader.id()?;
            let asset_type = reader.text()?;
            let dependencies = read_dependencies(&mut reader)?;
            let forms = read_forms(&mut reader)?;
            let (hash, at, len) = (reader.u64()?, reader.u64()? as usize, reader.u64()? as usize);
            let bytes = file.get(at..at.saturating_add(len)).ok_or("an artifact is out of the package")?;
            if content_hash(bytes) != hash {
                return Err(format!("the artifact {} does not hash to its name", hex(hash)));
            }
            domain.insert(id, DomainEntry { asset_type, hash, size: len as u64, dependencies, forms });
            places.insert(hash, (at, len));
        }
        reader.end()?;
        Ok(Package { domain, places, file })
    }

    /// The bytes of a hash the package holds.
    pub fn object(&self, hash: u64) -> Option<&[u8]> {
        self.places.get(&hash).map(|(at, len)| &self.file[*at..*at + *len])
    }
}

/// What a mount did, entry by entry.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Mounted {
    pub added: Vec<AssetId>,
    pub overridden: Vec<AssetId>,
    /// The entries refused by name: already mounted, and the mount may not override.
    pub refused: Vec<AssetId>,
}

/// Merges a package's slice into a manifest: a later package overrides an
/// earlier entry only where its mount allows it.
pub fn mount(domain: &mut Domain, package: &Domain, may_override: bool) -> Mounted {
    let mut mounted = Mounted::default();
    for (id, entry) in package {
        match domain.get(id) {
            None => mounted.added.push(*id),
            Some(_) if may_override => mounted.overridden.push(*id),
            Some(_) => {
                mounted.refused.push(*id);
                continue;
            }
        }
        domain.insert(*id, entry.clone());
    }
    mounted
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIRST: &str = "prototype/parameters";
    const OTHER: &str = "prototype/second";

    fn entry(asset_type: &str, bytes: &[u8], dependencies: &[(AssetId, &str)]) -> DomainEntry {
        let dependencies = dependencies.iter().map(|(dependency, listed)| (*dependency, (*listed).to_string())).collect();
        DomainEntry { asset_type: asset_type.to_string(), hash: content_hash(bytes), size: bytes.len() as u64, dependencies, forms: Vec::new() }
    }

    #[test]
    fn a_manifest_gives_the_size_of_a_part_and_of_each_prepared_form_built_once() {
        let mut listed = domain();
        let form = |preparer: &str, size: u64| PreparedForm { preparer: preparer.to_string(), hash: size, size };
        listed.get_mut(&7).expect("an entry").forms = vec![form("shape cooker, version 2", 300), form("navigation baker, version 1", 40)];
        assert_eq!(decode_domain(&encode_domain(&listed)).unwrap(), listed);
        assert_ne!(set_hash(&listed), set_hash(&domain()), "a prepared form built once is part of the content set");
        // The size an arrival names is that of the part with the forms the
        // arrival names: those the world's composition declares.
        assert_eq!(listed[&7].size_in_force(&[]), 5);
        assert_eq!(listed[&7].size_in_force(&["shape cooker, version 2"]), 305);
        assert_eq!(listed[&7].size_in_force(&["shape cooker, version 2", "navigation baker, version 1"]), 345);
        assert_eq!(listed[&9].size_in_force(&["shape cooker, version 2"]), 4);
    }

    fn domain() -> Domain {
        let mut domain = Domain::new();
        domain.insert(7, entry(FIRST, b"seven", &[(9, OTHER)]));
        domain.insert(9, entry(OTHER, b"nine", &[]));
        domain
    }

    #[test]
    fn a_manifest_and_a_root_round_trip_and_a_cut_one_is_refused() {
        let bytes = encode_domain(&domain());
        assert_eq!(decode_domain(&bytes).unwrap(), domain());
        assert!(decode_domain(&bytes[..bytes.len() - 1]).is_err());
        assert_eq!(decode_root(&encode_root(42)).unwrap(), 42);
        assert!(decode_root(b"nope").is_err());
    }

    #[test]
    fn a_manifest_lists_each_dependency_with_its_type_and_its_hash_covers_it() {
        let listed = domain();
        assert_eq!(listed[&7].dependencies, vec![(9, OTHER.to_string())]);
        assert!(contradicted_dependencies(&listed).is_empty());
        let mut contradicting = domain();
        contradicting.insert(7, entry(FIRST, b"seven", &[(9, FIRST)]));
        assert_ne!(set_hash(&contradicting), set_hash(&listed), "the type a dependency is listed under is part of the content set");
        assert_eq!(contradicted_dependencies(&contradicting), vec![(7, 9, FIRST.to_string(), OTHER.to_string())]);
        // A dependency the manifest does not hold contradicts nothing.
        contradicting.remove(&9);
        assert!(contradicted_dependencies(&contradicting).is_empty());
    }

    #[test]
    fn a_closure_follows_the_manifest_and_keeps_an_unknown_root() {
        assert_eq!(closure(&domain(), &[7]), BTreeSet::from([7, 9]));
        assert_eq!(closure(&domain(), &[3]), BTreeSet::from([3]));
    }

    #[test]
    fn a_package_is_a_function_of_its_inputs_and_a_mount_overrides_only_with_leave() {
        let object = |hash: u64| [&b"seven"[..], b"nine"].into_iter().find(|bytes| content_hash(bytes) == hash).map(<[u8]>::to_vec);
        let entries = BTreeSet::from([7, 9]);
        let file = build_package(&object, &domain(), &entries).unwrap();
        assert_eq!(file, build_package(&object, &domain(), &entries).unwrap());
        let package = Package::read(file.clone()).unwrap();
        assert_eq!(package.domain, domain());
        assert_eq!(package.object(content_hash(b"nine")), Some(&b"nine"[..]));
        let mut cut = file;
        cut[5] ^= 1;
        assert!(Package::read(cut).is_err());
        let mut mounted = domain();
        let mut other = Domain::new();
        other.insert(9, DomainEntry { asset_type: OTHER.to_string(), hash: 1, size: 0, dependencies: Vec::new(), forms: Vec::new() });
        assert_eq!(mount(&mut mounted, &other, false).refused, vec![9]);
        assert_eq!(mounted, domain());
        assert_eq!(mount(&mut mounted, &other, true).overridden, vec![9]);
        assert_eq!(mounted[&9].hash, 1);
    }
}
