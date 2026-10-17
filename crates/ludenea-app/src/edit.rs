//! Editor edits as data: a canonical log of whole-value changes and their application to a
//! simulation.

use std::any::Any;

use ludenea_ecs::{Codec, Entity};
use serde::{Deserialize, Serialize};

use crate::{SimError, Simulation};

const TIME: &str = "ludenea::Time";

/// One change to the world. Values are postcard bytes of the concrete type
/// (`Codec::encode`), so the log is canonical.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Edit {
    Spawn {
        components: Vec<(String, Vec<u8>)>,
    },
    Despawn {
        entity: Entity,
    },
    /// Inserts the component, or replaces it when the entity already has one.
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

/// An edit made while the simulation was at `tick`: part of the state at `tick` from then
/// on, so it takes effect when tick `tick + 1` runs.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TimedEdit {
    pub tick: u64,
    pub edit: Edit,
}

pub const EDIT_LOG_VERSION: u32 = 1;

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

    pub fn decode(bytes: &[u8]) -> Result<EditLog, SimError> {
        let (log, rest): (EditLog, _) = postcard::take_from_bytes(bytes)
            .map_err(|error| SimError::EditLogDecode(error.to_string()))?;
        if !rest.is_empty() {
            return Err(SimError::EditLogDecode(format!(
                "{} unexpected trailing bytes",
                rest.len()
            )));
        }
        if log.version != EDIT_LOG_VERSION {
            return Err(SimError::EditLogVersion {
                found: log.version,
                expected: EDIT_LOG_VERSION,
            });
        }
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

#[derive(Debug, thiserror::Error)]
pub enum EditError {
    #[error("entity {0} is not alive at this tick; pick an entity that exists")]
    NoSuchEntity(Entity),
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
                self.world
                    .despawn(*entity)
                    .map_err(|_| EditError::NoSuchEntity(*entity))?;
                Ok(None)
            }
            Edit::Insert {
                entity,
                component,
                value,
            } => {
                self.ensure_alive(*entity)?;
                let value = self.decode_component(component, value)?;
                self.world
                    .insert_boxed(*entity, component, value)
                    .expect("validated against the registry");
                Ok(None)
            }
            Edit::Remove { entity, component } => {
                self.ensure_alive(*entity)?;
                self.component_codec(component)?;
                self.world
                    .remove_by_name(*entity, component)
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
        }
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
