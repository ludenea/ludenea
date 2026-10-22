//! In-memory saves of a world: what a snapshot holds, cloned instead of encoded, so a
//! rollback can save every tick and load a past state quickly.

use std::any::TypeId;
use std::collections::BTreeMap;

use crate::assets::{AssetType, LoadedAsset};
use crate::commands::CommandQueue;
use crate::world::{ResourceCell, Storage};
use crate::{AssetId, EntitiesState, World};

/// A copy of a world's entity allocator, component storages, resources, and loaded assets
/// (shared, not copied). Saving into the same `SavedWorld` again reuses its allocations:
/// each storage's buffer and each resource's cell, and inside them whatever the type's
/// `Clone::clone_from` reuses (`#[derive(Clone)]` clones afresh).
/// Runtime resources, queued commands, and events are never saved.
#[derive(Default)]
pub struct SavedWorld {
    /// Whether a world was saved into this one.
    saved: bool,
    entities: EntitiesState,
    /// Every storage of the world saved, by stable name; `None` only while a save fills it.
    pub(crate) components: BTreeMap<&'static str, Option<Box<dyn Storage + Send>>>,
    resources: BTreeMap<&'static str, Option<Box<dyn ResourceCell + Send>>>,
    pub(crate) assets: BTreeMap<AssetId, LoadedAsset>,
    /// The asset types of the world saved, by stable name, so a load can refuse a world
    /// whose types would not read `assets`.
    asset_types: BTreeMap<&'static str, TypeId>,
    /// The `World::assets_version` of `assets`.
    assets_version: u64,
}

/// Why `World::load` refused a saved world; each names the stable type name involved: a
/// component, a resource, or an asset type.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SavedError {
    #[error("nothing was saved into this saved world; save a world into it before loading it")]
    Empty,
    #[error(
        "the saved world lacks `{name}`, which this world registers; load it into the world it was saved from"
    )]
    MissingStorage { name: String },
    #[error(
        "the saved world holds `{name}`, which this world does not register; load it into the world it was saved from"
    )]
    UnexpectedStorage { name: String },
    #[error(
        "`{name}` is another type in the saved world than in this world; load it into the world it was saved from"
    )]
    TypeMismatch { name: String },
}

impl World {
    /// Copies the simulation state into `into`, reusing the allocations it already holds.
    /// The asset map is copied only when it changed since the last save into `into`.
    pub fn save_into(&self, into: &mut SavedWorld) {
        into.saved = true;
        clone_entities(&self.entities, &mut into.entities);
        into.components
            .retain(|name, _| self.components.contains_key(name));
        for (&name, entry) in &self.components {
            entry
                .storage
                .clone_into(into.components.entry(name).or_default());
        }
        into.resources
            .retain(|name, _| self.resources.contains_key(name));
        for (&name, entry) in &self.resources {
            entry
                .value
                .clone_into(into.resources.entry(name).or_default());
        }
        if !same_asset_types(&self.asset_types, &into.asset_types) {
            into.asset_types = self
                .asset_types
                .iter()
                .map(|(&name, entry)| (name, entry.type_id))
                .collect();
        }
        if into.assets_version != self.assets_version {
            into.assets.clone_from(&self.assets);
            into.assets_version = self.assets_version;
        }
    }

    /// Replaces the entities, components, resources, and assets with those of `saved`, and
    /// drops any queued command and event, as `World::restore` does. Every storage and
    /// resource is checked before anything changes: on error the world is unchanged.
    /// Runtime resources are kept as they are.
    pub fn load(&mut self, saved: &SavedWorld) -> Result<(), SavedError> {
        if !saved.saved {
            return Err(SavedError::Empty);
        }
        check_matching(&self.components, &saved.components, |entry, saved| {
            entry.storage.check_saved(saved)
        })?;
        check_matching(&self.resources, &saved.resources, |entry, saved| {
            entry.value.check_saved(saved)
        })?;
        check_asset_types(&self.asset_types, &saved.asset_types)?;

        clone_entities(&saved.entities, &mut self.entities);
        // The checks above proved both maps hold the same names, so they zip in order.
        for (entry, saved) in self
            .components
            .values_mut()
            .zip(saved.components.values().flatten())
        {
            entry.storage.load_from(&**saved);
        }
        for (entry, saved) in self
            .resources
            .values_mut()
            .zip(saved.resources.values().flatten())
        {
            entry.value.load_from(&**saved);
        }
        if self.assets_version != saved.assets_version {
            self.assets.clone_from(&saved.assets);
            self.assets_version = saved.assets_version;
        }
        *self.commands.get_mut() = CommandQueue::default();
        self.clear_events();
        Ok(())
    }
}

/// Copies `from` into `into`, reusing its buffers. Destructured so that a new field of
/// `EntitiesState` fails to compile here instead of being left out of saves.
fn clone_entities(from: &EntitiesState, into: &mut EntitiesState) {
    let EntitiesState {
        generations,
        alive,
        free,
    } = from;
    into.generations.clone_from(generations);
    into.alive.clone_from(alive);
    into.free.clone_from(free);
}

/// Whether `saved` records exactly the asset types of `world`.
fn same_asset_types(
    world: &BTreeMap<&'static str, AssetType>,
    saved: &BTreeMap<&'static str, TypeId>,
) -> bool {
    world.len() == saved.len()
        && world
            .iter()
            .zip(saved)
            .all(|((name, entry), (saved_name, type_id))| {
                name == saved_name && entry.type_id == *type_id
            })
}

/// Fails unless `saved` records exactly the asset types of `world`, so every saved asset
/// is of a type this world reads as the saving world did.
fn check_asset_types(
    world: &BTreeMap<&'static str, AssetType>,
    saved: &BTreeMap<&'static str, TypeId>,
) -> Result<(), SavedError> {
    for (&name, entry) in world {
        match saved.get(name) {
            None => {
                return Err(SavedError::MissingStorage {
                    name: name.to_string(),
                });
            }
            Some(type_id) if *type_id != entry.type_id => {
                return Err(SavedError::TypeMismatch {
                    name: name.to_string(),
                });
            }
            Some(_) => {}
        }
    }
    match saved.keys().find(|name| !world.contains_key(*name)) {
        Some(name) => Err(SavedError::UnexpectedStorage {
            name: name.to_string(),
        }),
        None => Ok(()),
    }
}

/// Fails unless `saved` holds exactly the names of `world`, each passing `check`.
fn check_matching<E, S: ?Sized>(
    world: &BTreeMap<&'static str, E>,
    saved: &BTreeMap<&'static str, Option<Box<S>>>,
    check: impl Fn(&E, &S) -> Result<(), SavedError>,
) -> Result<(), SavedError> {
    for (&name, entry) in world {
        match saved.get(name) {
            Some(Some(saved)) => check(entry, saved)?,
            _ => {
                return Err(SavedError::MissingStorage {
                    name: name.to_string(),
                });
            }
        }
    }
    match saved.keys().find(|name| !world.contains_key(*name)) {
        Some(name) => Err(SavedError::UnexpectedStorage {
            name: name.to_string(),
        }),
        None => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use std::any::Any;
    use std::cell::RefCell;

    use serde::{Deserialize, Serialize};

    use super::SavedWorld;
    use crate::world::ComponentStorage;
    use crate::{Asset, AssetId, Component, Resource, World};

    #[derive(Component, Serialize, Deserialize, Clone)]
    struct Health(u32);

    /// `#[derive(Clone)]` never overrides `clone_from`, so a derived resource gets a fresh
    /// clone of its fields; this one forwards to `Vec::clone_from`, which shows that a save
    /// goes through `clone_from` and keeps the buffer.
    #[derive(Resource, Serialize, Deserialize)]
    struct History(Vec<u32>);

    impl Clone for History {
        fn clone(&self) -> Self {
            Self(self.0.clone())
        }

        fn clone_from(&mut self, source: &Self) {
            self.0.clone_from(&source.0);
        }
    }

    #[derive(Asset, Serialize, Deserialize)]
    struct Stats(u32);

    fn buffer(saved: &SavedWorld) -> *const Option<Health> {
        let storage: &dyn Any = saved.components[Health::NAME]
            .as_ref()
            .expect("saved")
            .as_any();
        let storage = storage
            .downcast_ref::<ComponentStorage<Health>>()
            .expect("a Health storage");
        storage.0.borrow().as_ptr()
    }

    fn history_buffer(saved: &SavedWorld) -> *const u32 {
        let cell: &dyn Any = saved.resources[History::NAME]
            .as_ref()
            .expect("saved")
            .as_any();
        let cell = cell
            .downcast_ref::<RefCell<History>>()
            .expect("a History cell");
        cell.borrow().0.as_ptr()
    }

    #[test]
    fn save_into_reuses_allocations() {
        let mut world = World::new();
        world.register_component::<Health>().unwrap();
        world.register_asset::<Stats>().unwrap();
        world.register_resource(History(vec![1, 2, 3])).unwrap();
        for hp in 0..8 {
            let entity = world.spawn();
            world.insert(entity, Health(hp)).unwrap();
        }
        let stats = postcard::to_allocvec(&Stats(1)).unwrap();
        world.insert_asset(AssetId(1), Stats::NAME, &stats).unwrap();
        let mut saved = SavedWorld::default();
        world.save_into(&mut saved);
        let first = buffer(&saved);
        let first_history = history_buffer(&saved);
        assert_eq!(saved.assets.len(), 1);

        // The same shape with other values, and the asset map unchanged: emptied here, it
        // stays empty because the save does not copy it again.
        let first_entity = world.entities().next().unwrap();
        world.get_mut::<Health>(first_entity).unwrap().0 = 99;
        world.resource_mut::<History>().unwrap().0[0] = 7;
        saved.assets.clear();
        world.save_into(&mut saved);
        assert_eq!(buffer(&saved), first, "the storage buffer was reallocated");
        assert_eq!(
            history_buffer(&saved),
            first_history,
            "the resource buffer was reallocated"
        );
        assert!(
            saved.assets.is_empty(),
            "an unchanged asset map was copied again"
        );

        // A changed asset map is copied.
        world.insert_asset(AssetId(2), Stats::NAME, &stats).unwrap();
        world.save_into(&mut saved);
        assert_eq!(saved.assets.len(), 2);
    }
}
