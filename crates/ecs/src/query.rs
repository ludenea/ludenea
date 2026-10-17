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

/// What a query yields for each entity: [`Entity`], `&T`, `&mut T`, or a
/// tuple of up to eight of them.
pub trait QueryData {
    /// The value yielded for one entity.
    type Item<'w>;

    #[doc(hidden)]
    type Fetch<'w>: Iterator<Item = Self::Item<'w>>;

    /// Pushes the identifier of every component accessed, in query order.
    #[doc(hidden)]
    fn component_ids(registry: &Registry, ids: &mut Vec<ComponentId>);

    /// Takes the columns this query reads from `columns`, indexed like
    /// `key`, and iterates over the rows of one archetype.
    #[doc(hidden)]
    fn fetch<'w>(
        registry: &Registry,
        key: &[ComponentId],
        entities: &'w [Entity],
        columns: &mut [Option<ColumnBorrow<'w>>],
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

/// Takes the column of `T` out of `columns` for one query element.
fn take_column<'w, T: Component>(
    registry: &Registry,
    key: &[ComponentId],
    columns: &mut [Option<ColumnBorrow<'w>>],
) -> ColumnBorrow<'w> {
    let position = key
        .binary_search(&registry.id::<T>())
        .expect("a matched archetype has every component of the query");
    columns[position]
        .take()
        .expect("a query accesses each component once")
}

impl QueryData for Entity {
    type Item<'w> = Entity;
    type Fetch<'w> = Copied<slice::Iter<'w, Entity>>;

    fn component_ids(_: &Registry, _: &mut Vec<ComponentId>) {}

    fn fetch<'w>(
        _: &Registry,
        _: &[ComponentId],
        entities: &'w [Entity],
        _: &mut [Option<ColumnBorrow<'w>>],
    ) -> Self::Fetch<'w> {
        entities.iter().copied()
    }
}

impl ReadOnlyQuery for Entity {}
impl sealed::Sealed for Entity {}

impl<T: Component> QueryData for &T {
    type Item<'w> = &'w T;
    type Fetch<'w> = slice::Iter<'w, T>;

    fn component_ids(registry: &Registry, ids: &mut Vec<ComponentId>) {
        ids.push(registry.id::<T>());
    }

    fn fetch<'w>(
        registry: &Registry,
        key: &[ComponentId],
        _: &'w [Entity],
        columns: &mut [Option<ColumnBorrow<'w>>],
    ) -> Self::Fetch<'w> {
        let column: &'w dyn Column = match take_column::<T>(registry, key, columns) {
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

    fn component_ids(registry: &Registry, ids: &mut Vec<ComponentId>) {
        ids.push(registry.id::<T>());
    }

    fn fetch<'w>(
        registry: &Registry,
        key: &[ComponentId],
        _: &'w [Entity],
        columns: &mut [Option<ColumnBorrow<'w>>],
    ) -> Self::Fetch<'w> {
        match take_column::<T>(registry, key, columns) {
            ColumnBorrow::Exclusive(column) => downcast_mut::<T>(column).iter_mut(),
            ColumnBorrow::Shared(_) => {
                unreachable!("a mutable query element only runs on an exclusive world")
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

            fn component_ids(registry: &Registry, ids: &mut Vec<ComponentId>) {
                $($element::component_ids(registry, ids);)+
            }

            fn fetch<'w>(
                registry: &Registry,
                key: &[ComponentId],
                entities: &'w [Entity],
                columns: &mut [Option<ColumnBorrow<'w>>],
            ) -> Self::Fetch<'w> {
                Zip(($($element::fetch(registry, key, entities, columns),)+))
            }
        }

        impl<$($element: ReadOnlyQuery),+> ReadOnlyQuery for ($($element,)+) {}
        impl<$($element: ReadOnlyQuery),+> sealed::Sealed for ($($element,)+) {}

        impl<$($element: Iterator),+> Iterator for Zip<($($element,)+)> {
            type Item = ($($element::Item,)+);

            fn next(&mut self) -> Option<Self::Item> {
                Some(($(self.0.$index.next()?,)+))
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

enum Archetypes<'w> {
    Shared(&'w [Archetype]),
    Exclusive(&'w mut [Archetype]),
}

/// A query over a world, refined with [`Query::with`] and
/// [`Query::without`], then consumed by [`Query::iter`].
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
pub struct Query<'w, D: QueryData> {
    registry: &'w Registry,
    archetypes: Archetypes<'w>,
    required: Vec<ComponentId>,
    without: Vec<ComponentId>,
    data: PhantomData<D>,
}

impl<'w, D: QueryData> Query<'w, D> {
    /// # Panics
    ///
    /// Panics when a component is not registered or appears twice in `D`.
    fn new(registry: &'w Registry, archetypes: Archetypes<'w>) -> Self {
        let mut required = Vec::new();
        D::component_ids(registry, &mut required);
        let required = registry.sorted_without_duplicates(required, "query");
        Self {
            registry,
            archetypes,
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
    // ponytail: matches every archetype on each call, cache the matches in a
    // query state when the scheduler needs to know the access of systems.
    pub fn iter(self) -> impl Iterator<Item = D::Item<'w>> + use<'w, D> {
        let Self {
            registry,
            archetypes,
            required,
            without,
            ..
        } = self;
        let matches = |key: &[ComponentId]| {
            required.iter().all(|id| key.binary_search(id).is_ok())
                && without.iter().all(|id| key.binary_search(id).is_err())
        };
        let fetches: Vec<D::Fetch<'w>> = match archetypes {
            Archetypes::Shared(archetypes) => archetypes
                .iter()
                .map(Archetype::parts)
                .filter(|(key, _, _)| matches(key))
                .map(|(key, entities, columns)| {
                    let mut columns: Vec<_> = columns
                        .iter()
                        .map(|column| Some(ColumnBorrow::Shared(column.as_ref())))
                        .collect();
                    D::fetch(registry, key, entities, &mut columns)
                })
                .collect(),
            Archetypes::Exclusive(archetypes) => archetypes
                .iter_mut()
                .map(Archetype::parts_mut)
                .filter(|(key, _, _)| matches(key))
                .map(|(key, entities, columns)| {
                    let mut columns: Vec<_> = columns
                        .iter_mut()
                        .map(|column| Some(ColumnBorrow::Exclusive(column.as_mut())))
                        .collect();
                    D::fetch(registry, key, entities, &mut columns)
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
    pub fn query<D: ReadOnlyQuery>(&self) -> Query<'_, D> {
        Query::new(&self.registry, Archetypes::Shared(&self.state.archetypes))
    }

    /// Starts a query that may write components.
    ///
    /// # Panics
    ///
    /// Panics when a component is not registered or appears twice in `D`.
    #[must_use]
    pub fn query_mut<D: QueryData>(&mut self) -> Query<'_, D> {
        Query::new(
            &self.registry,
            Archetypes::Exclusive(&mut self.state.archetypes),
        )
    }
}

#[cfg(test)]
mod tests {
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
}
