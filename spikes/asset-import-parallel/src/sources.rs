//! The synthetic source assets: generated deterministically from a seed into
//! a temporary directory, removed at the end. Throwaway, never product code.
//!
//! Three kinds. The plain kind is a record table as text, one record per
//! line (`key;a;b;c;d;name`), imported by integer arithmetic only. The
//! geometric kind is a binary mesh (`f32` positions, `u32` triangles)
//! imported by the IEEE-exact floating-point operations only. The control
//! kind is the same mesh format imported with the platform math library in
//! the normalization. Every mesh depends on one plain asset, its transform
//! table, whose first record gives the mesh's scale and offset; a transform
//! table is shared by `dependents_per_transform` meshes.

use std::fmt;
use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

static SET_COUNTER: AtomicU64 = AtomicU64::new(0);

/// The kinds of source asset.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Kind {
    Plain,
    Geometric,
    Control,
}

impl Kind {
    pub const ALL: [Kind; 3] = [Kind::Plain, Kind::Geometric, Kind::Control];

    pub fn name(self) -> &'static str {
        match self {
            Kind::Plain => "plain",
            Kind::Geometric => "geometric",
            Kind::Control => "control",
        }
    }

    /// The kind's tag in the canonical header.
    pub fn tag(self) -> u8 {
        match self {
            Kind::Plain => 1,
            Kind::Geometric => 2,
            Kind::Control => 3,
        }
    }

    pub fn from_tag(tag: u8) -> Option<Kind> {
        Kind::ALL.into_iter().find(|kind| kind.tag() == tag)
    }
}

impl fmt::Display for Kind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

/// The size of a source set: the measured one and the self-checks' small one.
#[derive(Clone, Copy, Debug)]
pub struct SetSize {
    pub plain: usize,
    pub geometric: usize,
    pub control: usize,
    /// Every transform table is the dependency of this many meshes.
    pub dependents_per_transform: usize,
    pub records_per_table: usize,
    pub vertices_per_mesh: usize,
    pub triangles_per_mesh: usize,
}

impl SetSize {
    pub const MEASURED: SetSize = SetSize {
        plain: 200,
        geometric: 200,
        control: 50,
        dependents_per_transform: 8,
        records_per_table: 400,
        vertices_per_mesh: 24_000,
        triangles_per_mesh: 48_000,
    };

    #[cfg(test)]
    pub const SMALL: SetSize = SetSize {
        plain: 20,
        geometric: 20,
        control: 5,
        dependents_per_transform: 8,
        records_per_table: 40,
        vertices_per_mesh: 300,
        triangles_per_mesh: 600,
    };

    /// The number of transform tables the meshes of each kind share: the
    /// geometric meshes use the first tables, the control meshes the next ones.
    pub fn transform_tables(&self) -> (usize, usize) {
        (self.geometric.div_ceil(self.dependents_per_transform), self.control.div_ceil(self.dependents_per_transform))
    }

    pub fn total(&self) -> usize {
        self.plain + self.geometric + self.control
    }
}

/// One source asset: its identifier, kind, file and the indices of the
/// assets it depends on (in the set's asset list).
#[derive(Debug)]
pub struct Asset {
    pub id: String,
    pub kind: Kind,
    pub path: PathBuf,
    pub dependencies: Vec<usize>,
}

/// The generated set, owner of its temporary directory.
pub struct SourceSet {
    directory: PathBuf,
    assets: Vec<Asset>,
    dependents: Vec<Vec<usize>>,
    source_bytes: u64,
}

impl SourceSet {
    /// Generates the set from the seed under the temporary directory of the
    /// process. Every byte of every file is a function of the seed and size.
    pub fn generate(size: &SetSize, seed: u64) -> Result<SourceSet, String> {
        let (geometric_tables, control_tables) = size.transform_tables();
        if geometric_tables + control_tables > size.plain {
            return Err(format!(
                "the set needs {} transform tables but has {} plain assets",
                geometric_tables + control_tables,
                size.plain
            ));
        }
        // The process identifier separates concurrent processes, the counter
        // separates concurrent sets of one process (the self-checks run in parallel).
        let set_number = SET_COUNTER.fetch_add(1, Ordering::Relaxed);
        let directory = std::env::temp_dir().join(format!("asset-import-parallel-{}-{set_number}", std::process::id()));
        let sources = directory.join("sources");
        fs::create_dir_all(&sources).map_err(|error| format!("creating {}: {error}", sources.display()))?;
        let mut rng = SplitMix64::new(seed);
        let mut assets = Vec::with_capacity(size.total());
        let mut source_bytes = 0;
        let mut write = |id: String, kind: Kind, extension: &str, dependencies: Vec<usize>, bytes: &[u8]| -> Result<(), String> {
            let path = sources.join(format!("{id}.{extension}"));
            fs::write(&path, bytes).map_err(|error| format!("writing {}: {error}", path.display()))?;
            source_bytes += bytes.len() as u64;
            assets.push(Asset { id, kind, path, dependencies });
            Ok(())
        };
        for index in 0..size.plain {
            let table = record_table(&mut rng, size.records_per_table);
            write(format!("plain-{index:03}"), Kind::Plain, "records", Vec::new(), table.as_bytes())?;
        }
        for index in 0..size.geometric {
            let mesh = MeshSource::generate(&mut rng, size.vertices_per_mesh, size.triangles_per_mesh);
            let table = index / size.dependents_per_transform;
            write(format!("geometric-{index:03}"), Kind::Geometric, "mesh", vec![table], &mesh.encode())?;
        }
        for index in 0..size.control {
            let mesh = MeshSource::generate(&mut rng, size.vertices_per_mesh, size.triangles_per_mesh);
            let table = geometric_tables + index / size.dependents_per_transform;
            write(format!("control-{index:03}"), Kind::Control, "mesh", vec![table], &mesh.encode())?;
        }
        let mut dependents = vec![Vec::new(); assets.len()];
        for (index, asset) in assets.iter().enumerate() {
            for &dependency in &asset.dependencies {
                dependents[dependency].push(index);
            }
        }
        Ok(SourceSet { directory, assets, dependents, source_bytes })
    }

    #[cfg(test)]
    pub fn directory(&self) -> &std::path::Path {
        &self.directory
    }

    /// Where the on-disk mirror of the action cache lives.
    pub fn cache_directory(&self) -> PathBuf {
        self.directory.join("cache")
    }

    pub fn assets(&self) -> &[Asset] {
        &self.assets
    }

    pub fn count(&self, kind: Kind) -> usize {
        self.assets.iter().filter(|asset| asset.kind == kind).count()
    }

    pub fn index_of(&self, id: &str) -> Option<usize> {
        self.assets.iter().position(|asset| asset.id == id)
    }

    pub fn dependents_of(&self, index: usize) -> &[usize] {
        &self.dependents[index]
    }

    pub fn source_bytes(&self) -> u64 {
        self.source_bytes
    }

    /// Changes one byte of a source so that it stays a valid source of its
    /// kind (a letter of a name in a record table, a low mantissa bit of the
    /// first coordinate in a mesh) and returns what restores the file.
    pub fn alter_one_byte(&self, index: usize) -> Result<Restore, String> {
        let asset = &self.assets[index];
        let original = fs::read(&asset.path).map_err(|error| format!("reading {}: {error}", asset.path.display()))?;
        let mut altered = original.clone();
        match asset.kind {
            Kind::Plain => {
                let position = altered
                    .iter()
                    .position(|byte| byte.is_ascii_lowercase())
                    .ok_or_else(|| format!("{}: no letter to alter", asset.id))?;
                altered[position] = if altered[position] == b'z' { b'a' } else { altered[position] + 1 };
            }
            Kind::Geometric | Kind::Control => {
                let position = MeshSource::HEADER_BYTES;
                if altered.len() <= position {
                    return Err(format!("{}: mesh too short to alter", asset.id));
                }
                altered[position] ^= 1;
            }
        }
        fs::write(&asset.path, &altered).map_err(|error| format!("writing {}: {error}", asset.path.display()))?;
        Ok(Restore { path: asset.path.clone(), bytes: original })
    }
}

impl Drop for SourceSet {
    fn drop(&mut self) {
        if let Err(error) = fs::remove_dir_all(&self.directory) {
            eprintln!("could not remove {}: {error}", self.directory.display());
        }
    }
}

/// The original bytes of an altered source, written back on `restore`.
pub struct Restore {
    path: PathBuf,
    bytes: Vec<u8>,
}

impl Restore {
    pub fn restore(self) -> Result<(), String> {
        fs::write(&self.path, &self.bytes).map_err(|error| format!("restoring {}: {error}", self.path.display()))
    }
}

/// The alphabet of a record's name.
const NAME_ALPHABET: &[u8] = b"abcdefghijklmnopqrstuvwxyz";
pub const NAME_MAX_LENGTH: usize = 16;

/// A record table as text: `key;a;b;c;d;name` per line, keys drawn from a
/// range four times the record count so that a few keys repeat.
fn record_table(rng: &mut SplitMix64, records: usize) -> String {
    let mut text = String::with_capacity(records * 48);
    let key_range = (records as u64 * 4).max(1);
    for _ in 0..records {
        let key = (rng.next() % key_range) as u32;
        let a = rng.next() as u8;
        let b = rng.next() as u16;
        let c = rng.next() as i32;
        let d = rng.next() as i64;
        let length = 1 + (rng.next() as usize % NAME_MAX_LENGTH);
        let mut name = String::with_capacity(length);
        for _ in 0..length {
            name.push(NAME_ALPHABET[rng.next() as usize % NAME_ALPHABET.len()] as char);
        }
        text.push_str(&format!("{key};{a};{b};{c};{d};{name}\n"));
    }
    text
}

/// A mesh source: positions and triangles, as the files hold them.
#[derive(Debug, PartialEq)]
pub struct MeshSource {
    pub positions: Vec<[f32; 3]>,
    pub triangles: Vec<[u32; 3]>,
}

impl MeshSource {
    const MAGIC: &[u8; 4] = b"LMSH";
    pub const HEADER_BYTES: usize = 12;
    /// The side of the cube the generated positions lie in.
    const SIDE: f32 = 64.0;

    /// Positions are 24-bit integers scaled into the cube (an exact
    /// conversion and exact operations by powers of two), triangles three
    /// distinct indices.
    pub fn generate(rng: &mut SplitMix64, vertices: usize, triangles: usize) -> MeshSource {
        let positions = (0..vertices)
            .map(|_| {
                let mut p = [0.0_f32; 3];
                for coordinate in &mut p {
                    *coordinate = ((rng.next() >> 40) as u32) as f32 / 16_777_216.0 * Self::SIDE;
                }
                p
            })
            .collect();
        let count = vertices as u64;
        let triangles = (0..triangles)
            .map(|_| {
                let a = rng.next() % count;
                let b = (a + 1 + rng.next() % (count - 1)) % count;
                let mut c = (a + 1 + rng.next() % (count - 1)) % count;
                if c == b {
                    c = (c + 1) % count;
                    if c == a {
                        c = (c + 1) % count;
                    }
                }
                [a as u32, b as u32, c as u32]
            })
            .collect();
        MeshSource { positions, triangles }
    }

    pub fn encode(&self) -> Vec<u8> {
        let mut bytes = Vec::with_capacity(Self::HEADER_BYTES + 12 * (self.positions.len() + self.triangles.len()));
        bytes.extend_from_slice(Self::MAGIC);
        bytes.extend_from_slice(&(self.positions.len() as u32).to_le_bytes());
        bytes.extend_from_slice(&(self.triangles.len() as u32).to_le_bytes());
        for position in &self.positions {
            for coordinate in position {
                bytes.extend_from_slice(&coordinate.to_le_bytes());
            }
        }
        for triangle in &self.triangles {
            for index in triangle {
                bytes.extend_from_slice(&index.to_le_bytes());
            }
        }
        bytes
    }

    pub fn decode(bytes: &[u8]) -> Result<MeshSource, String> {
        if bytes.len() < Self::HEADER_BYTES {
            return Err(format!("mesh source: header needs {} bytes, {} given", Self::HEADER_BYTES, bytes.len()));
        }
        if &bytes[..4] != Self::MAGIC {
            return Err(format!("mesh source: bad magic {:?}", &bytes[..4]));
        }
        let vertices = u32::from_le_bytes(bytes[4..8].try_into().expect("four bytes")) as usize;
        let triangles = u32::from_le_bytes(bytes[8..12].try_into().expect("four bytes")) as usize;
        let expected = Self::HEADER_BYTES + 12 * (vertices + triangles);
        if bytes.len() < expected {
            return Err(format!("mesh source: {} byte(s) short of {expected}", expected - bytes.len()));
        }
        if bytes.len() > expected {
            return Err(format!("mesh source: {} trailing byte(s)", bytes.len() - expected));
        }
        let mut cursor = Self::HEADER_BYTES;
        let mut positions = Vec::with_capacity(vertices);
        for _ in 0..vertices {
            let mut p = [0.0_f32; 3];
            for coordinate in &mut p {
                *coordinate = f32::from_le_bytes(bytes[cursor..cursor + 4].try_into().expect("four bytes"));
                if !coordinate.is_finite() {
                    return Err(format!("mesh source: coordinate {coordinate} is not finite"));
                }
                cursor += 4;
            }
            positions.push(p);
        }
        let mut triangle_list = Vec::with_capacity(triangles);
        for _ in 0..triangles {
            let mut t = [0_u32; 3];
            for index in &mut t {
                *index = u32::from_le_bytes(bytes[cursor..cursor + 4].try_into().expect("four bytes"));
                if *index as usize >= vertices {
                    return Err(format!("mesh source: index {index} out of {vertices} vertices"));
                }
                cursor += 4;
            }
            triangle_list.push(t);
        }
        Ok(MeshSource { positions, triangles: triangle_list })
    }
}

/// SplitMix64: a small deterministic generator, the same bits on every platform.
pub struct SplitMix64(u64);

impl SplitMix64 {
    pub fn new(seed: u64) -> SplitMix64 {
        SplitMix64(seed)
    }

    pub fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_small_set_has_the_announced_counts_and_dependencies() {
        let set = SourceSet::generate(&SetSize::SMALL, 7).unwrap();
        assert_eq!(set.count(Kind::Plain), 20);
        assert_eq!(set.count(Kind::Geometric), 20);
        assert_eq!(set.count(Kind::Control), 5);
        assert_eq!(set.assets().len(), 45);
        for asset in set.assets() {
            assert!(asset.path.is_file(), "{}", asset.path.display());
            match asset.kind {
                Kind::Plain => assert!(asset.dependencies.is_empty()),
                Kind::Geometric | Kind::Control => {
                    assert_eq!(asset.dependencies.len(), 1);
                    assert_eq!(set.assets()[asset.dependencies[0]].kind, Kind::Plain);
                }
            }
        }
        // The first transform table has exactly `dependents_per_transform` geometric dependents.
        let first_plain = set.index_of("plain-000").unwrap();
        assert_eq!(set.dependents_of(first_plain).len(), SetSize::SMALL.dependents_per_transform);
        assert!(set.dependents_of(first_plain).iter().all(|&i| set.assets()[i].kind == Kind::Geometric));
        // The last plain tables depend on nothing and nothing depends on them.
        let last_plain = set.index_of("plain-019").unwrap();
        assert!(set.dependents_of(last_plain).is_empty());
        assert!(set.source_bytes() > 0);
    }

    #[test]
    fn the_same_seed_gives_the_same_bytes_and_another_seed_does_not() {
        let a = SourceSet::generate(&SetSize::SMALL, 1).unwrap();
        let b = SourceSet::generate(&SetSize::SMALL, 1).unwrap();
        let c = SourceSet::generate(&SetSize::SMALL, 2).unwrap();
        for (x, y, z) in a.assets().iter().zip(b.assets()).zip(c.assets()).map(|((x, y), z)| (x, y, z)) {
            assert_eq!(x.id, y.id);
            assert_eq!(std::fs::read(&x.path).unwrap(), std::fs::read(&y.path).unwrap(), "{}", x.id);
            assert_ne!(std::fs::read(&x.path).unwrap(), std::fs::read(&z.path).unwrap(), "{}", x.id);
        }
    }

    #[test]
    fn the_directory_is_removed_when_the_set_is_dropped() {
        let set = SourceSet::generate(&SetSize::SMALL, 3).unwrap();
        let directory = set.directory().to_path_buf();
        assert!(directory.is_dir());
        drop(set);
        assert!(!directory.exists());
    }

    #[test]
    fn a_source_can_be_altered_by_one_byte_and_restored() {
        let set = SourceSet::generate(&SetSize::SMALL, 4).unwrap();
        let index = set.index_of("geometric-003").unwrap();
        let before = std::fs::read(&set.assets()[index].path).unwrap();
        let restore = set.alter_one_byte(index).unwrap();
        let altered = std::fs::read(&set.assets()[index].path).unwrap();
        assert_eq!(before.len(), altered.len());
        assert_eq!(before.iter().zip(&altered).filter(|(x, y)| x != y).count(), 1);
        restore.restore().unwrap();
        assert_eq!(std::fs::read(&set.assets()[index].path).unwrap(), before);
    }

    #[test]
    fn the_generated_mesh_is_well_formed() {
        let set = SourceSet::generate(&SetSize::SMALL, 5).unwrap();
        let index = set.index_of("control-000").unwrap();
        let bytes = std::fs::read(&set.assets()[index].path).unwrap();
        let mesh = MeshSource::decode(&bytes).unwrap();
        assert_eq!(mesh.positions.len(), SetSize::SMALL.vertices_per_mesh);
        assert_eq!(mesh.triangles.len(), SetSize::SMALL.triangles_per_mesh);
        assert!(mesh.triangles.iter().all(|t| t.iter().all(|&i| (i as usize) < mesh.positions.len())));
        assert!(mesh.positions.iter().all(|p| p.iter().all(|c| c.is_finite())));
        assert_eq!(MeshSource::decode(&bytes[..bytes.len() - 1]).unwrap_err(), format!("mesh source: 1 byte(s) short of {}", bytes.len()));
        assert!(MeshSource::decode(b"LMSHxx").unwrap_err().contains("header"));
        assert!(MeshSource::decode(&[0; 12]).unwrap_err().contains("magic"));
    }
}
