//! The flattened form of a prefab, loaded as an asset.

use std::collections::BTreeSet;
use std::sync::Arc;

use ludenea_macros::Schema;
use serde::{Deserialize, Serialize};

use crate::commands::ignore_dead;
use crate::{Asset, CommandKind, CommandRecord, Component, Entity, Handle, World, decode_exact};

/// A prefab with its nested instances expanded and their overrides applied, in prefab
/// order. Every `Entity` value inside a component names the position of its target in
/// `entities` (generation 0); a reference to an entity the prefab does not have names
/// position `u32::MAX`.
#[derive(Schema, Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Prefab {
    pub entities: Vec<PrefabEntity>,
}

impl Prefab {
    /// The bytes of a prefab as snapshots and edit logs of version 2 hold it, its entities
    /// followed by the names of their component types that hold an `Entity` (which
    /// registration now computes), in the current layout.
    pub fn upgrade_version_2(bytes: &[u8]) -> Result<Vec<u8>, String> {
        let (entities, _): (Vec<PrefabEntity>, BTreeSet<String>) = decode_exact(bytes)?;
        Ok(postcard::to_allocvec(&Prefab { entities }).expect("a prefab always encodes"))
    }
}

impl Asset for Prefab {
    const NAME: &'static str = "ludenea::Prefab";

    /// Refuses a prefab this world cannot spawn: an entity address listed twice, or a
    /// component that is not registered or whose bytes do not decode exactly.
    fn validate(&self, world: &World) -> Result<(), String> {
        let mut addresses = BTreeSet::new();
        for entity in &self.entities {
            let address = &entity.address;
            if !addresses.insert(address) {
                return Err(format!(
                    "entity {address:?} is listed twice; give each entity its own address"
                ));
            }
            for (name, bytes) in &entity.components {
                let codec = world.component_codec(name).ok_or_else(|| {
                    format!(
                        "entity {address:?}: component `{name}` is not registered; register it, \
                         or load the prefab into the game that made it"
                    )
                })?;
                (codec.decode)(bytes).map_err(|reason| {
                    format!("entity {address:?}: component `{name}` does not decode: {reason}")
                })?;
            }
        }
        Ok(())
    }
}

/// One entity of a flattened prefab: its address inside the prefab (local identifiers,
/// deeper for nested instances) and its components' canonical bytes, by stable name.
#[derive(Schema, Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PrefabEntity {
    pub address: Vec<u32>,
    pub components: Vec<(String, Vec<u8>)>,
}

/// Changes to the entities of a prefab spawned by `Commands::spawn_prefab`, applied in
/// call order right after the spawn. `entity` is a top-level local identifier of the
/// prefab.
#[derive(Default)]
pub struct PrefabOverrides {
    edits: Vec<(u32, Edit)>,
}

type Edit = Box<dyn FnOnce(&mut World, Entity) + Send>;

impl PrefabOverrides {
    /// Edits the component `C` of `entity`.
    ///
    /// # Panics
    /// When applied, if `entity` has no `C`.
    pub fn set<C: Component>(
        &mut self,
        entity: u32,
        edit: impl FnOnce(&mut C) + Send + 'static,
    ) -> &mut Self {
        self.edits.push((
            entity,
            Box::new(move |world, spawned| {
                let component = world
                    .get_mut::<C>(spawned)
                    .unwrap_or_else(|| panic!("prefab entity {spawned} has no {}", C::NAME));
                edit(component);
            }),
        ));
        self
    }

    /// Inserts or replaces the component `C` of `entity`.
    ///
    /// # Panics
    /// When applied, if `C` is not registered.
    pub fn insert<C: Component>(&mut self, entity: u32, value: C) -> &mut Self {
        self.edits.push((
            entity,
            Box::new(move |world, spawned| {
                ignore_dead(world.insert(spawned, value));
            }),
        ));
        self
    }
}

/// Spawns the entities of `prefab` in prefab order, remapping its `Entity` values from
/// positions to the new entities, then applies `overrides`.
pub(crate) fn spawn(
    world: &mut World,
    prefab: Handle<Prefab>,
    overrides: impl FnOnce(&mut PrefabOverrides),
    report: &mut dyn FnMut(CommandRecord),
) {
    let id = prefab.id;
    let flat = world
        .assets
        .get(&id)
        .and_then(|asset| Arc::clone(&asset.value).downcast::<Prefab>().ok())
        .unwrap_or_else(|| panic!("prefab {id} is not loaded"));
    let spawned: Vec<Entity> = flat.entities.iter().map(|_| world.spawn()).collect();
    let map = |entity: Entity| {
        usize::try_from(entity.index())
            .ok()
            .and_then(|position| spawned.get(position))
            .copied()
            // A reference to an entity outside the prefab stays dangling.
            .unwrap_or(entity)
    };
    for (entity, &target) in flat.entities.iter().zip(&spawned) {
        report(CommandRecord {
            kind: CommandKind::Spawn,
            entity: Some(target),
            component: None,
        });
        for (name, bytes) in &entity.components {
            let (component, codec, mentions_entity) = world
                .components
                .get_key_value(name.as_str())
                .map(|(&component, entry)| (component, entry.codec, entry.mentions_entity))
                .expect("`Prefab::validate` checked that every component is registered");
            let value = if mentions_entity {
                // `Prefab::validate` decoded these bytes with the prefab's positions in
                // them; the type can still refuse the spawned entities, as a `try_from` can.
                (codec.remap_entities)(bytes, &map)
                    .and_then(|remapped| (codec.decode)(&remapped))
                    .unwrap_or_else(|error| {
                        panic!(
                            "prefab {id}: `{name}` refuses the entities spawned for it: \
                             {error}; its type must accept every entity"
                        )
                    })
            } else {
                (codec.decode)(bytes).expect("`Prefab::validate` decoded these bytes")
            };
            world
                .insert_boxed(target, name, value)
                .expect("spawned this command");
            report(CommandRecord {
                kind: CommandKind::Insert,
                entity: Some(target),
                component: Some(component),
            });
        }
    }
    let mut edits = PrefabOverrides::default();
    overrides(&mut edits);
    for (local, edit) in edits.edits {
        let position = flat
            .entities
            .iter()
            .position(|entity| entity.address == [local])
            .unwrap_or_else(|| panic!("prefab {id} has no entity with local identifier {local}"));
        edit(world, spawned[position]);
    }
}
