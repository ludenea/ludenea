use std::collections::BTreeMap;

use crate::{FieldSchema, TypeSchema, VariantFields};

/// TypeScript declarations for the given `(stable name, type)` pairs: one `export type`
/// per named type reachable from them, and an `export interface ComponentTypes` keyed by
/// stable name.
///
/// A named type is called by its last path segment; when two distinct types share one, both
/// are called by their full path with `::` replaced by `_`. The declarations follow serde's
/// externally tagged JSON form. Integers that may exceed 2^53 (`u64`, `i64`) are
/// `number | string`, because large values travel as strings.
pub fn typescript_declarations(types: &[(&str, &TypeSchema)]) -> String {
    let mut named = BTreeMap::new();
    for (_, schema) in types {
        collect(schema, &mut named);
    }
    let names = Names::new(named.keys().map(String::as_str));
    let mut out = String::new();
    for (full_name, schema) in &named {
        out.push_str(&format!(
            "export type {} = {};\n",
            names.of(full_name),
            declaration(schema, &names)
        ));
    }
    out.push_str("export interface ComponentTypes {\n");
    for (stable_name, schema) in types {
        out.push_str(&format!(
            "  \"{stable_name}\": {};\n",
            reference(schema, &names)
        ));
    }
    out.push_str("}\n");
    out
}

/// Full name to schema of every named type reachable from `schema`. The first schema seen
/// for a full name wins.
fn collect<'a>(schema: &'a TypeSchema, named: &mut BTreeMap<String, &'a TypeSchema>) {
    use TypeSchema::*;
    match schema {
        Option { item } | List { item } | Array { item, .. } => collect(item, named),
        Map { key, value } => {
            collect(key, named);
            collect(value, named);
        }
        Tuple { items } => items.iter().for_each(|item| collect(item, named)),
        Struct { name, fields } => {
            if enter(name, schema, named) {
                fields
                    .iter()
                    .for_each(|field| collect(&field.schema, named));
            }
        }
        Newtype { name, item } => {
            if enter(name, schema, named) {
                collect(item, named);
            }
        }
        TupleStruct { name, items } => {
            if enter(name, schema, named) {
                items.iter().for_each(|item| collect(item, named));
            }
        }
        UnitStruct { name } => {
            enter(name, schema, named);
        }
        Enum { name, variants } => {
            if enter(name, schema, named) {
                for variant in variants {
                    match &variant.fields {
                        VariantFields::Unit => {}
                        VariantFields::Newtype { item } => collect(item, named),
                        VariantFields::Tuple { items } => {
                            items.iter().for_each(|item| collect(item, named));
                        }
                        VariantFields::Struct { fields } => {
                            fields
                                .iter()
                                .for_each(|field| collect(&field.schema, named));
                        }
                    }
                }
            }
        }
        Bool | U8 | U16 | U32 | U64 | I8 | I16 | I32 | I64 | F32 | F64 | String | Unit | Entity
        | AssetId => {}
    }
}

/// Records a named type; false when its full name was already recorded.
fn enter<'a>(
    name: &str,
    schema: &'a TypeSchema,
    named: &mut BTreeMap<String, &'a TypeSchema>,
) -> bool {
    if named.contains_key(name) {
        return false;
    }
    named.insert(name.to_string(), schema);
    true
}

/// The TypeScript name of each named type.
struct Names(BTreeMap<String, String>);

impl Names {
    fn new<'a>(full_names: impl Iterator<Item = &'a str>) -> Self {
        let full_names: Vec<_> = full_names.collect();
        let short = |full: &str| full.rsplit("::").next().unwrap_or(full).to_string();
        let names = full_names
            .iter()
            .map(|full| {
                let collides = full_names
                    .iter()
                    .any(|other| other != full && short(other) == short(full));
                let name = if collides {
                    full.replace("::", "_")
                } else {
                    short(full)
                };
                (full.to_string(), name)
            })
            .collect();
        Self(names)
    }

    fn of(&self, full_name: &str) -> &str {
        &self.0[full_name]
    }
}

/// The type expression that stands for `schema` where a type is expected.
fn reference(schema: &TypeSchema, names: &Names) -> String {
    use TypeSchema::*;
    match schema {
        Bool => "boolean".into(),
        U8 | U16 | U32 | I8 | I16 | I32 | F32 | F64 => "number".into(),
        U64 | I64 => "number | string".into(),
        String => "string".into(),
        Unit => "null".into(),
        Option { item } => format!("{} | null", reference(item, names)),
        List { item } | Array { item, .. } => format!("Array<{}>", reference(item, names)),
        // ponytail: a map whose keys are not strings or integers has no JSON object form, so it is a list of pairs; the host shows such values read-only
        Map { key, value } => match **key {
            String | U8 | U16 | U32 | U64 | I8 | I16 | I32 | I64 => {
                format!("Record<string, {}>", reference(value, names))
            }
            _ => format!(
                "Array<[{}, {}]>",
                reference(key, names),
                reference(value, names)
            ),
        },
        Tuple { items } => tuple(items, names),
        Struct { name, .. }
        | Newtype { name, .. }
        | TupleStruct { name, .. }
        | UnitStruct { name }
        | Enum { name, .. } => names.of(name).to_string(),
        Entity => "{ index: number; generation: number }".into(),
        AssetId => "string".into(),
    }
}

/// The right-hand side of the `export type` of a named type.
fn declaration(schema: &TypeSchema, names: &Names) -> String {
    match schema {
        TypeSchema::Struct { fields, .. } => object(fields, names),
        TypeSchema::Newtype { item, .. } => reference(item, names),
        TypeSchema::TupleStruct { items, .. } => tuple(items, names),
        TypeSchema::UnitStruct { .. } => "null".into(),
        TypeSchema::Enum { variants, .. } if variants.is_empty() => "never".into(),
        TypeSchema::Enum { variants, .. } => variants
            .iter()
            .map(|variant| match &variant.fields {
                VariantFields::Unit => format!("\"{}\"", variant.name),
                VariantFields::Newtype { item } => {
                    format!("{{ {}: {} }}", variant.name, reference(item, names))
                }
                VariantFields::Tuple { items } => {
                    format!("{{ {}: {} }}", variant.name, tuple(items, names))
                }
                VariantFields::Struct { fields } => {
                    format!("{{ {}: {} }}", variant.name, object(fields, names))
                }
            })
            .collect::<Vec<_>>()
            .join(" | "),
        other => reference(other, names),
    }
}

fn object(fields: &[FieldSchema], names: &Names) -> String {
    if fields.is_empty() {
        return "{}".into();
    }
    let fields: Vec<_> = fields
        .iter()
        .map(|field| format!("{}: {}", field.name, reference(&field.schema, names)))
        .collect();
    format!("{{ {} }}", fields.join("; "))
}

fn tuple(items: &[TypeSchema], names: &Names) -> String {
    let items: Vec<_> = items.iter().map(|item| reference(item, names)).collect();
    format!("[{}]", items.join(", "))
}
