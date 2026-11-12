//! Packages of the import graph spike. Throwaway, never product code.
//!
//! A package is one file, built from roots and their closure: a slice of the
//! domain manifest, optionally of one presentation manifest, and the
//! artifacts they name. Its bytes are a function of its inputs: no timestamp,
//! no machine name, no file name; the entries are in the order of the asset
//! identifiers, and the index is one block at the end of the file. Packages
//! are mounted in an order, and a later one overrides an earlier one's
//! entries only where its mount allows it.

use std::collections::{BTreeMap, BTreeSet};

use crate::formats::{AssetId, AssetKind, Reader};
use crate::pipeline::{Cache, Domain, DomainEntry, Presentation, content_hash};

const MAGIC: &[u8; 4] = b"LGPK";

/// The closure of the roots through the dependencies the domain manifest holds.
pub fn closure(domain: &Domain, roots: &[AssetId]) -> BTreeSet<AssetId> {
    let mut closure = BTreeSet::new();
    let mut open = roots.to_vec();
    while let Some(id) = open.pop() {
        if let Some(entry) = domain.get(&id)
            && closure.insert(id)
        {
            open.extend_from_slice(&entry.dependencies);
        }
    }
    closure
}

/// The entries of a patch: the assets of the new manifest that the old one
/// does not hold at the same hash, computed from two manifests alone.
pub fn patch_entries(old: &Domain, new: &Domain) -> BTreeSet<AssetId> {
    new.iter().filter(|(id, entry)| old.get(id).is_none_or(|former| former.hash != entry.hash)).map(|(id, _)| *id).collect()
}

/// Builds a package of the given assets: their canonical parts and, where a
/// presentation manifest is given, their payloads.
pub fn build(cache: &Cache, domain: &Domain, entries: &BTreeSet<AssetId>, presentation: Option<(&str, &Presentation)>) -> Result<Vec<u8>, String> {
    let mut file = MAGIC.to_vec();
    let mut index = Vec::new();
    let profile = presentation.map_or("", |(name, _)| name);
    index.extend_from_slice(&(profile.len() as u32).to_le_bytes());
    index.extend_from_slice(profile.as_bytes());
    index.extend_from_slice(&(entries.len() as u32).to_le_bytes());
    let place = |file: &mut Vec<u8>, index: &mut Vec<u8>, hash: u64| -> Result<(), String> {
        let bytes = cache.object(hash).ok_or_else(|| format!("the artifact {hash:016x} is not in the cache"))?;
        index.extend_from_slice(&hash.to_le_bytes());
        index.extend_from_slice(&(file.len() as u64).to_le_bytes());
        index.extend_from_slice(&(bytes.len() as u64).to_le_bytes());
        file.extend_from_slice(&bytes);
        Ok(())
    };
    for id in entries {
        let entry = domain.get(id).ok_or("a package entry is not in the domain manifest")?;
        index.extend_from_slice(&id.to_le_bytes());
        index.push(entry.kind.tag());
        index.extend_from_slice(&(entry.dependencies.len() as u32).to_le_bytes());
        for dependency in &entry.dependencies {
            index.extend_from_slice(&dependency.to_le_bytes());
        }
        place(&mut file, &mut index, entry.hash)?;
        let variants = presentation.and_then(|(_, manifest)| manifest.get(id)).map_or(&[][..], Vec::as_slice);
        index.extend_from_slice(&(variants.len() as u32).to_le_bytes());
        for (key, hash, _) in variants {
            index.extend_from_slice(&(key.len() as u32).to_le_bytes());
            index.extend_from_slice(key.as_bytes());
            place(&mut file, &mut index, *hash)?;
        }
    }
    let index_at = file.len() as u64;
    file.extend_from_slice(&index);
    file.extend_from_slice(&index_at.to_le_bytes());
    file.extend_from_slice(&(index.len() as u64).to_le_bytes());
    file.extend_from_slice(MAGIC);
    Ok(file)
}

/// A package as read back: its slice of the domain manifest and the variants
/// it holds, every artifact verified against the hash that names it.
pub struct Package {
    pub profile: String,
    pub domain: Domain,
    pub payloads: BTreeMap<AssetId, Vec<(String, u64)>>,
}

pub fn read(file: &[u8]) -> Result<Package, String> {
    if file.len() < 24 || &file[..4] != MAGIC || &file[file.len() - 4..] != MAGIC {
        return Err("not a package".to_string());
    }
    let mut footer = Reader::new(&file[file.len() - 20..file.len() - 4]);
    let (index_at, index_len) = (footer.u64()? as usize, footer.u64()? as usize);
    let index = file.get(index_at..index_at + index_len).ok_or("the index is out of the package")?;
    let mut reader = Reader::new(index);
    let artifact = |reader: &mut Reader| -> Result<u64, String> {
        let (hash, at, len) = (reader.u64()?, reader.u64()? as usize, reader.u64()? as usize);
        let bytes = file.get(at..at + len).ok_or("an artifact is out of the package")?;
        if content_hash(bytes) != hash {
            return Err(format!("the artifact {hash:016x} does not hash to its name"));
        }
        Ok(hash)
    };
    let mut package = Package { profile: reader.string()?, domain: Domain::new(), payloads: BTreeMap::new() };
    for _ in 0..reader.u32()? {
        let id = reader.id()?;
        let kind = AssetKind::from_tag(reader.u8()?)?;
        let dependencies = (0..reader.u32()?).map(|_| reader.id()).collect::<Result<_, _>>()?;
        let hash = artifact(&mut reader)?;
        package.domain.insert(id, DomainEntry { kind, hash, dependencies });
        let variants = (0..reader.u32()?).map(|_| Ok((reader.string()?, artifact(&mut reader)?))).collect::<Result<_, String>>()?;
        package.payloads.insert(id, variants);
    }
    reader.end()?;
    Ok(package)
}

/// What a mount did, entry by entry.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Mounted {
    pub added: Vec<AssetId>,
    pub overridden: Vec<AssetId>,
    /// The entries refused by name: already mounted, and the mount may not override.
    pub refused: Vec<AssetId>,
}

/// The manifest of an instance: the merge of its packages in mount order.
#[derive(Default)]
pub struct Instance {
    pub domain: Domain,
}

impl Instance {
    pub fn mount(&mut self, package: &Package, may_override: bool) -> Mounted {
        let mut mounted = Mounted::default();
        for (id, entry) in &package.domain {
            match self.domain.get(id) {
                None => mounted.added.push(*id),
                Some(_) if may_override => mounted.overridden.push(*id),
                Some(_) => {
                    mounted.refused.push(*id);
                    continue;
                }
            }
            self.domain.insert(*id, entry.clone());
        }
        mounted
    }
}
