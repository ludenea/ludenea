//! The frame a visual test captures, as the spike's own draw list, and the
//! changes brought about on purpose. Throwaway, never product code.
//!
//! The draw list is the device baseline spike's filtering and blending scene,
//! laid out as one view of three passes, plus one item that lies outside the
//! view. Its content hash is a stand-in for the renderer's: the hash of this
//! list, computed by the code that builds it, with a part per view and per
//! pass and a part per asset. It cannot show that a real renderer's record
//! lacks something.
//!
//! The adapter ([`Frame::device_scene`]) turns the list into what the device
//! baseline's native path draws. A fault of the adapter is applied there,
//! beneath the content: the list, and so its hash, do not see it.

use reference_workload::Fnv1a;
use render_device_baseline::scene::{Draw, Encoding, HEIGHT, SAMPLES, Scene, TEXTURE_GRADIENT, TEXTURE_HASHED, Texture, Vertex, WIDTH};

/// The name of the one view.
pub const VIEW: &str = "main";
/// The passes of the view, with the range of the scene's draws each holds.
const PASSES: [(&str, std::ops::Range<usize>); 3] = [("opaque", 0..6), ("translucent", 6..9), ("overlay", 9..11)];
/// The item added to the device baseline's scene: a triangle beyond the right
/// edge of the view, which no device draws a pixel of.
const OUTSIDE_ITEM: &str = "triangle_outside_the_view";

/// A change brought about on purpose, each an option of the frame's build.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Change {
    None,
    /// The middle triangle of the three is moved six pixels to the right.
    ItemMovedInsideTheView,
    /// The triangle that lies beyond the view is moved further away.
    ItemMovedOutsideEveryView,
    /// The gradient texture is encoded again to five bits per channel and
    /// back: other bytes, the same size, levels and encoding.
    TextureEncodedAgain,
    /// One texel of the hashed texture that no draw samples is changed: other
    /// bytes in the payload, none in the picture.
    TextureEncodedAgainWhereNothingSamples,
}

/// A fault of the adapter beneath the content.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AdapterFault {
    None,
    /// The red disc, a blended item, is drawn with blending off and depth
    /// write on: a blend state set wrongly.
    BlendStateSetWrongly,
}

pub struct Item {
    pub draw: Draw,
    pub vertices: Vec<Vertex>,
}

pub struct Pass {
    pub name: &'static str,
    pub items: Vec<Item>,
}

/// What a frame is drawn into.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Target {
    pub width: u32,
    pub height: u32,
    pub format: &'static str,
    pub encoding: &'static str,
    pub samples: u32,
}

impl Target {
    pub fn text(&self) -> String {
        format!("{} by {}, {}, {}, {} samples", self.width, self.height, self.format, self.encoding, self.samples)
    }
}

pub struct Frame {
    pub tick: u64,
    pub target: Target,
    pub passes: Vec<Pass>,
    pub assets: Vec<Texture>,
}

/// The content of a frame as a reference records it: one hash, its parts per
/// view and per pass in the frame's order, and a part per asset.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Content {
    pub hash: u64,
    /// The view's name and its hash.
    pub view: (String, u64),
    /// Each pass's name and its hash, in order.
    pub passes: Vec<(String, u64)>,
    /// For each asset, its identifier and the hash of what its payload feeds
    /// into the content: its size, the count of its levels and its encoding.
    pub assets: Vec<(String, u64)>,
}

fn bytes_of<T: Copy>(values: &[T]) -> &[u8] {
    // SAFETY: the callers pass slices of `Vertex`, a `repr(C)` type made of
    // floats with no padding; the slice covers exactly the elements.
    unsafe { std::slice::from_raw_parts(values.as_ptr() as *const u8, std::mem::size_of_val(values)) }
}

impl Frame {
    /// The frame at the device baseline's target size.
    pub fn build(change: Change) -> Frame {
        Frame::build_at(change, WIDTH, HEIGHT)
    }

    /// The same frame at another target size: the items are in clip space, so
    /// the picture is the scene stretched to the target.
    pub fn build_at(change: Change, width: u32, height: u32) -> Frame {
        let scene = Scene::build();
        let mut vertices = scene.vertices.clone();
        let mut draws = scene.draws.clone();
        // The triangle beyond the right edge: every x is above w, so it is
        // clipped away whole whatever the device's rule for edges.
        let first = vertices.len() as u32;
        let outside_by = if change == Change::ItemMovedOutsideEveryView { 4.0 } else { 2.0 };
        for (x, y) in [(outside_by, -0.5), (outside_by + 0.5, 0.5), (outside_by + 1.0, -0.5)] {
            vertices.push(Vertex { position: [x, y, 0.5, 1.0], uv: [0.0, 0.0], color: [1.0, 0.0, 1.0, 1.0] });
        }
        let white = draws[0];
        draws.push(Draw { name: OUTSIDE_ITEM, first_vertex: first, vertex_count: 3, texture: white.texture, sampler: white.sampler, blended: false, indirect: false });
        if change == Change::ItemMovedInsideTheView {
            let middle = draws.iter().find(|draw| draw.name == "triangle_middle").expect("the middle triangle");
            for vertex in &mut vertices[middle.first_vertex as usize..(middle.first_vertex + middle.vertex_count) as usize] {
                // Six pixels of the 256-pixel target; the division is exact.
                vertex.position[0] += 6.0 / 128.0;
            }
        }
        let mut assets = scene.textures;
        match change {
            Change::TextureEncodedAgain => {
                for value in assets[TEXTURE_GRADIENT as usize].levels.iter_mut().flatten() {
                    *value = (*value & 0xf8) | (*value >> 5);
                }
            }
            Change::TextureEncodedAgainWhereNothingSamples => {
                // The nearest-filtered quad reads the first 12 by 12 texels of
                // the first level; the last texel is read by nothing.
                let level = &mut assets[TEXTURE_HASHED as usize].levels[0];
                let last = level.len() - 4;
                level[last] ^= 0x80;
            }
            _ => {}
        }
        let passes = PASSES
            .iter()
            .map(|(name, range)| Pass {
                name,
                items: draws[range.clone()]
                    .iter()
                    .map(|draw| Item { draw: *draw, vertices: vertices[draw.first_vertex as usize..(draw.first_vertex + draw.vertex_count) as usize].to_vec() })
                    .collect(),
            })
            .collect();
        Frame { tick: 0, target: Target { width, height, format: "rgba8", encoding: "srgb", samples: SAMPLES }, passes, assets }
    }

    /// The content hash and its parts.
    pub fn content(&self) -> Content {
        let assets: Vec<(String, u64)> = self
            .assets
            .iter()
            .map(|texture| {
                let mut hasher = Fnv1a::new();
                hasher.write(&texture.width.to_le_bytes());
                hasher.write(&texture.height.to_le_bytes());
                hasher.write(&(texture.levels.len() as u32).to_le_bytes());
                hasher.write(&[matches!(texture.encoding, Encoding::Srgb) as u8]);
                (texture.name.to_string(), hasher.finish())
            })
            .collect();
        let passes: Vec<(String, u64)> = self
            .passes
            .iter()
            .map(|pass| {
                let mut hasher = Fnv1a::new();
                hasher.write(pass.name.as_bytes());
                for item in &pass.items {
                    hasher.write(item.draw.name.as_bytes());
                    hasher.write(bytes_of(&item.vertices));
                    hasher.write(self.assets[item.draw.texture as usize].name.as_bytes());
                    hasher.write(&[item.draw.sampler as u8, item.draw.blended as u8, item.draw.indirect as u8]);
                }
                (pass.name.to_string(), hasher.finish())
            })
            .collect();
        let mut view = Fnv1a::new();
        view.write(VIEW.as_bytes());
        view.write(self.target.text().as_bytes());
        for (_, hash) in &passes {
            view.write(&hash.to_le_bytes());
        }
        let view = view.finish();
        let mut content = Fnv1a::new();
        content.write(&self.tick.to_le_bytes());
        content.write(&view.to_le_bytes());
        for (name, hash) in &assets {
            content.write(name.as_bytes());
            content.write(&hash.to_le_bytes());
        }
        Content { hash: content.finish(), view: (VIEW.to_string(), view), passes, assets }
    }

    /// The hash of each asset's payload, the bytes a device binds.
    pub fn payloads(&self) -> Vec<(String, u64)> {
        self.assets
            .iter()
            .map(|texture| {
                let mut hasher = Fnv1a::new();
                for level in &texture.levels {
                    hasher.write(level);
                }
                (texture.name.to_string(), hasher.finish())
            })
            .collect()
    }

    /// The adapter: what the device baseline's native path draws for this
    /// frame, with a fault applied beneath the content where one is asked.
    pub fn device_scene(&self, fault: AdapterFault) -> Scene {
        let mut scene = Scene::build();
        scene.target = [self.target.width, self.target.height];
        scene.vertices.clear();
        scene.draws.clear();
        for item in self.passes.iter().flat_map(|pass| &pass.items) {
            let mut draw = item.draw;
            draw.first_vertex = scene.vertices.len() as u32;
            if fault == AdapterFault::BlendStateSetWrongly && draw.name == "disc_red" {
                draw.blended = false;
            }
            scene.vertices.extend_from_slice(&item.vertices);
            scene.draws.push(draw);
        }
        let fan = scene.draws.iter().find(|draw| draw.indirect).expect("the scene's indirect draw");
        scene.indirect_arguments = [fan.vertex_count, 1, fan.first_vertex, 0];
        scene.chunks = [0..3, 3..5, 5..7, 7..scene.draws.len()];
        scene.textures = self.assets.iter().map(|texture| Texture { name: texture.name, width: texture.width, height: texture.height, encoding: texture.encoding, levels: texture.levels.clone() }).collect();
        scene
    }

    /// The hash of the fixture's starting state: here, of every input the
    /// device baseline's scene is built from.
    pub fn fixture_hash() -> u64 {
        Scene::build().input_hash()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_unchanged_frame_keeps_the_device_baseline_draws_in_order() {
        let scene = Scene::build();
        let adapted = Frame::build(Change::None).device_scene(AdapterFault::None);
        assert_eq!(&adapted.draws[..scene.draws.len()], &scene.draws[..]);
        assert_eq!(&adapted.vertices[..scene.vertices.len()], &scene.vertices[..]);
        assert_eq!(adapted.indirect_arguments, scene.indirect_arguments);
        assert_eq!(adapted.draws.len(), scene.draws.len() + 1);
    }

    #[test]
    fn each_change_moves_the_part_of_the_content_it_should() {
        let base = Frame::build(Change::None);
        let (content, payloads) = (base.content(), base.payloads());
        let inside = Frame::build(Change::ItemMovedInsideTheView).content();
        assert_ne!(inside.hash, content.hash);
        assert_ne!(inside.passes[0], content.passes[0]);
        assert_eq!(inside.passes[1..], content.passes[1..]);
        let outside = Frame::build(Change::ItemMovedOutsideEveryView).content();
        assert_ne!(outside.hash, content.hash);
        assert_eq!(outside.passes[..2], content.passes[..2]);
        assert_ne!(outside.passes[2], content.passes[2]);
        for change in [Change::TextureEncodedAgain, Change::TextureEncodedAgainWhereNothingSamples] {
            let frame = Frame::build(change);
            assert_eq!(frame.content(), content, "an asset encoded again keeps its part in the content");
            assert_ne!(frame.payloads(), payloads);
        }
    }

    #[test]
    fn the_adapter_fault_is_beneath_the_content() {
        let frame = Frame::build(Change::None);
        let (right, wrong) = (frame.device_scene(AdapterFault::None), frame.device_scene(AdapterFault::BlendStateSetWrongly));
        assert_ne!(right.draws, wrong.draws);
        assert_eq!(right.vertices, wrong.vertices);
    }

    #[test]
    fn the_content_does_not_depend_on_the_target_size_alone_being_restated() {
        assert_eq!(Frame::build(Change::None).content(), Frame::build_at(Change::None, WIDTH, HEIGHT).content());
        assert_ne!(Frame::build(Change::None).content().hash, Frame::build_at(Change::None, 512, 512).content().hash);
    }
}
