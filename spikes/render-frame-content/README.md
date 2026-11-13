# Frame content

Throwaway spike of the rendering research. It runs the stages of a frame (extract, refresh, prepare, declare and compile, record, submit, read back) at the size of the [reference workload](../reference-workload/README.md), on the world of the [runtime skeleton](../runtime-skeleton/README.md) spike, above a render device port of its own: first with a device that draws nothing, then with the device of each machine that needs no graphics processor, through the native path of the [device baseline](../render-device-baseline/README.md) spike. It measures the design above the port, not a graphics interface: what a frame's content is and whether it is one value everywhere, what the arithmetic that makes it so costs, whether the renderer's optimizations change a byte, and how frames, worlds, lifetimes and a lost device behave when one presenter serves them.

Nothing here is engine code, and no technology is chosen by it.

## Layout

- `src/slices.rs`: the render slices and the extraction, an implementation of the skeleton's extension point: after every tick it copies, chunk by chunk, the position, the velocity, the generation and a visual column of every slot into one of two buffers, and the written blocks of the tile layers. The workload's entities carry no visual component, so the column is derived from the slot and the mass (six slots of eight carry a sprite, two a mesh); the tile layers are not in the workload's state, so their written blocks live in a stand-in that the extraction advances by one tile write per tick, a function of the tick alone.
- `src/visual.rs`: the assets a frame names: three sprites, two meshes at three levels of detail, a tile set, and the geometry of a tile block. Built with integers and exact operations, so that the parts of a payload that feed content are the same bytes everywhere.
- `src/math.rs`: the transcendental functions of prepare in two forms: a math layer written for the spike from the operations IEEE 754 specifies exactly, evaluated in double precision and rounded once, and the platform's math library. No engine math layer exists yet; this one stands in for it and its accuracy is not the layer's.
- `src/prepare.rs`: the view set (the whole map from above, a turning close-up that keeps no mesh, a camera in orbit, a camera near the ground that keeps what lies within a draw distance), the items each view keeps, their per-draw data, the total order of a list, the culling test, and the content hash.
- `src/graph.rs`: the frame graph: declaration through a builder that hands out access tokens, compilation (removal of the passes nothing reads, lifetimes, the order of declaration kept).
- `src/port.rs`: the render device port (typed generational handles, creation from descriptions, passes recorded in parts, submissions, read-backs), with what it adds to an adapter in every build: a stale handle refused, a released resource destroyed only after every frame declared before its release, two assertions that are counted, device memory counted per owner and class, and a loss simulated on purpose. The same file holds the content device, which accepts every call and executes none.
- `src/instance.rs`: what the worlds of an instance share: the caches of assets and pipelines, the pool of transient targets, the hand-off of one finished frame per world, and the presenter, a role taken by a presentation thread or served from a stepping thread.
- `src/pool.rs`: the frame pool of one world and the instrument that measures a stage (wall time, sum of the tasks' durations, count).
- `src/renderer.rs`: the renderer of one world: refresh (the geometry of a tile block, built when first seen and when its version changed), prepare, declare and compile, record.
- `src/scenarios.rs`, `src/harness.rs`, `src/main.rs`: the measurements and the report. The parent process measures on the device that draws nothing and starts one child process per device and one more under the interface's validation layer.

The three adapters behind the port are in the device baseline library, which this spike extends: `render-device-baseline/src/frames.rs` states what an adapter implements (resources that outlive a frame, passes recorded in parts by any thread, submissions that are polled) and holds a check of every call; `src/vulkan/frames.rs`, `src/d3d12/frames.rs` and `src/metal/frames.rs` implement it, each through the system's loader with no third-party graphics crate. `cargo run --release -p render-device-baseline --example frame_device_check` runs the check on every device of a machine.

## The frame that is measured

One world of 10 000 entities. A frame draws four views of 128 by 128 pixels into four transient images (8-bit RGBA in the sRGB encoding, 4 samples, a 32-bit float depth), then composes them texel for pixel onto the world's target of 256 by 256, which is read back. Each view's pipeline declares two passes: the scene (opaque items front to back, then blended items back to front, one draw per item, 88 bytes of per-draw data) and a selection mask whose image nothing reads while outlines are off. The four views keep about 32 000 items of the 40 000 the entities give them, and 32 tile blocks each.

The content of a frame is, for each view in the order of the view set, its data and its passes as declared, each with its sorted list before any culling, each item by its assets, by its pipeline's description less the device's shading form, and by the bytes of its per-draw data; then the composition. The content hash is FNV-1a over it.

## Hypotheses

1. The content hash of a frame prepared with the math layer is one value on the five cells at 1, 4 and 8 workers; prepared with the platform's arithmetic it is recorded per cell.
2. What the math layer costs prepare against the platform's arithmetic, for 10 000 drawables in four views, is recorded, with no threshold.
3. The bytes of a frame are the same in the ordinary mode and in the reference mode in full: recording serial, culling off, no removal of unread passes, transients filled with the marker.
4. Declaring and compiling a graph of 300 passes and 600 resources takes under one millisecond on every cell on one thread.
5. The lateness of ticks and the rate of frames with recording and submission overlapping the next ticks, and without, are recorded, with no threshold.
6. 1, 4 and 16 worlds drawn offscreen through one presenter meet the shared device claim (a frame cost per world at 16 worlds no more than twice the cost at one world), judged on the device that draws nothing, which isolates the presenter's share, and recorded on the software device, with device memory recorded per world.
7. After a loss simulated at the port, the frame streams end, the device is created again, and the images that follow carry the bytes of those before the loss.
8. A frame asked reproducible after ten thousand ticks unwatched carries the bytes of the same tick's frame in a world watched from its first tick.
9. Under the bound of three frames in flight, a resource released while a frame that names it is in flight is destroyed only after that frame, the port's assertion and the validation layer both staying silent, and a release ordered wrongly on purpose trips both.
10. The extraction's bytes per tick for a tile layer's written blocks, when one tile changes, are recorded at blocks of 32 by 32 cells and at two other sizes, with no threshold.
11. The time from a shader's new payload being resident to the frame that shows it is recorded, with no threshold.

What a refutation changes is stated with the specification, in the research findings.

## Method

Every measurement runs on the content device in the parent process. Those that need pixels or a device's time (the modes, the pace, the worlds, the loss, the unwatched world, the shader payload) run again in a child process on each device of the machine that needs no graphics processor: Mesa's software device through Vulkan on the Linux cells, the Windows software adapter through Direct3D 12 on the Windows cells, the machine's virtual device through Metal on the macOS cell. The lifetimes run in a second child, started with the validation layer's environment. Each child first runs the check of the adapter behind the port, the second under the layer, so that a device that draws wrongly is told apart from a renderer that does.

- **Content hash**: a world is stepped to tick 30 and a reproducible frame is prepared at the factor 0.5, then to tick 60 at 0.25, with the world and the renderer at 1, 4 and 8 workers, once with each arithmetic. Prepare writes each view's list in parts, one per chunk of the render slice and per task, and merges them in chunk order before the sort.
- **Cost of the math layer**: 31 repetitions of prepare alone (160 tasks, one per view and chunk), the two arithmetics in turn, at worker budgets of 1 and 4. The two differ by the transcendental functions only (an arc tangent and a sine and cosine per item, a logarithm per mesh, a few per view); every other operation is exact under both and written once.
- **Modes**: the same reproducible frame in the ordinary mode (a view's pass recorded in 8 parts by the pool's workers at once, culling on, unread passes removed), in the reference mode, and with each of the four switches alone, at two ticks. In the reference mode every pass is recorded in one part by one thread, pass after pass; the selection masks are drawn; each transient is cleared to a marker color by a pass of its own before its first use.
- **Graph**: 300 repetitions of the declaration and compilation of a synthetic graph: each pass creates two transients and reads up to three that earlier passes wrote, one pass in five feeds nothing, the last writes the one output.
- **Pace**: 180 ticks due every sixtieth of a second. A tick that is due runs before a frame is started; its lateness is the time between its due instant and its start. Two rates of frames (one frame after each tick, and frames back to back), two worker budgets (1 and 4), and the presenter as a presentation thread (recording, submission and read-back overlap the next ticks) or served from the stepping thread (the loop goes on when the image is read back).
- **Worlds**: 1, 4 and 16 worlds on one instance, each with its own stepping thread and a worker budget of one, each stepping one tick then drawing one frame and waiting for its image. The frame cost per world is the mean time from a frame's start to its image. On the device that draws nothing the series is taken five more times, beside a control in which each world has an instance, a device and a presentation thread of its own.
- **Loss**: a control world is drawn at four ticks. A second world is drawn at the first two; while a frame of the third is on its way the port starts answering "lost", drops the device behind it and makes every handle stale. The capability creates the device again and fills its caches by the path that filled them at startup; the world subscribes anew and is drawn at the third and fourth ticks.
- **Unwatched world**: one world draws an interactive frame every 10 ticks on the content device and every 500 on a device that draws, for 10 000 ticks; another is stepped 10 000 ticks with no frame. Both are then asked the reproducible frame of the last tick. The interactive frames keep a declared history, the level of detail each view last drew a mesh at; the tile blocks of the watched world were rebuilt as gameplay wrote them.
- **Lifetimes**: at the port, with no world. A buffer is released while three submitted frames name it; a frame declared after a buffer's release names it; a buffer is destroyed at once under a frame the device has finished but whose completion nobody observed; a buffer is destroyed at once right after the submission of a frame of 4 000 draws. The last is a real misuse of a device and may end the process: each step is written to the report before the next.
- **Tile layer**: a layer of 512 by 512 cells of two bytes; the written blocks are child entities stored in chunks of 256, with one change flag per chunk; one tile is written per tick for 1 000 ticks, after 64 writes in a corner or 20 000 over the whole map, at blocks of 16, 32 and 64 cells a side. A chunk a write touched is gathered from the domain; another is copied from the previous tick's slice, since the slices are two buffers.
- **Shader payload**: a world is drawn back to back at one tick. The second variant of the frame shader is imported (compiled by the interface's own compiler), then handed to the instance while a frame is on its way; the presenter builds its pipelines between two submissions and publishes a new cache entry; the time runs from the hand-over to the delivery of the first image drawn with it.

Times are wall-clock unless called processor time, which is the sum of a stage's task durations.

## Departures from the specification

- **The math layer** is the spike's own stand-in, as said above.
- **The port's command buffers are the interface's own**: a part is recorded natively by the task that makes it. On Metal the encoders of a pass are created in order when the pass is opened, by the task that opens it.
- **The frame pool is a second set of threads.** The skeleton's world spawns its workers for each step, so while a recording overlaps a tick both sets run, which the concurrency model's budget would not allow.
- **The frame's memory is a set of buffers a world keeps and reuses**, in place of a region tagged with the frame.
- **One draw per item**, with no batching: the 2D family's batching is not designed yet.
- **The tile layers' state and the visual column are stand-ins**, as said above, and no hash of the simulation covers them.
- **The simulated loss tests the engine's way back and not an interface.** A real removal is tried apart, on the devices of Direct3D 12, the one interface of the three with a call that removes a device (`ID3D12Device5::RemoveDevice`): the control world is drawn, the device is removed, the interface is asked what became of it, a frame is asked, and if the interface reports the removal the capability takes it for a loss and comes back the same way.

## Environment

Research CI, workflow `research-ci.yml`, five cells (see [`spikes/README.md`](../README.md)), run on demand with `-f spike=render-frame-content`. Rust toolchain pinned in [`rust-toolchain.toml`](../rust-toolchain.toml), release profile, default target features. The setup script is the device baseline spike's. The founder's Windows machine judges the claim on sixteen worlds and runs the hardware devices: a cell label that starts with `founder` measures every device of a machine, any other label only its software and virtual devices.

## Result

Measured on 2026-11-12 by the research CI on the five cells ([run 43313710084](https://github.com/ludenea/ludenea/actions/runs/43313710084), the raw measurements, the pictures and the environment of each cell in its artifacts), and on the founder's machine (Windows 11, an AMD Ryzen 9 9950X3D of 16 cores, an NVIDIA GeForce RTX 5080, an AMD Radeon integrated device and the software adapter, all three through Direct3D 12; Vulkan is unavailable there natively for want of a GLSL compiler on the path, as on the Windows cells). Rust 1.99.1, release profile. Three earlier runs on the cells are named where they add a fact ([first](https://github.com/ludenea/ludenea/actions/runs/43303981606), [macOS alone](https://github.com/ludenea/ludenea/actions/runs/43308012415), [third](https://github.com/ludenea/ludenea/actions/runs/43309422284)). The founder's machine was measured three times: twice while another spike's measurement ran on it, and a third time, the reference run all its figures below come from, at 22:56 UTC with no other measurement running. The three agree on every identity; the ratio of sixteen worlds, taken in one series then, read 2.25 in each of the two runs that overlapped.

| Cell | Device that needs no graphics processor | Driver or implementation | Processor of the run |
| --- | --- | --- | --- |
| `linux-x86_64` | llvmpipe (LLVM 20.1.2, 256 bits), Vulkan | Mesa 25.2.8 | AMD EPYC 9V74, 4 CPUs |
| `linux-arm64` | llvmpipe (LLVM 20.1.2, 128 bits), Vulkan | Mesa 25.2.8 | Neoverse-N2, 4 CPUs |
| `windows-x86_64` | Microsoft Basic Render Driver, Direct3D 12 | 10.0.26100.33648 | Intel Xeon Platinum 8573C, 4 CPUs |
| `windows-arm64` | Microsoft Basic Render Driver, Direct3D 12 | 10.0.26100.9711 | Cobalt 100, 4 CPUs |
| `macos-arm64` | Apple Paravirtual device, Metal | macOS 15.8.2 | Apple M1 (Virtual), 3 CPUs |

The check of the adapter behind the port holds on every device (15 lines of 15), and under each interface's validation layer on the five cells with no message of the layer.

### The eleven hypotheses, in one table

| What | Outcome |
| --- | --- |
| One content hash on the five cells at 1, 4 and 8 workers with the math layer | Holds: one value on the five cells and the founder's machine; five values on the five cells with the platform's arithmetic |
| What the math layer costs prepare | Recorded: 1.11 to 1.53 times the platform's arithmetic on the cells, 1.28 to 1.35 on the founder's machine |
| The same bytes in the ordinary mode and in the reference mode | Holds on every device measured, for the reference mode in full and for each of its four switches alone |
| A graph of 300 passes and 600 resources under one millisecond | Holds at the median and at the 99th percentile on every cell (40 to 142 microseconds at the median); one repetition of 300 took 1.8 milliseconds on the macOS cell |
| Lateness of ticks and rate of frames, with overlap and without | Recorded |
| 1, 4 and 16 worlds within twice one world's frame cost, on the device that draws nothing | At the threshold on the founder's machine: 2.04 at the median of five series (1.96 to 2.05), and 2.05 for sixteen worlds that share nothing; the presenter's share is 11 microseconds a frame. Recorded on the devices: 3.8 to 22 |
| After a simulated loss, the streams end, the device comes back, the images are those of a world that lost nothing | Holds on every device measured |
| A reproducible frame after 10 000 ticks unwatched is the watched world's | Holds on every device measured |
| A resource released under three frames in flight is destroyed only after them, both oracles silent; a wrong order trips both | The first half holds on every device. The port's assertion trips on every wrong order; the validation layer reports it on Vulkan only |
| The extraction's bytes for one changed tile | Recorded |
| From a shader's new payload to the image that shows it | Recorded |

### The content hash

| Machine | Math layer, the two frames | One value at 1, 4 and 8 workers | Platform's arithmetic, the two frames | One value at 1, 4 and 8 workers |
| --- | --- | --- | --- | --- |
| `linux-x86_64` | `01284e00a6490c3b`, `70e60289841660ef` | yes | `c802ebed70ab047f`, `4aeb77c01254dcb2` | yes |
| `linux-arm64` | `01284e00a6490c3b`, `70e60289841660ef` | yes | `3c005e12c102ed72`, `eb887c924f84b35c` | yes |
| `windows-x86_64` | `01284e00a6490c3b`, `70e60289841660ef` | yes | `60a84bdbdc08b463`, `34572b217c8e2c7b` | yes |
| `windows-arm64` | `01284e00a6490c3b`, `70e60289841660ef` | yes | `5cccf4c701d7b9d4`, `a9f61b6972ff079b` | yes |
| `macos-arm64` | `01284e00a6490c3b`, `70e60289841660ef` | yes | `02cbec19afc06035`, `4679fdeef8d4813b` | yes |
| Founder's machine | `01284e00a6490c3b`, `70e60289841660ef` | yes | `60a84bdbdc08b463`, `34572b217c8e2c7b` | yes |

With the platform's library the content is one value per operating system and architecture: the founder's machine gives the Windows x86_64 cell's. The content hash of the frame of tick 10 000 (`04cf0aa17b81e462`) is also one value on the five cells and the founder's machine, and the content hash is the same in every mode of a frame, on every device and on the device that draws nothing.

### What the math layer costs prepare

Processor time of the 160 prepare tasks, in microseconds, median of 31 repetitions, for 31 833 items kept of the 40 000 the four views are given.

| Machine | Worker budget | Math layer | Platform's arithmetic | Ratio of medians |
| --- | --- | --- | --- | --- |
| `linux-x86_64` | 1 | 2 939 | 2 279 | 1.29 |
| `linux-x86_64` | 4 | 3 824 | 3 202 | 1.19 |
| `linux-arm64` | 1 | 2 849 | 2 053 | 1.39 |
| `linux-arm64` | 4 | 3 179 | 2 434 | 1.31 |
| `windows-x86_64` | 1 | 3 307 | 2 434 | 1.36 |
| `windows-x86_64` | 4 | 4 669 | 4 212 | 1.11 |
| `windows-arm64` | 1 | 2 824 | 2 107 | 1.34 |
| `windows-arm64` | 4 | 3 173 | 2 479 | 1.28 |
| `macos-arm64` | 1 | 2 471 | 1 703 | 1.45 |
| `macos-arm64` | 4 | 2 595 | 1 702 | 1.53 |
| Founder's machine | 1 | 1 682 | 1 243 | 1.35 |
| Founder's machine | 4 | 1 882 | 1 471 | 1.28 |

On the founder's machine the math layer adds 0.44 millisecond of processor time to a prepare of 1.24, and 0.1 millisecond of wall time at a worker budget of four (0.48 against 0.38). The figure is the cost of this spike's functions, which are evaluated in double precision: an engine layer with single-precision kernels would cost something else.

### The modes

On every device the six frames of a tick (ordinary, reference, and each switch alone) are the same bytes, at both ticks. The ordinary mode records 22 060 draws in 5 passes; the reference mode 33 988 draws in 17 passes (the four selection masks, and a marker pass before each of the eight transients).

| Device | Image, tick 30 | Image, tick 60 |
| --- | --- | --- |
| llvmpipe, `linux-x86_64` (AMD host) | `b10726f7ad07c904` | `5f13174750ed52a9` |
| llvmpipe, `linux-arm64` | `10304e801b170703` | `5a66eec683639ddc` |
| Microsoft Basic Render Driver, both Windows cells and the founder's machine | `69a25be16fa530f3` | `8150e37cb6a15be4` |
| Apple Paravirtual device | `79bdc637cb1ae479` | `e10f9b3dd7dc24d3` |
| NVIDIA GeForce RTX 5080 | `1c9f680d34522c01` | `93ed562bff2a20f7` |
| AMD Radeon | `6cb2b09ba8cb7a79` | `a164e371d17e842d` |

Six images for one content, as the device baseline found of its scene.

### The graph

Declaring and compiling the synthetic graph (300 passes, 600 resources, of which 154 passes are kept and 146 removed), in microseconds over 300 repetitions on one thread.

| Machine | Fastest | Median | 99th percentile | Slowest |
| --- | --- | --- | --- | --- |
| `linux-x86_64` | 53 | 54 | 74 | 78 |
| `linux-arm64` | 63 | 66 | 71 | 75 |
| `windows-x86_64` | 80 | 142 | 282 | 418 |
| `windows-arm64` | 115 | 131 | 233 | 276 |
| `macos-arm64` | 33 | 40 | 543 | 1 844 |
| Founder's machine | 39 | 40 | 45 | 83 |

The macOS cell's slowest repetition is the one figure above a millisecond; it was 251 and 347 microseconds in two earlier runs. The frame's own graph (9 passes) takes 3 to 6 microseconds to declare and compile.

### The pace

On the device that draws nothing, with one frame after each tick, no tick is late by more than 30 microseconds on the four Linux and Windows cells and on the founder's machine, at either worker budget, with or without a presentation thread (the macOS cell's ticks are late by up to 25 milliseconds in every loop, its runner's own noise). With frames back to back a tick waits for the prepare it falls in: 0.4 to 3.2 milliseconds at the median and, the macOS cell apart, 0.8 to 6.3 at the 99th percentile, as the design says (one prepare, and one recording more at a worker budget of one).

On a device that draws, milliseconds, one frame after each tick at most:

| Machine and device | Worker budget | Overlap | Lateness: median | 99th percentile | Largest | Frames per second | Refresh | Prepare and sort | Record | Submit | Device |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| Founder's machine, NVIDIA | 4 | yes | 0 | 0 | 0 | 60 | 1.2 | 0.9 | 0.4 | 0.3 | 0.3 |
| Founder's machine, NVIDIA | 4 | no | 0 | 0 | 0 | 60 | 1.1 | 0.9 | 0.4 | 0.3 | 0.2 |
| Founder's machine, AMD | 4 | yes | 0 | 0 | 0 | 60 | 2.4 | 1.0 | 0.6 | 0.5 | 3.1 |
| Founder's machine, AMD | 4 | no | 0 | 0 | 0 | 60 | 2.5 | 1.0 | 0.6 | 0.5 | 3.0 |
| Founder's machine, software adapter | 4 | yes | 65.9 | 129.4 | 131.4 | 7.6 | 129.6 | 1.0 | 2.7 | 130.3 | 1.7 |
| Founder's machine, software adapter | 4 | no | 70.6 | 135.1 | 137.0 | 7.1 | 0.9 | 1.0 | 2.6 | 129.8 | 1.8 |
| `macos-arm64`, virtual device | 4 | yes | 1.9 | 9.9 | 10.0 | 58.0 | 0.1 | 1.8 | 4.7 | 0.1 | 20.3 |
| `macos-arm64`, virtual device | 4 | no | 11.6 | 31.2 | 32.5 | 41.4 | 0.1 | 1.4 | 4.2 | 0.1 | 12.6 |
| `macos-arm64`, virtual device | 1 | yes | 10.2 | 31.7 | 40.6 | 42.6 | 0.1 | 4.0 | 13.5 | 0.1 | 20.0 |
| `macos-arm64`, virtual device | 1 | no | 17.7 | 41.2 | 53.9 | 26.3 | 0.2 | 3.9 | 11.7 | 0.1 | 15.0 |
| `linux-x86_64`, llvmpipe | 4 | yes | 49.3 | 288.1 | 294.6 | 8.7 | 0.3 | 3.1 | 3.4 | 0 | 279.2 |
| `linux-x86_64`, llvmpipe | 4 | no | 63.7 | 123.4 | 128.0 | 7.5 | 0.5 | 2.0 | 1.3 | 0 | 118.8 |
| `linux-arm64`, llvmpipe | 4 | yes | 51.3 | 300.3 | 313.8 | 8.7 | 0.4 | 2.7 | 3.0 | 0 | 260.4 |
| `linux-arm64`, llvmpipe | 4 | no | 61.7 | 120.8 | 123.1 | 7.8 | 0.4 | 1.5 | 1.2 | 0 | 117.7 |
| `windows-x86_64`, software adapter | 4 | yes | 93.3 | 182.2 | 195.0 | 4.9 | 177.9 | 2.7 | 6.4 | 201.7 | 3.9 |
| `windows-x86_64`, software adapter | 4 | no | 121.5 | 230.7 | 239.6 | 3.9 | 3.1 | 2.6 | 6.4 | 218.8 | 4.1 |
| `windows-arm64`, software adapter | 4 | yes | 87.1 | 173.3 | 185.8 | 5.5 | 170.2 | 1.5 | 4.9 | 170.1 | 8.0 |
| `windows-arm64`, software adapter | 4 | no | 103.2 | 193.0 | 208.2 | 4.9 | 2.9 | 1.5 | 4.9 | 170.8 | 8.5 |

- **A hardware device's frame fits in a tick's period either way.** With frames back to back at a worker budget of four, the overlap gives 635 frames a second against 409 on the NVIDIA device and 328 against 167 on the AMD one, and a tick is late by 0.25 millisecond at the median against 1.05, and by nothing against 2.67.
- **The macOS cell's virtual device is where the overlap shows on the ticks**: 1.9 milliseconds of lateness at the median against 11.6 at a worker budget of four, and 58 frames a second against 41.
- **On Mesa's software device the overlap gives more frames and later ticks.** The rasterizer's threads and the tick's workers share four processors: with three frames in flight the rasterizer never rests, each frame takes it 260 to 280 milliseconds in place of 118, and the worst ticks are late by 290 to 310 milliseconds against 120 to 130.
- **On the Windows software adapter a presentation thread does not free the loop.** The adapter does a frame's work inside the submission (124 to 219 milliseconds), and the creation of a vertex buffer by refresh, a stage the loop waits for, waits for the submission under way: refresh takes 116 to 196 milliseconds with the overlap and 1 to 5 without.
- **Refresh costs the hardware devices 1.2 and 2.4 milliseconds a frame**, the creation of one tile block's vertex buffer (245 KB), inside the stage the stepping loop waits for.

### The worlds through one presenter

On the device that draws nothing, on the founder's machine (16 cores), the frame cost per world, the mean time from a frame's start to its image:

| Worlds | Frame cost per world, microseconds | Against one world | Processor time of the stages per frame | Presenter's time per frame |
| --- | --- | --- | --- | --- |
| 1 | 2 829 | 1 | 2 791 | 4.6 |
| 4 | 3 517 | 1.24 | 3 466 | 4.8 |
| 16 | 5 643 | 2.00 | 5 539 | 10.6 |

Over five series the ratio at sixteen worlds is 1.96 at the lowest, 2.04 at the median and 2.05 at the highest (1.21 to 1.25 at four worlds). A control in which each of the sixteen worlds has an instance, a device and a presentation thread of its own gives 1.97, 2.05 and 2.12. The claim's threshold, a cost above twice one world's, is therefore reached by sixteen worlds that share nothing: what a world costs beside fifteen others is its own stages, whose processor time doubles (2.8 to 5.5 milliseconds) on sixteen cores busy at once, and not the shared device, whose presenter spends 11 microseconds a frame. An earlier version of the spike allocated each frame's lists afresh and read 5 at sixteen worlds, all of it in the process allocator; the lists are kept per world since.

On the cells the sixteen stepping threads share four processors (three on the macOS cell), and the ratio measures the scheduler: 3.8 and 5.8 at the median on the Linux cells, 4.9 on the macOS cell, 1.1 and 1.6 on the Windows cells, where a frame of a few milliseconds ends inside one time slice.

On the devices that draw, the frame cost per world in milliseconds:

| Machine and device | 1 world | 4 worlds | 16 worlds | 16 against one | Presenter's time per frame at 16 | Most frames in flight |
| --- | --- | --- | --- | --- | --- | --- |
| Founder's machine, NVIDIA | 5.6 | 8.3 | 21.2 | 3.8 | 1.3 | 3 |
| Founder's machine, AMD | 10.0 | 16.6 | 47.1 | 4.7 | 2.2 | 3 |
| Founder's machine, software adapter | 136 | 495 | 2 018 | 14.8 | 128 | 2 |
| `linux-x86_64`, llvmpipe | 163 | 675 | 3 281 | 20.2 | 3.5 | 3 |
| `linux-arm64`, llvmpipe | 152 | 604 | 3 370 | 22.2 | 2.3 | 3 |
| `windows-x86_64`, software adapter | 240 | 829 | 3 173 | 13.2 | 212 | 2 |
| `windows-arm64`, software adapter | 202 | 691 | 2 767 | 13.7 | 185 | 3 |
| `macos-arm64`, virtual device | 28.7 | 55.7 | 204 | 7.1 | 1.7 | 3 |

On a software device the rasterizer is the cause, as expected: sixteen worlds wait for one device that draws one frame at a time. On the two hardware devices the presenter itself is the queue: it spends 0.8 to 2.2 milliseconds a frame (the submission of five passes and the read-back of one image), so sixteen worlds wait their turn.

Device memory, counted from the descriptions: 2.36 MB of targets per world (the target at 4 samples with its depth and its image), 5.2 MB of tile block geometry per world, 0.39 MB of textures and geometry for the instance, and transient targets that grow with the frames that coincide: 2.4 MB for one world on the device that draws nothing, 5.9 to 7.1 MB on a device that draws, and 37 to 45 MB at their peak for sixteen worlds.

### A lost device

On every device: the stream ends with the degraded error, a frame asked meanwhile is refused, the device is created again, 46 resources are created again by the paths that filled the caches (the textures, the shared geometry, the pipelines, the target, the transients and the tile blocks' buffers), and the images of the two ticks that follow are the bytes of the control world's. The frame on its way at the loss is dropped and nothing else is.

**A real removal**, on Direct3D 12: after `ID3D12Device5::RemoveDevice` the interface reports `DXGI_ERROR_DEVICE_REMOVED` on the NVIDIA device, the AMD device and the software adapter (the founder's machine and both Windows cells), the frame asked next is dropped at its first creation of a resource, and the capability comes back the way it does from the simulated loss, to the control's bytes. The adapter lets the queue finish its work before the call: removed with work in flight and released at once, the software adapter ended the process with an access violation in 6 runs of 8 while the adapter was built. Vulkan and Metal offer no call that removes a device.

### The unwatched world

On every device the two reproducible frames of tick 10 000 are the same, by content and by bytes. The watched world had drawn 1 000 frames on the device that draws nothing (20 on a device that draws) and rebuilt 6 131 tile blocks as gameplay wrote them (184), against 32 built at once in the unwatched one; its interactive frame of the same tick read its history of levels of detail and differs in content from its reproducible one, so the history is not what the reproducible frame drew from.

### Lifetimes

| Step | Port | Vulkan's layer (Mesa, both Linux cells) | Direct3D 12's layer (both Windows cells) | Metal's layer (macOS cell) |
| --- | --- | --- | --- | --- |
| A buffer released while three frames that name it are in flight | Destroyed after the third frame, not before; no assertion | Silent | Silent | Silent |
| A frame declared after the release names the buffer | Refused; the assertion trips | Never sees it | Never sees it | Never sees it |
| A buffer destroyed at once under a frame the device has finished and nobody observed | The assertion trips | Reports it (`VUID-vkDestroyBuffer-buffer-00922`) | Silent | Silent |
| A buffer destroyed at once right after the submission of 4 000 draws | The assertion trips | The process ends with a segmentation fault | The process ends with the exit code 2173 (0x87D) | Silent, and the process goes on |

The validation layer agrees with the port on a wrong order on Vulkan only. Direct3D 12's layer follows what the device has really finished, so it is silent on a destruction the device no longer cares about, and the process ended at the one it does care about before a message could be read. On Metal a command buffer retains what it names, so a destruction ordered wrongly destroys nothing early: the interface keeps the rule itself, with the adapter as written. On the founder's machine, which has no validation layer, the last step left the software adapter running and the two hardware devices reporting `DXGI_ERROR_DEVICE_HUNG` (the AMD device in one run of two).

The first form of the port's assertion refused any command that named a released resource, and dropped a legitimate frame on the macOS cell, declared before the release of a pair of pipelines and recorded after it; the rule is the one stated: a frame declared before a release keeps what it resolved.

### The tile layer

| Written before | Block side, cells | Written blocks | Bytes of one block | Gathered from the domain per tick | Copied from the previous slice per tick |
| --- | --- | --- | --- | --- | --- |
| A corner of the map (64 tiles) | 16 | 36 | 512 | 18 396 | 0 |
| A corner of the map | 32 | 9 | 2 048 | 18 432 | 0 |
| A corner of the map | 64 | 4 | 8 192 | 32 768 | 0 |
| The whole map (20 000 tiles) | 16 | 1 024 | 512 | 131 072 | 393 216 |
| The whole map | 32 | 256 | 2 048 | 524 288 | 0 |
| The whole map | 64 | 64 | 8 192 | 524 288 | 0 |

One changed tile costs every written block of its chunk: 9 to 256 times the bytes of the block it is in. With the two slices as two buffers, the written blocks of the other chunks are copied too, from the previous slice, so a tick moves the bytes of every written block of the layer whichever chunk changed (524 KB a tick once the whole map is written, at any block size). The same values on every machine.

### A shader's new payload

| Machine and device | Form | Import | Pipelines built, first time in the process | Pipelines built, payload seen before | Resident to the image that shows it | Frame start to image |
| --- | --- | --- | --- | --- | --- | --- |
| Founder's machine, NVIDIA | Bytecode by `D3DCompile` | 2.9 ms | 20 ms | 0.5 ms | 4.3 ms | 3.1 ms |
| Founder's machine, AMD | Bytecode | 2.8 ms | 7.6 ms | 0.4 ms | 11.5 ms | 6.0 ms |
| Founder's machine, software adapter | Bytecode | 3.1 ms | 0.5 ms | 0.5 ms | 253 ms | 130 ms |
| `windows-x86_64`, software adapter | Bytecode | 5.6 ms | 683 ms | 0.7 ms | 442 ms | 232 ms |
| `windows-arm64`, software adapter | Bytecode | 5.1 ms | 11.7 ms | 0.7 ms | 382 ms | 193 ms |
| `linux-x86_64`, llvmpipe | SPIR-V by the glslang tool | 144 ms | 3.3 ms | 1.5 ms | 350 ms | 167 ms |
| `linux-arm64`, llvmpipe | SPIR-V | 163 ms | 1.3 ms | 1.4 ms | 457 ms | 147 ms |
| `macos-arm64`, virtual device | Metal text, compiled by the platform | none | 200 ms | 0.7 ms | 78 ms | 30 ms |

The time to the image is the build of the pipelines and one to two frames: the frame on its way keeps the pipelines it resolved, one image is delivered with the old payload, and the next shows the new one. The first-time column is from the first runs, in which the process had never built the payload; in the final run the adapter's check had built it once. The content hash does not change with the payload: the shading form is not content.

### A hardware device through Metal

Measured on 2026-11-13, twice, on the founder's Apple M5: a MacBook Pro with 4 performance and 6 efficiency cores, macOS 26.5, Rust 1.99.1, release profile. Other applications ran on the machine (20 to 30 percent of its processors busy as sampled around the runs), so the times are a sample; the two runs agree on every identity. The figures are the second run's, on mains, with the first run's beside them where they differ.

- **The content hash**: with the math layer the value of the five cells and of the Windows machine (`01284e00a6490c3b`, `70e60289841660ef`), at 1, 4 and 8 workers; with the platform's arithmetic the macOS cell's (`02cbec19afc06035`, `4679fdeef8d4813b`), across macOS 15.8.2 and 26.5. The frame of tick 10 000 has the content hash of every other machine.
- **The math layer** costs prepare 1.28 times the platform's arithmetic at a worker budget of one (1 074 microseconds of processor time against 836) and 1.26 at four (1 154 against 915); 1.27 and 1.24 in the first run.
- **The graph** of 300 passes is declared and compiled in 25 microseconds at the median and 35 at the 99th percentile.
- **The modes**: the six frames of a tick are the same bytes, at both ticks. **The images are not the virtual Metal device's**: `f228433cfaf2df42` and `2806d73b9ee861d3`, a seventh image for one content, where the device baseline's scene gives one image on the two Metal devices.
- The check of the adapter behind the port holds (15 lines of 15), and under Metal's validation layer with no message.

The pace on the device, milliseconds, one frame after each tick at most:

| Worker budget | Overlap | Lateness: median | 99th percentile | Largest | Frames per second | Refresh | Prepare and sort | Record | Submit | Device |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| 4 | yes | 0 | 0 | 2.4 | 60 | 0.05 | 0.8 | 1.9 | 0.06 | 3.8 |
| 4 | no | 0 | 0 | 0 | 60 | 0.04 | 0.8 | 2.0 | 0.06 | 3.1 |
| 1 | yes | 0 | 0 | 1.7 | 60 | 0.04 | 2.1 | 5.1 | 0.05 | 4.1 |
| 1 | no | 0 | 0 | 1.7 | 60 | 0.04 | 2.1 | 5.0 | 0.05 | 3.4 |

- **The frame fits in a tick's period either way**, as on the two hardware devices of the Windows machine. In the first run the loop without overlap at a worker budget of one was late by 5.2 milliseconds at the 99th percentile and 10.2 at the most; no other loop of either run is late by more than 0.04 millisecond at the 99th percentile.
- **With frames back to back the overlap gives more frames and earlier ticks**: 286 to 293 frames a second against 157 to 166 at a worker budget of four, a tick late by nothing at the median against 2.4 milliseconds; 115 to 126 against 82 to 84 at a budget of one, a tick late by 2.3 to 3.6 milliseconds against 5.0 to 6.8.
- **Refresh costs 0.03 to 0.05 millisecond a frame**, where the creation of one tile block's vertex buffer costs the two hardware devices of the Windows machine 1.2 and 2.4.
- Recording the 22 060 draws takes 5.0 to 5.4 milliseconds on one thread and 1.9 to 2.3 at a worker budget of four. The device baseline's Metal code asks the runtime whether an object answers each message before it sends it, so the figure is that code's cost and not the interface's.

The worlds through one presenter:

- **On the device that draws nothing, sixteen worlds cost 2.6 to 3.7 times one world's frame** (the median of five series is 3.0 to 3.5 over three runs), and 1.1 to 1.4 at four worlds. The control in which each world has an instance, a device and a presentation thread of its own gives 2.7 to 4.0 (3.2 to 3.4 at the median). On this machine of ten cores, four of them fast, the threshold of twice is passed by sixteen worlds that share nothing: the cause is the processors, as on the Windows machine, and not the shared device, whose presenter spends 12 to 21 microseconds a frame.
- **On the device** a frame costs a world 10.9 to 11.3 milliseconds alone, 11.2 to 15.4 among four and 30.9 to 31.7 among sixteen, 2.8 times one world's; the presenter spends 0.3 millisecond a frame at sixteen, with three frames in flight at most. The transient targets take 4.7 MB for one world and 44.8 MB at their peak for sixteen.

The rest:

- **A lost device**: holds as on every device, by the simulated loss; Metal offers no call that removes a device.
- **The unwatched world**: the two reproducible frames of tick 10 000 are the same, by content and by bytes.
- **Lifetimes under Metal's layer on a hardware device**: as on the macOS cell. The layer is silent on the four steps, and the process goes on after the last.
- **A shader's new payload** (Metal text of 5 602 bytes, compiled by the platform): the pipelines are built in 0.24 to 0.25 millisecond, the payload seen before; the image that shows it comes 12.2 to 14.1 milliseconds after the payload is resident, a frame taking 8.2 to 8.5 from its start to its image.

### Not measured

- A hardware device through Vulkan: no such machine was available, and Vulkan natively on the founder's Windows machine lacks a GLSL compiler on the path.
- The times on the founder's Apple M5 with the machine at rest: other applications ran during both runs.
- Direct3D 12's validation layer on a hardware device: the founder's machine lacks the Graphics Tools component of Windows.
- What Direct3D 12's layer says before it ends the process on the last lifetime step.
- A pass of two or three parts on Metal: the cause of a one-part pass having no effect through a parallel encoder on the macOS cell's device was not found; one part is recorded through a plain encoder, and eight parts and four are measured.
- The cost of an engine math layer: the layer here is the spike's own.
- The content of a frame under batching, a level of detail chosen on the device, lights and their clusters, or a history other than the level of detail: none is built.
- The container cell: the spike was run in a container locally (Ubuntu 24.04 with Mesa 25.2.8, the Linux x86_64 cell's results) and not as a cell of the research CI.

The result holds for the reference workload version 1 at this toolchain, on these runner images and devices and on the founder's machine.
