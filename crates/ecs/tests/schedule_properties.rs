//! Property tests of the schedule: ambiguity detection against a naive
//! model, independence from insertion order, and rollback across runs.

use std::any::type_name;
use std::collections::BTreeSet;

use ludenea_ecs::{
    Ambiguity, Commands, Component, Conflict, Entity, IntoSystemConfig, Query, Res, ResMut,
    Resource, Schedule, ScheduleError, SystemConfig, Without, World,
};
use proptest::prelude::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct First(u8);
impl Resource for First {}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Second(u8);
impl Resource for Second {}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Third(u8);
impl Resource for Third {}

/// Spawned by a pool system when it runs, to observe the execution order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Ran(usize);
impl Component for Ran {}

/// Declares the pool of systems: each spawns `Ran` with its index and
/// accesses resources as listed. Recording commands is not a conflict, so
/// spawning does not change which systems conflict.
macro_rules! pool {
    ($($index:literal $name:ident ($($param:ident: $kind:ty),*) reads [$($read:literal),*] writes [$($write:literal),*];)*) => {
        $(
            fn $name(mut commands: Commands, $($param: $kind),*) {
                commands.spawn(Ran($index));
            }
        )*

        /// Resources read and written by each pool system: 0 for `First`, 1
        /// for `Second`, 2 for `Third`.
        const ACCESSES: &[(&[usize], &[usize])] = &[$((&[$($read),*], &[$($write),*])),*];

        fn config(index: usize) -> SystemConfig {
            match index {
                $($index => $name.into_config(),)*
                _ => unreachable!("the pool has {} systems", ACCESSES.len()),
            }
        }

        fn before(config: SystemConfig, target: usize) -> SystemConfig {
            match target {
                $($index => config.before($name),)*
                _ => unreachable!("the pool has {} systems", ACCESSES.len()),
            }
        }

        fn name(index: usize) -> &'static str {
            match index {
                $($index => type_name_of($name),)*
                _ => unreachable!("the pool has {} systems", ACCESSES.len()),
            }
        }
    };
}

pool! {
    0 read_first(_a: Res<First>) reads [0] writes [];
    1 write_first(_a: ResMut<First>) reads [] writes [0];
    2 read_second(_a: Res<Second>) reads [1] writes [];
    3 write_second(_a: ResMut<Second>) reads [] writes [1];
    4 read_both(_a: Res<First>, _b: Res<Second>) reads [0, 1] writes [];
    5 write_first_read_second(_a: ResMut<First>, _b: Res<Second>) reads [1] writes [0];
    6 read_first_write_second(_a: Res<First>, _b: ResMut<Second>) reads [0] writes [1];
    7 write_both(_a: ResMut<First>, _b: ResMut<Second>) reads [] writes [0, 1];
    8 read_third(_a: Res<Third>) reads [2] writes [];
    9 write_third(_a: ResMut<Third>) reads [] writes [2];
    10 read_first_write_third(_a: Res<First>, _b: ResMut<Third>) reads [0] writes [2];
    11 idle() reads [] writes [];
}

fn type_name_of<T>(_: T) -> &'static str {
    type_name::<T>()
}

fn resource_name(resource: usize) -> &'static str {
    [
        type_name::<First>(),
        type_name::<Second>(),
        type_name::<Third>(),
    ][resource]
}

/// Resources one pool system writes and the other reads or writes, in
/// identifier order.
fn conflict(first: usize, second: usize) -> Vec<&'static str> {
    let (first_reads, first_writes) = ACCESSES[first];
    let (second_reads, second_writes) = ACCESSES[second];
    let conflicting: BTreeSet<usize> = first_writes
        .iter()
        .filter(|resource| second_reads.contains(resource) || second_writes.contains(resource))
        .chain(
            second_writes
                .iter()
                .filter(|resource| first_reads.contains(resource)),
        )
        .copied()
        .collect();
    conflicting.into_iter().map(resource_name).collect()
}

/// A random schedule over the pool: the systems in insertion order, and
/// constraints `(a, b)` meaning pool system `a` runs before `b`. The
/// constraints follow a hidden ranking, so they never form a cycle.
#[derive(Clone, Debug)]
struct Plan {
    ranked: Vec<usize>,
    inserted: Vec<usize>,
    before: Vec<(usize, usize)>,
}

fn plan() -> impl Strategy<Value = Plan> {
    let ranked =
        prop::sample::subsequence((0..ACCESSES.len()).collect::<Vec<_>>(), 1..=ACCESSES.len())
            .prop_shuffle();
    ranked.prop_flat_map(|ranked| {
        let count = ranked.len();
        let edges = prop::collection::vec((0..count, 0..count), 0..2 * count);
        (Just(ranked.clone()).prop_shuffle(), edges).prop_map(move |(inserted, edges)| Plan {
            ranked: ranked.clone(),
            inserted,
            before: edges
                .into_iter()
                .filter(|(a, b)| a != b)
                .map(|(a, b)| (ranked[a.min(b)], ranked[a.max(b)]))
                .collect(),
        })
    })
}

impl Plan {
    fn world() -> World {
        let mut builder = World::builder();
        builder
            .register::<Ran>()
            .insert_resource(First(0))
            .insert_resource(Second(0))
            .insert_resource(Third(0));
        builder.build()
    }

    /// Builds the schedule and, when it is valid, runs it once and returns
    /// the pool indices in execution order.
    fn run(&self) -> Result<Vec<usize>, ScheduleError> {
        let mut world = Self::world();
        let mut schedule = Schedule::builder();
        for &system in &self.inserted {
            let config = self
                .before
                .iter()
                .filter(|(first, _)| *first == system)
                .fold(config(system), |config, &(_, then)| before(config, then));
            schedule.add(config);
        }
        schedule.build(&world)?.run(&mut world);
        Ok(world.query::<&Ran>().iter().map(|ran| ran.0).collect())
    }

    /// Orders every conflicting pair along the hidden ranking, so the
    /// schedule is never ambiguous.
    fn without_ambiguity(mut self) -> Self {
        for (position, &first) in self.ranked.iter().enumerate() {
            for &second in &self.ranked[position + 1..] {
                if !conflict(first, second).is_empty() {
                    self.before.push((first, second));
                }
            }
        }
        self
    }

    /// Whether a chain of constraints orders `first` before `second`.
    fn reaches(&self, first: usize, second: usize) -> bool {
        let mut seen = BTreeSet::from([first]);
        let mut pending = vec![first];
        while let Some(current) = pending.pop() {
            for &(from, to) in &self.before {
                if from == current && seen.insert(to) {
                    pending.push(to);
                }
            }
        }
        first != second && seen.contains(&second)
    }

    /// The ambiguities the schedule must report, in insertion order.
    fn expected_ambiguities(&self) -> Vec<Ambiguity> {
        let mut ambiguities = Vec::new();
        for (position, &first) in self.inserted.iter().enumerate() {
            for &second in &self.inserted[position + 1..] {
                let conflict = conflict(first, second);
                if !conflict.is_empty()
                    && !self.reaches(first, second)
                    && !self.reaches(second, first)
                {
                    ambiguities.push(Ambiguity {
                        first: name(first),
                        second: name(second),
                        conflict: Conflict::Data(conflict),
                    });
                }
            }
        }
        ambiguities
    }
}

/// Asserts that `order` runs every inserted system exactly once.
fn is_permutation_of(order: &[usize], inserted: &[usize]) -> bool {
    let (mut order, mut inserted) = (order.to_vec(), inserted.to_vec());
    order.sort_unstable();
    inserted.sort_unstable();
    order == inserted
}

/// Position of every pool system in an execution order.
fn positions(order: &[usize]) -> Vec<Option<usize>> {
    let mut positions = vec![None; ACCESSES.len()];
    for (position, &system) in order.iter().enumerate() {
        positions[system] = Some(position);
    }
    positions
}

proptest! {
    #[test]
    fn ambiguities_agree_with_the_model(plan in plan()) {
        let expected = plan.expected_ambiguities();
        match plan.run() {
            Err(ScheduleError::Ambiguous(ambiguities)) => prop_assert_eq!(ambiguities, expected),
            Err(error) => prop_assert!(false, "unexpected error: {error}"),
            Ok(order) => {
                prop_assert_eq!(expected, Vec::new());
                prop_assert!(is_permutation_of(&order, &plan.inserted), "every system runs once");
                let positions = positions(&order);
                for &(first, then) in &plan.before {
                    prop_assert!(positions[first] < positions[then], "{first} runs before {then}");
                }
            }
        }
    }

    #[test]
    fn conflicting_systems_keep_their_order_whatever_the_insertion_order(
        (plan, shuffled) in plan().prop_map(Plan::without_ambiguity).prop_flat_map(|plan| {
            let shuffled = Just(plan.inserted.clone()).prop_shuffle();
            (Just(plan), shuffled)
        })
    ) {
        let order = plan.run().expect("every conflicting pair is ordered");
        let reinserted = Plan { inserted: shuffled, ..plan.clone() };
        let other = reinserted.run().expect("only the insertion order changed");
        prop_assert!(is_permutation_of(&order, &plan.inserted), "every system runs once");
        prop_assert!(is_permutation_of(&other, &plan.inserted), "every system runs once");
        let (positions, other_positions) = (positions(&order), positions(&other));
        for &first in &plan.inserted {
            for &second in &plan.inserted {
                if first < second && !conflict(first, second).is_empty() {
                    prop_assert_eq!(
                        positions[first] < positions[second],
                        other_positions[first] < other_positions[second]
                    );
                }
            }
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Position(i32);
impl Component for Position {}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Velocity(i32);
impl Component for Velocity {}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Frozen;
impl Component for Frozen {}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Tick(u32);
impl Resource for Tick {}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Gravity(i32);
impl Resource for Gravity {}

fn count_ticks(mut tick: ResMut<Tick>) {
    tick.0 += 1;
}

fn apply_gravity(mut bodies: Query<&mut Velocity, Without<Frozen>>, gravity: Res<Gravity>) {
    for velocity in bodies.iter_mut() {
        velocity.0 += gravity.0;
    }
}

fn integrate(mut bodies: Query<(&mut Position, &Velocity)>) {
    for (position, velocity) in bodies.iter_mut() {
        position.0 += velocity.0;
    }
}

fn spawn_every_third_tick(mut commands: Commands, tick: Res<Tick>) {
    if tick.0.is_multiple_of(3) {
        let tick = i32::try_from(tick.0).expect("few ticks");
        commands.spawn((Position(tick), Velocity(tick % 5)));
    }
}

fn despawn_fallen(mut commands: Commands, bodies: Query<(Entity, &Position)>) {
    for (entity, position) in bodies.iter() {
        if position.0 < -50 {
            commands.despawn(entity);
        }
    }
}

fn freeze_fast(mut commands: Commands, bodies: Query<(Entity, &Velocity), Without<Frozen>>) {
    for (entity, velocity) in bodies.iter() {
        if velocity.0 < -20 {
            commands.insert(entity, Frozen);
        }
    }
}

/// What one tick lets an observer see.
type Observation = (Tick, Vec<(Entity, Position)>, Vec<Entity>);

fn observe(world: &World) -> Observation {
    (
        *world.resource::<Tick>(),
        world
            .query::<(Entity, &Position)>()
            .iter()
            .map(|(entity, position)| (entity, *position))
            .collect(),
        world.query::<Entity>().with::<Frozen>().iter().collect(),
    )
}

proptest! {
    #[test]
    fn replaying_a_schedule_after_a_restore_observes_the_same_ticks(
        bodies in prop::collection::vec((-20i32..20, -5i32..5), 0..16),
        gravity in -3i32..1,
        before in 0usize..12,
        after in 1usize..12,
    ) {
        let mut builder = World::builder();
        builder
            .register::<Position>()
            .register::<Velocity>()
            .register::<Frozen>()
            .insert_resource(Tick(0))
            .insert_resource(Gravity(gravity));
        let mut world = builder.build();
        for (position, velocity) in bodies {
            world.spawn((Position(position), Velocity(velocity)));
        }
        let mut schedule = Schedule::builder();
        schedule
            .add(count_ticks)
            .add(spawn_every_third_tick.after(count_ticks))
            .add(apply_gravity)
            .add(integrate.after(apply_gravity))
            .add(despawn_fallen.after(integrate))
            .add(freeze_fast.after(apply_gravity));
        let mut schedule = schedule.build(&world).expect("the schedule is valid");
        let mut tick = |world: &mut World| {
            schedule.run(world);
            observe(world)
        };

        for _ in 0..before {
            tick(&mut world);
        }
        let snapshot = world.snapshot();
        let first: Vec<Observation> = (0..after).map(|_| tick(&mut world)).collect();
        world.restore(&snapshot).expect("same registry");
        let replay: Vec<Observation> = (0..after).map(|_| tick(&mut world)).collect();

        prop_assert_eq!(first, replay);
    }
}
