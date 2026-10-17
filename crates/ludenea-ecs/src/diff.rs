//! Field-level differences between two snapshots of the same world.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use ron::Value;

use crate::hash::ENTITIES_LEAF;
use crate::snapshot::{ComponentValues, Snapshot, SnapshotError};
use crate::{Codec, Entity, World};

const ABSENT: &str = "absent";

/// One differing value: `subject` is `entity 3v0`, `resource`, or `entities`, and `path`
/// locates the field inside the component (empty when the whole value differs).
#[derive(Clone, Debug, PartialEq)]
pub struct Difference {
    pub subject: String,
    pub component: String,
    pub path: String,
    pub left: String,
    pub right: String,
}

impl fmt::Display for Difference {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "{} {} {}: {} != {}",
            self.subject, self.component, self.path, self.left, self.right
        )
    }
}

impl World {
    /// Lists every difference between `a` and `b`, both of which must match this world's
    /// registry: the entity allocator first, then resources and components by name.
    ///
    /// Field paths come from `ron::Value`, which collapses enum variants: a variant change
    /// shows up only when nothing else in that value differs (as one whole-value
    /// difference with an empty path), so it can go unreported beside a changed sibling
    /// field. The differing component or resource is always named.
    pub fn diff_snapshots(
        &self,
        a: &Snapshot,
        b: &Snapshot,
    ) -> Result<Vec<Difference>, SnapshotError> {
        self.check_compatible(a)?;
        self.check_compatible(b)?;
        let mut differences = Vec::new();

        if a.entities != b.entities {
            let left = ron::to_string(&a.entities).expect("entities always encode");
            let right = ron::to_string(&b.entities).expect("entities always encode");
            compare("entities", ENTITIES_LEAF, left, right, &mut differences);
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
                let left = to_ron(codec, name, left)?;
                let right = to_ron(codec, name, right)?;
                compare("resource", name, left, right, &mut differences);
            }
        }

        let right: BTreeMap<&str, &ComponentValues> = b
            .components
            .iter()
            .map(|(name, values)| (name.as_str(), values))
            .collect();
        for (name, left) in &a.components {
            let codec = self.component_codec(name).expect("checked");
            let left: BTreeMap<Entity, &Vec<u8>> = left.iter().map(|(e, v)| (*e, v)).collect();
            let right: BTreeMap<Entity, &Vec<u8>> =
                right[name.as_str()].iter().map(|(e, v)| (*e, v)).collect();
            let entities: BTreeSet<Entity> = left.keys().chain(right.keys()).copied().collect();
            for entity in entities {
                let subject = format!("entity {entity}");
                match (left.get(&entity), right.get(&entity)) {
                    (Some(l), Some(r)) if l == r => {}
                    (Some(l), Some(r)) => {
                        let l = to_ron(codec, name, l)?;
                        let r = to_ron(codec, name, r)?;
                        compare(&subject, name, l, r, &mut differences);
                    }
                    (l, r) => {
                        let side = |bytes: Option<&&Vec<u8>>| match bytes {
                            Some(bytes) => to_ron(codec, name, bytes),
                            None => Ok(ABSENT.to_string()),
                        };
                        differences.push(Difference {
                            subject,
                            component: name.clone(),
                            path: String::new(),
                            left: side(l)?,
                            right: side(r)?,
                        });
                    }
                }
            }
        }
        Ok(differences)
    }
}

fn to_ron(codec: Codec, name: &str, bytes: &[u8]) -> Result<String, SnapshotError> {
    let value = (codec.decode)(bytes).map_err(|reason| SnapshotError::Decode {
        name: name.to_string(),
        reason,
    })?;
    Ok((codec.encode_ron)(&*value))
}

/// Walks the RON values of two differing encodings. `ron::Value` collapses enum variants,
/// so when the walk finds nothing the whole values are reported instead.
fn compare(subject: &str, component: &str, left: String, right: String, out: &mut Vec<Difference>) {
    let found = out.len();
    let parse = |text: &str| ron::from_str::<Value>(text).ok();
    if let (Some(l), Some(r)) = (parse(&left), parse(&right)) {
        walk(String::new(), Some(&l), Some(&r), &mut |path, l, r| {
            out.push(Difference {
                subject: subject.to_string(),
                component: component.to_string(),
                path,
                left: l,
                right: r,
            });
        });
    }
    if out.len() == found {
        out.push(Difference {
            subject: subject.to_string(),
            component: component.to_string(),
            path: String::new(),
            left,
            right,
        });
    }
}

fn walk(
    path: String,
    left: Option<&Value>,
    right: Option<&Value>,
    emit: &mut dyn FnMut(String, String, String),
) {
    match (left, right) {
        (Some(l), Some(r)) if l == r => {}
        (Some(Value::Map(l)), Some(Value::Map(r))) => {
            let keys: BTreeSet<&Value> = l.keys().chain(r.keys()).collect();
            for key in keys {
                let child = match key {
                    Value::String(field) => format!("{path}.{field}"),
                    other => format!("{path}[{}]", render(Some(other))),
                };
                walk(child, l.get(key), r.get(key), emit);
            }
        }
        (Some(Value::Seq(l)), Some(Value::Seq(r))) => {
            for index in 0..l.len().max(r.len()) {
                walk(format!("{path}[{index}]"), l.get(index), r.get(index), emit);
            }
        }
        (Some(Value::Option(Some(l))), Some(Value::Option(Some(r)))) => {
            walk(path, Some(l), Some(r), emit);
        }
        (l, r) => emit(path, render(l), render(r)),
    }
}

fn render(value: Option<&Value>) -> String {
    match value {
        Some(value) => ron::to_string(value).expect("a RON value always encodes"),
        None => ABSENT.to_string(),
    }
}
