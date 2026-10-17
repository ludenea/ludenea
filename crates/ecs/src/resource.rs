//! Resources: values a world holds exactly once, outside any entity.

use std::any::type_name;
use std::fmt;
use std::ops::{Deref, DerefMut};

use crate::World;
use crate::access::Access;
use crate::column::{Column, downcast_mut, downcast_ref};
use crate::query::ColumnBorrow;
use crate::registry::ResourceId;
use crate::system::{SystemParam, WorldParts};

/// Marker for the types a [`World`](crate::World) stores as resources.
///
/// Like [`Component`](crate::Component), `Copy` is what makes a snapshot a
/// memory copy. Implement it explicitly:
///
/// ```
/// use ludenea_ecs::Resource;
///
/// #[derive(Clone, Copy)]
/// struct Gravity(i32);
///
/// impl Resource for Gravity {}
/// ```
pub trait Resource: Copy + 'static {}

/// One column of length one per resource, indexed by [`ResourceId`].
#[derive(Default)]
pub(crate) struct Resources {
    columns: Vec<Box<dyn Column>>,
}

impl Resources {
    /// Stores `value` under `id`, the next identifier or an existing one.
    pub(crate) fn insert<T: Resource>(&mut self, id: ResourceId, value: T) {
        if id.index() == self.columns.len() {
            self.columns.push(Box::new(vec![value]));
        } else {
            downcast_mut::<T>(self.columns[id.index()].as_mut())[0] = value;
        }
    }

    pub(crate) fn get<T: Resource>(&self, id: ResourceId) -> &T {
        &downcast_ref::<T>(self.columns[id.index()].as_ref())[0]
    }

    pub(crate) fn columns_mut(&mut self) -> &mut [Box<dyn Column>] {
        &mut self.columns
    }

    pub(crate) fn get_mut<T: Resource>(&mut self, id: ResourceId) -> &mut T {
        &mut downcast_mut::<T>(self.columns[id.index()].as_mut())[0]
    }
}

impl Clone for Resources {
    fn clone(&self) -> Self {
        Self {
            columns: self.columns.clone(),
        }
    }

    fn clone_from(&mut self, source: &Self) {
        self.columns.clone_from(&source.columns);
    }
}

/// Shows how many resources there are: resources need not be `Debug`.
impl fmt::Debug for Resources {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Resources")
            .field("count", &self.columns.len())
            .finish_non_exhaustive()
    }
}

/// A system parameter reading the resource `T`.
pub struct Res<'w, T: Resource> {
    value: &'w T,
}

impl<T: Resource> Deref for Res<'_, T> {
    type Target = T;

    fn deref(&self) -> &T {
        self.value
    }
}

/// A system parameter writing the resource `T`.
pub struct ResMut<'w, T: Resource> {
    value: &'w mut T,
}

impl<T: Resource> Deref for ResMut<'_, T> {
    type Target = T;

    fn deref(&self) -> &T {
        self.value
    }
}

impl<T: Resource> DerefMut for ResMut<'_, T> {
    fn deref_mut(&mut self) -> &mut T {
        self.value
    }
}

/// Declares a resource access of `system`.
///
/// # Panics
///
/// Panics, naming `system` and `T`, when the resource is not inserted.
fn init_resource<T: Resource>(
    world: &World,
    access: &mut Access,
    system: &'static str,
    write: bool,
) -> ResourceId {
    let id = world.registry.find_resource_id::<T>().unwrap_or_else(|| {
        panic!(
            "system {system} accesses resource {} which is not inserted",
            type_name::<T>()
        )
    });
    access.add_resource(id, write, &world.registry, system);
    id
}

impl<T: Resource> SystemParam for Res<'_, T> {
    type State = ResourceId;
    type Item<'w, 's> = Res<'w, T>;

    fn init(world: &World, access: &mut Access, system: &'static str) -> ResourceId {
        init_resource::<T>(world, access, system, false)
    }

    fn get<'w>(id: &mut ResourceId, parts: &mut WorldParts<'w>) -> Res<'w, T> {
        let ColumnBorrow::Shared(column) = parts.resource(*id, false) else {
            unreachable!("a read resource is lent shared");
        };
        Res {
            value: &downcast_ref::<T>(column)[0],
        }
    }
}

impl<T: Resource> SystemParam for ResMut<'_, T> {
    type State = ResourceId;
    type Item<'w, 's> = ResMut<'w, T>;

    fn init(world: &World, access: &mut Access, system: &'static str) -> ResourceId {
        init_resource::<T>(world, access, system, true)
    }

    fn get<'w>(id: &mut ResourceId, parts: &mut WorldParts<'w>) -> ResMut<'w, T> {
        let ColumnBorrow::Exclusive(column) = parts.resource(*id, true) else {
            unreachable!("a written resource is lent exclusively");
        };
        ResMut {
            value: &mut downcast_mut::<T>(column)[0],
        }
    }
}
