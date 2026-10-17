use ludenea_ecs::{Component, Entity, EntityFilter, FilterError, World};
use serde::{Deserialize, Serialize};

#[derive(Component, Serialize, Deserialize)]
struct Position(i32);

#[derive(Component, Serialize, Deserialize)]
struct Frozen;

mod a {
    use ludenea_ecs::Component;
    use serde::{Deserialize, Serialize};

    #[derive(Component, Serialize, Deserialize)]
    pub struct Tag;
}

mod b {
    use ludenea_ecs::Component;
    use serde::{Deserialize, Serialize};

    #[derive(Component, Serialize, Deserialize)]
    pub struct Tag;
}

/// Three entities: the first has `Position`, the second `Position` and `Frozen`, the third
/// only `a::Tag`.
fn world() -> (World, [Entity; 3]) {
    let mut world = World::new();
    world.register_component::<Position>().unwrap();
    world.register_component::<Frozen>().unwrap();
    world.register_component::<a::Tag>().unwrap();
    world.register_component::<b::Tag>().unwrap();
    let [first, second, third] = [(); 3].map(|()| world.spawn());
    world.insert(first, Position(1)).unwrap();
    world.insert(second, Position(2)).unwrap();
    world.insert(second, Frozen).unwrap();
    world.insert(third, a::Tag).unwrap();
    (world, [first, second, third])
}

#[test]
fn filter_with_and_without_by_short_and_full_name() {
    let (world, [first, second, _]) = world();

    let filter = EntityFilter::parse("With<Position>", &world).unwrap();
    assert_eq!(world.filter_entities(&filter), [first, second]);

    let filter = EntityFilter::parse("With<Position>, Without<Frozen>", &world).unwrap();
    assert_eq!(world.filter_entities(&filter), [first]);

    let full = format!("With<{}>,Without<{}>", Position::NAME, Frozen::NAME);
    let filter = EntityFilter::parse(&full, &world).unwrap();
    assert_eq!(world.filter_entities(&filter), [first]);
}

#[test]
fn ambiguous_short_name_lists_candidates() {
    let (world, _) = world();
    let Err(FilterError::Ambiguous { name, candidates }) = EntityFilter::parse("With<Tag>", &world)
    else {
        panic!("expected an ambiguity error");
    };
    assert_eq!(name, "Tag");
    assert_eq!(candidates, [a::Tag::NAME, b::Tag::NAME]);

    let filter = EntityFilter::parse(&format!("With<{}>", b::Tag::NAME), &world).unwrap();
    assert!(world.filter_entities(&filter).is_empty());
}

#[test]
fn unknown_name_and_bad_syntax_are_errors() {
    let (world, _) = world();
    let Err(FilterError::Unknown { name, candidates }) = EntityFilter::parse("With<Nope>", &world)
    else {
        panic!("expected an unknown component error");
    };
    assert_eq!(name, "Nope");
    assert!(candidates.contains(&Position::NAME));
    assert!(matches!(
        EntityFilter::parse("Position", &world),
        Err(FilterError::Syntax(_))
    ));
}

#[test]
fn empty_filter_matches_all() {
    let (world, [first, second, third]) = world();
    for text in ["", "  "] {
        let filter = EntityFilter::parse(text, &world).unwrap();
        assert_eq!(world.filter_entities(&filter), [first, second, third]);
    }
}
