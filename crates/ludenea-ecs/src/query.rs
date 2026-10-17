//! Typed queries over component storages. A query borrows every storage it touches for
//! its whole lifetime and iterates matching entities in ascending index order.

use std::cell::{Ref, RefMut};
use std::marker::PhantomData;

use crate::{Component, EcsError, EntitiesState, Entity, World};

/// One storage access declared by a query: `Read` for `&T`, `Option<&T>`, and filters,
/// `Write` for `&mut T` and `Option<&mut T>`. Every accessed name must be registered.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Access {
    Read(&'static str),
    Write(&'static str),
}

impl Access {
    pub fn name(self) -> &'static str {
        match self {
            Access::Read(name) | Access::Write(name) => name,
        }
    }
}

/// Every access of a query with data `D` and filter `F`, data first.
pub fn accesses<D: QueryData, F: QueryFilter>() -> Vec<Access> {
    let mut accesses = Vec::new();
    D::accesses(&mut accesses);
    F::accesses(&mut accesses);
    accesses
}

/// Rejects two accesses to the same name when at least one writes, returning that name
/// and the positions of the first conflicting pair.
pub fn check_accesses(accesses: &[Access]) -> Result<(), (&'static str, usize, usize)> {
    for (first, a) in accesses.iter().enumerate() {
        for (second, b) in accesses.iter().enumerate().skip(first + 1) {
            let writes = matches!(a, Access::Write(_)) || matches!(b, Access::Write(_));
            if writes && a.name() == b.name() {
                return Err((a.name(), first, second));
            }
        }
    }
    Ok(())
}

#[derive(Debug, PartialEq, Eq, thiserror::Error)]
pub enum QueryError {
    #[error("entity {0} does not exist or was despawned; check `World::is_alive` before using it")]
    NoSuchEntity(Entity),
    #[error(
        "entity {0} lacks a component this query requires or is excluded by its filter; use `Option<&T>` for optional components"
    )]
    DoesNotMatch(Entity),
}

/// What a query yields per entity. `State` holds the storage borrows for the lifetime of
/// the query.
pub trait QueryData {
    type Item<'a>;
    type State<'w>;

    fn accesses(accesses: &mut Vec<Access>);

    fn fetch(world: &World) -> Result<Self::State<'_>, EcsError>;

    /// Whether `entity`, which is alive, has every component this data requires.
    fn matches(state: &Self::State<'_>, entity: Entity) -> bool;

    /// # Safety
    /// `matches(state, entity)` is true, and unless `Self: ReadOnlyQueryData`, no other
    /// item fetched for `entity` from `state` is alive.
    unsafe fn get<'a>(state: &'a Self::State<'_>, entity: Entity) -> Self::Item<'a>;
}

/// # Safety
/// Implementors only read: items fetched for the same entity may alias.
pub unsafe trait ReadOnlyQueryData: QueryData {
    type ReadItem<'a>;

    fn into_read(item: Self::Item<'_>) -> Self::ReadItem<'_>;
}

/// Selects entities without yielding anything.
pub trait QueryFilter {
    type State<'w>;

    fn accesses(accesses: &mut Vec<Access>);

    fn fetch(world: &World) -> Result<Self::State<'_>, EcsError>;

    fn matches(state: &Self::State<'_>, entity: Entity) -> bool;
}

/// Keeps entities that have a `T`.
pub struct With<T>(PhantomData<T>);

/// Keeps entities that have no `T`.
pub struct Without<T>(PhantomData<T>);

type Slots<T> = Vec<Option<T>>;

fn borrow<T: Component>(world: &World) -> Result<Ref<'_, Slots<T>>, EcsError> {
    world
        .component_cell::<T>()?
        .try_borrow()
        .map_err(|_| EcsError::ComponentBorrowed(T::NAME))
}

fn has<T>(slots: &[Option<T>], entity: Entity) -> bool {
    matches!(slots.get(entity.index() as usize), Some(Some(_)))
}

/// A mutable storage borrow. `base` is derived once from the guard, which is never
/// touched again, so items for distinct indices never invalidate one another.
pub struct WriteState<'w, T> {
    _guard: RefMut<'w, Slots<T>>,
    base: *mut Option<T>,
    len: usize,
}

impl<T: Component> WriteState<'_, T> {
    fn new(world: &World) -> Result<WriteState<'_, T>, EcsError> {
        let mut guard = world
            .component_cell::<T>()?
            .try_borrow_mut()
            .map_err(|_| EcsError::ComponentBorrowed(T::NAME))?;
        let base = guard.as_mut_ptr();
        let len = guard.len();
        Ok(WriteState {
            _guard: guard,
            base,
            len,
        })
    }

    /// A pointer to slot `entity.index()`, or `None` past the end of the storage.
    fn slot(&self, entity: Entity) -> Option<*mut Option<T>> {
        let index = entity.index() as usize;
        // SAFETY: `index < len`, so the offset stays inside the buffer the guard keeps
        // borrowed and unchanged.
        (index < self.len).then(|| unsafe { self.base.add(index) })
    }
}

impl<T: Component> QueryData for &T {
    type Item<'a> = &'a T;
    type State<'w> = Ref<'w, Slots<T>>;

    fn accesses(accesses: &mut Vec<Access>) {
        accesses.push(Access::Read(T::NAME));
    }

    fn fetch(world: &World) -> Result<Self::State<'_>, EcsError> {
        borrow::<T>(world)
    }

    fn matches(state: &Self::State<'_>, entity: Entity) -> bool {
        has(state, entity)
    }

    unsafe fn get<'a>(state: &'a Self::State<'_>, entity: Entity) -> Self::Item<'a> {
        state[entity.index() as usize]
            .as_ref()
            .expect("query item fetched for a non-matching entity")
    }
}

// SAFETY: yields shared references only.
unsafe impl<T: Component> ReadOnlyQueryData for &T {
    type ReadItem<'a> = &'a T;

    fn into_read(item: Self::Item<'_>) -> Self::ReadItem<'_> {
        item
    }
}

impl<T: Component> QueryData for &mut T {
    type Item<'a> = &'a mut T;
    type State<'w> = WriteState<'w, T>;

    fn accesses(accesses: &mut Vec<Access>) {
        accesses.push(Access::Write(T::NAME));
    }

    fn fetch(world: &World) -> Result<Self::State<'_>, EcsError> {
        WriteState::new(world)
    }

    fn matches(state: &Self::State<'_>, entity: Entity) -> bool {
        // SAFETY: the caller of `matches` holds no item for `entity` (it asks before
        // fetching one), and this shared read ends before returning.
        state
            .slot(entity)
            .is_some_and(|slot| unsafe { (*slot).is_some() })
    }

    unsafe fn get<'a>(state: &'a Self::State<'_>, entity: Entity) -> Self::Item<'a> {
        // SAFETY: the caller guarantees no other live item for `entity` from `state`.
        state
            .slot(entity)
            .and_then(|slot| unsafe { (*slot).as_mut() })
            .expect("query item fetched for a non-matching entity")
    }
}

impl<T: Component> QueryData for Option<&T> {
    type Item<'a> = Option<&'a T>;
    type State<'w> = Ref<'w, Slots<T>>;

    fn accesses(accesses: &mut Vec<Access>) {
        accesses.push(Access::Read(T::NAME));
    }

    fn fetch(world: &World) -> Result<Self::State<'_>, EcsError> {
        borrow::<T>(world)
    }

    fn matches(_: &Self::State<'_>, _: Entity) -> bool {
        true
    }

    unsafe fn get<'a>(state: &'a Self::State<'_>, entity: Entity) -> Self::Item<'a> {
        state.get(entity.index() as usize)?.as_ref()
    }
}

// SAFETY: yields shared references only.
unsafe impl<T: Component> ReadOnlyQueryData for Option<&T> {
    type ReadItem<'a> = Option<&'a T>;

    fn into_read(item: Self::Item<'_>) -> Self::ReadItem<'_> {
        item
    }
}

impl<T: Component> QueryData for Option<&mut T> {
    type Item<'a> = Option<&'a mut T>;
    type State<'w> = WriteState<'w, T>;

    fn accesses(accesses: &mut Vec<Access>) {
        accesses.push(Access::Write(T::NAME));
    }

    fn fetch(world: &World) -> Result<Self::State<'_>, EcsError> {
        WriteState::new(world)
    }

    fn matches(_: &Self::State<'_>, _: Entity) -> bool {
        true
    }

    unsafe fn get<'a>(state: &'a Self::State<'_>, entity: Entity) -> Self::Item<'a> {
        // SAFETY: the caller guarantees no other live item for `entity` from `state`.
        state
            .slot(entity)
            .and_then(|slot| unsafe { (*slot).as_mut() })
    }
}

impl QueryData for Entity {
    type Item<'a> = Entity;
    type State<'w> = ();

    fn accesses(_: &mut Vec<Access>) {}

    fn fetch(_: &World) -> Result<(), EcsError> {
        Ok(())
    }

    fn matches(_: &(), _: Entity) -> bool {
        true
    }

    unsafe fn get(_: &(), entity: Entity) -> Entity {
        entity
    }
}

// SAFETY: yields a copied handle.
unsafe impl ReadOnlyQueryData for Entity {
    type ReadItem<'a> = Entity;

    fn into_read(item: Self::Item<'_>) -> Self::ReadItem<'_> {
        item
    }
}

macro_rules! tuple_data {
    ($(($name:ident, $index:tt)),+) => {
        impl<$($name: QueryData),+> QueryData for ($($name,)+) {
            type Item<'a> = ($($name::Item<'a>,)+);
            type State<'w> = ($($name::State<'w>,)+);

            fn accesses(accesses: &mut Vec<Access>) {
                $($name::accesses(accesses);)+
            }

            fn fetch(world: &World) -> Result<Self::State<'_>, EcsError> {
                Ok(($($name::fetch(world)?,)+))
            }

            fn matches(state: &Self::State<'_>, entity: Entity) -> bool {
                $($name::matches(&state.$index, entity))&&+
            }

            unsafe fn get<'a>(state: &'a Self::State<'_>, entity: Entity) -> Self::Item<'a> {
                // SAFETY: every element matches since the tuple does, and the storages are
                // distinct because `fetch` rejects a storage borrowed twice with a write.
                unsafe { ($($name::get(&state.$index, entity),)+) }
            }
        }

        // SAFETY: every element is read-only.
        unsafe impl<$($name: ReadOnlyQueryData),+> ReadOnlyQueryData for ($($name,)+) {
            type ReadItem<'a> = ($($name::ReadItem<'a>,)+);

            fn into_read(item: Self::Item<'_>) -> Self::ReadItem<'_> {
                ($($name::into_read(item.$index),)+)
            }
        }
    };
}

tuple_data!((A, 0));
tuple_data!((A, 0), (B, 1));
tuple_data!((A, 0), (B, 1), (C, 2));
tuple_data!((A, 0), (B, 1), (C, 2), (D, 3));
tuple_data!((A, 0), (B, 1), (C, 2), (D, 3), (E, 4));
tuple_data!((A, 0), (B, 1), (C, 2), (D, 3), (E, 4), (F, 5));
tuple_data!((A, 0), (B, 1), (C, 2), (D, 3), (E, 4), (F, 5), (G, 6));
tuple_data!(
    (A, 0),
    (B, 1),
    (C, 2),
    (D, 3),
    (E, 4),
    (F, 5),
    (G, 6),
    (H, 7)
);

impl QueryFilter for () {
    type State<'w> = ();

    fn accesses(_: &mut Vec<Access>) {}

    fn fetch(_: &World) -> Result<(), EcsError> {
        Ok(())
    }

    fn matches(_: &(), _: Entity) -> bool {
        true
    }
}

impl<T: Component> QueryFilter for With<T> {
    type State<'w> = Ref<'w, Slots<T>>;

    fn accesses(accesses: &mut Vec<Access>) {
        accesses.push(Access::Read(T::NAME));
    }

    fn fetch(world: &World) -> Result<Self::State<'_>, EcsError> {
        borrow::<T>(world)
    }

    fn matches(state: &Self::State<'_>, entity: Entity) -> bool {
        has(state, entity)
    }
}

impl<T: Component> QueryFilter for Without<T> {
    type State<'w> = Ref<'w, Slots<T>>;

    fn accesses(accesses: &mut Vec<Access>) {
        accesses.push(Access::Read(T::NAME));
    }

    fn fetch(world: &World) -> Result<Self::State<'_>, EcsError> {
        borrow::<T>(world)
    }

    fn matches(state: &Self::State<'_>, entity: Entity) -> bool {
        !has(state, entity)
    }
}

macro_rules! tuple_filter {
    ($(($name:ident, $index:tt)),+) => {
        impl<$($name: QueryFilter),+> QueryFilter for ($($name,)+) {
            type State<'w> = ($($name::State<'w>,)+);

            fn accesses(accesses: &mut Vec<Access>) {
                $($name::accesses(accesses);)+
            }

            fn fetch(world: &World) -> Result<Self::State<'_>, EcsError> {
                Ok(($($name::fetch(world)?,)+))
            }

            fn matches(state: &Self::State<'_>, entity: Entity) -> bool {
                $($name::matches(&state.$index, entity))&&+
            }
        }
    };
}

tuple_filter!((A, 0));
tuple_filter!((A, 0), (B, 1));
tuple_filter!((A, 0), (B, 1), (C, 2));
tuple_filter!((A, 0), (B, 1), (C, 2), (D, 3));

/// Entities with the components of `D` that pass `F`, borrowed from a world.
pub struct Query<'w, D: QueryData + 'static, F: QueryFilter + 'static = ()> {
    entities: &'w EntitiesState,
    data: D::State<'w>,
    filter: F::State<'w>,
}

impl<'w, D: QueryData + 'static, F: QueryFilter + 'static> Query<'w, D, F> {
    /// Borrows every storage of `D` and `F`. Fails when one is unregistered or already
    /// borrowed incompatibly, including twice by this query with a write.
    pub fn new(world: &'w World) -> Result<Self, EcsError> {
        Ok(Self {
            entities: &world.entities,
            data: D::fetch(world)?,
            filter: F::fetch(world)?,
        })
    }

    fn matches(&self, entity: Entity) -> bool {
        D::matches(&self.data, entity) && F::matches(&self.filter, entity)
    }

    fn check(&self, entity: Entity) -> Result<(), QueryError> {
        if !self.entities.is_alive(entity) {
            Err(QueryError::NoSuchEntity(entity))
        } else if !self.matches(entity) {
            Err(QueryError::DoesNotMatch(entity))
        } else {
            Ok(())
        }
    }

    pub fn iter(&self) -> impl Iterator<Item = D::ReadItem<'_>>
    where
        D: ReadOnlyQueryData,
    {
        self.entities
            .iter()
            .filter(|&entity| self.matches(entity))
            // SAFETY: the entity matches and `D` is read-only, so items may alias.
            .map(|entity| D::into_read(unsafe { D::get(&self.data, entity) }))
    }

    pub fn get(&self, entity: Entity) -> Result<D::ReadItem<'_>, QueryError>
    where
        D: ReadOnlyQueryData,
    {
        self.check(entity)?;
        // SAFETY: the entity matches and `D` is read-only, so items may alias.
        Ok(D::into_read(unsafe { D::get(&self.data, entity) }))
    }

    pub fn iter_mut(&mut self) -> impl Iterator<Item = D::Item<'_>> {
        let data = &self.data;
        let filter = &self.filter;
        self.entities
            .iter()
            .filter(move |&entity| D::matches(data, entity) && F::matches(filter, entity))
            // SAFETY: the entity matches, each alive index is visited once, and `&mut self`
            // keeps any other item from this query out of reach while these are alive.
            .map(move |entity| unsafe { D::get(data, entity) })
    }

    pub fn get_mut(&mut self, entity: Entity) -> Result<D::Item<'_>, QueryError> {
        self.check(entity)?;
        // SAFETY: the entity matches, and `&mut self` keeps any other item from this
        // query out of reach while this one is alive.
        Ok(unsafe { D::get(&self.data, entity) })
    }
}
