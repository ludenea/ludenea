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
}

impl<'a> Resolver<'a> {
    fn new(assets: &'a dyn AssetSource) -> Self {
        Self {
            assets,
            stack: Vec::new(),
            warnings: Vec::new(),
            instances: Vec::new(),
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
                    self.at += if self.peek()? == b'\\' { 2 } else { 1 };
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
                b'\\' => self.at += 2,
                b'"' => {
                    self.at += 1;
                    return Some(());
                }
                _ => self.at += 1,
            }
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
