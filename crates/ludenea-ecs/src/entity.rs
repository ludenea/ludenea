use std::fmt;

use ludenea_schema::{Schema, TypeSchema};
use serde::{Deserialize, Serialize};

/// A generational handle: the index of a slot plus the generation of that slot when the
/// entity was spawned, so a handle kept after despawn never aliases a reused slot.
#[derive(Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct Entity {
    index: u32,
    generation: u32,
}

impl Entity {
    pub fn index(self) -> u32 {
        self.index
    }

    pub fn generation(self) -> u32 {
        self.generation
    }
}

impl Schema for Entity {
    fn schema() -> TypeSchema {
        TypeSchema::Entity
    }
}

impl fmt::Display for Entity {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}v{}", self.index, self.generation)
    }
}

/// Entity allocator state: one generation and liveness flag per slot, and a
/// last-in-first-out list of free slots.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct EntitiesState {
    pub generations: Vec<u32>,
    pub alive: Vec<bool>,
    pub free: Vec<u32>,
}

impl EntitiesState {
    pub(crate) fn spawn(&mut self) -> Entity {
        let index = match self.free.pop() {
            Some(index) => index,
            None => {
                let index =
                    u32::try_from(self.generations.len()).expect("more than u32::MAX entities");
                self.generations.push(0);
                self.alive.push(false);
                index
            }
        };
        self.alive[index as usize] = true;
        Entity {
            index,
            generation: self.generations[index as usize],
        }
    }

    pub(crate) fn despawn(&mut self, entity: Entity) {
        let slot = entity.index as usize;
        self.alive[slot] = false;
        // A slot reused 2^32 times aliases its oldest handles; wrapping is accepted.
        self.generations[slot] = self.generations[slot].wrapping_add(1);
        self.free.push(entity.index);
    }

    pub(crate) fn is_alive(&self, entity: Entity) -> bool {
        let slot = entity.index as usize;
        self.alive.get(slot) == Some(&true) && self.generations[slot] == entity.generation
    }

    /// The handle of slot `index` with its current generation.
    pub(crate) fn entity_at(&self, index: u32) -> Entity {
        Entity {
            index,
            generation: self.generations[index as usize],
        }
    }

    pub(crate) fn iter(&self) -> impl Iterator<Item = Entity> + '_ {
        self.alive
            .iter()
            .zip(&self.generations)
            .zip(0u32..)
            .filter(|((alive, _), _)| **alive)
            .map(|((_, &generation), index)| Entity { index, generation })
    }
}
