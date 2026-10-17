use std::any::{Any, TypeId, type_name};
use std::cell::{Ref, RefCell};
use std::collections::BTreeMap;

use crate::{Codec, Component, EcsError, EntitiesState, Entity, Resource};

/// Type-erased access to one component storage.
pub(crate) trait Storage {
    fn as_any(&self) -> &dyn Any;
    fn as_any_mut(&mut self) -> &mut dyn Any;
    /// Drops the component held by the slot `index`, if any.
    fn clear(&mut self, index: u32);
    /// Visits every stored component in ascending index order.
    fn for_each(&self, visit: &mut dyn FnMut(u32, &dyn Any));
    /// Replaces every component with `values`, each a boxed `T` from this type's codec.
    fn replace_all(&mut self, values: Vec<(u32, Box<dyn Any + Send>)>);
}

/// Components of one type, indexed by entity index.
// ponytail: iteration visits every index up to the highest alive one; switch to sparse sets with a presence bitset if profiling shows it
pub(crate) struct ComponentStorage<T>(pub(crate) RefCell<Vec<Option<T>>>);

impl<T: Component> Storage for ComponentStorage<T> {
    fn as_any(&self) -> &dyn Any {
        self
    }

    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }

    fn clear(&mut self, index: u32) {
        if let Some(slot) = self.0.get_mut().get_mut(index as usize) {
            *slot = None;
        }
    }

    fn for_each(&self, visit: &mut dyn FnMut(u32, &dyn Any)) {
        for (index, slot) in (0u32..).zip(self.0.borrow().iter()) {
            if let Some(value) = slot {
                visit(index, value);
            }
        }
    }

    fn replace_all(&mut self, values: Vec<(u32, Box<dyn Any + Send>)>) {
        let slots = self.0.get_mut();
        slots.clear();
        for (index, value) in values {
            let index = index as usize;
            if slots.len() <= index {
                slots.resize_with(index + 1, || None);
            }
            slots[index] = Some(*downcast::<T>(value));
        }
    }
}

/// Type-erased access to one resource, stored as a `RefCell<T>`.
pub(crate) trait ResourceCell {
    fn as_any(&self) -> &dyn Any;
    fn as_any_mut(&mut self) -> &mut dyn Any;
    fn value(&self) -> Ref<'_, dyn Any>;
    /// Replaces the value with `value`, a boxed `T` from this type's codec.
    fn set(&mut self, value: Box<dyn Any + Send>);
}

impl<T: Resource> ResourceCell for RefCell<T> {
    fn as_any(&self) -> &dyn Any {
        self
    }

    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }

    fn value(&self) -> Ref<'_, dyn Any> {
        Ref::map(self.borrow(), |value| value as &dyn Any)
    }

    fn set(&mut self, value: Box<dyn Any + Send>) {
        *self.get_mut() = *downcast::<T>(value);
    }
}

fn downcast<T: 'static>(value: Box<dyn Any + Send>) -> Box<T> {
    value
        .downcast::<T>()
        .unwrap_or_else(|_| panic!("expected a value of {}", type_name::<T>()))
}

pub(crate) struct ComponentEntry {
    pub(crate) type_id: TypeId,
    pub(crate) type_name: &'static str,
    pub(crate) codec: Codec,
    pub(crate) storage: Box<dyn Storage + Send>,
}

pub(crate) struct ResourceEntry {
    pub(crate) type_id: TypeId,
    pub(crate) type_name: &'static str,
    pub(crate) codec: Codec,
    pub(crate) value: Box<dyn ResourceCell + Send>,
}

/// Entities, their components, and resources. Registered types are keyed by stable name,
/// so every iteration over them is in name order and never depends on `TypeId`.
#[derive(Default)]
pub struct World {
    pub(crate) entities: EntitiesState,
    pub(crate) components: BTreeMap<&'static str, ComponentEntry>,
    pub(crate) resources: BTreeMap<&'static str, ResourceEntry>,
    /// Never hashed, snapshotted, or iterated; each value is a `RefCell<T>`.
    runtime: BTreeMap<TypeId, Box<dyn Any + Send>>,
}

impl World {
    pub fn new() -> Self {
        Self::default()
    }

    /// Registers `T`; registering the same type again is a no-op.
    pub fn register_component<T: Component>(&mut self) -> Result<(), EcsError> {
        if let Some(entry) = self.components.get(T::NAME) {
            return check_same_type::<T>(T::NAME, entry.type_id, entry.type_name);
        }
        self.components.insert(
            T::NAME,
            ComponentEntry {
                type_id: TypeId::of::<T>(),
                type_name: type_name::<T>(),
                codec: Codec::of::<T>(),
                storage: Box::new(ComponentStorage::<T>(RefCell::new(Vec::new()))),
            },
        );
        Ok(())
    }

    /// Registers `T` and stores `value`, replacing any previous value of `T`.
    pub fn register_resource<T: Resource>(&mut self, value: T) -> Result<(), EcsError> {
        if let Some(entry) = self.resources.get(T::NAME) {
            check_same_type::<T>(T::NAME, entry.type_id, entry.type_name)?;
        }
        self.resources.insert(
            T::NAME,
            ResourceEntry {
                type_id: TypeId::of::<T>(),
                type_name: type_name::<T>(),
                codec: Codec::of::<T>(),
                value: Box::new(RefCell::new(value)),
            },
        );
        Ok(())
    }

    /// Stores a value outside the simulation state: it is never hashed or snapshotted.
    pub fn insert_runtime<T: Send + 'static>(&mut self, value: T) {
        self.runtime
            .insert(TypeId::of::<T>(), Box::new(RefCell::new(value)));
    }

    pub fn spawn(&mut self) -> Entity {
        self.entities.spawn()
    }

    /// Despawns `entity` and drops all of its components.
    pub fn despawn(&mut self, entity: Entity) -> Result<(), EcsError> {
        self.ensure_alive(entity)?;
        for entry in self.components.values_mut() {
            entry.storage.clear(entity.index());
        }
        self.entities.despawn(entity);
        Ok(())
    }

    pub fn is_alive(&self, entity: Entity) -> bool {
        self.entities.is_alive(entity)
    }

    /// Alive entities in ascending index order.
    pub fn entities(&self) -> impl Iterator<Item = Entity> + '_ {
        self.entities.iter()
    }

    /// Inserts or replaces the `T` component of `entity`.
    pub fn insert<T: Component>(&mut self, entity: Entity, value: T) -> Result<(), EcsError> {
        self.ensure_alive(entity)?;
        let slots = self
            .storage_mut::<T>()
            .ok_or(EcsError::UnregisteredComponent(T::NAME))?;
        let index = entity.index() as usize;
        if slots.len() <= index {
            slots.resize_with(index + 1, || None);
        }
        slots[index] = Some(value);
        Ok(())
    }

    /// Removes and returns the `T` component of `entity`, if it had one.
    pub fn remove<T: Component>(&mut self, entity: Entity) -> Result<Option<T>, EcsError> {
        self.ensure_alive(entity)?;
        let slots = self
            .storage_mut::<T>()
            .ok_or(EcsError::UnregisteredComponent(T::NAME))?;
        Ok(slots
            .get_mut(entity.index() as usize)
            .and_then(Option::take))
    }

    pub fn get<T: Component>(&self, entity: Entity) -> Option<Ref<'_, T>> {
        if !self.is_alive(entity) {
            return None;
        }
        let storage = self.components.get(T::NAME)?.storage.as_any();
        let slots = storage.downcast_ref::<ComponentStorage<T>>()?.0.borrow();
        Ref::filter_map(slots, |slots| slots.get(entity.index() as usize)?.as_ref()).ok()
    }

    pub fn get_mut<T: Component>(&mut self, entity: Entity) -> Option<&mut T> {
        if !self.is_alive(entity) {
            return None;
        }
        self.storage_mut::<T>()?
            .get_mut(entity.index() as usize)?
            .as_mut()
    }

    pub fn resource<T: Resource>(&self) -> Option<Ref<'_, T>> {
        let value = self
            .resources
            .get(T::NAME)?
            .value
            .as_any()
            .downcast_ref::<RefCell<T>>()?;
        Some(value.borrow())
    }

    pub fn resource_mut<T: Resource>(&mut self) -> Option<&mut T> {
        let value = self
            .resources
            .get_mut(T::NAME)?
            .value
            .as_any_mut()
            .downcast_mut::<RefCell<T>>()?;
        Some(value.get_mut())
    }

    /// Serialization functions of the component registered under `name`.
    pub fn component_codec(&self, name: &str) -> Option<Codec> {
        self.components.get(name).map(|entry| entry.codec)
    }

    /// Serialization functions of the resource registered under `name`.
    pub fn resource_codec(&self, name: &str) -> Option<Codec> {
        self.resources.get(name).map(|entry| entry.codec)
    }

    pub fn runtime<T: Send + 'static>(&self) -> Option<Ref<'_, T>> {
        let value = self
            .runtime
            .get(&TypeId::of::<T>())?
            .downcast_ref::<RefCell<T>>()?;
        Some(value.borrow())
    }

    pub fn runtime_mut<T: Send + 'static>(&mut self) -> Option<&mut T> {
        let value = self
            .runtime
            .get_mut(&TypeId::of::<T>())?
            .downcast_mut::<RefCell<T>>()?;
        Some(value.get_mut())
    }

    fn ensure_alive(&self, entity: Entity) -> Result<(), EcsError> {
        if self.is_alive(entity) {
            Ok(())
        } else {
            Err(EcsError::NoSuchEntity(entity))
        }
    }

    fn storage_mut<T: Component>(&mut self) -> Option<&mut Vec<Option<T>>> {
        let storage = self.components.get_mut(T::NAME)?.storage.as_any_mut();
        Some(storage.downcast_mut::<ComponentStorage<T>>()?.0.get_mut())
    }
}

/// Lookups go by `T::NAME` and the downcast checks the type, so the registry needs no
/// `TypeId` index; this guard keeps one name bound to one type.
fn check_same_type<T: 'static>(
    name: &'static str,
    registered: TypeId,
    registered_name: &'static str,
) -> Result<(), EcsError> {
    if registered == TypeId::of::<T>() {
        Ok(())
    } else {
        Err(EcsError::DuplicateName {
            name,
            first: registered_name,
            second: type_name::<T>(),
        })
    }
}
