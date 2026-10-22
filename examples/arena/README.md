# Arena

A platform arena where local players run and jump as LuizMelo fighters through a town: the
warrior for player 0, the hero for player 1. On the first tick, one fighter spawns per player
present at that player's `SpawnPoint`; the camera then frames them all, its view their
bounding box plus 6 units, at least 24 by 13.5.

Input: arrows or A and D run; `Space`, `KeyW` or `ArrowUp` jump; on a gamepad, the left
stick and the d-pad run and button 0 jumps.

The fighters are kinematic bodies moved by the game, not by the physics solver: after each
physics step, a downward raycast from the centre of a fighter's capsule finds the ground and
where a fall lands, and sideways raycasts at three heights (the centres of the capsule's
ends and its middle) stop a run at an obstacle. Colliders are one-way upward: a fighter
rises through any collider and stands only on a surface its feet are on or above, so a jump
that does not clear a platform drops back through it. Between those three heights, an
obstacle thinner than the gap between two rays (0.6 units) does not stop a run. A fighter
whose feet fall below `KILL_Y` (y = -19, 10 units under the bottom of the scene's tilemap,
which spans y -9 to 9) returns to its spawn point at rest, or to the first spawn point when
its own is gone; with no spawn point left, it is removed. The scene's `arena::FighterSheets`
resource names the fighters' sprite sheets, so loading the scene loads them: any arena scene
must list it, or the fighters spawn without art.

The scene (`assets/arena.scene.ron`) has four layers: `sky` (parallax 0.1) and `town` (0.4)
hold the town backgrounds, `tiles` holds a 32 by 18 tilemap on the town tileset, and
`fighters` holds the fighters. The art and its licenses are listed in `CREDITS.md`. The game
registers `FighterSheets` empty, since a control host starts on the built world, which holds
no asset, before it loads a scene.

It is a Ludenea project (`ludenea.toml`). Its tests live in `tests/`.

Play it:

```sh
cargo run --locked --release -p arena -- play examples/arena/assets/arena.scene.ron
```

Draw tick 300 of the golden replay to a PNG, offscreen, with the textures of this folder (a
replay holds no PNG):

```sh
cargo run --locked --release -p arena -- render golden/arena.replay --project examples/arena \
  --tick 300 --out arena.png
```

Record a replay of 600 ticks of scripted input for two players:

```sh
cargo run --locked --release -p arena --bin record-replay -- arena.replay
```

## The level

No one painted the level by hand: `build-level` built it through the control protocol, the
way the editor's tile palette and inspector do. It starts the arena serving the protocol on
its standard input and output (`host --stdio`), loads the scene, and sends commands only:
`paint_tiles` for a ground of town earth across the tilemap, its top at y -5, and two wooden
platforms of 5 cells with their top at y -2 (their underside a fighter's height above the
ground, their top within a jump's reach); then a `spawn` of a fixed box collider over each
horizontal run of painted cells; then `save_scene`, whose text it writes to the scene's file.
A refused command stops it before the file is written. It clears the colliders and tiles the
scene holds first, so it rebuilds the level from any state; rebuilt from a built level, the
colliders get new scene identifiers but the simulation is the same. Build the level again
after changing the tool:

```sh
cargo run --locked -p arena --bin build-level
```

The golden replay `golden/arena.replay` records 600 ticks of `scripted_inputs` on this scene;
record it again after a deliberate change to the level or the game (see the repository's
`README.md`).
