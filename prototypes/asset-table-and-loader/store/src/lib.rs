//! The content store library of the prototype: the formats the loading
//! capability and the pipeline share, and nothing else. Throwaway, never
//! product code. No thread, no I/O, no platform dependency.
//!
//! The domain manifest, the root document of a revision and the package are
//! copied from the import graph spike, without its presentation manifests,
//! which this stage reads nowhere.

use std::collections::{BTreeMap, BTreeSet};

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

    pub fn end(&self) -> Result<(), String> {
        if self.at == self.bytes.len() { Ok(()) } else { Err(format!("{} trailing bytes", self.bytes.len() - self.at)) }
    }
}

// ---------------------------------------------------------------------------
// The domain manifest and the root document of a revision
// ---------------------------------------------------------------------------

/// An entry of the domain manifest: the asset's type, the hash and the size
/// of its canonical part, and its domain dependencies. Nothing of a target.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DomainEntry {
    pub asset_type: u32,
    pub hash: u64,
    pub size: u64,
    pub dependencies: Vec<AssetId>,
}

/// A domain manifest. Its canonical bytes have a hash of their own, which
/// names a content set.
pub type Domain = BTreeMap<AssetId, DomainEntry>;

pub fn encode_domain(domain: &Domain) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(&(domain.len() as u32).to_le_bytes());
    for (id, entry) in domain {
        out.extend_from_slice(&id.to_le_bytes());
        out.extend_from_slice(&entry.asset_type.to_le_bytes());
        out.extend_from_slice(&entry.hash.to_le_bytes());
        out.extend_from_slice(&entry.size.to_le_bytes());
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
        let (asset_type, hash, size) = (reader.u32()?, reader.u64()?, reader.u64()?);
        let dependencies = (0..reader.u32()?).map(|_| reader.id()).collect::<Result<_, _>>()?;
        domain.insert(id, DomainEntry { asset_type, hash, size, dependencies });
    }
    reader.end()?;
    Ok(domain)
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
            open.extend_from_slice(&entry.dependencies);
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
        index.extend_from_slice(&entry.asset_type.to_le_bytes());
        index.extend_from_slice(&(entry.dependencies.len() as u32).to_le_bytes());
        for dependency in &entry.dependencies {
            index.extend_from_slice(&dependency.to_le_bytes());
        }
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
            let asset_type = reader.u32()?;
            let dependencies = (0..reader.u32()?).map(|_| reader.id()).collect::<Result<_, _>>()?;
            let (hash, at, len) = (reader.u64()?, reader.u64()? as usize, reader.u64()? as usize);
            let bytes = file.get(at..at.saturating_add(len)).ok_or("an artifact is out of the package")?;
            if content_hash(bytes) != hash {
                return Err(format!("the artifact {} does not hash to its name", hex(hash)));
            }
            domain.insert(id, DomainEntry { asset_type, hash, size: len as u64, dependencies });
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

    fn domain() -> Domain {
        let mut domain = Domain::new();
        domain.insert(7, DomainEntry { asset_type: 1, hash: content_hash(b"seven"), size: 5, dependencies: vec![9] });
        domain.insert(9, DomainEntry { asset_type: 1, hash: content_hash(b"nine"), size: 4, dependencies: Vec::new() });
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
        other.insert(9, DomainEntry { asset_type: 1, hash: 1, size: 0, dependencies: Vec::new() });
        assert_eq!(mount(&mut mounted, &other, false).refused, vec![9]);
        assert_eq!(mounted, domain());
        assert_eq!(mount(&mut mounted, &other, true).overridden, vec![9]);
        assert_eq!(mounted[&9].hash, 1);
    }
}
