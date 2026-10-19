use ludenea_app::{App, SceneError, Simulation};
use ludenea_ecs::Entity;
use ludenea_math::{Aabb, Rot, Vec2};
use ludenea_physics::{
    Body, BodyKind, Circle, Collider, PhysicsPlugin, PhysicsSettings, Polygon, Pose, Proxy, Shape,
    find_pairs,
};

fn simulation() -> Simulation {
    let mut app = App::new();
    app.plugin(PhysicsPlugin);
    app.build().unwrap()
}

fn spawn(
    simulation: &mut Simulation,
    position: Vec2,
    body: Body,
    collider: Option<Collider>,
) -> Entity {
    let world = simulation.world_mut();
    let entity = world.spawn();
    world
        .insert(
            entity,
            Pose {
                position,
                rotation: Rot::IDENTITY,
            },
        )
        .unwrap();
    world.insert(entity, body).unwrap();
    if let Some(collider) = collider {
        world.insert(entity, collider).unwrap();
    }
    entity
}

fn boxed(half_width: f32, half_height: f32) -> Collider {
    Collider::new(Shape::Polygon(Polygon::make_box(half_width, half_height)))
}

/// Top face of the ground box.
const GROUND_TOP: f32 = 0.5;

fn ground(simulation: &mut Simulation) -> Entity {
    spawn(
        simulation,
        Vec2::ZERO,
        Body::fixed(),
        Some(boxed(10.0, 0.5)),
    )
}

fn pose(simulation: &Simulation, entity: Entity) -> Pose {
    *simulation.world().get::<Pose>(entity).unwrap()
}

fn body(simulation: &Simulation, entity: Entity) -> Body {
    *simulation.world().get::<Body>(entity).unwrap()
}

fn step(simulation: &mut Simulation, ticks: usize) {
    for _ in 0..ticks {
        simulation.step();
    }
}

fn proxy(x0: f32, x1: f32, body: u32, kind: BodyKind) -> Proxy {
    Proxy {
        aabb: Aabb {
            min: Vec2::new(x0, 0.0),
            max: Vec2::new(x1, 1.0),
        },
        body,
        kind,
    }
}

#[test]
fn find_pairs_skips_static_static_and_is_sorted() {
    let proxies = [
        proxy(3.0, 5.0, 4, BodyKind::Dynamic),
        proxy(0.0, 2.0, 1, BodyKind::Static),
        proxy(1.0, 4.0, 2, BodyKind::Static),
        proxy(1.5, 3.5, 0, BodyKind::Dynamic),
        proxy(10.0, 11.0, 3, BodyKind::Dynamic),
    ];
    assert_eq!(find_pairs(&proxies), [(0, 1), (0, 2), (0, 4), (2, 4)]);
}

#[test]
fn falling_box_lands_and_rests_on_ground() {
    let mut simulation = simulation();
    ground(&mut simulation);
    let falling = spawn(
        &mut simulation,
        Vec2::new(0.0, 5.0),
        Body::dynamic(),
        Some(boxed(0.5, 0.5)),
    );
    step(&mut simulation, 300);
    let velocity = body(&simulation, falling).linear_velocity;
    let height = pose(&simulation, falling).position.y;
    assert!(velocity.length() < 0.01, "velocity {velocity:?}");
    assert!((height - 1.0).abs() < 0.01, "height {height}");
}

#[test]
fn pyramid_of_ten_rows_is_stable() {
    let mut simulation = simulation();
    ground(&mut simulation);
    let mut boxes = Vec::new();
    for row in 0..10 {
        let count = 10 - row;
        for column in 0..count {
            let x = column as f32 - 0.5 * (count - 1) as f32;
            let mut collider = boxed(0.5, 0.5);
            collider.friction = 0.6;
            boxes.push(spawn(
                &mut simulation,
                Vec2::new(x, GROUND_TOP + 0.5 + row as f32),
                Body::dynamic(),
                Some(collider),
            ));
        }
    }
    assert_eq!(boxes.len(), 55);
    step(&mut simulation, 300);
    let settled: Vec<Vec2> = boxes
        .iter()
        .map(|&b| pose(&simulation, b).position)
        .collect();
    step(&mut simulation, 300);
    let worst = boxes
        .iter()
        .zip(&settled)
        .map(|(&b, &before)| pose(&simulation, b).position.distance(before))
        .fold(0.0f32, |worst, d| if d > worst { d } else { worst });
    assert!(worst < 0.05, "maximum displacement {worst}");
}

#[test]
fn bouncing_ball_with_restitution_rebounds() {
    let mut simulation = simulation();
    ground(&mut simulation);
    let radius = 0.25;
    let drop = 4.0;
    let mut collider = Collider::new(Shape::Circle(Circle {
        center: Vec2::ZERO,
        radius,
    }));
    collider.restitution = 0.8;
    let ball = spawn(
        &mut simulation,
        Vec2::new(0.0, GROUND_TOP + radius + drop),
        Body::dynamic(),
        Some(collider),
    );
    let mut reversed = false;
    let mut apex = f32::NEG_INFINITY;
    for _ in 0..240 {
        simulation.step();
        let velocity = body(&simulation, ball).linear_velocity.y;
        if velocity > 0.0 {
            reversed = true;
        }
        if reversed {
            let height = pose(&simulation, ball).position.y - GROUND_TOP - radius;
            if height > apex {
                apex = height;
            }
        }
    }
    assert!(reversed, "the ball never bounced");
    let ratio = apex / drop;
    assert!((0.5..0.8).contains(&ratio), "apex ratio {ratio}");
}

#[test]
fn kinematic_body_moves_at_constant_velocity_and_ignores_gravity() {
    let mut simulation = simulation();
    let mut kinematic = Body::kinematic();
    kinematic.linear_velocity = Vec2::new(2.0, 0.0);
    let mover = spawn(
        &mut simulation,
        Vec2::ZERO,
        kinematic,
        Some(boxed(0.5, 0.5)),
    );
    step(&mut simulation, 60);
    let position = pose(&simulation, mover).position;
    assert!((position.x - 2.0).abs() < 1e-3, "position {position:?}");
    assert_eq!(position.y, 0.0);
    assert_eq!(
        body(&simulation, mover).linear_velocity,
        Vec2::new(2.0, 0.0)
    );
}

#[test]
fn static_body_velocity_is_ignored() {
    let mut simulation = simulation();
    let mut fixed = Body::fixed();
    fixed.linear_velocity = Vec2::new(5.0, 0.0);
    let platform = spawn(
        &mut simulation,
        Vec2::new(0.0, -0.5),
        fixed,
        Some(boxed(20.0, 0.5)),
    );
    let rider = spawn(
        &mut simulation,
        Vec2::new(0.0, 0.5),
        Body::dynamic(),
        Some(boxed(0.5, 0.5)),
    );
    step(&mut simulation, 120);
    assert_eq!(pose(&simulation, platform).position, Vec2::new(0.0, -0.5));
    assert_eq!(
        body(&simulation, platform).linear_velocity,
        Vec2::new(5.0, 0.0)
    );
    let x = pose(&simulation, rider).position.x;
    assert!(x.abs() < 1e-3, "rider drifted to x = {x}");
}

#[test]
fn zero_density_dynamic_body_does_not_produce_nan() {
    let mut simulation = simulation();
    ground(&mut simulation);
    let mut collider = boxed(0.5, 0.5);
    collider.density = 0.0;
    let weightless = spawn(
        &mut simulation,
        Vec2::new(0.0, 2.0),
        Body::dynamic(),
        Some(collider),
    );
    let bare = spawn(&mut simulation, Vec2::new(5.0, 2.0), Body::dynamic(), None);
    step(&mut simulation, 120);
    let resting = pose(&simulation, weightless);
    assert!(resting.position.x.is_finite() && resting.position.y.is_finite());
    assert!(
        (resting.position.y - 1.0).abs() < 0.02,
        "height {}",
        resting.position.y
    );
    assert!(pose(&simulation, bare).position.y.is_finite());
}

#[test]
fn offset_center_of_mass_rotates_about_its_center() {
    let mut simulation = simulation();
    simulation
        .world_mut()
        .resource_mut::<PhysicsSettings>()
        .unwrap()
        .gravity = Vec2::ZERO;
    let mut spinning = Body::dynamic();
    spinning.angular_velocity = 3.0;
    let spinner = spawn(
        &mut simulation,
        Vec2::ZERO,
        spinning,
        Some(Collider::new(Shape::Circle(Circle {
            center: Vec2::new(1.0, 0.0),
            radius: 0.5,
        }))),
    );
    for _ in 0..120 {
        simulation.step();
        let pose = pose(&simulation, spinner);
        let center = pose.position + pose.rotation.rotate(Vec2::new(1.0, 0.0));
        assert!(
            center.distance(Vec2::new(1.0, 0.0)) < 1e-4,
            "center of mass moved to {center:?}"
        );
    }
    let rotation = pose(&simulation, spinner).rotation;
    assert!(rotation != Rot::IDENTITY, "the body did not rotate");
}

#[test]
fn two_runs_give_identical_hash_sequences() {
    let run = || {
        let mut simulation = simulation();
        ground(&mut simulation);
        for i in 0..6 {
            let shape = if i % 2 == 0 {
                Shape::Polygon(Polygon::make_box(0.4, 0.3))
            } else {
                Shape::Circle(Circle {
                    center: Vec2::ZERO,
                    radius: 0.3,
                })
            };
            let mut body = Body::dynamic();
            body.angular_velocity = 0.5 * i as f32;
            spawn(
                &mut simulation,
                Vec2::new(0.3 * i as f32, 1.0 + 0.8 * i as f32),
                body,
                Some(Collider::new(shape)),
            );
        }
        (0..180)
            .map(|_| {
                simulation.step();
                simulation.hash()
            })
            .collect::<Vec<u64>>()
    };
    assert_eq!(run(), run());
}

/// One dynamic circle; each placeholder is replaced to inject one bad value.
const ONE_BODY_SCENE: &str = r#"Scene(version: 1, resources: {}, entities: [{
    "ludenea::Pose": (position: (x: 0.0, y: 1.0), rotation: (cos: COS, sin: 0.0)),
    "ludenea::Body": (kind: Dynamic, linear_velocity: (x: VX, y: 0.0), angular_velocity: 0.0,
        linear_damping: 0.0, angular_damping: 0.0, gravity_scale: 1.0),
    "ludenea::Collider": (shape: Circle((center: (x: 0.0, y: 0.0), radius: 0.5)),
        density: 1.0, friction: FRICTION, restitution: 0.0),
}])"#;

fn one_body_scene(cos: &str, vx: &str, friction: &str) -> String {
    ONE_BODY_SCENE
        .replace("COS", cos)
        .replace("VX", vx)
        .replace("FRICTION", friction)
}

/// Loading `scene` fails on `component` of its entity and changes nothing; returns why.
fn assert_rejected(scene: &str, component: &str) -> String {
    let mut simulation = simulation();
    let before = simulation.hash();
    let error = simulation.load_scene(scene).unwrap_err();
    assert_eq!(simulation.hash(), before);
    match error {
        SceneError::InvalidValue {
            entity: Some(entity),
            name,
            reason,
        } if entity == [0] && name == component => reason,
        error => panic!("{error:?}"),
    }
}

#[test]
fn valid_one_body_scene_loads() {
    simulation()
        .load_scene(&one_body_scene("1.0", "0.0", "0.6"))
        .unwrap();
}

#[test]
fn scene_with_negative_friction_is_rejected() {
    assert_rejected(&one_body_scene("1.0", "0.0", "-0.1"), "ludenea::Collider");
}

#[test]
fn scene_with_nan_velocity_is_rejected() {
    assert_rejected(&one_body_scene("1.0", "NaN", "0.6"), "ludenea::Body");
}

#[test]
fn scene_with_non_unit_rotation_is_rejected() {
    assert_rejected(&one_body_scene("2.0", "0.0", "0.6"), "ludenea::Pose");
}

#[test]
fn kinematic_spin_keeps_a_unit_rotation_and_restores() {
    let mut simulation = simulation();
    let mut spinning = Body::kinematic();
    spinning.angular_velocity = 1e30;
    let spinner = spawn(&mut simulation, Vec2::ZERO, spinning, Some(boxed(0.5, 0.5)));
    simulation.step();
    let Rot { cos, sin } = pose(&simulation, spinner).rotation;
    assert!(
        (cos * cos + sin * sin - 1.0).abs() < 1e-4,
        "rotation {cos}, {sin}"
    );
    let mut fresh = self::simulation();
    fresh.restore(&simulation.snapshot()).unwrap();
}

#[test]
fn kinematic_spin_is_clamped_like_a_dynamic_one() {
    let mut simulation = simulation();
    let ball = || {
        Some(Collider::new(Shape::Circle(Circle {
            center: Vec2::ZERO,
            radius: 0.5,
        })))
    };
    let mut spinning = Body::dynamic();
    spinning.angular_velocity = 1e30;
    spinning.gravity_scale = 0.0;
    let dynamic = spawn(&mut simulation, Vec2::ZERO, spinning, ball());
    spinning.kind = BodyKind::Kinematic;
    let kinematic = spawn(&mut simulation, Vec2::new(5.0, 0.0), spinning, ball());
    simulation.step();
    let clamped = body(&simulation, dynamic).angular_velocity;
    assert!(clamped < 1e3, "dynamic angular velocity {clamped}");
    assert_eq!(body(&simulation, kinematic).angular_velocity, clamped);
    let rotation = pose(&simulation, kinematic).rotation;
    assert_eq!(rotation, pose(&simulation, dynamic).rotation);
    assert_ne!(rotation, Rot::IDENTITY);
}

#[test]
fn an_invalid_pose_after_the_step_names_the_entity_and_the_field() {
    let mut simulation = simulation();
    let lost = spawn(
        &mut simulation,
        Vec2::new(f32::NAN, 0.0),
        Body::dynamic(),
        None,
    );
    let panic =
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| simulation.step())).unwrap_err();
    let message = panic.downcast_ref::<String>().unwrap();
    assert!(
        message.contains(&format!("entity {lost}")) && message.contains("position must be finite"),
        "{message}"
    );
}

/// `ludenea::PhysicsSettings` with the default values, `field` set to `value`.
fn settings(field: &str, value: &str) -> String {
    let fields = [
        ("gravity", "(x: 0.0, y: -10.0)"),
        ("substeps", "4"),
        ("contact_hertz", "30.0"),
        ("contact_damping_ratio", "10.0"),
        ("contact_push_max_velocity", "3.0"),
        ("restitution_threshold", "1.0"),
    ]
    .map(|(name, default)| format!("{name}: {}", if name == field { value } else { default }));
    format!(r#""ludenea::PhysicsSettings": ({})"#, fields.join(", "))
}

/// The valid one-body scene with `resources` as its resources.
fn scene_with_resources(resources: &str) -> String {
    one_body_scene("1.0", "0.0", "0.6")
        .replace("resources: {}", &format!("resources: {{{resources}}}"))
}

/// Loading `scene` fails on the resource `name`, with a reason that contains `reason`, and
/// changes nothing.
fn assert_resource_rejected(scene: &str, name: &str, reason: &str) {
    let mut simulation = simulation();
    let before = simulation.hash();
    let error = simulation.load_scene(scene).unwrap_err();
    assert!(
        matches!(&error, SceneError::InvalidValue { entity: None, name: refused, reason: why } if refused == name && why.contains(reason)),
        "{error:?}"
    );
    assert_eq!(simulation.hash(), before);
}

#[test]
fn scene_with_nan_gravity_is_rejected() {
    assert_resource_rejected(
        &scene_with_resources(&settings("gravity", "(x: NaN, y: -10.0)")),
        "ludenea::PhysicsSettings",
        "gravity must be finite with each component within ±1000000.0, got (NaN, -10.0)",
    );
}

#[test]
fn scene_with_huge_substeps_is_rejected() {
    assert_resource_rejected(
        &scene_with_resources(&settings("substeps", "4000000000")),
        "ludenea::PhysicsSettings",
        "substeps must be within 1 to 64, got 4000000000",
    );
}

#[test]
fn scene_with_physics_settings_out_of_range_is_rejected() {
    for (field, value) in [
        ("gravity", "(x: 0.0, y: -1000001.0)"),
        ("gravity", "(x: inf, y: 0.0)"),
        ("substeps", "0"),
        ("substeps", "65"),
        ("contact_hertz", "-1.0"),
        ("contact_damping_ratio", "NaN"),
        ("contact_push_max_velocity", "inf"),
        ("restitution_threshold", "-0.5"),
    ] {
        assert_resource_rejected(
            &scene_with_resources(&settings(field, value)),
            "ludenea::PhysicsSettings",
            field,
        );
    }
}

#[test]
fn scene_with_physics_settings_at_their_limits_loads() {
    for (field, value) in [
        ("gravity", "(x: -1000000.0, y: 1000000.0)"),
        ("substeps", "1"),
        ("substeps", "64"),
        ("contact_hertz", "0.0"),
        ("contact_damping_ratio", "0.0"),
        ("contact_push_max_velocity", "0.0"),
        ("restitution_threshold", "0.0"),
    ] {
        simulation()
            .load_scene(&scene_with_resources(&settings(field, value)))
            .unwrap();
    }
}

#[test]
fn scene_with_huge_density_is_rejected() {
    let scene = one_body_scene("1.0", "0.0", "0.6").replace("density: 1.0", "density: 3e38");
    let reason = assert_rejected(&scene, "ludenea::Collider");
    assert!(
        reason.ends_with("density must be at most 1000000.0, got 3e38"),
        "{reason}"
    );
}

#[test]
fn scene_with_huge_gravity_scale_is_rejected() {
    let scene =
        one_body_scene("1.0", "0.0", "0.6").replace("gravity_scale: 1.0", "gravity_scale: -3e38");
    let reason = assert_rejected(&scene, "ludenea::Body");
    assert!(
        reason.ends_with("gravity_scale must be finite and within ±1000.0, got -3e38"),
        "{reason}"
    );
}

#[test]
fn kinematic_body_at_huge_finite_speed_stays_finite() {
    let mut simulation = simulation();
    let mut fast = Body::dynamic();
    fast.linear_velocity = Vec2::new(3e38, -3e38);
    fast.gravity_scale = 0.0;
    let dynamic = spawn(&mut simulation, Vec2::ZERO, fast, Some(boxed(0.5, 0.5)));
    fast.kind = BodyKind::Kinematic;
    let kinematic = spawn(
        &mut simulation,
        Vec2::new(0.0, 5.0),
        fast,
        Some(boxed(0.5, 0.5)),
    );
    step(&mut simulation, 120);
    let velocity = body(&simulation, kinematic).linear_velocity;
    assert_eq!(velocity, body(&simulation, dynamic).linear_velocity);
    assert!(
        (velocity.length() - 400.0).abs() < 1e-3 && (velocity.x + velocity.y).abs() < 1e-3,
        "velocity {velocity:?}"
    );
    let position = pose(&simulation, kinematic).position;
    assert!(
        (position - Vec2::new(0.0, 5.0)).distance(velocity * 2.0) < 0.1,
        "position {position:?}"
    );
}

/// Bodies at the largest gravity, gravity scale, density, time step, and velocities that
/// loading accepts, around a static one.
const LIMITS_SCENE: &str = r#"Scene(version: 1, resources: {"ludenea::PhysicsSettings": (
        gravity: (x: 1000000.0, y: -1000000.0), substeps: SUBSTEPS, contact_hertz: 0.0,
        contact_damping_ratio: 0.0, contact_push_max_velocity: 0.0, restitution_threshold: 0.0),
        "ludenea::Time": (tick: 0, dt: 1.0)}, entities: [
    {
        "ludenea::Pose": (position: (x: 0.0, y: 0.0), rotation: (cos: 1.0, sin: 0.0)),
        "ludenea::Body": (kind: Static, linear_velocity: (x: 0.0, y: 0.0), angular_velocity: 0.0,
            linear_damping: 0.0, angular_damping: 0.0, gravity_scale: 1.0),
        "ludenea::Collider": (shape: Circle((center: (x: 0.0, y: 0.0), radius: 10.0)),
            density: 0.0, friction: 1.0, restitution: 1.0),
    },
    {
        "ludenea::Pose": (position: (x: 0.0, y: 10.5), rotation: (cos: 1.0, sin: 0.0)),
        "ludenea::Body": (kind: Dynamic, linear_velocity: (x: -3.4e38, y: 3.4e38),
            angular_velocity: 3.4e38, linear_damping: 0.0, angular_damping: 0.0,
            gravity_scale: 1000.0),
        "ludenea::Collider": (shape: Circle((center: (x: 0.0, y: 0.0), radius: 0.5)),
            density: 1000000.0, friction: 1.0, restitution: 1.0),
    },
    {
        "ludenea::Pose": (position: (x: 3.0, y: 10.5), rotation: (cos: 1.0, sin: 0.0)),
        "ludenea::Body": (kind: Kinematic, linear_velocity: (x: 3.4e38, y: -3.4e38),
            angular_velocity: -3.4e38, linear_damping: 0.0, angular_damping: 0.0,
            gravity_scale: -1000.0),
        "ludenea::Collider": (shape: Circle((center: (x: 0.0, y: 0.0), radius: 0.25)),
            density: 1000000.0, friction: 1.0, restitution: 1.0),
    },
])"#;

#[test]
fn bodies_at_every_limit_stay_finite() {
    for substeps in ["1", "64"] {
        let mut simulation = simulation();
        simulation
            .load_scene(&LIMITS_SCENE.replace("SUBSTEPS", substeps))
            .unwrap();
        step(&mut simulation, 100);
    }
}

#[test]
fn an_invalid_state_after_the_step_names_the_step_inputs() {
    let mut simulation = simulation();
    let mut moving = Body::dynamic();
    moving.linear_velocity = Vec2::new(1.5, 0.0);
    let lost = spawn(&mut simulation, Vec2::ZERO, moving, None);
    simulation
        .world_mut()
        .resource_mut::<PhysicsSettings>()
        .unwrap()
        .gravity = Vec2::new(f32::NAN, -10.0);
    let panic =
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| simulation.step())).unwrap_err();
    let message = panic.downcast_ref::<String>().unwrap();
    for expected in [
        format!("entity {lost}"),
        "position must be finite".to_string(),
        format!("dt {:?}", 1.0f32 / 60.0),
        "gravity (NaN, -10.0) at scale 1.0".to_string(),
        "linear velocity (1.5, 0.0)".to_string(),
    ] {
        assert!(
            message.contains(&expected),
            "{expected:?} is missing from {message}"
        );
    }
}
