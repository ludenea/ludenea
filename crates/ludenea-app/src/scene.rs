//! RON scene files: the initial resources and entities of a simulation.

use std::any::Any;
use std::fmt;

use ron::value::RawValue;
use serde::de::{MapAccess, Visitor};
use serde::{Deserialize, Deserializer};

use crate::Simulation;

const SCENE_VERSION: u32 = 1;

#[derive(Debug, thiserror::Error)]
pub enum SceneError {
    #[error("cannot parse the scene: {0}; check it against the scene format")]
    Parse(String),
    #[error("scene version {0} is not supported (expected 1); convert the scene to version 1")]
    Version(u32),
    #[error(
        "entity {entity} has component `{name}`, which this app does not register; register it or remove it from the scene"
    )]
    UnknownComponent { entity: usize, name: String },
    #[error(
        "scene has resource `{0}`, which this app does not register; register it or remove it from the scene"
    )]
    UnknownResource(String),
    #[error("{} `{name}` is invalid: {reason}", subject(*.entity))]
    InvalidValue {
        entity: Option<usize>,
        name: String,
        reason: String,
    },
}

fn subject(entity: Option<usize>) -> String {
    match entity {
        Some(index) => format!("component of entity {index}"),
        None => "resource".to_string(),
    }
}

/// The file format; the RON name `Scene` is checked against this struct name.
#[derive(Deserialize)]
struct Scene {
    version: u32,
    resources: NamedValues,
    entities: Vec<NamedValues>,
}

/// A RON map kept in file order so duplicated keys can be reported.
struct NamedValues(Vec<(String, Box<RawValue>)>);

impl<'de> Deserialize<'de> for NamedValues {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct Pairs;

        impl<'de> Visitor<'de> for Pairs {
            type Value = NamedValues;

            fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
                formatter.write_str("a map of names to values")
            }

            fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<NamedValues, A::Error> {
                let mut pairs = Vec::new();
                while let Some(pair) = map.next_entry()? {
                    pairs.push(pair);
                }
                Ok(NamedValues(pairs))
            }
        }

        deserializer.deserialize_map(Pairs)
    }
}

type Decoded = Vec<(String, Box<dyn Any + Send>)>;

impl Simulation {
    /// Replaces the world with the initial one from `App::build`, then spawns the scene's
    /// entities in file order (entity `i` of the file gets index `i`) and overrides the
    /// listed resources. On error the simulation is unchanged.
    pub fn load_scene(&mut self, ron: &str) -> Result<(), SceneError> {
        let file: Scene =
            ron::from_str(ron).map_err(|error| SceneError::Parse(error.to_string()))?;
        if file.version != SCENE_VERSION {
            return Err(SceneError::Version(file.version));
        }
        let resources = self.decode_resources(file.resources)?;
        let entities = file
            .entities
            .into_iter()
            .enumerate()
            .map(|(index, components)| self.decode_components(index, components))
            .collect::<Result<Vec<_>, _>>()?;

        let initial = self.initial.clone();
        self.world
            .restore(&initial)
            .expect("the initial snapshot restores into its own world");
        for components in entities {
            let entity = self.world.spawn();
            for (name, value) in components {
                self.world
                    .insert_boxed(entity, &name, value)
                    .expect("decoded against the registry");
            }
        }
        for (name, value) in resources {
            self.world
                .insert_resource_boxed(&name, value)
                .expect("decoded against the registry");
        }
        Ok(())
    }

    /// Writes the alive entities in ascending index order and every registered resource.
    /// Generations and the free list are not kept, so entities are renumbered `0..n`:
    /// use it on freshly built worlds, such as scene generators, not to save a running game.
    pub fn save_scene(&self) -> String {
        let snapshot = self.world.snapshot();
        let resources = snapshot
            .resources
            .iter()
            .map(|(name, bytes)| {
                let codec = self.world.resource_codec(name).expect("snapshotted name");
                let value = (codec.decode)(bytes).expect("snapshot bytes decode");
                format!("{name:?}: {}", (codec.encode_ron)(&*value))
            })
            .collect::<Vec<_>>()
            .join(", ");

        let alive: Vec<_> = self.world.entities().collect();
        let entities = alive
            .iter()
            .map(|&entity| {
                let components = snapshot
                    .components
                    .iter()
                    .filter_map(|(name, values)| {
                        let (_, bytes) = values.iter().find(|(owner, _)| *owner == entity)?;
                        let codec = self.world.component_codec(name).expect("snapshotted name");
                        let value = (codec.decode)(bytes).expect("snapshot bytes decode");
                        Some(format!("{name:?}: {}", (codec.encode_ron)(&*value)))
                    })
                    .collect::<Vec<_>>()
                    .join(", ");
                format!("    {{{components}}},\n")
            })
            .collect::<String>();
        format!(
            "Scene(\n    version: {SCENE_VERSION},\n    resources: {{{resources}}},\n    entities: [\n{entities}    ],\n)\n"
        )
    }

    fn decode_resources(&self, values: NamedValues) -> Result<Decoded, SceneError> {
        let mut decoded: Decoded = Vec::new();
        for (name, raw) in values.0 {
            let codec = self
                .world
                .resource_codec(&name)
                .ok_or_else(|| SceneError::UnknownResource(name.clone()))?;
            if decoded.iter().any(|(seen, _)| *seen == name) {
                return Err(invalid(None, name, "listed twice".to_string()));
            }
            let value = (codec.decode_ron)(raw.get_ron())
                .map_err(|reason| invalid(None, name.clone(), reason))?;
            decoded.push((name, value));
        }
        Ok(decoded)
    }

    fn decode_components(&self, entity: usize, values: NamedValues) -> Result<Decoded, SceneError> {
        let mut decoded: Decoded = Vec::new();
        for (name, raw) in values.0 {
            let codec =
                self.world
                    .component_codec(&name)
                    .ok_or_else(|| SceneError::UnknownComponent {
                        entity,
                        name: name.clone(),
                    })?;
            if decoded.iter().any(|(seen, _)| *seen == name) {
                return Err(invalid(Some(entity), name, "listed twice".to_string()));
            }
            let value = (codec.decode_ron)(raw.get_ron())
                .map_err(|reason| invalid(Some(entity), name.clone(), reason))?;
            decoded.push((name, value));
        }
        Ok(decoded)
    }
}

fn invalid(entity: Option<usize>, name: String, reason: String) -> SceneError {
    SceneError::InvalidValue {
        entity,
        name,
        reason,
    }
}
