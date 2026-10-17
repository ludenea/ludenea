//! Resources: values a world holds exactly once, outside any entity.

use std::fmt;

use crate::column::{Column, downcast_mut, downcast_ref};
use crate::registry::ResourceId;

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
