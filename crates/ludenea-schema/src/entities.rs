//! Finds the entity references inside a JSON value, guided by its schema.

use serde_json::Value;

use crate::{FieldSchema, TypeSchema, VariantFields};

/// An `Entity` found in a value, with the path to it (`target`, `hits[2]`, `shape.Orbit.0`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EntityRefAt {
    pub path: String,
    pub index: u32,
    pub generation: u32,
}

/// Every `Entity` in `value`, which has the serde JSON shape of `schema`, in document order.
/// A struct that merely has `index` and `generation` fields is not an entity: only the
/// schema decides. A value that does not match the schema contributes nothing.
pub fn entity_refs(schema: &TypeSchema, value: &Value) -> Vec<EntityRefAt> {
    let mut found = Vec::new();
    walk(schema, value, String::new(), &mut found);
    found
}

fn field(path: &str, name: &str) -> String {
    if path.is_empty() {
        name.to_string()
    } else {
        format!("{path}.{name}")
    }
}

fn walk(schema: &TypeSchema, value: &Value, path: String, found: &mut Vec<EntityRefAt>) {
    use TypeSchema::*;
    match schema {
        Entity => {
            let number = |key: &str| value.get(key)?.as_u64()?.try_into().ok();
            if let (Some(index), Some(generation)) = (number("index"), number("generation")) {
                found.push(EntityRefAt {
                    path,
                    index,
                    generation,
                });
            }
        }
        Option { item } | Newtype { item, .. } => {
            if !value.is_null() {
                walk(item, value, path, found);
            }
        }
        List { item } | Array { item, .. } => {
            for (position, element) in value.as_array().into_iter().flatten().enumerate() {
                walk(item, element, format!("{path}[{position}]"), found);
            }
        }
        Map { value: item, .. } => {
            for (key, entry) in value.as_object().into_iter().flatten() {
                walk(item, entry, format!("{path}[{key}]"), found);
            }
        }
        Tuple { items } | TupleStruct { items, .. } => walk_tuple(items, value, &path, found),
        Struct { fields, .. } => walk_fields(fields, value, &path, found),
        Enum { variants, .. } => {
            let Some((name, content)) = value.as_object().and_then(|object| object.iter().next())
            else {
                return;
            };
            let Some(variant) = variants.iter().find(|variant| &variant.name == name) else {
                return;
            };
            let path = field(&path, name);
            match &variant.fields {
                VariantFields::Unit => {}
                VariantFields::Newtype { item } => walk(item, content, path, found),
                VariantFields::Tuple { items } => walk_tuple(items, content, &path, found),
                VariantFields::Struct { fields } => walk_fields(fields, content, &path, found),
            }
        }
        Bool
        | U8
        | U16
        | U32
        | U64
        | I8
        | I16
        | I32
        | I64
        | F32
        | F64
        | String
        | Unit
        | UnitStruct { .. }
        | AssetId
        | Handle { .. } => {}
    }
}

fn walk_fields(fields: &[FieldSchema], value: &Value, path: &str, found: &mut Vec<EntityRefAt>) {
    for field_schema in fields {
        if let Some(inner) = value.get(&field_schema.name) {
            walk(
                &field_schema.schema,
                inner,
                field(path, &field_schema.name),
                found,
            );
        }
    }
}

fn walk_tuple(items: &[TypeSchema], value: &Value, path: &str, found: &mut Vec<EntityRefAt>) {
    for (position, (item, element)) in items
        .iter()
        .zip(value.as_array().into_iter().flatten())
        .enumerate()
    {
        walk(item, element, field(path, &position.to_string()), found);
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn named(name: &str, fields: Vec<(&str, TypeSchema)>) -> TypeSchema {
        TypeSchema::Struct {
            name: name.into(),
            fields: fields
                .into_iter()
                .map(|(name, schema)| FieldSchema {
                    name: name.into(),
                    schema,
                })
                .collect(),
        }
    }

    fn at(path: &str, index: u32, generation: u32) -> EntityRefAt {
        EntityRefAt {
            path: path.into(),
            index,
            generation,
        }
    }

    #[test]
    fn entity_refs_finds_nested_entities_with_paths() {
        let list = |item| TypeSchema::List {
            item: Box::new(item),
        };
        let schema = named(
            "Group",
            vec![
                (
                    "leader",
                    TypeSchema::Option {
                        item: Box::new(TypeSchema::Entity),
                    },
                ),
                ("hits", list(TypeSchema::Entity)),
                (
                    "link",
                    TypeSchema::Enum {
                        name: "Link".into(),
                        variants: vec![crate::VariantSchema {
                            name: "To".into(),
                            fields: VariantFields::Struct {
                                fields: vec![FieldSchema {
                                    name: "target".into(),
                                    schema: TypeSchema::Entity,
                                }],
                            },
                        }],
                    },
                ),
            ],
        );
        let entity = |index, generation| json!({ "index": index, "generation": generation });
        let value = json!({
            "leader": entity(1, 0),
            "hits": [entity(2, 0), entity(3, 4), entity(5, 6)],
            "link": { "To": { "target": entity(7, 8) } },
        });
        assert_eq!(
            entity_refs(&schema, &value),
            [
                at("leader", 1, 0),
                at("hits[0]", 2, 0),
                at("hits[1]", 3, 4),
                at("hits[2]", 5, 6),
                at("link.To.target", 7, 8),
            ]
        );
    }

    #[test]
    fn entity_refs_paths_through_maps_tuples_arrays_and_tuple_variants() {
        let entity_box = || Box::new(TypeSchema::Entity);
        let schema = named(
            "Board",
            vec![
                (
                    "by_name",
                    TypeSchema::Map {
                        key: Box::new(TypeSchema::String),
                        value: entity_box(),
                    },
                ),
                (
                    "pair",
                    TypeSchema::Tuple {
                        items: vec![TypeSchema::U8, TypeSchema::Entity],
                    },
                ),
                (
                    "slots",
                    TypeSchema::Array {
                        item: entity_box(),
                        len: 2,
                    },
                ),
                (
                    "shape",
                    TypeSchema::Enum {
                        name: "Shape".into(),
                        variants: vec![crate::VariantSchema {
                            name: "Orbit".into(),
                            fields: VariantFields::Tuple {
                                items: vec![TypeSchema::Entity, TypeSchema::F32],
                            },
                        }],
                    },
                ),
            ],
        );
        let entity = |index: u32| json!({ "index": index, "generation": 0 });
        let value = json!({
            "by_name": { "a": entity(1), "b": entity(2) },
            "pair": [9, entity(3)],
            "slots": [entity(4), entity(5)],
            "shape": { "Orbit": [entity(6), 1.5] },
        });
        assert_eq!(
            entity_refs(&schema, &value),
            [
                at("by_name[a]", 1, 0),
                at("by_name[b]", 2, 0),
                at("pair.1", 3, 0),
                at("slots[0]", 4, 0),
                at("slots[1]", 5, 0),
                at("shape.Orbit.0", 6, 0),
            ]
        );
    }

    #[test]
    fn entity_refs_ignores_structs_shaped_like_entities() {
        let lookalike = named(
            "Slot",
            vec![("index", TypeSchema::U32), ("generation", TypeSchema::U32)],
        );
        let value = json!({ "index": 1, "generation": 2 });
        assert!(entity_refs(&lookalike, &value).is_empty());
        let schema = named("Holder", vec![("slot", lookalike)]);
        assert!(entity_refs(&schema, &json!({ "slot": value })).is_empty());
    }
}
