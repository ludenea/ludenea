use ludenea::prelude::*;
use ludenea::schema::{FieldSchema, TypeSchema, typescript_declarations};

#[test]
fn typescript_declarations_for_boxes_components() {
    let simulation = boxes::app().build().expect("the boxes app builds");
    let components = simulation.world().component_schemas();
    let types: Vec<_> = components
        .iter()
        .map(|(name, schema)| (*name, schema))
        .collect();

    let text = typescript_declarations(&types);

    assert!(text.contains("export type Shape ="), "{text}");
    assert!(text.contains("{ Circle: Circle }"), "{text}");
    assert!(text.contains("\"ludenea::Pose\": Pose"), "{text}");
    assert!(text.contains("export interface ComponentTypes"), "{text}");
}

fn thing(module: &str) -> TypeSchema {
    TypeSchema::Struct {
        name: format!("{module}::Thing"),
        fields: vec![FieldSchema {
            name: "id".into(),
            schema: TypeSchema::U8,
        }],
    }
}

#[test]
fn typescript_names_collide_to_full_paths() {
    let (left, right) = (thing("left"), thing("right"));

    let text = typescript_declarations(&[("left::Thing", &left), ("right::Thing", &right)]);

    assert!(
        text.contains("export type left_Thing = { id: number };"),
        "{text}"
    );
    assert!(
        text.contains("export type right_Thing = { id: number };"),
        "{text}"
    );
    assert!(text.contains("\"right::Thing\": right_Thing;"), "{text}");
}

#[test]
fn input_and_wide_integers_map_to_typescript() {
    let simulation = boxes::app().build().expect("the boxes app builds");
    let input = simulation.input_schema().expect("boxes registers an input");
    let wide = <(u64, Entity, Option<Vec<i64>>)>::schema();

    let text = typescript_declarations(&[("input", &input), ("wide", &wide)]);

    assert!(text.contains("velocity_change: Vec2 | null"), "{text}");
    assert!(
        text.contains("[number | string, { index: number; generation: number }, Array<number | string> | null]"),
        "{text}"
    );
}
