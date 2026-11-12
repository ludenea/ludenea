# Prepared collision geometry

Throwaway spike of the assets and content pipelines research. A collision shape built from a large input (a triangle mesh with its acceleration structure, a convex hull, a chain of segments) is the one part of a physics world that an importer could build ahead and ship as an asset. The spike measures, instead of assuming, with the two physics libraries under study (Box2D v3.2.0 and Rapier 3D 0.36.1): whether such a prepared form exists for each shape kind, whether its bytes are the same on every cell, whether a world that reads a form written on another cell steps like a world that built its shapes itself, and what building the shape outside the simulation step saves. The answers decide, per kind, between a prepared form shipped with the content, a prepared form kept as a cache of each machine, and a plain build at load.

Nothing here is the engine's importer, asset format or physics boundary.

## Hypothesis

Stated before the run. One pinned Rust toolchain, the default target features, one thread, a fixed step of one sixtieth of a second (16.67 ms).

The first step, per library and per kind, before any other: does the library expose a form of the shape that is built outside a world, written and read back? "No prepared form for this kind" is a recorded result. The build is then an action at load on each machine, and the two hypotheses on portability do not apply to that kind.

- **Portable bytes.** The prepared form of each shape kind, written on each cell, has one hash across the five cells. A difference refutes it for that kind, and a refuted kind is built on each machine and never shipped.
- **Portable behavior.** A world whose shapes are read from a form written on another cell steps to the same hashes, for 600 steps, as a world that built them locally. A differing hash refutes it, which would make the prepared form an artifact of one cell or forbid it for that kind.
- **What it saves.** The time to build each kind from 1 000, 10 000 and 100 000 input elements, against the time to read its prepared form, is recorded beside the fixed step. The mechanism is justified where a build exceeds one step. Where no build does, the prepared form is dropped for that kind as needless.

Informational, not a hypothesis: the size of each form, what was built from each input (triangles, hull vertices, polygons, chain segments), whether two builds in one process give the same bytes, whether a form read back and written again gives the same bytes, the count of bytes by which a form from elsewhere differs from the local one, and, for the Box2D chain, the figures of the image of a whole world that holds it.

## Method

Two programs. `src/` is the Rapier half, a member of the spikes' Cargo workspace, and the parent of the other half: `inputs.rs` generates the inputs, `kinds.rs` holds the library calls of each kind, `worlds.rs` the worlds and their hashes, `measure.rs` the measurement of one kind, `box2d.rs` runs the other program, and `main.rs` writes the report. `box2d/main.c` is the Box2D half, a C17 program that `setup.sh` builds with CMake before the Cargo build. The parent runs it as a child process, embeds its report under the key `box2d`, and records a missing or failing child without failing itself (`"box2d": {"ran": false, "reason": ...}`, and the top-level `complete` is false).

Inputs. Every input is a function of its size and of a seed, drawn from the SplitMix64 generator of the reference workload (ported as written to C), with sums and single products only and no transcendental function, so that every cell builds from the same input bytes. The hash of the input bytes is recorded per kind and size so that the cells can be compared on their inputs as well. Hashes are FNV-1a 64 as in the spikes before this one.

The ladder is 1 000, 10 000 and 100 000 input elements. A timed build or read is repeated 7 times in an optimized build and 3 times in an unoptimized one, and the report gives the median, the fastest and the slowest in milliseconds. A build is timed around the library's call alone, and a read is the form's bytes in memory turned back into a shape: the file system is in neither.

Rapier 3D, the kinds:

- A triangle mesh with its acceleration structure, in two variants reported separately: with no flag (`TriMeshFlags::empty()`, the bounding volume hierarchy alone) and with `TriMeshFlags::FIX_INTERNAL_EDGES`, which the library asks for so that bodies do not catch on the inner edges of a ground. The input is a terrain grid of quads of one unit, each split into two triangles, heights within plus or minus 0.3: 20 by 25 quads (1 000 triangles), 50 by 100 (10 000) and 200 by 250 (100 000). The shape is built by `TriMesh::with_flags`, the buffers copied outside the timed region because the library takes them by value.
- A convex hull, built by `ConvexPolyhedron::from_convex_hull` from 1 000, 10 000 and 100 000 points drawn in the cube that holds the unit ball, kept when inside the ball, then scaled by (1.0, 0.6, 0.8). The count of hull vertices is recorded.
- For both, the prepared form is the `bincode` bytes of the shape: the two types derive serde's traits under the library's `serde-serialize` feature. The first step is answered by doing it: `prepared_form` is "yes" once a shape was built with no world, written and read back at every size.

Box2D, the kinds:

- A convex polygon. The first step is recorded as measured facts: `b2ComputeHull` is called on the whole input as one cloud (it returns an empty hull above `B2_MAX_POLYGON_VERTICES`, 8 points) and on 8 points (accepted). A `b2Polygon` made by `b2MakePolygon` is a plain value built with no world, and the library has no function that writes or reads one, so the prepared form is the spike's own bytes: the count of polygons, then every field of each, in little-endian order, never the memory of the struct. The ladder as the library takes it, which is this spike's reading: N input points are N/8 clouds of 8 points (125, 1 250 and 12 500 polygons), each cloud around its own center, each point a corner of an octagon moved by up to an eighth of a unit on each axis, so that most hulls have 6 to 8 points. The count of polygons and of clouds the library refused is recorded.
- A chain. The first step is a reading of `include/box2d` and `src/shape.c` at the tag: a chain exists only as chain segment shapes created on a body of a world by `b2CreateChain` (or segment by segment by `b2CreateChainSegmentShape`), and no function builds a chain outside a world or writes or reads one, so the record is `prepared_form: "none"` and the two hypotheses on portability do not apply. What is measured is the build at load: `b2CreateChain` alone, on a static body of a new world each time, for an open chain of 1 000, 10 000 and 100 000 points (a terrain polyline, x growing by half a unit, y within plus or minus 0.3, the two ghost points set).
- Beside the chain, and named as such in the report because it is not a prepared form of the kind: the one written form the library has that holds a built chain is the image of a whole world (`b2World_GetSnapshot`, `b2CreateWorldFromSnapshot`). For a world that holds only the static body and its chain, at each ladder size: the image's bytes and hash, the time to take it (the size query and the write, as the Box2D snapshot continuation spike times them), the time to create a world from it, and whether two worlds built the same way in one process give the same bytes.

Worlds of the behavior measurement, each with the shapes of the smallest ladder size, 600 steps, the hash of every body's position and rotation after every step and a run hash over the step hashes:

- Rapier, for each mesh variant: a fixed body that carries the terrain mesh and 150 dynamic bodies (balls, cuboids and capsules in turn) placed over it on a grid, each moved by the seeded generator, dropped from staggered heights. For the hull: a fixed ground cuboid and 150 dynamic bodies that each carry the hull, scaled by one half with the library's own `scaled` so that a body is about one unit across. Gravity 10 downwards, the library's default integration parameters, sleeping left on, the bodies sorted by handle and hashed field by field.
- Box2D, 4 substeps and 1 worker: a static ground box and one dynamic body per polygon (125). For the chain: a world created from the image, to which the same 125 bodies, built locally, are then added, against a world whose chain was created locally. The bodies are found through a broadphase query and sorted by their stored identifier, as the Box2D snapshot continuation spike does.
- The sources of the shape, each a separate world compared with the first: built locally; read from the form this run wrote, through its file; and read from the form of every directory under `forms-from-other-cells/`. Per source: whether the form was read, whether its bytes equal the local form's and by how many bytes they differ, the first step whose hash differs from the locally built world (or -1), and the run hash. `bodies` counts every body hashed, the fixed one included.

Forms from other cells take two runs. The first run writes, on each cell, the forms of the smallest ladder size under `out/<cell>/forms/` (`rapier-triangle-mesh.bin`, `rapier-triangle-mesh-fixed-internal-edges.bin`, `rapier-convex-hull.bin`, `box2d-convex-polygons.bin`, `box2d-chain-world-image.bin`), which the research CI publishes with the measurements. They are then committed under `forms-from-other-cells/<cell>/`, and the second run reads them on every cell. With no such directory the report says that no form from another cell was present.

Not built as specified, and why:

- Standard C lists no directory, so the parent lists `forms-from-other-cells/` and names the cell directories after it on the child's command line.
- A Box2D chain is one-sided, its solid side on the right of the walk, so the polyline is handed to the library from its highest x to its lowest. The spike's self-check fails when the bodies fall through it.
- As read in Parry's source and checked by a test, `FIX_INTERNAL_EDGES` merges duplicate vertices and computes pseudo-normals, and computes no half-edge topology.
- The report adds, per source, whether the form was read and the count of bytes that differ from the local form, and durations are printed to a tenth of a microsecond.
- `--quick` divides the ladder, the steps and the Rapier body count by ten for a fast local check, and its forms compare with no other run's.

Not measured: the reduction of a cloud above eight points to polygons Box2D takes, which is the importer's own computation; the time to write a form; a build on several threads; Rapier 2D.

Checks of the driver's own logic, not of the hypotheses: `cargo test -p physics-prepared-geometry` (the hash and generator vectors, the inputs, two builds giving the same bytes, a form read back giving a world that steps like the built one, a form from another directory read and compared, the worlds catching their bodies, the report) and `physics-prepared-geometry-box2d --self-check` (the same vectors, a polygon written and read back equal field by field, a damaged form refused, the chain and the ground catching their bodies). `ci/run-spike.sh physics-prepared-geometry <cell>` builds and runs both halves and saves `facts.json`, `environment.txt` and `setup.txt`.

Relationship to the external components: Box2D v3.2.0 (MIT) and Rapier 3D 0.36.1 with Parry 0.31.1 (Apache 2.0) are the subjects of the measurement, studied, integrated nowhere. `bincode` 1.3.3 (MIT) is the serializer the measurement needs. Box2D is fetched by CMake at its tag and the crates by Cargo at exact versions: nothing is vendored.

## Environment

Research CI, workflow `research-ci.yml`, five cells (Linux x86_64 and ARM64, Windows x86_64 and ARM64, macOS ARM64, see [`spikes/README.md`](../README.md)), run on demand with `-f spike=physics-prepared-geometry`, in release and in unoptimized builds (`SPIKE_PROFILE=dev`, which builds the Rust half with the `dev` profile and the Box2D half in the Debug configuration, in a build directory of its own). The Rust half is built with the pinned toolchain (Rust 1.99.1), default target features, with Rapier's `enhanced-determinism` feature and without its `parallel` feature.

The Box2D half is built with the default C compiler of each image (GCC on the Linux cells, Apple Clang on macOS, MSVC on Windows x86_64), `-ffp-contract=off` on GCC and Clang, Box2D's own CMake flags otherwise. On Windows ARM64 it is built with `clang-cl` and the Ninja generator: the engine's determinism contract admits MSVC for foreign code on x86_64 only, and Box2D v3.2.0 as released compiles with `clang-cl` for ARM64 only with `/FIintrin.h /DARM64_FPCR=0x5A20` (see the Windows ARM64 section of the [Box2D worker sweep](../box2d-worker-sweep/README.md)). `setup.sh` sets that compiler when none is named and prints the compiler CMake found.

A magnitude (a time) is judged on the founder's machine (AMD Ryzen 9 9950X3D, 16 cores and 32 logical processors, Windows 11, MSVC for the Box2D half) outside the matrix. The cells give the ordinal reading and decide identity.

## Result

Not measured yet.
