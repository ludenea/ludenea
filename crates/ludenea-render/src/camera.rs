//! The 2D camera's math, GPU-free: how much of the world a target shows, where a world point
//! lands in clip space, and which world point a screen pixel shows. Display only: nothing here
//! feeds back into the simulation.

use ludenea_draw::frame::CameraView;
use ludenea_math::Vec2;

/// A target's size in pixels.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Size {
    pub width: u32,
    pub height: u32,
}

/// What a frame without a camera shows: 20 by 11.25 world units (16:9) around the origin.
pub const DEFAULT_CAMERA: CameraView = CameraView {
    center: Vec2::ZERO,
    view: Vec2::new(20.0, 11.25),
};

/// The visible area for a target of `aspect = width / height`: `view` widened on the longer
/// side to match `aspect`, never narrowed and never stretched, so the whole minimum view
/// stays on screen at the same scale on both axes.
///
/// `aspect` must be positive and finite; for any other aspect (a target with no pixels, or a
/// NaN), `view` is returned unchanged rather than an infinite or NaN area.
pub fn fit(view: Vec2, aspect: f32) -> Vec2 {
    if !(aspect.is_finite() && aspect > 0.0) {
        return view;
    }
    if view.x < view.y * aspect {
        Vec2::new(view.y * aspect, view.y)
    } else {
        Vec2::new(view.x, view.x / aspect)
    }
}

/// Where `point` lands in clip space (-1 to 1 across `visible`, +y up) for a layer of
/// `parallax`: the layer sees the camera at `center * parallax`, so 0 is fixed to the screen
/// and 1 moves with the world.
pub fn world_to_clip(point: Vec2, camera: CameraView, visible: Vec2, parallax: f32) -> [f32; 2] {
    [
        2.0 * (point.x - camera.center.x * parallax) / visible.x,
        2.0 * (point.y - camera.center.y * parallax) / visible.y,
    ]
}

/// The world point a layer of `parallax` shows at `screen`, in pixels from the top-left of a
/// target of `size` (+y down) through `camera`: the inverse of `world_to_clip` for that layer,
/// with the visible area `fit` gives the target. A side of 0 pixels counts as 1, as the
/// renderer draws it.
pub fn screen_to_world(screen: Vec2, size: Size, camera: CameraView, parallax: f32) -> Vec2 {
    let (width, height) = (size.width.max(1) as f32, size.height.max(1) as f32);
    let visible = fit(camera.view, width / height);
    let clip = Vec2::new(2.0 * screen.x / width - 1.0, 1.0 - 2.0 * screen.y / height);
    Vec2::new(
        clip.x * visible.x / 2.0 + camera.center.x * parallax,
        clip.y * visible.y / 2.0 + camera.center.y * parallax,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fit_widens_the_longer_side() {
        assert_eq!(fit(Vec2::new(4.0, 4.0), 2.0), Vec2::new(8.0, 4.0));
        assert_eq!(fit(Vec2::new(4.0, 4.0), 0.5), Vec2::new(4.0, 8.0));
    }

    #[test]
    fn fit_never_narrows_the_view() {
        assert_eq!(fit(Vec2::new(16.0, 9.0), 1.0), Vec2::new(16.0, 16.0));
        assert_eq!(fit(Vec2::new(16.0, 9.0), 16.0 / 9.0), Vec2::new(16.0, 9.0));
        assert_eq!(fit(Vec2::new(9.0, 16.0), 1.0), Vec2::new(16.0, 16.0));
    }

    #[test]
    fn fit_keeps_the_view_for_an_aspect_that_is_not_positive_and_finite() {
        let view = Vec2::new(16.0, 9.0);
        for aspect in [0.0, -1.0, f32::INFINITY, f32::NEG_INFINITY, f32::NAN] {
            assert_eq!(fit(view, aspect), view, "aspect {aspect}");
        }
    }

    #[test]
    fn parallax_zero_ignores_the_camera() {
        let camera = CameraView {
            center: Vec2::new(5.0, 5.0),
            view: Vec2::new(4.0, 4.0),
        };
        let visible = Vec2::new(4.0, 4.0);
        let point = Vec2::new(1.0, 1.0);
        assert_eq!(
            world_to_clip(point, camera.clone(), visible, 0.0),
            [0.5, 0.5]
        );
        assert_eq!(
            world_to_clip(point, camera.clone(), visible, 1.0),
            [-2.0, -2.0]
        );
        assert_eq!(world_to_clip(point, camera, visible, 0.5), [-0.75, -0.75]);
    }
}
