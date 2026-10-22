//! The fighters: spawned on the first tick, sensing the level with raycasts after the physics
//! step, moved by their own velocity, and animated from what they do.

use ludenea::math::{clamp, max};
use ludenea::physics::LINEAR_SLOP;
use ludenea::prelude::*;

use crate::{ArenaInput, Fighter, FighterSheets, SpawnPoint};

/// Horizontal distance a fighter covers in one tick at full run, in world units.
pub const RUN_SPEED: f32 = 0.12;
/// Upward velocity a jump starts with, in units per tick.
pub const JUMP_SPEED: f32 = 0.35;
/// Velocity lost every tick in the air, in units per tick squared.
pub const GRAVITY: f32 = 0.02;
/// The fastest fall, in units per tick.
pub const MAX_FALL: f32 = 0.5;
/// The fighter's capsule: its height from the feet (the `Pose`) and its radius.
pub const FIGHTER_HEIGHT: f32 = 2.0;
pub const FIGHTER_RADIUS: f32 = 0.4;
/// From the capsule's centre to its bottom.
const HALF_HEIGHT: f32 = FIGHTER_HEIGHT / 2.0;
/// How far below its bottom the ground still holds a fighter.
const GROUND_PROBE: f32 = 0.05;

/// On the first tick, spawns the fighter of each player present at the first spawn point of
/// that player, in entity order. A player without a spawn point gets no fighter. A world
/// that already holds fighters, such as a scene saved during a match, gets none.
pub(crate) fn spawn_fighters(
    time: Res<Time>,
    inputs: Runtime<Inputs<ArenaInput>>,
    sheets: Res<FighterSheets>,
    points: Query<(&SpawnPoint, &Pose)>,
    fighters: Query<&Fighter>,
    mut commands: Commands,
) {
    if time.tick != 0 || fighters.iter().next().is_some() {
        return;
    }
    for player in 0..inputs.len() {
        let Some((point, pose)) = points
            .iter()
            .find(|(point, _)| usize::from(point.player) == player)
        else {
            continue;
        };
        let capsule = Capsule {
            center1: Vec2::new(0.0, FIGHTER_RADIUS),
            center2: Vec2::new(0.0, FIGHTER_HEIGHT - FIGHTER_RADIUS),
            radius: FIGHTER_RADIUS,
        };
        commands
            .spawn()
            .insert(Pose {
                position: pose.position,
                rotation: Rot::IDENTITY,
            })
            .insert(Body::kinematic())
            .insert(Collider::new(Shape::Capsule(capsule)))
            .insert(Sprite {
                layer: "fighters".to_string(),
                ..Sprite::default()
            })
            .insert(SpriteAnimation {
                sheet: sheets.look(point.player).idle,
                clip: "idle".to_string(),
                started: time.tick,
                looping: true,
            })
            .insert(Fighter {
                player: point.player,
                velocity: Vec2::ZERO,
                grounded: false,
                facing: 1,
            });
    }
}

/// Turns each fighter's input into this tick's move, limited by the level: a downward ray
/// from the capsule's centre finds the ground (`GROUND_PROBE` past the bottom) and where a
/// fall lands, a sideways ray how far a run goes before a wall. Each ray starts inside the
/// fighter's own capsule, which `raycast` therefore ignores. Colliders are one-way upward: a
/// fighter rises through them and stands only on a surface its feet are on or above.
pub(crate) fn sense_fighters(
    inputs: Runtime<Inputs<ArenaInput>>,
    colliders: Query<(Entity, &Pose, &Collider)>,
    mut fighters: Query<(Entity, &mut Fighter)>,
) {
    for (entity, fighter) in fighters.iter_mut() {
        let Ok((_, pose, _)) = colliders.get(entity) else {
            continue;
        };
        let index = usize::from(fighter.player);
        let input = if index < inputs.len() {
            inputs.player(index).clone()
        } else {
            ArenaInput::default()
        };
        let centre = pose.position + Vec2::new(0.0, HALF_HEIGHT);

        let ground =
            surface_below(&colliders, centre, GROUND_PROBE).filter(|_| fighter.velocity.y <= 0.0);
        let on_ground = ground.is_some();
        let mut rise = if let Some(gap) = ground {
            // Settles onto the ground rather than hovering up to `GROUND_PROBE` above it.
            if input.jump { JUMP_SPEED } else { -free(gap) }
        } else {
            max(fighter.velocity.y - GRAVITY, -MAX_FALL)
        };
        if rise < 0.0
            && let Some(gap) = surface_below(&colliders, centre, -rise)
        {
            rise = -free(gap);
        }

        let run = if input.run.is_finite() {
            clamp(input.run, -1.0, 1.0)
        } else {
            0.0
        };
        let mut step = run * RUN_SPEED;
        if step != 0.0 {
            let direction = if step > 0.0 { 1.0 } else { -1.0 };
            let reach = FIGHTER_RADIUS + step * direction;
            if let Some(hit) = raycast(&colliders, centre, Vec2::new(reach * direction, 0.0)) {
                step = direction * free(hit.fraction * reach - FIGHTER_RADIUS);
            }
            fighter.facing = if direction > 0.0 { 1 } else { -1 };
        }

        fighter.velocity = Vec2::new(step, rise);
        fighter.grounded = on_ground && rise <= 0.0;
    }
}

/// The gap from the feet of the fighter centred at `centre` down to the first surface at
/// most `depth` below them, skipping each surface above the feet (within `LINEAR_SLOP`): a
/// fighter rising through a collider does not stand inside it. A skipped surface is passed
/// by casting again from just inside its collider, which `raycast` then ignores.
fn surface_below(
    colliders: &Query<(Entity, &Pose, &Collider)>,
    centre: Vec2,
    depth: f32,
) -> Option<f32> {
    let feet = centre.y - HALF_HEIGHT;
    let mut origin = centre;
    // Each pass moves the origin down by `LINEAR_SLOP` at least, toward `feet - depth`.
    loop {
        let hit = raycast(colliders, origin, Vec2::new(0.0, feet - depth - origin.y))?;
        let gap = feet - hit.point.y;
        if gap >= -LINEAR_SLOP {
            return Some(gap);
        }
        origin = Vec2::new(origin.x, hit.point.y - LINEAR_SLOP);
    }
}

/// The move allowed toward a surface `gap` away. A gap within the physics' `LINEAR_SLOP` is
/// float residue of the last move: the fighter touches the surface and stays, rather than
/// creeping toward it forever.
fn free(gap: f32) -> f32 {
    if gap > LINEAR_SLOP { gap } else { 0.0 }
}

/// Moves each fighter by the velocity `sense_fighters` allowed.
pub(crate) fn move_fighters(mut fighters: Query<(&Fighter, &mut Pose)>) {
    for (fighter, pose) in fighters.iter_mut() {
        pose.position += fighter.velocity;
    }
}

/// Plays `jump` while rising, `fall` while falling, else `run` or `idle`, restarting the
/// animation only when its clip changes, and flips the sprite to face the last run.
pub(crate) fn animate_fighters(
    time: Res<Time>,
    sheets: Res<FighterSheets>,
    mut fighters: Query<(&Fighter, &mut SpriteAnimation, &mut Sprite)>,
) {
    for (fighter, animation, sprite) in fighters.iter_mut() {
        let look = sheets.look(fighter.player);
        let (clip, sheet, looping) = if !fighter.grounded && fighter.velocity.y > 0.0 {
            ("jump", look.jump, false)
        } else if !fighter.grounded && fighter.velocity.y < 0.0 {
            ("fall", look.fall, false)
        } else if fighter.velocity.x != 0.0 {
            ("run", look.run, true)
        } else {
            ("idle", look.idle, true)
        };
        if animation.clip != clip {
            *animation = SpriteAnimation {
                sheet,
                clip: clip.to_string(),
                started: time.tick,
                looping,
            };
        }
        sprite.flip_x = fighter.facing < 0;
    }
}
