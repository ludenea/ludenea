//! The camera follows the fighters.

use ludenea::math::{max, min};
use ludenea::prelude::*;

use crate::Fighter;

/// Added to the fighters' bounding box, on each axis, so they never touch the screen's edge.
const MARGIN: f32 = 6.0;
/// The smallest view, so that a lone fighter or two close ones do not fill the screen.
const MIN_VIEW: Vec2 = Vec2::new(24.0, 13.5);

/// Centres every camera on the midpoint of the fighters' bounding box and makes its view that
/// box plus `MARGIN`, at least `MIN_VIEW`. Without fighters the cameras stay as they are.
///
/// One query holds both, since the cameras' and the fighters' `Pose` are the same storage.
pub(crate) fn follow_camera(
    mut entities: Query<(&mut Pose, Option<&Fighter>, Option<&mut Camera2d>)>,
) {
    let mut bounds: Option<(Vec2, Vec2)> = None;
    for (pose, fighter, _) in entities.iter_mut() {
        if fighter.is_none() {
            continue;
        }
        let p = pose.position;
        bounds = Some(match bounds {
            None => (p, p),
            Some((low, high)) => (
                Vec2::new(min(low.x, p.x), min(low.y, p.y)),
                Vec2::new(max(high.x, p.x), max(high.y, p.y)),
            ),
        });
    }
    let Some((low, high)) = bounds else {
        return;
    };
    let centre = (low + high) * 0.5;
    let view = Vec2::new(
        max(high.x - low.x + MARGIN, MIN_VIEW.x),
        max(high.y - low.y + MARGIN, MIN_VIEW.y),
    );
    for (pose, _, camera) in entities.iter_mut() {
        if let Some(camera) = camera {
            pose.position = centre;
            camera.view = view;
        }
    }
}
