//! System queries: the [`Query`] parameter and the state caching the
//! archetypes it matches between runs.

use std::marker::PhantomData;
use std::mem;

use crate::access::{Access, QueryAccess};
use crate::archetype::ArchetypeId;
use crate::query::{
    ColumnBorrow, ComponentAccess, QueryData, QueryFilter, ReadOnlyQuery, accesses_of,
    column_positions, matches,
};
use crate::registry::ComponentId;
use crate::system::{SystemParam, WorldParts};
use crate::world::Location;
use crate::{Entity, World};

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

    /// What the query reads, writes and can touch, for conflict checks.
    fn access(&self) -> QueryAccess {
        let ids = |write: bool| {
            self.accesses
                .iter()
                .filter(|access| access.write == write)
                .map(|access| access.id)
                .collect()
        };
        QueryAccess {
            reads: ids(false),
            writes: ids(true),
            required: self.required.clone(),
            excluded: self.excluded.clone(),
        }
    }

    fn width(&self) -> usize {
        self.accesses.len()
    }

    /// Column positions of the `index`-th matched archetype.
    pub(crate) fn positions(&self, index: usize) -> &[usize] {
        let width = self.width();
        &self.positions[index * width..(index + 1) * width]
    }

    /// Index in [`QueryState::matched`] of `archetype`, `None` when it does
    /// not match.
    pub(crate) fn matched_index(&self, archetype: ArchetypeId) -> Option<usize> {
        self.lookup.get(archetype.0).copied().flatten()
    }
}

/// A system parameter iterating over the entities that have the components
/// of `D` and pass the filters of `F`.
///
/// ```
/// use ludenea_ecs::{Component, Entity, Query, Without};
///
/// #[derive(Clone, Copy)]
/// struct Velocity(i32);
/// impl Component for Velocity {}
///
/// #[derive(Clone, Copy)]
/// struct Static;
/// impl Component for Static {}
///
/// fn slow_down(mut bodies: Query<(Entity, &mut Velocity), Without<Static>>) {
///     for (_, velocity) in bodies.iter_mut() {
///         velocity.0 /= 2;
///     }
/// }
/// ```
pub struct Query<'w, 's, D: QueryData, F: QueryFilter = ()> {
    state: &'s QueryState<D, F>,
    locations: &'w [Option<Location>],
    /// Entities of every matched archetype, in matched order.
    entities: Vec<&'w [Entity]>,
    /// Columns of every matched archetype, one per access of `D`.
    columns: Vec<ColumnBorrow<'w>>,
}

impl<'w, D: QueryData, F: QueryFilter> Query<'w, '_, D, F> {
    /// Iterates over the matching entities: archetypes in creation order,
    /// then rows in storage order.
    pub fn iter(&self) -> impl Iterator<Item = D::Item<'_>>
    where
        D: ReadOnlyQuery,
    {
        let width = self.state.width();
        let mut columns: &[ColumnBorrow<'w>] = &self.columns;
        self.entities.iter().flat_map(move |entities| {
            let (archetype, rest) = columns.split_at(width);
            columns = rest;
            D::fetch(entities, &mut archetype.iter().map(ColumnBorrow::share))
        })
    }

    /// Iterates over the matching entities with write access, in the order
    /// of [`Query::iter`].
    pub fn iter_mut(&mut self) -> impl Iterator<Item = D::Item<'_>> {
        let width = self.state.width();
        let mut columns: &mut [ColumnBorrow<'w>] = &mut self.columns;
        self.entities.iter().flat_map(move |entities| {
            let (archetype, rest) = mem::take(&mut columns).split_at_mut(width);
            columns = rest;
            D::fetch(
                entities,
                &mut archetype.iter_mut().map(ColumnBorrow::reborrow),
            )
        })
    }

    /// Returns the item of `entity`, or `None` when it is not alive or does
    /// not match the query.
    pub fn get(&self, entity: Entity) -> Option<D::Item<'_>>
    where
        D: ReadOnlyQuery,
    {
        let (index, row) = self.find(entity)?;
        let width = self.state.width();
        let columns = &self.columns[index * width..(index + 1) * width];
        D::fetch(
            self.entities[index],
            &mut columns.iter().map(ColumnBorrow::share),
        )
        .nth(row)
    }

    /// Returns the item of `entity` with write access, or `None` when it is
    /// not alive or does not match the query.
    pub fn get_mut(&mut self, entity: Entity) -> Option<D::Item<'_>> {
        let (index, row) = self.find(entity)?;
        let width = self.state.width();
        let columns = &mut self.columns[index * width..(index + 1) * width];
        D::fetch(
            self.entities[index],
            &mut columns.iter_mut().map(ColumnBorrow::reborrow),
        )
        .nth(row)
    }

    /// The matched archetype index and the row of a live `entity`.
    fn find(&self, entity: Entity) -> Option<(usize, usize)> {
        let location = (*self.locations.get(entity.index())?)?;
        let index = self.state.matched_index(location.archetype)?;
        (self.entities[index].get(location.row) == Some(&entity)).then_some((index, location.row))
    }
}

impl<D, F> SystemParam for Query<'_, '_, D, F>
where
    D: QueryData + 'static,
    F: QueryFilter + 'static,
{
    type State = QueryState<D, F>;
    type Item<'w, 's> = Query<'w, 's, D, F>;

    fn init(world: &World, access: &mut Access, system: &'static str) -> Self::State {
        let state = QueryState::new(world);
        access.add_query(state.access(), &world.registry, system);
        state
    }

    fn update(state: &mut Self::State, world: &World) {
        state.update(world);
    }

    fn get<'w, 's>(state: &'s mut Self::State, parts: &mut WorldParts<'w>) -> Self::Item<'w, 's> {
        let state: &'s QueryState<D, F> = state;
        let mut entities = Vec::with_capacity(state.matched.len());
        let mut columns = Vec::with_capacity(state.positions.len());
        for (index, &archetype) in state.matched.iter().enumerate() {
            entities.push(parts.entities(archetype));
            for (&position, access) in state.positions(index).iter().zip(&state.accesses) {
                columns.push(parts.column(archetype, position, access.write));
            }
        }
        Query {
            state,
            locations: parts.locations,
            entities,
            columns,
        }
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

        assert_eq!(state.matched, [ArchetypeId(1), ArchetypeId(3)]);
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

        assert_eq!(state.matched, [ArchetypeId(1)]);
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
        assert_eq!(state.matched, [ArchetypeId(1)]);

        world.restore(&snapshot).expect("same registry");
        world.spawn(Velocity);
        state.update(&world);

        assert_eq!(state.matched, []);
    }

    #[test]
    fn positions_follow_the_query_order() {
        let mut world = world();
        world.spawn((Position, Velocity));

        let state = QueryState::<(&Velocity, &Position)>::new(&world);

        assert_eq!(state.positions(0), [1, 0]);
    }
}
