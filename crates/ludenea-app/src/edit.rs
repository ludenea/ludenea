//! Editor edits as data: a canonical log of whole-value changes and their application to a
//! simulation.

use std::any::Any;
use std::collections::{BTreeMap, BTreeSet};

use ludenea_ecs::{AssetId, Codec, Entity};
use serde::{Deserialize, Serialize};

use crate::assets::{AssetSource, visit_handles};
use crate::scene::{LoadAddress, LoadedValue, Reference};
use crate::{SceneError, SimError, Simulation};

const TIME: &str = "ludenea::Time";

/// The entity an edit targets: by load address when the scene load spawned it, so the edit
/// follows it when the scene or its prefabs change, otherwise by entity identifier.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum EntityRef {
    Loaded(LoadAddress),
    Runtime(Entity),
}

/// One change to the world. Values are postcard bytes of the concrete type
/// (`Codec::encode`), so the log is canonical.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Edit {
    Spawn {
        components: Vec<(String, Vec<u8>)>,
    },
    Despawn {
        entity: EntityRef,
    },
    /// Inserts the component, or replaces it when the entity already has one.
    Insert {
        entity: EntityRef,
        component: String,
        value: Vec<u8>,
    },
    Remove {
        entity: EntityRef,
        component: String,
    },
    SetResource {
        resource: String,
        value: Vec<u8>,
    },
    /// Replaces the value of a loaded asset.
    SetAsset {
        id: AssetId,
        value: Vec<u8>,
    },
    /// Loads (or replaces) the asset `id` of the asset type `asset`.
    LoadAsset {
        id: AssetId,
        asset: String,
        value: Vec<u8>,
    },
}

/// An edit made while the simulation was at `tick`: part of the state at `tick` from then
/// on, so it takes effect when tick `tick + 1` runs.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TimedEdit {
    pub tick: u64,
    pub edit: Edit,
}

pub const EDIT_LOG_VERSION: u32 = 2;

/// The edits of a session, ordered by tick, then record order.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct EditLog {
    pub version: u32,
    pub edits: Vec<TimedEdit>,
}

impl Default for EditLog {
    fn default() -> Self {
        Self::new()
    }
}

impl EditLog {
    pub fn new() -> Self {
        Self {
            version: EDIT_LOG_VERSION,
            edits: Vec::new(),
        }
    }

    pub fn encode(&self) -> Vec<u8> {
        postcard::to_allocvec(self).expect("an edit log always encodes")
    }

    /// Decodes the current version, or version 1 with every target as `EntityRef::Runtime`.
    pub fn decode(bytes: &[u8]) -> Result<EditLog, SimError> {
        let (version, _): (u32, _) = postcard::take_from_bytes(bytes)
            .map_err(|error| SimError::EditLogDecode(error.to_string()))?;
        let log = match version {
            EDIT_LOG_VERSION => decode_exact::<EditLog>(bytes)?,
            1 => decode_exact::<version_1::EditLog>(bytes)?.into(),
            found => {
                return Err(SimError::EditLogVersion {
                    found,
                    expected: EDIT_LOG_VERSION,
                });
            }
        };
        if log.edits.windows(2).any(|pair| pair[0].tick > pair[1].tick) {
            return Err(SimError::EditLogDecode(
                "edits are not ordered by tick".to_string(),
            ));
        }
        Ok(log)
    }

    /// The edits made at `tick`, in record order.
    pub fn at(&self, tick: u64) -> &[TimedEdit] {
        let start = self.edits.partition_point(|edit| edit.tick < tick);
        let end = self.edits.partition_point(|edit| edit.tick <= tick);
        &self.edits[start..end]
    }

    pub fn last_tick(&self) -> Option<u64> {
        self.edits.last().map(|edit| edit.tick)
    }
}

fn decode_exact<T: for<'de> Deserialize<'de>>(bytes: &[u8]) -> Result<T, SimError> {
    let (value, rest) = postcard::take_from_bytes(bytes)
        .map_err(|error| SimError::EditLogDecode(error.to_string()))?;
    if !rest.is_empty() {
        return Err(SimError::EditLogDecode(format!(
            "{} unexpected trailing bytes",
            rest.len()
        )));
    }
    Ok(value)
}

/// Edit log version 1, which named every target by entity identifier; also read inside
/// other formats that embed its edits.
pub mod version_1 {
    use ludenea_ecs::Entity;
    use serde::Deserialize;

    use super::EntityRef;

    #[derive(Deserialize)]
    pub enum Edit {
        Spawn {
            components: Vec<(String, Vec<u8>)>,
        },
        Despawn {
            entity: Entity,
        },
        Insert {
            entity: Entity,
            component: String,
            value: Vec<u8>,
        },
        Remove {
            entity: Entity,
            component: String,
        },
        SetResource {
            resource: String,
            value: Vec<u8>,
        },
    }

    #[derive(Deserialize)]
    pub struct TimedEdit {
        tick: u64,
        edit: Edit,
    }

    #[derive(Deserialize)]
    pub struct EditLog {
        #[allow(dead_code)]
        version: u32,
        edits: Vec<TimedEdit>,
    }

    impl From<Edit> for super::Edit {
        fn from(edit: Edit) -> Self {
            let target = EntityRef::Runtime;
            match edit {
                Edit::Spawn { components } => super::Edit::Spawn { components },
                Edit::Despawn { entity } => super::Edit::Despawn {
                    entity: target(entity),
                },
                Edit::Insert {
                    entity,
                    component,
                    value,
                } => super::Edit::Insert {
                    entity: target(entity),
                    component,
                    value,
                },
                Edit::Remove { entity, component } => super::Edit::Remove {
                    entity: target(entity),
                    component,
                },
                Edit::SetResource { resource, value } => {
                    super::Edit::SetResource { resource, value }
                }
            }
        }
    }

    impl From<TimedEdit> for super::TimedEdit {
        fn from(timed: TimedEdit) -> Self {
            super::TimedEdit {
                tick: timed.tick,
                edit: timed.edit.into(),
            }
        }
    }

    impl From<EditLog> for super::EditLog {
        fn from(log: EditLog) -> Self {
            super::EditLog {
                version: super::EDIT_LOG_VERSION,
                edits: log.edits.into_iter().map(Into::into).collect(),
            }
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum EditError {
    #[error("entity {0} is not alive at this tick; pick an entity that exists")]
    NoSuchEntity(Entity),
    #[error("the scene has no entity at load address {0:?}; pick an entity that exists")]
    NoSuchAddress(LoadAddress),
    #[error("asset {0} is not available; add the asset or pick one that exists")]
    MissingAsset(AssetId),
    #[error("component `{0}` is not registered; use a component the app registers")]
    UnknownComponent(String),
    #[error("resource `{0}` is not registered; use a resource the app registers")]
    UnknownResource(String),
    #[error("`{0}` is read-only in the editor; move the timeline instead")]
    ReadOnly(String),
    #[error("`{name}` rejects this value: {reason}; fix the value and try again")]
    InvalidValue { name: String, reason: String },
}

/// Whether a name refers to a component or to a resource.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ValueKind {
    Component,
    Resource,
}

type Decoded = Box<dyn Any + Send>;

impl Simulation {
    /// Applies `edit`. Everything is validated before anything changes, so on error the
    /// world is untouched. Returns the entity a `Spawn` created.
    pub fn apply_edit(&mut self, edit: &Edit) -> Result<Option<Entity>, EditError> {
        match edit {
            Edit::Spawn { components } => {
                for (index, (name, _)) in components.iter().enumerate() {
                    if components[..index].iter().any(|(other, _)| other == name) {
                        return Err(invalid(name, "the component appears twice".to_string()));
                    }
                }
                let decoded = components
                    .iter()
                    .map(|(name, bytes)| Ok((name, self.decode_component(name, bytes)?)))
                    .collect::<Result<Vec<_>, EditError>>()?;
                let entity = self.world.spawn();
                for (name, value) in decoded {
                    self.world
                        .insert_boxed(entity, name, value)
                        .expect("validated against the registry");
                }
                Ok(Some(entity))
            }
            Edit::Despawn { entity } => {
                let entity = self.resolve_ref(entity)?;
                self.world
                    .despawn(entity)
                    .map_err(|_| EditError::NoSuchEntity(entity))?;
                Ok(None)
            }
            Edit::Insert {
                entity,
                component,
                value,
            } => {
                let entity = self.resolve_ref(entity)?;
                self.ensure_alive(entity)?;
                let value = self.decode_component(component, value)?;
                self.world
                    .insert_boxed(entity, component, value)
                    .expect("validated against the registry");
                Ok(None)
            }
            Edit::Remove { entity, component } => {
                let entity = self.resolve_ref(entity)?;
                self.ensure_alive(entity)?;
                self.component_codec(component)?;
                self.world
                    .remove_by_name(entity, component)
                    .expect("validated against the registry");
                Ok(None)
            }
            Edit::SetResource { resource, value } => {
                if resource == TIME {
                    return Err(EditError::ReadOnly(resource.clone()));
                }
                let codec = self.resource_codec(resource)?;
                let value = decode(&codec, resource, value)?;
                self.world
                    .insert_resource_boxed(resource, value)
                    .expect("validated against the registry");
                Ok(None)
            }
            Edit::SetAsset { id, value } => {
                let asset = self.asset_type_of(*id)?;
                self.load_asset(*id, &asset, value)
            }
            Edit::LoadAsset { id, asset, value } => self.load_asset(*id, asset, value),
        }
    }

    /// How an edit made now names `entity`: by load address when the last scene load
    /// spawned it (same generation), otherwise by identifier.
    pub fn entity_ref(&self, entity: Entity) -> EntityRef {
        match self.load_addresses.get(&entity) {
            Some(address) => EntityRef::Loaded(address.clone()),
            None => EntityRef::Runtime(entity),
        }
    }

    /// The entity `target` names in this simulation.
    pub fn resolve_ref(&self, target: &EntityRef) -> Result<Entity, EditError> {
        match target {
            EntityRef::Loaded(address) => self
                .entity_at_address(address)
                .ok_or_else(|| EditError::NoSuchAddress(address.clone())),
            EntityRef::Runtime(entity) => Ok(*entity),
        }
    }

    /// The `LoadAsset` edits `edit` needs first: one per asset that a handle in its values
    /// reaches (recursively) and that is not loaded, read from `sources`. In value order,
    /// then identifier order.
    pub fn assets_introduced_by(
        &self,
        edit: &Edit,
        sources: &dyn AssetSource,
    ) -> Result<Vec<Edit>, EditError> {
        let values: Vec<(String, Codec, &[u8])> = match edit {
            Edit::Spawn { components } => components
                .iter()
                .map(|(name, bytes)| Ok((name.clone(), self.component_codec(name)?, &bytes[..])))
                .collect::<Result<_, EditError>>()?,
            Edit::Insert {
                component, value, ..
            } => vec![(component.clone(), self.component_codec(component)?, value)],
            Edit::SetResource { resource, value } => {
                vec![(resource.clone(), self.resource_codec(resource)?, value)]
            }
            Edit::SetAsset { id, value } => {
                let asset = self.asset_type_of(*id)?;
                vec![(asset.clone(), self.asset_codec(&asset)?, value)]
            }
            Edit::LoadAsset { asset, value, .. } => {
                vec![(asset.clone(), self.asset_codec(asset)?, value)]
            }
            Edit::Despawn { .. } | Edit::Remove { .. } => Vec::new(),
        };
        let mut known: BTreeSet<AssetId> = self.world.asset_ids().into_iter().collect();
        if let Edit::LoadAsset { id, .. } = edit {
            known.insert(*id);
        }
        let mut loads = Vec::new();
        for (name, codec, bytes) in values {
            let name = name.as_str();
            let value = decode(&codec, name, bytes)?;
            let ron: ron::Value = ron::from_str(&(codec.encode_ron)(&*value))
                .map_err(|error| invalid(name, error.to_string()))?;
            let mut references = Vec::new();
            visit_handles(
                &(codec.schema)(),
                &ron,
                &mut String::new(),
                &mut |id, asset, field| {
                    references.push(Reference {
                        id,
                        asset: asset.to_string(),
                        referenced_by: format!("{name}{field}"),
                    });
                },
            );
            let resolved = self
                .resolve_assets(references, sources, &known)
                .map_err(|error| match error {
                    SceneError::MissingAsset { id, .. } => EditError::MissingAsset(id),
                    other => invalid(name, other.to_string()),
                })?;
            for (id, (asset, value)) in resolved {
                let value = match value {
                    LoadedValue::Bytes(bytes) => bytes,
                    LoadedValue::Ron(ron) => {
                        let codec = self.asset_codec(&asset)?;
                        let decoded = (codec.decode_ron)(&ron)
                            .map_err(|reason| invalid(&asset, format!("asset {id}: {reason}")))?;
                        (codec.encode)(&*decoded)
                    }
                };
                known.insert(id);
                loads.push(Edit::LoadAsset { id, asset, value });
            }
        }
        Ok(loads)
    }

    /// Converts a JSON value into the canonical bytes `Edit` carries, running the type's
    /// own validation.
    pub fn encode_json_value(
        &self,
        kind: ValueKind,
        name: &str,
        value: serde_json::Value,
    ) -> Result<Vec<u8>, EditError> {
        let codec = match kind {
            ValueKind::Component => self.component_codec(name)?,
            ValueKind::Resource => self.resource_codec(name)?,
        };
        let value = (codec.from_json)(value).map_err(|reason| invalid(name, reason))?;
        Ok((codec.encode)(&*value))
    }

    /// Checks that every value `edit` carries decodes with this app's types, without
    /// touching the world. `asset_types` names the type of each asset a `SetAsset` may
    /// target; a `LoadAsset` adds its own.
    pub(crate) fn check_edit_values(
        &self,
        edit: &Edit,
        asset_types: &mut BTreeMap<AssetId, String>,
    ) -> Result<(), EditError> {
        match edit {
            Edit::Spawn { components } => {
                for (name, bytes) in components {
                    self.decode_component(name, bytes)?;
                }
            }
            Edit::Despawn { .. } => {}
            Edit::Insert {
                component, value, ..
            } => {
                self.decode_component(component, value)?;
            }
            Edit::Remove { component, .. } => {
                self.component_codec(component)?;
            }
            Edit::SetResource { resource, value } => {
                decode(&self.resource_codec(resource)?, resource, value)?;
            }
            Edit::SetAsset { id, value } => {
                let asset = asset_types.get(id).ok_or(EditError::MissingAsset(*id))?;
                decode(&self.asset_codec(asset)?, asset, value)?;
            }
            Edit::LoadAsset { id, asset, value } => {
                decode(&self.asset_codec(asset)?, asset, value)?;
                asset_types.insert(*id, asset.clone());
            }
        }
        Ok(())
    }

    fn load_asset(
        &mut self,
        id: AssetId,
        asset: &str,
        value: &[u8],
    ) -> Result<Option<Entity>, EditError> {
        self.world
            .insert_asset(id, asset, value)
            .map_err(|error| match error {
                ludenea_ecs::EcsError::InvalidAsset { reason, .. } => invalid(asset, reason),
                other => invalid(asset, other.to_string()),
            })?;
        Ok(None)
    }

    fn asset_type_of(&self, id: AssetId) -> Result<String, EditError> {
        self.world
            .asset_bytes(id)
            .map(|(asset, _)| asset.to_string())
            .ok_or(EditError::MissingAsset(id))
    }

    fn asset_codec(&self, asset: &str) -> Result<Codec, EditError> {
        self.world
            .asset_codec(asset)
            .ok_or_else(|| invalid(asset, "the asset type is not registered".to_string()))
    }

    fn ensure_alive(&self, entity: Entity) -> Result<(), EditError> {
        if self.world.is_alive(entity) {
            Ok(())
        } else {
            Err(EditError::NoSuchEntity(entity))
        }
    }

    fn component_codec(&self, name: &str) -> Result<Codec, EditError> {
        self.world
            .component_codec(name)
            .ok_or_else(|| EditError::UnknownComponent(name.to_string()))
    }

    fn resource_codec(&self, name: &str) -> Result<Codec, EditError> {
        self.world
            .resource_codec(name)
            .ok_or_else(|| EditError::UnknownResource(name.to_string()))
    }

    fn decode_component(&self, name: &str, bytes: &[u8]) -> Result<Decoded, EditError> {
        decode(&self.component_codec(name)?, name, bytes)
    }
}

fn decode(codec: &Codec, name: &str, bytes: &[u8]) -> Result<Decoded, EditError> {
    (codec.decode)(bytes).map_err(|reason| invalid(name, reason))
}

fn invalid(name: &str, reason: String) -> EditError {
    EditError::InvalidValue {
        name: name.to_string(),
        reason,
    }
}
