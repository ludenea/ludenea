//! Prefab files, and the resolution of a scene's prefab instances into plain entities.
//!
//! `Entity` values in a component name local identifiers of the file the entity comes
//! from: an entity at address `[7, 3, 1]` names its siblings `[7, 3, x]`. Override and
//! added values use the scope of the entity they apply to.

use std::collections::{BTreeMap, BTreeSet};
use std::ops::Range;

use ludenea_ecs::{Asset, AssetId, Entity, Prefab};
use ron::value::RawValue;
use serde::{Deserialize, Serialize};

use crate::Simulation;
use crate::assets::{AssetSource, parse_asset_file};
use crate::path::{Path, PathSegment};
use crate::scene::{LoadAddress, NamedValues, SceneEntity, SceneError, parse_scene};

pub const PREFAB_VERSION: u32 = 1;

/// Instance addresses and their prefabs.
pub(crate) type Instances = Vec<(LoadAddress, AssetId)>;

/// A scene with every instance expanded: what a simulation loads.
#[derive(Clone, Debug)]
pub struct Resolved {
    pub resources: Vec<(String, String)>,
    pub entities: Vec<ResolvedEntity>,
    pub warnings: Vec<String>,
    pub(crate) next_id: u32,
    /// Whether scene entities' `Entity` values must name identifiers below `next_id`.
    pub(crate) checked: bool,
    /// Every instance, nested ones included, parents first.
    pub(crate) instances: Instances,
    /// The field overrides of the file's own instances, by instance address.
    pub(crate) overrides: BTreeMap<LoadAddress, Vec<OverrideInfo>>,
}

/// One entity of a resolved scene and its components' RON, by stable name.
#[derive(Clone, Debug, PartialEq)]
pub struct ResolvedEntity {
    pub address: LoadAddress,
    pub components: Vec<(String, String)>,
}

/// A prefab instance of the last loaded scene and the entities it spawned, nested
/// instances' included.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct InstanceInfo {
    pub address: LoadAddress,
    pub prefab: AssetId,
    pub entities: Vec<Entity>,
    /// The instance's field overrides, as its entry in the loaded file gives them; empty for
    /// a nested instance, whose overrides belong to its prefab file.
    #[serde(default)]
    #[cfg_attr(feature = "ts", ts(optional = false))]
    pub overrides: Vec<OverrideInfo>,
}

/// A field override of an instance: `path` of `component` on its entity `entity` (a path of
/// local identifiers). `stale` when the prefab no longer has that target, so the load
/// ignored it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct OverrideInfo {
    pub entity: Vec<u32>,
    pub component: String,
    pub path: Vec<PathSegment>,
    pub stale: bool,
}

/// An instance as loaded, with each entity's components then, to tell whether it changed.
pub(crate) struct LoadedInstance {
    pub(crate) info: InstanceInfo,
    pub(crate) components: Vec<BTreeMap<String, String>>,
}

/// An instance entry of a scene or prefab file.
#[derive(Deserialize)]
pub(crate) struct Instance {
    pub(crate) id: u32,
    prefab: AssetId,
    #[serde(default)]
    overrides: Vec<Override>,
    #[serde(default)]
    added: Vec<Added>,
    #[serde(default)]
    removed: Vec<Removed>,
}

#[derive(Deserialize)]
struct Override {
    entity: Vec<u32>,
    component: String,
    path: Vec<PathSegment>,
    value: Box<RawValue>,
}

#[derive(Deserialize)]
struct Added {
    entity: Vec<u32>,
    component: String,
    value: Box<RawValue>,
}

#[derive(Deserialize)]
struct Removed {
    entity: Vec<u32>,
    component: String,
}

#[derive(Deserialize)]
#[serde(rename = "Prefab")]
struct PrefabHeader {
    version: u32,
}

#[derive(Deserialize)]
#[serde(rename = "Prefab")]
struct PrefabFile {
    #[serde(rename = "next_id")]
    _next_id: u32,
    entities: Vec<SceneEntity>,
    #[serde(default)]
    instances: Vec<Instance>,
}

/// Expands every instance of the scene `file` with the prefabs `assets` holds. Stale
/// overrides are reported in `warnings` and ignored.
pub fn resolve(file: &str, assets: &dyn AssetSource) -> Result<Resolved, SceneError> {
    let scene = parse_scene(file)?;
    let mut resolver = Resolver::new(assets);
    let mut entities: Vec<ResolvedEntity> = scene
        .entities
        .into_iter()
        .map(|(address, components)| ResolvedEntity {
            address,
            components: texts(components),
        })
        .collect();
    for instance in scene.instances {
        resolver.instance(&[], instance, &mut entities)?;
    }
    Ok(Resolved {
        resources: texts(scene.resources),
        entities,
        warnings: resolver.warnings,
        next_id: scene.next_id,
        checked: scene.checked,
        instances: resolver.instances,
        overrides: resolver.overrides,
    })
}

impl Resolved {
    /// A version 2 scene without instances, each entity written with its address.
    pub fn to_scene_text(&self) -> String {
        let named = |values: &[(String, String)]| {
            values
                .iter()
                .map(|(name, value)| format!("{name:?}: {value}"))
                .collect::<Vec<_>>()
                .join(", ")
        };
        let entities: String = self
            .entities
            .iter()
            .map(|entity| {
                format!(
                    "    (address: {:?}, components: {{{}}}),\n",
                    entity.address,
                    named(&entity.components)
                )
            })
            .collect();
        format!(
            "Scene(\n    version: 2,\n    next_id: {},\n    resources: {{{}}},\n    entities: [\n{entities}    ],\n)\n",
            self.next_id,
            named(&self.resources)
        )
    }
}

/// Expands the prefab `id` alone, with addresses relative to it, for the flattened asset.
/// Also returns every prefab its nested instances use.
pub(crate) fn expand_prefab(
    id: AssetId,
    assets: &dyn AssetSource,
) -> Result<(Vec<ResolvedEntity>, Instances), SceneError> {
    let mut resolver = Resolver::new(assets);
    let mut entities = Vec::new();
    // ponytail: warnings of a prefab reached only by handle are dropped; the editor shows
    // them when the prefab itself is opened.
    resolver.prefab(id, &[], &mut entities)?;
    Ok((entities, resolver.instances))
}

/// Whether `text` is a prefab file rather than a data asset file.
pub(crate) fn is_prefab_file(text: &str) -> bool {
    ron::from_str::<PrefabHeader>(text).is_ok()
}

/// An address as it reads in messages, such as `7/3/1`.
pub(crate) fn address_text(address: &[u32]) -> String {
    address
        .iter()
        .map(u32::to_string)
        .collect::<Vec<_>>()
        .join("/")
}

fn texts(values: NamedValues) -> Vec<(String, String)> {
    values
        .0
        .into_iter()
        .map(|(name, raw)| (name, raw.get_ron().trim().to_string()))
        .collect()
}

struct Resolver<'a> {
    assets: &'a dyn AssetSource,
    /// The prefabs being expanded, outermost first.
    stack: Vec<AssetId>,
    warnings: Vec<String>,
    instances: Instances,
    overrides: BTreeMap<LoadAddress, Vec<OverrideInfo>>,
}

impl<'a> Resolver<'a> {
    fn new(assets: &'a dyn AssetSource) -> Self {
        Self {
            assets,
            stack: Vec::new(),
            warnings: Vec::new(),
            instances: Vec::new(),
            overrides: BTreeMap::new(),
        }
    }

    fn name(&self, id: AssetId) -> String {
        self.assets
            .path(id)
            .map_or_else(|| id.to_string(), str::to_string)
    }

    /// Appends the entities of `instance`, inside `prefix`, with its overrides applied.
    fn instance(
        &mut self,
        prefix: &[u32],
        instance: Instance,
        out: &mut Vec<ResolvedEntity>,
    ) -> Result<(), SceneError> {
        let address = [prefix, &[instance.id]].concat();
        self.instances.push((address.clone(), instance.prefab));
        let start = out.len();
        self.prefab(instance.prefab, &address, out)?;
        let entities = &mut out[start..];
        let mut stale = Vec::new();
        let find = |entities: &mut [ResolvedEntity], entity: &[u32]| {
            let target = [address.as_slice(), entity].concat();
            entities.iter().position(|e| e.address == target)
        };

        for removed in instance.removed {
            let found = find(entities, &removed.entity).and_then(|position| {
                let components = &mut entities[position].components;
                let index = components
                    .iter()
                    .position(|(n, _)| *n == removed.component)?;
                components.remove(index);
                Some(())
            });
            if found.is_none() {
                stale.push(format!(
                    "removal of `{}` from entity {}",
                    removed.component,
                    address_text(&removed.entity)
                ));
            }
        }
        for added in instance.added {
            let value = added.value.get_ron().trim().to_string();
            match find(entities, &added.entity) {
                Some(position) => {
                    let components = &mut entities[position].components;
                    match components.iter_mut().find(|(n, _)| *n == added.component) {
                        Some((_, existing)) => *existing = value,
                        None => components.push((added.component, value)),
                    }
                }
                None => stale.push(format!(
                    "added `{}` on entity {}",
                    added.component,
                    address_text(&added.entity)
                )),
            }
        }
        let mut overrides = Vec::new();
        for change in instance.overrides {
            let value = change.value.get_ron().trim();
            let found = find(entities, &change.entity).and_then(|position| {
                let components = &mut entities[position].components;
                let (_, text) = components
                    .iter_mut()
                    .find(|(n, _)| *n == change.component)?;
                *text = set_at_ron_path(text, &change.path, value)?;
                Some(())
            });
            if found.is_none() {
                stale.push(format!(
                    "override of `{}` at `{}` on entity {}",
                    change.component,
                    Path(&change.path),
                    address_text(&change.entity)
                ));
            }
            overrides.push(OverrideInfo {
                entity: change.entity,
                component: change.component,
                path: change.path,
                stale: found.is_none(),
            });
        }
        if prefix.is_empty() {
            self.overrides.insert(address.clone(), overrides);
        }

        let prefab = self.name(instance.prefab);
        self.warnings.extend(stale.into_iter().map(|target| {
            format!(
                "instance {} of prefab {prefab}: the {target} targets nothing the prefab has; it is kept in the file and ignored",
                address_text(&address)
            )
        }));
        Ok(())
    }

    /// Appends the entities of the prefab `id`, instantiated at `address`.
    fn prefab(
        &mut self,
        id: AssetId,
        address: &[u32],
        out: &mut Vec<ResolvedEntity>,
    ) -> Result<(), SceneError> {
        let referenced_by = format!("instance {}", address_text(address));
        let text = self
            .assets
            .text(id)
            .ok_or_else(|| SceneError::MissingAsset {
                referenced_by: referenced_by.clone(),
                id,
            })?;
        if let Some(position) = self.stack.iter().position(|&seen| seen == id) {
            let chain = self.stack[position..].iter().chain([&id]);
            return Err(SceneError::PrefabCycle(
                chain.map(|&id| self.name(id)).collect(),
            ));
        }
        let path = || self.assets.path(id).map(str::to_string);
        let invalid = |reason: String| SceneError::InvalidAsset {
            id,
            path: path(),
            reason,
        };
        let file = match ron::from_str::<PrefabHeader>(text) {
            Ok(header) if header.version != PREFAB_VERSION => {
                return Err(invalid(format!(
                    "prefab version {} is not supported (expected {PREFAB_VERSION})",
                    header.version
                )));
            }
            Ok(_) => ron::from_str::<PrefabFile>(text).map_err(|e| invalid(e.to_string()))?,
            Err(error) => {
                return Err(match parse_asset_file(text) {
                    Ok((found, _)) => SceneError::WrongAssetType {
                        id,
                        path: path(),
                        referenced_by,
                        expected: Prefab::NAME.to_string(),
                        found,
                    },
                    Err(_) => invalid(error.to_string()),
                });
            }
        };
        let mut seen = BTreeSet::new();
        let ids = file
            .entities
            .iter()
            .map(|e| e.id)
            .chain(file.instances.iter().map(|i| i.id));
        for local in ids {
            if !seen.insert(local) {
                return Err(invalid(format!(
                    "local identifier {local} is given twice; give each entity and instance its own"
                )));
            }
        }

        self.stack.push(id);
        out.extend(file.entities.into_iter().map(|entity| ResolvedEntity {
            address: [address, &[entity.id]].concat(),
            components: texts(entity.components),
        }));
        for instance in file.instances {
            self.instance(address, instance, out)?;
        }
        self.stack.pop();
        Ok(())
    }
}

/// A prefab file of `entities`' current components, with local identifiers `0..n` in
/// that order. An `Entity` value naming an entity outside `entities` is refused.
pub fn create_prefab(sim: &Simulation, entities: &[Entity]) -> Result<String, SceneError> {
    sim.write_prefab(entities)
}

/// Appends an instance of `prefab` to the scene or prefab `file` with the identifier
/// `next_id` gives, and returns the new text and that identifier.
pub fn place_instance(file: &str, prefab: AssetId) -> Result<(String, u32), SceneError> {
    let root = parse_file(file)?;
    let missing =
        || SceneError::Parse("the file has no `next_id`; save it as version 2 first".to_string());
    let next = field(file, &root, "next_id").ok_or_else(missing)?;
    let id: u32 = file[next.span.clone()].parse().map_err(|_| missing())?;
    let text = splice(file, next.span.clone(), &(id + 1).to_string());
    let entry = format!("(id: {id}, prefab: \"{prefab}\", overrides: [], added: [], removed: [])");
    Ok((append(&text, ROOT_INSTANCES, &entry)?, id))
}

/// Removes the scene entities `entities` from `file` and places an instance of `prefab`
/// in their stead; returns the new text and the instance's local identifier.
pub fn replace_with_instance(
    file: &str,
    entities: &[u32],
    prefab: AssetId,
) -> Result<(String, u32), SceneError> {
    let mut text = file.to_string();
    for &id in entities {
        let named = |text: &str, entry: &Node| id_of(text, entry) == Some(id);
        let (removed, count) = remove_where(&text, ROOT_ENTITIES, &named)?;
        if count == 0 {
            let root = parse_file(&text)?;
            let (_, instances) = list(&text, &root, ROOT_INSTANCES)?;
            let instances = instances.map_or(&[][..], items);
            return Err(if instances.iter().any(|entry| named(&text, entry)) {
                SceneError::InstanceEntity(vec![id])
            } else {
                SceneError::MissingTarget(format!("entity {id}"))
            });
        }
        text = removed;
    }
    place_instance(&text, prefab)
}

/// Sets the override of `component` at `path` on `entity` of the instance at `instance`
/// (a path of local identifiers, the first one in `file`), replacing an existing one.
/// Overrides apply in file order. An empty `path` overrides the whole component: it drops
/// the component's field overrides (the whole value holds them) and is appended, so field
/// overrides set afterwards come after it and win on their fields.
pub fn set_override(
    file: &str,
    instance: &[u32],
    entity: &[u32],
    component: &str,
    path: &[PathSegment],
    value: &str,
) -> Result<String, SceneError> {
    check_value(value)?;
    let (id, entity) = split_target(instance, entity)?;
    let path_ron = ron::to_string(path).expect("a path encodes");
    let entry =
        format!("(entity: {entity:?}, component: {component:?}, path: {path_ron}, value: {value})");
    let at = ListAt {
        instance: Some(id),
        name: "overrides",
    };
    let file = if path.is_empty() {
        &remove_where(file, at, &targets(&entity, component, None))?.0
    } else {
        file
    };
    upsert(
        file,
        at,
        &targets(&entity, component, Some(path)),
        value,
        &entry,
    )
}

/// Removes the override of `component` at `path` on `entity` of `instance`; an empty
/// `path` removes every override of the component.
pub fn revert_override(
    file: &str,
    instance: &[u32],
    entity: &[u32],
    component: &str,
    path: &[PathSegment],
) -> Result<String, SceneError> {
    let (id, entity) = split_target(instance, entity)?;
    let path = (!path.is_empty()).then_some(path);
    let at = ListAt {
        instance: Some(id),
        name: "overrides",
    };
    Ok(remove_where(file, at, &targets(&entity, component, path))?.0)
}

/// Gives `entity` of `instance` the component `component` with `value`. An `added` entry
/// only ever holds a component the prefab lacks: a component this instance removed is a
/// prefab one, so its `removed` entry is dropped and `value` becomes a whole-component
/// override (`set_override` with an empty path) instead.
pub fn add_component_override(
    file: &str,
    instance: &[u32],
    entity: &[u32],
    component: &str,
    value: &str,
) -> Result<String, SceneError> {
    check_value(value)?;
    let (id, entity) = split_target(instance, entity)?;
    let is = targets(&entity, component, None);
    let (text, removed) = remove_where(
        file,
        ListAt {
            instance: Some(id),
            name: "removed",
        },
        &is,
    )?;
    if removed > 0 {
        return set_override(&text, &[id], &entity, component, &[], value);
    }
    let entry = format!("(entity: {entity:?}, component: {component:?}, value: {value})");
    upsert(
        &text,
        ListAt {
            instance: Some(id),
            name: "added",
        },
        &is,
        value,
        &entry,
    )
}

/// Removes the component `component` from `entity` of `instance`, with its overrides
/// (whole or by field): an `added` component (one the prefab lacks, see
/// `add_component_override`) is taken back, a prefab component gets a `removed` entry.
pub fn remove_component_override(
    file: &str,
    instance: &[u32],
    entity: &[u32],
    component: &str,
) -> Result<String, SceneError> {
    let (id, entity) = split_target(instance, entity)?;
    let is = targets(&entity, component, None);
    let at = |name| ListAt {
        instance: Some(id),
        name,
    };
    let (text, _) = remove_where(file, at("overrides"), &is)?;
    // An added component is one the prefab lacks: the session overrides existing ones.
    let (text, added) = remove_where(&text, at("added"), &is)?;
    if added > 0 {
        return Ok(text);
    }
    let (text, _) = remove_where(&text, at("removed"), &is)?;
    let entry = format!("(entity: {entity:?}, component: {component:?})");
    append(&text, at("removed"), &entry)
}

/// Writes `value` at `path` of `component` into the prefab file: on its own entity when
/// `entity` has one element, else as an override of its nested instance `entity[0]`.
pub fn apply_to_prefab(
    prefab_file: &str,
    entity: &[u32],
    component: &str,
    path: &[PathSegment],
    value: &str,
) -> Result<String, SceneError> {
    check_value(value)?;
    let local = match entity {
        [local] => *local,
        [instance, rest @ ..] => {
            return set_override(prefab_file, &[*instance], rest, component, path, value);
        }
        [] => {
            return Err(SceneError::MissingTarget(
                "an empty entity address".to_string(),
            ));
        }
    };
    let missing = || {
        SceneError::MissingTarget(format!(
            "component `{component}` at `{}` of entity {local}",
            Path(path)
        ))
    };
    let root = parse_file(prefab_file)?;
    let (_, entities) = list(prefab_file, &root, ROOT_ENTITIES)?;
    let entry = entities
        .map_or(&[][..], items)
        .iter()
        .find(|entry| id_of(prefab_file, entry) == Some(local))
        .ok_or_else(missing)?;
    let Some(Node {
        kind: Kind::Map(pairs),
        ..
    }) = field(prefab_file, entry, "components")
    else {
        return Err(missing());
    };
    let key = format!("{component:?}");
    let (_, current) = pairs
        .iter()
        .find(|(name, _)| prefab_file[name.span.clone()] == key)
        .ok_or_else(missing)?;
    let target = locate(prefab_file, current, path).ok_or_else(missing)?;
    Ok(splice(prefab_file, target.span.clone(), value))
}

/// Checks the prefab `id` before `file` is written as its text: refuses an `Entity` value
/// of its own entities naming none of them, and an instance cycle, whose chain starts at
/// `id`. `file` replaces whatever `assets` holds for `id`; a new prefab passes the
/// identifier its meta will get.
pub fn save_prefab_checked(
    id: AssetId,
    file: &str,
    assets: &dyn AssetSource,
) -> Result<(), SceneError> {
    let parse = |error: ron::error::SpannedError| SceneError::Parse(error.to_string());
    let header: PrefabHeader = ron::from_str(file).map_err(parse)?;
    if header.version != PREFAB_VERSION {
        return Err(SceneError::Parse(format!(
            "prefab version {} is not supported (expected {PREFAB_VERSION})",
            header.version
        )));
    }
    let prefab: PrefabFile = ron::from_str(file).map_err(parse)?;
    let locals: BTreeSet<u32> = prefab.entities.iter().map(|entity| entity.id).collect();
    for entity in &prefab.entities {
        for (name, value) in &entity.components.0 {
            let text = value.get_ron();
            let node = parse_file(text)?;
            if let Some(path) = foreign(text, &node, &locals, &mut Vec::new()) {
                return Err(SceneError::ForeignReference {
                    entity: entity.id,
                    component: name.clone(),
                    path: Path(&path).to_string(),
                });
            }
        }
    }
    let overlay = Overlay { id, file, assets };
    Resolver::new(&overlay).prefab(id, &[], &mut Vec::new())
}

/// `assets` with the text of `id` replaced by `file`.
struct Overlay<'a> {
    id: AssetId,
    file: &'a str,
    assets: &'a dyn AssetSource,
}

impl AssetSource for Overlay<'_> {
    fn text(&self, id: AssetId) -> Option<&str> {
        if id == self.id {
            Some(self.file)
        } else {
            self.assets.text(id)
        }
    }

    fn path(&self, id: AssetId) -> Option<&str> {
        self.assets.path(id)
    }
}

/// The entities and instances of a prefab file, as written.
#[derive(Deserialize)]
#[serde(rename = "Prefab")]
struct PrefabParts<'a> {
    next_id: u32,
    #[serde(borrow)]
    entities: &'a RawValue,
    #[serde(borrow, default, deserialize_with = "present")]
    instances: Option<&'a RawValue>,
}

/// The entities and instances of a version 2 scene file, as written.
#[derive(Deserialize)]
#[serde(rename = "Scene")]
struct SceneParts<'a> {
    next_id: u32,
    #[serde(borrow)]
    entities: &'a RawValue,
    #[serde(borrow, default, deserialize_with = "present")]
    instances: Option<&'a RawValue>,
}

/// A field that may be left out, as written when present.
fn present<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<&'de RawValue>, D::Error> {
    <&RawValue>::deserialize(deserializer).map(Some)
}

/// A prefab file as a version 2 scene without resources, so a simulation loads it like a
/// scene. Entities and instances are copied as written.
pub fn prefab_as_scene(prefab_file: &str) -> Result<String, SceneError> {
    let parse = |error: ron::error::SpannedError| SceneError::Parse(error.to_string());
    let header: PrefabHeader = ron::from_str(prefab_file).map_err(parse)?;
    if header.version != PREFAB_VERSION {
        return Err(SceneError::Parse(format!(
            "prefab version {} is not supported (expected {PREFAB_VERSION})",
            header.version
        )));
    }
    let file: PrefabParts = ron::from_str(prefab_file).map_err(parse)?;
    let instances = file.instances.map_or("[]", RawValue::get_ron);
    Ok(format!(
        "Scene(\n    version: 2,\n    next_id: {},\n    resources: {{}},\n    entities: {},\n    instances: {instances},\n)\n",
        file.next_id,
        file.entities.get_ron()
    ))
}

/// The prefab file of a version 2 scene, its resources left out: the reverse of
/// `prefab_as_scene`.
pub fn scene_as_prefab(scene: &str) -> Result<String, SceneError> {
    let file: SceneParts =
        ron::from_str(scene).map_err(|error| SceneError::Parse(error.to_string()))?;
    let instances = file.instances.map_or("[]", RawValue::get_ron);
    Ok(format!(
        "Prefab(\n    version: {PREFAB_VERSION},\n    next_id: {},\n    entities: {},\n    instances: {instances},\n)\n",
        file.next_id,
        file.entities.get_ron()
    ))
}

/// The RON text of the value at `path` inside the RON value `text`, as written.
pub fn ron_at_path(text: &str, path: &[PathSegment]) -> Option<String> {
    let root = parse_file(text).ok()?;
    let node = locate(text, &root, path)?;
    Some(text[node.span.clone()].to_string())
}

/// Refuses despawning an entity a prefab instance spawned (a load address of more than one
/// element): at tick 0 its components are overridden instead, and the instance is removed
/// as a whole.
pub fn check_despawn(address: &[u32]) -> Result<(), SceneError> {
    match address {
        [_, _, ..] => Err(SceneError::InstanceEntity(address.to_vec())),
        _ => Ok(()),
    }
}

/// The path, without schema, to the first `(index: n, generation: _)` value in `node` whose
/// `n` is not in `locals`.
fn foreign(
    text: &str,
    node: &Node,
    locals: &BTreeSet<u32>,
    path: &mut Vec<PathSegment>,
) -> Option<Vec<PathSegment>> {
    let inside = |segment: PathSegment, node: &Node, path: &mut Vec<PathSegment>| {
        path.push(segment);
        let found = foreign(text, node, locals, path);
        path.pop();
        found
    };
    match &node.kind {
        Kind::Fields(fields) => {
            if let [(index, value), (generation, _)] = &fields[..]
                && text[index.clone()] == *"index"
                && text[generation.clone()] == *"generation"
            {
                let local = text[value.span.clone()].parse::<u32>().ok()?;
                return (!locals.contains(&local)).then(|| path.clone());
            }
            fields.iter().find_map(|(key, value)| {
                let name = text[key.clone()].to_string();
                inside(PathSegment::Field { name }, value, path)
            })
        }
        Kind::Items(items) if items.len() == 1 => foreign(text, &items[0], locals, path),
        Kind::Items(items) | Kind::List(items) => (0..)
            .zip(items)
            .find_map(|(index, item)| inside(PathSegment::Index { index }, item, path)),
        Kind::Map(pairs) => (0..)
            .zip(pairs)
            .find_map(|(index, (_, value))| inside(PathSegment::Index { index }, value, path)),
        Kind::Named { name, body } if text[name.clone()] == *"Some" => {
            foreign(text, body, locals, path)
        }
        Kind::Named { name, body } => {
            let name = text[name.clone()].to_string();
            inside(PathSegment::Field { name }, body, path)
        }
        Kind::Atom => None,
    }
}

/// Refuses a `value` that is not exactly one RON value, before it is spliced into a file.
fn check_value(value: &str) -> Result<(), SceneError> {
    ron::from_str::<serde::de::IgnoredAny>(value)
        .map(drop)
        .map_err(|error| {
            SceneError::Parse(format!("the value `{value}` is not one RON value: {error}"))
        })
}

/// The identifier of an instance entry and the entity path inside it.
fn split_target(instance: &[u32], entity: &[u32]) -> Result<(u32, Vec<u32>), SceneError> {
    let (&id, nested) = instance
        .split_first()
        .ok_or_else(|| SceneError::MissingTarget("an empty instance address".to_string()))?;
    Ok((id, [nested, entity].concat()))
}

/// The `entity`, `component`, and `path` of an override, added, or removed entry.
#[derive(Deserialize)]
struct Target {
    entity: Vec<u32>,
    component: String,
    #[serde(default)]
    path: Vec<PathSegment>,
}

/// Whether an entry's text targets `entity` and `component`, and `path` when given.
fn targets<'a>(
    entity: &'a [u32],
    component: &'a str,
    path: Option<&'a [PathSegment]>,
) -> impl Fn(&str, &Node) -> bool + 'a {
    move |text, entry| {
        ron::from_str::<Target>(&text[entry.span.clone()]).is_ok_and(|target| {
            target.entity == entity
                && target.component == component
                && path.is_none_or(|path| target.path == path)
        })
    }
}

/// A list field of a file: on its root, or on its instance entry `instance`.
#[derive(Clone, Copy)]
struct ListAt<'a> {
    instance: Option<u32>,
    name: &'a str,
}

const ROOT_ENTITIES: ListAt = ListAt {
    instance: None,
    name: "entities",
};
const ROOT_INSTANCES: ListAt = ListAt {
    instance: None,
    name: "instances",
};

fn parse_file(text: &str) -> Result<Node, SceneError> {
    Parser { text, at: 0 }
        .node()
        .ok_or_else(|| SceneError::Parse("the file is not valid RON".to_string()))
}

/// The node holding the list `at` names, and the list when the field is there.
fn list<'n>(
    text: &str,
    root: &'n Node,
    at: ListAt,
) -> Result<(&'n Node, Option<&'n Node>), SceneError> {
    let owner = match at.instance {
        None => root,
        Some(id) => field(text, root, "instances")
            .map_or(&[][..], items)
            .iter()
            .find(|entry| id_of(text, entry) == Some(id))
            .ok_or_else(|| SceneError::MissingTarget(format!("instance {id}")))?,
    };
    Ok((owner, field(text, owner, at.name)))
}

/// Appends `entry` to the list `at`, adding the field when it is missing.
fn append(text: &str, at: ListAt, entry: &str) -> Result<String, SceneError> {
    let root = parse_file(text)?;
    Ok(match list(text, &root, at)? {
        (_, Some(list)) => insert(
            text,
            list,
            items(list).last().map(|e| e.span.clone()),
            entry,
        ),
        (owner, None) => {
            let last = fields_of(owner)
                .last()
                .map(|(key, value)| key.start..value.span.end);
            insert(text, owner, last, &format!("{}: [{entry}]", at.name))
        }
    })
}

/// Replaces the `value` of the first entry of the list `at` that `matches`, or appends
/// `entry` when none does.
fn upsert(
    text: &str,
    at: ListAt,
    matches: &dyn Fn(&str, &Node) -> bool,
    value: &str,
    entry: &str,
) -> Result<String, SceneError> {
    let root = parse_file(text)?;
    let (_, found) = list(text, &root, at)?;
    let existing = found
        .map_or(&[][..], items)
        .iter()
        .find(|e| matches(text, e))
        .and_then(|e| field(text, e, "value"));
    match existing {
        Some(current) => Ok(splice(text, current.span.clone(), value)),
        None => append(text, at, entry),
    }
}

/// Removes every entry of the list `at` that `matches`, and counts them; an emptied list
/// becomes `[]`.
fn remove_where(
    text: &str,
    at: ListAt,
    matches: &dyn Fn(&str, &Node) -> bool,
) -> Result<(String, usize), SceneError> {
    let mut text = text.to_string();
    let mut removed = 0;
    loop {
        let root = parse_file(&text)?;
        let Some(list) = list(&text, &root, at)?.1 else {
            return Ok((text, removed));
        };
        let all = items(list);
        let Some(entry) = all.iter().find(|e| matches(&text, e)) else {
            return Ok((text, removed));
        };
        let range = if all.len() == 1 {
            list.span.clone()
        } else {
            element_range(&text, entry.span.clone())
        };
        let with = if all.len() == 1 { "[]" } else { "" };
        text = splice(&text, range, with);
        removed += 1;
    }
}

/// Inserts `element` into the sequence `container` after its `last` element, on a line of
/// its own when the elements are; `None` for an empty list.
fn insert(text: &str, container: &Node, last: Option<Range<usize>>, element: &str) -> String {
    let Some(last) = last else {
        let indent = line_indent(text, container.span.start);
        let inner = container.span.start + 1..container.span.end - 1;
        return splice(text, inner, &format!("\n{indent}    {element},\n{indent}"));
    };
    let separator = if starts_line(text, last.start) {
        format!("\n{}", line_indent(text, last.start))
    } else {
        " ".to_string()
    };
    let rest = &text[last.end..];
    let gap = rest.len() - rest.trim_start().len();
    if rest[gap..].starts_with(',') {
        let at = last.end + gap + 1;
        splice(text, at..at, &format!("{separator}{element},"))
    } else {
        splice(text, last.end..last.end, &format!(",{separator}{element}"))
    }
}

/// The range to remove for the element at `span` of a list of several: the element, the
/// whitespace before it, and its comma.
fn element_range(text: &str, span: Range<usize>) -> Range<usize> {
    let start = text[..span.start].trim_end().len();
    let rest = &text[span.end..];
    let gap = rest.len() - rest.trim_start().len();
    if rest[gap..].starts_with(',') {
        start..span.end + gap + 1
    } else if text[..start].ends_with(',') {
        start - 1..span.end
    } else {
        start..span.end
    }
}

fn line_indent(text: &str, at: usize) -> &str {
    let start = text[..at].rfind('\n').map_or(0, |newline| newline + 1);
    let line = &text[start..];
    &line[..line.len() - line.trim_start_matches([' ', '\t']).len()]
}

fn starts_line(text: &str, at: usize) -> bool {
    text[..at]
        .rsplit('\n')
        .next()
        .is_some_and(|before| before.trim().is_empty())
}

fn splice(text: &str, range: Range<usize>, with: &str) -> String {
    format!("{}{with}{}", &text[..range.start], &text[range.end..])
}

/// The fields of a `Name(...)` or `(...)` node.
fn fields_of(node: &Node) -> &[(Range<usize>, Node)] {
    match &node.kind {
        Kind::Named { body, .. } => fields_of(body),
        Kind::Fields(fields) => fields,
        _ => &[],
    }
}

fn field<'n>(text: &str, node: &'n Node, name: &str) -> Option<&'n Node> {
    fields_of(node)
        .iter()
        .find(|(key, _)| text[key.clone()] == *name)
        .map(|(_, value)| value)
}

fn items(node: &Node) -> &[Node] {
    match &node.kind {
        Kind::List(items) => items,
        _ => &[],
    }
}

fn id_of(text: &str, entry: &Node) -> Option<u32> {
    field(text, entry, "id").and_then(|id| text[id.span.clone()].parse().ok())
}

/// Replaces the value at `path` (serde's externally tagged form, as `SetField` uses) in the
/// RON `text` with `value`, keeping the rest of the text; `None` when the path does not
/// exist. Without a schema, a one-element `(x)` is read as a newtype (transparent) unless
/// an index into it only works as a tuple.
fn set_at_ron_path(text: &str, path: &[PathSegment], value: &str) -> Option<String> {
    let mut parser = Parser { text, at: 0 };
    let root = parser.node()?;
    let span = locate(text, &root, path)?.span.clone();
    Some(format!(
        "{}{value}{}",
        &text[..span.start],
        &text[span.end..]
    ))
}

/// A RON value with the byte range it spans.
struct Node {
    span: Range<usize>,
    kind: Kind,
}

enum Kind {
    /// A number, string, character, boolean, unit variant, or `None`.
    Atom,
    List(Vec<Node>),
    Map(Vec<(Node, Node)>),
    /// `(a: 1, b: 2)`.
    Fields(Vec<(Range<usize>, Node)>),
    /// `(1, 2)`, `(x)`, or `()`.
    Items(Vec<Node>),
    /// `Name(...)`: a struct, a variant with content, or `Some`.
    Named {
        name: Range<usize>,
        body: Box<Node>,
    },
}

fn locate<'n>(text: &str, node: &'n Node, path: &[PathSegment]) -> Option<&'n Node> {
    let Some((segment, rest)) = path.split_first() else {
        return Some(node);
    };
    match (&node.kind, segment) {
        (Kind::Named { name, body }, PathSegment::Field { name: field })
            if text[name.clone()] == *field =>
        {
            match &body.kind {
                Kind::Items(items) if items.len() == 1 => locate(text, &items[0], rest),
                _ => locate(text, body, rest),
            }
        }
        (Kind::Named { body, .. }, _) => locate(text, body, path),
        (Kind::Items(items), _) if items.len() == 1 => {
            locate(text, &items[0], path).or_else(|| match segment {
                PathSegment::Index { index: 0 } => locate(text, &items[0], rest),
                _ => None,
            })
        }
        (Kind::Items(items) | Kind::List(items), PathSegment::Index { index }) => {
            locate(text, items.get(*index as usize)?, rest)
        }
        (Kind::Fields(fields), PathSegment::Field { name }) => {
            let (_, value) = fields.iter().find(|(key, _)| text[key.clone()] == *name)?;
            locate(text, value, rest)
        }
        (Kind::Map(pairs), PathSegment::Field { name }) => {
            let (_, value) = pairs.iter().find(|(key, _)| {
                let key = &text[key.span.clone()];
                key == name || key.strip_prefix('"').and_then(|k| k.strip_suffix('"')) == Some(name)
            })?;
            locate(text, value, rest)
        }
        _ => None,
    }
}

/// Reads RON values with their spans; `None` on malformed text.
struct Parser<'a> {
    text: &'a str,
    at: usize,
}

impl<'a> Parser<'a> {
    fn peek(&self) -> Option<u8> {
        self.text.as_bytes().get(self.at).copied()
    }

    fn rest(&self) -> &'a str {
        &self.text[self.at..]
    }

    /// Skips whitespace and comments.
    fn skip(&mut self) {
        loop {
            let rest = self.rest();
            let trimmed = rest.trim_start();
            self.at += rest.len() - trimmed.len();
            if trimmed.starts_with("//") {
                self.at += trimmed.find('\n').unwrap_or(trimmed.len());
            } else if trimmed.starts_with("/*") {
                let mut depth = 0;
                while self.at < self.text.len() {
                    if self.rest().starts_with("/*") {
                        depth += 1;
                        self.at += 2;
                    } else if self.rest().starts_with("*/") {
                        depth -= 1;
                        self.at += 2;
                        if depth == 0 {
                            break;
                        }
                    } else {
                        self.at += self.rest().chars().next().map_or(1, char::len_utf8);
                    }
                }
            } else {
                return;
            }
        }
    }

    fn eat(&mut self, byte: u8) -> bool {
        self.skip();
        let found = self.peek() == Some(byte);
        if found {
            self.at += 1;
        }
        found
    }

    fn node(&mut self) -> Option<Node> {
        self.skip();
        let start = self.at;
        let kind = match self.peek()? {
            b'[' => {
                self.at += 1;
                Kind::List(self.sequence(b']', Self::node)?)
            }
            b'{' => {
                self.at += 1;
                Kind::Map(self.sequence(b'}', |parser| {
                    let key = parser.node()?;
                    parser.eat(b':').then_some(())?;
                    Some((key, parser.node()?))
                })?)
            }
            b'(' => return self.group(),
            b'"' => {
                self.string()?;
                Kind::Atom
            }
            b'\'' => {
                self.at += 1;
                while self.peek()? != b'\'' {
                    self.char_at();
                }
                self.at += 1;
                Kind::Atom
            }
            _ => {
                if let Some((open, hashes)) = raw_string_open(self.rest()) {
                    let close = format!("\"{}", "#".repeat(hashes));
                    self.at += open + self.text[self.at + open..].find(&close)? + close.len();
                    return Some(Node {
                        span: start..self.at,
                        kind: Kind::Atom,
                    });
                }
                if self.rest().starts_with("b\"") {
                    self.at += 1;
                    self.string()?;
                    return Some(Node {
                        span: start..self.at,
                        kind: Kind::Atom,
                    });
                }
                let word = self.word();
                if word.is_empty() {
                    return None;
                }
                let end = self.at;
                self.skip();
                if self.peek() == Some(b'(') && is_identifier(&self.text[word.clone()]) {
                    let body = self.group()?;
                    return Some(Node {
                        span: start..body.span.end,
                        kind: Kind::Named {
                            name: word,
                            body: Box::new(body),
                        },
                    });
                }
                self.at = end;
                Kind::Atom
            }
        };
        Some(Node {
            span: start..self.at,
            kind,
        })
    }

    /// `(...)`: named fields or positional items.
    fn group(&mut self) -> Option<Node> {
        self.skip();
        let start = self.at;
        self.at += 1;
        let before = self.at;
        self.skip();
        let word = self.word();
        let named = is_identifier(&self.text[word]) && {
            self.skip();
            self.peek() == Some(b':')
        };
        self.at = before;
        let kind = if named {
            Kind::Fields(self.sequence(b')', |parser| {
                parser.skip();
                let key = parser.word();
                parser.eat(b':').then_some(())?;
                Some((key, parser.node()?))
            })?)
        } else {
            Kind::Items(self.sequence(b')', Self::node)?)
        };
        Some(Node {
            span: start..self.at,
            kind,
        })
    }

    /// Comma-separated elements up to `close` (a trailing comma allowed), after the opener.
    fn sequence<T>(
        &mut self,
        close: u8,
        mut element: impl FnMut(&mut Self) -> Option<T>,
    ) -> Option<Vec<T>> {
        let mut elements = Vec::new();
        loop {
            if self.eat(close) {
                return Some(elements);
            }
            elements.push(element(self)?);
            if !self.eat(b',') {
                return self.eat(close).then_some(elements);
            }
        }
    }

    fn string(&mut self) -> Option<()> {
        self.at += 1;
        loop {
            match self.peek()? {
                b'"' => {
                    self.at += 1;
                    return Some(());
                }
                _ => self.char_at(),
            }
        }
    }

    /// Steps over the character at `at`, and the one it escapes, whole.
    fn char_at(&mut self) {
        let escaped = self.peek() == Some(b'\\');
        for _ in 0..=usize::from(escaped) {
            self.at += self.rest().chars().next().map_or(1, char::len_utf8);
        }
    }

    /// An identifier, number, or other bare token.
    fn word(&mut self) -> Range<usize> {
        let start = self.at;
        let length = self
            .rest()
            .find(|c: char| c.is_whitespace() || ",:()[]{}\"/".contains(c))
            .unwrap_or(self.rest().len());
        self.at += length;
        start..self.at
    }
}

/// The length of the opening of a raw string starting `text` (such as `r#"`) and its
/// number of `#`.
fn raw_string_open(text: &str) -> Option<(usize, usize)> {
    let after = text.strip_prefix("br").or_else(|| text.strip_prefix('r'))?;
    let hashes = after.len() - after.trim_start_matches('#').len();
    let prefix = text.len() - after.len();
    after[hashes..]
        .starts_with('"')
        .then_some((prefix + hashes + 1, hashes))
}

fn is_identifier(word: &str) -> bool {
    let word = word.strip_prefix("r#").unwrap_or(word);
    word.chars()
        .next()
        .is_some_and(|c| c.is_alphabetic() || c == '_')
        && word.chars().all(|c| c.is_alphanumeric() || c == '_')
}

#[cfg(test)]
mod tests {
    use super::*;

    fn field(name: &str) -> PathSegment {
        PathSegment::Field {
            name: name.to_string(),
        }
    }

    #[test]
    fn sets_values_by_path_keeping_the_rest() {
        let text = "Body(kind: Static, shape: Polygon((vertices: [(x: 1.0), (x: 2.0)])), tag: Some(3), pair: (1, \"a,)\"), map: {\"k\": 1}) // done";
        let set = |path: &[PathSegment]| set_at_ron_path(text, path, "9").unwrap();
        assert!(set(&[field("kind")]).contains("kind: 9,"));
        let vertex = [
            field("shape"),
            field("Polygon"),
            field("vertices"),
            PathSegment::Index { index: 1 },
            field("x"),
        ];
        assert!(set(&vertex).contains("(x: 1.0), (x: 9)"));
        assert!(set(&[field("tag")]).contains("tag: 9,"));
        assert!(set(&[field("pair"), PathSegment::Index { index: 1 }]).contains("(1, 9)"));
        assert!(set(&[field("map"), field("k")]).contains("{\"k\": 9}"));
        assert_eq!(set(&[]), "9 // done");
        assert!(set_at_ron_path(text, &[field("missing")], "9").is_none());
        assert!(
            set_at_ron_path("(r#\"a)\"#, 2)", &[PathSegment::Index { index: 1 }], "3")
                .is_some_and(|text| text == "(r#\"a)\"#, 3)")
        );
        assert!(
            set_at_ron_path("(x: 1.0 /* café */, y: 2.0)", &[field("y")], "4.0")
                .is_some_and(|text| text == "(x: 1.0 /* café */, y: 4.0)")
        );
        assert!(
            set_at_ron_path("((5))", &[PathSegment::Index { index: 0 }], "6")
                .is_some_and(|text| text == "((6))")
        );
    }
}
