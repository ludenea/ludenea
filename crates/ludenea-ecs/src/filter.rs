//! Entity filters written as text, such as `With<Position>, Without<Frozen>`, for the
//! editor's entity explorer.

use crate::{Entity, World};

#[derive(Debug, PartialEq, Eq, thiserror::Error)]
pub enum FilterError {
    #[error("cannot read filter term `{0}`; write `With<Name>` or `Without<Name>`")]
    Syntax(String),
    #[error("no component is named `{name}`; registered components: {candidates:?}")]
    Unknown {
        name: String,
        candidates: Vec<&'static str>,
    },
    #[error("`{name}` matches several components; use the full name of one of {candidates:?}")]
    Ambiguous {
        name: String,
        candidates: Vec<&'static str>,
    },
}

/// Entities that have every `with` component and none of the `without` ones, by stable name.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct EntityFilter {
    with: Vec<&'static str>,
    without: Vec<&'static str>,
}

impl EntityFilter {
    /// Parses comma-separated `With<Name>` and `Without<Name>` terms. `Name` is a stable
    /// name, or its last `::` segment when exactly one component ends with it. Empty text
    /// matches every entity.
    pub fn parse(text: &str, world: &World) -> Result<Self, FilterError> {
        let mut filter = Self::default();
        for term in text
            .split(',')
            .map(str::trim)
            .filter(|term| !term.is_empty())
        {
            let (list, name) = if let Some(name) = bracketed(term, "Without") {
                (&mut filter.without, name)
            } else if let Some(name) = bracketed(term, "With") {
                (&mut filter.with, name)
            } else {
                return Err(FilterError::Syntax(term.to_string()));
            };
            list.push(resolve(name, world)?);
        }
        Ok(filter)
    }
}

/// The trimmed name inside `prefix<...>`, if `term` has that shape.
fn bracketed<'a>(term: &'a str, prefix: &str) -> Option<&'a str> {
    let name = term.strip_prefix(prefix)?.trim_start();
    let name = name.strip_prefix('<')?.strip_suffix('>')?.trim();
    (!name.is_empty()).then_some(name)
}

fn resolve(name: &str, world: &World) -> Result<&'static str, FilterError> {
    let all = || world.components.keys().copied();
    if let Some(full) = all().find(|&full| full == name) {
        return Ok(full);
    }
    let matches: Vec<&'static str> = all()
        .filter(|full| full.rsplit("::").next() == Some(name))
        .collect();
    match matches.as_slice() {
        [only] => Ok(only),
        [] => Err(FilterError::Unknown {
            name: name.to_string(),
            candidates: all().collect(),
        }),
        _ => Err(FilterError::Ambiguous {
            name: name.to_string(),
            candidates: matches,
        }),
    }
}

impl World {
    /// The alive entities matching `filter`, in ascending index order.
    pub fn filter_entities(&self, filter: &EntityFilter) -> Vec<Entity> {
        let has = |name: &str, entity: Entity| {
            self.components
                .get(name)
                .is_some_and(|entry| entry.storage.has(entity.index()))
        };
        self.entities()
            .filter(|&entity| {
                filter.with.iter().all(|name| has(name, entity))
                    && !filter.without.iter().any(|name| has(name, entity))
            })
            .collect()
    }
}
