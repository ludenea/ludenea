# Arena

A platform arena where local players run and jump as LuizMelo fighters through a town: the
warrior for player 0, the hero for player 1. On the first tick, one fighter spawns per player
present at that player's `SpawnPoint`; the camera then frames them all, its view their
bounding box plus 6 units, at least 24 by 13.5.

Input: arrows or A and D run; `Space`, `KeyW` or `ArrowUp` jump; on a gamepad, the left
stick and the d-pad run and button 0 jumps.

The fighters are kinematic bodies moved by the game, not by the physics solver: after each
physics step, a downward raycast from a fighter's capsule finds the ground and where a fall
lands, and a sideways one stops a run at a wall. The scene's `arena::FighterSheets` resource
names the fighters' sprite sheets, so loading the scene loads them.

The scene (`assets/arena.scene.ron`) has four layers: `sky` (parallax 0.1) and `town` (0.4)
hold the town backgrounds, `tiles` holds a 32 by 18 tilemap on the town tileset, and
`fighters` holds the fighters. The art and its licenses are listed in `CREDITS.md`.

It is a Ludenea project (`ludenea.toml`). Its tests live in `tests/`.

Play it:

```sh
cargo run --locked --release -p arena -- play examples/arena/assets/arena.scene.ron
```

Record a replay of 600 ticks of scripted input for two players:

```sh
cargo run --locked --release -p arena --bin record-replay -- arena.replay
```
