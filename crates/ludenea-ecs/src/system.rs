//! Function systems and their parameters. Each parameter declares its accesses and
//! required registrations, which `ScheduleBuilder::build` checks, so the `RefCell` borrows
//! taken by `fetch` never fail at run time.

use std::any::type_name;
use std::cell::{Ref, RefMut};
use std::collections::BTreeSet;
use std::marker::PhantomData;
use std::ops::{Deref, DerefMut};

use crate::schedule::BuildError;
use crate::{Access, Query, QueryData, QueryFilter, Resource, World, accesses, check_accesses};

pub(crate) const COMPONENT: &str = "component";
pub(crate) const RESOURCE: &str = "resource";
pub(crate) const EVENT: &str = "event";
pub(crate) const RUNTIME: &str = "runtime resource";

/// Accesses declared by one parameter, each tagged with its kind (`"component"`,
/// `"resource"`, `"event"`, or `"runtime resource"`): only accesses of one kind conflict.
pub type Accesses = Vec<(&'static str, Access)>;

/// The kind and name of a registration a parameter requires but the world lacks.
pub type Missing = (&'static str, &'static str);

/// A value a system receives as an argument, borrowed from the world for one run.
pub trait SystemParam {
    type Item<'w>;

    /// Checks that every registration this parameter needs exists in `world`, then
    /// pushes its accesses.
    fn declare(world: &World, accesses: &mut Accesses) -> Result<(), Missing>;

    /// # Panics
    /// If `declare` would fail on `world`, or a declared access is already borrowed.
    fn fetch(world: &World) -> Self::Item<'_>;
}

/// A system ready to run on a world.
pub trait System: Send {
    /// The function's type name, for diagnostics only.
    fn name(&self) -> &'static str;

    /// Rejects missing registrations and conflicting accesses between parameters.
    fn check(&self, world: &World) -> Result<(), BuildError>;

    fn run(&mut self, world: &World);
}

/// Converts a function into a system. `M` is the function's parameter list, which keeps
/// the implementations for different arities apart.
pub trait IntoSystem<M> {
    fn into_system(self) -> Box<dyn System>;
}

pub struct FunctionSystem<F, M> {
    function: F,
    _params: PhantomData<fn() -> M>,
}

type Declare = fn(&World, &mut Accesses) -> Result<(), Missing>;

/// Runs every parameter's `declare`, then checks accesses of each kind for conflicts.
/// Parameters are numbered from 1 in error messages.
fn check(system: &'static str, world: &World, declares: &[Declare]) -> Result<(), BuildError> {
    let mut all = Vec::new();
    for (param, declare) in (1..).zip(declares) {
        let mut accesses = Vec::new();
        declare(world, &mut accesses).map_err(|(kind, name)| BuildError::Unregistered {
            system,
            kind,
            name,
        })?;
        all.extend(
            accesses
                .into_iter()
                .map(|(kind, access)| (param, kind, access)),
        );
    }
    let kinds: BTreeSet<_> = all.iter().map(|&(_, kind, _)| kind).collect();
    for kind in kinds {
        let (params, accesses): (Vec<usize>, Vec<Access>) = all
            .iter()
            .filter(|&&(_, other, _)| other == kind)
            .map(|&(param, _, access)| (param, access))
            .unzip();
        if let Err((component, first, second)) = check_accesses(&accesses) {
            return Err(BuildError::AccessConflict {
                system,
                component,
                first: params[first],
                second: params[second],
            });
        }
    }
    Ok(())
}

macro_rules! function_system {
    ($($param:ident),*) => {
        impl<F, $($param: SystemParam + 'static),*> IntoSystem<fn($($param,)*)> for F
        where
            F: Send + 'static,
            for<'a> &'a mut F: FnMut($($param),*) + FnMut($($param::Item<'_>),*),
        {
            fn into_system(self) -> Box<dyn System> {
                Box::new(FunctionSystem::<F, fn($($param,)*)> {
                    function: self,
                    _params: PhantomData,
                })
            }
        }

        impl<F, $($param: SystemParam + 'static),*> System for FunctionSystem<F, fn($($param,)*)>
        where
            F: Send + 'static,
            for<'a> &'a mut F: FnMut($($param),*) + FnMut($($param::Item<'_>),*),
        {
            fn name(&self) -> &'static str {
                type_name::<F>()
            }

            fn check(&self, world: &World) -> Result<(), BuildError> {
                check(self.name(), world, &[$($param::declare as Declare),*])
            }

            #[allow(non_snake_case, unused_variables)]
            fn run(&mut self, world: &World) {
                // Calling through a generic function pins each argument to `$param`'s
                // item type, which the compiler cannot infer through `&mut F` alone.
                #[allow(clippy::too_many_arguments)]
                fn call<$($param),*>(mut function: impl FnMut($($param),*), $($param: $param),*) {
                    function($($param),*);
                }
                $(let $param = $param::fetch(world);)*
                call(&mut self.function, $($param),*);
            }
        }
    };
}

function_system!();
function_system!(A);
function_system!(A, B);
function_system!(A, B, C);
function_system!(A, B, C, D);
function_system!(A, B, C, D, E);
function_system!(A, B, C, D, E, G);
function_system!(A, B, C, D, E, G, H);
function_system!(A, B, C, D, E, G, H, I);

/// Pushes `access` of `kind` when `cell` exists, the shared `declare` of every
/// parameter backed by one registered cell.
pub(crate) fn declare_cell<C>(
    cell: Option<C>,
    kind: &'static str,
    access: Access,
    accesses: &mut Accesses,
) -> Result<(), Missing> {
    cell.ok_or((kind, access.name()))?;
    accesses.push((kind, access));
    Ok(())
}

/// Unwraps a cell that `declare_cell` checked at build time.
pub(crate) fn registered<C>(cell: Option<C>, kind: &str, name: &str) -> C {
    cell.unwrap_or_else(|| {
        panic!("{kind} `{name}` is not registered in the world this schedule runs on")
    })
}

impl<D: QueryData + 'static, F: QueryFilter + 'static> SystemParam for Query<'_, D, F> {
    type Item<'w> = Query<'w, D, F>;

    fn declare(world: &World, out: &mut Accesses) -> Result<(), Missing> {
        for access in accesses::<D, F>() {
            declare_cell(world.component_codec(access.name()), COMPONENT, access, out)?;
        }
        Ok(())
    }

    fn fetch(world: &World) -> Self::Item<'_> {
        Query::new(world).unwrap_or_else(|error| panic!("{error}"))
    }
}

/// Reads the resource `T`.
pub struct Res<'w, T>(Ref<'w, T>);

/// Reads and writes the resource `T`.
pub struct ResMut<'w, T>(RefMut<'w, T>);

/// Reads the runtime resource `T`, which is never hashed or snapshotted. Systems cannot
/// write runtime resources: a write would change future behaviour without changing the
/// state hash.
pub struct Runtime<'w, T>(Ref<'w, T>);

impl<T: Resource> SystemParam for Res<'_, T> {
    type Item<'w> = Res<'w, T>;

    fn declare(world: &World, accesses: &mut Accesses) -> Result<(), Missing> {
        declare_cell(
            world.resource_cell::<T>(),
            RESOURCE,
            Access::Read(T::NAME),
            accesses,
        )
    }

    fn fetch(world: &World) -> Self::Item<'_> {
        Res(registered(world.resource_cell::<T>(), RESOURCE, T::NAME).borrow())
    }
}

impl<T: Resource> SystemParam for ResMut<'_, T> {
    type Item<'w> = ResMut<'w, T>;

    fn declare(world: &World, accesses: &mut Accesses) -> Result<(), Missing> {
        declare_cell(
            world.resource_cell::<T>(),
            RESOURCE,
            Access::Write(T::NAME),
            accesses,
        )
    }

    fn fetch(world: &World) -> Self::Item<'_> {
        ResMut(registered(world.resource_cell::<T>(), RESOURCE, T::NAME).borrow_mut())
    }
}

impl<T: Send + 'static> SystemParam for Runtime<'_, T> {
    type Item<'w> = Runtime<'w, T>;

    fn declare(world: &World, accesses: &mut Accesses) -> Result<(), Missing> {
        let access = Access::Read(type_name::<T>());
        declare_cell(world.runtime_cell::<T>(), RUNTIME, access, accesses)
    }

    fn fetch(world: &World) -> Self::Item<'_> {
        Runtime(registered(world.runtime_cell::<T>(), RUNTIME, type_name::<T>()).borrow())
    }
}

macro_rules! deref {
    ($($wrapper:ident),*) => {$(
        impl<T> Deref for $wrapper<'_, T> {
            type Target = T;

            fn deref(&self) -> &T {
                &self.0
            }
        }
    )*};
}

macro_rules! deref_mut {
    ($($wrapper:ident),*) => {$(
        impl<T> DerefMut for $wrapper<'_, T> {
            fn deref_mut(&mut self) -> &mut T {
                &mut self.0
            }
        }
    )*};
}

deref!(Res, ResMut, Runtime);
deref_mut!(ResMut);
