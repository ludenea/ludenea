# Rapier's worlds with its platform math calls routed to the pure Rust library

Throwaway spike of the physics research. The spike [`rapier-platform-math-calls`](../rapier-platform-math-calls/) found five calls Rapier 3D 0.36.1 makes to the platform's math library on the path of a step with its `enhanced-determinism` feature on, and worlds that reach them hashing differently from one cell to the next. This spike measures whether those five calls are the whole cause: it builds the same worlds against copies of the three libraries in which the five calls go to the pure Rust library the feature already uses, and compares their hashes across the cells.

It is evidence, never product code. The patches exist for this measurement; what to make of the libraries is not this spike's decision.

## Layout

- `patches/`: one patch per library, the whole of what the spike changes in them.
- `setup.sh`: fetches the three libraries as published, verifies them, applies the patches under `patched/` (ignored by Git), and installs GDB on a Linux machine of the research CI.
- `Cargo.toml`, `Cargo.lock`: a workspace of its own that replaces the three libraries with the patched copies.
- `src/main.rs`: the report.

The worlds and the count of the calls are the earlier spike's source files (`scenes.rs`, `reach.rs`), compiled here by path against the patched libraries: the same source, so the same worlds and the same breakpoints.

## Hypotheses

Stated before the spike ran on the cells.

1. **The worlds.** With the five calls routed, each of the five worlds and of the two twins hashes to one value on the five cells, before the first step and at every checkpoint, and so does the inertia its bodies are given. Refuted, per world, by two cells with two hashes.
2. **The versions of one platform's library.** On one machine, the containers with glibc 2.36 and with glibc 2.41, which gave two worlds with the libraries as published, give the same hashes, and those of the cells. Refuted by one world with two hashes.
3. **The reach.** No world, while it is built or while it steps, calls a function of the platform's math library, and the lines of the sources that held the calls are still met. Refuted by one call counted.

A fact to record, with no threshold: whether the worlds that hashed alike with the libraries as published (the control and the twins) hash as they did, and which cells' results with the libraries as published, if any, each routed world ends on.

## Method

**The patches.** In the three libraries the trait that carries the deterministic functions is imported only in builds without the standard library. In a build with it, which is the default, a function written as a method on a plain `f32` is the standard library's, which hands it to the platform. Each patch imports the trait in every build in the file that holds a call, and writes the call through the trait, which the determinism feature routes to `libm`:

| Library | File | Call | Routed form |
| --- | --- | --- | --- |
| parry3d 0.31.1 | `src/partitioning/bvh/bvh_optimize.rs` | two base-2 logarithms of a count | `ComplexField::log2(...)` |
| rapier3d 0.36.1 | `src/dynamics/solver/joint_constraint/joint_constraint_helper.rs` | the sine and cosine of half the center of an angular limit | `ComplexField::sin_cos(center * 0.5)` |
| parry3d 0.31.1 | `src/shape/convex_polyhedron.rs` | the sine and cosine of one degree | `ComplexField::sin_cos(eps)` |
| rapier3d 0.36.1 | `src/dynamics/solver/joint_constraint/generic_joint_constraint_builder.rs` | the sine of half a motor's target angle | `ComplexField::sin(motor_params.target_pos / 2.0)` |
| glamx 0.3.1 | `src/eigen3.rs` | an arc cosine and two cosines in the eigenvalues of a symmetric matrix | `<$Real as ComplexField>::acos(r)`, `::cos(...)` |

A patch replaces lines one for one, so every line of the sources keeps its number and the earlier spike's line breakpoints still name the same code. The forms of two of the calls that a 2D build compiles, on the neighbouring lines, are not touched: the spike does not build them. Nothing else in the libraries is changed.

**The build.** `setup.sh` downloads the three archives the registry publishes, verifies each against its SHA-256, which is the checksum the spikes' workspace records in its lock file for the same version, extracts them under `patched/`, applies the patches and checks that each call is there in its routed form. The spike's manifest replaces the three libraries with those copies. That replacement holds for every package of a workspace, so the spike is a workspace of its own, outside the spikes' workspace, with its own lock file: besides the spike itself, 62 packages, each at the version the spikes' workspace locks, the three libraries taken from a path. The build is locked to that file, at the pinned toolchain. The program reports the hash of the text of each patch it was built with and whether its lock file takes the three libraries from a path, so that two cells are known to have built the same thing.

**The worlds** are the earlier spike's five, with the two twins: every parameter drawn from a seeded stream with exact arithmetic, 600 ticks of one sixtieth of a second on the calling thread, sleeping off, hashed (the transform and the motion of every body, in order) before the first step and after ticks 1, 2, 5, 10, 30, 100, 300 and 600, with the hash of every body's principal inertia and inertia frame after the build and after the steps. Beside each world the program prints what the libraries as published gave on the five cells, recorded from the earlier spike's second run: the hash before the first step, and each final hash with the cells that ended on it.

**The reach**, on Linux, with GDB, as in the earlier spike: each world is run again for 320 ticks under the debugger with a breakpoint on each of the 28 functions of the platform's math library a method on an `f32` or an `f64` can reach, set at the address the program itself calls, and on the six lines of the sources that held the calls. The hits before the marker a world calls when its build ends are the build's, the hits after it the steps'.

`cargo test --manifest-path rapier-platform-math-routed/Cargo.toml` runs the earlier spike's checks of its worlds and of its reading of the debugger's output against the patched libraries, and verifies that every world has its record of what the published libraries gave, that a patch hashes alike with either line ending, and that the build is locked to the patched copies. `--scene <name> [ticks]` steps one world and prints its hashes.

Its limits, stated before it ran: those of the earlier spike (the worlds are synthetic and small; a hash that is equal on two cells says nothing of inputs the world did not take; the reach is counted on Linux only; a result holds for these versions of the libraries and this toolchain). The patches route the five calls these worlds reach and no other: a search finds 18 source files of the three libraries (4 of rapier3d, 10 of parry3d, 4 of glamx) that import the trait only without the standard library, and the spike did not read each of them for what it calls on a plain `f32`. What the routed calls cost in time is not measured.

## Environment

Research CI, workflow `research-ci.yml`, five cells (see [`spikes/README.md`](../README.md)), run on demand with `-f spike=rapier-platform-math-routed`; a developer machine (AMD Ryzen 9 9950X3D, Windows 11) natively and, under its Linux subsystem, in the `rust:1.99.1-slim-trixie` and `rust:1.99.1-slim-bookworm` containers (glibc 2.41 and 2.36). Release builds at the pinned Rust toolchain.

Relationship to the external components: Rapier 3D 0.36.1 (Apache 2.0) with its `enhanced-determinism` feature, parry3d 0.31.1 (Apache 2.0) and glamx 0.3.1 (MIT or Apache 2.0) are the subjects of the measurement, each built from its published archive with this spike's patch applied; `libm` 0.2.16 (MIT) is where the patched calls go, as the feature's other calls do; GDB (GPL 3.0) is run as a tool on the Linux cells. Their code is not in this repository beyond the lines the patches replace and their context: the archives are fetched when the spike is built. Nothing of them is reused beyond this spike.

## Result

Measured on 2026-11-13: a [first run](https://github.com/ludenea/ludenea/actions/runs/43489738477) of the research CI on the five cells (hosts: AMD EPYC 7763, Arm Neoverse N2, AMD EPYC 9V74, Cobalt 100, a virtual Apple M1; glibc 2.39 on the two Linux cells, the Universal C Runtime of Windows Server 2025 and of Windows 11, the system library of macOS 15.8.2); a [second run](https://github.com/ludenea/ludenea/actions/runs/43490503725), whose two x86_64 cells exchanged those hosts and whose five cells repeat the first run's hashes and counts exactly; and the developer machine natively and in the two containers. The raw measurements and the debugger's output are in the runs' artifacts.

Every machine built the same thing: the hash of each patch's text is one value on the five cells and on the developer machine, and every lock file takes the three libraries from the patched copies.

1. **The worlds: holds for all seven.** Each world has one hash before the first step, one at each of the eight ticks hashed and one inertia hash after the build and after the steps, on the five cells, on the developer machine natively and in its two containers.

   | World | Distinct hashes before the first step, at each tick hashed, of the inertia | Final hash, on every machine | With the libraries as published |
   | --- | --- | --- | --- |
   | `control` | 1, 1, 1 | `978720a6934c0050` | this hash, on the five cells |
   | `joint_limits` | 1, 1, 1 | `b7aa90e8b7a11b06` | two hashes; this one on Windows x86_64 and macOS |
   | `joint_limits`, the twin without limits | 1, 1, 1 | `a890594c53a00ad1` | this hash, on the five cells |
   | `motor_targets` | 1, 1, 1 | `7c60329141664112` | three hashes; this one on Windows x86_64 |
   | `motor_targets`, the twin without motors | 1, 1, 1 | `4f5d5c00da016a20` | this hash, on the five cells |
   | `several_colliders` | 1, 1, 1 | `72fd85fa0af432cd` | three hashes, none of them this one |
   | `balls_on_hulls` | 1, 1, 1 | `3727c16910e54471` | three hashes; this one on the two Windows cells |

   - Every world starts from the hash it started from with the libraries as published, and on every machine the limits and the motors still change what their worlds do.
   - The worlds that held with the libraries as published, the control and the two twins, hash as they did: routing the logarithms of the broad phase's tree changes nothing in them.
   - A routed world is the world of the cells whose library returned, on the inputs the world took, what the pure Rust library returns: Windows on x86_64 and macOS for the limits, Windows on x86_64 for the motors' sines, the two Windows cells for the hulls. For the bodies of several colliders no cell's library did, and the routed world is a new one.
2. **The versions of one platform's library: holds.** The containers with glibc 2.36 and with glibc 2.41 give the hashes of the cells for every world, the hulls and the bodies of several colliders included, which they gave two worlds for with the libraries as published.
3. **The reach: holds.** Counted on the two Linux cells and in the two containers over 320 ticks, the same on the four: none of the 28 functions of the platform's math library is hit while any of the five worlds is built or steps. The lines that held the calls are still met during the steps:

   | World | Lines of the sources met during the steps |
   | --- | --- |
   | `control` | the tree's optimization 317 |
   | `joint_limits` | the limit's half center 64, the tree's optimization 72 |
   | `motor_targets` | the motor's half target 40 960, the tree's optimization 306 |
   | `several_colliders` | the eigenvalues' arc cosine 192 and cosines 192, the tree's optimization 175 |
   | `balls_on_hulls` | the eigenvalues' arc cosine 768 and cosines 768, the one-degree tolerance 8 561, the tree's optimization 308 |

   The limit's, the motor's and the eigenvalues' lines are met as often as the earlier spike counted their calls. The line of the tree's optimization is met about twice as often as in the earlier spike, where it held two calls: a line's breakpoint counts every place the compiler put the line's code, and why it put this line's in more places here was not traced. The one-degree line is met 8 561 times against 8 661: on Linux the hulls' world is no longer the same world.

**What it says and does not say.** On these worlds the five calls are all that separated the cells: with them routed to the pure Rust library, a world with asymmetric joint limits, with motor targets on a joint that hangs from a multibody, with bodies of several colliders or with convex hulls hashes to one value on the five cells, on a sixth machine and under two versions of one platform's library, and its steps make no call to the platform's math library. The change is 14 lines in 5 files of three crates. Not measured: any world beyond these five (other joint kinds, a limit centered on zero, the soft bodies, the mesh intersection code, whose own methods on a plain `f32` the patches leave as published); the 2D build; what the routed calls cost in time; the reach on Windows and macOS, the sources being the same there; an x86_64 processor that is not AMD's, the two x86_64 cells and the developer machine having landed on three AMD models. Whether the libraries' authors would take such a change, and what a project that depends on the libraries should do meanwhile, are not this spike's to say.
