//! Extraction: the camera, sprites, tilemaps and teleports of a world, added to its render
//! frame, with a warning naming the entity for everything that cannot be drawn.

use ludenea_app::pose::Pose;
use ludenea_draw::frame::{CameraView, RenderFrame, SpriteItem, TilemapItem};
use ludenea_ecs::{Assets, Entity, Query, SystemParam, World};

use crate::components::{Camera2d, Sprite, SpriteAnimation, Teleported, Tilemap};
use crate::sheet::{SpriteSheet, quoted};

/// The extractor `RenderPlugin` registers.
pub fn extract(world: &World, frame: &mut RenderFrame) {
    extract_camera(world, frame);
    extract_sprites(world, frame);
    extract_tilemaps(world, frame);
    check_animations(world, frame);
    if let Ok(teleported) = Query::<(Entity, &Teleported)>::new(world) {
        frame.teleported = teleported
            .iter()
            .filter(|(_, teleported)| teleported.tick == frame.tick)
            .map(|(entity, _)| entity)
            .collect();
    }
}

/// The camera of the lowest entity with a `Camera2d` and a `Pose`.
fn extract_camera(world: &World, frame: &mut RenderFrame) {
    let Ok(cameras) = Query::<(Entity, &Camera2d, &Pose)>::new(world) else {
        return;
    };
    let mut cameras: Vec<_> = cameras.iter().collect();
    cameras.sort_by_key(|(entity, _, _)| *entity);
    let Some(&(first, camera, pose)) = cameras.first() else {
        return;
    };
    if cameras.len() > 1 {
        let entities: Vec<String> = cameras
            .iter()
            .map(|(entity, _, _)| entity.to_string())
            .collect();
        frame.warnings.push(format!(
            "{} entities have a Camera2d and a Pose ({}); {first} is used",
            cameras.len(),
            entities.join(", ")
        ));
    }
    frame.camera = Some(CameraView {
        center: pose.position,
        view: camera.view,
    });
}

fn extract_sprites(world: &World, frame: &mut RenderFrame) {
    let Ok(sprites) = Query::<(Entity, &Sprite, Option<&Pose>)>::new(world) else {
        return;
    };
    for (entity, sprite, pose) in sprites.iter() {
        let Some(pose) = posed(frame, entity, "Sprite", pose) else {
            continue;
        };
        let Some(layer) = layer(frame, entity, "sprite", &sprite.layer) else {
            continue;
        };
        frame.sprites.push(SpriteItem {
            entity,
            layer,
            order: sprite.order,
            texture: sprite.texture,
            frame: sprite.frame,
            position: pose.position,
            rotation: pose.rotation,
            flip_x: sprite.flip_x,
            flip_y: sprite.flip_y,
            tint: sprite.tint,
        });
    }
    frame
        .sprites
        .sort_by_key(|item| (item.layer, item.order, item.entity));
}

fn extract_tilemaps(world: &World, frame: &mut RenderFrame) {
    let Ok(tilemaps) = Query::<(Entity, &Tilemap, Option<&Pose>)>::new(world) else {
        return;
    };
    for (entity, tilemap, pose) in tilemaps.iter() {
        let Some(pose) = posed(frame, entity, "Tilemap", pose) else {
            continue;
        };
        let Some(layer) = layer(frame, entity, "tilemap", &tilemap.layer) else {
            continue;
        };
        frame.tilemaps.push(TilemapItem {
            entity,
            layer,
            order: tilemap.order,
            tileset: tilemap.tileset,
            position: pose.position,
            cell_size: tilemap.cell_size,
            width: tilemap.width,
            height: tilemap.height,
            tiles: tilemap.tiles.clone(),
        });
    }
    frame
        .tilemaps
        .sort_by_key(|item| (item.layer, item.order, item.entity));
}

/// Warns about each animation `animate` cannot play, whose sprite keeps its last frame.
fn check_animations(world: &World, frame: &mut RenderFrame) {
    let Ok(animations) = Query::<(Entity, &SpriteAnimation)>::new(world) else {
        return;
    };
    let sheets = <Assets<SpriteSheet> as SystemParam>::fetch(world);
    for (entity, animation) in animations.iter() {
        let id = animation.sheet.id;
        match sheets.try_get(animation.sheet) {
            None => frame.warnings.push(format!(
                "entity {entity}: sprite sheet {id} is not loaded, so the sprite keeps its last frame"
            )),
            Some(sheet) if !sheet.clips.contains_key(&animation.clip) => {
                let known: Vec<String> = sheet.clips.keys().cloned().collect();
                frame.warnings.push(format!(
                    "entity {entity}: animation clip `{}` is not in sprite sheet {id}; its clips: {}",
                    animation.clip,
                    quoted(&known)
                ));
            }
            Some(_) => {}
        }
    }
}

/// `pose`, or a warning that the entity is not drawn without one.
fn posed<'a>(
    frame: &mut RenderFrame,
    entity: Entity,
    component: &str,
    pose: Option<&'a Pose>,
) -> Option<&'a Pose> {
    if pose.is_none() {
        frame.warnings.push(format!(
            "entity {entity}: has a {component} but no Pose, so it is not drawn; add a Pose"
        ));
    }
    pose
}

/// The index of the layer `name` (empty is the first declared layer), or a warning listing
/// the declared layers.
fn layer(frame: &mut RenderFrame, entity: Entity, what: &str, name: &str) -> Option<u16> {
    let index = if name.is_empty() {
        (!frame.layers.is_empty()).then_some(0)
    } else {
        frame.layer_index(name)
    };
    if index.is_none() {
        let declared: Vec<String> = frame
            .layers
            .iter()
            .map(|layer| layer.name.clone())
            .collect();
        let layer = if name.is_empty() {
            "the first layer".to_string()
        } else {
            format!("`{name}`")
        };
        frame.warnings.push(format!(
            "entity {entity}: {what} layer {layer} is not declared; declared layers: {}",
            quoted(&declared)
        ));
    }
    index
}
