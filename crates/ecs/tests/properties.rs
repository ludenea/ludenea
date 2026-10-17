//! Property tests of the world: random operation sequences checked against
//! a naive model, and the rollback guarantee.

use std::collections::{BTreeMap, BTreeSet};
use std::mem;

use ludenea_ecs::{Bundle, Component, EcsError, Entity, World};
use proptest::prelude::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Small(u8);
impl Component for Small {}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Large(u16);
impl Component for Large {}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Tag;
impl Component for Tag {}

fn world() -> World {
    let mut builder = World::builder();
    builder
        .register::<Small>()
        .register::<Large>()
        .register::<Tag>();
    builder.build()
}

/// One world operation. A `usize` target picks, modulo their count, one of
/// the entities spawned so far, alive or not, so stale identifiers are
/// exercised too.
#[derive(Clone, Copy, Debug)]
enum Operation {
    SpawnEmpty,
    SpawnSmall(u8),
    SpawnSmallLarge(u8, u16),
    SpawnLargeTag(u16),
    Despawn(usize),
    InsertSmall(usize, u8),
    InsertLargeTag(usize, u16),
    RemoveSmall(usize),
    RemoveLarge(usize),
    RemoveTag(usize),
}

fn operation() -> impl Strategy<Value = Operation> {
    prop_oneof![
        Just(Operation::SpawnEmpty),
        any::<u8>().prop_map(Operation::SpawnSmall),
        any::<(u8, u16)>().prop_map(|(small, large)| Operation::SpawnSmallLarge(small, large)),
        any::<u16>().prop_map(Operation::SpawnLargeTag),
        any::<usize>().prop_map(Operation::Despawn),
        any::<(usize, u8)>().prop_map(|(target, small)| Operation::InsertSmall(target, small)),
        any::<(usize, u16)>().prop_map(|(target, large)| Operation::InsertLargeTag(target, large)),
        any::<usize>().prop_map(Operation::RemoveSmall),
        any::<usize>().prop_map(Operation::RemoveLarge),
        any::<usize>().prop_map(Operation::RemoveTag),
    ]
}

/// The components of one entity in the model.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct Components {
    small: Option<Small>,
    large: Option<Large>,
    tag: bool,
}

/// The naive model: every entity spawned so far, and the live ones.
#[derive(Clone, Debug, Default)]
struct Model {
    spawned: Vec<Entity>,
    alive: BTreeMap<Entity, Components>,
}

impl Model {
    fn target(&self, index: usize) -> Entity {
        self.spawned[index % self.spawned.len()]
    }

    /// Applies `update` to the components of a live `entity`, or reports it
    /// stale.
    fn update<R>(
        &mut self,
        entity: Entity,
        update: impl FnOnce(&mut Components) -> R,
    ) -> Result<R, EcsError> {
        self.alive
            .get_mut(&entity)
            .map(update)
            .ok_or(EcsError::StaleEntity(entity))
    }
}

/// What an operation returned, for comparing two runs.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Outcome {
    Spawned(Entity),
    Done(Result<(), EcsError>),
    RemovedSmall(Result<Option<Small>, EcsError>),
    RemovedLarge(Result<Option<Large>, EcsError>),
    RemovedTag(Result<Option<Tag>, EcsError>),
}

fn spawn(
    world: &mut World,
    model: &mut Model,
    bundle: impl Bundle,
    components: Components,
) -> Outcome {
    let entity = world.spawn(bundle);
    assert!(
        !model.alive.contains_key(&entity),
        "{entity} is already alive"
    );
    model.spawned.push(entity);
    model.alive.insert(entity, components);
    Outcome::Spawned(entity)
}

/// Applies `operation` to both the world and the model, checks that their
/// results agree and returns the result of the world. An operation with a
/// target spawns the empty bundle instead while nothing was spawned.
fn step(world: &mut World, model: &mut Model, operation: Operation) -> Outcome {
    let operation = match operation {
        Operation::Despawn(_)
        | Operation::InsertSmall(..)
        | Operation::InsertLargeTag(..)
        | Operation::RemoveSmall(_)
        | Operation::RemoveLarge(_)
        | Operation::RemoveTag(_)
            if model.spawned.is_empty() =>
        {
            Operation::SpawnEmpty
        }
        operation => operation,
    };
    match operation {
        Operation::SpawnEmpty => spawn(world, model, (), Components::default()),
        Operation::SpawnSmall(small) => {
            let components = Components {
                small: Some(Small(small)),
                ..Components::default()
            };
            spawn(world, model, Small(small), components)
        }
        Operation::SpawnSmallLarge(small, large) => {
            let components = Components {
                small: Some(Small(small)),
                large: Some(Large(large)),
                tag: false,
            };
            spawn(world, model, (Small(small), Large(large)), components)
        }
        Operation::SpawnLargeTag(large) => {
            let components = Components {
                large: Some(Large(large)),
                tag: true,
                ..Components::default()
            };
            spawn(world, model, (Large(large), Tag), components)
        }
        Operation::Despawn(index) => {
            let entity = model.target(index);
            let result = world.despawn(entity);
            let expected = model
                .alive
                .remove(&entity)
                .map(|_| ())
                .ok_or(EcsError::StaleEntity(entity));
            assert_eq!(result, expected);
            Outcome::Done(result)
        }
        Operation::InsertSmall(index, small) => {
            let entity = model.target(index);
            let result = world.insert(entity, Small(small));
            let expected = model.update(entity, |components| components.small = Some(Small(small)));
            assert_eq!(result, expected);
            Outcome::Done(result)
        }
        Operation::InsertLargeTag(index, large) => {
            let entity = model.target(index);
            let result = world.insert(entity, (Large(large), Tag));
            let expected = model.update(entity, |components| {
                components.large = Some(Large(large));
                components.tag = true;
            });
            assert_eq!(result, expected);
            Outcome::Done(result)
        }
        Operation::RemoveSmall(index) => {
            let entity = model.target(index);
            let result = world.remove::<Small>(entity);
            assert_eq!(
                result,
                model.update(entity, |components| components.small.take())
            );
            Outcome::RemovedSmall(result)
        }
        Operation::RemoveLarge(index) => {
            let entity = model.target(index);
            let result = world.remove::<Large>(entity);
            assert_eq!(
                result,
                model.update(entity, |components| components.large.take())
            );
            Outcome::RemovedLarge(result)
        }
        Operation::RemoveTag(index) => {
            let entity = model.target(index);
            let result = world.remove::<Tag>(entity);
            let expected = model.update(entity, |components| {
                mem::take(&mut components.tag).then_some(Tag)
            });
            assert_eq!(result, expected);
            Outcome::RemovedTag(result)
        }
    }
}

/// Checks that every lookup and query of the world agrees with the model.
fn check(world: &World, model: &Model) {
    for &entity in &model.spawned {
        let expected = model.alive.get(&entity);
        assert_eq!(world.contains(entity), expected.is_some());
        assert_eq!(
            world.get::<Small>(entity),
            expected.and_then(|components| components.small.as_ref())
        );
        assert_eq!(
            world.get::<Large>(entity),
            expected.and_then(|components| components.large.as_ref())
        );
        assert_eq!(
            world.get::<Tag>(entity).is_some(),
            expected.is_some_and(|components| components.tag)
        );
    }

    let alive: BTreeSet<Entity> = world.query::<Entity>().iter().collect();
    assert_eq!(alive, model.alive.keys().copied().collect());

    let small: BTreeMap<Entity, Small> = world
        .query::<(Entity, &Small)>()
        .iter()
        .map(|(entity, small)| (entity, *small))
        .collect();
    let expected: BTreeMap<Entity, Small> = model
        .alive
        .iter()
        .filter_map(|(&entity, components)| Some((entity, components.small?)))
        .collect();
    assert_eq!(small, expected);

    let small_untagged: BTreeSet<Entity> = world
        .query::<Entity>()
        .with::<Small>()
        .without::<Tag>()
        .iter()
        .collect();
    let expected: BTreeSet<Entity> = model
        .alive
        .iter()
        .filter(|(_, components)| components.small.is_some() && !components.tag)
        .map(|(&entity, _)| entity)
        .collect();
    assert_eq!(small_untagged, expected);
}

/// What a run lets an observer see after one operation: its outcome and the
/// iteration order of the world.
type Observation = (Outcome, Vec<Entity>, Vec<(Entity, Small)>);

fn observe(world: &World, outcome: Outcome) -> Observation {
    let entities = world.query::<Entity>().iter().collect();
    let small = world
        .query::<(Entity, &Small)>()
        .iter()
        .map(|(entity, small)| (entity, *small))
        .collect();
    (outcome, entities, small)
}

fn run(world: &mut World, model: &mut Model, operations: &[Operation]) -> Vec<Observation> {
    operations
        .iter()
        .map(|&operation| {
            let outcome = step(world, model, operation);
            observe(world, outcome)
        })
        .collect()
}

proptest! {
    #[test]
    fn the_world_agrees_with_the_model(operations in prop::collection::vec(operation(), 1..64)) {
        let mut world = world();
        let mut model = Model::default();
        for operation in operations {
            step(&mut world, &mut model, operation);
            check(&world, &model);
        }
    }

    #[test]
    fn replaying_after_a_restore_observes_the_same_run(
        before in prop::collection::vec(operation(), 0..32),
        after in prop::collection::vec(operation(), 1..32),
    ) {
        let mut world = world();
        let mut model = Model::default();
        run(&mut world, &mut model, &before);
        let snapshot = world.snapshot();
        let model_at_snapshot = model.clone();

        let first = run(&mut world, &mut model, &after);
        world.restore(&snapshot).expect("same registry");
        let mut model = model_at_snapshot;
        let replay = run(&mut world, &mut model, &after);

        prop_assert_eq!(first, replay);
    }
}
