use ludenea_app::pose::{Pose, PoseError};
use ludenea_app::{App, Plugin};
use ludenea_ecs::Component;
use ludenea_math::{Rot, Vec2};

fn pose(position: Vec2, rotation: Rot) -> Pose {
    Pose { position, rotation }
}

#[test]
fn pose_keeps_its_stable_name() {
    assert_eq!(Pose::NAME, "ludenea::Pose");
}

#[test]
fn a_default_pose_is_valid() {
    assert_eq!(Pose::default().validate(), Ok(()));
}

#[test]
fn a_non_finite_position_is_refused() {
    let bad = pose(Vec2::new(f32::NAN, 0.0), Rot::IDENTITY);
    let Err(PoseError::Position(_)) = bad.validate() else {
        panic!("a NaN position must be refused");
    };
    let message = bad.validate().unwrap_err().to_string();
    assert!(message.contains("position must be finite"), "{message}");
}

#[test]
fn a_rotation_that_is_not_unit_length_is_refused() {
    let bad = pose(Vec2::ZERO, Rot { cos: 1.0, sin: 0.5 });
    let message = bad.validate().unwrap_err().to_string();
    assert!(message.contains("cos² + sin² = 1"), "{message}");
}

#[test]
fn deserialization_applies_the_same_validation() {
    let good = r#"(position: (x: 1.0, y: 2.0), rotation: (cos: 1.0, sin: 0.0))"#;
    let pose: Pose = ron::from_str(good).unwrap();
    assert_eq!(pose.position, Vec2::new(1.0, 2.0));
    let bad = r#"(position: (x: 1.0, y: 2.0), rotation: (cos: 2.0, sin: 0.0))"#;
    let error = ron::from_str::<Pose>(bad).unwrap_err().to_string();
    assert!(error.contains("rotation must be finite"), "{error}");
}

struct First;
struct Second;

impl Plugin for First {
    fn build(&self, app: &mut App) {
        app.component::<Pose>();
    }
}

impl Plugin for Second {
    fn build(&self, app: &mut App) {
        app.component::<Pose>();
    }
}

#[test]
fn pose_registered_by_two_plugins_is_registered_once() {
    let mut app = App::new();
    app.plugin(First).plugin(Second);
    let simulation = app.build().unwrap();
    let poses = simulation
        .world()
        .component_schemas()
        .into_iter()
        .filter(|(name, _)| *name == "ludenea::Pose")
        .count();
    assert_eq!(poses, 1);
}
