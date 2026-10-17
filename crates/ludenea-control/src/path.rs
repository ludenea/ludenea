//! Field paths into JSON values in serde's externally tagged form.

use std::fmt;

use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;

/// One step into a value: a struct field, or a list, array, or tuple index. An enum
/// variant's content is the field named after the variant; a newtype variant's content
/// is that field's value directly, so `Collider.shape` as `{"Circle": {"radius": ..}}`
/// takes the path `shape`, `Circle`, `radius`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum PathSegment {
    Field { name: String },
    Index { index: u32 },
}

/// Replaces the value at `path` in `root` with `value`. Every step must exist: a path
/// never adds a field or an element.
pub fn set_at_path(
    root: &mut JsonValue,
    path: &[PathSegment],
    value: JsonValue,
) -> Result<(), String> {
    let mut target = root;
    for (depth, segment) in path.iter().enumerate() {
        let found = match segment {
            PathSegment::Field { name } => target.get_mut(name.as_str()),
            PathSegment::Index { index } => target.get_mut(*index as usize),
        };
        target = found.ok_or_else(|| {
            format!(
                "`{}` has no {segment}; pick a path the value has",
                Path(&path[..depth])
            )
        })?;
    }
    *target = value;
    Ok(())
}

impl fmt::Display for PathSegment {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PathSegment::Field { name } => write!(formatter, "field `{name}`"),
            PathSegment::Index { index } => write!(formatter, "index {index}"),
        }
    }
}

/// Displays a path the way it reads in code, such as `shape.Circle.radius` or
/// `points[2].x`; the empty path displays as `.`, the whole value.
pub struct Path<'a>(pub &'a [PathSegment]);

impl fmt::Display for Path<'_> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.0.is_empty() {
            return formatter.write_str(".");
        }
        for (position, segment) in self.0.iter().enumerate() {
            match segment {
                PathSegment::Field { name } if position == 0 => formatter.write_str(name)?,
                PathSegment::Field { name } => write!(formatter, ".{name}")?,
                PathSegment::Index { index } => write!(formatter, "[{index}]")?,
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn field(name: &str) -> PathSegment {
        PathSegment::Field {
            name: name.to_string(),
        }
    }

    #[test]
    fn sets_fields_indices_and_the_whole_value() {
        let mut value = json!({"shape": {"Circle": {"radius": 1.0}}, "points": [1, 2]});
        set_at_path(
            &mut value,
            &[field("shape"), field("Circle"), field("radius")],
            json!(2.0),
        )
        .unwrap();
        set_at_path(
            &mut value,
            &[field("points"), PathSegment::Index { index: 1 }],
            json!(5),
        )
        .unwrap();
        assert_eq!(
            value,
            json!({"shape": {"Circle": {"radius": 2.0}}, "points": [1, 5]})
        );
        set_at_path(&mut value, &[], json!(null)).unwrap();
        assert_eq!(value, json!(null));
    }

    #[test]
    fn rejects_missing_steps_naming_where_they_fail() {
        let mut value = json!({"points": [1, 2]});
        let index = PathSegment::Index { index: 2 };
        let error = set_at_path(&mut value, &[field("points"), index.clone()], json!(0));
        assert_eq!(
            error.unwrap_err(),
            "`points` has no index 2; pick a path the value has"
        );
        assert!(set_at_path(&mut value, &[field("x")], json!(0)).is_err());
        assert_eq!(
            Path(&[field("points"), index, field("x")]).to_string(),
            "points[2].x"
        );
    }
}
