use ludenea_ecs::{
    Component, EcsError, Entity, Query, QueryError, With, Without, World, accesses, check_accesses,
};
use serde::{Deserialize, Serialize};

#[derive(Component, Serialize, Deserialize, Debug, PartialEq)]
struct Health(u32);

#[derive(Component, Serialize, Deserialize, Debug, PartialEq)]
struct Armor(u32);

#[derive(Component, Serialize, Deserialize, Debug, PartialEq)]
struct Dead;

#[derive(Query)]
#[query(filter = Without<Dead>)]
struct Fighter<'w> {
    entity: Entity,
    health: &'w mut Health,
    armor: Option<&'w Armor>,
}

#[derive(Query)]
struct Status<'w> {
    entity: Entity,
    health: &'w Health,
}

fn world() -> World {
    let mut world = World::new();
    world.register_component::<Health>().unwrap();
    world.register_component::<Armor>().unwrap();
    world.register_component::<Dead>().unwrap();
    world
}

/// Spawns four entities: 0 Health(10), 1 Health(20)+Armor(5), 2 Health(30)+Dead, 3 Armor(1).
fn populated() -> (World, Vec<Entity>) {
    let mut world = world();
    let entities: Vec<Entity> = (0..4).map(|_| world.spawn()).collect();
    world.insert(entities[0], Health(10)).unwrap();
    world.insert(entities[1], Health(20)).unwrap();
    world.insert(entities[1], Armor(5)).unwrap();
    world.insert(entities[2], Health(30)).unwrap();
    world.insert(entities[2], Dead).unwrap();
    world.insert(entities[3], Armor(1)).unwrap();
    (world, entities)
}

#[test]
fn query_iterates_in_ascending_entity_order() {
    let mut world = world();
    let first = world.spawn();
    let second = world.spawn();
    let third = world.spawn();
    // Insert out of order and reuse a freed slot, which must still come back by index.
    world.insert(third, Health(3)).unwrap();
    world.insert(first, Health(1)).unwrap();
    world.despawn(second).unwrap();
    let reused = world.spawn();
    world.insert(reused, Health(2)).unwrap();

    let query = Query::<(Entity, &Health)>::new(&world).unwrap();
    let seen: Vec<(Entity, u32)> = query
        .iter()
        .map(|(entity, health)| (entity, health.0))
        .collect();
    assert_eq!(seen, vec![(first, 1), (reused, 2), (third, 3)]);
}

#[test]
fn with_and_without_filters_select_expected_entities() {
    let (world, entities) = populated();

    let with_armor = Query::<Entity, (With<Health>, With<Armor>)>::new(&world).unwrap();
    assert_eq!(with_armor.iter().collect::<Vec<_>>(), vec![entities[1]]);

    let alive = Query::<&Health, Without<Dead>>::new(&world).unwrap();
    let values: Vec<u32> = alive.iter().map(|health| health.0).collect();
    assert_eq!(values, vec![10, 20]);

    let optional = Query::<(Entity, Option<&Armor>), With<Health>>::new(&world).unwrap();
    let armors: Vec<Option<u32>> = optional
        .iter()
        .map(|(_, armor)| armor.map(|armor| armor.0))
        .collect();
    assert_eq!(armors, vec![None, Some(5), None]);
}

#[test]
fn get_mut_on_stale_entity_is_no_such_entity() {
    let (mut world, entities) = populated();
    world.despawn(entities[0]).unwrap();
    let reused = world.spawn();
    world.insert(reused, Health(99)).unwrap();

    let mut query = Query::<&mut Health>::new(&world).unwrap();
    assert!(matches!(
        query.get_mut(entities[0]),
        Err(QueryError::NoSuchEntity(entity)) if entity == entities[0]
    ));
    assert_eq!(query.get_mut(reused).unwrap().0, 99);
}

#[test]
fn get_on_non_matching_entity_is_does_not_match() {
    let (world, entities) = populated();
    let query = Query::<&Health, Without<Dead>>::new(&world).unwrap();
    assert!(matches!(
        query.get(entities[3]),
        Err(QueryError::DoesNotMatch(entity)) if entity == entities[3]
    ));
    assert!(matches!(
        query.get(entities[2]),
        Err(QueryError::DoesNotMatch(entity)) if entity == entities[2]
    ));
    assert_eq!(query.get(entities[1]).unwrap().0, 20);
}

#[test]
fn iter_mut_writes_every_matching_component() {
    let (world, _) = populated();
    {
        let mut query = Query::<(&mut Health, Option<&mut Armor>)>::new(&world).unwrap();
        // Collect first so every `&mut` is alive at once: the items must be disjoint.
        let items: Vec<_> = query.iter_mut().collect();
        for (health, armor) in items {
            health.0 += 1;
            if let Some(armor) = armor {
                armor.0 *= 10;
            }
        }
    }
    let query = Query::<(&Health, Option<&Armor>)>::new(&world).unwrap();
    let values: Vec<(u32, Option<u32>)> = query
        .iter()
        .map(|(health, armor)| (health.0, armor.map(|armor| armor.0)))
        .collect();
    assert_eq!(values, vec![(11, None), (21, Some(50)), (31, None)]);
}

#[test]
fn derived_query_with_filter_skips_filtered_entities() {
    let (world, entities) = populated();
    {
        let mut query = Query::<Fighter>::new(&world).unwrap();
        for fighter in query.iter_mut() {
            fighter.health.0 += fighter.armor.map_or(0, |armor| armor.0);
        }
        assert!(matches!(
            query.get_mut(entities[2]),
            Err(QueryError::DoesNotMatch(_))
        ));
        assert_eq!(query.get_mut(entities[1]).unwrap().entity, entities[1]);
    }
    let health: Vec<u32> = entities[..3]
        .iter()
        .map(|&entity| world.get::<Health>(entity).unwrap().0)
        .collect();
    assert_eq!(health, vec![10, 25, 30]);
}

#[test]
fn derived_read_only_query_supports_iter() {
    let (world, entities) = populated();
    let query = Query::<Status>::new(&world).unwrap();
    let seen: Vec<(Entity, u32)> = query
        .iter()
        .map(|status| (status.entity, status.health.0))
        .collect();
    assert_eq!(
        seen,
        vec![(entities[0], 10), (entities[1], 20), (entities[2], 30)]
    );
    assert_eq!(query.get(entities[2]).unwrap().health.0, 30);
}

#[test]
fn query_with_conflicting_data_is_rejected() {
    let error = check_accesses(&accesses::<(&mut Health, &Health), ()>()).unwrap_err();
    assert_eq!(error, (Health::NAME, 0, 1));
    assert!(check_accesses(&accesses::<(&mut Health, Option<&Health>), ()>()).is_err());
    assert!(check_accesses(&accesses::<(&Health, Option<&Health>, &mut Armor), ()>()).is_ok());

    let world = world();
    assert!(matches!(
        Query::<(&mut Health, &Health)>::new(&world),
        Err(EcsError::ComponentBorrowed(name)) if name == Health::NAME
    ));
}

#[test]
fn query_on_unregistered_component_errors_with_its_name() {
    let world = World::new();
    assert!(matches!(
        Query::<&Health>::new(&world),
        Err(EcsError::UnregisteredComponent(name)) if name == Health::NAME
    ));
}
