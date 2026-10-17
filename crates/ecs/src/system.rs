//! Systems: functions whose parameters borrow disjoint parts of a world.

use std::any::{TypeId, type_name};
use std::marker::PhantomData;

use crate::World;
use crate::access::Access;
use crate::archetype::ArchetypeId;
use crate::column::Column;
use crate::commands::CommandBuffer;
use crate::entity::{Allocator, Entity};
use crate::query::ColumnBorrow;
use crate::registry::{Registry, ResourceId};
use crate::world::{Location, State};

/// The parts of a world lent to the parameters of one system run: every
/// column of every archetype and every resource, each in its own slot.
///
/// A parameter takes the slots it declared in its access. A written slot is
/// taken once; a read slot is turned into a shared borrow, which several
/// parameters can hold.
#[doc(hidden)]
pub struct WorldParts<'w> {
    pub(crate) registry: &'w Registry,
    pub(crate) locations: &'w [Option<Location>],
    /// Entities of every archetype and the index of its first column in
    /// `columns`.
    archetypes: Vec<(&'w [Entity], usize)>,
    columns: Vec<Option<ColumnBorrow<'w>>>,
    resources: Vec<Option<ColumnBorrow<'w>>>,
    /// Taken by the only commands parameter of the system.
    pub(crate) commands: Option<(&'w mut Allocator, &'w mut CommandBuffer)>,
}

impl<'w> WorldParts<'w> {
    // ponytail: lends every column of every archetype on each system run,
    // lend only the archetypes a system matches if the counters of the
    // benchmark harness show it.
    pub(crate) fn new(world: &'w mut World, commands: &'w mut CommandBuffer) -> Self {
        let State {
            allocator,
            locations,
            archetypes,
            resources,
            ..
        } = &mut world.state;
        let mut parts = Self {
            registry: &world.registry,
            commands: Some((allocator, commands)),
            locations,
            archetypes: Vec::with_capacity(archetypes.len()),
            columns: Vec::new(),
            resources: resources
                .columns_mut()
                .iter_mut()
                .map(|column| Some(ColumnBorrow::Exclusive(column.as_mut())))
                .collect(),
        };
        for archetype in archetypes.iter_mut() {
            let (_, entities, columns) = archetype.parts_mut();
            parts.archetypes.push((entities, parts.columns.len()));
            parts.columns.extend(
                columns
                    .iter_mut()
                    .map(|column| Some(ColumnBorrow::Exclusive(column.as_mut()))),
            );
        }
        parts
    }

    pub(crate) fn entities(&self, archetype: ArchetypeId) -> &'w [Entity] {
        self.archetypes[archetype.0].0
    }

    /// Lends the column at `position` in the key of `archetype`.
    pub(crate) fn column(
        &mut self,
        archetype: ArchetypeId,
        position: usize,
        write: bool,
    ) -> ColumnBorrow<'w> {
        let start = self.archetypes[archetype.0].1;
        lend(&mut self.columns[start + position], write)
    }

    pub(crate) fn resource(&mut self, id: ResourceId, write: bool) -> ColumnBorrow<'w> {
        lend(&mut self.resources[id.index()], write)
    }
}

/// Takes a written column out of its slot, or leaves a shared borrow in
/// place of a read one.
fn lend<'w>(slot: &mut Option<ColumnBorrow<'w>>, write: bool) -> ColumnBorrow<'w> {
    let borrow = slot
        .take()
        .expect("the access check lends a written column once");
    if write {
        assert!(
            matches!(borrow, ColumnBorrow::Exclusive(_)),
            "the access check never writes a column it also reads"
        );
        return borrow;
    }
    let shared: &'w dyn Column = match borrow {
        ColumnBorrow::Shared(column) => column,
        ColumnBorrow::Exclusive(column) => column,
    };
    *slot = Some(ColumnBorrow::Shared(shared));
    ColumnBorrow::Shared(shared)
}

/// A system parameter: [`Query`](crate::Query), [`Res`](crate::Res),
/// [`ResMut`](crate::ResMut), [`Commands`](crate::Commands), or a tuple of
/// up to eight of them.
pub trait SystemParam {
    /// What the parameter keeps between runs, such as a query cache. It is
    /// derived from the world and never part of a snapshot.
    #[doc(hidden)]
    type State: 'static;

    /// The value a system receives.
    #[doc(hidden)]
    type Item<'w, 's>;

    /// Creates the state and declares the access of the parameter.
    #[doc(hidden)]
    fn init(world: &World, access: &mut Access, system: &'static str) -> Self::State;

    /// Brings the state up to date before a run.
    #[doc(hidden)]
    fn update(_state: &mut Self::State, _world: &World) {}

    /// Takes the parts of the world the parameter declared.
    #[doc(hidden)]
    fn get<'w, 's>(state: &'s mut Self::State, parts: &mut WorldParts<'w>) -> Self::Item<'w, 's>;
}

/// The value a system receives for the parameter `P`.
#[doc(hidden)]
pub type SystemParamItem<'w, 's, P> = <P as SystemParam>::Item<'w, 's>;

impl SystemParam for () {
    type State = ();
    type Item<'w, 's> = ();

    fn init(_: &World, _: &mut Access, _: &'static str) {}

    fn get(_state: &mut (), _parts: &mut WorldParts<'_>) {}
}

macro_rules! impl_system_param_for_tuple {
    ($($param:ident $state:ident),+) => {
        impl<$($param: SystemParam),+> SystemParam for ($($param,)+) {
            type State = ($($param::State,)+);
            type Item<'w, 's> = ($($param::Item<'w, 's>,)+);

            fn init(world: &World, access: &mut Access, system: &'static str) -> Self::State {
                ($($param::init(world, access, system),)+)
            }

            fn update(state: &mut Self::State, world: &World) {
                let ($($state,)+) = state;
                $($param::update($state, world);)+
            }

            fn get<'w, 's>(
                state: &'s mut Self::State,
                parts: &mut WorldParts<'w>,
            ) -> Self::Item<'w, 's> {
                let ($($state,)+) = state;
                ($($param::get($state, parts),)+)
            }
        }
    };
}

impl_system_param_for_tuple!(A a);
impl_system_param_for_tuple!(A a, B b);
impl_system_param_for_tuple!(A a, B b, C c);
impl_system_param_for_tuple!(A a, B b, C c, D d);
impl_system_param_for_tuple!(A a, B b, C c, D d, E e);
impl_system_param_for_tuple!(A a, B b, C c, D d, E e, F f);
impl_system_param_for_tuple!(A a, B b, C c, D d, E e, F f, G g);
impl_system_param_for_tuple!(A a, B b, C c, D d, E e, F f, G g, H h);

/// A system the schedule can run.
#[doc(hidden)]
pub trait System {
    /// The name of the function, for messages only.
    fn name(&self) -> &'static str;

    /// Creates the parameter states and returns the access of the system.
    fn initialize(&mut self, world: &World) -> Access;

    /// Runs the system, recording its commands in `commands`.
    fn run(&mut self, world: &mut World, commands: &mut CommandBuffer);
}

/// A function whose parameters are all [`SystemParam`]s. `Marker` tells
/// apart the implementations for each number of parameters.
#[doc(hidden)]
pub trait SystemParamFunction<Marker>: 'static {
    type Param: SystemParam;

    fn run(&mut self, params: SystemParamItem<'_, '_, Self::Param>);
}

// The double bound makes the compiler infer each parameter type from the
// function signature (`FnMut(P)`) while calling it with the borrowed items
// (`FnMut(SystemParamItem<P>)`), whatever their lifetimes.
macro_rules! impl_system_param_function {
    ($($param:ident $value:ident),*) => {
        impl<Function, $($param: SystemParam),*> SystemParamFunction<fn($($param,)*)> for Function
        where
            Function: 'static,
            for<'a> &'a mut Function: FnMut($($param),*) + FnMut($(SystemParamItem<$param>),*),
        {
            type Param = ($($param,)*);

            fn run(&mut self, params: SystemParamItem<'_, '_, Self::Param>) {
                fn call<$($param),*>(
                    mut function: impl FnMut($($param),*),
                    ($($value,)*): ($($param,)*),
                ) {
                    function($($value),*);
                }
                call(self, params);
            }
        }
    };
}

impl_system_param_function!();
impl_system_param_function!(A a);
impl_system_param_function!(A a, B b);
impl_system_param_function!(A a, B b, C c);
impl_system_param_function!(A a, B b, C c, D d);
impl_system_param_function!(A a, B b, C c, D d, E e);
impl_system_param_function!(A a, B b, C c, D d, E e, F f);
impl_system_param_function!(A a, B b, C c, D d, E e, F f, G g);
impl_system_param_function!(A a, B b, C c, D d, E e, F f, G g, H h);

/// A function with [`SystemParam`] parameters, run by the schedule.
struct FunctionSystem<Function: SystemParamFunction<Marker>, Marker> {
    function: Function,
    state: Option<<Function::Param as SystemParam>::State>,
    marker: PhantomData<fn() -> Marker>,
}

impl<Function, Marker> System for FunctionSystem<Function, Marker>
where
    Function: SystemParamFunction<Marker>,
    Marker: 'static,
{
    fn name(&self) -> &'static str {
        type_name::<Function>()
    }

    fn initialize(&mut self, world: &World) -> Access {
        let mut access = Access::default();
        self.state = Some(Function::Param::init(world, &mut access, self.name()));
        access
    }

    fn run(&mut self, world: &mut World, commands: &mut CommandBuffer) {
        let state = self
            .state
            .as_mut()
            .expect("the schedule initializes a system before running it");
        Function::Param::update(state, world);
        let mut parts = WorldParts::new(world, commands);
        let params = Function::Param::get(state, &mut parts);
        self.function.run(params);
    }
}

/// A function taking the whole world, run alone.
struct ExclusiveSystem<Function> {
    function: Function,
}

impl<Function: FnMut(&mut World) + 'static> System for ExclusiveSystem<Function> {
    fn name(&self) -> &'static str {
        type_name::<Function>()
    }

    fn initialize(&mut self, _: &World) -> Access {
        Access {
            exclusive: true,
            ..Access::default()
        }
    }

    fn run(&mut self, world: &mut World, _: &mut CommandBuffer) {
        (self.function)(world);
    }
}

#[doc(hidden)]
pub struct FunctionMarker;

#[doc(hidden)]
pub struct ExclusiveMarker;

/// A function the schedule can run as a system: every parameter is a
/// [`SystemParam`], or the only parameter is `&mut World`.
///
/// A parameter that is not a system parameter is rejected at compile time:
///
/// ```compile_fail,E0277
/// use ludenea_ecs::IntoSystem;
///
/// fn accepts_system<M>(_: impl IntoSystem<M>) {}
///
/// fn counter(_: u32) {}
///
/// accepts_system(counter);
/// ```
pub trait IntoSystem<Marker>: Sized + 'static {
    #[doc(hidden)]
    fn into_system(self) -> Box<dyn System>;

    /// Identifies the function, to refer to it from ordering constraints.
    #[doc(hidden)]
    #[must_use]
    fn system_id() -> TypeId {
        TypeId::of::<Self>()
    }
}

impl<Function, Marker> IntoSystem<(FunctionMarker, Marker)> for Function
where
    Function: SystemParamFunction<Marker>,
    Marker: 'static,
{
    fn into_system(self) -> Box<dyn System> {
        Box::new(FunctionSystem {
            function: self,
            state: None,
            marker: PhantomData,
        })
    }
}

impl<Function: FnMut(&mut World) + 'static> IntoSystem<ExclusiveMarker> for Function {
    fn into_system(self) -> Box<dyn System> {
        Box::new(ExclusiveSystem { function: self })
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use std::cell::Cell;
    use std::rc::Rc;

    use super::IntoSystem;
    use crate::commands::CommandBuffer;
    use crate::{Component, Entity, Query, Res, ResMut, Resource, With, Without, World};

    #[derive(Clone, Copy, Debug, PartialEq)]
    struct Position(i32);
    impl Component for Position {}

    #[derive(Clone, Copy, Debug, PartialEq)]
    struct Velocity(i32);
    impl Component for Velocity {}

    #[derive(Clone, Copy, Debug, PartialEq)]
    struct Player;
    impl Component for Player {}

    #[derive(Clone, Copy, Debug, PartialEq)]
    struct Target(Entity);
    impl Component for Target {}

    #[derive(Clone, Copy, Debug, PartialEq)]
    struct Gravity(i32);
    impl Resource for Gravity {}

    #[derive(Clone, Copy, Debug, PartialEq)]
    struct Steps(u32);
    impl Resource for Steps {}

    fn world() -> World {
        let mut builder = World::builder();
        builder
            .register::<Position>()
            .register::<Velocity>()
            .register::<Player>()
            .register::<Target>()
            .insert_resource(Gravity(-1))
            .insert_resource(Steps(0));
        builder.build()
    }

    /// Initializes `system` on `world` and runs it `times` times, applying
    /// its commands after each run.
    pub(crate) fn run<M>(world: &mut World, system: impl IntoSystem<M>, times: usize) {
        let mut system = system.into_system();
        let mut commands = CommandBuffer::default();
        system.initialize(world);
        for _ in 0..times {
            system.run(world, &mut commands);
            commands.apply(world);
        }
    }

    fn apply_gravity(mut bodies: Query<&mut Velocity>, gravity: Res<Gravity>) {
        for velocity in bodies.iter_mut() {
            velocity.0 += gravity.0;
        }
    }

    #[test]
    fn a_system_reads_a_resource_and_writes_components() {
        let mut world = world();
        let body = world.spawn(Velocity(0));

        run(&mut world, apply_gravity, 2);

        assert_eq!(world.get::<Velocity>(body), Some(&Velocity(-2)));
    }

    #[test]
    fn a_system_writes_a_resource() {
        fn count(mut steps: ResMut<Steps>) {
            steps.0 += 1;
        }
        let mut world = world();

        run(&mut world, count, 3);

        assert_eq!(world.resource::<Steps>(), &Steps(3));
    }

    #[test]
    fn two_disjoint_queries_write_the_same_component() {
        fn split(
            mut players: Query<&mut Position, With<Player>>,
            mut others: Query<&mut Position, Without<Player>>,
        ) {
            for position in players.iter_mut() {
                position.0 += 10;
            }
            for position in others.iter_mut() {
                position.0 -= 10;
            }
        }
        let mut world = world();
        let player = world.spawn((Position(0), Player));
        let other = world.spawn(Position(0));

        run(&mut world, split, 1);

        assert_eq!(world.get::<Position>(player), Some(&Position(10)));
        assert_eq!(world.get::<Position>(other), Some(&Position(-10)));
    }

    #[test]
    fn a_query_reads_another_entity_while_another_query_writes() {
        fn chase(mut chasers: Query<(&Target, &mut Velocity)>, positions: Query<&Position>) {
            for (target, velocity) in chasers.iter_mut() {
                if let Some(position) = positions.get(target.0) {
                    velocity.0 = position.0;
                }
            }
        }
        let mut world = world();
        let target = world.spawn(Position(7));
        let chaser = world.spawn((Target(target), Velocity(0)));

        run(&mut world, chase, 1);

        assert_eq!(world.get::<Velocity>(chaser), Some(&Velocity(7)));
    }

    #[test]
    fn get_ignores_stale_and_unmatched_entities() {
        let mut world = world();
        let stale = world.spawn(Position(1));
        world.despawn(stale).expect("the entity is alive");
        let only_velocity = world.spawn(Velocity(1));
        let both = world.spawn((Position(2), Velocity(3)));

        run(
            &mut world,
            move |positions: Query<&Position>, mut velocities: Query<&mut Velocity>| {
                assert_eq!(positions.get(stale), None);
                assert_eq!(positions.get(only_velocity), None);
                assert_eq!(positions.get(both), Some(&Position(2)));
                *velocities.get_mut(both).expect("the entity has a velocity") = Velocity(30);
            },
            1,
        );

        assert_eq!(world.get::<Velocity>(both), Some(&Velocity(30)));
    }

    #[test]
    fn an_entity_query_needs_no_column() {
        let mut world = world();
        let first = world.spawn(Position(1));
        let second = world.spawn(Velocity(1));

        run(
            &mut world,
            move |entities: Query<Entity>, mut steps: ResMut<Steps>| {
                assert_eq!(entities.iter().collect::<Vec<_>>(), [first, second]);
                assert_eq!(entities.get(second), Some(second));
                steps.0 += 1;
            },
            1,
        );

        assert_eq!(world.resource::<Steps>(), &Steps(1));
    }

    #[test]
    fn several_parameters_read_the_same_data() {
        fn read_twice(
            gravity: Res<Gravity>,
            again: Res<Gravity>,
            positions: Query<&Position>,
            entities: Query<(Entity, &Position)>,
            mut steps: ResMut<Steps>,
        ) {
            assert_eq!(gravity.0, again.0);
            assert_eq!(positions.iter().count(), entities.iter().count());
            steps.0 += 1;
        }
        let mut world = world();
        world.spawn(Position(1));

        run(&mut world, read_twice, 1);

        assert_eq!(world.resource::<Steps>(), &Steps(1));
    }

    #[test]
    fn a_query_sees_archetypes_created_between_runs() {
        let mut world = world();
        let mut system = apply_gravity.into_system();
        system.initialize(&world);
        let body = world.spawn((Velocity(0), Player));

        system.run(&mut world, &mut CommandBuffer::default());

        assert_eq!(world.get::<Velocity>(body), Some(&Velocity(-1)));
    }

    #[test]
    fn an_exclusive_system_takes_the_whole_world() {
        fn spawn_body(world: &mut World) {
            world.spawn(Velocity(5));
        }
        let mut world = world();

        run(&mut world, spawn_body, 2);

        assert_eq!(world.query::<&Velocity>().iter().count(), 2);
    }

    #[test]
    fn a_system_without_parameters_runs() {
        let runs = Rc::new(Cell::new(0));
        let counter = Rc::clone(&runs);
        let mut world = world();

        run(&mut world, move || counter.set(counter.get() + 1), 2);

        assert_eq!(runs.get(), 2);
    }

    #[test]
    #[should_panic(expected = "accesses component")]
    fn two_overlapping_queries_writing_a_component_panic() {
        fn aliased(_: Query<&mut Position>, _: Query<&Position>) {}

        run(&mut world(), aliased, 0);
    }

    #[test]
    #[should_panic(expected = "accesses resource")]
    fn reading_and_writing_a_resource_panics() {
        fn aliased(_: Res<Gravity>, _: ResMut<Gravity>) {}

        run(&mut world(), aliased, 0);
    }

    #[test]
    #[should_panic(expected = "which is not inserted")]
    fn a_missing_resource_panics_naming_the_system() {
        #[derive(Clone, Copy)]
        struct Missing;
        impl Resource for Missing {}
        fn needs_missing(_: Res<Missing>) {}

        run(&mut world(), needs_missing, 0);
    }
}
