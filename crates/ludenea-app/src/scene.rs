//! RON scene files: the initial resources and entities of a simulation, and the assets they
//! reach through handles.

use std::any::Any;
use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use ludenea_ecs::schema::{TypeSchema, VariantFields};
use ludenea_ecs::{Asset, AssetId, Codec, EcsError, Entity, Prefab, PrefabEntity};
use ron::value::RawValue;
use serde::de::{IgnoredAny, MapAccess, Visitor};
use serde::{Deserialize, Deserializer};

use crate::Simulation;
use crate::assets::{
    AssetSource, AssetTexts, NoAssets, asset_file, parse_asset_file_in_place, visit_handles,
};
use crate::prefab::{
    Instance, InstanceInfo, LoadedInstance, Resolved, address_text, expand_prefab, is_prefab_file,
    resolve,
};

const SCENE_VERSION: u32 = 2;

/// Where an entity came from in the last loaded scene: a path of local identifiers, one
/// element for a scene entity.
pub type LoadAddress = Vec<u32>;

/// What a successful scene load reports.
#[derive(Debug, Default)]
pub struct SceneLoad {
    pub warnings: Vec<String>,
}

#[derive(Debug, thiserror::Error)]
pub enum SceneError {
    #[error("cannot parse the scene: {0}; check it against the scene format")]
    Parse(String),
    #[error("scene version {0} is not supported (expected 1 or 2); convert the scene to version 2")]
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
    #[error(
        "{referenced_by} references asset {id}, which is not available; add it or fix the reference"
    )]
    MissingAsset { referenced_by: String, id: AssetId },
    #[error(
        "asset {id}{} is a `{found}`, but {referenced_by} expects a `{expected}`; fix the reference or the asset type",
        located(path)
    )]
    WrongAssetType {
        id: AssetId,
        path: Option<String>,
        referenced_by: String,
        expected: String,
        found: String,
    },
    #[error("asset {id}{} is invalid: {reason}", located(path))]
    InvalidAsset {
        id: AssetId,
        path: Option<String>,
        reason: String,
    },
    #[error("local identifier {0} is given to two entities of the scene; give each entity its own")]
    DuplicateLocal(u32),
    #[error(
        "component `{component}` of entity {entity} names local identifier {local}, which is not below the scene's `next_id`; fix the reference"
    )]
    UnknownLocal {
        entity: u32,
        component: String,
        local: u32,
    },
    #[error("prefab instances form a cycle: {}; remove one of these instances", .0.join(" -> "))]
    PrefabCycle(Vec<String>),
}

fn subject(entity: Option<usize>) -> String {
    match entity {
        Some(index) => format!("component of entity {index}"),
        None => "resource".to_string(),
    }
}

fn located(path: &Option<String>) -> String {
    path.as_ref()
        .map(|path| format!(" ({path})"))
        .unwrap_or_default()
}

/// The fields every scene version shares; the RON name `Scene` is checked against it.
#[derive(Deserialize)]
#[serde(rename = "Scene")]
struct Header {
    version: u32,
    #[serde(default)]
    next_id: u32,
    entities: Vec<IgnoredAny>,
}

/// Version 1: entity identifiers are positions in the file.
#[derive(Deserialize)]
#[serde(rename = "Scene")]
struct SceneV1 {
    resources: NamedValues,
    entities: Vec<NamedValues>,
}

#[derive(Deserialize)]
#[serde(rename = "Scene")]
struct SceneV2 {
    next_id: u32,
    resources: NamedValues,
    entities: Vec<SceneEntity>,
    #[serde(default)]
    instances: Vec<Instance>,
}

/// A resolved scene (`Resolved::to_scene_text`): entities carry their whole address.
#[derive(Deserialize)]
#[serde(rename = "Scene")]
struct ResolvedV2 {
    next_id: u32,
    resources: NamedValues,
    entities: Vec<AddressedEntity>,
}

#[derive(Deserialize)]
pub(crate) struct SceneEntity {
    pub(crate) id: u32,
    pub(crate) components: NamedValues,
}

#[derive(Deserialize)]
struct AddressedEntity {
    address: LoadAddress,
    components: NamedValues,
}

/// A RON map kept in file order so duplicated keys can be reported.
pub(crate) struct NamedValues(pub(crate) Vec<(String, Box<RawValue>)>);

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

/// A scene file of any version, or a resolved scene.
pub(crate) struct ParsedScene {
    pub(crate) next_id: u32,
    /// Whether scene entities' `Entity` values must name identifiers below `next_id`.
    pub(crate) checked: bool,
    pub(crate) resources: NamedValues,
    pub(crate) entities: Vec<(LoadAddress, NamedValues)>,
    pub(crate) instances: Vec<Instance>,
}

pub(crate) fn parse_scene(ron: &str) -> Result<ParsedScene, SceneError> {
    let parse = |error: ron::error::SpannedError| SceneError::Parse(error.to_string());
    let header: Header = ron::from_str(ron).map_err(parse)?;
    match header.version {
        1 => {
            let file: SceneV1 = ron::from_str(ron).map_err(parse)?;
            Ok(ParsedScene {
                next_id: u32::try_from(file.entities.len()).expect("fewer than u32::MAX entities"),
                checked: false,
                resources: file.resources,
                entities: (0..).map(|local| vec![local]).zip(file.entities).collect(),
                instances: Vec::new(),
            })
        }
        2 => {
            let file: SceneV2 = match ron::from_str(ron) {
                Ok(file) => file,
                Err(error) => return parse_resolved(ron).map_err(|_| parse(error)),
            };
            let mut seen = BTreeSet::new();
            let locals = file.entities.iter().map(|entity| entity.id);
            if let Some(local) = locals
                .chain(file.instances.iter().map(|instance| instance.id))
                .find(|&local| !seen.insert(local))
            {
                return Err(SceneError::DuplicateLocal(local));
            }
            Ok(ParsedScene {
                next_id: file.next_id,
                checked: true,
                resources: file.resources,
                entities: file
                    .entities
                    .into_iter()
                    .map(|entity| (vec![entity.id], entity.components))
                    .collect(),
                instances: file.instances,
            })
        }
        version => Err(SceneError::Version(version)),
    }
}

fn parse_resolved(ron: &str) -> Result<ParsedScene, SceneError> {
    let file: ResolvedV2 =
        ron::from_str(ron).map_err(|error| SceneError::Parse(error.to_string()))?;
    if file.entities.iter().any(|entity| entity.address.is_empty()) {
        return Err(SceneError::Parse(
            "an entity has an empty address; give it at least its local identifier".to_string(),
        ));
    }
    let mut seen = BTreeSet::new();
    if let Some(entity) = file.entities.iter().find(|e| !seen.insert(&e.address)) {
        return Err(SceneError::Parse(format!(
            "address {:?} is given to two entities",
            entity.address
        )));
    }
    Ok(ParsedScene {
        next_id: file.next_id,
        checked: false,
        resources: file.resources,
        entities: file
            .entities
            .into_iter()
            .map(|entity| (entity.address, entity.components))
            .collect(),
        instances: Vec::new(),
    })
}

/// The `next_id` of a scene file: the entity count for version 1, 0 when it does not parse.
fn next_id_of(ron: &str) -> u32 {
    match ron::from_str::<Header>(ron) {
        Ok(header) if header.version == 1 => {
            u32::try_from(header.entities.len()).expect("fewer than u32::MAX entities")
        }
        Ok(header) => header.next_id,
        Err(_) => 0,
    }
}

/// A decoded value, or its postcard bytes when its `Entity` values still name local
/// identifiers.
enum Pending {
    Ready(Box<dyn Any + Send>),
    Local(Vec<u8>),
}

/// A handle found in the scene or in a loaded asset.
struct Reference {
    id: AssetId,
    asset: String,
    referenced_by: String,
}

/// Decodes scene values, collecting the handles they hold and checking their local
/// identifiers.
struct Decoder {
    /// The scene's `next_id` when its own values' `Entity` values are checked against it.
    next_id: Option<u32>,
    /// Local identifiers of the scene's own entities.
    locals: BTreeSet<u32>,
    shapes: BTreeMap<String, Shape>,
    references: Vec<Reference>,
}

/// What a scene load needs to know about a type, computed once per name.
struct Shape {
    schema: TypeSchema,
    handles: bool,
    entities: bool,
}

impl Shape {
    fn of(codec: Codec) -> Self {
        let schema = (codec.schema)();
        Self {
            handles: mentions(&schema, &|schema| {
                matches!(schema, TypeSchema::Handle { .. })
            }),
            entities: mentions(&schema, &|schema| matches!(schema, TypeSchema::Entity)),
            schema,
        }
    }
}

impl Decoder {
    /// Decodes the value `name` of `owner` (an entity's load address, or `None` for a
    /// resource).
    fn decode(
        &mut self,
        codec: Codec,
        name: &str,
        text: &str,
        owner: Option<&[u32]>,
    ) -> Result<Pending, SceneError> {
        let owner_local = owner.and_then(|address| address.last().copied());
        let invalid = |reason| invalid(owner_local.map(|local| local as usize), name, reason);
        let value = (codec.decode_ron)(text).map_err(invalid)?;
        let shape = self
            .shapes
            .entry(name.to_string())
            .or_insert_with(|| Shape::of(codec));

        if shape.handles {
            let value: ron::Value =
                ron::from_str(text).map_err(|error| invalid(error.to_string()))?;
            let references = &mut self.references;
            visit_handles(
                &shape.schema,
                &value,
                &mut String::new(),
                &mut |id, asset, field| {
                    let referenced_by = match owner {
                        Some(address) => {
                            format!("entity {}, {name}{field}", address_text(address))
                        }
                        None => format!("resource {name}{field}"),
                    };
                    references.push(Reference {
                        id,
                        asset: asset.to_string(),
                        referenced_by,
                    });
                },
            );
        }

        if !shape.entities {
            return Ok(Pending::Ready(value));
        }
        let bytes = (codec.encode)(&*value);
        // Only the scene's own values are checked; elsewhere, and below `next_id`, an
        // identifier no entity has names a deleted entity.
        let scene_value = owner.is_none_or(|address| address.len() == 1);
        if let Some(next_id) = self.next_id.filter(|_| scene_value) {
            let locals = &self.locals;
            rewrite_entities(&shape.schema, &bytes, &mut |local, _| {
                (locals.contains(&local) || local < next_id).then_some((local, 0))
            })
            .map_err(|local| match owner_local {
                Some(entity) => SceneError::UnknownLocal {
                    entity,
                    component: name.to_string(),
                    local,
                },
                None => invalid(format!(
                    "names local identifier {local}, which is not below the scene's `next_id`"
                )),
            })?;
        }
        Ok(Pending::Local(bytes))
    }
}

type Decoded = Vec<(String, Pending)>;

impl Simulation {
    /// `load_scene_with` without assets.
    pub fn load_scene(&mut self, ron: &str) -> Result<(), SceneError> {
        self.load_scene_with(ron, &NoAssets).map(|_| ())
    }

    /// Replaces the world with the initial one from `App::build`, spawns the scene's
    /// entities in file order, then each prefab instance's (see `resolve`), overrides the
    /// listed resources, and replaces the loaded assets with every asset reachable through
    /// handles and instances from the scene, recursively (handles may form cycles). Every
    /// prefab reached is loaded flattened. On error the simulation is unchanged.
    pub fn load_scene_with(
        &mut self,
        ron: &str,
        assets: &dyn AssetSource,
    ) -> Result<SceneLoad, SceneError> {
        let resolved = resolve(ron, assets)?;
        self.load_resolved(resolved, assets)
    }

    fn load_resolved(
        &mut self,
        resolved: Resolved,
        assets: &dyn AssetSource,
    ) -> Result<SceneLoad, SceneError> {
        let mut decoder = Decoder {
            next_id: resolved.checked.then_some(resolved.next_id),
            locals: resolved
                .entities
                .iter()
                .filter_map(|entity| match entity.address[..] {
                    [local] => Some(local),
                    _ => None,
                })
                .collect(),
            shapes: BTreeMap::new(),
            references: Vec::new(),
        };
        let resources = self.decode_resources(&mut decoder, resolved.resources)?;
        let entities = resolved
            .entities
            .into_iter()
            .map(|entity| {
                let components =
                    self.decode_components(&mut decoder, &entity.address, entity.components)?;
                Ok((entity.address, components))
            })
            .collect::<Result<Vec<_>, SceneError>>()?;
        let mut references = decoder.references;
        references.extend(resolved.instances.iter().map(|(address, id)| Reference {
            id: *id,
            asset: Prefab::NAME.to_string(),
            referenced_by: format!("instance {}", address_text(address)),
        }));
        let loaded = self.resolve_assets(references, assets)?;

        let backup = (!loaded.is_empty()).then(|| self.world.snapshot());
        let initial = self.initial.clone();
        self.world
            .restore(&initial)
            .expect("the initial snapshot restores into its own world");
        for (&id, (type_name, value)) in &loaded {
            let inserted = match value {
                LoadedValue::Ron(ron) => self.world.insert_asset_ron(id, type_name, ron),
                LoadedValue::Bytes(bytes) => self.world.insert_asset(id, type_name, bytes),
            };
            if let Err(error) = inserted {
                if let Some(backup) = backup {
                    self.world
                        .restore(&backup)
                        .expect("a snapshot restores into the world it was taken from");
                }
                let reason = match error {
                    EcsError::InvalidAsset { reason, .. } => reason,
                    other => other.to_string(),
                };
                return Err(SceneError::InvalidAsset {
                    id,
                    path: assets.path(id).map(str::to_string),
                    reason,
                });
            }
        }

        let spawned: Vec<(LoadAddress, Entity)> = entities
            .iter()
            .map(|(address, _)| (address.clone(), self.world.spawn()))
            .collect();
        let by_address: BTreeMap<LoadAddress, Entity> = spawned.iter().cloned().collect();
        for (address, components) in entities {
            let entity = by_address[&address];
            let scope = &address[..address.len() - 1];
            for (name, value) in components {
                let value = self.ready(&decoder.shapes, &name, value, &by_address, scope, false);
                self.world
                    .insert_boxed(entity, &name, value)
                    .expect("decoded against the registry");
            }
        }
        for (name, value) in resources {
            let value = self.ready(&decoder.shapes, &name, value, &by_address, &[], true);
            self.world
                .insert_resource_boxed(&name, value)
                .expect("decoded against the registry");
        }
        self.instances = resolved
            .instances
            .into_iter()
            .map(|(address, prefab)| {
                let entities: Vec<Entity> = spawned
                    .iter()
                    .filter(|(at, _)| at.len() > address.len() && at.starts_with(&address))
                    .map(|&(_, entity)| entity)
                    .collect();
                let components = entities
                    .iter()
                    .map(|&entity| self.world.entity_components_ron(entity).unwrap_or_default())
                    .collect();
                LoadedInstance {
                    info: InstanceInfo {
                        address,
                        prefab,
                        entities,
                    },
                    components,
                }
            })
            .collect();
        self.load_addresses = spawned
            .into_iter()
            .map(|(address, entity)| (entity, address))
            .collect();
        Ok(SceneLoad {
            warnings: resolved.warnings,
        })
    }

    /// Writes the alive entities in ascending index order with local identifiers `0..n`,
    /// `next_id: n`, and every registered resource. Generations and the free list are not
    /// kept: use it on freshly built worlds, such as scene generators, not to save a
    /// running game. An `Entity` value naming an entity that is not saved (a stale
    /// reference) gets an identifier of its own that no entity has, so it loads as a
    /// reference to a dead entity.
    pub fn save_scene(&self) -> String {
        let alive: Vec<Entity> = self.world.entities().collect();
        let next_id = u32::try_from(alive.len()).expect("fewer than u32::MAX entities");
        self.write_scene(alive.into_iter().zip(0..).collect(), next_id, &[])
    }

    /// Like `save_scene`, but entities that still have a load address keep their local
    /// identifier from `previous` (the scene last loaded), and new entities get fresh
    /// identifiers from `previous`'s `next_id`, so an identifier is never reused. An
    /// instance of `previous` whose entities are all alive and unchanged keeps its entry
    /// byte for byte; the entities of other instances are written as scene entities.
    pub fn save_scene_keeping(&self, previous: &str) -> String {
        let kept = |entity: &Entity| match self.load_addresses.get(entity).map(Vec::as_slice) {
            Some(&[local]) => Some(local),
            _ => None,
        };
        let entries = instance_entries(previous);
        let instances: Vec<&LoadedInstance> = self
            .instances
            .iter()
            .filter(|instance| match instance.info.address[..] {
                [id] => entries.iter().any(|(entry, _)| *entry == id) && self.unchanged(instance),
                _ => false,
            })
            .collect();
        let in_instances: BTreeSet<Entity> = instances
            .iter()
            .flat_map(|instance| instance.info.entities.iter().copied())
            .collect();
        let entries: Vec<String> = entries
            .into_iter()
            .filter(|(id, _)| instances.iter().any(|i| i.info.address == [*id]))
            .map(|(_, entry)| entry)
            .collect();
        let alive: Vec<Entity> = self
            .world
            .entities()
            .filter(|entity| !in_instances.contains(entity))
            .collect();
        let mut next_id = alive
            .iter()
            .filter_map(kept)
            .map(|local| local + 1)
            .fold(next_id_of(previous), u32::max);
        let locals = alive
            .into_iter()
            .map(|entity| {
                let local = kept(&entity).unwrap_or_else(|| {
                    next_id += 1;
                    next_id - 1
                });
                (entity, local)
            })
            .collect();
        self.write_scene(locals, next_id, &entries)
    }

    /// The prefab instances of the last loaded scene, nested ones included, parents first.
    pub fn instances(&self) -> Vec<InstanceInfo> {
        self.instances
            .iter()
            .map(|instance| instance.info.clone())
            .collect()
    }

    /// Whether every entity of `instance` is alive with the components it was loaded with.
    fn unchanged(&self, instance: &LoadedInstance) -> bool {
        instance
            .info
            .entities
            .iter()
            .zip(&instance.components)
            .all(|(&entity, components)| {
                self.world.entity_components_ron(entity).as_ref() == Some(components)
            })
    }

    /// The loaded assets in file form.
    pub fn assets_ron(&self) -> AssetTexts {
        self.world
            .asset_ids()
            .into_iter()
            .map(|id| {
                let (type_name, _) = self.world.asset_bytes(id).expect("a loaded identifier");
                let value = self.world.asset_ron(id).expect("a loaded identifier");
                (id, asset_file(type_name, &value))
            })
            .collect()
    }

    /// Writes `locals` (alive entities in index order with their local identifiers), then
    /// the instance entries `instances` as they are.
    fn write_scene(
        &self,
        locals: Vec<(Entity, u32)>,
        next_id: u32,
        instances: &[String],
    ) -> String {
        let by_entity: BTreeMap<(u32, u32), u32> = locals
            .iter()
            .map(|(entity, local)| ((entity.index(), entity.generation()), *local))
            .collect();
        // Stale references, each with an identifier from `next_id` on, in write order.
        let stale: RefCell<BTreeMap<(u32, u32), u32>> = RefCell::default();
        let ron = |codec: Codec, bytes: &[u8]| {
            let schema = (codec.schema)();
            let bytes = if mentions(&schema, &|schema| matches!(schema, TypeSchema::Entity)) {
                rewrite_entities(&schema, bytes, &mut |index, generation| {
                    let local = by_entity.get(&(index, generation)).copied();
                    let local = local.unwrap_or_else(|| {
                        let mut stale = stale.borrow_mut();
                        let fresh = next_id + u32::try_from(stale.len()).expect("few references");
                        *stale.entry((index, generation)).or_insert(fresh)
                    });
                    Some((local, 0))
                })
                .expect("every entity is mapped")
            } else {
                bytes.to_vec()
            };
            let value = (codec.decode)(&bytes).expect("snapshot bytes decode");
            (codec.encode_ron)(&*value)
        };

        let snapshot = self.world.snapshot();
        let resources = snapshot
            .resources
            .iter()
            .map(|(name, bytes)| {
                let codec = self.world.resource_codec(name).expect("snapshotted name");
                format!("{name:?}: {}", ron(codec, bytes))
            })
            .collect::<Vec<_>>()
            .join(", ");
        let entities = locals
            .iter()
            .map(|&(entity, local)| {
                let components = snapshot
                    .components
                    .iter()
                    .filter_map(|(name, values)| {
                        let (_, bytes) = values.iter().find(|(owner, _)| *owner == entity)?;
                        let codec = self.world.component_codec(name).expect("snapshotted name");
                        Some(format!("{name:?}: {}", ron(codec, bytes)))
                    })
                    .collect::<Vec<_>>()
                    .join(", ");
                format!("    (id: {local}, components: {{{components}}}),\n")
            })
            .collect::<String>();
        let next_id = next_id + u32::try_from(stale.borrow().len()).expect("few references");
        let instances = if instances.is_empty() {
            String::new()
        } else {
            let entries: String = instances
                .iter()
                .map(|entry| format!("        {entry},\n"))
                .collect();
            format!("    instances: [\n{entries}    ],\n")
        };
        format!(
            "Scene(\n    version: {SCENE_VERSION},\n    next_id: {next_id},\n    resources: {{{resources}}},\n    entities: [\n{entities}    ],\n{instances})\n"
        )
    }

    /// Turns a pending value into a value, remapping local identifiers of `scope` (the
    /// address prefix of the value's file) to `spawned`. A deleted entity's identifier
    /// becomes the dead entity `u32::MAX` of generation `local`.
    fn ready(
        &self,
        shapes: &BTreeMap<String, Shape>,
        name: &str,
        value: Pending,
        spawned: &BTreeMap<LoadAddress, Entity>,
        scope: &[u32],
        resource: bool,
    ) -> Box<dyn Any + Send> {
        let bytes = match value {
            Pending::Ready(value) => return value,
            Pending::Local(bytes) => bytes,
        };
        let bytes = rewrite_entities(&shapes[name].schema, &bytes, &mut |local, _| {
            let address = [scope, &[local]].concat();
            Some(spawned.get(&address).map_or((u32::MAX, local), |entity| {
                (entity.index(), entity.generation())
            }))
        })
        .expect("local identifiers were checked");
        let codec = if resource {
            self.world.resource_codec(name)
        } else {
            self.world.component_codec(name)
        };
        (codec.expect("decoded against the registry").decode)(&bytes)
            .expect("remapped bytes decode")
    }

    /// Reads every asset reachable from `references`, by identifier.
    fn resolve_assets(
        &self,
        references: Vec<Reference>,
        assets: &dyn AssetSource,
    ) -> Result<BTreeMap<AssetId, (String, LoadedValue)>, SceneError> {
        let schemas: BTreeMap<&str, TypeSchema> = self.world.asset_schemas().into_iter().collect();
        let mut loaded: BTreeMap<AssetId, (String, LoadedValue)> = BTreeMap::new();
        let mut pending = references;
        pending.reverse();
        while let Some(Reference {
            id,
            asset,
            referenced_by,
        }) = pending.pop()
        {
            let path = || assets.path(id).map(str::to_string);
            let wrong_type = |found: &str| SceneError::WrongAssetType {
                id,
                path: path(),
                referenced_by: referenced_by.clone(),
                expected: asset.clone(),
                found: found.to_string(),
            };
            if let Some((found, _)) = loaded.get(&id) {
                if *found != asset {
                    return Err(wrong_type(found));
                }
                continue;
            }
            let text = assets.text(id).ok_or_else(|| SceneError::MissingAsset {
                referenced_by: referenced_by.clone(),
                id,
            })?;
            let invalid = |reason| SceneError::InvalidAsset {
                id,
                path: path(),
                reason,
            };
            if asset == Prefab::NAME {
                if !is_prefab_file(text)
                    && let Ok((found, _)) = parse_asset_file_in_place(text)
                {
                    return Err(wrong_type(&found));
                }
                let (prefab, nested) = self.flatten(id, assets)?;
                pending.extend(nested.into_iter().rev());
                let bytes = postcard::to_allocvec(&prefab).expect("a prefab encodes");
                loaded.insert(id, (asset, LoadedValue::Bytes(bytes)));
                continue;
            }
            if is_prefab_file(text) {
                return Err(wrong_type(Prefab::NAME));
            }
            let (found, value) = parse_asset_file_in_place(text).map_err(invalid)?;
            if found != asset {
                return Err(wrong_type(&found));
            }
            if let Some(schema) = schemas.get(found.as_str()) {
                let parsed: ron::Value =
                    ron::from_str(&value).map_err(|error| invalid(error.to_string()))?;
                let mut nested = Vec::new();
                visit_handles(
                    schema,
                    &parsed,
                    &mut String::new(),
                    &mut |to, asset, field| {
                        nested.push(Reference {
                            id: to,
                            asset: asset.to_string(),
                            referenced_by: format!("asset {id}, {found}{field}"),
                        });
                    },
                );
                pending.extend(nested.into_iter().rev());
            }
            loaded.insert(id, (found, LoadedValue::Ron(value)));
        }
        Ok(loaded)
    }

    /// The flattened prefab `id`, and the handles and prefabs it reaches.
    fn flatten(
        &self,
        id: AssetId,
        assets: &dyn AssetSource,
    ) -> Result<(Prefab, Vec<Reference>), SceneError> {
        let invalid = |error: SceneError| SceneError::InvalidAsset {
            id,
            path: assets.path(id).map(str::to_string),
            reason: error.to_string(),
        };
        let (entities, instances) = expand_prefab(id, assets)?;
        let positions: BTreeMap<&[u32], u32> = entities
            .iter()
            .zip(0..)
            .map(|(entity, position)| (entity.address.as_slice(), position))
            .collect();
        let mut decoder = Decoder {
            next_id: None,
            locals: BTreeSet::new(),
            shapes: BTreeMap::new(),
            references: Vec::new(),
        };
        let mut flat = Vec::with_capacity(entities.len());
        let mut entity_components = BTreeSet::new();
        for entity in &entities {
            let scope = &entity.address[..entity.address.len() - 1];
            let decoded = self
                .decode_components(&mut decoder, &entity.address, entity.components.clone())
                .map_err(invalid)?;
            let components = decoded
                .into_iter()
                .map(|(name, value)| {
                    let codec = self.world.component_codec(&name).expect("decoded");
                    let bytes = match value {
                        Pending::Ready(value) => (codec.encode)(&*value),
                        Pending::Local(bytes) => {
                            entity_components.insert(name.clone());
                            let schema = &decoder.shapes[&name].schema;
                            rewrite_entities(schema, &bytes, &mut |local, _| {
                                let address = [scope, &[local]].concat();
                                Some(
                                    positions
                                        .get(address.as_slice())
                                        .map_or((u32::MAX, local), |&position| (position, 0)),
                                )
                            })
                            .expect("every reference is mapped")
                        }
                    };
                    (name, bytes)
                })
                .collect();
            flat.push(PrefabEntity {
                address: entity.address.clone(),
                components,
            });
        }

        let mut references: Vec<Reference> = decoder
            .references
            .into_iter()
            .map(|reference| Reference {
                referenced_by: format!("asset {id}, {}", reference.referenced_by),
                ..reference
            })
            .collect();
        references.extend(instances.into_iter().map(|(address, prefab)| Reference {
            id: prefab,
            asset: Prefab::NAME.to_string(),
            referenced_by: format!("asset {id}, instance {}", address_text(&address)),
        }));
        let prefab = Prefab {
            entities: flat,
            entity_components,
        };
        Ok((prefab, references))
    }

    fn decode_resources(
        &self,
        decoder: &mut Decoder,
        values: Vec<(String, String)>,
    ) -> Result<Decoded, SceneError> {
        let mut decoded: Decoded = Vec::new();
        for (name, text) in values {
            let codec = self
                .world
                .resource_codec(&name)
                .ok_or_else(|| SceneError::UnknownResource(name.clone()))?;
            if decoded.iter().any(|(seen, _)| *seen == name) {
                return Err(invalid(None, &name, "listed twice".to_string()));
            }
            let value = decoder.decode(codec, &name, &text, None)?;
            decoded.push((name, value));
        }
        Ok(decoded)
    }

    fn decode_components(
        &self,
        decoder: &mut Decoder,
        address: &[u32],
        values: Vec<(String, String)>,
    ) -> Result<Decoded, SceneError> {
        let entity = *address.last().expect("an address is never empty");
        let mut decoded: Decoded = Vec::new();
        for (name, text) in values {
            let codec =
                self.world
                    .component_codec(&name)
                    .ok_or_else(|| SceneError::UnknownComponent {
                        entity: entity as usize,
                        name: name.clone(),
                    })?;
            if decoded.iter().any(|(seen, _)| *seen == name) {
                return Err(invalid(
                    Some(entity as usize),
                    &name,
                    "listed twice".to_string(),
                ));
            }
            let value = decoder.decode(codec, &name, &text, Some(address))?;
            decoded.push((name, value));
        }
        Ok(decoded)
    }
}

/// An asset to insert: RON of a data asset, or the canonical bytes of a flattened prefab.
enum LoadedValue {
    Ron(String),
    Bytes(Vec<u8>),
}

/// The `id` and text of each instance entry of a scene file, in file order.
fn instance_entries(ron: &str) -> Vec<(u32, String)> {
    #[derive(Deserialize)]
    #[serde(rename = "Scene")]
    struct Entries {
        #[serde(default)]
        instances: Vec<Box<RawValue>>,
    }
    #[derive(Deserialize)]
    struct Entry {
        id: u32,
    }
    let Ok(file) = ron::from_str::<Entries>(ron) else {
        return Vec::new();
    };
    file.instances
        .iter()
        .filter_map(|raw| {
            let entry: Entry = ron::from_str(raw.get_ron()).ok()?;
            Some((entry.id, raw.get_ron().trim().to_string()))
        })
        .collect()
}

fn invalid(entity: Option<usize>, name: &str, reason: String) -> SceneError {
    SceneError::InvalidValue {
        entity,
        name: name.to_string(),
        reason,
    }
}

/// Whether `schema` or any type inside it satisfies `matches`.
fn mentions(schema: &TypeSchema, matches: &dyn Fn(&TypeSchema) -> bool) -> bool {
    let any = |items: &[TypeSchema]| items.iter().any(|item| mentions(item, matches));
    matches(schema)
        || match schema {
            TypeSchema::Option { item }
            | TypeSchema::List { item }
            | TypeSchema::Array { item, .. }
            | TypeSchema::Newtype { item, .. } => mentions(item, matches),
            TypeSchema::Map { key, value } => mentions(key, matches) || mentions(value, matches),
            TypeSchema::Tuple { items } | TypeSchema::TupleStruct { items, .. } => any(items),
            TypeSchema::Struct { fields, .. } => {
                fields.iter().any(|field| mentions(&field.schema, matches))
            }
            TypeSchema::Enum { variants, .. } => {
                variants.iter().any(|variant| match &variant.fields {
                    VariantFields::Unit => false,
                    VariantFields::Newtype { item } => mentions(item, matches),
                    VariantFields::Tuple { items } => any(items),
                    VariantFields::Struct { fields } => {
                        fields.iter().any(|field| mentions(&field.schema, matches))
                    }
                })
            }
            _ => false,
        }
}

/// Copies the postcard `bytes` of a value of `schema`, replacing every `Entity` (index,
/// generation) with what `map` returns for it. Fails with the index `map` refused.
///
/// # Panics
/// If `bytes` do not encode a value of `schema`.
fn rewrite_entities(
    schema: &TypeSchema,
    bytes: &[u8],
    map: &mut dyn FnMut(u32, u32) -> Option<(u32, u32)>,
) -> Result<Vec<u8>, u32> {
    let mut rewriter = Rewriter {
        bytes,
        out: Vec::with_capacity(bytes.len()),
        map,
    };
    rewriter.value(schema)?;
    assert!(rewriter.bytes.is_empty(), "bytes left after the value");
    Ok(rewriter.out)
}

struct Rewriter<'a> {
    bytes: &'a [u8],
    out: Vec<u8>,
    map: &'a mut dyn FnMut(u32, u32) -> Option<(u32, u32)>,
}

impl Rewriter<'_> {
    fn value(&mut self, schema: &TypeSchema) -> Result<(), u32> {
        match schema {
            TypeSchema::Bool | TypeSchema::U8 | TypeSchema::I8 => self.copy(1),
            TypeSchema::U16
            | TypeSchema::U32
            | TypeSchema::U64
            | TypeSchema::I16
            | TypeSchema::I32
            | TypeSchema::I64 => {
                self.copy_varint();
            }
            TypeSchema::F32 => self.copy(4),
            TypeSchema::F64 => self.copy(8),
            TypeSchema::String | TypeSchema::AssetId | TypeSchema::Handle { .. } => {
                let length = self.copy_varint();
                self.copy(to_usize(length));
            }
            TypeSchema::Unit | TypeSchema::UnitStruct { .. } => {}
            TypeSchema::Option { item } => {
                let some = self.bytes[0] == 1;
                self.copy(1);
                if some {
                    self.value(item)?;
                }
            }
            TypeSchema::List { item } => {
                for _ in 0..self.copy_varint() {
                    self.value(item)?;
                }
            }
            TypeSchema::Array { item, len } => {
                for _ in 0..*len {
                    self.value(item)?;
                }
            }
            TypeSchema::Map { key, value } => {
                for _ in 0..self.copy_varint() {
                    self.value(key)?;
                    self.value(value)?;
                }
            }
            TypeSchema::Tuple { items } | TypeSchema::TupleStruct { items, .. } => {
                self.values(items)?;
            }
            TypeSchema::Struct { fields, .. } => {
                for field in fields {
                    self.value(&field.schema)?;
                }
            }
            TypeSchema::Newtype { item, .. } => self.value(item)?,
            TypeSchema::Enum { variants, .. } => {
                let variant = self.copy_varint();
                match &variants[to_usize(variant)].fields {
                    VariantFields::Unit => {}
                    VariantFields::Newtype { item } => self.value(item)?,
                    VariantFields::Tuple { items } => self.values(items)?,
                    VariantFields::Struct { fields } => {
                        for field in fields {
                            self.value(&field.schema)?;
                        }
                    }
                }
            }
            TypeSchema::Entity => {
                let index = to_u32(self.read_varint());
                let generation = to_u32(self.read_varint());
                let (index, generation) = (self.map)(index, generation).ok_or(index)?;
                write_varint(&mut self.out, index.into());
                write_varint(&mut self.out, generation.into());
            }
        }
        Ok(())
    }

    fn values(&mut self, items: &[TypeSchema]) -> Result<(), u32> {
        items.iter().try_for_each(|item| self.value(item))
    }

    fn copy(&mut self, length: usize) {
        let (head, rest) = self.bytes.split_at(length);
        self.out.extend_from_slice(head);
        self.bytes = rest;
    }

    fn copy_varint(&mut self) -> u64 {
        let before = self.bytes;
        let value = self.read_varint();
        let used = before.len() - self.bytes.len();
        self.out.extend_from_slice(&before[..used]);
        value
    }

    /// A postcard (LEB128) unsigned integer.
    fn read_varint(&mut self) -> u64 {
        let mut value = 0;
        let mut shift = 0;
        loop {
            let byte = self.bytes[0];
            self.bytes = &self.bytes[1..];
            value |= u64::from(byte & 0x7f) << shift;
            if byte & 0x80 == 0 {
                return value;
            }
            shift += 7;
        }
    }
}

fn write_varint(out: &mut Vec<u8>, mut value: u64) {
    while value >= 0x80 {
        out.push((value & 0x7f) as u8 | 0x80);
        value >>= 7;
    }
    out.push(value as u8);
}

fn to_usize(value: u64) -> usize {
    usize::try_from(value).expect("a length that fits in memory")
}

fn to_u32(value: u64) -> u32 {
    u32::try_from(value).expect("an entity field fits in u32")
}
