//! Queries: iteration over the entities that have a set of components.

use std::iter::Copied;
use std::marker::PhantomData;
use std::slice;

use crate::archetype::Archetype;
use crate::column::{Column, downcast_mut, downcast_ref};
use crate::registry::{ComponentId, Registry};
use crate::{Component, Entity, World};

/// One column of an archetype, lent to a query.
#[doc(hidden)]
pub enum ColumnBorrow<'w> {
    Shared(&'w dyn Column),
    Exclusive(&'w mut dyn Column),
}

/// One component a query element accesses, and whether it writes it.
#[doc(hidden)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ComponentAccess {
    pub(crate) id: ComponentId,
    pub(crate) write: bool,
}

/// What a query yields for each entity: [`Entity`], `&T`, `&mut T`, or a
/// tuple of up to eight of them.
pub trait QueryData {
    /// The value yielded for one entity.
    type Item<'w>;

    #[doc(hidden)]
    type Fetch<'w>: Iterator<Item = Self::Item<'w>>;

    /// Pushes every component accessed, in query order.
    #[doc(hidden)]
    fn accesses(registry: &Registry, accesses: &mut Vec<ComponentAccess>);

    /// Iterates over the rows of one archetype. `columns` yields one column
    /// per access, in the order of [`QueryData::accesses`]; a written column
    /// is always lent exclusively.
    #[doc(hidden)]
    fn fetch<'w>(
        entities: &'w [Entity],
        columns: &mut impl Iterator<Item = ColumnBorrow<'w>>,
    ) -> Self::Fetch<'w>;
}

/// A [`QueryData`] that only reads, usable through a shared [`World`].
///
/// Sealed: only this crate decides which queries are read-only.
pub trait ReadOnlyQuery: QueryData + sealed::Sealed {}

mod sealed {
    /// Restricts [`ReadOnlyQuery`](super::ReadOnlyQuery) to this crate.
    pub trait Sealed {}
}

fn next_column<'w>(columns: &mut impl Iterator<Item = ColumnBorrow<'w>>) -> ColumnBorrow<'w> {
    columns
        .next()
        .expect("a query receives one column per accessed component")
}

impl QueryData for Entity {
    type Item<'w> = Entity;
    type Fetch<'w> = Copied<slice::Iter<'w, Entity>>;

    fn accesses(_: &Registry, _: &mut Vec<ComponentAccess>) {}

    fn fetch<'w>(
        entities: &'w [Entity],
        _: &mut impl Iterator<Item = ColumnBorrow<'w>>,
    ) -> Self::Fetch<'w> {
        entities.iter().copied()
    }
}

impl ReadOnlyQuery for Entity {}
impl sealed::Sealed for Entity {}

impl<T: Component> QueryData for &T {
    type Item<'w> = &'w T;
    type Fetch<'w> = slice::Iter<'w, T>;

    fn accesses(registry: &Registry, accesses: &mut Vec<ComponentAccess>) {
        accesses.push(ComponentAccess {
            id: registry.id::<T>(),
            write: false,
        });
    }

    fn fetch<'w>(
        _: &'w [Entity],
        columns: &mut impl Iterator<Item = ColumnBorrow<'w>>,
    ) -> Self::Fetch<'w> {
        let column: &'w dyn Column = match next_column(columns) {
            ColumnBorrow::Shared(column) => column,
            ColumnBorrow::Exclusive(column) => column,
        };
        downcast_ref::<T>(column).iter()
    }
}

impl<T: Component> ReadOnlyQuery for &T {}
impl<T: Component> sealed::Sealed for &T {}

impl<T: Component> QueryData for &mut T {
    type Item<'w> = &'w mut T;
    type Fetch<'w> = slice::IterMut<'w, T>;

    fn accesses(registry: &Registry, accesses: &mut Vec<ComponentAccess>) {
        accesses.push(ComponentAccess {
            id: registry.id::<T>(),
            write: true,
        });
    }

    fn fetch<'w>(
        _: &'w [Entity],
        columns: &mut impl Iterator<Item = ColumnBorrow<'w>>,
    ) -> Self::Fetch<'w> {
        match next_column(columns) {
            ColumnBorrow::Exclusive(column) => downcast_mut::<T>(column).iter_mut(),
            ColumnBorrow::Shared(_) => {
                unreachable!("a written column is always lent exclusively")
            }
        }
    }
}

/// Iterates over several fetches in lockstep; every fetch of one archetype
/// has one item per row.
#[doc(hidden)]
pub struct Zip<T>(T);

macro_rules! impl_query_data_for_tuple {
    ($($element:ident $index:tt),+) => {
        impl<$($element: QueryData),+> QueryData for ($($element,)+) {
            type Item<'w> = ($($element::Item<'w>,)+);
            type Fetch<'w> = Zip<($($element::Fetch<'w>,)+)>;

            fn accesses(registry: &Registry, accesses: &mut Vec<ComponentAccess>) {
                $($element::accesses(registry, accesses);)+
            }

            fn fetch<'w>(
                entities: &'w [Entity],
                columns: &mut impl Iterator<Item = ColumnBorrow<'w>>,
            ) -> Self::Fetch<'w> {
                Zip(($($element::fetch(entities, columns),)+))
            }
        }

        impl<$($element: ReadOnlyQuery),+> ReadOnlyQuery for ($($element,)+) {}
        impl<$($element: ReadOnlyQuery),+> sealed::Sealed for ($($element,)+) {}

        impl<$($element: Iterator),+> Iterator for Zip<($($element,)+)> {
            type Item = ($($element::Item,)+);

            fn next(&mut self) -> Option<Self::Item> {
                Some(($(self.0.$index.next()?,)+))
            }

            /// Skips in every fetch at once: slice iterators skip in
            /// constant time, so a random access stays cheap.
            fn nth(&mut self, n: usize) -> Option<Self::Item> {
                Some(($(self.0.$index.nth(n)?,)+))
            }
        }
    };
}

impl_query_data_for_tuple!(A 0);
impl_query_data_for_tuple!(A 0, B 1);
impl_query_data_for_tuple!(A 0, B 1, C 2);
impl_query_data_for_tuple!(A 0, B 1, C 2, D 3);
impl_query_data_for_tuple!(A 0, B 1, C 2, D 3, E 4);
impl_query_data_for_tuple!(A 0, B 1, C 2, D 3, E 4, F 5);
impl_query_data_for_tuple!(A 0, B 1, C 2, D 3, E 4, F 5, G 6);
impl_query_data_for_tuple!(A 0, B 1, C 2, D 3, E 4, F 5, G 6, H 7);

/// Typed filters of a system `Query`: `()`, [`With`], [`Without`], or a
/// tuple of up to eight of them.
pub trait QueryFilter {
    /// Pushes the components a matched archetype must have and must not
    /// have.
    #[doc(hidden)]
    fn filter_ids(
        registry: &Registry,
        required: &mut Vec<ComponentId>,
        excluded: &mut Vec<ComponentId>,
    );
}

/// Only matches entities that also have a `T`.
pub struct With<T>(PhantomData<T>);

/// Only matches entities without a `T`.
pub struct Without<T>(PhantomData<T>);

impl QueryFilter for () {
    fn filter_ids(_: &Registry, _: &mut Vec<ComponentId>, _: &mut Vec<ComponentId>) {}
}

impl<T: Component> QueryFilter for With<T> {
    fn filter_ids(registry: &Registry, required: &mut Vec<ComponentId>, _: &mut Vec<ComponentId>) {
        required.push(registry.id::<T>());
    }
}

impl<T: Component> QueryFilter for Without<T> {
    fn filter_ids(registry: &Registry, _: &mut Vec<ComponentId>, excluded: &mut Vec<ComponentId>) {
        excluded.push(registry.id::<T>());
    }
}

macro_rules! impl_query_filter_for_tuple {
    ($($filter:ident),+) => {
        impl<$($filter: QueryFilter),+> QueryFilter for ($($filter,)+) {
            fn filter_ids(
                registry: &Registry,
                required: &mut Vec<ComponentId>,
                excluded: &mut Vec<ComponentId>,
            ) {
                $($filter::filter_ids(registry, required, excluded);)+
            }
        }
    };
}

impl_query_filter_for_tuple!(A);
impl_query_filter_for_tuple!(A, B);
impl_query_filter_for_tuple!(A, B, C);
impl_query_filter_for_tuple!(A, B, C, D);
impl_query_filter_for_tuple!(A, B, C, D, E);
impl_query_filter_for_tuple!(A, B, C, D, E, F);
impl_query_filter_for_tuple!(A, B, C, D, E, F, G);
impl_query_filter_for_tuple!(A, B, C, D, E, F, G, H);

/// Whether an archetype with the sorted `key` has every `required`
/// component and none of the `excluded` ones.
pub(crate) fn matches(
    key: &[ComponentId],
    required: &[ComponentId],
    excluded: &[ComponentId],
) -> bool {
    required.iter().all(|id| key.binary_search(id).is_ok())
        && excluded.iter().all(|id| key.binary_search(id).is_err())
}

/// Position in the sorted `key` of the column of every access.
pub(crate) fn column_positions(key: &[ComponentId], accesses: &[ComponentAccess]) -> Vec<usize> {
    accesses
        .iter()
        .map(|access| {
            key.binary_search(&access.id)
                .expect("a matched archetype has every component of the query")
        })
        .collect()
}

/// Lends the columns at `positions`, all distinct, exclusively and in order.
fn lend_exclusive<'w>(
    columns: &'w mut [Box<dyn Column>],
    positions: &[usize],
) -> Vec<ColumnBorrow<'w>> {
    let mut slots: Vec<Option<&'w mut dyn Column>> = columns
        .iter_mut()
        .map(|column| Some(column.as_mut()))
        .collect();
    positions
        .iter()
        .map(|&position| {
            ColumnBorrow::Exclusive(
                slots[position]
                    .take()
                    .expect("a query accesses each component once"),
            )
        })
        .collect()
}

/// Collects the accesses of `D`, checking that no component appears twice.
///
/// # Panics
///
/// Panics when a component is not registered or appears twice in `D`.
pub(crate) fn accesses_of<D: QueryData>(registry: &Registry) -> Vec<ComponentAccess> {
    let mut accesses = Vec::new();
    D::accesses(registry, &mut accesses);
    registry.sorted_without_duplicates(accesses.iter().map(|access| access.id).collect(), "query");
    accesses
}

enum Archetypes<'w> {
    Shared(&'w [Archetype]),
    Exclusive(&'w mut [Archetype]),
}

/// An ad hoc query over a world, refined with [`WorldQuery::with`] and
/// [`WorldQuery::without`], then consumed by [`WorldQuery::iter`]. Systems
/// use a `Query` parameter instead.
///
/// A mutable query needs an exclusive world, so a shared world rejects it at
/// compile time:
///
/// ```compile_fail,E0277
/// use ludenea_ecs::{Component, World};
///
/// #[derive(Clone, Copy)]
/// struct Position(f32);
/// impl Component for Position {}
///
/// let mut builder = World::builder();
/// builder.register::<Position>();
/// let world = builder.build();
/// world.query::<&mut Position>();
/// ```
pub struct WorldQuery<'w, D: QueryData> {
    registry: &'w Registry,
    archetypes: Archetypes<'w>,
    accesses: Vec<ComponentAccess>,
    required: Vec<ComponentId>,
    without: Vec<ComponentId>,
    data: PhantomData<D>,
}

impl<'w, D: QueryData> WorldQuery<'w, D> {
    /// # Panics
    ///
    /// Panics when a component is not registered or appears twice in `D`.
    fn new(registry: &'w Registry, archetypes: Archetypes<'w>) -> Self {
        let accesses = accesses_of::<D>(registry);
        let required = accesses.iter().map(|access| access.id).collect();
        Self {
            registry,
            archetypes,
            accesses,
            required,
            without: Vec::new(),
            data: PhantomData,
        }
    }

    /// Only matches entities that also have a `T`.
    ///
    /// # Panics
    ///
    /// Panics when `T` is not registered.
    #[must_use]
    pub fn with<T: Component>(mut self) -> Self {
        self.required.push(self.registry.id::<T>());
        self
    }

    /// Only matches entities without a `T`.
    ///
    /// # Panics
    ///
    /// Panics when `T` is not registered.
    #[must_use]
    pub fn without<T: Component>(mut self) -> Self {
        self.without.push(self.registry.id::<T>());
        self
    }

    /// Iterates over the matching entities: archetypes in creation order,
    /// then rows in storage order.
    // ponytail: matches every archetype on each call; systems use a cached
    // `QueryState`, cache here too if an ad hoc query becomes hot.
    pub fn iter(self) -> impl Iterator<Item = D::Item<'w>> + use<'w, D> {
        let Self {
            archetypes,
            accesses,
            required,
            without,
            ..
        } = self;
        let fetches: Vec<D::Fetch<'w>> = match archetypes {
            Archetypes::Shared(archetypes) => archetypes
                .iter()
                .map(Archetype::parts)
                .filter(|(key, _, _)| matches(key, &required, &without))
                .map(|(key, entities, columns)| {
                    let mut borrows = column_positions(key, &accesses)
                        .into_iter()
                        .map(|position| ColumnBorrow::Shared(columns[position].as_ref()));
                    D::fetch(entities, &mut borrows)
                })
                .collect(),
            Archetypes::Exclusive(archetypes) => archetypes
                .iter_mut()
                .map(Archetype::parts_mut)
                .filter(|(key, _, _)| matches(key, &required, &without))
                .map(|(key, entities, columns)| {
                    let mut borrows =
                        lend_exclusive(columns, &column_positions(key, &accesses)).into_iter();
                    D::fetch(entities, &mut borrows)
                })
                .collect(),
        };
        fetches.into_iter().flatten()
    }
}

impl World {
    /// Starts a read-only query.
    ///
    /// # Panics
    ///
    /// Panics when a component is not registered or appears twice in `D`.
    #[must_use]
    pub fn query<D: ReadOnlyQuery>(&self) -> WorldQuery<'_, D> {
        WorldQuery::new(&self.registry, Archetypes::Shared(&self.state.archetypes))
    }

    /// Starts a query that may write components.
    ///
    /// # Panics
    ///
    /// Panics when a component is not registered or appears twice in `D`.
    #[must_use]
    pub fn query_mut<D: QueryData>(&mut self) -> WorldQuery<'_, D> {
        WorldQuery::new(
            &self.registry,
            Archetypes::Exclusive(&mut self.state.archetypes),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::{QueryFilter, With, Without};
    use crate::{Component, Entity, World};

    #[derive(Clone, Copy, Debug, PartialEq)]
    struct Position(i32);
    impl Component for Position {}

    #[derive(Clone, Copy, Debug, PartialEq)]
    struct Velocity(i32);
    impl Component for Velocity {}

    #[derive(Clone, Copy, Debug, PartialEq)]
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
    fn query_mut_writes_across_archetypes() {
        let mut world = world();
        let a = world.spawn((Position(1), Velocity(10)));
        let b = world.spawn((Position(2), Velocity(20), Frozen));
        let c = world.spawn((Position(3), Velocity(30)));

        let mut visited = Vec::new();
        for (entity, position, velocity) in world
            .query_mut::<(Entity, &mut Position, &Velocity)>()
            .iter()
        {
            visited.push(entity);
            position.0 += velocity.0;
        }

        assert_eq!(visited, [a, c, b]);
        assert_eq!(world.get::<Position>(a), Some(&Position(11)));
        assert_eq!(world.get::<Position>(b), Some(&Position(22)));
        assert_eq!(world.get::<Position>(c), Some(&Position(33)));
    }

    #[test]
    fn yields_matching_entities_in_archetype_then_row_order() {
        let mut world = world();
        let moving = world.spawn((Position(1), Velocity(1)));
        let still = world.spawn(Position(2));
        let other_moving = world.spawn((Position(3), Velocity(3)));

        let entities: Vec<Entity> = world
            .query::<(Entity, &Position)>()
            .iter()
            .map(|(entity, _)| entity)
            .collect();

        assert_eq!(entities, [moving, other_moving, still]);
    }

    #[test]
    fn skips_archetypes_missing_a_component() {
        let mut world = world();
        world.spawn(Position(1));
        let moving = world.spawn((Position(2), Velocity(5)));

        let items: Vec<_> = world.query::<(Entity, &Velocity)>().iter().collect();

        assert_eq!(items, [(moving, &Velocity(5))]);
    }

    #[test]
    fn with_and_without_filter_on_other_components() {
        let mut world = world();
        let frozen = world.spawn((Position(1), Frozen));
        let free = world.spawn(Position(2));

        let with: Vec<Entity> = world.query::<Entity>().with::<Frozen>().iter().collect();
        let without: Vec<Entity> = world
            .query::<Entity>()
            .with::<Position>()
            .without::<Frozen>()
            .iter()
            .collect();

        assert_eq!(with, [frozen]);
        assert_eq!(without, [free]);
    }

    #[test]
    fn a_mutable_query_writes_components() {
        let mut world = world();
        let entity = world.spawn((Position(1), Velocity(2)));

        for (position, velocity) in world.query_mut::<(&mut Position, &Velocity)>().iter() {
            position.0 += velocity.0;
        }

        assert_eq!(world.get::<Position>(entity), Some(&Position(3)));
    }

    #[test]
    #[should_panic(expected = "Position appears twice in the query")]
    fn a_query_accessing_a_component_twice_panics() {
        let mut world = world();
        let _ = world.query_mut::<(&mut Position, &Position)>();
    }

    #[test]
    fn filtering_out_a_queried_component_yields_nothing() {
        let mut world = world();
        world.spawn(Position(1));

        assert_eq!(
            world
                .query::<&Position>()
                .without::<Position>()
                .iter()
                .count(),
            0
        );
    }

    #[test]
    fn requiring_a_queried_component_again_changes_nothing() {
        let mut world = world();
        let entity = world.spawn(Position(1));

        let entities: Vec<Entity> = world
            .query::<(Entity, &Position)>()
            .with::<Position>()
            .iter()
            .map(|(entity, _)| entity)
            .collect();

        assert_eq!(entities, [entity]);
    }

    #[test]
    fn typed_filters_push_required_and_excluded_components() {
        let world = world();
        let mut required = Vec::new();
        let mut excluded = Vec::new();

        <(With<Velocity>, Without<Frozen>)>::filter_ids(
            &world.registry,
            &mut required,
            &mut excluded,
        );

        assert_eq!(required, [world.registry.id::<Velocity>()]);
        assert_eq!(excluded, [world.registry.id::<Frozen>()]);
    }
}
