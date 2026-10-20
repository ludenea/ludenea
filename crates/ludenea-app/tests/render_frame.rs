use std::collections::BTreeSet;

use ludenea_app::pose::Pose;
use ludenea_app::{App, AppError, SimError, Simulation};
use ludenea_draw::frame::{LayerInfo, RenderFrame};
use ludenea_draw::{Color, DebugDrawFrame, Primitive};
use ludenea_ecs::World;
use ludenea_math::Vec2;

fn layers(names: &[&str]) -> BTreeSet<String> {
    names.iter().map(|name| name.to_string()).collect()
}

fn note(frame: &mut RenderFrame, text: &str) {
    frame.warnings.push(text.to_string());
}

fn first(_: &World, frame: &mut RenderFrame) {
    note(frame, "first");
}

fn second(_: &World, frame: &mut RenderFrame) {
    note(frame, "second");
}

#[test]
fn layers_come_in_declaration_order() {
    let mut app = App::new();
    app.render_layer("world", 1.0)
        .render_layer("background", 0.5)
        .render_layer("hud", 0.0);
    let frame = app.build().unwrap().render_frame(&layers(&[])).unwrap();
    let layer = |name: &str, parallax| LayerInfo {
        name: name.to_string(),
        parallax,
    };
    assert_eq!(
        frame.layers,
        [
            layer("world", 1.0),
            layer("background", 0.5),
            layer("hud", 0.0)
        ]
    );
    assert_eq!(frame.layer_index("hud"), Some(2));
}

#[test]
fn duplicate_layer_is_refused_at_build() {
    let mut app = App::new();
    app.render_layer("world", 1.0)
        .render_layer("hud", 0.0)
        .render_layer("world", 0.5);
    let Err(error) = app.build() else {
        panic!("an app declaring a layer twice built");
    };
    assert!(
        matches!(&error, AppError::DuplicateLayer(name) if name == "world"),
        "{error:?}"
    );
    assert_eq!(
        error.to_string(),
        "the render layer `world` is declared twice; give each layer its own name"
    );
}

#[test]
fn non_finite_parallax_is_refused_at_build() {
    for parallax in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
        let mut app = App::new();
        app.render_layer("world", 1.0).render_layer("bg", parallax);
        let Err(error) = app.build() else {
            panic!("an app declaring a parallax of {parallax} built");
        };
        assert!(
            matches!(&error, AppError::NonFiniteParallax { layer, parallax: value }
                if *layer == "bg" && value.to_bits() == parallax.to_bits()),
            "{error:?}"
        );
        assert_eq!(
            error.to_string(),
            format!("the render layer `bg` has the parallax {parallax}; give it a finite number")
        );
    }
}

#[test]
fn more_layers_than_a_layer_index_holds_are_refused_at_build() {
    let limit = usize::from(u16::MAX) + 1;
    let mut app = App::new();
    for index in 0..=limit {
        app.render_layer(Box::leak(format!("layer {index}").into_boxed_str()), 1.0);
    }
    let Err(error) = app.build() else {
        panic!("an app declaring {} layers built", limit + 1);
    };
    assert!(
        matches!(error, AppError::TooManyLayers { count, limit: max } if count == limit + 1 && max == limit),
        "{error:?}"
    );
    assert_eq!(
        error.to_string(),
        "the app declares 65537 render layers, past the limit of 65536; merge some of them"
    );
}

#[test]
fn as_many_layers_as_a_layer_index_holds_build() {
    let mut app = App::new();
    for index in 0..=u16::MAX {
        app.render_layer(Box::leak(format!("layer {index}").into_boxed_str()), 1.0);
    }
    let frame = app.build().unwrap().render_frame(&layers(&[])).unwrap();
    assert_eq!(frame.layer_index("layer 65535"), Some(u16::MAX));
}

#[test]
fn extractors_run_in_registration_order() {
    let mut app = App::new();
    app.extractor(second).extractor(first).extractor(second);
    let mut simulation = app.build().unwrap();
    simulation.step().unwrap();
    let frame = simulation.render_frame(&layers(&[])).unwrap();
    assert_eq!(frame.tick, 1);
    assert_eq!(frame.warnings, ["second", "first", "second"]);
}

fn draw_line(_: &World, layers: &BTreeSet<String>, frame: &mut DebugDrawFrame) {
    if layers.contains("lines") {
        frame.push(
            "lines",
            None,
            Primitive::Line {
                a: Vec2::ZERO,
                b: Vec2::new(1.0, 0.0),
                color: Color(0xFFFFFFFF),
            },
        );
    }
}

#[test]
fn render_frame_carries_the_debug_draw() {
    let mut app = App::new();
    app.debug_draw(&["lines"], draw_line);
    let simulation = app.build().unwrap();
    let requested = layers(&["lines"]);
    let frame = simulation.render_frame(&requested).unwrap();
    assert_eq!(frame.debug, simulation.debug_draw(&requested).unwrap());
    assert_eq!(frame.debug.layers["lines"].len(), 1);
    assert!(
        simulation
            .render_frame(&layers(&[]))
            .unwrap()
            .debug
            .layers
            .is_empty()
    );
}

fn extract_panics(_: &World, _: &mut RenderFrame) {
    panic!("no extracting here");
}

fn draw_panics(_: &World, _: &BTreeSet<String>, _: &mut DebugDrawFrame) {
    panic!("no extracting here");
}

fn panic_of(result: Result<impl std::fmt::Debug, SimError>) -> ludenea_app::GamePanic {
    match result {
        Err(SimError::Panicked(panic)) => *panic,
        other => panic!("expected a panic, got {other:?}"),
    }
}

#[test]
fn extractor_panic_is_reported_like_a_drawer_panic() {
    let mut extracting = App::new();
    extracting.extractor(extract_panics);
    let mut extracting = extracting.build().unwrap();
    let mut drawing = App::new();
    drawing.debug_draw(&["any"], draw_panics);
    let mut drawing = drawing.build().unwrap();
    extracting.step().unwrap();
    drawing.step().unwrap();

    let extracted = panic_of(extracting.render_frame(&layers(&[])));
    let drawn = panic_of(drawing.debug_draw(&layers(&[])));
    assert_eq!(
        (extracted.tick, extracted.system, extracted.message.as_str()),
        (1, None, "no extracting here")
    );
    assert_eq!(
        (extracted.tick, extracted.system, &extracted.message),
        (drawn.tick, drawn.system, &drawn.message)
    );
    // The simulation is still usable after a caught panic.
    extracting.step().unwrap();
    assert_eq!(extracting.tick(), 2);
}

fn extract_poses(world: &World, frame: &mut RenderFrame) {
    for entity in world.entities() {
        if let Some(pose) = world.get::<Pose>(entity) {
            note(frame, &format!("{entity:?} at {:?}", pose.position));
        }
    }
}

fn posed() -> Simulation {
    let mut app = App::new();
    app.component::<Pose>()
        .render_layer("world", 1.0)
        .extractor(extract_poses);
    let mut simulation = app.build().unwrap();
    let world = simulation.world_mut();
    let entity = world.spawn();
    world
        .insert(
            entity,
            Pose {
                position: Vec2::new(2.0, 3.0),
                ..Pose::default()
            },
        )
        .unwrap();
    simulation
}

#[test]
fn render_frame_does_not_change_the_hash() {
    let mut simulation = posed();
    simulation.step().unwrap();
    let before = simulation.hash();
    let frame = simulation.render_frame(&layers(&[])).unwrap();
    assert_eq!(frame.warnings.len(), 1);
    assert_eq!(simulation.hash(), before);

    // Rendering between ticks never changes where the simulation goes.
    let mut rendered = posed();
    let mut plain = posed();
    for _ in 0..3 {
        rendered.render_frame(&layers(&[])).unwrap();
        rendered.step().unwrap();
        plain.step().unwrap();
    }
    assert_eq!(rendered.hash(), plain.hash());
}
