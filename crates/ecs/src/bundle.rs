//! Sets of components spawned or inserted together.

use crate::Component;
use crate::archetype::Archetype;
use crate::registry::{ComponentId, Registry};

/// A set of components spawned or inserted together: `()`, one component,
/// or a tuple of up to eight components.
pub trait Bundle: 'static {
    /// Pushes the identifier of every component, in bundle order.
    #[doc(hidden)]
    fn component_ids(registry: &Registry, ids: &mut Vec<ComponentId>);

    /// Writes every component at `row` of `archetype`, overwriting a value
    /// already there or pushing a new one.
    #[doc(hidden)]
    fn write(self, registry: &Registry, archetype: &mut Archetype, row: usize);
}

impl Bundle for () {
    fn component_ids(_: &Registry, _: &mut Vec<ComponentId>) {}

    fn write(self, _: &Registry, _: &mut Archetype, _: usize) {}
}

impl<T: Component> Bundle for T {
    fn component_ids(registry: &Registry, ids: &mut Vec<ComponentId>) {
        ids.push(registry.id::<T>());
    }

    fn write(self, registry: &Registry, archetype: &mut Archetype, row: usize) {
        let column = archetype
            .column_mut::<T>(registry.id::<T>())
            .expect("the archetype of a bundle has a column for each of its components");
        if row < column.len() {
            column[row] = self;
        } else {
            column.push(self);
        }
    }
}

macro_rules! impl_bundle_for_tuple {
    ($($component:ident $index:tt),+) => {
        impl<$($component: Component),+> Bundle for ($($component,)+) {
            fn component_ids(registry: &Registry, ids: &mut Vec<ComponentId>) {
                $($component::component_ids(registry, ids);)+
            }

            fn write(self, registry: &Registry, archetype: &mut Archetype, row: usize) {
                $(self.$index.write(registry, archetype, row);)+
            }
        }
    };
}

impl_bundle_for_tuple!(A 0);
impl_bundle_for_tuple!(A 0, B 1);
impl_bundle_for_tuple!(A 0, B 1, C 2);
impl_bundle_for_tuple!(A 0, B 1, C 2, D 3);
impl_bundle_for_tuple!(A 0, B 1, C 2, D 3, E 4);
impl_bundle_for_tuple!(A 0, B 1, C 2, D 3, E 4, F 5);
impl_bundle_for_tuple!(A 0, B 1, C 2, D 3, E 4, F 5, G 6);
impl_bundle_for_tuple!(A 0, B 1, C 2, D 3, E 4, F 5, G 6, H 7);
