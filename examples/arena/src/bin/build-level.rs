//! Builds the arena's level through the control surface, as the editor builds one: it starts
//! the game serving the control protocol on its standard input and output (`host --stdio`),
//! loads the committed scene, clears the level it may already hold, paints the ground and two
//! platforms with `paint_tiles`, spawns a fixed box collider over each horizontal run of
//! painted cells, then saves the scene to its file. Every collider of the scene is the
//! level's. A refused command stops it with the game's own error before the scene file is
//! written.
//!
//! `build-level [<scene>]` builds the committed scene, or `<scene>`, whose prefabs and data
//! assets come from its project folder; `host --stdio` is how it starts itself as the game.

use std::collections::BTreeMap;
use std::fs;
use std::io::{BufRead, BufReader, Lines, Write};
use std::path::Path;
use std::process::{Child, ChildStdin, ChildStdout, Command as Process, ExitCode, Stdio};

use ludenea::app::project::project_assets;
use ludenea::control::{
    ClientMessage, Command, ComponentValue, Document, PROTOCOL_FINGERPRINT, PROTOCOL_VERSION,
    Reply, ServerMessage, TileArea,
};
use ludenea::prelude::*;
use serde::de::DeserializeOwned;

/// The tilemap the level is painted on, as the scene declares it.
const WIDTH: u32 = 32;
const HEIGHT: u32 = 18;

/// The town tileset's frames (`assets/art/town/tileset.png`, 37 frames a row): the earth the
/// ground is made of, its top row, and a wooden platform's ends and middle.
const EARTH: [u32; 2] = [390, 391];
const EARTH_TOP: [u32; 2] = [353, 354];
const PLATFORM_LEFT: u32 = 269;
const PLATFORM_MIDDLE: u32 = 270;
const PLATFORM_RIGHT: u32 = 271;

/// The ground fills rows 0 to 3, so its top is 4 cells above the tilemap's bottom.
const GROUND_ROWS: u32 = 4;
/// The platforms' row: their underside is 2 cells above the ground, a fighter's height, so a
/// fighter walks under them; their top is 3 cells above it, within a jump's reach (3.24 cells
/// with the fighter's jump speed and gravity).
const PLATFORM_ROW: u32 = GROUND_ROWS + 2;
/// The first column of each platform; each is 5 cells wide, mirrored about the centre.
const PLATFORM_COLUMNS: [u32; 2] = [4, WIDTH - 4 - PLATFORM_WIDTH];
const PLATFORM_WIDTH: u32 = 5;

fn main() -> ExitCode {
    let arguments: Vec<String> = std::env::args().skip(1).collect();
    if arguments.first().map(String::as_str) == Some("host") {
        // The game side: the arena's command line serves the control protocol.
        return ludenea::cli::main(arena::app);
    }
    let committed = concat!(env!("CARGO_MANIFEST_DIR"), "/assets/arena.scene.ron");
    let scene = match arguments.as_slice() {
        [] => Path::new(committed),
        [scene] => Path::new(scene),
        _ => {
            eprintln!("usage: build-level [<scene>]");
            return ExitCode::from(2);
        }
    };
    match build(scene) {
        Ok(()) => {
            println!("wrote {}", scene.display());
            ExitCode::SUCCESS
        }
        Err(message) => {
            eprintln!("error: {message}");
            ExitCode::FAILURE
        }
    }
}

fn build(scene_path: &Path) -> Result<(), String> {
    let scene = fs::read_to_string(scene_path)
        .map_err(|error| format!("cannot read {}: {error}", scene_path.display()))?;
    let assets = project_assets(scene_path, None)
        .map_err(|error| format!("cannot read the scene's assets: {error}"))?;
    let mut game = Game::start()?;
    let built = game.build_level(scene_path, scene, assets.texts.into_iter().collect());
    let ended = game.end();
    built.and(ended)
}

/// The game, run as a child process speaking the control protocol.
struct Game {
    child: Child,
    input: Option<ChildStdin>,
    output: Lines<BufReader<ChildStdout>>,
    next_id: u64,
}

impl Game {
    /// Starts this program as the game (`host --stdio`) and opens the connection.
    fn start() -> Result<Game, String> {
        let program = std::env::current_exe()
            .map_err(|error| format!("cannot find this program to start the game: {error}"))?;
        let mut child = Process::new(program)
            .args(["host", "--stdio"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .map_err(|error| format!("cannot start the game: {error}"))?;
        let (Some(input), Some(output)) = (child.stdin.take(), child.stdout.take()) else {
            return Err("the game started without its standard input and output".into());
        };
        let mut game = Game {
            child,
            input: Some(input),
            output: BufReader::new(output).lines(),
            next_id: 1,
        };
        game.send(&ClientMessage::Hello {
            protocol_version: PROTOCOL_VERSION,
            protocol_fingerprint: PROTOCOL_FINGERPRINT,
        })?;
        match game.receive(|message| !matches!(message, ServerMessage::Event { .. }))? {
            ServerMessage::Welcome { .. } => Ok(game),
            ServerMessage::Refused { error } => Err(format!(
                "the game refused the connection: {}",
                error.message
            )),
            other => Err(format!("the game answered the hello with {other:?}")),
        }
    }

    /// Loads `scene`, builds the level in it, and saves the scene to `scene_path`.
    fn build_level(
        &mut self,
        scene_path: &Path,
        scene: String,
        assets: Vec<(ludenea::ecs::AssetId, String)>,
    ) -> Result<(), String> {
        let Reply::Loaded { warnings, .. } = self.request(Command::LoadScene {
            scene,
            assets,
            document: Document::Scene,
        })?
        else {
            return Err("the game answered the scene load with another reply".into());
        };
        if !warnings.is_empty() {
            return Err(format!(
                "the scene loads with warnings: {}",
                warnings.join("; ")
            ));
        }
        // A level built before is cleared, so the level can be built again from any state:
        // in one batch, since a scene edit can renumber the entities after the ones it
        // removes.
        let colliders = self.entities("With<ludenea::Collider>")?;
        let despawns = colliders
            .into_iter()
            .map(|entity| Command::Despawn { entity })
            .collect();
        self.request(Command::Batch { commands: despawns })?;

        let tilemap = self.only_entity("With<ludenea::Tilemap>")?;
        let (pose, map): (Pose, Tilemap) = (
            self.component(tilemap, "ludenea::Pose")?,
            self.component(tilemap, "ludenea::Tilemap")?,
        );
        if (map.width, map.height) != (WIDTH, HEIGHT) {
            return Err(format!(
                "the level is drawn for a {WIDTH} by {HEIGHT} tilemap, but the scene's is {} by {}",
                map.width, map.height
            ));
        }
        self.paint(tilemap, area(0, 0, WIDTH, HEIGHT), None)?;

        for (rows, tiles) in [
            (0..GROUND_ROWS - 1, EARTH),
            (GROUND_ROWS - 1..GROUND_ROWS, EARTH_TOP),
        ] {
            let height = rows.end - rows.start;
            self.paint(tilemap, area(0, rows.start, WIDTH, height), Some(tiles[0]))?;
            for column in (1..WIDTH).step_by(2) {
                self.paint(tilemap, area(column, rows.start, 1, height), Some(tiles[1]))?;
            }
        }
        for first in PLATFORM_COLUMNS {
            let last = first + PLATFORM_WIDTH - 1;
            self.paint(
                tilemap,
                area(first, PLATFORM_ROW, 1, 1),
                Some(PLATFORM_LEFT),
            )?;
            let middle = area(first + 1, PLATFORM_ROW, PLATFORM_WIDTH - 2, 1);
            self.paint(tilemap, middle, Some(PLATFORM_MIDDLE))?;
            self.paint(
                tilemap,
                area(last, PLATFORM_ROW, 1, 1),
                Some(PLATFORM_RIGHT),
            )?;
        }

        // The colliders follow what the game holds now, not what was meant to be painted.
        let painted: Tilemap = self.component(tilemap, "ludenea::Tilemap")?;
        for (column, row, width) in tile_runs(&painted) {
            // The run's cells, in world units from the tilemap's bottom-left corner.
            let size = painted.cell_size;
            let half = Vec2::new(width as f32 * size.x / 2.0, size.y / 2.0);
            let corner = Vec2::new(column as f32 * size.x, row as f32 * size.y);
            let centre = pose.position + corner + half;
            let components = BTreeMap::from([
                (
                    "ludenea::Pose".to_string(),
                    json(&Pose {
                        position: centre,
                        rotation: Rot::IDENTITY,
                    })?,
                ),
                ("ludenea::Body".to_string(), json(&Body::fixed())?),
                (
                    "ludenea::Collider".to_string(),
                    json(&Collider::new(Shape::Polygon(Polygon::make_box(
                        half.x, half.y,
                    ))))?,
                ),
            ]);
            self.request(Command::Spawn { components })?;
        }

        let Reply::Scene { ron } = self.request(Command::SaveScene)? else {
            return Err("the game answered the scene save with another reply".into());
        };
        fs::write(scene_path, &ron)
            .map_err(|error| format!("cannot write {}: {error}", scene_path.display()))?;
        self.request(Command::MarkSceneSaved { text: ron })
            .map(drop)
    }

    fn paint(&mut self, entity: Entity, area: TileArea, tile: Option<u32>) -> Result<(), String> {
        self.request(Command::PaintTiles { entity, area, tile })
            .map(drop)
    }

    /// Every entity `filter` matches, in index order.
    fn entities(&mut self, filter: &str) -> Result<Vec<Entity>, String> {
        let Reply::Entities { rows, total } = self.request(Command::QueryEntities {
            filter: filter.to_string(),
            offset: 0,
            limit: u32::MAX,
        })?
        else {
            return Err(format!(
                "the game answered the query `{filter}` with another reply"
            ));
        };
        if usize::try_from(total).ok() != Some(rows.len()) {
            return Err(format!(
                "the query `{filter}` matches {total} entities but the game sent {}",
                rows.len()
            ));
        }
        Ok(rows.into_iter().map(|row| row.entity).collect())
    }

    fn only_entity(&mut self, filter: &str) -> Result<Entity, String> {
        match self.entities(filter)?.as_slice() {
            [entity] => Ok(*entity),
            found => Err(format!(
                "the scene must hold one entity matching `{filter}`, not {}",
                found.len()
            )),
        }
    }

    /// The value of `component` on `entity`.
    fn component<T: DeserializeOwned>(
        &mut self,
        entity: Entity,
        component: &str,
    ) -> Result<T, String> {
        let Reply::Entity { mut components, .. } = self.request(Command::GetEntity { entity })?
        else {
            return Err(format!(
                "the game answered the read of entity {entity} with another reply"
            ));
        };
        match components.remove(component) {
            Some(ComponentValue::Json { value }) => serde_json::from_value(value)
                .map_err(|error| format!("cannot read {component} of entity {entity}: {error}")),
            Some(ComponentValue::Ron { .. }) => Err(format!(
                "{component} of entity {entity} has no JSON form; the level cannot read it"
            )),
            None => Err(format!("entity {entity} has no {component}")),
        }
    }

    /// Sends `command` and returns the game's reply; a refusal is an error naming the command.
    fn request(&mut self, command: Command) -> Result<Reply, String> {
        let id = self.next_id;
        self.next_id += 1;
        let name = serde_json::to_value(&command)
            .ok()
            .and_then(|value| value.get("type")?.as_str().map(str::to_string))
            .unwrap_or_else(|| "command".to_string());
        self.send(&ClientMessage::Request { id, command })?;
        match self.receive(|message| matches!(message, ServerMessage::Response { id: answered, .. } if *answered == id))? {
            ServerMessage::Response { result, .. } => {
                result.map_err(|error| format!("the game refused `{name}`: {}", error.message))
            }
            other => Err(format!("the game answered `{name}` with {other:?}")),
        }
    }

    fn send(&mut self, message: &ClientMessage) -> Result<(), String> {
        let input = self
            .input
            .as_mut()
            .ok_or("the connection to the game is closed")?;
        let line = serde_json::to_string(message)
            .map_err(|error| format!("cannot write the message to the game: {error}"))?;
        writeln!(input, "{line}")
            .and_then(|()| input.flush())
            .map_err(|error| format!("cannot send to the game: {error}"))
    }

    /// The next protocol message that `wanted` accepts. Pushed events and what the game
    /// prints to its console (lines that are not protocol) are skipped, as a client reads
    /// past them; a `Refused` always ends the wait.
    fn receive(
        &mut self,
        wanted: impl Fn(&ServerMessage) -> bool,
    ) -> Result<ServerMessage, String> {
        loop {
            let line = match self.output.next() {
                Some(line) => {
                    line.map_err(|error| format!("cannot read from the game: {error}"))?
                }
                None => return Err("the game exited before it answered".into()),
            };
            if !line.starts_with(r#"{"type":"#) || line.starts_with(r#"{"type":"event""#) {
                continue;
            }
            let message: ServerMessage = serde_json::from_str(&line)
                .map_err(|error| format!("cannot read the game's message: {error}: {line}"))?;
            if wanted(&message) || matches!(message, ServerMessage::Refused { .. }) {
                return Ok(message);
            }
        }
    }

    /// Closes the connection, which ends the game, and waits for it to exit.
    fn end(mut self) -> Result<(), String> {
        self.input = None;
        let status = self
            .child
            .wait()
            .map_err(|error| format!("cannot wait for the game to exit: {error}"))?;
        if status.success() {
            Ok(())
        } else {
            Err(format!("the game exited with {status}"))
        }
    }
}

/// Each maximal horizontal run of painted cells of `map`, as its first column, its row, and
/// its width in cells, bottom row first.
fn tile_runs(map: &Tilemap) -> Vec<(u32, u32, u32)> {
    let mut runs = Vec::new();
    for row in 0..map.height {
        let mut column = 0;
        while column < map.width {
            let painted = |column: u32| {
                map.tiles
                    .get((row * map.width + column) as usize)
                    .is_some_and(Option::is_some)
            };
            if !painted(column) {
                column += 1;
                continue;
            }
            let first = column;
            while column < map.width && painted(column) {
                column += 1;
            }
            runs.push((first, row, column - first));
        }
    }
    runs
}

fn area(x: u32, y: u32, width: u32, height: u32) -> TileArea {
    TileArea {
        x,
        y,
        width,
        height,
    }
}

fn json(value: &impl serde::Serialize) -> Result<serde_json::Value, String> {
    serde_json::to_value(value).map_err(|error| format!("cannot write a component: {error}"))
}
