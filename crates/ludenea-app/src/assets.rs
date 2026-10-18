//! Asset texts a scene load reads and the data asset file form.

use std::collections::BTreeMap;

use ludenea_ecs::AssetId;
use ron::value::RawValue;
use serde::Deserialize;

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
/// When `text` is a file wrapped by [`asset_file`], the file is the wrapped text: the
/// wrapper's prefix is not counted in the columns of line 1, in errors parsing `text` too.
pub(crate) fn parse_asset_file_in_place(text: &str) -> Result<(String, String), String> {
    let prefix = wrapper_prefix(text);
    let file: AssetFile = ron::from_str(text)
        .map_err(|error| shift_line_one(&error.to_string(), prefix.unwrap_or(0)))?;
    let value = file.value.get_ron();
    let start = value.as_ptr() as usize - text.as_ptr() as usize;
    // The value may start before the wrapped file, at the space after `value:`.
    let in_place: String = text[..start]
        .chars()
        .map(|c| if c == '\n' { '\n' } else { ' ' })
        .chain(value.chars())
        .skip(prefix.unwrap_or(0))
        .collect();
    Ok((file.type_name, in_place))
}

/// The length in characters of the prefix [`asset_file`] writes on line 1 before the
/// wrapped file, when `text` starts with it.
fn wrapper_prefix(text: &str) -> Option<usize> {
    const OPENING: &str = "AssetFile(type: \"";
    const VALUE: &str = ", value: ";
    let first = text.lines().next()?;
    let end = first.strip_prefix(OPENING)?.find(VALUE)? + OPENING.len() + VALUE.len();
    Some(first[..end].chars().count())
}

/// `message` with the columns of its leading line 1 positions (`1:30: ..`, `1:30-1:31: ..`)
/// moved left by `by`.
fn shift_line_one(message: &str, by: usize) -> String {
    let Some((positions, rest)) = message.split_once(": ") else {
        return message.to_string();
    };
    let shifted: Option<Vec<String>> = positions
        .split('-')
        .map(|position| {
            let (line, column) = position.split_once(':')?;
            let (line, column): (usize, usize) = (line.parse().ok()?, column.parse().ok()?);
            let column = if line == 1 {
                column.saturating_sub(by).max(1)
            } else {
                column
            };
            Some(format!("{line}:{column}"))
        })
        .collect();
    match shifted {
        Some(shifted) => format!("{}: {rest}", shifted.join("-")),
        None => message.to_string(),
    }
}
