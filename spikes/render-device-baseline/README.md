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
- `src/lib.rs`: the modules above as a library, for a later spike of the rendering research that stands on this native path.
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

For every device of every interface the report carries: its name, kind, identifiers, driver and interface version, and a reference key made of them and of the device's settings; each baseline item and each item beyond it with the feature bits, limits, tiers or formats it was read from; the hash of the scene in each of the three recordings on a first device and on a second one created from scratch, with the render time, the read-back time, the number of times completion was polled and the time between two device timestamps around the pass; the hash of the image, a hash per block of an 8 by 8 grid, the count of nearest-filtered pixels that do not carry the bytes of their texel, and the count of drawn pixels per region; the same three hashes from a second process, and from one child process per setting of a software device (for Mesa's software device the thread count and the vector width through its environment variables, the vector width runs with its on-disk shader cache disabled; for the Windows software adapter, which documents no setting, the process restricted to one and to four processors before the device is created), with the difference from the first image where a hash differs; the compute exercise's mismatches; the pass-through exercise's differing texels and bytes for each half; the four timings of the lit scene, five repetitions each, from the start of the recording to the completion of the device's work; and what a child process created under the validation layer saw while it rendered the scene in its three recordings and after one deliberate misuse.

The images are published as PNG files next to the measurements. Images of several cells are compared after a run by the binary itself:

```bash
gh run download <run> --name render-device-baseline-all-cells --dir downloaded
cargo run --release -p render-device-baseline -- --compare downloaded
```

It groups the scene images into sets of identical bytes and prints, for each pair of sets, the count of differing pixels and bytes, the largest difference per channel and the differing pixels per region of the scene.

`setup.sh` installs, on a Linux machine, the Vulkan loader, Mesa's Vulkan drivers, the GLSL compiler and the Vulkan validation layer from the distribution's packages. On a Windows machine of the CI it asks the system for the Graphics Tools optional component, which holds Direct3D 12's validation layer; outside the CI it changes nothing and says what is absent. macOS needs nothing.

The self-checks (`cargo test -p render-device-baseline`) verify the inputs' hash, the shape of the scene the interfaces are written against, the compute expectations, the image comparison and the PNG round trip, the thread count, and that every device of the machine renders the scene with something drawn in each region and computes exactly, or that its interface reports its absence.

## Environment

The five cells of the research CI, whose devices need no graphics processor: Mesa's software Vulkan device on the two Linux cells, the Windows software adapter through Direct3D 12 on the two Windows cells, the virtual Metal device of the macOS runner. For hardware devices, a developer machine per platform is a sample and no more: what the hardware devices of the engine's boundary offer at large is read from their makers' documents, not measured here.

No third-party library is linked or vendored. The tools and components the runs use are the system's or the distribution's: Mesa (MIT license), the glslang command-line tool (BSD and Apache 2.0 licenses), the Khronos Vulkan validation layers (Apache 2.0 license), the Direct3D 12 runtime, its software adapter, its shader compiler `d3dcompiler_47.dll` and its Graphics Tools component (parts of Windows), Metal and its compiler (parts of macOS).

## Result

Measured on 2026-11-12 on the five cells of the research CI ([first run](https://github.com/ludenea/ludenea/actions/runs/43197512334); [second run](https://github.com/ludenea/ludenea/actions/runs/43201643120) on the two Linux and the two Windows cells, the macOS cell of that run having found no runner, and [its own run](https://github.com/ludenea/ludenea/actions/runs/43205233475) after it), on the two Linux cells inside the `rust:1.99.1-slim-bookworm` container ([run](https://github.com/ludenea/ludenea/actions/runs/43201693809)), and on a developer machine (Windows 11, an NVIDIA GeForce RTX 5080, an AMD Radeon integrated device and the software adapter). Rust 1.99.1, release profile. The raw measurements and the images are in the runs' artifacts.

| Machine | Interface | Device | Driver or implementation | Interface version | Scene image |
| --- | --- | --- | --- | --- | --- |
| `linux-x86_64` | Vulkan | llvmpipe (LLVM 20.1.2, 256 bits) | Mesa 25.2.8 | 1.4.318 | `e3a283950520fad1` |
| `linux-arm64` | Vulkan | llvmpipe (LLVM 20.1.2, 128 bits) | Mesa 25.2.8 | 1.4.318 | `a96b67d1d9209ee0` |
| `windows-x86_64` | Direct3D 12 | Microsoft Basic Render Driver | 10.0.26100.33648 | feature level 12_1, shader model 6.8 | `f81b63a351b9eedc` |
| `windows-arm64` | Direct3D 12 | Microsoft Basic Render Driver | 10.0.26100.9711 | feature level 12_1, shader model 6.8 | `f81b63a351b9eedc` |
| `macos-arm64` | Metal | Apple Paravirtual device | macOS 15.8.2 | family Apple5, Mac2, no Metal3 | `e46b7fa74288cad1` |
| developer machine | Direct3D 12 | Microsoft Basic Render Driver | 10.0.26100.9818 | feature level 12_1, shader model 6.8 | `f81b63a351b9eedc` |
| developer machine | Direct3D 12 | NVIDIA GeForce RTX 5080 | 32.0.16.1742 | feature level 12_2, shader model 6.8 | `1224e7a191051008` |
| developer machine | Direct3D 12 | AMD Radeon integrated device | 32.0.21042.62 | feature level 12_2, shader model 6.8 | `9c2c44cbc03f1459` |
| Linux cells in the container | Vulkan | llvmpipe (LLVM 15.0.6) | Mesa 22.3.6 | 1.3.230 | not rendered |

Vulkan is unavailable on the Windows cells and on the developer machine for want of a GLSL compiler on the path, and on the macOS cell for want of a loader; each reports its reason.

1. **Direct3D 12 at run time on the software adapter: holds.** `d3d12.dll`, `dxgi.dll` and `d3dcompiler_47.dll` are opened at run time, and the Microsoft Basic Render Driver gives a device at feature level 12_1 on both Windows cells and on the developer machine. The binary imports no windowing, graphics or audio library on any cell. DXGI lists the software adapter twice on every machine, the second entry without the software flag; the two give the same bytes.
2. **The baseline: held by the devices of four cells, one item missing on the fifth.** Mesa's software device on the two Linux cells and the Windows software adapter on the two Windows cells report every item. The macOS cell's virtual device reports no timestamp queries: its list of counter sets is empty, and only a command buffer's start and end times exist. Metal has no query for six of the items, which are reported from the device's GPU family. Outside the five cells, Mesa 22.3.6 in the container does not index an array of textures dynamically (`shaderSampledImageArrayDynamicIndexing` is 0), so the scene, written against that item, is not rendered there.
3. **Beyond the baseline**, as reported, none of it exercised:

   | Device | BC | ETC2 | ASTC | 64-bit atomics | Draw count from a buffer | Resource arrays without a fixed bound | Mesh shaders | Ray queries | Second queue |
   | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
   | llvmpipe, Mesa 25.2.8 (both Linux cells) | yes | no | no | yes | yes | yes | yes | yes | no |
   | Microsoft Basic Render Driver (both Windows cells) | yes | no format in the interface | no format in the interface | yes | yes | yes | yes | yes | yes |
   | Apple Paravirtual device | yes | yes | yes | no | no call in the interface | yes | no | no | yes |
   | llvmpipe, Mesa 22.3.6 (container) | yes | no | no | yes | yes | no | no | no | no |

   On the virtual Metal device the compiler accepts a kernel with a 64-bit atomic and the device then fails to build its pipeline. The hardware devices of the developer machine report every item Direct3D 12 can ask.
4. **Identical bytes within a device: holds everywhere it was rendered.** On each device the three recordings, on a first device and on a second one created from scratch, in a second process and under every setting, give one image: 21 renders of 21 on each Linux cell (one thread, four threads, vectors of 128 and of 256 bits), 15 of 15 on each Windows cell and on the developer machine's software adapter (one processor, four processors), 9 of 9 on the macOS cell and on each hardware device. The second run of each cell gives the first run's image; the two macOS runs landed on hosts in two data centers. Recording on four threads gives the bytes of one buffer on the three interfaces. Two facts on the settings: restricting the process to one processor takes the Windows software adapter's process from 8 threads to 7 or 6; and Mesa's on-disk shader cache does not tell vector widths apart, so that a process at 128 bits after one at 256 bits loaded code compiled for 256 and aborted with a corrupted heap in 21 runs of 30 on a local container, and in none of 30 with the cache disabled, which is how the two vector width runs are made.
5. **Across devices: six images, the largest difference 25 levels on one channel.** The Windows software adapter gives one image on three machines, two processor architectures and three versions of the adapter. Mesa's software device gives two, one per architecture, 11 448 pixels apart by at most 4 levels, 11 242 of them in the floor under the trilinear filter. Every pair of devices agrees on all 9 216 pixels of the nearest-filtered quad; the floor under the trilinear filter holds between 7 368 and 11 520 of each pair's differing pixels.

   | Images | Differing pixels of 65 536 | Largest difference per channel |
   | --- | --- | --- |
   | llvmpipe on x86_64 and on ARM64 | 11 448 | 4, 4, 4, 0 |
   | Windows software adapter and virtual Metal device | 10 532 | 6, 7, 7, 0 |
   | Windows software adapter and AMD Radeon | 9 954 | 4, 8, 4, 0 |
   | Windows software adapter and llvmpipe on x86_64 | 16 549 | 8, 8, 8, 0 |
   | Virtual Metal device and llvmpipe on x86_64 | 16 330 | 11, 10, 10, 0 |
   | AMD Radeon and llvmpipe on x86_64 | 16 461 | 11, 13, 8, 0 |
   | NVIDIA GeForce RTX 5080 and AMD Radeon | 14 635 | 17, 23, 15, 0 |
   | NVIDIA GeForce RTX 5080 and llvmpipe on ARM64 | 17 549 | 21, 25, 22, 0 |

6. **The validation layers: each delivers its messages to the program and reports the misuse.** Vulkan's layer, installed from the distribution's package, calls a function the program registers; Direct3D 12's layer, whose component the setup added on both Windows cells, calls a function registered on the device's information queue; both stay silent on the three recordings and report the misuse once. Metal's layer has no callback: started by an environment variable and told to log, it writes to the standard error of the process, which the spike points at a file and reads back; on the first run it reported the pipeline state set again to the same value before each draw, which the spike then stopped doing, and it is silent since. The developer machine lacks the Windows component and reports the reason.
7. **The pass-through form: holds on every device.** Each of the 1 024 texels arrives with its four bytes, and the converting form beside it on the same target returns every texel unchanged as well.
8. **The lit scene on the software devices**, fastest of five repetitions in milliseconds, two runs where a cell has two:

   | Cell and device | One image, lit | Geometry buffer of 160 bits, fill | Of 192 bits, fill | Of 160 bits, fill then light | Fill then light against one image |
   | --- | --- | --- | --- | --- | --- |
   | `linux-x86_64`, llvmpipe | 9.3, 9.4 | 7.0, 7.0 | 7.8, 7.2 | 10.9, 10.6 | 1.17, 1.13 |
   | `linux-arm64`, llvmpipe | 11.1, 11.1 | 10.7, 10.7 | 11.4, 11.4 | 16.2, 15.2 | 1.45, 1.37 |
   | `windows-x86_64`, software adapter | 11.5, 22.3 | 2.4, 4.9 | 2.6, 5.4 | 11.7, 24.1 | 1.02, 1.08 |
   | `windows-arm64`, software adapter | 16.6, 16.3 | 3.6, 3.6 | 3.9, 3.9 | 16.7, 16.7 | 1.01, 1.02 |
   | `macos-arm64`, virtual device | 0.7, 1.3 | 0.6, 1.0 | 0.7, 0.8 | 0.7, 0.9 | 0.96, 0.70 |
   | container on x86_64, llvmpipe of Mesa 22.3.6 | 14.4 | 9.4 | 9.9 | 14.4 | 1.00 |
   | container on ARM64, llvmpipe of Mesa 22.3.6 | 13.1 | 13.8 | 16.0 | 18.7 | 1.43 |

   On the software devices the fifth attachment costs 3 to 16 percent of the fill. The fill alone is cheaper than the lit image on every software device but one. Filling and then lighting costs the Windows software adapter 1 to 8 percent more than lighting in one pass, and Mesa's software device from nothing to 45 percent more. The lit image and the geometry buffer are the same bytes on both architectures for the Windows software adapter and for Mesa 25.2.8, and differ between the two architectures under Mesa 22.3.6.

Not measured here: any hardware device through Vulkan or Metal, and Direct3D 12's validation layer on a hardware device; a capability beyond the baseline in use (a mesh shader, a ray query, a 64-bit atomic that runs); the cause of the difference between Mesa's two architectures; the scene on Mesa 22.3.6.
