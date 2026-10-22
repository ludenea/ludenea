//! A second save of a world of sprites into the same `SavedWorld` allocates nothing: the
//! hand-written `clone_from` of `Sprite` and `SpriteAnimation` reuses their strings. A
//! counting global allocator proves it, which pointer checks on the private storages could
//! not do. The file holds one test only, so no other thread allocates during the count.
#![cfg(not(target_family = "wasm"))]

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicUsize, Ordering};

use ludenea_ecs::{AssetId, Handle, SavedWorld, World};
use ludenea_render::{Sprite, SpriteAnimation, SpriteSheet};

struct Counting;

static ALLOCATIONS: AtomicUsize = AtomicUsize::new(0);

unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        ALLOCATIONS.fetch_add(1, Ordering::Relaxed);
        unsafe { System.alloc(layout) }
    }

    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        unsafe { System.dealloc(pointer, layout) }
    }

    unsafe fn realloc(&self, pointer: *mut u8, layout: Layout, size: usize) -> *mut u8 {
        ALLOCATIONS.fetch_add(1, Ordering::Relaxed);
        unsafe { System.realloc(pointer, layout, size) }
    }
}

#[global_allocator]
static ALLOCATOR: Counting = Counting;

#[test]
fn saving_sprites_again_allocates_nothing() {
    let mut world = World::new();
    world.register_component::<Sprite>().unwrap();
    world.register_component::<SpriteAnimation>().unwrap();
    for index in 0..1_000_u32 {
        let entity = world.spawn();
        let sprite = Sprite {
            layer: "world".to_owned(),
            frame: index,
            ..Sprite::default()
        };
        world.insert(entity, sprite).unwrap();
        let animation = SpriteAnimation {
            sheet: Handle::<SpriteSheet>::new(AssetId(1)),
            clip: "fly".to_owned(),
            started: 0,
            looping: true,
        };
        world.insert(entity, animation).unwrap();
    }
    let mut saved = SavedWorld::default();
    world.save_into(&mut saved);

    // The same shape with other values.
    for entity in world.entities().collect::<Vec<_>>() {
        world.get_mut::<Sprite>(entity).unwrap().frame += 1;
        world.get_mut::<SpriteAnimation>(entity).unwrap().started += 1;
    }
    let before = ALLOCATIONS.load(Ordering::Relaxed);
    world.save_into(&mut saved);
    let during_save = ALLOCATIONS.load(Ordering::Relaxed) - before;
    assert_eq!(during_save, 0, "the second save allocated");

    let before = ALLOCATIONS.load(Ordering::Relaxed);
    world.load(&saved).unwrap();
    let during_load = ALLOCATIONS.load(Ordering::Relaxed) - before;
    assert_eq!(during_load, 0, "the load allocated");
}
