# Shading forms across platforms

Throwaway spike of the rendering research. One small shader set goes through the two kinds of chain the research compares, on each development platform, to the four forms a graphics device takes (SPIR-V, DXIL, a Metal library, Metal text):

- **Slang**: its compiler run as a tool, which calls the DirectX shader compiler for DXIL and Apple's compiler for a Metal library.
- **A front end to SPIR-V with a translator**: the glslang tool makes the SPIR-V, which is the Vulkan form; a translator writes HLSL and Metal text from it; the DirectX shader compiler makes DXIL from the HLSL and Apple's compiler a Metal library from the text. The chain is run twice, once with SPIRV-Cross and once with naga as its translator.

It is evidence, never product code. Every tool and library here is a candidate under study, run or linked inside this spike only: nothing here adopts one.

## Layout

- `src/main.rs`: the shader set, the measurements and the report.
- `src/chains.rs`: the tools found on the machine and each step of each chain.
- `src/device.rs`: the device baseline's scene drawn natively with the forms a chain made.
- `src/json.rs`: a reader of JSON documents, for the reflection Slang writes.
- `forms-from-other-cells/`: the scene's forms as a first run made them on two cells, inputs of the draws on the other cells.
- `shaders/glsl/` and `shaders/slang/`: the shader set in the source language of each kind of chain.
- `setup.sh`: fetches the tools from their published releases (below).

## The shader set

- **The scene of the device baseline spike**, with the bindings its three native interfaces expect: five textures as one array, three samplers as another, per-draw constants. In GLSL it is the baseline's own source; in Slang it is written for this spike.
- **A lit surface composed with a lighting module**, in four variants of the surface function: one that samples a texture at its interpolated coordinate, one that also reads the view, one whose coordinate it computes itself (scaled, then rotated, through functions that carry the coordinate's differentials), and one whose material moves vertices. In GLSL the composition is by text: the lighting module, the surface and a template are concatenated before the front end. In Slang the lighting is a module, each surface is a module of the same name, and the template imports both: the module search path of the compile chooses the surface.
- **A compute shader**: the device baseline's compute exercise.
- **A mesh shader** that emits one triangle and **a compute shader with a ray query**, compiled and never run.

## Hypotheses

Stated before the first run. A hypothesis with no threshold is a figure to record.

1. Which of the four forms each chain produces on each of the three development platforms is recorded, pair by pair, with no threshold.
2. A form produced on one cell is accepted by the device of another and renders the device baseline's scene to the bytes of the form produced at home: DXIL made on Linux on the Windows software adapter with no developer mode, a Metal library made on Windows and Metal text made on Linux on the macOS cell.
3. A reflection in one format, built from each chain's output, names the same stages, parameters and kinds for the four forms.
4. A change of the lighting module changes every payload composed with it and nothing else.
5. The surface function with a computed coordinate gives the same image compiled for a raster pass, with the hardware's differentials, and for a compute pass, with computed ones, within a bounded difference the test states; in the raster pass the engine's sampling is timed against plain sampling, with no threshold.
6. Each chain reports the inputs a function uses, well enough for an importer to derive whether a surface function reads the view and whether it moves vertices.
7. The time of each import, and of the platform's compile of Metal text at load, is recorded, with no threshold.

## Method

The research CI runs `setup.sh`, builds the binary and runs it on the five cells; locally it runs the same way (`ci/run-spike.sh shader-forms-across-platforms <cell>`). On a machine where a tool is absent, the forms that need it are recorded as not made, with the reason.

An import is one stage of one shader through one chain to one form: 13 stages, 3 chains and 4 forms. Every import is made three times: twice from the same sources, to see whether its bytes repeat, and once with one constant of the lighting module changed, to see which payloads change. The payloads are told apart in three kinds: those of a stage that calls the lighting, those of a stage composed with the module that calls nothing of it, and those of shaders composed with no module.

The reflection of the translated chains is read from the SPIR-V every form of theirs is made from, once through naga and once through SPIRV-Cross, and the spike checks that each translator keeps the parameters' names in the text it writes. The reflection of Slang is the document its compiler writes for each form, reduced to one line per stage and per parameter and compared across forms; the documents are published with the measurements.

Whether a chain tells which inputs a surface function uses is asked three ways: of naga's analysis, which says per function which parameters it reads through the functions it calls, on the lit shader itself; of SPIRV-Cross's list of the parameters an entry point uses, on a probe, a stub entry point around the surface function alone; and of Slang's reflection document on the same probe, with its SPIR-V read through SPIRV-Cross beside it.

The scene is then drawn natively, by the device baseline spike's own code over Vulkan, Direct3D 12 and Metal, on every device of the machine, with the scene's two stages in each form its interface takes, in place of the shaders that code compiles itself: the forms the run made, and the forms a first run made on the Linux x86_64 and Windows x86_64 cells, committed under `forms-from-other-cells/`. Each image is compared, byte for byte, with the image of the native path's own shaders on the same device. On Metal the time to make the two libraries and the two pipelines is taken, and where Apple's compiler is present every payload of Metal text a chain made goes through it.

`setup.sh` runs the device baseline spike's setup, then fetches from their published releases Slang's compiler for the five cells, the DirectX shader compiler where Microsoft publishes one (Windows on both architectures, Linux on x86_64) and the glslang tool for Windows and macOS, the Linux cells taking the distribution's package. Each license was read before the first run.

The self-checks (`cargo test -p shader-forms-across-platforms`) verify the shader set and its sources, the composition of a lit stage with the lighting module as written and changed, the reading of a reflection document and the JSON reader.

## Where the spike departs from its specification

- **The surface with a computed coordinate is compiled and not run.** The measurement of its image in a raster pass against a compute pass, and of the engine's sampling against plain sampling, is not built: the hypothesis on it is not measured.
- **A form reaches another cell through the repository.** The research CI has no step that hands one cell's artifacts to another, so the forms of a first run are committed as inputs of the next.
- **naga's forms are not drawn natively.** Its HLSL binds every sampler through a heap of its own design and its Metal text binds an array of textures through an argument buffer, neither of which the native path implements; its SPIR-V form is the front end's, the same bytes as the other translated chain's. The backend comparison spike draws naga's output through wgpu, to the native path's bytes.
- **No Metal library is made on Windows**: Apple gives its tools for Windows behind a sign-in, which the specification foresaw.
- **The translated chains have one reflection for their four forms by construction**, read from the SPIR-V; what is checked of the other forms is that the parameters' names survive in the text.
- **The inputs a surface function uses are asked of SPIRV-Cross and of Slang on a probe**, a stub entry point around the function alone, since neither says it per function; naga does.

## Environment

The five cells of the research CI, and a developer machine (Windows 11, with developer mode off) for the DXIL made on Linux.

Relationship to external components, each studied here and none adopted: Slang 2026.22 (Apache 2.0 license with the LLVM exception), the DirectX shader compiler 1.9.2609 (University of Illinois/NCSA license) and the glslang tool 16.6.0 or the distribution's (BSD, MIT and Apache 2.0 parts) are tools run as child processes; naga 30.0.1 (MIT or Apache 2.0 license) and SPIRV-Cross through the `spirv-cross2` crate 0.7.2 (Apache 2.0 license; the crate MIT or Apache 2.0) are libraries linked into this spike only; Apple's Metal compiler is part of Xcode.

## Result

Measured on 2026-11-12 on the research CI ([first run](https://github.com/ludenea/ludenea/actions/runs/43225378062), whose forms are the inputs of the draws on other cells; [second run](https://github.com/ludenea/ludenea/actions/runs/43226939318), the first with the native draws; [third run](https://github.com/ludenea/ludenea/actions/runs/43227608968), after a correction of the spike: its change of the lighting module did not reach Slang's imports before it) and on a developer machine (Windows 11, developer mode off, an NVIDIA GeForce RTX 5080, an AMD Radeon integrated device and the software adapter) for the DXIL made on Linux. Slang 2026.22, DirectX shader compiler 1.9.2609, glslang 15.1.0 (Linux) and 16.6.0, naga 30.0.1, SPIRV-Cross at `aa217ae`, Apple's Metal compiler 32023.620. Rust 1.99.1, release profile. The five cells ran in each run; on the first, the two Windows cells lacked the DirectX shader compiler through a defect of the setup, corrected since. The raw measurements, the reflection documents and the scene's forms are in the runs' artifacts.

1. **Which forms each chain produces where**, as stages made of the 13 of the shader set:

   | Cell | Chain | SPIR-V | DXIL | Metal text | Metal library |
   | --- | --- | --- | --- | --- | --- |
   | `linux-x86_64` | Slang | 13 | 13 | 12 | 0 |
   | `linux-x86_64` | glslang and SPIRV-Cross | 13 | 13 | 13 | 0 |
   | `linux-x86_64` | glslang and naga | 13 | 10 | 10 | 0 |
   | `linux-arm64` | Slang | 13 | 0 | 12 | 0 |
   | `linux-arm64` | glslang and SPIRV-Cross | 13 | 0 | 13 | 0 |
   | `linux-arm64` | glslang and naga | 13 | 0 | 10 | 0 |
   | `windows-x86_64` and `windows-arm64` | Slang | 13 | 13 | 12 | 0 |
   | `windows-x86_64` and `windows-arm64` | glslang and SPIRV-Cross | 13 | 13 | 13 | 0 |
   | `windows-x86_64` and `windows-arm64` | glslang and naga | 13 | 10 | 10 | 0 |
   | `macos-arm64` | Slang | 13 | 0 | 12 | 6 |
   | `macos-arm64` | glslang and SPIRV-Cross | 13 | 0 | 13 | 13 |
   | `macos-arm64` | glslang and naga | 13 | 0 | 10 | 10 |

   What limits a form is the platform's tools before the chain: Microsoft publishes the DirectX shader compiler for Windows and for Linux on x86_64, and none for Linux on ARM64 nor for macOS; Apple's compiler exists on macOS, and on Windows behind a sign-in no runner can take, so no Metal library is made outside macOS. What a chain itself leaves out: naga refuses three stages of thirteen from SPIR-V (the compute shader, for a storage buffer declared write-only, the mesh shader and the ray query, whose SPIR-V extensions it does not read), and its HLSL writer ends the process, instead of returning an error, when per-draw constants have no place named; Slang refuses the mesh shader's source for Metal ("mesh output must be assigned as whole struct"); SPIRV-Cross writes every stage, the mesh shader and the ray query included. A text made is not a text accepted: on the macOS cell Apple's compiler takes the 13 texts of SPIRV-Cross, the 10 of naga and 6 of Slang's 12. It refuses Slang's six others ("cannot reserve 'buffer' resource location at index 0"): the sources give Direct3D registers, and Slang carries a register's number into Metal's one space of buffer indexes, where two constant buffers at the same register of two spaces, or a buffer at `t0` and another at `u0`, collide.
2. **A form made on one cell on the device of another: holds for every pair that could be made.** The DXIL made on the Linux x86_64 cell, by Slang and by the chain with SPIRV-Cross, is accepted by the Windows software adapter of the two Windows cells and, on the developer machine with developer mode off, by the NVIDIA device, the AMD device and the software adapter; each draws the scene to the bytes of the native path's own shaders. The hosted Windows cells are in developer mode, as their registry says; the spike enables no experimental feature of Direct3D 12 on either machine. The Metal text made on the Linux x86_64 cell and on the Windows x86_64 cell, by both chains, is accepted by the macOS cell's device and draws the scene to the reference bytes, as do the Metal text and the Metal libraries made on that cell. The SPIR-V made on the Windows x86_64 cell and on the Linux x86_64 cell draws to the reference bytes on both Linux cells. Not made, so not run: a Metal library made on Windows.
3. **One reflection for the four forms: holds for Slang as far as its document goes.** Reduced to stages and parameters with their kinds, the document Slang writes is the same for every form it makes of a stage, on every cell (13 stages of 13 where three forms are made, 12 of 12 where two are). The translated chains have one reflection by construction, read from the SPIR-V; naga and SPIRV-Cross read the same parameters from it, naga naming a block by its instance and SPIRV-Cross by its type. The slots are another matter and are the importer's on every chain: SPIRV-Cross takes them from its caller, naga from its caller or from its own scheme, Slang from the source's annotations.
4. **A change of the lighting module: holds for Slang; the translated chains change four payloads too many.** An import gives the same bytes twice on every chain and every cell. With one constant of the module changed, Slang changes every payload whose stage calls the lighting (12 of 12 on the cells that make three forms) and no other: none of the vertex stages of the same shaders, none of the shaders apart. The translated chains change every payload that calls it, none apart, and four more: the vertex stage's SPIR-V of each lit shader, composed with the module by text, in which the front end keeps the function no stage calls. Their translated texts of the same stages do not change. No payload stays stale; four are rebuilt for nothing.
5. **The surface with a computed coordinate, raster pass against compute pass: not measured.** The surface compiles through the three chains to every form its neighbours do; its two images and the cost of the engine's sampling were not measured (see the departures).
6. **The inputs a surface function uses: told by naga per function, by SPIRV-Cross on a probe, not by Slang's reflection document.** For the four surfaces, naga's analysis of the lit shader says which parameters the surface function reads, through the functions it calls, and SPIRV-Cross's list of the parameters an entry point uses says it of a probe: both give whether the surface reads the view and whether its material moves vertices as the sources have it, four of four. Slang's reflection document has no member that says whether a parameter is used; the SPIR-V Slang makes of the same probe, read through SPIRV-Cross, gives the four answers.
7. **The time of an import**, median per stage and form in milliseconds, a process started for each: Slang 175 to 296; the glslang tool with either translator 73 to 136, the DXIL costing 10 to 15 more; a Metal library 198 to 255, the first of a run 0.4 to 2.3 seconds. **The platform's compile of Metal text at load**, on the macOS cell's device: making the scene's two libraries from Slang's text takes 85 to 223 milliseconds, from SPIRV-Cross's text under one millisecond in the same runs, a gap whose cause was not looked for; from a Metal library under one millisecond, the two pipelines then taking 2 to 4. The first text a process loads took 764 milliseconds. As a tool, Apple's compiler takes 54 to 110 milliseconds a stage at the median.

One fact outside the hypotheses: on the third run the Linux x86_64 cell landed on a host with an Intel Xeon Platinum 8573C, where Mesa's software device, under the same Mesa and LLVM and the same vector width, draws the scene to another image (`1109dde3338ef346`) than on the AMD hosts of every earlier run (`e3a283950520fad1`). Every draw of that run agrees with that run's own reference. The size of the difference between the two images was not measured.

Not measured here: the computed coordinate in a raster pass and in a compute pass; a Metal library made outside macOS; DXIL made on macOS or on Linux for ARM64, for want of a published compiler; naga's forms on the native path; the mesh shader and the ray query in use on a device; a hardware device through Metal or Vulkan.
