//! Full copies of the simulation state, restorable into a world with the same registry.

use std::any::Any;
use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

use crate::commands::CommandQueue;
use crate::component::decode_exact;
use crate::hash::ENTITIES_LEAF;
use crate::{Asset, AssetId, EntitiesState, Entity, Prefab, PrefabEntity, World};

pub const SNAPSHOT_VERSION: u32 = 3;

/// Postcard bytes of one component type, per entity.
pub type ComponentValues = Vec<(Entity, Vec<u8>)>;

/// Entity allocator state plus every registered resource and component, encoded with
/// postcard and listed in name order, and the loaded assets. Runtime resources are never
/// included.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Snapshot {
    pub version: u32,
    pub entities: EntitiesState,
    /// Every registered resource, even one unchanged since registration.
    pub resources: Vec<(String, Vec<u8>)>,
    /// Every registered component, even one no entity has.
    pub components: Vec<(String, ComponentValues)>,
    /// Loaded assets in identifier order: type name and canonical postcard bytes.
    pub assets: Vec<(AssetId, String, Vec<u8>)>,
    /// Identifies the game build that recorded the snapshot; `0` means unknown.
    pub fingerprint: u64,
}

#[derive(Debug, thiserror::Error)]
pub enum SnapshotError {
    #[error(
        "snapshot version {found} is not supported (expected {expected}); record it again with this build"
    )]
    Version { found: u32, expected: u32 },
    #[error(
        "snapshot contains component `{0}`, which this world does not register; restore it into the app that recorded it"
    )]
    UnknownComponent(String),
    #[error(
        "snapshot contains resource `{0}`, which this world does not register; restore it into the app that recorded it"
    )]
    UnknownResource(String),
    #[error(
        "snapshot lacks component `{0}`, which this world registers; restore it into the app that recorded it"
    )]
    MissingComponent(String),
    #[error(
        "snapshot lacks resource `{0}`, which this world registers; restore it into the app that recorded it"
    )]
    MissingResource(String),
    #[error("cannot decode `{name}` from the snapshot: {reason}")]
    Decode { name: String, reason: String },
    #[error(
        "the snapshots were recorded by different game builds (schema fingerprints {left:016x} and {right:016x}); compare snapshots of one build"
    )]
    Fingerprint { left: u64, right: u64 },
}

impl Snapshot {
    pub fn encode(&self) -> Vec<u8> {
        postcard::to_allocvec(self).expect("a snapshot always encodes")
    }

    /// Reads the version first, so a snapshot of another version fails as such. Version 2
    /// is read as the current version.
    pub fn decode(bytes: &[u8]) -> Result<Snapshot, SnapshotError> {
        let (version, _) = postcard::take_from_bytes::<u32>(bytes)
            .map_err(|error| decode_error("snapshot")(error.to_string()))?;
        if version != SNAPSHOT_VERSION && version != 2 {
            return Err(SnapshotError::Version {
                found: version,
                expected: SNAPSHOT_VERSION,
            });
        }
        let mut snapshot: Snapshot = decode_exact(bytes).map_err(decode_error("snapshot"))?;
        if version == 2 {
            snapshot.upgrade_version_2()?;
        }
        Ok(snapshot)
    }

    /// Version 2 wrote each prefab followed by the names of its component types that hold
    /// an `Entity`, which registration now computes.
    fn upgrade_version_2(&mut self) -> Result<(), SnapshotError> {
        for (id, type_name, bytes) in &mut self.assets {
            if type_name == Prefab::NAME {
                let (entities, _): (Vec<PrefabEntity>, BTreeSet<String>) =
                    decode_exact(bytes).map_err(decode_error(&format!("asset {id}")))?;
                *bytes =
                    postcard::to_allocvec(&Prefab { entities }).expect("a prefab always encodes");
            }
        }
        self.version = SNAPSHOT_VERSION;
        Ok(())
    }
}

type DecodedComponents = Vec<(&'static str, Vec<(u32, Box<dyn Any + Send>)>)>;

impl World {
    pub fn snapshot(&self) -> Snapshot {
        let resources = self
            .resources
            .iter()
            .map(|(name, entry)| {
                (
                    name.to_string(),
                    (entry.codec.encode)(&*entry.value.value()),
                )
            })
            .collect();
        let components = self
            .components
            .iter()
            .map(|(name, entry)| {
                let mut values = Vec::new();
                entry.storage.for_each(&mut |index, value| {
                    values.push((self.entities.entity_at(index), (entry.codec.encode)(value)));
                });
                (name.to_string(), values)
            })
            .collect();
        Snapshot {
            version: SNAPSHOT_VERSION,
            entities: self.entities.clone(),
            resources,
            components,
            assets: self
                .assets
                .iter()
                .map(|(&id, asset)| (id, asset.type_name.to_string(), asset.bytes.clone()))
                .collect(),
            fingerprint: 0,
        }
    }

    /// Replaces the entities, components, resources, and assets with those of `snapshot`,
    /// and drops any queued command and event (left behind by a tick that panicked). On
    /// error the world is unchanged. Runtime resources are kept as they are.
    pub fn restore(&mut self, snapshot: &Snapshot) -> Result<(), SnapshotError> {
        self.check_compatible(snapshot)?;
        check_entities(&snapshot.entities)?;

        let mut resources = Vec::with_capacity(snapshot.resources.len());
        for (name, bytes) in &snapshot.resources {
            let (&key, entry) = self
                .resources
                .get_key_value(name.as_str())
                .expect("checked");
            resources.push((
                key,
                (entry.codec.decode)(bytes).map_err(decode_error(name))?,
            ));
        }
        let mut components: DecodedComponents = Vec::with_capacity(snapshot.components.len());
        for (name, values) in &snapshot.components {
            let (&key, entry) = self
                .components
                .get_key_value(name.as_str())
                .expect("checked");
            let mut decoded = Vec::with_capacity(values.len());
            for (entity, bytes) in values {
                if !snapshot.entities.is_alive(*entity) {
                    return Err(decode_error(name)(format!("entity {entity} is not alive")));
                }
                let value = (entry.codec.decode)(bytes)
                    .map_err(|reason| decode_error(name)(format!("entity {entity}: {reason}")))?;
                decoded.push((entity.index(), value));
            }
            components.push((key, decoded));
        }
        let mut assets = Vec::with_capacity(snapshot.assets.len());
        for (id, type_name, bytes) in &snapshot.assets {
            if assets.last().is_some_and(|(last, _)| last >= id) {
                return Err(decode_error(&format!("asset {id}"))(
                    "asset identifiers are not in strictly ascending order".to_string(),
                ));
            }
            let asset = self
                .decode_asset(*id, type_name, bytes)
                .map_err(|error| decode_error(&format!("asset {id}"))(error.to_string()))?;
            assets.push(asset);
        }

        self.entities = snapshot.entities.clone();
        for (name, value) in resources {
            self.resources
                .get_mut(name)
                .expect("checked")
                .value
                .set(value);
        }
        for (name, values) in components {
            let entry = self.components.get_mut(name).expect("checked");
            entry.storage.replace_all(values);
        }
        self.replace_assets(assets);
        *self.commands.get_mut() = CommandQueue::default();
        self.clear_events();
        Ok(())
    }

    /// Fails unless `snapshot` has this build's version and exactly this world's names.
    pub(crate) fn check_compatible(&self, snapshot: &Snapshot) -> Result<(), SnapshotError> {
        if snapshot.version != SNAPSHOT_VERSION {
            return Err(SnapshotError::Version {
                found: snapshot.version,
                expected: SNAPSHOT_VERSION,
            });
        }
        let names: BTreeSet<&str> = snapshot
            .components
            .iter()
            .map(|(n, _)| n.as_str())
            .collect();
        if let Some(name) = names
            .iter()
            .find(|name| !self.components.contains_key(**name))
        {
            return Err(SnapshotError::UnknownComponent(name.to_string()));
        }
        if let Some(name) = self.components.keys().find(|name| !names.contains(**name)) {
            return Err(SnapshotError::MissingComponent(name.to_string()));
        }
        let names: BTreeSet<&str> = snapshot.resources.iter().map(|(n, _)| n.as_str()).collect();
        if let Some(name) = names
            .iter()
            .find(|name| !self.resources.contains_key(**name))
        {
            return Err(SnapshotError::UnknownResource(name.to_string()));
        }
        if let Some(name) = self.resources.keys().find(|name| !names.contains(**name)) {
            return Err(SnapshotError::MissingResource(name.to_string()));
        }
        Ok(())
    }
}

/// Rejects allocator states that would make later spawns or liveness checks panic.
fn check_entities(entities: &EntitiesState) -> Result<(), SnapshotError> {
    let error = decode_error(ENTITIES_LEAF);
    if entities.generations.len() != entities.alive.len() {
        return Err(error(
            "generations and alive flags differ in length".to_string(),
        ));
    }
    let mut free = BTreeSet::new();
    for &index in &entities.free {
        if entities.alive.get(index as usize) != Some(&false) || !free.insert(index) {
            return Err(error(format!("free slot {index} is invalid")));
        }
    }
    Ok(())
}

fn decode_error(name: &str) -> impl Fn(String) -> SnapshotError + '_ {
    move |reason| SnapshotError::Decode {
        name: name.to_string(),
        reason,
    }
}
