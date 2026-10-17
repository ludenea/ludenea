//! Full copies of the simulation state, restorable into a world with the same registry.

use std::any::Any;
use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

use crate::hash::ENTITIES_LEAF;
use crate::{EntitiesState, Entity, World};

pub const SNAPSHOT_VERSION: u32 = 1;

/// Postcard bytes of one component type, per entity.
pub type ComponentValues = Vec<(Entity, Vec<u8>)>;

/// Entity allocator state plus every registered resource and component, encoded with
/// postcard and listed in name order. Runtime resources are never included.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Snapshot {
    pub version: u32,
    pub entities: EntitiesState,
    /// Every registered resource, even one unchanged since registration.
    pub resources: Vec<(String, Vec<u8>)>,
    /// Every registered component, even one no entity has.
    pub components: Vec<(String, ComponentValues)>,
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
}

impl Snapshot {
    pub fn encode(&self) -> Vec<u8> {
        postcard::to_allocvec(self).expect("a snapshot always encodes")
    }

    pub fn decode(bytes: &[u8]) -> Result<Snapshot, SnapshotError> {
        postcard::from_bytes(bytes).map_err(|error| SnapshotError::Decode {
            name: "snapshot".to_string(),
            reason: error.to_string(),
        })
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
        }
    }

    /// Replaces the entities, components, and resources with those of `snapshot`. On error
    /// the world is unchanged. Runtime resources are kept as they are.
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
                let value = (entry.codec.decode)(bytes).map_err(decode_error(name))?;
                decoded.push((entity.index(), value));
            }
            components.push((key, decoded));
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
