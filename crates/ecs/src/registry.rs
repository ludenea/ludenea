//! Component and resource registration, frozen when the world is built.

use std::any::{TypeId, type_name};
use std::collections::BTreeMap;
use std::mem::{align_of, size_of};

use crate::column::Column;
use crate::{Component, Resource};

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

/// Stable identifier of an inserted resource: its insertion order.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct ResourceId(u32);

impl ResourceId {
    pub(crate) fn index(self) -> usize {
        self.0 as usize
    }
}

/// Identifies one type well enough to tell two binaries apart.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Fingerprint {
    name: &'static str,
    size: usize,
    align: usize,
}

impl Fingerprint {
    fn of<T>() -> Self {
        Self {
            name: type_name::<T>(),
            size: size_of::<T>(),
            align: align_of::<T>(),
        }
    }
}

/// What a snapshot or a schedule checks to make sure a world has the same
/// components and resources, in the same order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct RegistryFingerprint {
    components: Vec<Fingerprint>,
    resources: Vec<Fingerprint>,
}

struct ComponentInfo {
    fingerprint: Fingerprint,
    new_column: fn() -> Box<dyn Column>,
}

/// The components and resources a world knows, in registration order.
#[derive(Default)]
pub struct Registry {
    components: Vec<ComponentInfo>,
    /// Lookup only: `TypeId` order never decides an iteration order.
    ids: BTreeMap<TypeId, ComponentId>,
    resources: Vec<Fingerprint>,
    /// Lookup only, like `ids`.
    resource_ids: BTreeMap<TypeId, ResourceId>,
}

impl Registry {
    /// Registers `T`; registering it again keeps its first identifier.
    pub(crate) fn register<T: Component>(&mut self) {
        let next = ComponentId(
            u32::try_from(self.components.len()).expect("component identifiers are exhausted"),
        );
        self.ids.entry(TypeId::of::<T>()).or_insert_with(|| {
            self.components.push(ComponentInfo {
                fingerprint: Fingerprint::of::<T>(),
                new_column: || Box::new(Vec::<T>::new()),
            });
            next
        });
    }

    /// Registers the resource `T` and returns its identifier; registering it
    /// again returns its first identifier.
    pub(crate) fn register_resource<T: Resource>(&mut self) -> ResourceId {
        let next = ResourceId(
            u32::try_from(self.resources.len()).expect("resource identifiers are exhausted"),
        );
        *self
            .resource_ids
            .entry(TypeId::of::<T>())
            .or_insert_with(|| {
                self.resources.push(Fingerprint::of::<T>());
                next
            })
    }

    /// # Panics
    ///
    /// Panics when the resource `T` is not inserted.
    pub(crate) fn resource_id<T: Resource>(&self) -> ResourceId {
        *self
            .resource_ids
            .get(&TypeId::of::<T>())
            .unwrap_or_else(|| panic!("resource {} is not inserted", type_name::<T>()))
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

    pub(crate) fn fingerprint(&self) -> RegistryFingerprint {
        RegistryFingerprint {
            components: self
                .components
                .iter()
                .map(|info| info.fingerprint)
                .collect(),
            resources: self.resources.clone(),
        }
    }

    /// Compares without allocating, so a schedule can check it on every run.
    pub(crate) fn has_fingerprint(&self, fingerprint: &RegistryFingerprint) -> bool {
        self.components
            .iter()
            .map(|info| &info.fingerprint)
            .eq(&fingerprint.components)
            && self.resources == fingerprint.resources
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
        assert_eq!(registry.fingerprint().components.len(), 2);
    }

    #[derive(Clone, Copy)]
    struct Gravity;
    impl Resource for Gravity {}

    #[derive(Clone, Copy)]
    struct Score;
    impl Resource for Score {}

    #[test]
    fn resource_identifiers_follow_insertion_order_and_stay_stable() {
        let mut registry = Registry::default();

        let score = registry.register_resource::<Score>();
        let gravity = registry.register_resource::<Gravity>();

        assert_eq!(score, ResourceId(0));
        assert_eq!(gravity, ResourceId(1));
        assert_eq!(registry.register_resource::<Score>(), ResourceId(0));
        assert_eq!(registry.resource_id::<Gravity>(), ResourceId(1));
    }

    #[test]
    #[should_panic(expected = "is not inserted")]
    fn a_missing_resource_panics() {
        Registry::default().resource_id::<Score>();
    }

    #[test]
    fn the_fingerprint_tells_components_and_resources_apart() {
        let mut with_resource = Registry::default();
        with_resource.register_resource::<Score>();
        let mut other_resource = Registry::default();
        other_resource.register_resource::<Gravity>();

        assert!(!with_resource.has_fingerprint(&Registry::default().fingerprint()));
        assert!(!with_resource.has_fingerprint(&other_resource.fingerprint()));
        assert!(with_resource.has_fingerprint(&with_resource.fingerprint()));
    }

    #[test]
    #[should_panic(expected = "is not registered")]
    fn an_unregistered_component_panics() {
        Registry::default().id::<Position>();
    }
}
