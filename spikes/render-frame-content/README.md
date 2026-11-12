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

Every measurement runs on the content device in the parent process. Those that need pixels or a device's time (the modes, the pace, the worlds, the loss, the unwatched world, the shader payload) run again in a child process on each device of the machine that needs no graphics processor: Mesa's software device through Vulkan on the Linux cells, the Windows software adapter through Direct3D 12 on the Windows cells, the machine's virtual device through Metal on the macOS cell. The lifetimes run in a second child, started with the validation layer's environment.

- **Content hash**: a world is stepped to tick 30 and a reproducible frame is prepared at the factor 0.5, then to tick 60 at 0.25, with the world and the renderer at 1, 4 and 8 workers, once with each arithmetic. Prepare writes each view's list in parts, one per chunk of the render slice and per task, and merges them in chunk order before the sort.
- **Cost of the math layer**: 31 repetitions of prepare alone (160 tasks, one per view and chunk), the two arithmetics in turn, at worker budgets of 1 and 4. The two differ by the transcendental functions only (an arc tangent and a sine and cosine per item, a logarithm per mesh, a few per view); every other operation is exact under both and written once.
- **Modes**: the same reproducible frame in the ordinary mode (a view's pass recorded in 8 parts by the pool's workers at once, culling on, unread passes removed), in the reference mode, and with each of the four switches alone, at two ticks. In the reference mode every pass is recorded in one part by one thread, pass after pass; the selection masks are drawn; each transient is cleared to a marker color by a pass of its own before its first use.
- **Graph**: 300 repetitions of the declaration and compilation of a synthetic graph: each pass creates two transients and reads up to three that earlier passes wrote, one pass in five feeds nothing, the last writes the one output.
- **Pace**: 180 ticks due every sixtieth of a second. A tick that is due runs before a frame is started; its lateness is the time between its due instant and its start. Two rates of frames (one frame after each tick, and frames back to back), two worker budgets (1 and 4), and the presenter as a presentation thread (recording, submission and read-back overlap the next ticks) or served from the stepping thread (the loop goes on when the image is read back).
- **Worlds**: 1, 4 and 16 worlds on one instance, each with its own stepping thread and a worker budget of one, each stepping one tick then drawing one frame and waiting for its image. The frame cost per world is the mean time from a frame's start to its image.
- **Loss**: a control world is drawn at four ticks. A second world is drawn at the first two; while a frame of the third is on its way the port starts answering "lost", drops the device behind it and makes every handle stale. The capability creates the device again and fills its caches by the path that filled them at startup; the world subscribes anew and is drawn at the third and fourth ticks.
- **Unwatched world**: one world draws an interactive frame every 10 ticks on the content device and every 500 on a device that draws, for 10 000 ticks; another is stepped 10 000 ticks with no frame. Both are then asked the reproducible frame of the last tick. The interactive frames keep a declared history, the level of detail each view last drew a mesh at; the tile blocks of the watched world were rebuilt as gameplay wrote them.
- **Lifetimes**: at the port, with no world. A buffer is released while three submitted frames name it; a command names a buffer already released; a buffer is destroyed at once under a frame the device has finished but whose completion nobody observed; a buffer is destroyed at once right after the submission of a frame of 4 000 draws. The last is a real misuse of a device and may end the process: each step is written to the report before the next.
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

Research CI, workflow `research-ci.yml`, five cells (see [`spikes/README.md`](../README.md)), run on demand with `-f spike=render-frame-content`. Rust toolchain pinned in [`rust-toolchain.toml`](../rust-toolchain.toml), release profile, default target features. The setup script is the device baseline spike's. The founder's Windows machine judges the claim on sixteen worlds and runs the hardware devices.

## Result

Not measured yet.
