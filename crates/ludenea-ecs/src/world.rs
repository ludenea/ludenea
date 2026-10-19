use std::any::{Any, TypeId, type_name};
use std::cell::{Ref, RefCell};
use std::collections::BTreeMap;

use ludenea_schema::{Schema, TypeSchema};

use crate::assets::{AssetType, LoadedAsset};
use crate::commands::CommandQueue;
use crate::event::{Event, EventQueue};
use crate::{AssetId, Codec, Component, EcsError, EntitiesState, Entity, Resource};

/// Type-erased access to one component storage.
pub(crate) trait Storage {
    fn as_any(&self) -> &dyn Any;
    fn as_any_mut(&mut self) -> &mut dyn Any;
    /// Whether the slot `index` holds a component.
    fn has(&self, index: u32) -> bool;
    /// Drops the component held by the slot `index`, if any.
    fn clear(&mut self, index: u32);
    /// Visits every stored component in ascending index order.
    fn for_each(&self, visit: &mut dyn FnMut(u32, &dyn Any));
    /// Visits the component held by the slot `index`, if any.
    fn visit(&self, index: u32, visit: &mut dyn FnMut(&dyn Any));
    /// Inserts or replaces the component of slot `index` with `value`, a boxed `T` from
    /// this type's codec.
    fn insert_boxed(&mut self, index: u32, value: Box<dyn Any + Send>);
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

    fn has(&self, index: u32) -> bool {
        matches!(self.0.borrow().get(index as usize), Some(Some(_)))
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

    fn visit(&self, index: u32, visit: &mut dyn FnMut(&dyn Any)) {
        if let Some(Some(value)) = self.0.borrow().get(index as usize) {
            visit(value);
        }
    }

    fn insert_boxed(&mut self, index: u32, value: Box<dyn Any + Send>) {
        let slots = self.0.get_mut();
        let index = index as usize;
        if slots.len() <= index {
            slots.resize_with(index + 1, || None);
        }
        slots[index] = Some(*downcast::<T>(value));
    }

    fn replace_all(&mut self, values: Vec<(u32, Box<dyn Any + Send>)>) {
        self.0.get_mut().clear();
        for (index, value) in values {
            self.insert_boxed(index, value);
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
    pub(crate) asset_types: BTreeMap<&'static str, AssetType>,
    /// Loaded assets: hashed (only when not empty) and snapshotted, never written by systems.
    pub(crate) assets: BTreeMap<AssetId, LoadedAsset>,
    /// Never hashed, snapshotted, or iterated; each value is a `RefCell<T>`.
    runtime: BTreeMap<TypeId, Box<dyn Any + Send>>,
    /// Events of the current tick, keyed by stable name; each value is a `RefCell<Vec<T>>`.
    events: BTreeMap<&'static str, Box<dyn EventQueue + Send>>,
    /// Commands issued by the running system, applied when it returns.
    pub(crate) commands: RefCell<CommandQueue>,
}

impl World {
    pub fn new() -> Self {
        Self::default()
    }

    /// Registers `T`; registering the same type again is a no-op. Fails with
    /// `EcsError::RecursiveType` when `T` holds a type that contains itself.
    pub fn register_component<T: Component>(&mut self) -> Result<(), EcsError> {
        if let Some(entry) = self.components.get(T::NAME) {
            return check_same_type::<T>(T::NAME, entry.type_id, entry.type_name);
        }
        check_not_recursive::<T>()?;
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

    /// Registers `T` and stores `value`, replacing any previous value of `T`. Fails with
    /// `EcsError::RecursiveType` when `T` holds a type that contains itself.
    pub fn register_resource<T: Resource>(&mut self, value: T) -> Result<(), EcsError> {
        if let Some(entry) = self.resources.get(T::NAME) {
            check_same_type::<T>(T::NAME, entry.type_id, entry.type_name)?;
        }
        check_not_recursive::<T>()?;
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

    /// The schema of every registered component type, by stable name.
    pub fn component_schemas(&self) -> Vec<(&'static str, TypeSchema)> {
        self.components
            .iter()
            .map(|(&name, entry)| (name, (entry.codec.schema)()))
            .collect()
    }

    /// The schema of every registered resource type, by stable name.
    pub fn resource_schemas(&self) -> Vec<(&'static str, TypeSchema)> {
        self.resources
            .iter()
            .map(|(&name, entry)| (name, (entry.codec.schema)()))
            .collect()
    }

    /// Stores a value outside the simulation state: it is never hashed or snapshotted.
    pub fn insert_runtime<T: Send + 'static>(&mut self, value: T) {
        self.runtime
            .insert(TypeId::of::<T>(), Box::new(RefCell::new(value)));
    }

    /// Registers the event type `T`; registering it again is a no-op. Fails with
    /// `EcsError::DuplicateName` when another type already has `T::NAME`, and with
    /// `EcsError::RecursiveType` when `T` holds a type that contains itself.
    pub fn register_event<T: Event>(&mut self) -> Result<(), EcsError> {
        if let Some(queue) = self.events.get(T::NAME) {
            let (type_id, type_name) = queue.event_type();
            return check_same_type::<T>(T::NAME, type_id, type_name);
        }
        check_not_recursive::<T>()?;
        self.events
            .insert(T::NAME, Box::new(RefCell::new(Vec::<T>::new())));
        Ok(())
    }

    /// The events sent so far this tick as JSON, per registered event type name (name order,
    /// empty types included), each in send order. An event without a JSON form shows as
    /// `{"error": "<message>"}`.
    pub fn events_json(&self) -> Vec<(&'static str, Vec<serde_json::Value>)> {
        self.events
            .iter()
            .map(|(&name, queue)| (name, queue.to_json()))
            .collect()
    }

    /// The schema of every registered event type, by stable name.
    pub fn event_schemas(&self) -> Vec<(&'static str, TypeSchema)> {
        self.events
            .iter()
            .map(|(&name, queue)| (name, queue.schema()))
            .collect()
    }

    /// Drops every event sent this tick.
    pub fn clear_events(&mut self) {
        for queue in self.events.values_mut() {
            queue.clear();
        }
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
            .ok_or_else(|| EcsError::UnregisteredComponent(T::NAME.to_string()))?;
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
            .ok_or_else(|| EcsError::UnregisteredComponent(T::NAME.to_string()))?;
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

    /// Inserts or replaces the component registered under `name` on `entity`. `value` must
    /// come from that name's codec (`decode` or `decode_ron`).
    ///
    /// # Panics
    /// If `value` is not of the type registered under `name`.
    pub fn insert_boxed(
        &mut self,
        entity: Entity,
        name: &str,
        value: Box<dyn Any + Send>,
    ) -> Result<(), EcsError> {
        self.ensure_alive(entity)?;
        let entry = self
            .components
            .get_mut(name)
            .ok_or_else(|| EcsError::UnregisteredComponent(name.to_string()))?;
        entry.storage.insert_boxed(entity.index(), value);
        Ok(())
    }

    /// Replaces the resource registered under `name`. `value` must come from that name's
    /// codec (`decode` or `decode_ron`).
    ///
    /// # Panics
    /// If `value` is not of the type registered under `name`.
    pub fn insert_resource_boxed(
        &mut self,
        name: &str,
        value: Box<dyn Any + Send>,
    ) -> Result<(), EcsError> {
        let entry = self
            .resources
            .get_mut(name)
            .ok_or_else(|| EcsError::UnregisteredResource(name.to_string()))?;
        entry.value.set(value);
        Ok(())
    }

    pub fn resource<T: Resource>(&self) -> Option<Ref<'_, T>> {
        Some(self.resource_cell::<T>()?.borrow())
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

    /// The components of `entity` as RON strings keyed by stable name (name order), or
    /// `None` when `entity` is not alive.
    // ponytail: scans each storage; index lookup per type if entity inspection gets hot
    pub fn entity_components_ron(&self, entity: Entity) -> Option<BTreeMap<String, String>> {
        if !self.is_alive(entity) {
            return None;
        }
        let mut out = BTreeMap::new();
        for (name, entry) in &self.components {
            entry.storage.visit(entity.index(), &mut |value| {
                out.insert(name.to_string(), (entry.codec.encode_ron)(value));
            });
        }
        Some(out)
    }

    /// The `name` component of `entity` as JSON, or `None` when `entity` is not alive, the
    /// name is not registered, or the entity has no such component. The inner error says
    /// why the value has no exact JSON form.
    pub fn component_json(
        &self,
        entity: Entity,
        name: &str,
    ) -> Option<Result<serde_json::Value, String>> {
        self.with_component(entity, name, |codec, value| (codec.to_json)(value))
    }

    /// Whether `entity` is alive and has the component registered under `name`, without
    /// encoding its value.
    pub fn has_component_named(&self, entity: Entity, name: &str) -> bool {
        self.with_component(entity, name, |_, _| ()).is_some()
    }

    /// The `name` component of `entity` as a RON string, or `None` when `entity` is not
    /// alive, the name is not registered, or the entity has no such component.
    pub fn component_ron(&self, entity: Entity, name: &str) -> Option<String> {
        self.with_component(entity, name, |codec, value| (codec.encode_ron)(value))
    }

    /// The `name` component of `entity` as canonical postcard bytes (`Codec::encode`), or
    /// `None` when `entity` is not alive, the name is not registered, or the entity has no
    /// such component.
    pub fn component_bytes(&self, entity: Entity, name: &str) -> Option<Vec<u8>> {
        self.with_component(entity, name, |codec, value| (codec.encode)(value))
    }

    fn with_component<R>(
        &self,
        entity: Entity,
        name: &str,
        read: impl FnOnce(&Codec, &dyn Any) -> R,
    ) -> Option<R> {
        if !self.is_alive(entity) {
            return None;
        }
        let entry = self.components.get(name)?;
        let mut read = Some(read);
        let mut out = None;
        entry.storage.visit(entity.index(), &mut |value| {
            if let Some(read) = read.take() {
                out = Some(read(&entry.codec, value));
            }
        });
        out
    }

    /// Every component of `entity` as JSON keyed by stable name, or `None` when `entity` is
    /// not alive.
    pub fn entity_components_json(
        &self,
        entity: Entity,
    ) -> Option<BTreeMap<String, Result<serde_json::Value, String>>> {
        if !self.is_alive(entity) {
            return None;
        }
        let mut out = BTreeMap::new();
        for (name, entry) in &self.components {
            entry.storage.visit(entity.index(), &mut |value| {
                out.insert(name.to_string(), (entry.codec.to_json)(value));
            });
        }
        Some(out)
    }

    /// The resource registered under `name` as JSON, or `None` when it is not registered.
    pub fn resource_json(&self, name: &str) -> Option<Result<serde_json::Value, String>> {
        let entry = self.resources.get(name)?;
        let value = entry.value.value();
        Some((entry.codec.to_json)(&*value))
    }

    /// The resource registered under `name` as a RON string, or `None` when it is not
    /// registered.
    pub fn resource_ron(&self, name: &str) -> Option<String> {
        let entry = self.resources.get(name)?;
        let value = entry.value.value();
        Some((entry.codec.encode_ron)(&*value))
    }

    /// Removes the component registered under `name` from `entity`, if it has one.
    pub fn remove_by_name(&mut self, entity: Entity, name: &str) -> Result<(), EcsError> {
        self.ensure_alive(entity)?;
        let entry = self
            .components
            .get_mut(name)
            .ok_or_else(|| EcsError::UnregisteredComponent(name.to_string()))?;
        entry.storage.clear(entity.index());
        Ok(())
    }

    /// Serialization functions of the resource registered under `name`.
    pub fn resource_codec(&self, name: &str) -> Option<Codec> {
        self.resources.get(name).map(|entry| entry.codec)
    }

    pub fn runtime<T: Send + 'static>(&self) -> Option<Ref<'_, T>> {
        Some(self.runtime_cell::<T>()?.borrow())
    }

    pub fn runtime_mut<T: Send + 'static>(&mut self) -> Option<&mut T> {
        let value = self
            .runtime
            .get_mut(&TypeId::of::<T>())?
            .downcast_mut::<RefCell<T>>()?;
        Some(value.get_mut())
    }

    /// The storage cell of `T`, for queries that borrow it for their whole lifetime.
    pub(crate) fn component_cell<T: Component>(
        &self,
    ) -> Result<&RefCell<Vec<Option<T>>>, EcsError> {
        let entry = self
            .components
            .get(T::NAME)
            .ok_or_else(|| EcsError::UnregisteredComponent(T::NAME.to_string()))?;
        match entry.storage.as_any().downcast_ref::<ComponentStorage<T>>() {
            Some(storage) => Ok(&storage.0),
            None => Err(EcsError::DuplicateName {
                name: T::NAME,
                first: entry.type_name,
                second: type_name::<T>(),
            }),
        }
    }

    pub(crate) fn resource_cell<T: Resource>(&self) -> Option<&RefCell<T>> {
        self.resources
            .get(T::NAME)?
            .value
            .as_any()
            .downcast_ref::<RefCell<T>>()
    }

    pub(crate) fn runtime_cell<T: Send + 'static>(&self) -> Option<&RefCell<T>> {
        self.runtime.get(&TypeId::of::<T>())?.downcast_ref()
    }

    pub(crate) fn event_cell<T: Event>(&self) -> Option<&RefCell<Vec<T>>> {
        self.events.get(T::NAME)?.as_any().downcast_ref()
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

/// Refuses `T` when its schema holds a type that contains itself: recursive types have no
/// schema yet, so tools could not describe its values. Every registration checks it.
pub fn check_not_recursive<T: Schema>() -> Result<(), EcsError> {
    match T::schema().recursive_type() {
        None => Ok(()),
        Some(recursive) => Err(EcsError::RecursiveType {
            name: type_name::<T>(),
            recursive: recursive.to_owned(),
        }),
    }
}

/// Lookups go by `T::NAME` and the downcast checks the type, so the registry needs no
/// `TypeId` index; this guard keeps one name bound to one type.
pub(crate) fn check_same_type<T: 'static>(
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
