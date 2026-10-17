//! Schedules: systems run in a deterministic order, conflicting systems in
//! an explicit one.

use std::any::{Any, TypeId, type_name};
use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Debug;

use crate::World;
use crate::access::Access;
use crate::commands::CommandBuffer;
use crate::error::{Ambiguity, ScheduleError};
use crate::registry::RegistryFingerprint;
use crate::system::{IntoSystem, System};

/// A named group of systems, ordered as a whole with
/// [`ScheduleBuilder::configure_set`].
///
/// ```
/// use ludenea_ecs::SystemSet;
///
/// #[derive(Clone, Copy, Debug, PartialEq, Eq)]
/// struct Physics;
/// impl SystemSet for Physics {}
/// ```
pub trait SystemSet: Copy + Eq + Debug + 'static {
    /// Orders every system of `self` before every system of `other`.
    fn before<S: SystemSet>(self, other: S) -> SetOrder {
        SetOrder {
            first: Box::new(self),
            then: Box::new(other),
        }
    }

    /// Orders every system of `self` after every system of `other`.
    fn after<S: SystemSet>(self, other: S) -> SetOrder {
        other.before(self)
    }
}

/// A system set of any type, compared by value.
trait AnySet: Debug {
    fn as_any(&self) -> &dyn Any;

    fn equals(&self, other: &dyn AnySet) -> bool;
}

impl<S: SystemSet> AnySet for S {
    fn as_any(&self) -> &dyn Any {
        self
    }

    fn equals(&self, other: &dyn AnySet) -> bool {
        other.as_any().downcast_ref::<S>() == Some(self)
    }
}

/// Every system of one set runs before every system of another.
pub struct SetOrder {
    first: Box<dyn AnySet>,
    then: Box<dyn AnySet>,
}

/// A system with its sets and ordering constraints.
pub struct SystemConfig {
    system: Box<dyn System>,
    id: TypeId,
    name: &'static str,
    sets: Vec<Box<dyn AnySet>>,
    before: Vec<(TypeId, &'static str)>,
    after: Vec<(TypeId, &'static str)>,
}

#[doc(hidden)]
pub struct ConfigMarker;

/// A system, or a system already configured, to which more sets and
/// ordering constraints can be added.
pub trait IntoSystemConfig<Marker>: Sized {
    #[doc(hidden)]
    fn into_config(self) -> SystemConfig;

    /// Adds the system to `set`.
    fn in_set(self, set: impl SystemSet) -> SystemConfig {
        let mut config = self.into_config();
        config.sets.push(Box::new(set));
        config
    }

    /// Runs the system before `other`.
    fn before<M, S: IntoSystem<M>>(self, _other: S) -> SystemConfig {
        let mut config = self.into_config();
        config.before.push((S::system_id(), type_name::<S>()));
        config
    }

    /// Runs the system after `other`.
    fn after<M, S: IntoSystem<M>>(self, _other: S) -> SystemConfig {
        let mut config = self.into_config();
        config.after.push((S::system_id(), type_name::<S>()));
        config
    }
}

impl<Marker, S: IntoSystem<Marker>> IntoSystemConfig<Marker> for S {
    fn into_config(self) -> SystemConfig {
        SystemConfig {
            system: self.into_system(),
            id: S::system_id(),
            name: type_name::<S>(),
            sets: Vec::new(),
            before: Vec::new(),
            after: Vec::new(),
        }
    }
}

impl IntoSystemConfig<ConfigMarker> for SystemConfig {
    fn into_config(self) -> SystemConfig {
        self
    }
}

/// Collects systems, sets and ordering constraints, then builds a
/// [`Schedule`].
#[derive(Default)]
pub struct ScheduleBuilder {
    systems: Vec<SystemConfig>,
    set_orders: Vec<SetOrder>,
}

impl ScheduleBuilder {
    /// Adds a system. Systems are told apart by their function, so a
    /// function is added once.
    pub fn add<M>(&mut self, system: impl IntoSystemConfig<M>) -> &mut Self {
        self.systems.push(system.into_config());
        self
    }

    /// Orders two sets, such as `Input.before(Physics)`. A set without
    /// systems is allowed.
    pub fn configure_set(&mut self, order: SetOrder) -> &mut Self {
        self.set_orders.push(order);
        self
    }

    /// Initializes every system on `world` and freezes the execution order:
    /// a topological order of the constraints, ties broken by insertion
    /// order.
    ///
    /// # Errors
    ///
    /// Returns, in this priority, [`ScheduleError::DuplicateSystem`],
    /// [`ScheduleError::UnknownSystem`], [`ScheduleError::Cycle`], or
    /// [`ScheduleError::Ambiguous`] with every pair of conflicting systems
    /// left unordered.
    ///
    /// # Panics
    ///
    /// Panics when a system accesses the same data through two conflicting
    /// parameters, has two commands parameters, or accesses a component or
    /// resource missing from `world`.
    pub fn build(self, world: &World) -> Result<Schedule, ScheduleError> {
        let Self {
            systems,
            set_orders,
        } = self;
        let mut indices = BTreeMap::new();
        for (index, config) in systems.iter().enumerate() {
            if indices.insert(config.id, index).is_some() {
                return Err(ScheduleError::DuplicateSystem(config.name));
            }
        }
        let graph = Graph::new(&systems, &set_orders, &indices)?;
        let order = graph.topological_order()?;
        let mut systems: Vec<Box<dyn System>> =
            systems.into_iter().map(|config| config.system).collect();
        let accesses: Vec<Access> = systems
            .iter_mut()
            .map(|system| system.initialize(world))
            .collect();
        let ambiguities = graph.ambiguities(&order, &systems, &accesses, world);
        if !ambiguities.is_empty() {
            return Err(ScheduleError::Ambiguous(ambiguities));
        }
        Ok(Schedule {
            exclusive: accesses.iter().map(|access| access.exclusive).collect(),
            systems,
            order,
            fingerprint: world.registry.fingerprint(),
            commands: CommandBuffer::default(),
        })
    }
}

/// The ordering constraints between systems, by insertion index.
struct Graph {
    successors: Vec<BTreeSet<usize>>,
    predecessors: Vec<BTreeSet<usize>>,
    names: Vec<&'static str>,
}

impl Graph {
    fn new(
        systems: &[SystemConfig],
        set_orders: &[SetOrder],
        indices: &BTreeMap<TypeId, usize>,
    ) -> Result<Self, ScheduleError> {
        let mut graph = Self {
            successors: vec![BTreeSet::new(); systems.len()],
            predecessors: vec![BTreeSet::new(); systems.len()],
            names: systems.iter().map(|config| config.name).collect(),
        };
        let find = |(id, name): &(TypeId, &'static str)| {
            indices
                .get(id)
                .copied()
                .ok_or(ScheduleError::UnknownSystem(name))
        };
        for (index, config) in systems.iter().enumerate() {
            for target in &config.before {
                graph.add_edge(index, find(target)?);
            }
            for target in &config.after {
                graph.add_edge(find(target)?, index);
            }
        }
        let members = |set: &dyn AnySet| -> Vec<usize> {
            systems
                .iter()
                .enumerate()
                .filter(|(_, config)| config.sets.iter().any(|member| member.equals(set)))
                .map(|(index, _)| index)
                .collect()
        };
        for order in set_orders {
            for first in members(order.first.as_ref()) {
                for then in members(order.then.as_ref()) {
                    graph.add_edge(first, then);
                }
            }
        }
        Ok(graph)
    }

    fn add_edge(&mut self, from: usize, to: usize) {
        self.successors[from].insert(to);
        self.predecessors[to].insert(from);
    }

    /// Kahn's algorithm, always taking the ready system inserted first.
    fn topological_order(&self) -> Result<Vec<usize>, ScheduleError> {
        let mut pending: Vec<usize> = self.predecessors.iter().map(BTreeSet::len).collect();
        let mut ready: BTreeSet<usize> = (0..pending.len())
            .filter(|&index| pending[index] == 0)
            .collect();
        let mut order = Vec::with_capacity(pending.len());
        while let Some(index) = ready.pop_first() {
            order.push(index);
            for &next in &self.successors[index] {
                pending[next] -= 1;
                if pending[next] == 0 {
                    ready.insert(next);
                }
            }
        }
        if order.len() == pending.len() {
            Ok(order)
        } else {
            Err(ScheduleError::Cycle(self.cycle(&pending)))
        }
    }

    /// A cycle among the systems Kahn's algorithm could not order, starting
    /// at the first inserted one. Each of them has an unordered
    /// predecessor, so walking predecessors always closes a cycle.
    fn cycle(&self, pending: &[usize]) -> Vec<&'static str> {
        let blocked = |index: usize| pending[index] > 0;
        let start = (0..pending.len())
            .find(|&index| blocked(index))
            .expect("a cycle leaves a system unordered");
        let mut walk = vec![start];
        let mut current = start;
        loop {
            current = *self.predecessors[current]
                .iter()
                .find(|&&index| blocked(index))
                .expect("an unordered system has an unordered predecessor");
            if let Some(position) = walk.iter().position(|&index| index == current) {
                let mut cycle = walk.split_off(position);
                cycle.reverse();
                let first = cycle
                    .iter()
                    .enumerate()
                    .min_by_key(|&(_, &index)| index)
                    .map_or(0, |(position, _)| position);
                cycle.rotate_left(first);
                return cycle.into_iter().map(|index| self.names[index]).collect();
            }
            walk.push(current);
        }
    }

    /// Every pair of conflicting systems with no path between them, in
    /// insertion order.
    // ponytail: reachability as a matrix of booleans, quadratic in the
    // number of systems; switch to bit sets past a few thousand systems.
    fn ambiguities(
        &self,
        order: &[usize],
        systems: &[Box<dyn System>],
        accesses: &[Access],
        world: &World,
    ) -> Vec<Ambiguity> {
        let count = order.len();
        let mut reachable = vec![vec![false; count]; count];
        for &index in order.iter().rev() {
            for &next in &self.successors[index] {
                let through = reachable[next].clone();
                reachable[index][next] = true;
                for (reached, through) in reachable[index].iter_mut().zip(through) {
                    *reached |= through;
                }
            }
        }
        let mut ambiguities = Vec::new();
        for first in 0..count {
            for second in first + 1..count {
                if reachable[first][second] || reachable[second][first] {
                    continue;
                }
                if let Some(conflict) = accesses[first].conflict(&accesses[second], &world.registry)
                {
                    ambiguities.push(Ambiguity {
                        first: systems[first].name(),
                        second: systems[second].name(),
                        conflict,
                    });
                }
            }
        }
        ambiguities
    }
}

/// Systems frozen in a deterministic execution order, built with
/// [`Schedule::builder`].
pub struct Schedule {
    systems: Vec<Box<dyn System>>,
    exclusive: Vec<bool>,
    order: Vec<usize>,
    fingerprint: RegistryFingerprint,
    commands: CommandBuffer,
}

impl Schedule {
    /// Starts a [`ScheduleBuilder`].
    #[must_use]
    pub fn builder() -> ScheduleBuilder {
        ScheduleBuilder::default()
    }

    /// Runs every system once, in the frozen order. Commands are applied
    /// before each exclusive system and at the end, in recording order.
    ///
    /// The world may differ from the one the schedule was built with, as
    /// long as its registry is identical.
    ///
    /// # Panics
    ///
    /// Panics when the registry of `world` differs from the one the
    /// schedule was built with.
    pub fn run(&mut self, world: &mut World) {
        assert!(
            world.registry.has_fingerprint(&self.fingerprint),
            "the schedule runs on a world whose registry differs from the one it was built with"
        );
        for &index in &self.order {
            if self.exclusive[index] {
                self.commands.apply(world);
            }
            self.systems[index].run(world, &mut self.commands);
        }
        self.commands.apply(world);
    }
}

#[cfg(test)]
mod tests {
    use std::any::type_name;

    use crate::{
        Ambiguity, Commands, Component, Conflict, Entity, IntoSystemConfig, Query, Res, ResMut,
        Resource, Schedule, ScheduleError, SystemSet, World,
    };

    #[derive(Clone, Copy, Debug, PartialEq)]
    struct Position(i32);
    impl Component for Position {}

    #[derive(Clone, Copy, Debug, PartialEq)]
    struct Spawner(u8);
    impl Component for Spawner {}

    /// The systems that ran, one decimal digit each, most recent last.
    #[derive(Clone, Copy, Debug, PartialEq)]
    struct Trace(u64);
    impl Resource for Trace {}

    #[derive(Clone, Copy, Debug, PartialEq)]
    struct Count(usize);
    impl Resource for Count {}

    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    struct Early;
    impl SystemSet for Early {}

    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    struct Middle;
    impl SystemSet for Middle {}

    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    struct Late;
    impl SystemSet for Late {}

    fn world() -> World {
        let mut builder = World::builder();
        builder
            .register::<Position>()
            .register::<Spawner>()
            .insert_resource(Trace(0))
            .insert_resource(Count(0));
        builder.build()
    }

    fn one(mut trace: ResMut<Trace>) {
        trace.0 = trace.0 * 10 + 1;
    }

    fn two(mut trace: ResMut<Trace>) {
        trace.0 = trace.0 * 10 + 2;
    }

    fn three(mut trace: ResMut<Trace>) {
        trace.0 = trace.0 * 10 + 3;
    }

    fn read_trace(_: Res<Trace>) {}

    fn trace_after_one_run(schedule: &mut crate::ScheduleBuilder) -> u64 {
        let mut world = world();
        let mut built = std::mem::take(schedule)
            .build(&world)
            .expect("the schedule is valid");
        built.run(&mut world);
        world.resource::<Trace>().0
    }

    #[test]
    fn conflicting_systems_run_in_the_declared_order() {
        let mut schedule = Schedule::builder();
        schedule.add(three.after(two)).add(one.before(two)).add(two);

        assert_eq!(trace_after_one_run(&mut schedule), 123);
    }

    #[test]
    fn set_orders_apply_to_every_member() {
        let mut schedule = Schedule::builder();
        schedule
            .configure_set(Late.after(Early))
            .add(two.in_set(Late))
            .add(three.in_set(Late).after(two))
            .add(one.in_set(Early));

        assert_eq!(trace_after_one_run(&mut schedule), 123);
    }

    #[test]
    fn unordered_systems_without_conflict_run_in_insertion_order() {
        fn spawn_first(mut commands: Commands) {
            commands.spawn(Spawner(1));
        }
        fn spawn_second(mut commands: Commands) {
            commands.spawn(Spawner(2));
        }
        let spawned = |first_inserted_first: bool| {
            let mut world = world();
            let mut schedule = Schedule::builder();
            if first_inserted_first {
                schedule.add(spawn_first).add(spawn_second);
            } else {
                schedule.add(spawn_second).add(spawn_first);
            }
            schedule.build(&world).expect("no conflict").run(&mut world);
            world
                .query::<(Entity, &Spawner)>()
                .iter()
                .map(|(entity, spawner)| (entity.to_string(), spawner.0))
                .collect::<Vec<_>>()
        };

        assert_eq!(spawned(true), [("0v0".into(), 1), ("1v0".into(), 2)]);
        assert_eq!(spawned(false), [("0v0".into(), 2), ("1v0".into(), 1)]);
    }

    #[test]
    fn every_unordered_conflicting_pair_is_reported() {
        let world = world();
        let mut schedule = Schedule::builder();
        schedule.add(one).add(read_trace).add(two.after(one));

        let error = schedule.build(&world).err();

        let ambiguity = |first, second| Ambiguity {
            first,
            second,
            conflict: Conflict::Data(vec![type_name::<Trace>()]),
        };
        assert_eq!(
            error,
            Some(ScheduleError::Ambiguous(vec![
                ambiguity(type_name_of(one), type_name_of(read_trace)),
                ambiguity(type_name_of(read_trace), type_name_of(two)),
            ]))
        );
    }

    #[test]
    fn disjoint_queries_are_not_ambiguous() {
        fn move_spawners(_: Query<&mut Position, crate::With<Spawner>>) {}
        fn move_others(_: Query<&mut Position, crate::Without<Spawner>>) {}
        let world = world();
        let mut schedule = Schedule::builder();
        schedule.add(move_spawners).add(move_others);

        assert!(schedule.build(&world).is_ok());
    }

    #[test]
    fn an_exclusive_system_must_be_ordered_against_every_other() {
        fn exclusive(_: &mut World) {}
        fn idle() {}
        let world = world();
        let mut schedule = Schedule::builder();
        schedule.add(idle).add(exclusive);

        let Err(ScheduleError::Ambiguous(ambiguities)) = schedule.build(&world) else {
            panic!("an unordered exclusive system is ambiguous");
        };

        assert_eq!(ambiguities[0].conflict, Conflict::Exclusive);
    }

    #[test]
    fn a_cycle_is_reported_from_the_first_inserted_system() {
        let world = world();
        let mut schedule = Schedule::builder();
        schedule
            .add(three.before(two))
            .add(one.before(three))
            .add(two.before(one));

        assert_eq!(
            schedule.build(&world).err(),
            Some(ScheduleError::Cycle(vec![
                type_name_of(three),
                type_name_of(two),
                type_name_of(one),
            ]))
        );
    }

    #[test]
    fn a_system_ordered_after_itself_is_a_cycle() {
        let world = world();
        let mut schedule = Schedule::builder();
        schedule.add(one.after(one));

        assert_eq!(
            schedule.build(&world).err(),
            Some(ScheduleError::Cycle(vec![type_name_of(one)]))
        );
    }

    #[test]
    fn a_cycle_is_reported_before_ambiguities() {
        let world = world();
        let mut schedule = Schedule::builder();
        schedule
            .add(one.before(two))
            .add(two.before(one))
            .add(three);

        assert!(matches!(
            schedule.build(&world),
            Err(ScheduleError::Cycle(_))
        ));
    }

    #[test]
    fn an_ordering_constraint_on_a_missing_system_is_an_error() {
        let world = world();
        let mut schedule = Schedule::builder();
        schedule.add(one.before(two));

        assert_eq!(
            schedule.build(&world).err(),
            Some(ScheduleError::UnknownSystem(type_name_of(two)))
        );
    }

    #[test]
    fn adding_a_system_twice_is_an_error() {
        let world = world();
        let mut schedule = Schedule::builder();
        schedule.add(one).add(one);

        assert_eq!(
            schedule.build(&world).err(),
            Some(ScheduleError::DuplicateSystem(type_name_of(one)))
        );
    }

    #[test]
    fn commands_are_applied_before_an_exclusive_system() {
        fn spawn(mut commands: Commands) {
            commands.spawn(Position(1));
        }
        fn count(world: &mut World) {
            let count = world.query::<&Position>().iter().count();
            world.resource_mut::<Count>().0 = count;
        }
        let mut world = world();
        let mut schedule = Schedule::builder();
        schedule.add(spawn).add(count.after(spawn));

        schedule.build(&world).expect("ordered").run(&mut world);

        assert_eq!(world.resource::<Count>(), &Count(1));
    }

    #[test]
    fn a_schedule_runs_on_another_world_with_the_same_registry() {
        let world = world();
        let mut schedule = Schedule::builder();
        schedule.add(one);
        let mut schedule = schedule.build(&world).expect("valid");
        let mut peer = self::world();

        schedule.run(&mut peer);

        assert_eq!(peer.resource::<Trace>(), &Trace(1));
    }

    #[test]
    #[should_panic(expected = "registry differs")]
    fn a_schedule_rejects_a_world_with_another_registry() {
        let world = world();
        let mut schedule = Schedule::builder();
        schedule.add(one);
        let mut schedule = schedule.build(&world).expect("valid");

        schedule.run(&mut World::builder().build());
    }

    #[test]
    fn an_empty_schedule_runs() {
        let mut world = world();

        Schedule::builder()
            .build(&world)
            .expect("nothing to order")
            .run(&mut world);

        assert_eq!(world.resource::<Trace>(), &Trace(0));
        assert_eq!(world.query::<&Position>().iter().count(), 0);
    }

    #[test]
    fn an_exclusive_system_may_restore_a_snapshot_mid_schedule() {
        fn spawn(mut commands: Commands) {
            commands.spawn(Position(1));
        }
        fn count(positions: Query<&Position>, mut count: ResMut<Count>) {
            count.0 = positions.iter().count() + 10;
        }
        let mut world = world();
        let snapshot = world.snapshot();
        let mut schedule = Schedule::builder();
        schedule
            .configure_set(Early.before(Middle))
            .configure_set(Middle.before(Late))
            .add(spawn.in_set(Early))
            .add(
                (move |world: &mut World| world.restore(&snapshot).expect("same registry"))
                    .in_set(Middle),
            )
            .add(count.in_set(Late));
        let mut schedule = schedule.build(&world).expect("ordered");

        schedule.run(&mut world);
        schedule.run(&mut world);

        assert_eq!(world.resource::<Count>(), &Count(10));
        assert_eq!(world.query::<&Position>().iter().count(), 0);
    }

    #[test]
    fn a_schedule_keeps_working_when_a_restore_recreates_archetypes() {
        fn move_bodies(mut bodies: Query<&mut Position>) {
            for position in bodies.iter_mut() {
                position.0 += 1;
            }
        }
        let mut world = world();
        let snapshot = world.snapshot();
        let mut schedule = Schedule::builder();
        schedule.add(move_bodies);
        let mut schedule = schedule.build(&world).expect("valid");
        world.spawn(Position(0));
        schedule.run(&mut world);

        world.restore(&snapshot).expect("same registry");
        world.spawn(Spawner(1));
        let body = world.spawn((Position(5), Spawner(2)));
        schedule.run(&mut world);

        assert_eq!(world.get::<Position>(body), Some(&Position(6)));
    }

    #[test]
    fn error_messages_name_the_systems_and_the_fix() {
        let ambiguous = ScheduleError::Ambiguous(vec![
            Ambiguity {
                first: "game::integrate",
                second: "game::apply_gravity",
                conflict: Conflict::Data(vec!["game::Velocity", "game::Gravity"]),
            },
            Ambiguity {
                first: "game::integrate",
                second: "game::save",
                conflict: Conflict::Exclusive,
            },
        ]);
        let cycle = ScheduleError::Cycle(vec!["game::input", "game::physics"]);

        insta::assert_snapshot!(format!("{ambiguous}\n\n{cycle}"));
    }

    fn type_name_of<T>(_: T) -> &'static str {
        type_name::<T>()
    }
}
