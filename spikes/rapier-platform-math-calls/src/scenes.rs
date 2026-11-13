//! The worlds: one per call the reading of the sources found on the path of
//! a step, each built so that the call is reached with many inputs, and a
//! control that holds none of the features.
//!
//! Every parameter is drawn from a seeded stream with exact arithmetic, so
//! the worlds are the same on every cell before their first step.

use rapier3d::prelude::*;
use reference_workload::{Fnv1a, SplitMix64};

pub const TICKS: usize = 600;
/// The ticks after which a world is hashed.
pub const CHECKPOINTS: [usize; 8] = [1, 2, 5, 10, 30, 100, 300, 600];
const SEED: u64 = 0x4c75_6465_6e65_6133;
const PENDULUMS: usize = 32;
const COMPOUND_BODIES: usize = 48;
const TURNED_BODIES: usize = 16;
/// The tick before which a third of the compound bodies gain a collider, and
/// the tick before which another third change a collider's density.
const COLLIDER_ADDED_AT: usize = 150;
const DENSITY_CHANGED_AT: usize = 300;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind {
    Control,
    JointLimits,
    MotorTargets,
    SeveralColliders,
    BallsOnHulls,
}

pub const KINDS: [Kind; 5] = [Kind::Control, Kind::JointLimits, Kind::MotorTargets, Kind::SeveralColliders, Kind::BallsOnHulls];

impl Kind {
    pub fn name(self) -> &'static str {
        match self {
            Kind::Control => "control",
            Kind::JointLimits => "joint_limits",
            Kind::MotorTargets => "motor_targets",
            Kind::SeveralColliders => "several_colliders",
            Kind::BallsOnHulls => "balls_on_hulls",
        }
    }

    pub fn what(self) -> &'static str {
        match self {
            Kind::Control => "cuboids and balls falling on a floor: no joint, no convex hull, one collider per body placed at the body's own frame",
            Kind::JointLimits => "32 pendulums of two links on revolute impulse joints, each joint with its own asymmetric angular limits",
            Kind::MotorTargets => "32 arms of three links, the first two on revolute multibody joints and the third on a revolute impulse joint, each joint with a motor that has its own position target",
            Kind::SeveralColliders => "48 bodies of three colliders placed apart and at angles and 16 bodies of one collider placed at an angle, on a floor; a third gain a collider and a third change a density while the world steps",
            Kind::BallsOnHulls => "40 balls and 20 convex hulls falling on 8 fixed convex hulls above a floor",
        }
    }

    /// What the call's path is, as the sources were read.
    pub fn call(self) -> &'static str {
        match self {
            Kind::Control => "the base-2 logarithms of the broad phase's tree, which every world reaches",
            Kind::JointLimits => "the sine and cosine of half the center of an angular limit, each time a joint's constraints are built",
            Kind::MotorTargets => "the sine of half a motor's target angle, in the constraint of a joint one of whose bodies is a link of a multibody",
            Kind::SeveralColliders => "the arc cosine and the cosines of the eigenvalues of a symmetric matrix, when mass properties are summed and the inertia matrix is not diagonal",
            Kind::BallsOnHulls => "the sine and cosine of one degree, in the feature a point projects to on a convex polyhedron",
        }
    }

    pub fn from_name(name: &str) -> Option<Kind> {
        KINDS.into_iter().find(|kind| kind.name() == name)
    }

    /// Whether the world has a twin without its feature, whose hashes say the
    /// feature acted.
    pub fn has_a_twin(self) -> bool {
        matches!(self, Kind::JointLimits | Kind::MotorTargets)
    }
}

/// A value of [low, high) on a grid of 2^-24 of the range: exact arithmetic,
/// the same on every cell.
fn between(random: &mut SplitMix64, low: f32, high: f32) -> f32 {
    low + (high - low) * random.u01()
}

fn vector_between(random: &mut SplitMix64, low: f32, high: f32) -> Vector {
    Vector::new(between(random, low, high), between(random, low, high), between(random, low, high))
}

/// The limits of the 64 joints: a lower bound in [-1.2, -0.2) and an upper
/// bound in [0.1, 1.0), so that no center is zero.
pub fn joint_limits() -> Vec<[f32; 2]> {
    let mut random = SplitMix64::new(SEED ^ 0x6c69_6d69_7473);
    (0..2 * PENDULUMS).map(|_| [between(&mut random, -1.2, -0.2), between(&mut random, 0.1, 1.0)]).collect()
}

/// The value the library takes the sine and cosine of for a limit: half the
/// center, computed as it computes it.
pub fn limit_half_centers() -> Vec<f32> {
    joint_limits().iter().map(|[min, max]| ((min + max) * 0.5) * 0.5).collect()
}

/// The position targets of the 96 motors, in [-1.4, 1.4).
pub fn motor_targets() -> Vec<f32> {
    let mut random = SplitMix64::new(SEED ^ 0x6d6f_746f_7273);
    (0..3 * PENDULUMS).map(|_| between(&mut random, -1.4, 1.4)).collect()
}

/// The value the library takes the sine of for a motor: half its target.
pub fn motor_half_targets() -> Vec<f32> {
    motor_targets().iter().map(|target| target / 2.0).collect()
}

/// One degree, as the collision library writes it.
pub fn one_degree() -> f32 {
    (std::f64::consts::PI / 180.0) as f32
}

pub struct Scene {
    pub kind: Kind,
    pub world: PhysicsWorld,
    /// The bodies hashed, in the order they were created.
    pub bodies: Vec<RigidBodyHandle>,
    /// The first collider of each compound body, for the changes made while
    /// the world steps.
    first_colliders: Vec<ColliderHandle>,
    pub joints: usize,
}

fn floor(world: &mut PhysicsWorld, height: f32) {
    world.insert(RigidBodyBuilder::fixed().translation(Vector::new(0.0, height - 0.5, 0.0)), ColliderBuilder::cuboid(40.0, 0.5, 40.0));
}

fn link(world: &mut PhysicsWorld, center: Vector, spin: f32) -> RigidBodyHandle {
    world.insert(RigidBodyBuilder::dynamic().translation(center).angvel(Vector::new(0.0, 0.0, spin)).can_sleep(false), ColliderBuilder::cuboid(0.5, 0.1, 0.1)).0
}

/// Links hung in a chain from a fixed anchor by revolute joints about Z and
/// released horizontal, every second pendulum thrown upward so that both
/// bounds of a limit are met. `joint` receives the two bodies, the place of
/// the joint in its chain and its index among all.
fn pendulums(scene: &mut Scene, links: usize, mut joint: impl FnMut(&mut PhysicsWorld, RigidBodyHandle, RigidBodyHandle, usize, usize)) {
    for index in 0..PENDULUMS {
        let origin = Vector::new(8.0 * index as f32, 0.0, 0.0);
        let mut previous = scene.world.insert_body(RigidBodyBuilder::fixed().translation(origin));
        let spin = if index % 2 == 0 { 0.0 } else { 6.0 };
        for place in 0..links {
            let current = link(&mut scene.world, origin + Vector::new(0.5 + place as f32, 0.0, 0.0), spin);
            joint(&mut scene.world, previous, current, place, links * index + place);
            scene.bodies.push(current);
            scene.joints += 1;
            previous = current;
        }
    }
}

/// A revolute joint about Z between the end of one link, or a fixed anchor's
/// origin, and the start of the next.
fn revolute(from_an_anchor: bool) -> RevoluteJointBuilder {
    let end = if from_an_anchor { Vector::ZERO } else { Vector::new(0.5, 0.0, 0.0) };
    RevoluteJointBuilder::new(Vector::Z).local_anchor1(end).local_anchor2(Vector::new(-0.5, 0.0, 0.0)).contacts_enabled(false)
}

fn build_control(scene: &mut Scene) {
    let mut random = SplitMix64::new(SEED ^ 0x636f_6e74_726f_6c);
    floor(&mut scene.world, 0.0);
    for index in 0..64 {
        let center = Vector::new((index % 8) as f32 * 1.5 - 5.25, 1.0 + (index / 8) as f32 * 1.2, between(&mut random, -0.3, 0.3));
        let body = RigidBodyBuilder::dynamic().translation(center).angvel(vector_between(&mut random, -2.0, 2.0)).can_sleep(false);
        let collider = if index % 2 == 0 { ColliderBuilder::cuboid(0.4, 0.4, 0.4) } else { ColliderBuilder::ball(0.4) };
        scene.bodies.push(scene.world.insert(body, collider).0);
    }
}

fn build_joint_limits(scene: &mut Scene, feature: bool) {
    let limits = joint_limits();
    pendulums(scene, 2, |world, first, second, place, index| {
        let mut joint = revolute(place == 0);
        if feature {
            joint = joint.limits(limits[index]);
        }
        world.insert_impulse_joint(first, second, joint);
    });
}

/// Arms of three links: the first two on multibody joints, the third hung
/// from the second by an impulse joint, which is the joint between a
/// multibody's link and another body. Every joint has a motor with a
/// position target of its own.
fn build_motor_targets(scene: &mut Scene, feature: bool) {
    let targets = motor_targets();
    pendulums(scene, 3, |world, first, second, place, index| {
        let mut joint = revolute(place == 0);
        if feature {
            joint = joint.motor_position(targets[index], 80.0, 8.0);
        }
        if place < 2 {
            world.insert_multibody_joint(first, second, joint).expect("a chain of two links closes no loop");
        } else {
            world.insert_impulse_joint(first, second, joint);
        }
    });
}

fn build_several_colliders(scene: &mut Scene) {
    let mut random = SplitMix64::new(SEED ^ 0x636f_6c6c_6964_6572);
    floor(&mut scene.world, 0.0);
    for index in 0..COMPOUND_BODIES + TURNED_BODIES {
        let center = Vector::new((index % 8) as f32 * 3.0 - 10.5, 2.0 + (index / 8) as f32 * 2.5, (index / 8 % 2) as f32);
        let body = RigidBodyBuilder::dynamic().translation(center).rotation(vector_between(&mut random, -0.5, 0.5)).angvel(vector_between(&mut random, -2.0, 2.0)).can_sleep(false);
        let handle = scene.world.insert_body(body);
        if index < COMPOUND_BODIES {
            let cuboid = ColliderBuilder::cuboid(0.4, 0.3, 0.2).translation(vector_between(&mut random, -0.5, 0.5)).rotation(vector_between(&mut random, -1.0, 1.0)).density(1.0);
            let ball = ColliderBuilder::ball(0.3).translation(vector_between(&mut random, -0.5, 0.5)).density(2.0);
            let capsule = ColliderBuilder::capsule_x(0.3, 0.15).translation(vector_between(&mut random, -0.5, 0.5)).rotation(vector_between(&mut random, -1.0, 1.0)).density(0.5);
            scene.first_colliders.push(scene.world.insert_collider(cuboid, Some(handle)));
            scene.world.insert_collider(ball, Some(handle));
            scene.world.insert_collider(capsule, Some(handle));
        } else {
            scene.world.insert_collider(ColliderBuilder::cuboid(0.5, 0.3, 0.2).rotation(vector_between(&mut random, -1.0, 1.0)), Some(handle));
        }
        scene.bodies.push(handle);
    }
}

fn hull(random: &mut SplitMix64, points: usize, half: Vector) -> ColliderBuilder {
    let cloud: Vec<Vector> = (0..points).map(|_| Vector::new(between(random, -half.x, half.x), between(random, -half.y, half.y), between(random, -half.z, half.z))).collect();
    ColliderBuilder::convex_hull(&cloud).expect("a cloud of points in a box has a hull")
}

fn build_balls_on_hulls(scene: &mut Scene) {
    let mut random = SplitMix64::new(SEED ^ 0x6875_6c6c_73);
    floor(&mut scene.world, -3.0);
    for index in 0..8 {
        let center = Vector::new((index % 4) as f32 * 3.5 - 5.25, 0.0, (index / 4) as f32 * 3.5 - 1.75);
        let rock = hull(&mut random, 24, Vector::new(2.0, 0.6, 2.0));
        scene.world.insert(RigidBodyBuilder::fixed().translation(center), rock);
    }
    for index in 0..40 {
        let center = Vector::new(between(&mut random, -6.0, 6.0), 2.0 + 0.6 * index as f32, between(&mut random, -3.0, 3.0));
        let ball = ColliderBuilder::ball(between(&mut random, 0.25, 0.45));
        scene.bodies.push(scene.world.insert(RigidBodyBuilder::dynamic().translation(center).can_sleep(false), ball).0);
    }
    for index in 0..20 {
        let center = Vector::new(between(&mut random, -6.0, 6.0), 4.0 + 0.7 * index as f32, between(&mut random, -3.0, 3.0));
        let stone = hull(&mut random, 12, Vector::new(0.5, 0.5, 0.5));
        scene.bodies.push(scene.world.insert(RigidBodyBuilder::dynamic().translation(center).can_sleep(false), stone).0);
    }
}

impl Scene {
    /// Builds a world; without its feature, a twin that differs by the
    /// limits or the motors alone.
    pub fn build(kind: Kind, feature: bool) -> Scene {
        let mut scene = Scene { kind, world: PhysicsWorld::new(), bodies: Vec::new(), first_colliders: Vec::new(), joints: 0 };
        match kind {
            Kind::Control => build_control(&mut scene),
            Kind::JointLimits => build_joint_limits(&mut scene, feature),
            Kind::MotorTargets => build_motor_targets(&mut scene, feature),
            Kind::SeveralColliders => build_several_colliders(&mut scene),
            Kind::BallsOnHulls => build_balls_on_hulls(&mut scene),
        }
        scene
    }

    /// What a game does to a world between two steps: here, to the bodies of
    /// several colliders alone, so that mass properties are summed again by
    /// a step.
    fn change_before(&mut self, tick: usize) {
        if self.kind != Kind::SeveralColliders {
            return;
        }
        if tick == COLLIDER_ADDED_AT {
            let mut random = SplitMix64::new(SEED ^ 0x6164_6465_64);
            for index in (0..COMPOUND_BODIES).step_by(3) {
                let added = ColliderBuilder::ball(0.25).translation(vector_between(&mut random, -0.6, 0.6)).density(1.5);
                self.world.insert_collider(added, Some(self.bodies[index]));
            }
        }
        if tick == DENSITY_CHANGED_AT {
            for index in (1..COMPOUND_BODIES).step_by(3) {
                self.world.colliders.get_mut(self.first_colliders[index]).expect("the collider is in the world").set_density(3.0);
            }
        }
    }

    /// Steps the world once; `tick` counts from 1.
    pub fn step(&mut self, tick: usize) {
        self.change_before(tick);
        self.world.step();
    }

    /// The hash of the transform and the motion of every body, in order.
    pub fn hash(&self) -> u64 {
        let mut hash = Fnv1a::new();
        for &handle in &self.bodies {
            let body = &self.world.bodies[handle];
            let (pose, linear, angular) = (body.position(), body.linvel(), body.angvel());
            let fields = [
                pose.translation.x, pose.translation.y, pose.translation.z,
                pose.rotation.x, pose.rotation.y, pose.rotation.z, pose.rotation.w,
                linear.x, linear.y, linear.z,
                angular.x, angular.y, angular.z,
            ];
            for field in fields {
                hash.write(&field.to_bits().to_le_bytes());
            }
        }
        hash.finish()
    }

    /// The hash of the principal inertia and of its frame for every body,
    /// which is what the eigenvalues of the inertia matrix give, and the
    /// count of bodies whose frame is turned (the matrix was not diagonal).
    pub fn inertia(&self) -> (u64, usize) {
        let mut hash = Fnv1a::new();
        let mut turned = 0;
        for &handle in &self.bodies {
            let properties = &self.world.bodies[handle].mass_properties().local_mprops;
            let (inertia, frame) = (properties.inv_principal_inertia, properties.principal_inertia_local_frame);
            for field in [inertia.x, inertia.y, inertia.z, frame.x, frame.y, frame.z, frame.w] {
                hash.write(&field.to_bits().to_le_bytes());
            }
            if frame.w.abs() != 1.0 {
                turned += 1;
            }
        }
        (hash.finish(), turned)
    }

    /// The contact pairs with an active contact between a ball and a convex
    /// polyhedron, and between two convex polyhedra.
    pub fn hull_contacts(&self) -> (usize, usize) {
        let (mut with_a_ball, mut between_hulls) = (0, 0);
        for pair in self.world.contact_pairs().filter(|pair| pair.has_any_active_contact()) {
            let (Some(a), Some(b)) = (self.world.colliders.get(pair.collider1), self.world.colliders.get(pair.collider2)) else { continue };
            let hulls = [a, b].iter().filter(|collider| collider.shape().as_convex_polyhedron().is_some()).count();
            let balls = [a, b].iter().filter(|collider| collider.shape().as_ball().is_some()).count();
            match (hulls, balls) {
                (1, 1) => with_a_ball += 1,
                (2, 0) => between_hulls += 1,
                _ => {}
            }
        }
        (with_a_ball, between_hulls)
    }
}

/// The start of the steps, as a symbol a debugger can stop at: what a world
/// calls before it belongs to its build, what it calls after to its steps.
#[unsafe(no_mangle)]
#[inline(never)]
pub extern "C" fn rapier_platform_math_steps_begin() {
    std::hint::black_box(());
}

pub struct Ran {
    pub kind: Kind,
    pub feature: bool,
    pub bodies: usize,
    pub colliders: usize,
    pub joints: usize,
    pub initial_hash: u64,
    pub checkpoints: Vec<(usize, u64)>,
    pub inertia_after_build: (u64, usize),
    pub inertia_after_steps: (u64, usize),
    /// The most contact pairs seen at a checkpoint: a ball on a hull, two hulls.
    pub hull_contacts_peak: (usize, usize),
}

impl Ran {
    pub fn final_hash(&self) -> u64 {
        self.checkpoints.last().map(|(_, hash)| *hash).unwrap_or(self.initial_hash)
    }
}

pub fn run(kind: Kind, feature: bool, ticks: usize) -> Ran {
    let mut scene = Scene::build(kind, feature);
    let mut ran = Ran {
        kind,
        feature,
        bodies: scene.bodies.len(),
        colliders: scene.world.colliders.len(),
        joints: scene.joints,
        initial_hash: scene.hash(),
        checkpoints: Vec::new(),
        inertia_after_build: scene.inertia(),
        inertia_after_steps: (0, 0),
        hull_contacts_peak: (0, 0),
    };
    rapier_platform_math_steps_begin();
    for tick in 1..=ticks {
        scene.step(tick);
        if CHECKPOINTS.contains(&tick) || tick == ticks {
            ran.checkpoints.push((tick, scene.hash()));
            let (with_a_ball, between_hulls) = scene.hull_contacts();
            ran.hull_contacts_peak = (ran.hull_contacts_peak.0.max(with_a_ball), ran.hull_contacts_peak.1.max(between_hulls));
        }
    }
    ran.checkpoints.dedup();
    ran.inertia_after_steps = scene.inertia();
    ran
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_world_run_twice_gives_the_same_hashes() {
        for kind in KINDS {
            let (first, second) = (run(kind, true, 30), run(kind, true, 30));
            assert_eq!(first.checkpoints, second.checkpoints, "{}", kind.name());
            assert_ne!(first.initial_hash, first.final_hash(), "{} does not move", kind.name());
        }
    }

    #[test]
    fn no_limit_is_centered_on_zero_and_every_limit_holds_the_rest_angle() {
        for [min, max] in joint_limits() {
            assert!(min < 0.0 && max > 0.0 && min + max != 0.0);
            assert!(max - min < std::f32::consts::TAU);
        }
        assert_eq!(limit_half_centers().len(), 2 * PENDULUMS);
        assert_eq!(motor_half_targets().len(), 3 * PENDULUMS);
    }

    #[test]
    fn the_limits_and_the_motors_change_what_their_worlds_do() {
        for kind in [Kind::JointLimits, Kind::MotorTargets] {
            let (with, without) = (run(kind, true, 100), run(kind, false, 100));
            assert_eq!(with.initial_hash, without.initial_hash, "{}", kind.name());
            assert_ne!(with.final_hash(), without.final_hash(), "{}", kind.name());
        }
    }

    #[test]
    fn the_bodies_of_several_colliders_have_a_turned_inertia_frame_and_the_control_has_none() {
        assert_eq!(run(Kind::Control, true, 1).inertia_after_build.1, 0);
        let ran = run(Kind::SeveralColliders, true, DENSITY_CHANGED_AT + 1);
        assert!(ran.inertia_after_build.1 >= COMPOUND_BODIES, "{} bodies turned", ran.inertia_after_build.1);
        assert_ne!(ran.inertia_after_build.0, ran.inertia_after_steps.0);
    }

    #[test]
    fn balls_come_to_touch_hulls() {
        let ran = run(Kind::BallsOnHulls, true, 300);
        assert!(ran.hull_contacts_peak.0 > 0, "no ball touched a hull");
    }
}
