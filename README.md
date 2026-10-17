# Ludenea

Ludenea is a deterministic 2D game engine written in Rust. It combines an
entity component system, a Box2D-style physics engine, a versioned control
protocol, and a command line tool, all built so that a simulation can be
recorded, replayed, and verified bit for bit.

## Determinism guarantee

The same scene, the same input log, and the same engine version produce the
same state hash at every tick, on macOS, Linux, Windows, and WebAssembly, and
for any thread count. The guarantee is enforced by:

- a golden trace of 10,001 state hashes, checked in continuous integration on
  every platform at 1, 2, and 8 threads and under `wasmtime`;
- lints that forbid platform-dependent floating point functions, wall-clock
  time, and unordered collections in simulation code;
- a pinned toolchain and a committed `Cargo.lock`.

## Workspace layout

| Path                      | Purpose                                        |
| ------------------------- | ---------------------------------------------- |
| `crates/ludenea-math`     | Deterministic vectors, matrices, and functions |
| `crates/ludenea-ecs`      | World, components, queries, and state hashing  |
| `crates/ludenea-macros`   | Derive macros                                  |
| `crates/ludenea-app`      | Simulation loop, scenes, snapshots, input logs |
| `crates/ludenea-physics`  | Rigid body physics                             |
| `crates/ludenea-control`  | Versioned control protocol and host            |
| `crates/ludenea-cli`      | `run`, `verify`, and `diff` commands           |
| `crates/ludenea`          | Facade crate re-exporting the engine           |
| `examples/boxes`          | Reference scenario, golden trace, and benchmark |
| `golden`                  | Golden hash traces                             |

## Commands

Run the tests:

```sh
cargo test --locked --workspace
```

Verify the golden trace natively (use `--threads 1`, `2`, or `8`):

```sh
cargo run --locked --release -p boxes -- verify examples/boxes/assets/boxes.scene.ron \
  --inputs examples/boxes/assets/boxes.inputs --golden golden/boxes.trace --threads 8
```

Verify the golden trace under WebAssembly:

```sh
cargo build --locked --release --target wasm32-wasip1 -p boxes --bin boxes
wasmtime run --dir=. target/wasm32-wasip1/release/boxes.wasm verify \
  examples/boxes/assets/boxes.scene.ron --inputs examples/boxes/assets/boxes.inputs \
  --golden golden/boxes.trace
```

Run the benchmark:

```sh
cargo run --locked --release -p boxes --bin bench
```
