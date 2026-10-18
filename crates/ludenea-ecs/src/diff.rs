//! Field-level differences between two snapshots of the same world.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use ludenea_schema::{TypeSchema, VariantFields};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::hash::ENTITIES_LEAF;
use crate::snapshot::{ComponentValues, Snapshot, SnapshotError};
use crate::{AssetId, Codec, Entity, World};

const ABSENT: &str = "absent";

/// One differing value: `subject` is `entity 3v0`, `resource`, `entities`, or `asset <id>`,
/// and `path` locates the field inside the component (empty when the whole value differs):
/// field names, and `[i]` for items, tuple positions, newtype contents, and map keys.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct Difference {
    pub subject: String,
    pub component: String,
    pub path: Vec<String>,
    /// The value as JSON (a RON string when it has no JSON form); `None` when absent. A
    /// variant change gives the variant names.
    pub left: Option<Value>,
    pub right: Option<Value>,
    /// The value as RON, or `absent`.
    pub left_text: String,
    pub right_text: String,
}

impl fmt::Display for Difference {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{} {} ", self.subject, self.component)?;
        for segment in &self.path {
            if segment.starts_with('[') {
                write!(formatter, "{segment}")?;
            } else {
                write!(formatter, ".{segment}")?;
            }
        }
        write!(formatter, ": {} != {}", self.left_text, self.right_text)
    }
}

/// One side of a compared value: its JSON, whether the JSON is its real form (a value with
/// non-finite floats has none and is not walked), and its RON.
struct Side {
    json: Value,
    walkable: bool,
    ron: String,
}

impl World {
    /// Lists every difference between `a` and `b`, both of which must match this world's
    /// registry: the entity allocator first, then resources and components by name, then
    /// assets by identifier. Values are walked as JSON guided by their schema, so a variant
    /// change reports the two variant names. Snapshots of two known, different schema
    /// fingerprints are refused.
    pub fn diff_snapshots(
        &self,
        a: &Snapshot,
        b: &Snapshot,
    ) -> Result<Vec<Difference>, SnapshotError> {
        if a.fingerprint != 0 && b.fingerprint != 0 && a.fingerprint != b.fingerprint {
            return Err(SnapshotError::Fingerprint {
                left: a.fingerprint,
                right: b.fingerprint,
            });
        }
        self.check_compatible(a)?;
        self.check_compatible(b)?;
        let mut differences = Vec::new();

        if a.entities != b.entities {
            let side = |entities| Side {
                json: serde_json::to_value(entities).expect("entities always encode"),
                walkable: true,
                ron: ron::to_string(entities).expect("entities always encode"),
            };
            let (left, right) = (side(&a.entities), side(&b.entities));
            let mut found = Found::new("entities", ENTITIES_LEAF, &mut differences);
            found.compare(None, Some(&left), Some(&right));
        }

        let right: BTreeMap<&str, &Vec<u8>> = b
            .resources
            .iter()
            .map(|(name, bytes)| (name.as_str(), bytes))
            .collect();
        for (name, left) in &a.resources {
            let right = right[name.as_str()];
            if left != right {
                let codec = self.resource_codec(name).expect("checked");
                let (left, right) = (side(codec, name, left)?, side(codec, name, right)?);
                let mut found = Found::new("resource", name, &mut differences);
                found.compare(Some(&(codec.schema)()), Some(&left), Some(&right));
            }
        }

        let right: BTreeMap<&str, &ComponentValues> = b
            .components
            .iter()
            .map(|(name, values)| (name.as_str(), values))
            .collect();
        for (name, left) in &a.components {
            let codec = self.component_codec(name).expect("checked");
            let schema = (codec.schema)();
            let left: BTreeMap<Entity, &Vec<u8>> = left.iter().map(|(e, v)| (*e, v)).collect();
            let right: BTreeMap<Entity, &Vec<u8>> =
                right[name.as_str()].iter().map(|(e, v)| (*e, v)).collect();
            let entities: BTreeSet<Entity> = left.keys().chain(right.keys()).copied().collect();
            for entity in entities {
                let (l, r) = (left.get(&entity), right.get(&entity));
                if l.is_some() && l == r {
                    continue;
                }
                let l = l.map(|bytes| side(codec, name, bytes)).transpose()?;
                let r = r.map(|bytes| side(codec, name, bytes)).transpose()?;
                let subject = format!("entity {entity}");
                let mut found = Found::new(&subject, name, &mut differences);
                found.compare(Some(&schema), l.as_ref(), r.as_ref());
            }
        }

        let assets = |snapshot: &'_ Snapshot| -> BTreeMap<AssetId, (String, Vec<u8>)> {
            snapshot
                .assets
                .iter()
                .map(|(id, type_name, bytes)| (*id, (type_name.clone(), bytes.clone())))
                .collect()
        };
        let (left, right) = (assets(a), assets(b));
        let ids: BTreeSet<AssetId> = left.keys().chain(right.keys()).copied().collect();
        for id in ids {
            let (l, r) = (left.get(&id), right.get(&id));
            if l.is_some() && l == r {
                continue;
            }
            let asset_side = |asset: Option<&(String, Vec<u8>)>| {
                asset
                    .map(|(type_name, bytes)| {
                        let codec =
                            self.asset_codec(type_name)
                                .ok_or_else(|| SnapshotError::Decode {
                                    name: format!("asset {id}"),
                                    reason: format!("asset type `{type_name}` is not registered"),
                                })?;
                        Ok((type_name.clone(), codec, side(codec, type_name, bytes)?))
                    })
                    .transpose()
            };
            let (l, r) = (asset_side(l)?, asset_side(r)?);
            let (type_name, codec) = match (&l, &r) {
                (Some((type_name, codec, _)), _) | (None, Some((type_name, codec, _))) => {
                    (type_name.clone(), *codec)
                }
                (None, None) => unreachable!("the identifier comes from one side"),
            };
            let same_type = l.as_ref().map(|l| &l.0) == r.as_ref().map(|r| &r.0);
            let schema = same_type.then(|| (codec.schema)());
            let subject = format!("asset {id}");
            let mut found = Found::new(&subject, &type_name, &mut differences);
            let (l, r) = (l.map(|l| l.2), r.map(|r| r.2));
            match schema {
                Some(schema) => found.compare(Some(&schema), l.as_ref(), r.as_ref()),
                None => found.whole(l.as_ref(), r.as_ref()),
            }
        }
        Ok(differences)
    }
}

fn side(codec: Codec, name: &str, bytes: &[u8]) -> Result<Side, SnapshotError> {
    let value = (codec.decode)(bytes).map_err(|reason| SnapshotError::Decode {
        name: name.to_string(),
        reason,
    })?;
    let ron = (codec.encode_ron)(&*value);
    Ok(match (codec.to_json)(&*value) {
        Ok(json) => Side {
            json,
            walkable: true,
            ron,
        },
        Err(_) => Side {
            json: Value::String(ron.clone()),
            walkable: false,
            ron,
        },
    })
}

/// The differences of one subject and component, appended to `out`.
struct Found<'a> {
    subject: &'a str,
    component: &'a str,
    out: &'a mut Vec<Difference>,
}

impl<'a> Found<'a> {
    fn new(subject: &'a str, component: &'a str, out: &'a mut Vec<Difference>) -> Self {
        Found {
            subject,
            component,
            out,
        }
    }

    /// Walks two values; when the walk finds nothing (bytes that differ in a way JSON does
    /// not show, such as `-0.0` and `0.0`) the whole values are reported instead.
    fn compare(&mut self, schema: Option<&TypeSchema>, left: Option<&Side>, right: Option<&Side>) {
        let found = self.out.len();
        if let (Some(l), Some(r)) = (left, right)
            && l.walkable
            && r.walkable
        {
            self.walk(schema, &mut Vec::new(), Some(&l.json), Some(&r.json));
        }
        if self.out.len() == found {
            self.whole(left, right);
        }
    }

    fn whole(&mut self, left: Option<&Side>, right: Option<&Side>) {
        let text = |side: Option<&Side>| side.map_or(ABSENT.to_string(), |side| side.ron.clone());
        self.push(
            Vec::new(),
            left.map(|side| side.json.clone()),
            right.map(|side| side.json.clone()),
            text(left),
            text(right),
        );
    }

    fn push(
        &mut self,
        path: Vec<String>,
        left: Option<Value>,
        right: Option<Value>,
        left_text: String,
        right_text: String,
    ) {
        self.out.push(Difference {
            subject: self.subject.to_string(),
            component: self.component.to_string(),
            path,
            left,
            right,
            left_text,
            right_text,
        });
    }

    fn walk(
        &mut self,
        schema: Option<&TypeSchema>,
        path: &mut Vec<String>,
        left: Option<&Value>,
        right: Option<&Value>,
    ) {
        let (l, r) = match (left, right) {
            (Some(l), Some(r)) if l == r => return,
            (Some(l), Some(r)) => (l, r),
            _ => return self.leaf(schema, path, left, right),
        };
        match (schema, l, r) {
            (Some(TypeSchema::Option { item }), l, r) if !l.is_null() && !r.is_null() => {
                self.walk(Some(item), path, Some(l), Some(r));
            }
            (
                Some(TypeSchema::List { item } | TypeSchema::Array { item, .. }),
                Value::Array(l),
                Value::Array(r),
            ) => {
                for index in 0..l.len().max(r.len()) {
                    self.at(path, format!("[{index}]"), |found, path| {
                        found.walk(Some(item), path, l.get(index), r.get(index));
                    });
                }
            }
            (
                Some(TypeSchema::Tuple { items } | TypeSchema::TupleStruct { items, .. }),
                Value::Array(l),
                Value::Array(r),
            ) => self.items(items, path, l, r),
            (Some(TypeSchema::Struct { fields, .. }), Value::Object(l), Value::Object(r)) => {
                self.fields(fields, path, l, r);
            }
            (Some(TypeSchema::Newtype { item, .. }), l, r) => {
                self.at(path, "[0]".to_string(), |found, path| {
                    found.walk(Some(item), path, Some(l), Some(r));
                });
            }
            (Some(TypeSchema::Map { key, value }), Value::Object(l), Value::Object(r)) => {
                let keys: BTreeSet<&String> = l.keys().chain(r.keys()).collect();
                for name in keys {
                    // Milestone 1 paths: `.name` for a string key, `[key]` otherwise.
                    let segment = match **key {
                        TypeSchema::String => name.clone(),
                        _ => format!("[{name}]"),
                    };
                    self.at(path, segment, |found, path| {
                        found.walk(Some(value), path, l.get(name), r.get(name));
                    });
                }
            }
            (Some(TypeSchema::Enum { variants, .. }), l, r) => {
                let ((l_name, l_content), (r_name, r_content)) = (variant(l), variant(r));
                let fields = variants
                    .iter()
                    .find(|v| v.name == l_name)
                    .map(|v| &v.fields);
                match fields {
                    _ if l_name != r_name => self.push(
                        path.clone(),
                        Some(Value::String(l_name.to_string())),
                        Some(Value::String(r_name.to_string())),
                        l_name.to_string(),
                        r_name.to_string(),
                    ),
                    Some(VariantFields::Newtype { item }) => {
                        self.at(path, "[0]".to_string(), |found, path| {
                            found.walk(Some(item), path, l_content, r_content);
                        });
                    }
                    Some(VariantFields::Tuple { items }) => match (l_content, r_content) {
                        (Some(Value::Array(l)), Some(Value::Array(r))) => {
                            self.items(items, path, l, r);
                        }
                        _ => self.leaf(schema, path, left, right),
                    },
                    Some(VariantFields::Struct { fields }) => match (l_content, r_content) {
                        (Some(Value::Object(l)), Some(Value::Object(r))) => {
                            self.fields(fields, path, l, r);
                        }
                        _ => self.leaf(schema, path, left, right),
                    },
                    _ => self.leaf(schema, path, left, right),
                }
            }
            // Without a schema (the entity allocator, an `Entity` value), objects and arrays
            // are walked as they are.
            (None | Some(TypeSchema::Entity), Value::Object(l), Value::Object(r)) => {
                let keys: BTreeSet<&String> = l.keys().chain(r.keys()).collect();
                for name in keys {
                    self.at(path, name.clone(), |found, path| {
                        found.walk(None, path, l.get(name), r.get(name));
                    });
                }
            }
            (None, Value::Array(l), Value::Array(r)) => {
                for index in 0..l.len().max(r.len()) {
                    self.at(path, format!("[{index}]"), |found, path| {
                        found.walk(None, path, l.get(index), r.get(index));
                    });
                }
            }
            _ => self.leaf(schema, path, left, right),
        }
    }

    fn items(&mut self, items: &[TypeSchema], path: &mut Vec<String>, l: &[Value], r: &[Value]) {
        for index in 0..l.len().max(r.len()) {
            self.at(path, format!("[{index}]"), |found, path| {
                found.walk(items.get(index), path, l.get(index), r.get(index));
            });
        }
    }

    /// Fields in name order, as milestone 1 listed them.
    fn fields(
        &mut self,
        fields: &[ludenea_schema::FieldSchema],
        path: &mut Vec<String>,
        l: &serde_json::Map<String, Value>,
        r: &serde_json::Map<String, Value>,
    ) {
        let mut sorted: Vec<_> = fields.iter().collect();
        sorted.sort_by(|a, b| a.name.cmp(&b.name));
        for field in sorted {
            self.at(path, field.name.clone(), |found, path| {
                found.walk(
                    Some(&field.schema),
                    path,
                    l.get(&field.name),
                    r.get(&field.name),
                );
            });
        }
    }

    fn leaf(
        &mut self,
        schema: Option<&TypeSchema>,
        path: &[String],
        left: Option<&Value>,
        right: Option<&Value>,
    ) {
        let text = |value: Option<&Value>| {
            value.map_or(ABSENT.to_string(), |value| {
                milestone_1(ron_text(schema, value))
            })
        };
        self.push(
            path.to_vec(),
            left.cloned(),
            right.cloned(),
            text(left),
            text(right),
        );
    }

    fn at(
        &mut self,
        path: &mut Vec<String>,
        segment: String,
        visit: impl FnOnce(&mut Self, &mut Vec<String>),
    ) {
        path.push(segment);
        visit(self, path);
        path.pop();
    }
}

/// The variant name and content of an externally tagged JSON enum value.
fn variant(value: &Value) -> (&str, Option<&Value>) {
    match value {
        Value::String(name) => (name, None),
        Value::Object(map) if map.len() == 1 => {
            let (name, content) = map.iter().next().expect("one entry");
            (name, Some(content))
        }
        _ => ("", None),
    }
}

/// RON text as milestone 1 rendered a part of a value: through `ron::Value`, so a struct
/// reads as a map with its fields in name order.
fn milestone_1(text: String) -> String {
    match ron::from_str::<ron::Value>(&text) {
        Ok(value) => ron::to_string(&value).expect("a RON value always encodes"),
        Err(_) => text,
    }
}

/// A JSON map key as RON: quoted when the key type is a string.
fn key_text(schema: &TypeSchema, key: &str) -> String {
    match schema {
        TypeSchema::String => ron::to_string(key).expect("a string always encodes"),
        _ => key.to_string(),
    }
}

/// The RON text of a JSON value, guided by its schema so floats, options, newtypes, and
/// enums read as RON writes them.
fn ron_text(schema: Option<&TypeSchema>, value: &Value) -> String {
    let join = |texts: Vec<String>| texts.join(",");
    let items = |items: &[TypeSchema], values: &[Value]| {
        join(
            values
                .iter()
                .enumerate()
                .map(|(index, value)| ron_text(items.get(index), value))
                .collect(),
        )
    };
    let fields = |fields: &[ludenea_schema::FieldSchema], map: &serde_json::Map<String, Value>| {
        join(
            fields
                .iter()
                .filter_map(|field| {
                    let value = map.get(&field.name)?;
                    Some(format!(
                        "{}:{}",
                        field.name,
                        ron_text(Some(&field.schema), value)
                    ))
                })
                .collect(),
        )
    };
    match (schema, value) {
        (Some(TypeSchema::F32), Value::Number(number)) => {
            let value = number.as_f64().unwrap_or_default() as f32;
            ron::to_string(&value).expect("a float always encodes")
        }
        (Some(TypeSchema::F64), Value::Number(number)) => {
            let value = number.as_f64().unwrap_or_default();
            ron::to_string(&value).expect("a float always encodes")
        }
        (Some(TypeSchema::Option { .. }), Value::Null) => "None".to_string(),
        (Some(TypeSchema::Option { item }), value) => {
            format!("Some({})", ron_text(Some(item), value))
        }
        (Some(TypeSchema::Unit | TypeSchema::UnitStruct { .. }), _) => "()".to_string(),
        (Some(TypeSchema::Newtype { item, .. }), value) => {
            format!("({})", ron_text(Some(item), value))
        }
        (
            Some(TypeSchema::List { item } | TypeSchema::Array { item, .. }),
            Value::Array(values),
        ) => {
            format!(
                "[{}]",
                join(
                    values
                        .iter()
                        .map(|value| ron_text(Some(item), value))
                        .collect()
                )
            )
        }
        (
            Some(
                TypeSchema::Tuple { items: schemas }
                | TypeSchema::TupleStruct { items: schemas, .. },
            ),
            Value::Array(values),
        ) => format!("({})", items(schemas, values)),
        (
            Some(TypeSchema::Struct {
                fields: schemas, ..
            }),
            Value::Object(map),
        ) => {
            format!("({})", fields(schemas, map))
        }
        (Some(TypeSchema::Map { key, value }), Value::Object(map)) => format!(
            "{{{}}}",
            join(
                map.iter()
                    .map(|(name, item)| {
                        format!("{}:{}", key_text(key, name), ron_text(Some(value), item))
                    })
                    .collect()
            )
        ),
        (Some(TypeSchema::Enum { variants, .. }), value) => {
            let (name, content) = variant(value);
            let shape = variants.iter().find(|v| v.name == name).map(|v| &v.fields);
            match (shape, content) {
                (Some(VariantFields::Newtype { item }), Some(content)) => {
                    format!("{name}({})", ron_text(Some(item), content))
                }
                (Some(VariantFields::Tuple { items: schemas }), Some(Value::Array(values))) => {
                    format!("{name}({})", items(schemas, values))
                }
                (Some(VariantFields::Struct { fields: schemas }), Some(Value::Object(map))) => {
                    format!("{name}({})", fields(schemas, map))
                }
                _ => name.to_string(),
            }
        }
        (_, Value::String(text)) => ron::to_string(text).expect("a string always encodes"),
        (_, Value::Bool(_) | Value::Number(_)) => value.to_string(),
        _ => ron::to_string(value).expect("a JSON value always encodes"),
    }
}
