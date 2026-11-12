# Device baseline

Throwaway spike of the rendering research. It opens every device a machine offers through Direct3D 12, Vulkan and Metal, each reached through the system's loader with no third-party graphics crate, and measures instead of assuming: that Direct3D 12 opens at run time and that the Windows software adapter gives it a device; which items of an assumed baseline each device reports, and what it reports beyond it; whether a scene with filtered textures, blending, depth and multisampling renders to the same bytes twice, in a second process, under the settings of a software device, and whether its commands were recorded by one thread or by four; how far two devices are from one another on that scene; whether each interface's validation layer delivers its messages to the program; whether a texel written through a view that converts nothing arrives with the bytes of its source; and what a lit scene costs a software device when it is written to one image and when it is written to a wide geometry buffer.

It extends the [offscreen rendering spike](../render-offscreen/README.md) by its method. It is evidence, never product code.

## Layout

- `src/backend.rs`: what each interface implements and reports. Its documentation is the specification of the four scenes (targets, formats, samplers, pipelines, the order of draws, the lighting arithmetic), written once so that three independent implementations render the same thing.
- `src/scene.rs`: the inputs, the image type, its hashes and its PNG form. Every input is built with integer arithmetic or with float operations that are correctly rounded everywhere (no transcendental function), so the bytes handed to a device are the same on every machine; one hash over all of them is checked by a self-check and reported by every run.
- `src/vulkan.rs` and `src/vulkan/`: Vulkan through `libvulkan.so.1`, `vulkan-1.dll` or a loader library on macOS, the GLSL compiled at run time by the glslang command-line tool.
- `src/d3d12.rs` and `src/d3d12/` (Windows): Direct3D 12 through `d3d12.dll` and `dxgi.dll`, the COM interfaces called through their virtual tables, the HLSL compiled at run time by `D3DCompile`.
- `src/metal.rs` and `src/metal/` (macOS): Metal through the Objective-C runtime, the Metal framework opened at run time, the Metal Shading Language compiled at run time by the system.
- `src/main.rs`: the measurements and the report, the same for every interface. It also holds the child process's side and the comparison of images across machines.
- `src/platform.rs`: the count of the process's threads and the restriction of a process to a number of processors.
- `src/loader.rs`, `src/imports.rs`: `dlopen` and `LoadLibraryW`, and the reader of the binary's own imports (copies of the offscreen spike's).
- `shaders/`: one set of sources per interface, named after what they shade.
- `setup.sh`: what a machine of the research CI needs installed (below).

## The scenes

- **The filtering and blending scene**: 256 by 256 pixels at 4 samples, 8-bit color in the sRGB encoding and 32-bit float depth, resolved and read back. Ten draws: three overlapping triangles drawn nearest, farthest, middle (the depth test, and edges at arbitrary slopes); 12 by 12 hashed texels at 8 pixels each through the nearest filter; a gradient magnified 3.4 times through the linear filter; a floor receding in perspective whose texture has a different tint per level, through the trilinear filter; three translucent discs blended over what is there, one of them between the depths of the triangles; sixteen slivers under two pixels wide, issued as an indirect draw. Three textures are stored in the sRGB encoding and converted when sampled, two are plain; all five are bound as one array. The scene is recorded three ways: in one command buffer; in four, one per chunk of draws, by one thread; in four by four threads at once.
- **The compute exercise**: 4 096 invocations read a storage buffer and write a second storage buffer and a storage texture of 32-bit integers with wrapping integer arithmetic, compared with the values computed on the processor.
- **The pass-through exercise**: a source of 32 by 32 texels in which every byte value occurs in every channel, drawn texel for pixel into the two halves of one target: sampled and written through views that convert nothing on the left, through views in the sRGB encoding on the right.
- **The lit scene**: a height field of 18 432 triangles under four point lights at 512 by 512, written to one 16-bit float image with the lighting in the fragment shader; written to a geometry buffer of 160 bits per pixel and to one of 192 bits per pixel with no lighting; and written to the geometry buffer of 160 bits and then lit by a second pass that reads it.

## Hypotheses

Stated before the first run. A hypothesis with no threshold is a figure to record.

1. Direct3D 12 is opened at run time and the Windows software adapter gives a device on both Windows cells of the research CI; the binary imports no graphics library on any cell.
2. Every device of the five cells reports each item of the assumed baseline, recorded item by item: render passes with several color attachments and a depth attachment, multisampling at 4 samples, compute passes, indirect draws, storage buffers, storage textures, arrays of textures, timestamp queries, asynchronous read-back.
3. What each device reports beyond the baseline is recorded per device, with no threshold: the texture compression families it decodes (BC, ETC2, ASTC), 64-bit atomic operations, a draw count read from a buffer, arrays of resources without a fixed bound, mesh shaders, ray queries and a second queue.
4. The filtering and blending scene renders to identical bytes twice in one process (two devices created from scratch), in a second process, under each setting a software device has (thread counts and processor capabilities), and in its three recordings.
5. Across devices the same scene differs by a bounded amount, recorded as the largest difference per channel and the count of differing pixels, with no threshold.
6. Each interface's validation layer, installed by the run's setup where a cell lacks it, delivers its messages to the program and reports a deliberate misuse.
7. A texel of the pass-through form arrives with the bytes of its source on every device, beside the converting form on the same target.
8. The lit scene written to one image and the same scene written to a geometry buffer of 160 and of 192 bits per pixel are timed on each software device, with no threshold: the expectation under test is that the wide targets cost a software device more.

## Method

The research CI builds and runs the binary on the five cells after `setup.sh`; locally it runs the same way (`ci/run-spike.sh render-device-baseline <cell>`). On a machine where an interface's loader or shader compiler is absent, the interface reports itself unavailable with the reason.

For every device of every interface the report carries: its name, kind, identifiers, driver and interface version, and a reference key made of them and of the device's settings; each baseline item and each item beyond it with the feature bits, limits, tiers or formats it was read from; the hash of the scene in each of the three recordings on a first device and on a second one created from scratch, with the render time, the read-back time, the number of times completion was polled and the time between two device timestamps around the pass; the hash of the image, a hash per block of an 8 by 8 grid, the count of nearest-filtered pixels that do not carry the bytes of their texel, and the count of drawn pixels per region; the same three hashes from a second process, and from one child process per setting of a software device (for Mesa's software device the thread count and the vector width through its environment variables; for the Windows software adapter, which documents no setting, the process restricted to one and to four processors before the device is created), with the difference from the first image where a hash differs; the compute exercise's mismatches; the pass-through exercise's differing texels and bytes for each half; the four timings of the lit scene, five repetitions each, from the start of the recording to the completion of the device's work; and what a child process created under the validation layer saw while it rendered the scene and after one deliberate misuse.

The images are published as PNG files next to the measurements. Images of several cells are compared after a run by the binary itself:

```bash
gh run download <run> --name render-device-baseline-all-cells --dir downloaded
cargo run --release -p render-device-baseline -- --compare downloaded
```

It groups the scene images into sets of identical bytes and prints, for each pair of sets, the count of differing pixels and bytes and the largest difference per channel.

`setup.sh` installs, on a Linux machine, the Vulkan loader, Mesa's Vulkan drivers, the GLSL compiler and the Vulkan validation layer from the distribution's packages. On a Windows machine of the CI it asks the system for the Graphics Tools optional component, which holds Direct3D 12's validation layer; outside the CI it changes nothing and says what is absent. macOS needs nothing.

The self-checks (`cargo test -p render-device-baseline`) verify the inputs' hash, the shape of the scene the interfaces are written against, the compute expectations, the image comparison and the PNG round trip, the thread count, and that every device of the machine renders the scene with something drawn in each region and computes exactly, or that its interface reports its absence.

## Environment

The five cells of the research CI, whose devices need no graphics processor: Mesa's software Vulkan device on the two Linux cells, the Windows software adapter through Direct3D 12 on the two Windows cells, the virtual Metal device of the macOS runner. For hardware devices, a developer machine per platform is a sample and no more: what the hardware devices of the engine's boundary offer at large is read from their makers' documents, not measured here.

No third-party library is linked or vendored. The tools and components the runs use are the system's or the distribution's: Mesa (MIT license), the glslang command-line tool (BSD and Apache 2.0 licenses), the Khronos Vulkan validation layers (Apache 2.0 license), the Direct3D 12 runtime, its software adapter, its shader compiler `d3dcompiler_47.dll` and its Graphics Tools component (parts of Windows), Metal and its compiler (parts of macOS).

## Result

Not measured yet.
