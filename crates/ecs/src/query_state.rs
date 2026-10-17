//! Query states: the archetypes a system query matches, cached between runs.
#![cfg_attr(
    not(test),
    expect(dead_code, reason = "the system query parameter uses it")
)]

use std::marker::PhantomData;

use crate::World;
use crate::archetype::ArchetypeId;
use crate::query::{
    ComponentAccess, QueryData, QueryFilter, accesses_of, column_positions, matches,
};
use crate::registry::ComponentId;

/// What a system query accesses and the archetypes it matches. Matching
/// only runs on archetypes created since the last update.
pub struct QueryState<D: QueryData, F: QueryFilter = ()> {
    accesses: Vec<ComponentAccess>,
    /// Sorted components a matched archetype has: the accessed ones and the
    /// `With` filters.
    required: Vec<ComponentId>,
    excluded: Vec<ComponentId>,
    /// Matched archetypes, in creation order.
    matched: Vec<ArchetypeId>,
    /// Column positions of every matched archetype, `accesses.len()` per
    /// archetype, in the order of `accesses`.
    positions: Vec<usize>,
    /// Index in `matched` of every archetype seen, `None` when it does not
    /// match. Its length is the number of archetypes seen.
    lookup: Vec<Option<usize>>,
    epoch: u64,
    /// How many archetypes were tested against the query, for the tests
    /// proving the cache works.
    #[cfg(test)]
    tested: usize,
    marker: PhantomData<fn() -> (D, F)>,
}

impl<D: QueryData, F: QueryFilter> QueryState<D, F> {
    /// # Panics
    ///
    /// Panics when a component is not registered or appears twice in `D`.
    pub(crate) fn new(world: &World) -> Self {
        let accesses = accesses_of::<D>(&world.registry);
        let mut required: Vec<ComponentId> = accesses.iter().map(|access| access.id).collect();
        let mut excluded = Vec::new();
        F::filter_ids(&world.registry, &mut required, &mut excluded);
        required.sort_unstable();
        required.dedup();
        let mut state = Self {
            accesses,
            required,
            excluded,
            matched: Vec::new(),
            positions: Vec::new(),
            lookup: Vec::new(),
            epoch: world.archetype_epoch,
            #[cfg(test)]
            tested: 0,
            marker: PhantomData,
        };
        state.update(world);
        state
    }

    /// Matches the archetypes created since the last update, or every
    /// archetype again when the world's archetype epoch changed.
    pub(crate) fn update(&mut self, world: &World) {
        if self.epoch != world.archetype_epoch {
            self.matched.clear();
            self.positions.clear();
            self.lookup.clear();
            self.epoch = world.archetype_epoch;
        }
        let seen = self.lookup.len();
        for (index, archetype) in world.state.archetypes.iter().enumerate().skip(seen) {
            #[cfg(test)]
            {
                self.tested += 1;
            }
            let key = archetype.key();
            if matches(key, &self.required, &self.excluded) {
                self.lookup.push(Some(self.matched.len()));
                self.matched.push(ArchetypeId(index));
                self.positions.extend(column_positions(key, &self.accesses));
            } else {
                self.lookup.push(None);
            }
        }
    }

    pub(crate) fn matched(&self) -> &[ArchetypeId] {
        &self.matched
    }

    /// Column positions of the `index`-th matched archetype.
    pub(crate) fn positions(&self, index: usize) -> &[usize] {
        let width = self.accesses.len();
        &self.positions[index * width..(index + 1) * width]
    }

    /// Index in [`QueryState::matched`] of `archetype`, `None` when it does
    /// not match.
    pub(crate) fn matched_index(&self, archetype: ArchetypeId) -> Option<usize> {
        self.lookup.get(archetype.0).copied().flatten()
    }
}

#[cfg(test)]
mod tests {
    use super::QueryState;
    use crate::archetype::ArchetypeId;
    use crate::{Component, With, Without, World};

    #[derive(Clone, Copy)]
    struct Position;
    impl Component for Position {}

    #[derive(Clone, Copy)]
    struct Velocity;
    impl Component for Velocity {}

    #[derive(Clone, Copy)]
    struct Frozen;
    impl Component for Frozen {}

    fn world() -> World {
        let mut builder = World::builder();
        builder
            .register::<Position>()
            .register::<Velocity>()
            .register::<Frozen>();
        builder.build()
    }

    #[test]
    fn matches_archetypes_created_before_and_after_the_state() {
        let mut world = world();
        world.spawn(Position);
        let mut state = QueryState::<&Position>::new(&world);

        world.spawn(Velocity);
        world.spawn((Position, Velocity));
        state.update(&world);

        assert_eq!(state.matched(), [ArchetypeId(1), ArchetypeId(3)]);
        assert_eq!(state.matched_index(ArchetypeId(3)), Some(1));
        assert_eq!(state.matched_index(ArchetypeId(2)), None);
    }

    #[test]
    fn honours_typed_filters() {
        let mut world = world();
        world.spawn((Position, Velocity));
        world.spawn((Position, Velocity, Frozen));
        world.spawn(Position);

        let state = QueryState::<&Position, (With<Velocity>, Without<Frozen>)>::new(&world);

        assert_eq!(state.matched(), [ArchetypeId(1)]);
    }

    #[test]
    fn updating_tests_each_archetype_once() {
        let mut world = world();
        world.spawn(Position);
        let mut state = QueryState::<&Position>::new(&world);
        assert_eq!(state.tested, 2);

        state.update(&world);
        state.update(&world);
        assert_eq!(state.tested, 2);

        world.spawn(Velocity);
        state.update(&world);
        assert_eq!(state.tested, 3);
    }

    #[test]
    fn a_restore_rebuilds_the_matches() {
        let mut world = world();
        let snapshot = world.snapshot();
        world.spawn(Position);
        let mut state = QueryState::<&Position>::new(&world);
        assert_eq!(state.matched(), [ArchetypeId(1)]);

        world.restore(&snapshot).expect("same registry");
        world.spawn(Velocity);
        state.update(&world);

        assert_eq!(state.matched(), []);
    }

    #[test]
    fn positions_follow_the_query_order() {
        let mut world = world();
        world.spawn((Position, Velocity));

        let state = QueryState::<(&Velocity, &Position)>::new(&world);

        assert_eq!(state.positions(0), [1, 0]);
    }
}
