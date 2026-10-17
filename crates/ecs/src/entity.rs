//! Entity identifiers and their allocator.

use std::fmt;

/// Identifies an entity: an index reused after a despawn, and a generation
/// telling apart the successive entities that use the same index.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Entity {
    index: u32,
    generation: u32,
}

impl Entity {
    /// Position of the entity in the per-index tables of the world.
    pub(crate) fn index(self) -> usize {
        self.index as usize
    }
}

impl fmt::Display for Entity {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}v{}", self.index, self.generation)
    }
}

/// Hands out entity identifiers. Its whole state is part of a snapshot, so a
/// restored world hands out the same identifiers as the original.
#[derive(Debug, Default)]
pub(crate) struct Allocator {
    /// Current generation of every index ever allocated.
    generations: Vec<u32>,
    /// Indices free for reuse, the most recently freed last.
    free: Vec<u32>,
}

impl Allocator {
    /// Reuses the most recently freed index, or allocates a new one.
    ///
    /// # Panics
    ///
    /// Panics when every `u32` index is in use or retired.
    pub(crate) fn allocate(&mut self) -> Entity {
        if let Some(index) = self.free.pop() {
            let generation = self.generations[index as usize];
            return Entity { index, generation };
        }
        let index = u32::try_from(self.generations.len()).expect("entity indices are exhausted");
        self.generations.push(0);
        Entity {
            index,
            generation: 0,
        }
    }

    /// Frees the index of a live `entity`. An index whose generation reaches
    /// `u32::MAX` is retired instead of wrapping, so a stale identifier can
    /// never become valid again.
    pub(crate) fn free(&mut self, entity: Entity) {
        let generation = &mut self.generations[entity.index()];
        *generation += 1;
        if *generation < u32::MAX {
            self.free.push(entity.index);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn allocates_increasing_indices_at_generation_zero() {
        let mut allocator = Allocator::default();

        let first = allocator.allocate();
        let second = allocator.allocate();

        assert_eq!(first.to_string(), "0v0");
        assert_eq!(second.to_string(), "1v0");
    }

    #[test]
    fn reuses_the_most_recently_freed_index_first() {
        let mut allocator = Allocator::default();
        let first = allocator.allocate();
        let second = allocator.allocate();

        allocator.free(first);
        allocator.free(second);

        assert_eq!(allocator.allocate().to_string(), "1v1");
        assert_eq!(allocator.allocate().to_string(), "0v1");
    }

    #[test]
    fn a_reused_index_never_equals_its_previous_entity() {
        let mut allocator = Allocator::default();
        let first = allocator.allocate();

        allocator.free(first);

        assert_ne!(allocator.allocate(), first);
    }

    #[test]
    fn retires_an_index_whose_generation_reaches_the_maximum() {
        let mut allocator = Allocator {
            generations: vec![u32::MAX - 1],
            free: Vec::new(),
        };
        let last_use = Entity {
            index: 0,
            generation: u32::MAX - 1,
        };

        allocator.free(last_use);

        assert_eq!(allocator.allocate().to_string(), "1v0");
    }
}
