//! An archetype: the table of every entity with exactly one set of
//! components.

use std::fmt;

use crate::column::{Column, downcast_mut, downcast_ref};
use crate::registry::{ComponentId, Registry};
use crate::{Component, Entity};

/// Position of an archetype in the world, in creation order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct ArchetypeId(pub(crate) usize);

impl ArchetypeId {
    /// The archetype without components, created with the world.
    pub(crate) const EMPTY: Self = Self(0);
}

/// One row per entity, one column per component, ordered by
/// [`ComponentId`].
pub struct Archetype {
    key: Vec<ComponentId>,
    entities: Vec<Entity>,
    columns: Vec<Box<dyn Column>>,
}

impl Archetype {
    /// Creates an empty archetype for a sorted `key` without duplicates.
    pub(crate) fn new(key: Vec<ComponentId>, registry: &Registry) -> Self {
        let columns = key.iter().map(|&id| registry.new_column(id)).collect();
        Self {
            key,
            entities: Vec::new(),
            columns,
        }
    }

    pub(crate) fn key(&self) -> &[ComponentId] {
        &self.key
    }

    pub(crate) fn entities(&self) -> &[Entity] {
        &self.entities
    }

    pub(crate) fn column<T: Component>(&self, id: ComponentId) -> Option<&Vec<T>> {
        let position = self.key.binary_search(&id).ok()?;
        Some(downcast_ref(self.columns[position].as_ref()))
    }

    pub(crate) fn column_mut<T: Component>(&mut self, id: ComponentId) -> Option<&mut Vec<T>> {
        let position = self.key.binary_search(&id).ok()?;
        Some(downcast_mut(self.columns[position].as_mut()))
    }

    /// Adds `entity` in a new row and returns the row. The caller then
    /// writes one value in every column.
    pub(crate) fn push_entity(&mut self, entity: Entity) -> usize {
        self.entities.push(entity);
        self.entities.len() - 1
    }

    /// Removes `row` and returns the entity moved into it, if any.
    pub(crate) fn swap_remove(&mut self, row: usize) -> Option<Entity> {
        self.entities.swap_remove(row);
        for column in &mut self.columns {
            column.swap_remove(row);
        }
        self.entities.get(row).copied()
    }

    /// Moves `row` to the end of `destination`, carrying the values of the
    /// components both archetypes share and dropping the others. Returns the
    /// entity moved into `row`, if any.
    pub(crate) fn move_row(&mut self, row: usize, destination: &mut Self) -> Option<Entity> {
        let entity = self.entities.swap_remove(row);
        destination.entities.push(entity);
        for (id, column) in self.key.iter().zip(&mut self.columns) {
            match destination.key.binary_search(id) {
                Ok(position) => column.move_row(row, destination.columns[position].as_mut()),
                Err(_) => column.swap_remove(row),
            }
        }
        self.entities.get(row).copied()
    }
}

/// Shows the layout, not the values: components need not be `Debug`.
impl fmt::Debug for Archetype {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Archetype")
            .field("key", &self.key)
            .field("entities", &self.entities)
            .finish_non_exhaustive()
    }
}
