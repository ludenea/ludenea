use ludenea_ecs::{Component, EcsError, Resource, World};
use serde::{Deserialize, Serialize};

#[derive(Component, Serialize, Deserialize, Debug, PartialEq)]
struct Health(u32);

#[derive(Component, Serialize, Deserialize, Debug, PartialEq)]
#[component(name = "game::Hp")]
struct Hp(u32);

#[derive(Component, Serialize, Deserialize, Debug, PartialEq)]
#[component(name = "game::Hp")]
struct OtherHp(u32);

#[derive(Resource, Serialize, Deserialize, Debug, PartialEq)]
struct Score(u64);

#[derive(Resource, Serialize, Deserialize, Debug, PartialEq)]
#[resource(name = "game::Score")]
struct NamedScore(u64);

#[derive(Resource, Serialize, Deserialize, Debug, PartialEq)]
#[resource(name = "game::Score")]
struct OtherNamedScore(u64);

#[test]
fn spawn_reuses_freed_index_with_bumped_generation() {
    let mut world = World::new();
    let a = world.spawn();
    let _b = world.spawn();
    world.despawn(a).unwrap();
    let c = world.spawn();
    assert_eq!(c.index(), a.index());
    assert_eq!(c.generation(), a.generation() + 1);
    assert!(!world.is_alive(a));
    assert!(world.is_alive(c));
}

#[test]
fn free_list_is_last_in_first_out() {
    let mut world = World::new();
    let entities: Vec<_> = (0..3).map(|_| world.spawn()).collect();
    world.despawn(entities[1]).unwrap();
    world.despawn(entities[2]).unwrap();
    assert_eq!(world.spawn().index(), 2);
    assert_eq!(world.spawn().index(), 1);
}

#[test]
fn entities_iterate_in_ascending_index() {
    let mut world = World::new();
    let entities: Vec<_> = (0..4).map(|_| world.spawn()).collect();
    world.despawn(entities[2]).unwrap();
    world.despawn(entities[0]).unwrap();
    let reused = world.spawn();
    let listed: Vec<_> = world.entities().collect();
    assert_eq!(listed, vec![reused, entities[1], entities[3]]);
    assert_eq!(reused.to_string(), "0v1");
}

#[test]
fn get_on_stale_entity_returns_none() {
    let mut world = World::new();
    world.register_component::<Health>().unwrap();
    let a = world.spawn();
    world.insert(a, Health(10)).unwrap();
    assert_eq!(*world.get::<Health>(a).unwrap(), Health(10));
    world.despawn(a).unwrap();
    let reused = world.spawn();
    assert!(world.get::<Health>(a).is_none());
    assert!(world.get::<Health>(reused).is_none());
    assert!(world.get_mut::<Health>(a).is_none());
    assert!(matches!(world.despawn(a), Err(EcsError::NoSuchEntity(e)) if e == a));
    assert!(matches!(
        world.insert(a, Health(1)),
        Err(EcsError::NoSuchEntity(_))
    ));
}

#[test]
fn components_can_be_mutated_and_removed() {
    let mut world = World::new();
    world.register_component::<Health>().unwrap();
    let a = world.spawn();
    world.insert(a, Health(10)).unwrap();
    world.get_mut::<Health>(a).unwrap().0 = 7;
    assert_eq!(world.remove::<Health>(a).unwrap(), Some(Health(7)));
    assert_eq!(world.remove::<Health>(a).unwrap(), None);
}

#[test]
fn insert_unregistered_component_errors_with_its_name() {
    let mut world = World::new();
    let a = world.spawn();
    let error = world.insert(a, Health(1)).unwrap_err();
    assert!(
        matches!(&error, EcsError::UnregisteredComponent(name) if name == <Health as Component>::NAME)
    );
    assert!(error.to_string().contains(<Health as Component>::NAME));
}

#[test]
fn resources_can_be_read_and_mutated() {
    let mut world = World::new();
    world.register_resource(Score(1)).unwrap();
    world.resource_mut::<Score>().unwrap().0 += 1;
    assert_eq!(*world.resource::<Score>().unwrap(), Score(2));
    assert!(world.resource::<NamedScore>().is_none());
}

#[test]
fn runtime_resources_are_accessible() {
    struct Cache(Vec<u8>);
    let mut world = World::new();
    assert!(world.runtime::<Cache>().is_none());
    world.insert_runtime(Cache(vec![1]));
    world.runtime_mut::<Cache>().unwrap().0.push(2);
    assert_eq!(world.runtime::<Cache>().unwrap().0, vec![1, 2]);
}

#[test]
fn derive_uses_module_path_by_default_and_honours_override() {
    assert_eq!(
        <Health as Component>::NAME,
        concat!(module_path!(), "::Health")
    );
    assert_eq!(<Hp as Component>::NAME, "game::Hp");
    assert_eq!(
        <Score as Resource>::NAME,
        concat!(module_path!(), "::Score")
    );
    assert_eq!(<NamedScore as Resource>::NAME, "game::Score");
}

#[test]
fn registering_two_types_with_one_name_is_duplicate_name() {
    let mut world = World::new();
    world.register_component::<Hp>().unwrap();
    world.register_component::<Hp>().unwrap();
    let error = world.register_component::<OtherHp>().unwrap_err();
    assert!(matches!(
        error,
        EcsError::DuplicateName { name: "game::Hp", first, second }
            if first == std::any::type_name::<Hp>() && second == std::any::type_name::<OtherHp>()
    ));

    world.register_resource(NamedScore(1)).unwrap();
    let error = world.register_resource(OtherNamedScore(2)).unwrap_err();
    assert!(matches!(
        error,
        EcsError::DuplicateName {
            name: "game::Score",
            ..
        }
    ));
}

#[test]
fn world_is_send() {
    fn assert_send<T: Send>() {}
    assert_send::<World>();
}
