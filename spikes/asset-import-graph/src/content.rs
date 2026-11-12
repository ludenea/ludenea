//! The synthetic content root of the import graph spike, and the edits the
//! scenarios make to it. Throwaway, never product code.
//!
//! Every file is a function of the size and of a seed, so that the same
//! content exists on every machine:
//!
//! - `tables/`: tables with a sidecar. The first half are palettes of the
//!   sheets; the second half are used by the scenes.
//! - `sheets/`: sheets with a sidecar, each with sprites and clips. A sprite
//!   names its palette; one sprite in six names it as late, so that its
//!   import discovers the dependency.
//! - `scenes/`: scenes with their identifier in their first line. Every
//!   prototype names the position component or one other component type that
//!   depends on the scene; some prototypes use a table, a clip of a sheet or,
//!   in every second scene, the scene before it; one scene in four uses a
//!   table as late. One more scene, when asked, holds many prototypes.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use reference_workload::SplitMix64;

use crate::formats::{AssetId, SourceKind, derived_id, drawn_id, id_text, sidecar_text};

#[derive(Clone, Copy, Debug)]
pub struct SetSize {
    pub tables: usize,
    pub records: usize,
    pub sheets: usize,
    pub sprites: usize,
    /// The side of a sprite, in pixels of four bytes.
    pub side: usize,
    pub clips: usize,
    pub scenes: usize,
    pub prototypes: usize,
    /// The prototypes of the one large scene; none where zero.
    pub large_scene: usize,
}

impl SetSize {
    pub const MEASURED: SetSize =
        SetSize { tables: 128, records: 12_000, sheets: 64, sprites: 8, side: 256, clips: 2, scenes: 32, prototypes: 400, large_scene: 10_000 };
    pub const SMALL: SetSize = SetSize { tables: 12, records: 300, sheets: 6, sprites: 4, side: 24, clips: 2, scenes: 8, prototypes: 40, large_scene: 0 };

    pub fn assets(&self) -> usize {
        self.tables + self.sheets * (self.sprites + self.clips) + self.scenes + usize::from(self.large_scene > 0)
    }

    pub fn sources(&self) -> usize {
        self.tables + self.sheets + self.scenes + usize::from(self.large_scene > 0)
    }
}

/// The component type a scene names beside the position: by the scene's index.
pub const COMPONENTS: [&str; 3] = ["velocity", "health", "sprite"];

pub struct Content {
    pub root: PathBuf,
    pub size: SetSize,
    pub tables: Vec<AssetId>,
    pub sheets: Vec<AssetId>,
    pub scenes: Vec<AssetId>,
    pub large_scene: Option<AssetId>,
}

fn sprite_key(sprite: usize) -> String {
    format!("sprite-{sprite:02}")
}

fn clip_key(clip: usize) -> String {
    format!("clip-{clip}")
}

impl Content {
    pub fn table_path(&self, table: usize) -> PathBuf {
        self.root.join(format!("tables/table-{table:03}.tbl"))
    }

    pub fn sheet_path(&self, sheet: usize) -> PathBuf {
        self.root.join(format!("sheets/sheet-{sheet:03}.sheet"))
    }

    pub fn scene_path(&self, scene: usize) -> PathBuf {
        self.root.join(format!("scenes/scene-{scene:03}.scene"))
    }

    pub fn sprite(&self, sheet: usize, sprite: usize) -> AssetId {
        derived_id(self.sheets[sheet], &sprite_key(sprite))
    }

    pub fn clip(&self, sheet: usize, clip: usize) -> AssetId {
        derived_id(self.sheets[sheet], &clip_key(clip))
    }

    /// The palette of a sprite: one of the first half of the tables.
    pub fn palette_of(&self, sheet: usize, sprite: usize) -> usize {
        (sheet * self.size.sprites + sprite) % (self.size.tables / 2)
    }

    /// Whether the import of a sprite discovers its palette.
    pub fn sprite_is_late(&self, sheet: usize, sprite: usize) -> bool {
        (sheet * self.size.sprites + sprite).is_multiple_of(6)
    }

    /// The table a prototype of a scene uses, if it uses one: one of the second half.
    fn table_used(&self, scene: usize, prototype: usize) -> Option<usize> {
        let half = self.size.tables / 2;
        (prototype % 8 == 3).then(|| half + (scene * 3 + prototype / 8) % half)
    }

    /// The table a scene uses as late, if it does: one of the second half.
    pub fn late_table_of(&self, scene: usize) -> Option<usize> {
        let half = self.size.tables / 2;
        (scene % 4 == 3).then(|| half + (scene * 5 + 1) % half)
    }

    /// The scenes whose import reads a table, declared or late.
    pub fn scenes_using_table(&self, table: usize) -> Vec<usize> {
        let uses = |scene: usize| (0..self.size.prototypes).any(|prototype| self.table_used(scene, prototype) == Some(table)) || self.late_table_of(scene) == Some(table);
        (0..self.size.scenes).filter(|scene| uses(*scene)).collect()
    }

    /// The sprites whose palette a table is, as (sheet, sprite).
    pub fn sprites_using_table(&self, table: usize) -> Vec<(usize, usize)> {
        let all = (0..self.size.sheets).flat_map(|sheet| (0..self.size.sprites).map(move |sprite| (sheet, sprite)));
        all.filter(|(sheet, sprite)| self.palette_of(*sheet, *sprite) == table).collect()
    }

    /// The component type a scene names beside the position.
    pub fn component_of(scene: usize) -> &'static str {
        COMPONENTS[scene % COMPONENTS.len()]
    }

    /// How many imports discover a dependency the describe function did not report.
    pub fn discovering_imports(&self) -> usize {
        let sprites = (0..self.size.sheets * self.size.sprites).filter(|index| index.is_multiple_of(6)).count();
        sprites + (0..self.size.scenes).filter(|scene| scene % 4 == 3).count()
    }

    fn table_text(&self, table: usize, revision: u64) -> String {
        let mut stream = SplitMix64::new(0x7461_626c_6500 ^ ((table as u64) << 20) ^ revision.wrapping_mul(0x9e37_79b9));
        let records = self.size.records;
        // The keys are a permutation of the record indices: the import sorts them.
        let step = (1..).map(|k| 7_919 + 2 * k).find(|step| gcd(*step, records) == 1).expect("a step coprime with the record count");
        let mut text = String::with_capacity(records * 24);
        for record in 0..records {
            let key = (record * step) % records;
            let value = stream.next() % 100_000;
            let name: String = (0..6).map(|letter| (b'a' + ((stream.state >> (letter * 5)) % 26) as u8) as char).collect();
            text.push_str(&format!("{key};{value};{name}\n"));
        }
        text
    }

    fn sheet_bytes(&self, sheet: usize) -> Vec<u8> {
        let size = &self.size;
        let pixels = size.side * size.side * 4;
        let mut text = String::from("sheet");
        for sprite in 0..size.sprites {
            let palette = id_text(self.tables[self.palette_of(sheet, sprite)]);
            let mode = if self.sprite_is_late(sheet, sprite) { "late" } else { "declared" };
            text.push_str(&format!("\nsprite {} {} {} {} {palette} {mode}", sprite_key(sprite), size.side, size.side, sprite * pixels));
        }
        for clip in 0..size.clips {
            let frames: Vec<String> = (0..3).map(|frame| sprite_key((clip + frame) % size.sprites)).collect();
            text.push_str(&format!("\nclip {} {} {}", clip_key(clip), 12 + clip, frames.join(" ")));
        }
        text.push_str("\npixels\n");
        let mut bytes = text.into_bytes();
        let mut stream = SplitMix64::new(0x7368_6565_7400 ^ ((sheet as u64) << 20));
        bytes.reserve(size.sprites * pixels);
        for _ in 0..size.sprites * pixels / 8 {
            bytes.extend_from_slice(&stream.next().to_le_bytes());
        }
        bytes
    }

    fn scene_text(&self, id: AssetId, scene: usize, prototypes: usize, plain: bool) -> String {
        let mut text = format!("scene {}\n", id_text(id));
        for prototype in 0..prototypes {
            let (component, fields) = if prototype % 2 == 0 {
                ("position", format!("x={} y={}", prototype * 3, 7 - prototype as i64))
            } else {
                match Content::component_of(scene) {
                    "velocity" => ("velocity", format!("dx={}", prototype % 9)),
                    "health" => ("health", format!("points={}", 10 + prototype % 90)),
                    _ => ("sprite", format!("frame={}", prototype % 4)),
                }
            };
            text.push_str(&format!("prototype entity-{prototype:05} {component} {fields}"));
            if let Some(table) = self.table_used(scene, prototype) {
                text.push_str(&format!(" uses {}", id_text(self.tables[table])));
            } else if !plain && prototype % 50 == 10 {
                text.push_str(&format!(" uses {}", id_text(self.clip(scene % self.size.sheets, 0))));
            } else if !plain && prototype == 1 && scene % 2 == 1 {
                text.push_str(&format!(" uses {}", id_text(self.scenes[scene - 1])));
            } else if !plain && prototype + 2 == prototypes && let Some(table) = self.late_table_of(scene) {
                text.push_str(&format!(" late-uses {}", id_text(self.tables[table])));
            }
            text.push('\n');
        }
        text
    }

    fn write_with_sidecar(path: &Path, bytes: &[u8], id: AssetId, kind: SourceKind) -> io::Result<()> {
        fs::write(path, bytes)?;
        fs::write(sidecar_of(path), sidecar_text(id, kind))
    }

    /// Writes the content root from the size and the seed.
    pub fn generate(root: &Path, size: SetSize, seed: u64) -> io::Result<Content> {
        let mut stream = SplitMix64::new(seed);
        let mut draw = |count: usize| -> Vec<AssetId> { (0..count).map(|_| drawn_id(stream.next(), stream.next())).collect() };
        let (tables, sheets, scenes) = (draw(size.tables), draw(size.sheets), draw(size.scenes));
        let large_scene = (size.large_scene > 0).then(|| draw(1)[0]);
        let content = Content { root: root.to_path_buf(), size, tables, sheets, scenes, large_scene };
        for directory in ["tables", "sheets", "scenes"] {
            fs::create_dir_all(root.join(directory))?;
        }
        for table in 0..size.tables {
            Content::write_with_sidecar(&content.table_path(table), content.table_text(table, 0).as_bytes(), content.tables[table], SourceKind::Table)?;
        }
        for sheet in 0..size.sheets {
            Content::write_with_sidecar(&content.sheet_path(sheet), &content.sheet_bytes(sheet), content.sheets[sheet], SourceKind::Sheet)?;
        }
        for scene in 0..size.scenes {
            fs::write(content.scene_path(scene), content.scene_text(content.scenes[scene], scene, size.prototypes, false))?;
        }
        if let Some(id) = content.large_scene {
            fs::write(content.large_scene_path(), content.scene_text(id, 0, size.large_scene, true))?;
        }
        Ok(content)
    }

    pub fn large_scene_path(&self) -> PathBuf {
        self.root.join("scenes/large.scene")
    }

    // --- the edits ---------------------------------------------------------

    /// Adds a comment to a table: its source changes, its canonical part does not.
    pub fn add_comment(&self, table: usize) -> io::Result<()> {
        let path = self.table_path(table);
        let mut text = fs::read_to_string(&path)?;
        text.push_str("# a note that changes no record\n");
        fs::write(path, text)
    }

    /// Rewrites a table with other values under the same keys: its canonical
    /// part changes, its record count does not.
    pub fn change_values(&self, table: usize, revision: u64) -> io::Result<()> {
        fs::write(self.table_path(table), self.table_text(table, revision))
    }

    /// Rewrites a table in place with one digit changed, at the same size,
    /// then sets its modification time back to what it was: the change a
    /// journal of sizes and times cannot see.
    pub fn change_keeping_size_and_time(&self, table: usize) -> io::Result<()> {
        let path = self.table_path(table);
        let modified = fs::metadata(&path)?.modified()?;
        let mut bytes = fs::read(&path)?;
        let at = bytes.iter().position(|byte| *byte == b';').expect("a record") + 1;
        bytes[at] = if bytes[at] == b'1' { b'2' } else { b'1' };
        fs::write(&path, bytes)?;
        set_modified(&path, modified)
    }

    /// Copies a sheet with its sidecar under a new identifier, as a tool
    /// that duplicates an asset does. Returns the copy's identifier.
    pub fn copy_sheet(&self, sheet: usize, name: &str, id: AssetId) -> io::Result<PathBuf> {
        let path = self.root.join(format!("sheets/{name}.sheet"));
        fs::copy(self.sheet_path(sheet), &path)?;
        fs::write(sidecar_of(&path), sidecar_text(id, SourceKind::Sheet))?;
        Ok(path)
    }

    /// Adds a table and a scene that uses it: new content for a later package.
    pub fn add_table_and_scene(&self, table: AssetId, scene: AssetId) -> io::Result<()> {
        let path = self.root.join("tables/added.tbl");
        Content::write_with_sidecar(&path, self.table_text(self.size.tables, 0).as_bytes(), table, SourceKind::Table)?;
        let text = format!("scene {}\nprototype added position x=1 y=2 uses {}\n", id_text(scene), id_text(table));
        fs::write(self.root.join("scenes/added.scene"), text)
    }
}

pub fn sidecar_of(path: &Path) -> PathBuf {
    let mut name = path.as_os_str().to_owned();
    name.push(".meta");
    PathBuf::from(name)
}

pub fn set_modified(path: &Path, modified: SystemTime) -> io::Result<()> {
    fs::File::options().write(true).open(path)?.set_modified(modified)
}

fn gcd(a: usize, b: usize) -> usize {
    if b == 0 { a } else { gcd(b, a % b) }
}

/// Two scenes that each use the other as late: a cycle of requests.
pub fn write_cycle(root: &Path, one: AssetId, other: AssetId) -> io::Result<()> {
    fs::create_dir_all(root.join("scenes"))?;
    let scene = |id: AssetId, uses: AssetId| format!("scene {}\nprototype entity position x=1 late-uses {}\n", id_text(id), id_text(uses));
    fs::write(root.join("scenes/one.scene"), scene(one, other))?;
    fs::write(root.join("scenes/other.scene"), scene(other, one))
}
