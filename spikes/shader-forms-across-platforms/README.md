# Shading forms across platforms

Throwaway spike of the rendering research. One small shader set goes through the two kinds of chain the research compares, on each development platform, to the four forms a graphics device takes (SPIR-V, DXIL, a Metal library, Metal text):

- **Slang**: its compiler run as a tool, which calls the DirectX shader compiler for DXIL and Apple's compiler for a Metal library.
- **A front end to SPIR-V with a translator**: the glslang tool makes the SPIR-V, which is the Vulkan form; a translator writes HLSL and Metal text from it; the DirectX shader compiler makes DXIL from the HLSL and Apple's compiler a Metal library from the text. The chain is run twice, once with SPIRV-Cross and once with naga as its translator.

It is evidence, never product code. Every tool and library here is a candidate under study, run or linked inside this spike only: nothing here adopts one.

## Layout

- `src/main.rs`: the shader set, the measurements and the report.
- `src/chains.rs`: the tools found on the machine and each step of each chain.
- `src/json.rs`: a reader of JSON documents, for the reflection Slang writes.
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

`setup.sh` runs the device baseline spike's setup, then fetches from their published releases Slang's compiler for the five cells, the DirectX shader compiler where Microsoft publishes one (Windows on both architectures, Linux on x86_64) and the glslang tool for Windows and macOS, the Linux cells taking the distribution's package. Each license was read before the first run.

The self-checks (`cargo test -p shader-forms-across-platforms`) verify the shader set and its sources, the composition of a lit stage with the lighting module as written and changed, the reading of a reflection document and the JSON reader.

## Environment

The five cells of the research CI.

Relationship to external components, each studied here and none adopted: Slang 2026.22 (Apache 2.0 license with the LLVM exception), the DirectX shader compiler 1.9.2609 (University of Illinois/NCSA license) and the glslang tool 16.6.0 or the distribution's (BSD, MIT and Apache 2.0 parts) are tools run as child processes; naga 30.0.1 (MIT or Apache 2.0 license) and SPIRV-Cross through the `spirv-cross2` crate 0.7.2 (Apache 2.0 license; the crate MIT or Apache 2.0) are libraries linked into this spike only; Apple's Metal compiler is part of Xcode.

## Result

Not measured yet.
