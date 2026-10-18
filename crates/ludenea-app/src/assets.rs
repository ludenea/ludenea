//! Asset texts a scene load reads, the data asset file form, and the handles a value holds.

use std::collections::{BTreeMap, BTreeSet};

use ludenea_ecs::AssetId;
use ludenea_ecs::schema::{TypeSchema, VariantFields};
use ron::Value;
use ron::value::RawValue;
use serde::Deserialize;
use serde::de::IntoDeserializer;

/// Where a scene load finds asset texts, by identifier.
pub trait AssetSource {
    fn text(&self, id: AssetId) -> Option<&str>;

    /// The file the asset was read from, named in errors when known.
    fn path(&self, _id: AssetId) -> Option<&str> {
        None
    }
}

/// No asset at all: a scene that references one fails to load.
pub struct NoAssets;

impl AssetSource for NoAssets {
    fn text(&self, _id: AssetId) -> Option<&str> {
        None
    }
}

/// Asset texts in file form, by identifier.
pub type AssetTexts = BTreeMap<AssetId, String>;

impl AssetSource for AssetTexts {
    fn text(&self, id: AssetId) -> Option<&str> {
        self.get(&id).map(String::as_str)
    }
}

/// Asset texts with the file each one was read from.
pub struct NamedAssetTexts {
    pub texts: AssetTexts,
    pub paths: BTreeMap<AssetId, String>,
}

impl AssetSource for NamedAssetTexts {
    fn text(&self, id: AssetId) -> Option<&str> {
        self.texts.text(id)
    }

    fn path(&self, id: AssetId) -> Option<&str> {
        self.paths.get(&id).map(String::as_str)
    }
}

/// The data asset file form; the RON name `AssetFile` is checked against this struct name.
#[derive(Deserialize)]
struct AssetFile<'a> {
    #[serde(rename = "type")]
    type_name: String,
    #[serde(borrow)]
    value: &'a RawValue,
}

/// Writes an asset in file form.
pub fn asset_file(type_name: &str, value_ron: &str) -> String {
    format!("AssetFile(type: {type_name:?}, value: {value_ron})")
}

/// The type name and the value RON of a data asset file, or the parse error with its line
/// and column.
pub fn parse_asset_file(text: &str) -> Result<(String, String), String> {
    let (type_name, value) = parse_asset_file_in_place(text)?;
    Ok((type_name, value.trim_start().to_string()))
}

/// Like `parse_asset_file`, but the value RON keeps its place in the file: the text before
/// it is blanked (newlines kept), so errors decoding it give line and column in the file.
pub(crate) fn parse_asset_file_in_place(text: &str) -> Result<(String, String), String> {
    let file: AssetFile = ron::from_str(text).map_err(|error| error.to_string())?;
    let value = file.value.get_ron();
    let start = value.as_ptr() as usize - text.as_ptr() as usize;
    let blank: String = text[..start]
        .chars()
        .map(|c| if c == '\n' { '\n' } else { ' ' })
        .collect();
    Ok((file.type_name, blank + value))
}

/// Adds every `Handle` inside `value` (identifier and asset type name), walked by `schema`.
/// RON values do not keep enum variant names, so every variant whose shape matches is walked.
pub fn reachable_handles(
    schema: &TypeSchema,
    value: &Value,
    out: &mut BTreeSet<(AssetId, String)>,
) {
    visit_handles(schema, value, &mut String::new(), &mut |id, asset, _| {
        out.insert((id, asset.to_string()));
    });
}

/// Calls `found` with the identifier, asset type name, and field path (such as `.stats` or
/// `.weapons[2]`, appended to `path`) of every handle inside `value`.
pub(crate) fn visit_handles(
    schema: &TypeSchema,
    value: &Value,
    path: &mut String,
    found: &mut dyn FnMut(AssetId, &str, &str),
) {
    match (schema, value) {
        (TypeSchema::Handle { asset }, Value::String(text)) => {
            let text: &str = text;
            if let Ok(id) =
                AssetId::deserialize(text.into_deserializer()) as Result<_, serde::de::value::Error>
            {
                found(id, asset, path);
            }
        }
        (TypeSchema::Option { item }, Value::Option(Some(inner))) => {
            visit_handles(item, inner, path, found);
        }
        (TypeSchema::List { item } | TypeSchema::Array { item, .. }, Value::Seq(values)) => {
            for (index, value) in values.iter().enumerate() {
                at(path, &format!("[{index}]"), |path| {
                    visit_handles(item, value, path, found);
                });
            }
        }
        (TypeSchema::Map { key, value: item }, Value::Map(map)) => {
            for (index, (key_value, value)) in map.iter().enumerate() {
                at(path, &format!("[{index}]"), |path| {
                    visit_handles(key, key_value, path, found);
                    visit_handles(item, value, path, found);
                });
            }
        }
        (TypeSchema::Tuple { items } | TypeSchema::TupleStruct { items, .. }, value) => {
            visit_items(items, value, path, found);
        }
        (TypeSchema::Struct { fields, .. }, value) => visit_fields(fields, value, path, found),
        (TypeSchema::Newtype { item, .. }, value) => {
            visit_handles(item, newtype_inner(value), path, found);
        }
        (TypeSchema::Enum { variants, .. }, value) => {
            for variant in variants {
                match &variant.fields {
                    VariantFields::Unit => {}
                    VariantFields::Newtype { item } => {
                        visit_handles(item, newtype_inner(value), path, found);
                    }
                    VariantFields::Tuple { items } => visit_items(items, value, path, found),
                    VariantFields::Struct { fields } => visit_fields(fields, value, path, found),
                }
            }
        }
        _ => {}
    }
}

fn visit_items(
    items: &[TypeSchema],
    value: &Value,
    path: &mut String,
    found: &mut dyn FnMut(AssetId, &str, &str),
) {
    if let Value::Seq(values) = value
        && values.len() == items.len()
    {
        for (index, (item, value)) in items.iter().zip(values).enumerate() {
            at(path, &format!(".{index}"), |path| {
                visit_handles(item, value, path, found);
            });
        }
    }
}

fn visit_fields(
    fields: &[ludenea_ecs::schema::FieldSchema],
    value: &Value,
    path: &mut String,
    found: &mut dyn FnMut(AssetId, &str, &str),
) {
    let Value::Map(map) = value else { return };
    for field in fields {
        if let Some(value) = map.get(&Value::String(field.name.clone())) {
            at(path, &format!(".{}", field.name), |path| {
                visit_handles(&field.schema, value, path, found);
            });
        }
    }
}

/// RON writes a newtype as `(inner)`, which parses as a one-element sequence.
fn newtype_inner(value: &Value) -> &Value {
    match value {
        Value::Seq(values) if values.len() == 1 => &values[0],
        _ => value,
    }
}

/// Runs `visit` with `segment` appended to `path`.
fn at(path: &mut String, segment: &str, visit: impl FnOnce(&mut String)) {
    let length = path.len();
    path.push_str(segment);
    visit(path);
    path.truncate(length);
}
