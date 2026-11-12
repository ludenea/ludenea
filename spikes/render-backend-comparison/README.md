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

Not measured yet.
