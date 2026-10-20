//! RON scene files: the initial resources and entities of a simulation, and the assets they
//! reach through handles.

use std::any::Any;
use std::cell::{Cell, RefCell};
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use ludenea_ecs::schema::{FieldSchema, TypeSchema, VariantFields};
use ludenea_ecs::{Asset, AssetId, Codec, EcsError, Entity, Prefab, PrefabEntity, Snapshot};
use ron::value::RawValue;
use serde::de::{IgnoredAny, MapAccess, Visitor};
use serde::{Deserialize, Deserializer};

use crate::Simulation;
use crate::assets::{AssetSource, AssetTexts, NoAssets, asset_file, parse_asset_file_in_place};
use crate::path::{Path, PathSegment};
use crate::prefab::{
    Instance, InstanceInfo, LoadedInstance, PREFAB_VERSION, Resolved, address_text, expand_prefab,
    is_prefab_file, resolve,
};

pub const SCENE_VERSION: u32 = 2;

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
        "entity {} has component `{name}`, which this app does not register; register it or remove it from the scene",
        address_text(entity)
    )]
    UnknownComponent { entity: LoadAddress, name: String },
    #[error(
        "scene has resource `{0}`, which this app does not register; register it or remove it from the scene"
    )]
    UnknownResource(String),
    #[error("{} `{name}` is invalid: {reason}", subject(entity.as_deref()))]
    InvalidValue {
        entity: Option<LoadAddress>,
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
    #[error(
        "component `{component}` of entity {entity} references an entity outside the prefab at `{path}`; include that entity or clear the reference"
    )]
    ForeignReference {
        entity: u32,
        component: String,
        path: String,
    },
    #[error(
        "entity {} belongs to a prefab instance; remove the instance or override its components",
        address_text(.0)
    )]
    InstanceEntity(LoadAddress),
    #[error("{0} is not in the file; pick one it has")]
    MissingTarget(String),
    /// Writing the scene needs more local identifiers than remain from `next_id`: new
    /// entities, a new instance, and stale references each take one, and identifiers are
    /// never reused.
    #[error(
        "the scene needs more local identifiers than remain from its `next_id` {next_id}; renumber the scene from 0 and lower its `next_id`"
    )]
    IdentifiersExhausted { next_id: u32 },
}

fn subject(entity: Option<&[u32]>) -> String {
    match entity {
        Some(address) => format!("component of entity {}", address_text(address)),
        None => "resource".to_string(),
    }
}

fn located(path: &Option<String>) -> String {
    path.as_ref()
        .map(|path| format!(" ({path})"))
        .unwrap_or_default()
}

/// Read before anything else, so a scene of another version is refused as such.
#[derive(Deserialize)]
#[serde(rename = "Scene")]
struct Versioned {
    version: u32,
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
    /// Read only to refuse it: a resolved scene has its instances expanded.
    #[serde(default)]
    instances: Vec<IgnoredAny>,
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
    let versioned: Versioned = ron::from_str(ron).map_err(parse)?;
    match versioned.version {
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
                // A text in neither form is reported against the usual one. Without
                // entities, a text is not resolved even if it reads as one.
                Err(error) => {
                    return match ron::from_str::<ResolvedV2>(ron) {
                        Ok(resolved) if !resolved.entities.is_empty() => parse_resolved(resolved),
                        _ => Err(parse(error)),
                    };
                }
            };
            let next_id = growable(file.next_id)?;
            let mut seen = BTreeSet::new();
            let locals = file.entities.iter().map(|entity| entity.id);
            for local in locals.chain(file.instances.iter().map(|instance| instance.id)) {
                if !seen.insert(local) {
                    return Err(SceneError::DuplicateLocal(local));
                }
                // A new entity takes `next_id`, so it must not be taken already.
                if local >= next_id {
                    return Err(not_below_next_id(local, next_id));
                }
            }
            Ok(ParsedScene {
                next_id,
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

fn parse_resolved(file: ResolvedV2) -> Result<ParsedScene, SceneError> {
    if file.entities.iter().any(|entity| entity.address.is_empty()) {
        return Err(SceneError::Parse(
            "an entity has an empty address; give it at least its local identifier".to_string(),
        ));
    }
    if !file.instances.is_empty() {
        return Err(SceneError::Parse(
            "a scene whose entities have addresses is resolved and has no instances; remove them"
                .to_string(),
        ));
    }
    let mut seen = BTreeSet::new();
    if let Some(entity) = file.entities.iter().find(|e| !seen.insert(&e.address)) {
        return Err(SceneError::Parse(format!(
            "address {:?} is given to two entities",
            entity.address
        )));
    }
    let next_id = growable(file.next_id)?;
    // An address starts with the local identifier of its entity or instance in this scene.
    if let Some(&local) = file
        .entities
        .iter()
        .filter_map(|entity| entity.address.first())
        .find(|&&local| local >= next_id)
    {
        return Err(not_below_next_id(local, next_id));
    }
    Ok(ParsedScene {
        next_id,
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

fn not_below_next_id(local: u32, next_id: u32) -> SceneError {
    SceneError::Parse(format!(
        "local identifier {local} is not below `next_id` {next_id}; give `next_id` a value above every entity and instance identifier"
    ))
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
pub(crate) struct Reference {
    pub(crate) id: AssetId,
    pub(crate) asset: String,
    pub(crate) referenced_by: String,
}

/// Decodes scene values, collecting the handles they hold and checking their local
/// identifiers.
struct Decoder {
    /// The scene's `next_id` when its own values' `Entity` values are checked against it.
    next_id: Option<u32>,
    /// Local identifiers of the scene's own entities.
    locals: BTreeSet<u32>,
    /// By whether the value is a resource, then stable name: a component and a resource
    /// may share one.
    shapes: BTreeMap<(bool, String), Shape>,
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
            handles: schema.mentions(&|schema| matches!(schema, TypeSchema::Handle { .. })),
            entities: schema.mentions(&|schema| matches!(schema, TypeSchema::Entity)),
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
        let invalid = |reason| invalid(owner, name, reason);
        let value = (codec.decode_ron)(text).map_err(invalid)?;
        let shape = self
            .shapes
            .entry((owner.is_none(), name.to_string()))
            .or_insert_with(|| Shape::of(codec));

        if shape.handles {
            let references = &mut self.references;
            visit_handles(
                &shape.schema,
                &(codec.encode)(&*value),
                &mut |id, asset, field| {
                    let referenced_by = match owner {
                        Some(address) => format!("entity {}, {name}{field}", address_text(address)),
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
            .map_err(|(local, _)| match owner_local {
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
        let text = resolved.to_scene_text();
        let load = self.load_resolved(resolved, assets)?;
        self.resolved_scene = Some(text);
        self.asset_paths = self
            .world
            .asset_ids()
            .into_iter()
            .filter_map(|id| Some((id, assets.path(id)?.to_string())))
            .collect();
        Ok(load)
    }

    /// The resolved text (`Resolved::to_scene_text`) of the last scene loaded; not part of
    /// the state or its hash.
    pub fn resolved_scene(&self) -> Option<&str> {
        self.resolved_scene.as_deref()
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
        let loaded = self.resolve_assets(references, assets, &BTreeSet::new())?;

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
        let mut overrides = resolved.overrides;
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
                        overrides: overrides.remove(&address).unwrap_or_default(),
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
    /// reference to a dead entity. Refused with `SceneError::IdentifiersExhausted` when
    /// they need `u32::MAX` identifiers or more.
    pub fn save_scene(&self) -> Result<String, SceneError> {
        let alive = self.world.entities().map(|entity| (entity, None)).collect();
        self.write_scene(alive, 0, &[])
    }

    /// Like `save_scene`, but entities that still have a load address keep their local
    /// identifier from `previous` (the scene last loaded), and new entities get fresh
    /// identifiers from `previous`'s `next_id`, so an identifier is never reused. An
    /// instance of `previous` whose entities are all alive and unchanged keeps its entry
    /// byte for byte; the entities of other instances are written as scene entities.
    /// Refused with `SceneError::IdentifiersExhausted` when the new entities and stale
    /// references need more identifiers than remain from `previous`'s `next_id`.
    pub fn save_scene_keeping(&self, previous: &str) -> Result<String, SceneError> {
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
        let next_id = next_id_of(previous);
        let next_id = alive
            .iter()
            .filter_map(kept)
            .try_fold(next_id, |next_id, local| Some(after(local)?.max(next_id)))
            .ok_or(SceneError::IdentifiersExhausted { next_id })?;
        let locals = alive
            .into_iter()
            .map(|entity| (entity, kept(&entity)))
            .collect();
        self.write_scene(locals, next_id, &entries)
    }

    /// The RON of `component`'s postcard `bytes` as an override or added value of the
    /// entity at load address `address`: its `Entity` values name local identifiers of the
    /// file that entity comes from, and one naming an entity outside that file is refused.
    /// A `component` the app does not register is refused with
    /// `SceneError::UnknownComponent`, and `bytes` that do not decode with
    /// `SceneError::InvalidValue`.
    pub fn override_value_ron(
        &self,
        address: &[u32],
        component: &str,
        bytes: &[u8],
    ) -> Result<String, SceneError> {
        let codec =
            self.world
                .component_codec(component)
                .ok_or_else(|| SceneError::UnknownComponent {
                    entity: address.to_vec(),
                    name: component.to_string(),
                })?;
        let scope = &address[..address.len().saturating_sub(1)];
        value_ron(codec, bytes, &mut |index, generation| {
            let (_, at) = self
                .load_addresses
                .iter()
                .find(|(entity, _)| entity.index() == index && entity.generation() == generation)?;
            match at.strip_prefix(scope) {
                Some(&[local]) => Some((local, 0)),
                _ => None,
            }
        })
        .map_err(|error| {
            error.into_scene_error(Some(address), component, |(_, path)| {
                SceneError::ForeignReference {
                    entity: address.last().copied().unwrap_or_default(),
                    component: component.to_string(),
                    path: Path(&path).to_string(),
                }
            })
        })
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

    /// Writes `locals` (alive entities in index order with their local identifier, none for
    /// a new entity), then the instance entries `instances` as they are. New entities, then
    /// stale references in write order, get identifiers from `next_id` on.
    fn write_scene(
        &self,
        locals: Vec<(Entity, Option<u32>)>,
        next_id: u32,
        instances: &[String],
    ) -> Result<String, SceneError> {
        let exhausted = || SceneError::IdentifiersExhausted { next_id };
        // `previous` may be any text: one whose `next_id` cannot grow would not load back.
        let free = Cell::new(growable(next_id).map_err(|_| exhausted())?);
        let fresh = || {
            let local = free.get();
            free.set(after(local)?);
            Some(local)
        };
        let locals: Vec<(Entity, u32)> = locals
            .into_iter()
            .map(|(entity, local)| Some((entity, local.or_else(fresh)?)))
            .collect::<Option<_>>()
            .ok_or_else(exhausted)?;
        let by_entity: BTreeMap<(u32, u32), u32> = locals
            .iter()
            .map(|(entity, local)| ((entity.index(), entity.generation()), *local))
            .collect();
        let stale: RefCell<BTreeMap<(u32, u32), u32>> = RefCell::default();
        // Only `fresh` refuses an entity, once the identifiers run out.
        let ron = |entity: Option<&[u32]>, name: &str, codec: Codec, bytes: &[u8]| {
            value_ron(codec, bytes, &mut |index, generation| {
                let key = (index, generation);
                let mut stale = stale.borrow_mut();
                let local = match by_entity.get(&key).or(stale.get(&key)) {
                    Some(&local) => local,
                    None => {
                        let local = fresh()?;
                        stale.insert(key, local);
                        local
                    }
                };
                Some((local, 0))
            })
            .map_err(|error| error.into_scene_error(entity, name, |_| exhausted()))
        };

        let snapshot = self.world.snapshot();
        let resources = snapshot
            .resources
            .iter()
            .map(|(name, bytes)| {
                let codec = self.world.resource_codec(name).expect("snapshotted name");
                Ok(format!("{name:?}: {}", ron(None, name, codec, bytes)?))
            })
            .collect::<Result<Vec<_>, SceneError>>()?
            .join(", ");
        let entities = locals
            .iter()
            .map(|&(entity, local)| {
                let components = self.components_ron(&snapshot, entity, |name, codec, bytes| {
                    ron(Some(&[local]), name, codec, bytes)
                })?;
                Ok(format!(
                    "    (id: {local}, components: {{{components}}}),\n"
                ))
            })
            .collect::<Result<String, SceneError>>()?;
        let next_id = free.get();
        let instances = if instances.is_empty() {
            String::new()
        } else {
            let entries: String = instances
                .iter()
                .map(|entry| format!("        {entry},\n"))
                .collect();
            format!("    instances: [\n{entries}    ],\n")
        };
        Ok(format!(
            "Scene(\n    version: {SCENE_VERSION},\n    next_id: {next_id},\n    resources: {{{resources}}},\n    entities: [\n{entities}    ],\n{instances})\n"
        ))
    }

    /// Writes `entities` as a prefab file with local identifiers `0..n` in that order, each
    /// entity once. A dead entity, and an `Entity` value naming an entity outside
    /// `entities`, are refused.
    pub(crate) fn write_prefab(&self, entities: &[Entity]) -> Result<String, SceneError> {
        if let Some(dead) = entities
            .iter()
            .find(|&&entity| !self.world.is_alive(entity))
        {
            return Err(SceneError::MissingTarget(format!("entity {dead}")));
        }
        let mut seen = BTreeSet::new();
        let entities: Vec<Entity> = entities
            .iter()
            .copied()
            .filter(|&entity| seen.insert(entity))
            .collect();
        let entities = &entities[..];
        let locals: BTreeMap<(u32, u32), u32> = entities
            .iter()
            .map(|entity| (entity.index(), entity.generation()))
            .zip(0..)
            .collect();
        let snapshot = self.world.snapshot();
        let mut written = String::new();
        for (&entity, local) in entities.iter().zip(0..) {
            let components = self.components_ron(&snapshot, entity, |name, codec, bytes| {
                value_ron(codec, bytes, &mut |index, generation| {
                    let local = locals.get(&(index, generation))?;
                    Some((*local, 0))
                })
                .map_err(|error| {
                    error.into_scene_error(Some(&[local]), name, |(_, path)| {
                        SceneError::ForeignReference {
                            entity: entity.index(),
                            component: name.to_string(),
                            path: Path(&path).to_string(),
                        }
                    })
                })
            })?;
            written.push_str(&format!(
                "        (id: {local}, components: {{{components}}}),\n"
            ));
        }
        Ok(format!(
            "Prefab(\n    version: {PREFAB_VERSION},\n    next_id: {},\n    entities: [\n{written}    ],\n)\n",
            entities.len()
        ))
    }

    /// The components of `entity` in `snapshot` as `"name": value` pairs, each value
    /// written by `ron`.
    fn components_ron<E>(
        &self,
        snapshot: &Snapshot,
        entity: Entity,
        mut ron: impl FnMut(&str, Codec, &[u8]) -> Result<String, E>,
    ) -> Result<String, E> {
        let mut pairs = Vec::new();
        for (name, values) in &snapshot.components {
            if let Some((_, bytes)) = values.iter().find(|(owner, _)| *owner == entity) {
                let codec = self.world.component_codec(name).expect("snapshotted name");
                pairs.push(format!("{name:?}: {}", ron(name, codec, bytes)?));
            }
        }
        Ok(pairs.join(", "))
    }

    /// Turns a pending value into a value, remapping local identifiers of `scope` (the
    /// address prefix of the value's file) to `spawned`. A deleted entity's identifier
    /// becomes the dead entity `u32::MAX` of generation `local`.
    fn ready(
        &self,
        shapes: &BTreeMap<(bool, String), Shape>,
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
        let bytes = rewrite_entities(
            &shapes[&(resource, name.to_string())].schema,
            &bytes,
            &mut |local, _| {
                let address = [scope, &[local]].concat();
                Some(spawned.get(&address).map_or((u32::MAX, local), |entity| {
                    (entity.index(), entity.generation())
                }))
            },
        )
        .expect("local identifiers were checked");
        let codec = if resource {
            self.world.resource_codec(name)
        } else {
            self.world.component_codec(name)
        };
        (codec.expect("decoded against the registry").decode)(&bytes)
            .expect("remapped bytes decode")
    }

    /// Reads every asset reachable from `references`, by identifier, except the `known`
    /// ones and what only they reach.
    pub(crate) fn resolve_assets(
        &self,
        references: Vec<Reference>,
        assets: &dyn AssetSource,
        known: &BTreeSet<AssetId>,
    ) -> Result<BTreeMap<AssetId, (String, LoadedValue)>, SceneError> {
        let mut loaded: BTreeMap<AssetId, (String, LoadedValue)> = BTreeMap::new();
        let mut pending = references;
        pending.reverse();
        while let Some(Reference {
            id,
            asset,
            referenced_by,
        }) = pending.pop()
        {
            if known.contains(&id) {
                continue;
            }
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
                    && let Ok((found, value)) = parse_asset_file_in_place(text)
                {
                    if found != asset {
                        return Err(wrong_type(&found));
                    }
                    // Already flattened, in file form: how a replay stores a prefab.
                    let prefab: Prefab =
                        ron::from_str(&value).map_err(|error| invalid(error.to_string()))?;
                    let nested = self.flat_prefab_handles(id, &prefab).map_err(invalid)?;
                    pending.extend(nested.into_iter().rev());
                    loaded.insert(id, (found, LoadedValue::Ron(value)));
                    continue;
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
            if let Some(codec) = self.world.asset_codec(&found) {
                let decoded = (codec.decode_ron)(&value).map_err(invalid)?;
                let mut nested = Vec::new();
                visit_handles(
                    &(codec.schema)(),
                    &(codec.encode)(&*decoded),
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

    /// The handles inside the components of `prefab`, the already flattened asset `id`.
    fn flat_prefab_handles(&self, id: AssetId, prefab: &Prefab) -> Result<Vec<Reference>, String> {
        let mut references = Vec::new();
        for entity in &prefab.entities {
            for (name, bytes) in &entity.components {
                let codec = self
                    .world
                    .component_codec(name)
                    .ok_or_else(|| format!("component `{name}` is not registered"))?;
                let value = (codec.decode)(bytes)
                    .map_err(|reason| format!("component `{name}`: {reason}"))?;
                visit_handles(
                    &(codec.schema)(),
                    &(codec.encode)(&*value),
                    &mut |to, asset, field| {
                        references.push(Reference {
                            id: to,
                            asset: asset.to_string(),
                            referenced_by: format!(
                                "asset {id}, entity {}, {name}{field}",
                                address_text(&entity.address)
                            ),
                        });
                    },
                );
            }
        }
        Ok(references)
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
                            let schema = &decoder.shapes[&(false, name.clone())].schema;
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
        Ok((Prefab { entities: flat }, references))
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
        let mut decoded: Decoded = Vec::new();
        for (name, text) in values {
            let codec =
                self.world
                    .component_codec(&name)
                    .ok_or_else(|| SceneError::UnknownComponent {
                        entity: address.to_vec(),
                        name: name.clone(),
                    })?;
            if decoded.iter().any(|(seen, _)| *seen == name) {
                return Err(invalid(Some(address), &name, "listed twice".to_string()));
            }
            let value = decoder.decode(codec, &name, &text, Some(address))?;
            decoded.push((name, value));
        }
        Ok(decoded)
    }
}

/// An asset to insert: RON of a data asset, or the canonical bytes of a flattened prefab.
pub(crate) enum LoadedValue {
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

/// `next_id` when an identifier can still be given from it: `u32::MAX` is refused at load
/// rather than when a save runs out of identifiers.
fn growable(next_id: u32) -> Result<u32, SceneError> {
    if next_id == u32::MAX {
        return Err(SceneError::Parse(format!(
            "`next_id` is {next_id}, which leaves no identifier for a new entity; renumber the scene"
        )));
    }
    Ok(next_id)
}

/// The local identifier after `id`, or `None` when it would be `u32::MAX`: no scene holds
/// it, as `growable` refuses it as a `next_id`.
pub(crate) fn after(id: u32) -> Option<u32> {
    id.checked_add(1).filter(|&after| after < u32::MAX)
}

fn invalid(entity: Option<&[u32]>, name: &str, reason: String) -> SceneError {
    SceneError::InvalidValue {
        entity: entity.map(<[u32]>::to_vec),
        name: name.to_string(),
        reason,
    }
}

/// Why `value_ron` cannot write a value.
enum Unwritten {
    /// `map` refused an entity: its index and the path to it.
    Refused(Refused),
    /// The value does not decode, as when the game's own conversion refuses a value its
    /// systems or its build wrote.
    Undecodable(String),
}

impl Unwritten {
    /// The error of a value of `name` at `entity` (none for a resource), with `refused`
    /// for an entity `map` refused.
    fn into_scene_error(
        self,
        entity: Option<&[u32]>,
        name: &str,
        refused: impl FnOnce(Refused) -> SceneError,
    ) -> SceneError {
        match self {
            Unwritten::Refused(at) => refused(at),
            Unwritten::Undecodable(reason) => invalid(entity, name, reason),
        }
    }
}

/// The RON of a value's postcard `bytes`, its `Entity` values mapped by `map` (see
/// `rewrite_entities`).
fn value_ron(
    codec: Codec,
    bytes: &[u8],
    map: &mut dyn FnMut(u32, u32) -> Option<(u32, u32)>,
) -> Result<String, Unwritten> {
    let schema = (codec.schema)();
    let bytes = if schema.mentions(&|schema| matches!(schema, TypeSchema::Entity)) {
        rewrite_entities(&schema, bytes, map).map_err(Unwritten::Refused)?
    } else {
        bytes.to_vec()
    };
    let value = (codec.decode)(&bytes).map_err(Unwritten::Undecodable)?;
    Ok((codec.encode_ron)(&*value))
}

/// Copies the postcard `bytes` of a value of `schema`, replacing every `Entity` (index,
/// generation) with what `map` returns for it. Fails with the index `map` refused and the
/// path to it.
///
/// # Panics
/// If `bytes` do not encode a value of `schema`.
fn rewrite_entities(
    schema: &TypeSchema,
    bytes: &[u8],
    map: &mut dyn FnMut(u32, u32) -> Option<(u32, u32)>,
) -> Result<Vec<u8>, (u32, Vec<PathSegment>)> {
    let mut rewriter = Rewriter {
        bytes,
        out: Vec::with_capacity(bytes.len()),
        map,
        handles: &mut |_, _, _| {},
        path: Vec::new(),
    };
    rewriter.value(schema)?;
    assert!(rewriter.bytes.is_empty(), "bytes left after the value");
    Ok(rewriter.out)
}

/// Calls `found` with the identifier, asset type name, and field path (such as `.stats`,
/// `.weapons[2]`, or `.Pair[0]` for a tuple variant: tuple elements read as indexes, as in
/// every path `Rewriter` reports) of every handle inside the postcard `bytes` of a value of
/// `schema`. Enum values are walked through their own variant only.
///
/// # Panics
/// If `bytes` do not encode a value of `schema`.
pub(crate) fn visit_handles(
    schema: &TypeSchema,
    bytes: &[u8],
    found: &mut dyn FnMut(AssetId, &str, &str),
) {
    let mut rewriter = Rewriter {
        bytes,
        out: Vec::new(),
        map: &mut |index, generation| Some((index, generation)),
        handles: &mut |id, asset, path| {
            let field: String = path
                .iter()
                .map(|segment| match segment {
                    PathSegment::Field { name } => format!(".{name}"),
                    PathSegment::Index { index } => format!("[{index}]"),
                })
                .collect();
            found(id, asset, &field);
        },
        path: Vec::new(),
    };
    rewriter.value(schema).expect("every entity is kept");
    assert!(rewriter.bytes.is_empty(), "bytes left after the value");
}

struct Rewriter<'a> {
    bytes: &'a [u8],
    out: Vec<u8>,
    map: &'a mut dyn FnMut(u32, u32) -> Option<(u32, u32)>,
    /// Called with every handle and its path.
    handles: &'a mut dyn FnMut(AssetId, &str, &[PathSegment]),
    /// Where the value being copied lies, for errors.
    path: Vec<PathSegment>,
}

type Refused = (u32, Vec<PathSegment>);

impl Rewriter<'_> {
    fn value(&mut self, schema: &TypeSchema) -> Result<(), Refused> {
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
            TypeSchema::String | TypeSchema::AssetId => {
                let length = self.copy_varint();
                self.copy(to_usize(length));
            }
            TypeSchema::Handle { asset } => {
                let length = to_usize(self.copy_varint());
                let id = std::str::from_utf8(&self.bytes[..length])
                    .ok()
                    .and_then(|hex| u128::from_str_radix(hex, 16).ok())
                    .expect("a handle encodes its identifier in hexadecimal");
                (self.handles)(AssetId(id), asset, &self.path);
                self.copy(length);
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
                for index in 0..self.copy_varint() {
                    self.at(index, |this| this.value(item))?;
                }
            }
            TypeSchema::Array { item, len } => {
                for index in 0..*len {
                    self.at(index as u64, |this| this.value(item))?;
                }
            }
            TypeSchema::Map { key, value } => {
                // ponytail: a map entry is named by its position, not its key.
                for index in 0..self.copy_varint() {
                    self.at(index, |this| {
                        this.value(key)?;
                        this.value(value)
                    })?;
                }
            }
            TypeSchema::Tuple { items } | TypeSchema::TupleStruct { items, .. } => {
                self.values(items)?;
            }
            TypeSchema::Struct { fields, .. } => self.fields(fields)?,
            TypeSchema::Newtype { item, .. } => self.value(item)?,
            TypeSchema::Enum { variants, .. } => {
                let variant = &variants[to_usize(self.copy_varint())];
                self.path.push(PathSegment::Field {
                    name: variant.name.clone(),
                });
                match &variant.fields {
                    VariantFields::Unit => {}
                    VariantFields::Newtype { item } => self.value(item)?,
                    VariantFields::Tuple { items } => self.values(items)?,
                    VariantFields::Struct { fields } => self.fields(fields)?,
                }
                self.path.pop();
            }
            TypeSchema::Entity => {
                let index = to_u32(self.read_varint());
                let generation = to_u32(self.read_varint());
                let (index, generation) =
                    (self.map)(index, generation).ok_or_else(|| (index, self.path.clone()))?;
                write_varint(&mut self.out, index.into());
                write_varint(&mut self.out, generation.into());
            }
            TypeSchema::Recursive { name } => {
                unreachable!("registration refuses `{name}`, a recursive type")
            }
        }
        Ok(())
    }

    fn values(&mut self, items: &[TypeSchema]) -> Result<(), Refused> {
        (0..)
            .zip(items)
            .try_for_each(|(index, item)| self.at(index, |this| this.value(item)))
    }

    fn fields(&mut self, fields: &[FieldSchema]) -> Result<(), Refused> {
        for field in fields {
            self.path.push(PathSegment::Field {
                name: field.name.clone(),
            });
            self.value(&field.schema)?;
            self.path.pop();
        }
        Ok(())
    }

    /// Runs `copy` with `index` appended to the path.
    fn at(
        &mut self,
        index: u64,
        copy: impl FnOnce(&mut Self) -> Result<(), Refused>,
    ) -> Result<(), Refused> {
        self.path.push(PathSegment::Index {
            index: to_u32(index),
        });
        copy(self)?;
        self.path.pop();
        Ok(())
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
