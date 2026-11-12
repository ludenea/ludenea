# Backend comparison

Throwaway spike of the rendering research. It renders the scene of the [device baseline spike](../render-device-baseline/README.md) through three paths on every device a machine offers, and measures what a layer adds beneath a render device: natively, by the device baseline spike's own code over Vulkan, Direct3D 12 and Metal, each reached through the system's loader; through [wgpu](https://github.com/gfx-rs/wgpu) at its lower level, the `wgpu-hal` crate, one unsafe interface over the three that validates nothing; and through wgpu at its safe level, the `wgpu` crate, which validates every call.

It is evidence, never product code. wgpu is a candidate under study: it is linked inside this spike only, and nothing here adopts it.

## Layout

- `src/main.rs`: the measurements and the report. Each path runs in its own child process per device, so that the threads and the libraries counted are those of one path.
- `src/safe.rs`: the scene through the `wgpu` crate.
- `src/lower.rs`: the scene through the `wgpu-hal` crate, with the transitions, the lifetimes and the destruction that level leaves to its caller.
- `src/ahead.rs`: the shaders compiled ahead, by tools run as child processes.
- `src/workload.rs`: the many draws workload, the log the process collects, and the shader compiler libraries a process has loaded.
- `shaders/`: the scene in WGSL (with one array of five textures and one array of three samplers, and with eight separate bindings), and the sources compiled ahead for wgpu on each interface.
- `setup.sh`: what a machine of the research CI needs installed; it runs the device baseline spike's own.

The native path is the device baseline spike's crate, used as a library: its contract (`backend.rs`), its deterministic scene (`scene.rs`) and its three interfaces. For this spike that crate gained a library target, the times at which a recording ends and at which its submission returns, and on Direct3D 12 a pipeline bound when it changes instead of before every draw, as its two other interfaces already did. Its measurements are unchanged: its self-checks give the images its README records.

## The paths and the recordings

The pass is the one the device baseline specifies: a target of 256 by 256 at 4 samples in the sRGB encoding with a 32-bit float depth, ten draws through nearest, linear and trilinear samplers, opaque and blended, one of them indirect, resolved and read back.

- **Serial**: one command buffer recorded and submitted by one thread.
- **Four workers and a fifth thread**: natively, the device baseline's own recording on four threads (secondary command buffers, command lists or a parallel encoder, inside one pass). Through wgpu, which has no command buffer that continues a pass, four command encoders on four worker threads, one pass each on the same targets (the first clears them, the others load them, the last resolves), then a fifth thread submits the four command buffers in order.
- **The many draws workload**: 10 000 draws in one pass on the same target, one quad of six vertices in each cell of a grid of 100 by 100, each with its own texture and sampler indexes, the pipeline changing 100 times. The time to record, the time of the submission and the wait for the device are taken apart, five repetitions after one that is not timed.

## Hypotheses

Stated before the first run. A hypothesis with no threshold is a figure to record.

1. A binary that links wgpu imports no windowing, graphics or audio library on any cell, and on a machine with no Vulkan loader it starts and reports no device.
2. The adapter wgpu gives when asked for its fallback is, on each cell, the device that needs no graphics processor there: Mesa's software device on Linux, the Windows software adapter on Windows, the machine's virtual device on macOS.
3. Command buffers recorded on four workers and submitted by a fifth thread give the bytes of a serial recording, through wgpu on the three interfaces.
4. Shaders compiled ahead (SPIR-V, DXIL, a Metal library) enter through wgpu's path for shaders it does not translate, on each interface, with no shader compiler library opened by the process; Metal text enters too, the platform compiling it.
5. The threads the process holds before a device exists, with one, and after it is dropped are counted on each path, with no threshold.
6. The time to record and submit 10 000 draws is recorded for the native path and for wgpu's two levels, with no threshold.
7. Under one device the bytes are the same through wgpu and natively.
8. Whether a compiled pipeline can be saved and loaded again through wgpu is recorded per interface, with no threshold.
9. The interface's validation layer, switched on beneath wgpu, delivers its messages to the program.

## Method

The research CI builds and runs the binary on the five cells after `setup.sh`; locally it runs the same way (`ci/run-spike.sh render-backend-comparison <cell>`). The imports are read from the binary itself. A first child process lists the adapters wgpu exposes at each level and asks each interface for its fallback adapter. Then, for every device, one child process per path and per form of shaders renders the scene in both recordings and times the many draws workload; the parent compares each image with the native path's image of the same device, byte for byte, and publishes the distinct images as PNG files. A child process per path counts its threads at each step while it opens one device and renders the scene once. A last pair of child processes switches the interface's validation layer on beneath wgpu, renders the scene, then commits one deliberate misuse.

The shaders compiled ahead are made on each cell, before any device exists, by its platform's own compiler run as a tool: SPIR-V by the glslang command-line tool, DXIL by the DirectX shader compiler of the Windows SDK, a Metal library by the Metal compiler of Xcode. After the adapters are listed, after the device is opened and after the pipelines are made, the child lists the libraries of its own process and reports those whose name marks a shader compiler.

A cell whose label contains `no-loader` installs nothing, which is how the case of a machine with no Vulkan loader is run:

```bash
gh workflow run research-ci.yml -f spike=render-backend-comparison \
  -f cells='[{"cell":"linux-x86_64-no-loader","runner":"ubuntu-24.04","container":"rust:1.99.1-slim-bookworm"},{"cell":"linux-arm64-no-loader","runner":"ubuntu-24.04-arm","container":"rust:1.99.1-slim-bookworm"}]'
```

The self-checks (`cargo test -p render-backend-comparison`) verify the shape of the many draws workload, the reading of the adapters a child lists, the pairing of a device across the three paths, the values a child prints, the sources the forms are made from, and that the process lists its libraries with no shader compiler among them.

## Where the spike departs from its specification

- **A shader compiled ahead for wgpu is not the native path's shader.** wgpu assigns registers and slots from the pipeline layout by rules of its own, and its documentation says not to expect bindings to work on this path except with SPIR-V. The sources under `shaders/` are written against what wgpu's translator writes for the same layout, which the spike saves next to its measurements. On Vulkan the fragment shader is the baseline's and the vertex shader differs by one sign; the baseline's own pair is run as well, as a form of its own, and compared.
- **On Metal the sources compiled ahead bind the five textures and the three samplers separately**, and choose among them in the fragment function: wgpu implements an array of textures there as an argument buffer of its own design. The WGSL binds arrays where the adapter offers them and eight separate bindings where it does not; the report says which.
- **The lower level takes shaders compiled ahead only**: it has no front end for WGSL, and running wgpu's translator by hand is the safe level's work. Where a cell cannot make a form, the lower level is not measured on that interface.
- **The deliberate misuse is committed beneath wgpu**, through the handle wgpu gives to the interface's own device: at the safe level wgpu's validation stops a misuse before the interface sees it, so only a call made beneath it can show whether the interface's layer still reports.
- **The threads are counted in a process of their own per path**, which opens one device and renders the scene once, and not in the process that times the many draws workload.

## Environment

The five cells of the research CI, whose devices need no graphics processor (Mesa's software Vulkan device on the two Linux cells, the Windows software adapter on the two Windows cells, the virtual Metal device of the macOS runner), the two Linux cells again in a container with no Vulkan loader, and a developer machine with hardware devices for Direct3D 12.

Relationship to external components: wgpu 30.0.1 (`wgpu`, `wgpu-hal`, `wgpu-types`, with its translator naga; MIT or Apache 2.0 license) is an integrated library inside this spike only, a candidate studied here and not adopted. The tools run are the system's or the distribution's: Mesa (MIT license), the glslang command-line tool (BSD and Apache 2.0 licenses), the Khronos Vulkan validation layers (Apache 2.0 license), the DirectX shader compiler of the Windows SDK and the Direct3D 12 runtime with its software adapter and its Graphics Tools component (parts of Windows and of its SDK), Metal and its compiler (parts of macOS and of Xcode).

## Result

Measured on 2026-11-12 on the five cells of the research CI ([first run](https://github.com/ludenea/ludenea/actions/runs/43221197296); [second run](https://github.com/ludenea/ludenea/actions/runs/43222446833), whose macOS cell found no runner, with [that cell alone](https://github.com/ludenea/ludenea/actions/runs/43225121841); [third run](https://github.com/ludenea/ludenea/actions/runs/43225528866)), on the two Linux cells in a container with no Vulkan loader ([run](https://github.com/ludenea/ludenea/actions/runs/43222453408)), and on a developer machine (Windows 11, an NVIDIA GeForce RTX 5080, an AMD Radeon integrated device and the software adapter). wgpu 30.0.1, Rust 1.99.1, release profile. The first run predates two corrections of the spike (the lower level on Metal, and what is collected of the validation layers); the figures below are the last run's of each cell, with the first run's beside them where a time is given twice. The raw measurements and the images are in the runs' artifacts.

1. **Imports: refuted on Windows and on macOS; holds on Linux.** The binary that links wgpu imports `user32.dll` and `dxgi.dll` on both Windows cells, and the Metal, QuartzCore and CoreGraphics frameworks on the macOS cell; on Linux it imports the C library, the math library and `libgcc_s` only. With no Vulkan loader, on both Linux architectures, it starts, wgpu lists no adapter at either level, and the process goes on.
2. **The fallback adapter: holds on Linux and on Windows, refuted on macOS.** Asked for its fallback, wgpu gives Mesa's software device on the two Linux cells and the Windows software adapter on the two Windows cells and on the developer machine. On the macOS cell it gives none: it names the runner's virtual device an integrated one, and its fallback is a device it names a processor. Through Vulkan on the developer machine, which has no software Vulkan device, it gives none either.
3. **Four workers and a fifth thread: holds.** On every device and at both levels, the four command buffers recorded by four workers and submitted in order by a fifth thread give the bytes of the serial recording: Vulkan on the two Linux cells, Direct3D 12 on the two Windows cells and on the three devices of the developer machine, Metal on the macOS cell.
4. **Shaders compiled ahead: they enter on the three interfaces; the process opens a shader compiler on Direct3D 12.** SPIR-V, DXIL, a Metal library and Metal text each enter at both levels and render the scene. On Vulkan and on Metal the process holds no shader compiler library at any step. On Direct3D 12 wgpu loads `d3dcompiler_47.dll` as soon as it lists adapters, at both levels and whatever the form of the shaders, and at the safe level it compiles a shader of its own when the device opens. Two facts on what "compiled ahead" asks through wgpu, both read from its translator's output and then measured: the sources had to be written against wgpu's own bindings (on Direct3D 12 every sampler of the device sits in one heap of 2 048 with a buffer of indexes per bind group; on Metal an array of textures is an argument buffer); and the SPIR-V of the native path renders the scene upside down through wgpu at both levels (54 146 pixels of 65 536 differ), since wgpu draws through a viewport of negative height.
5. **Threads**, counted in a process that opens one device and renders the scene once (at start, with the device open, after the scene, after the device is dropped, at the end):

   | Cell | Native | wgpu, lower level | wgpu, safe level |
   | --- | --- | --- | --- |
   | `linux-x86_64` and `linux-arm64` | 1, 11, 11, 1, 1 | 1, 11, 11, 10, 1 | 1, 11, 11, 10, 1 |
   | `windows-x86_64` | 4, 4, 8, 8, 8 | 4, 4, 14, 14, 14 | 4, 4, 14, 14, 14 |
   | `windows-arm64` | 4, 4, 8, 8, 8 | 4, 4, 10, 10, 10 | 4, 4, 13, 13, 13 |
   | `macos-arm64` | 1, 3, 3, 3, 3 | 1, 3, 3, 3, 3 | 1, 4, 4, 4, 4 |

   The threads are the software device's on Linux and on Windows. Through wgpu, Mesa's ten threads stay until the instance is dropped, where the native path ends them with its device. On the developer machine, listing adapters through wgpu leaves 17 threads in the process before any device is asked for: wgpu creates a device on each of the three adapters to read what it offers, and the drivers' threads stay; the native path holds 14 with the NVIDIA device open and 5 once it is dropped.
6. **The time to record and submit 10 000 draws**, fastest of five in microseconds, recorded by one thread (the first run in parentheses):

   | Cell and device | Native, record | Lower level, record | Safe level, record | Submit (native, lower, safe) |
   | --- | --- | --- | --- | --- |
   | `linux-x86_64`, Mesa's software device | 714 (574) | 720 (592) | 1 295 (1 299) | 4, 9, 7 |
   | `linux-arm64`, Mesa's software device | 919 (944) | 950 (955) | 1 839 (1 868) | 14, 18, 30 |
   | `windows-x86_64`, software adapter | 548 (544) | 905 (738) | 2 896 (2 601) | 72 588, 94 485, 72 541 |
   | `windows-arm64`, software adapter | 838 (834) | 908 (919) | 2 839 (2 876) | 67 285, 77 843, 73 794 |
   | `macos-arm64`, virtual device | 3 275 (3 621) | 2 562 (not run) | 4 092 (3 603) | 14, 11, 33 |
   | developer machine, NVIDIA through Direct3D 12 | 136 | 298 | 1 114 | 15, 30, 24 |

   The lower level records within 0 to 65 percent of the native path's time on the software devices and at twice its time on the hardware device; the safe level takes 1.8 to 5.3 times the native path's time on the Linux and Windows cells and 8 times on the hardware device, about 0.1 to 0.3 microsecond a draw. The Windows software adapter does its work inside the submission on every path, 65 to 95 milliseconds. On the macOS cell the native path is slower than wgpu's lower level: the device baseline's Metal code asks the runtime whether the object answers each message before it sends it, so its figure is that spike's cost and not an interface's. With four workers the recording takes 351 to 1 447 microseconds at the lower level and 757 to 1 856 at the safe level on the cells, and the fifth thread's submission 49 to 143 microseconds on Linux and macOS, thread start included.
7. **Bytes against the native path: holds.** Under one device the scene and the many draws workload are the native path's bytes through wgpu's safe level with WGSL and through both levels with shaders compiled ahead: on Mesa's software device on both architectures, on the Windows software adapter on both architectures and on the developer machine, on the macOS cell's virtual device, and on the NVIDIA and AMD devices of the developer machine through Direct3D 12. On the macOS cell the sources compiled ahead bind the textures and the samplers separately and give the same bytes as the array form of the WGSL.
8. **The pipeline cache**: offered through Vulkan only. On Mesa's software device the cache saved after the scene's two pipelines is 96 bytes and loads again (the scene made in 1.1 to 1.4 milliseconds through an empty cache and 0.7 to 1.0 through the loaded one); on the developer machine through Vulkan it is 24 135 bytes on the NVIDIA device (3.0 milliseconds, then 0.8) and 20 700 on the AMD one. wgpu offers no cache through Direct3D 12 nor through Metal.
9. **The validation layer beneath wgpu: holds on Vulkan, holds on Metal through the standard error only, refuted on Direct3D 12.** On the Linux cells the layer is silent on the scene and its two messages on the misuse arrive in wgpu's log, at both levels. On the macOS cell nothing arrives in the log: the layer writes to the standard error, as it does natively, two lines on the misuse and about a hundred while the scene renders, on bindings wgpu sets that the shaders do not use. On the Windows cells the layer's library is loaded and it reports: a handler of the spike's own collects two debug strings on the misuse and nine while the scene renders, one of them an error on wgpu's own use of the resolve target, which it resolves into before any clear. None reaches wgpu's log: a debug string of the spike's own, written in the layer's form, is seen by the spike's handler and not forwarded by wgpu's. The developer machine lacks the layer's component.

Not measured here: any hardware device through Vulkan natively or through Metal; the validation layers on a hardware device; the lower level with WGSL, which that level does not take; the SDL3 GPU API, which the specification adds only if the windowed presentation spike refutes the presentation thread; the cost of the safe level's validation apart from its tracking of resources.
