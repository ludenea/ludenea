//! `PaintTiles`: filling or erasing a rectangle of a tilemap, applied and recorded exactly
//! as the `SetField` of the tilemap's `tiles` that has the same result.

use boxes::BoxesInput;
use ludenea::control::*;
use ludenea::ecs::AssetId;
use ludenea::prelude::*;
use serde_json::{Value, json};

const TILEMAP: &str = "ludenea::Tilemap";
/// The tilemap's grid: 32 by 18 cells.
const WIDTH: u32 = 32;
const HEIGHT: u32 = 18;
/// The identifier of the prefab holding one tilemap.
const PREFAB: &str = "000000000000000000000000000000a1";

fn app() -> App {
    let mut app = boxes::app();
    app.plugin(RenderPlugin).render_layer("world", 1.0);
    app
}

/// The tilemap component's RON, every cell empty.
fn tilemap_ron() -> String {
    let tiles = vec!["None"; (WIDTH * HEIGHT) as usize].join(", ");
    format!(
        "\"{TILEMAP}\": (tileset: \"{:032x}\", cell_size: (x: 1.0, y: 1.0), width: {WIDTH}, \
         height: {HEIGHT}, tiles: [{tiles}], layer: \"world\", order: 0)",
        9
    )
}

/// A scene of a tilemap (local identifier 0), an entity without one (1), and an instance
/// (2) of `PREFAB`.
fn scene() -> String {
    format!(
        "Scene(version: 2, next_id: 3, resources: {{}}, entities: [\
         (id: 0, components: {{{}}}), \
         (id: 1, components: {{\"ludenea::Pose\": (position: (x: 0.0, y: 0.0), rotation: (cos: 1.0, sin: 0.0))}})], \
         instances: [(id: 2, prefab: \"{PREFAB}\")])",
        tilemap_ron()
    )
}

fn prefab() -> String {
    format!(
        "Prefab(version: 1, next_id: 1, entities: [(id: 0, components: {{{}}})], instances: [])",
        tilemap_ron()
    )
}

fn ok(host: &mut Host, command: Command) -> Reply {
    host.handle(command).unwrap()
}

fn error(host: &mut Host, command: Command) -> ProtocolError {
    host.handle(command).unwrap_err()
}

/// A host on `scene()` with 20 idle recorded ticks.
fn loaded() -> Host {
    let mut host = Host::new(app).unwrap();
    ok(
        &mut host,
        Command::LoadScene {
            scene: scene(),
            assets: vec![(AssetId(0xa1), prefab())],
            document: Document::Scene,
        },
    );
    let mut log = InputLog::new(1);
    for _ in 0..20 {
        log.record(&[BoxesInput::default()]);
    }
    ok(
        &mut host,
        Command::LoadInputs {
            log: Bytes(log.encode()),
        },
    );
    host
}

/// The entity of the scene's entity of local identifier `id`, or of instance `id`'s entity.
fn loaded_entity(host: &mut Host, id: u32) -> Entity {
    match ok(host, Command::GetInstances) {
        Reply::Instances { load_addresses, .. } => load_addresses
            .into_iter()
            .find(|(_, address)| address[0] == id)
            .map(|(entity, _)| entity)
            .unwrap(),
        other => panic!("{other:?}"),
    }
}

fn tilemap(host: &mut Host) -> Entity {
    loaded_entity(host, 0)
}

fn step(host: &mut Host, ticks: u32) {
    ok(
        host,
        Command::Step {
            ticks,
            record_hashes: false,
        },
    );
}

fn paint(
    entity: Entity,
    (x, y, width, height): (u32, u32, u32, u32),
    tile: Option<u32>,
) -> Command {
    Command::PaintTiles {
        entity,
        area: TileArea {
            x,
            y,
            width,
            height,
        },
        tile,
    }
}

fn tiles(host: &mut Host, entity: Entity) -> Vec<Value> {
    let Reply::Entity { components, .. } = ok(host, Command::GetEntity { entity }) else {
        panic!("get_entity replies entity");
    };
    match components.get(TILEMAP) {
        Some(ComponentValue::Json { value }) => value["tiles"].as_array().unwrap().clone(),
        other => panic!("{other:?}"),
    }
}

/// The tiles of an empty grid with `tile` in the cells of the area, row-major from the
/// bottom-left cell.
fn expected(area: (u32, u32, u32, u32), tile: Value) -> Vec<Value> {
    let (x, y, width, height) = area;
    (0..HEIGHT)
        .flat_map(|row| (0..WIDTH).map(move |column| (column, row)))
        .map(|(column, row)| {
            let inside = (x..x + width).contains(&column) && (y..y + height).contains(&row);
            if inside { tile.clone() } else { Value::Null }
        })
        .collect()
}

fn set_tiles(entity: Entity, tiles: Vec<Value>) -> Command {
    Command::SetField {
        target: FieldTarget::Component {
            entity,
            component: TILEMAP.to_string(),
        },
        path: vec![PathSegment::Field {
            name: "tiles".to_string(),
        }],
        value: Value::Array(tiles),
    }
}

fn timeline(host: &mut Host) -> Timeline {
    match ok(host, Command::GetTimeline) {
        Reply::Timeline(timeline) => timeline,
        other => panic!("{other:?}"),
    }
}

fn hash(host: &mut Host) -> Hash64 {
    match ok(host, Command::GetHash) {
        Reply::Hash { hash, .. } => hash,
        other => panic!("{other:?}"),
    }
}

fn recorded_edits(host: &mut Host) -> EditLog {
    match ok(host, Command::ExportSession) {
        Reply::Session { edits, .. } => EditLog::decode(&edits.0).unwrap(),
        other => panic!("{other:?}"),
    }
}

fn scene_text(host: &mut Host) -> String {
    match ok(host, Command::SaveScene) {
        Reply::Scene { ron } => ron,
        other => panic!("{other:?}"),
    }
}

/// Sends `command` the way a remote client does: as JSON text, parsed back.
fn through_json_text(command: Command) -> Command {
    let text = serde_json::to_string(&command).unwrap();
    serde_json::from_str(&text).unwrap()
}

#[test]
fn paint_fills_the_rectangle_at_tick_0_as_one_scene_edit() {
    let mut host = loaded();
    let entity = tilemap(&mut host);
    let command = through_json_text(paint(entity, (2, 1, 3, 2), Some(7)));
    assert_eq!(ok(&mut host, command), Reply::Done);

    assert_eq!(tiles(&mut host, entity), expected((2, 1, 3, 2), json!(7)));
    let timeline = timeline(&mut host);
    assert!(timeline.scene_dirty);
    assert!(timeline.edit_ticks.is_empty());
    assert_eq!(timeline.undo_label.as_deref(), Some("Undo scene edit"));
    // The bottom-left cell of the area is the third cell of the second row.
    let row_and_two = "None,".repeat(WIDTH as usize + 2);
    let scene = scene_text(&mut host);
    assert!(
        scene.contains(&format!(
            "tiles:[{row_and_two}Some(7),Some(7),Some(7),None,"
        )),
        "{scene}"
    );
}

#[test]
fn erase_clears_the_rectangle() {
    let mut host = loaded();
    let entity = tilemap(&mut host);
    ok(&mut host, paint(entity, (0, 0, WIDTH, HEIGHT), Some(1)));
    ok(&mut host, paint(entity, (30, 16, 2, 2), None));

    let mut tiles_left = expected((0, 0, WIDTH, HEIGHT), json!(1));
    for (column, row) in [(30, 16), (31, 16), (30, 17), (31, 17)] {
        tiles_left[(row * WIDTH + column) as usize] = Value::Null;
    }
    assert_eq!(tiles(&mut host, entity), tiles_left);
}

#[test]
fn paint_undoes_in_one_step() {
    let mut host = loaded();
    let entity = tilemap(&mut host);
    let before = hash(&mut host);
    ok(&mut host, paint(entity, (0, 0, 4, 4), Some(3)));
    assert_ne!(hash(&mut host), before);

    ok(&mut host, Command::Undo);
    assert_eq!(
        tiles(&mut host, entity),
        expected((0, 0, 0, 0), Value::Null)
    );
    assert_eq!(hash(&mut host), before);
    assert_eq!(
        host.handle(Command::Undo).unwrap_err().code,
        ErrorCode::NothingToUndo
    );
}

#[test]
fn paint_on_a_prefab_instance_is_an_override() {
    let mut host = loaded();
    let entity = loaded_entity(&mut host, 2);
    ok(&mut host, paint(entity, (1, 0, 1, 1), Some(4)));

    let entity = loaded_entity(&mut host, 2);
    assert_eq!(tiles(&mut host, entity), expected((1, 0, 1, 1), json!(4)));
    let scene = scene_text(&mut host);
    assert!(
        scene.contains(
            r#"overrides: [(entity: [0], component: "ludenea::Tilemap", path: [(type:"field",name:"tiles")], value: [None,Some(4),None"#
        ),
        "{scene}"
    );
    assert!(timeline(&mut host).edit_ticks.is_empty());
}

#[test]
fn paint_past_tick_0_records_a_timed_edit_that_replays() {
    let mut host = loaded();
    step(&mut host, 5);
    let entity = tilemap(&mut host);
    ok(&mut host, paint(entity, (5, 5, 2, 3), Some(2)));
    let painted = hash(&mut host);
    step(&mut host, 10);
    let end = hash(&mut host);

    let edits = recorded_edits(&mut host);
    assert_eq!(edits.edits.len(), 1);
    assert_eq!(edits.edits[0].tick, 5);
    assert_eq!(timeline(&mut host).edit_ticks, vec![5]);

    ok(&mut host, Command::Seek { tick: 0 });
    assert_eq!(
        tiles(&mut host, entity),
        expected((0, 0, 0, 0), Value::Null)
    );
    ok(&mut host, Command::Seek { tick: 5 });
    assert_eq!(hash(&mut host), painted);
    assert_eq!(tiles(&mut host, entity), expected((5, 5, 2, 3), json!(2)));
    ok(&mut host, Command::Seek { tick: 15 });
    assert_eq!(hash(&mut host), end);
}

#[test]
fn paint_records_the_same_edit_as_set_field() {
    let area = (3, 4, 5, 6);
    let mut painted = loaded();
    let mut set = loaded();
    let entity = tilemap(&mut painted);

    // At tick 0, the same scene.
    ok(&mut painted, paint(entity, area, Some(8)));
    ok(&mut set, set_tiles(entity, expected(area, json!(8))));
    assert_eq!(scene_text(&mut painted), scene_text(&mut set));

    // Later, the same recorded edit.
    step(&mut painted, 7);
    step(&mut set, 7);
    ok(&mut painted, paint(entity, area, None));
    ok(
        &mut set,
        set_tiles(entity, expected((0, 0, 0, 0), Value::Null)),
    );
    assert_eq!(recorded_edits(&mut painted), recorded_edits(&mut set));
    assert_eq!(hash(&mut painted), hash(&mut set));
}

#[test]
fn paint_and_set_field_of_one_tilemap_in_a_batch_are_refused() {
    let mut host = loaded();
    let entity = tilemap(&mut host);
    let before = hash(&mut host);
    let error = error(
        &mut host,
        Command::Batch {
            commands: vec![
                paint(entity, (0, 0, 1, 1), Some(1)),
                set_tiles(entity, expected((0, 0, 0, 0), Value::Null)),
            ],
        },
    );
    assert_eq!(error.code, ErrorCode::InvalidCommand);
    assert!(
        error.message.contains(&format!(
            "command 1 of the batch changes `ludenea::Tilemap` of entity {entity} again"
        )),
        "{}",
        error.message
    );
    assert_eq!(hash(&mut host), before);
}

#[test]
fn paint_in_a_batch_applies_with_the_other_commands() {
    let mut host = loaded();
    let entity = tilemap(&mut host);
    let instance = loaded_entity(&mut host, 2);
    step(&mut host, 3);
    ok(
        &mut host,
        Command::Batch {
            commands: vec![
                paint(entity, (0, 0, 2, 1), Some(1)),
                paint(instance, (0, 0, 1, 1), Some(2)),
            ],
        },
    );
    assert_eq!(tiles(&mut host, entity), expected((0, 0, 2, 1), json!(1)));
    assert_eq!(tiles(&mut host, instance), expected((0, 0, 1, 1), json!(2)));
    let edits = recorded_edits(&mut host);
    assert_eq!(edits.edits.len(), 1);
    assert!(matches!(&edits.edits[0].edit, Edit::Group(edits) if edits.len() == 2));
}

#[test]
fn area_outside_the_grid_is_refused_naming_both() {
    let mut host = loaded();
    let entity = tilemap(&mut host);
    let before = hash(&mut host);
    let refused = error(&mut host, paint(entity, (30, 0, 4, 2), Some(1)));
    assert_eq!(refused.code, ErrorCode::InvalidCommand);
    assert_eq!(
        refused.message,
        format!(
            "entity {entity}, component `ludenea::Tilemap`, paint_tiles: area x 30..34, y 0..2 \
             is outside the 32 by 18 grid"
        )
    );
    let refused = error(&mut host, paint(entity, (0, 17, 1, 2), Some(1)));
    assert!(
        refused
            .message
            .ends_with("area x 0..1, y 17..19 is outside the 32 by 18 grid"),
        "{}",
        refused.message
    );
    assert_eq!(hash(&mut host), before);
    assert!(!timeline(&mut host).scene_dirty);
}

#[test]
fn overflowing_area_is_refused() {
    let mut host = loaded();
    let entity = tilemap(&mut host);
    for area in [
        (u32::MAX, 0, 2, 1),
        (0, u32::MAX, 1, 2),
        (1, 0, u32::MAX, 1),
    ] {
        let error = error(&mut host, paint(entity, area, Some(1)));
        assert_eq!(error.code, ErrorCode::InvalidCommand);
        assert!(
            error.message.contains("is outside the 32 by 18 grid"),
            "{}",
            error.message
        );
        assert!(
            error.message.starts_with(&format!("entity {entity}, ")),
            "{}",
            error.message
        );
    }
}

#[test]
fn entity_without_a_tilemap_is_refused() {
    let mut host = loaded();
    let entity = loaded_entity(&mut host, 1);
    let error = error(&mut host, paint(entity, (0, 0, 1, 1), Some(1)));
    assert_eq!(error.code, ErrorCode::InvalidCommand);
    assert!(
        error.message.ends_with(&format!(
            "entity {entity} has no ludenea::Tilemap; is RenderPlugin added?"
        )),
        "{}",
        error.message
    );
}

#[test]
fn a_game_without_render_plugin_is_refused_the_same_way() {
    let mut host = Host::new(boxes::app).unwrap();
    ok(
        &mut host,
        Command::LoadScene {
            scene: boxes::generate_scene(2, 7),
            assets: Vec::new(),
            document: Document::Scene,
        },
    );
    let entity = match ok(
        &mut host,
        Command::QueryEntities {
            filter: String::new(),
            offset: 0,
            limit: 1,
        },
    ) {
        Reply::Entities { rows, .. } => rows[0].entity,
        other => panic!("{other:?}"),
    };
    let error = error(&mut host, paint(entity, (0, 0, 1, 1), Some(1)));
    assert_eq!(error.code, ErrorCode::InvalidCommand);
    assert!(
        error.message.ends_with(&format!(
            "entity {entity} has no ludenea::Tilemap; is RenderPlugin added?"
        )),
        "{}",
        error.message
    );
}

#[test]
fn dead_entity_is_refused() {
    let mut host = loaded();
    let entity = tilemap(&mut host);
    step(&mut host, 2);
    ok(&mut host, Command::Despawn { entity });
    let error = error(&mut host, paint(entity, (0, 0, 1, 1), Some(1)));
    assert_eq!(error.code, ErrorCode::NoSuchEntity);
    assert!(
        error.message.contains(&format!("entity {entity}")),
        "{}",
        error.message
    );
    assert_eq!(timeline(&mut host).edit_ticks, vec![2]);
}

#[test]
fn empty_area_is_refused() {
    let mut host = loaded();
    let entity = tilemap(&mut host);
    for area in [(3, 4, 0, 2), (3, 4, 2, 0)] {
        let error = error(&mut host, paint(entity, area, Some(1)));
        assert_eq!(error.code, ErrorCode::InvalidCommand);
        assert!(error.message.contains("is empty"), "{}", error.message);
    }
    assert!(!timeline(&mut host).scene_dirty);
}

#[test]
fn unknown_tile_index_is_accepted() {
    let mut host = loaded();
    let entity = tilemap(&mut host);
    assert_eq!(
        ok(&mut host, paint(entity, (0, 0, 1, 1), Some(u32::MAX))),
        Reply::Done
    );
    assert_eq!(
        tiles(&mut host, entity),
        expected((0, 0, 1, 1), json!(u32::MAX))
    );
}

/// Shortens each tilemap by one cell every tick, as game code that breaks the invariant.
fn shrink(mut tilemaps: Query<&mut Tilemap>) {
    for tilemap in tilemaps.iter_mut() {
        tilemap.tiles.pop();
    }
}

fn shrinking_app() -> App {
    let mut app = app();
    app.system(Sim, shrink);
    app
}

#[test]
fn a_tilemap_whose_tiles_do_not_fill_its_grid_is_refused() {
    let mut host = Host::new(shrinking_app).unwrap();
    ok(
        &mut host,
        Command::LoadScene {
            scene: scene(),
            assets: vec![(AssetId(0xa1), prefab())],
            document: Document::Scene,
        },
    );
    let mut log = InputLog::new(1);
    log.record(&[BoxesInput::default()]);
    ok(
        &mut host,
        Command::LoadInputs {
            log: Bytes(log.encode()),
        },
    );
    step(&mut host, 1);
    let entity = tilemap(&mut host);
    let error = error(&mut host, paint(entity, (0, 0, 1, 1), Some(1)));
    assert_eq!(error.code, ErrorCode::InvalidCommand, "{}", error.message);
    assert!(
        error
            .message
            .contains("tiles has 575 entries, but width 32 by height 18 needs 576"),
        "{}",
        error.message
    );
}
