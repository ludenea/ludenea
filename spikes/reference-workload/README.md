# Reference workload

Specification, version 1, of the synthetic simulation that the spikes of several research subjects (multithreading, determinism, the ECS, the runtime, memory, game testing, networking) measure against. One workload makes their numbers comparable: scheduling overhead, deterministic reductions, iteration throughput, structural change cost, allocation neutrality, divergence detection and lockstep all run the same ticks.

It is a spike fixture. It is not a prototype of the engine: it implies no entity component system design, no scheduler interface and no public API. An implementation is plain arrays and loops.

## What it is

A population of entities moves in a square world. Entities repel each other at short range, bounce on the walls, and a few are destroyed and created every tick. A hash of the complete state is computed at the end of every tick. Every operation is specified down to the order of floating-point operations, so that two correct implementations produce identical hashes on every platform where the `exact` facts of the spike `platform-facts` hold.

## Parameters

| Parameter              | Symbol   | Default                    | Notes                                                           |
| ---------------------- | -------- | -------------------------- | --------------------------------------------------------------- |
| Seed                   | `seed`   | `0x4c7564656e656121`       | Any 64-bit value                                                |
| Entity count           | `N`      | 10 000                     | Sizes: small 1 000, default 10 000, large 100 000               |
| Tick count             | `T`      | 1 000                      |                                                                 |
| Time step              | `dt`     | 2^-6 = 0.015625            | Exact in binary                                                 |
| World side             | `W`      | 1024                       | The world is [0, W) x [0, W)                                    |
| Cell size              | `C`      | 16                         | A grid of (W / C)^2 = 4096 cells                                |
| Interaction radius     | `r`      | 8                          | r <= C, so the 3 x 3 cell neighborhood suffices                 |
| Repulsion strength     | `k`      | 64                         |                                                                 |
| Speed limit            | `vmax`   | 64                         | Per component; vmax * dt <= 1 keeps one wall reflection enough  |
| Damping                | `damp`   | 1 - 2^-8 = 0.99609375      | Exact in binary                                                 |
| Kills per tick         | `D`      | 8                          |                                                                 |
| Spawns per tick        | `S`      | 8                          |                                                                 |
| Reduction chunk        | `K`      | 256                        | Slots per chunk in the energy reduction                         |
| Wander system          | `wander` | off                        | On enables the math-library-dependent system                    |

All arithmetic on state is IEEE 754 binary32 (`f32`), round to nearest even, no flush to zero, no fused multiply-add, no reassociation. Every constant above is exactly representable in `f32`. Version 1 has no `f64` variant; adding one is a new version.

## State

Slots indexed from 0 to N - 1. A slot holds:

- `generation`: unsigned 32-bit, starts at 0, incremented each time the slot is populated again.
- `alive`: boolean.
- `x`, `y`, `vx`, `vy`: `f32`.
- `mass`: `f32`, one of 1, 2, 4.

Two globals: the pseudo-random generator state `rng` (unsigned 64-bit) and the tick's `energy` (`f32`). An entity is identified by (slot index, generation).

## Pseudo-random numbers

One global SplitMix64 stream, all integer arithmetic wrapping:

```text
next():
  state = state + 0x9e3779b97f4a7c15
  z = state
  z = (z xor (z >> 30)) * 0xbf58476d1ce4e5b9
  z = (z xor (z >> 27)) * 0x94d049bb133111eb
  return z xor (z >> 31)

u01():  (next() >> 40) converted to f32, times 2^-24      // in [0, 1), exact
mass(): one of [1, 2, 4] indexed by next() mod 3
```

## Initialization

`rng = seed`. For each slot `i` from 0 to N - 1, in order, draw in this order:

```text
x    = u01() * W
y    = u01() * W
vx   = (u01() * 2 - 1) * 32
vy   = (u01() * 2 - 1) * 32
mass = mass()
generation = 0, alive = true
```

Every operation above is exact. `energy = 0`. The hash of the initial state is `h_0`.

## One tick

Systems run in this order. Each system reads the state produced by the previous one. Implementations may run the work inside a system in parallel when the result is unchanged; the conditions are stated per system.

### 1. Spatial hashing

Each alive slot is assigned to the cell `(cx, cy) = (floor(x / C), floor(y / C))` (exact). Each cell lists its slots in increasing index order. Parallel-safe: the lists depend on positions only.

### 2. Repulsion

For each alive slot `i`, with position `(xi, yi)` and mass `mi`, start with `fx = 0`, `fy = 0`. Visit the cells `(cx + a, cy + b)` for `b` in -1, 0, 1 (outer loop) and `a` in -1, 0, 1 (inner loop), skipping cells outside the grid. In each cell visit the alive slots `j` other than `i`, in increasing index order:

```text
dx = xj - xi
dy = yj - yi
d2 = (dx * dx) + (dy * dy)        // two products rounded separately, then one addition
if d2 < r * r and d2 > 0:
  d  = sqrt(d2)
  s  = (k * (r - d)) / d
  fx = fx - (dx * s)
  fy = fy - (dy * s)
```

Then, for slot `i`:

```text
ax = fx / mi
ay = fy / mi
vx = clamp((vx + (ax * dt)) * damp, -vmax, vmax)
vy = clamp((vy + (ay * dt)) * damp, -vmax, vmax)
```

The system reads positions and writes velocities only, and each slot's sum is independent, so running slots in parallel is deterministic provided the visit order inside each slot is kept. `d2` is a textbook contraction candidate: an implementation whose compiler fuses it into a fused multiply-add produces different hashes, which is one of the facts the workload exists to expose.

### 3. Wander (only when `wander` is on)

```text
theta = ((t + i) mod 4096) converted to f32, times 2^-6     // t is the tick number, from 1
vx = clamp(vx + (cos(theta) * 0.25), -vmax, vmax)
vy = clamp(vy + (sin(theta) * 0.25), -vmax, vmax)
```

`cos` and `sin` are the platform's `f32` functions. This system isolates math library variance so that it can be measured on a realistic workload without contaminating the core.

### 4. Integration

```text
x = x + (vx * dt)
y = y + (vy * dt)
```

### 5. Boundary

For each axis, with `x` as the example:

```text
if x < 0:   x = -x;        vx = -vx
if x >= W:  x = (2 * W) - x; vx = -vx
if x >= W:  x = W - 0.0625
```

Since the displacement per tick is at most `vmax * dt = 1`, one reflection suffices; the last line handles the case `x == W` exactly, so that spatial hashing stays inside the grid.

### 6. Energy

For each alive slot, `e_i = ((vx * vx) + (vy * vy)) * (0.5 * mass)`. Chunk `c` covers the slots from `c * K` to `(c + 1) * K - 1`; the chunk sum adds the `e_i` of its alive slots in increasing index order; `energy` adds the chunk sums in increasing chunk order. Running chunks in parallel is deterministic by construction. This is the reduction whose deterministic cost the multithreading and determinism research measure.

### 7. Structural changes

From the global stream, in this order:

1. `D` kills: `j = next() mod N`; if slot `j` is alive it becomes dead. A dead slot drawn again stays dead; there is no redraw.
2. `S` spawns: find the lowest-index dead slot; if none exists the spawn is skipped and nothing is drawn; otherwise draw `x`, `y`, `vx`, `vy`, `mass` exactly as in initialization, increment `generation`, set `alive`.

Entities spawned at tick `t` take part from tick `t + 1`.

### 8. Hash

`h_t` is FNV-1a 64 over these bytes, little-endian: for each slot in index order, `generation` (4 bytes), `alive` (1 byte, 1 or 0), and when alive `x`, `y`, `vx`, `vy`, `mass` (4 bytes each); then `energy` (4 bytes); then `rng` (8 bytes).

```text
fnv1a(bytes): hash = 0xcbf29ce484222325; for each byte: hash = (hash xor byte) * 0x100000001b3 (wrapping)
```

The run hash `H` is FNV-1a 64 over `h_0` to `h_T`, each as 8 little-endian bytes. Two runs are compared by `H` first and, when they differ, by the first `t` whose `h_t` differs.

## What a spike reports

- The workload version, the parameters (size, `T`, `seed`, `wander`) and the number of workers or any other variable of the experiment.
- `H`, `h_T` and, when comparing runs, the first divergent tick.
- The wall time of the `T` ticks after 10 warm-up ticks, as ticks per second, and per-system times when the spike's question needs them.
- The toolchain and the cell, which the research CI records in `environment.txt`.

## Reference values

The sequential reference implementation (Rust, standard library only) lives in this directory once the first spike that needs the workload writes it. The hashes it produces on the five cells of the research CI are then recorded in `reference-hashes.md`, per workload version and toolchain version. Until then, a spike compares its own runs with one another (worker counts, cells), which is the comparison the tracks need.

## Versioning

Version 1, specified on 2026-11-02. Any change to this specification, parameter defaults included, bumps the version. A spike records the version it measured; results of different versions are not compared.

## Rationale

- Power-of-two constants and a 24-bit uniform keep initialization exact, so initial states agree everywhere.
- The core needs addition, subtraction, multiplication, division and square root only: operations IEEE 754 defines exactly. The core is reproducible wherever the `exact` facts hold, and the contraction candidate in the repulsion detects a compiler that fuses.
- Per-slot accumulation in a fixed visit order and the chunked energy reduction make a parallel implementation deterministic by construction: the cost of that construction is what the multithreading and determinism research measure against a free-running baseline.
- Kills and spawns give the ECS research structural changes with identifier reuse and generations.
- The optional wander system isolates math library variance for the determinism and cross-platform compatibility research.
