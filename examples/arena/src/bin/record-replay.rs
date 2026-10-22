//! Records a replay of the arena: 600 ticks of `arena::scripted_inputs` for two players on
//! the committed scene, written to the path given as the only argument.

use std::fs;
use std::path::Path;
use std::process::ExitCode;

use ludenea::app::edit::EditLog;
use ludenea::app::project::project_assets;
use ludenea::app::replay::record_replay;
use ludenea::prelude::InputLog;

const TICKS: u64 = 600;
const PLAYERS: u16 = 2;

fn main() -> ExitCode {
    let mut arguments = std::env::args().skip(1);
    let (Some(output), None) = (arguments.next(), arguments.next()) else {
        eprintln!("usage: record-replay <output.replay>");
        return ExitCode::from(2);
    };
    match record(Path::new(&output)) {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("error: {message}");
            ExitCode::FAILURE
        }
    }
}

fn record(output: &Path) -> Result<(), String> {
    let scene_path = Path::new(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/assets/arena.scene.ron"
    ));
    let scene = fs::read_to_string(scene_path)
        .map_err(|error| format!("cannot read {}: {error}", scene_path.display()))?;
    let assets = project_assets(scene_path, None)
        .map_err(|error| format!("cannot read the scene's assets: {error}"))?;
    let mut simulation = arena::app()
        .build()
        .map_err(|error| format!("the arena app does not build: {error}"))?;
    let mut inputs = InputLog::new(PLAYERS);
    for tick in 0..TICKS {
        inputs.record(&arena::scripted_inputs(tick));
    }
    let replay = record_replay(
        &mut simulation,
        &scene,
        &assets,
        &inputs,
        &EditLog::new(),
        TICKS,
    )
    .map_err(|error| format!("cannot record the replay: {error}"))?;
    fs::write(output, replay.encode())
        .map_err(|error| format!("cannot write {}: {error}", output.display()))
}
