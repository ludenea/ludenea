# Rapier's calls to the platform's math library

Throwaway spike of the physics research. With its `enhanced-determinism` feature on, Rapier 3D 0.36.1 routes most of its mathematical functions to a library written in Rust, the same computation on every platform. A reading of its sources and of its collision library's found calls the feature does not reach: methods called on a plain `f32`, which the standard library hands to the platform's math library. Five of them are on the path of a simulation step. This spike measures whether the platforms differ on those calls at all, which is what a decision on the library's place in a deterministic simulation rests on; that decision is not this spike's.

It is evidence, never product code. Rapier, its collision library and the pure Rust math library are studied here and integrated nowhere.

## Layout

- `src/functions.rs`: the platform's functions themselves, swept against the pure Rust library.
- `src/scenes.rs`: the worlds, one per call, and a control.
- `src/reach.rs`: the calls a world's steps make, counted by a debugger on Linux.
- `src/main.rs`: the report.
- `setup.sh`: installs GDB on a Linux machine of the research CI.

## Hypotheses

The five calls, as read in the sources of rapier3d 0.36.1, parry3d 0.31.1 and glamx 0.3.1:

1. two base-2 logarithms of a count, in the incremental optimization of the broad phase's tree;
2. the sine and cosine of half the center of an angular joint limit;
3. the sine and cosine of one degree, in the feature a point projects to on a convex polyhedron;
4. the sine of half a motor's target angle, in the constraint of a joint with a multibody;
5. an arc cosine and two cosines in the eigenvalues of a symmetric matrix, when mass properties are summed and the inertia matrix is not diagonal.

Stated before the spike ran; none has a threshold, each is a fact to record.

1. **The functions.** On inputs spread over the ranges the calls take, the five cells return the same bits for the sine, the cosine, the two taken together, the arc cosine and the base-2 logarithm. Refuted by two cells that return different bits for one input.
2. **The worlds.** A world built to reach one of the calls with many inputs hashes, at the same ticks, to the same value on the five cells. Refuted, per world, by two cells with two hashes. A world with none of the features is the control and is expected to hash alike everywhere, as the worlds measured before did.
3. **The reach.** Each call is made while a world steps, as the reading says, and not only while it is built. Refuted, per call, by a world built for it whose steps make no such call.

## Method

**The functions.** Each function is called as the libraries call it, as a method on a plain `f32`, and beside it the same function of the pure Rust library the determinism feature routes the other calls to (`libm` 0.2.16). The reference is the same computation on every cell, so two cells return the same bits over a sweep exactly when their differences from the reference are the same. The sweeps: the sine, the cosine and the two together over every sixteenth `f32` of [2^-6, 4) in both signs (8 388 608 inputs each); the arc cosine over every sixteenth `f32` of [2^-8, 1) in both signs (8 388 608 inputs); the base-2 logarithm over every whole number from 1 to 2^20, which is what the tree's optimization takes it of. A sweep reports the hash of the platform's results, the hash of the reference's, the count of inputs on which they differ, the hash of those differences and the largest distance in units of the last place, and writes the first 20 000 differences to `functions/<name>.txt` in the run's output. The values the worlds hand to the platform by construction (the half centers of their 64 limits, the half targets of their 96 motors, one degree) are evaluated as the libraries take them: the sine and cosine together for a limit and for one degree, the sine alone for a motor.

**The worlds.** Five worlds, every parameter drawn from a seeded stream with exact arithmetic, stepped 600 ticks of one sixtieth of a second on the calling thread (no thread pool), sleeping off, and hashed (the transform and the motion of every body, in order) before the first step and after ticks 1, 2, 5, 10, 30, 100, 300 and 600:

- `control`: 64 cuboids and balls falling on a floor. No joint, no convex hull, one collider per body at the body's own frame.
- `joint_limits`: 32 pendulums of two links on revolute impulse joints, each of the 64 joints with its own asymmetric limits (a lower bound in [-1.2, -0.2), an upper bound in [0.1, 1.0), so that no center is zero), every second pendulum thrown upward so that both bounds are met.
- `motor_targets`: 32 arms of three links, the first two on revolute multibody joints and the third hung from the second by a revolute impulse joint, each of the 96 joints with a motor whose position target is its own, in [-1.4, 1.4).
- `several_colliders`: 48 bodies of three colliders (a cuboid, a ball, a capsule) placed apart and at angles, and 16 bodies of one cuboid placed at an angle, falling on a floor. Before tick 150 a third of the compound bodies gain a collider and before tick 300 another third change a collider's density, as a game does between two steps.
- `balls_on_hulls`: 40 balls and 20 convex hulls of 12 points falling on 8 fixed convex hulls of 24 points above a floor.

The two jointed worlds have a twin without the limits or without the motors: a twin that hashes differently from its world shows that the feature acted, and a twin that hashes alike on every cell shows that the cells part on the feature and on nothing else. Beside the hashes a world reports the hash of every body's principal inertia and inertia frame, which is what the eigenvalues give, after the build and after the steps, the count of bodies whose inertia frame is turned, and the most contact pairs it saw between a ball and a hull and between two hulls.

**The reach**, on Linux, with GDB. Each world is run again for 320 ticks under the debugger with a breakpoint on every function of the platform's math library a method on an `f32` or an `f64` can reach (28 functions), set at the address the program itself calls: several of them are chosen at load time among variants written for the processor at hand, and a breakpoint by name lands on the code that chooses. The program calls a marker when a world's build ends and its steps begin; the hits before it are the build's, the hits after it the steps', and the first hit of each function after the marker prints its callers. Six more breakpoints count how often the lines of the sources that hold the five calls are met, so that a line met with no call counted is a call the compiler computed when it built the program.

`cargo test -p rapier-platform-math-calls` verifies the sweeps on functions made by hand, that a world run twice gives the same hashes, that the limits and the motors change what their worlds do, that the compound bodies have a turned inertia frame and the control none, that balls come to touch hulls, and the reading of the debugger's output. `--scene <name> [ticks]` steps one world and prints its hashes.

Its limits, stated before it ran: the worlds are synthetic and small; a hash that is equal on two cells says nothing of inputs the world did not take; the reach is counted on Linux only, the libraries' sources being the same on every platform; the sweeps sample one input in sixteen; a result holds for these versions of the libraries, this toolchain, and the versions of the platforms' libraries the machines had.

## Environment

Research CI, workflow `research-ci.yml`, five cells (see [`spikes/README.md`](../README.md)), run on demand with `-f spike=rapier-platform-math-calls`; a developer machine (AMD Ryzen 9 9950X3D, Windows 11) natively and, under its Linux subsystem, in the `rust:1.99.1-slim-trixie` and `rust:1.99.1-slim-bookworm` containers. Release builds at the pinned Rust toolchain. The platforms' math libraries: glibc 2.39 on the two Linux cells (Ubuntu 24.04), glibc 2.41 and 2.36 in the two containers (Debian 13 and 12), the Universal C Runtime of Windows Server 2025 on the Windows x86_64 cell, of Windows 11 on the Windows ARM64 cell and on the developer machine, and the system library of macOS 15.8.2 on the macOS cell.

Relationship to the external components: Rapier 3D 0.36.1 (Apache 2.0) with its `enhanced-determinism` feature, and through it parry3d 0.31.1 and glamx 0.3.1, are the subjects of the measurement; `libm` 0.2.16 (MIT) is the reference they are measured against and the library the feature itself uses; GDB (GPL 3.0) is run as a tool on the Linux cells. Nothing of them is vendored or reused beyond this spike.

## Result

Measured on 2026-11-13: a [first run](https://github.com/ludenea/ludenea/actions/runs/43447842257) of the research CI on the five cells (hosts: AMD EPYC 9V45, Arm Neoverse N2, AMD EPYC 9V74, Cobalt 100, a virtual Apple M1); a [second run](https://github.com/ludenea/ludenea/actions/runs/43450145269), after the motors' half targets were evaluated by the sine alone, whose Linux x86_64 cell landed on another host (AMD EPYC 7763) and whose five cells repeat the first run's hashes, sweeps and counts exactly; and the developer machine natively and in the two containers. The raw measurements, the differences of every sweep and the debugger's output are in the runs' artifacts.

1. **The functions: refuted for all five.** The reference's results are one value on every machine, and every platform's result is within one unit of the last place of it. The count of inputs on which a platform differs from the reference, and which machines return the same results:

   | Function | Inputs | Linux x86_64, glibc 2.39 | Linux ARM64, glibc 2.39 | Windows x86_64 | Windows ARM64 | macOS ARM64 | Distinct results on the five cells |
   | --- | --- | --- | --- | --- | --- | --- | --- |
   | Sine and cosine together | 8 388 608 | 162 546 | the same results | 15 668, other results | the Linux results | 3 560, other results | 3 |
   | Sine | 8 388 608 | 119 992 | the same results | 7 328, other results | the Linux results | 122 954, other results | 3 |
   | Cosine | 8 388 608 | 42 554 | the same results | 8 354, other results | the Linux results | 43 526, other results | 3 |
   | Arc cosine | 8 388 608 | 65 707 | 66 575, other results | 353 254, other results | the Windows x86_64 results | 358 507, other results | 4 |
   | Base-2 logarithm of a count | 1 048 576 | 5 825 | the same results | the same results | the same results | 5 812, other results | 2 |

   - The sine and the cosine split the cells in three: the Windows x86_64 library, the macOS library, and the three others, Windows on ARM64 among them. The arc cosine has four results on five cells: the two architectures of one version of glibc differ. The base-2 logarithm of a whole number up to 2^20 differs on macOS alone.
   - On macOS the sine taken together with the cosine is not the sine taken alone: the pair differs from the reference on 3 560 inputs, the sine alone on 122 954.
   - The developer machine, natively, returns the results of the Windows x86_64 cell for every function. In the container with glibc 2.36 it returns the results of the Linux x86_64 cell for every function; in the container with glibc 2.41 its arc cosine returns other results (354 888 inputs differ from the reference), a fifth set, and its other functions the Linux results.
   - Of the values the worlds take by construction: one of the 64 half centers of the limits gets another sine or cosine on the two Linux cells and Windows on ARM64 than on Windows x86_64 and macOS; the 96 half targets of the motors get three sets of sines, those of the Windows x86_64 cell, which are the reference's, those of the two Linux cells and of Windows on ARM64, which differ from them on three values, and those of macOS, which differ on two; one degree gets the same sine and cosine on every machine.
2. **The worlds: refuted for the four worlds with a feature; the control and the twins hold.**

   | World | Distinct hashes before the first step | Distinct hashes after ticks 1, 2, 5, 10, 30, 100, 300, 600 | Which cells end on the same hash |
   | --- | --- | --- | --- |
   | `control` | 1 | 1 at every tick | all five |
   | `joint_limits` | 1 | 1, 1, 1, 1, 1, 2, 2, 2 | the two Linux cells and Windows ARM64; Windows x86_64 and macOS |
   | `joint_limits`, the twin without limits | 1 | 1 at every tick | all five |
   | `motor_targets` | 1 | 3 at every tick | the two Linux cells and Windows ARM64; Windows x86_64; macOS |
   | `motor_targets`, the twin without motors | 1 | 1 at every tick | all five |
   | `several_colliders` | 1 | 2, 2, 2, 2, 2, 2, 3, 3 | the two Linux cells; Windows x86_64; Windows ARM64 and macOS |
   | `balls_on_hulls` | 1 | 1, 1, 1, 1, 1, 2, 3, 3 | the two Linux cells; the two Windows cells; macOS |

   - On every cell the limits and the motors change what their worlds do: a world and its twin start from one hash and end on two.
   - The worlds part where the functions do. The pendulums part once the one limit whose half center the platforms disagree on is met. The arms part at the first tick, as the sine alone does, in three groups. The worlds whose inertia is not diagonal part as the arc cosine and the cosine do, less the pairs of machines whose results differ only on inputs these bodies did not take.
   - The inertia the eigenvalues give differs before any step: the hash of every body's principal inertia and inertia frame after the build has two values on the five cells for the bodies of several colliders (the two Linux cells against the three others) and three for the hulls (the Linux cells, the Windows cells, macOS); after the steps, in which colliders were added and a density changed, it has three for the bodies of several colliders.
   - **Two versions of one platform's library give two worlds on one machine.** On the developer machine, the container with glibc 2.36 gives the hashes of the Linux x86_64 cell for every world, and the container with glibc 2.41 gives other hashes for the hulls and for the bodies of several colliders; the control, the two twins and the two jointed worlds hash alike under both.
   - The developer machine, natively, gives the hashes of the Windows x86_64 cell for every world.
3. **The reach: holds for four calls; the fifth is computed by the compiler.** Counted on the two Linux cells over 320 ticks, the same on both and in both runs; the container with glibc 2.41, whose worlds of hulls and of several colliders are other worlds, makes the same calls and counts within 6 percent of these:

   | World | Calls during the build | Calls during the steps | Lines of the sources met during the steps |
   | --- | --- | --- | --- |
   | `control` | none | base-2 logarithm 634 | the tree's optimization 159 |
   | `joint_limits` | none | sine and cosine together 64, base-2 logarithm 144 | the limit's half center 64, the tree's optimization 36 |
   | `motor_targets` | none | sine 40 960, base-2 logarithm 612 | the motor's half target 40 960, the tree's optimization 153 |
   | `several_colliders` | cosine 192, arc cosine 96 | cosine 384, arc cosine 192, base-2 logarithm 376 | the eigenvalues 192, the tree's optimization 94 |
   | `balls_on_hulls` | cosine 112, arc cosine 56 | cosine 1 536, arc cosine 768, base-2 logarithm 616 | the eigenvalues 768, the one-degree tolerance 8 661, the tree's optimization 154 |

   - **The limit's sine and cosine are taken once per joint**, at the first step, and not again while the joints stay as they are: 64 calls for 64 joints, all made by the first tick (counted in the container at 1, 2, 10, 140 and 160 ticks). Their callers lead from the joint constraint builder to the island solver's initialization of joints, inside the pipeline's step.
   - **The motor's sine is taken at every solver step of every tick**, for the 32 impulse joints that hang from a multibody's link: 40 960 calls. The joints of the multibodies themselves make no such call: an earlier form of this world, with multibody joints alone, made none during its steps while its motors acted. The call's callers lead from the generic joint constraint to the island solver, inside the step.
   - **The eigenvalues are taken by a step**, through the handling of what changed since the last step. For the bodies of several colliders: at the first step, then when a collider is added or its density changes, and at no other tick. For the hulls: at the first step and then again and again while the world steps, with no change made to the world from outside (in the container, 608 more arc cosines between ticks 10 and 140); what makes the library sum a hull's mass properties again was not traced.
   - **The sine and cosine of one degree are not a call**: the line that holds them is met 8 661 times while the hulls' world steps and no call to the sine or the cosine is counted. The argument is a constant, and the compiler computed the two values when it built the program. In this spike's own code the same expression on a constant gives the bits the platform returns for one degree, on every machine measured.
   - No function of the platform's math library other than these was called by any world.

**What it says and does not say.** The platforms differ on the five functions, in the last bit. Worlds that use an asymmetric joint limit, a motor with a position target on a joint that hangs from a multibody, a body whose inertia matrix is not diagonal, or a convex hull, diverge between cells from the tick the feature first acts; a world with none of them, and the same worlds without the limits or the motors, do not, the logarithm's difference on macOS included. One version of one platform's library does not fix the results either. Not measured: a platform's library on other processors of an architecture, beyond the two AMD models the Linux x86_64 cell landed on (on x86_64, glibc chooses its sine and its cosine at load time among variants, which is why a breakpoint by name misses them); the reach on Windows and macOS, the libraries' sources being the same there; joint kinds other than the revolute, and a limit centered on zero; the soft bodies; the one arc cosine in the mesh intersection code; any way of removing the calls.
