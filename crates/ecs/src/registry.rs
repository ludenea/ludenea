//! Component registration, frozen when the world is built.

use std::any::{TypeId, type_name};
use std::collections::BTreeMap;
use std::mem::{align_of, size_of};

use crate::Component;
use crate::column::Column;

/// Stable identifier of a registered component: its registration order.
/// Unlike `TypeId`, it is identical on every platform and build, so it can
/// decide an order.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct ComponentId(u32);

impl ComponentId {
    fn index(self) -> usize {
        self.0 as usize
    }
}

/// What a snapshot checks to make sure it comes from the same binary.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Fingerprint {
    name: &'static str,
    size: usize,
    align: usize,
}

struct ComponentInfo {
    fingerprint: Fingerprint,
    new_column: fn() -> Box<dyn Column>,
}

/// The components a world knows, in registration order.
#[derive(Default)]
pub struct Registry {
    components: Vec<ComponentInfo>,
    /// Lookup only: `TypeId` order never decides an iteration order.
    ids: BTreeMap<TypeId, ComponentId>,
}

impl Registry {
    /// Registers `T`; registering it again keeps its first identifier.
    pub(crate) fn register<T: Component>(&mut self) {
        let next = ComponentId(
            u32::try_from(self.components.len()).expect("component identifiers are exhausted"),
        );
        self.ids.entry(TypeId::of::<T>()).or_insert_with(|| {
            self.components.push(ComponentInfo {
                fingerprint: Fingerprint {
                    name: type_name::<T>(),
                    size: size_of::<T>(),
                    align: align_of::<T>(),
                },
                new_column: || Box::new(Vec::<T>::new()),
            });
            next
        });
    }

    /// # Panics
    ///
    /// Panics when `T` is not registered.
    pub(crate) fn id<T: Component>(&self) -> ComponentId {
        *self
            .ids
            .get(&TypeId::of::<T>())
            .unwrap_or_else(|| panic!("component {} is not registered", type_name::<T>()))
    }

    pub(crate) fn name(&self, id: ComponentId) -> &'static str {
        self.components[id.index()].fingerprint.name
    }

    /// Sorts the component identifiers accessed by a `context` (a bundle or
    /// a query).
    ///
    /// # Panics
    ///
    /// Panics when a component appears twice.
    pub(crate) fn sorted_without_duplicates(
        &self,
        mut ids: Vec<ComponentId>,
        context: &str,
    ) -> Vec<ComponentId> {
        ids.sort_unstable();
        if let Some(pair) = ids.windows(2).find(|pair| pair[0] == pair[1]) {
            panic!(
                "component {} appears twice in the {context}",
                self.name(pair[0])
            );
        }
        ids
    }

    pub(crate) fn new_column(&self, id: ComponentId) -> Box<dyn Column> {
        (self.components[id.index()].new_column)()
    }

    pub(crate) fn fingerprint(&self) -> Vec<Fingerprint> {
        self.components
            .iter()
            .map(|info| info.fingerprint)
            .collect()
    }

    pub(crate) fn has_fingerprint(&self, fingerprint: &[Fingerprint]) -> bool {
        self.components
            .iter()
            .map(|info| &info.fingerprint)
            .eq(fingerprint)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Clone, Copy)]
    struct Position;
    impl Component for Position {}

    #[derive(Clone, Copy)]
    struct Velocity;
    impl Component for Velocity {}

    #[test]
    fn identifiers_follow_registration_order() {
        let mut registry = Registry::default();

        registry.register::<Velocity>();
        registry.register::<Position>();

        assert_eq!(registry.id::<Velocity>(), ComponentId(0));
        assert_eq!(registry.id::<Position>(), ComponentId(1));
    }

    #[test]
    fn registering_twice_keeps_the_first_identifier() {
        let mut registry = Registry::default();

        registry.register::<Position>();
        registry.register::<Velocity>();
        registry.register::<Position>();

        assert_eq!(registry.id::<Position>(), ComponentId(0));
        assert_eq!(registry.fingerprint().len(), 2);
    }

    #[test]
    #[should_panic(expected = "is not registered")]
    fn an_unregistered_component_panics() {
        Registry::default().id::<Position>();
    }
}
