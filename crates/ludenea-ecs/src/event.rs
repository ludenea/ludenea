//! Events of the current tick: written by one system, read by later ones, and dropped by
//! `World::clear_events`, so none outlive the tick.

use std::any::{Any, type_name};
use std::cell::{Ref, RefCell, RefMut};

use crate::system::{Accesses, EVENT, Missing, SystemParam, declare_cell, registered};
use crate::{Access, World};

/// Type-erased access to the events of one type, stored as a `RefCell<Vec<T>>`.
pub(crate) trait EventQueue {
    fn as_any(&self) -> &dyn Any;
    fn clear(&mut self);
}

impl<T: Send + 'static> EventQueue for RefCell<Vec<T>> {
    fn as_any(&self) -> &dyn Any {
        self
    }

    fn clear(&mut self) {
        self.get_mut().clear();
    }
}

/// Reads the `T` events sent so far this tick, in send order.
pub struct Events<'w, T>(Ref<'w, Vec<T>>);

impl<T> Events<'_, T> {
    pub fn iter(&self) -> std::slice::Iter<'_, T> {
        self.0.iter()
    }
}

/// Sends `T` events, visible to the systems that run later this tick.
pub struct EventWriter<'w, T>(RefMut<'w, Vec<T>>);

impl<T> EventWriter<'_, T> {
    pub fn send(&mut self, event: T) {
        self.0.push(event);
    }
}

impl<T: Send + 'static> SystemParam for Events<'_, T> {
    type Item<'w> = Events<'w, T>;

    fn declare(world: &World, accesses: &mut Accesses) -> Result<(), Missing> {
        let access = Access::Read(type_name::<T>());
        declare_cell(world.event_cell::<T>(), EVENT, access, accesses)
    }

    fn fetch(world: &World) -> Self::Item<'_> {
        Events(registered(world.event_cell::<T>(), EVENT, type_name::<T>()).borrow())
    }
}

impl<T: Send + 'static> SystemParam for EventWriter<'_, T> {
    type Item<'w> = EventWriter<'w, T>;

    fn declare(world: &World, accesses: &mut Accesses) -> Result<(), Missing> {
        let access = Access::Write(type_name::<T>());
        declare_cell(world.event_cell::<T>(), EVENT, access, accesses)
    }

    fn fetch(world: &World) -> Self::Item<'_> {
        let cell = registered(world.event_cell::<T>(), EVENT, type_name::<T>());
        EventWriter(cell.borrow_mut())
    }
}
