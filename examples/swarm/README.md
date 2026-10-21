# Swarm

The test game of the game runtime: 10,000 animated sprites circling on deterministic paths
(`Wanderer`) and four player sprites (`Player`) moved by local players. A player sprite whose
index is at or above the session's player count is hidden. Input: arrows and WASD, gamepad 0's
left stick and d-pad. The scene has a `background` layer (parallax 0.5) and a `world` layer,
seen by a 64 by 36 unit camera.

It is a Ludenea project (`ludenea.toml`). Its hash checks live in `tests/swarm.rs`; it has no
golden trace.

Draw one frame:

```sh
cargo run --locked --release -p swarm -- render examples/swarm/assets/swarm.scene.ron \
  --tick 60 --out swarm.png
```

## Regenerating the assets

The scene and the bug sprite sheet (flat shapes, four 16 by 16 frames, drawn by code) are
generated and committed. Running the generator twice writes identical bytes, and a test fails
when the committed files are stale:

```sh
cargo run --locked -p swarm --bin generate
```
