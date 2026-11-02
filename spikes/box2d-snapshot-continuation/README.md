# Box2D snapshot continuation

Throwaway spike of the physics research, whose first stage studies Box2D v3.2.0 as a study object, not as an integration choice. It measures whether the library's experimental snapshots give identical continuation: a world restored from a snapshot must step exactly like the world that was never interrupted, which is what rollback, replay and deterministic save games need from a physics library inside the deterministic domain.

## Hypothesis

With Box2D v3.2.0 built as its author prescribes (contraction into fused multiply-add disabled, no fast math), on the author's own benchmark scenes:

1. Taking a snapshot does not perturb the world: the world that keeps stepping after the snapshot was taken stays identical to the uninterrupted run.
2. A world restored in place from the snapshot, a new world built from the snapshot with the same worker count, and a new world built from the snapshot with another worker count all reproduce the uninterrupted run at every step after the snapshot instant, at one and four workers.
3. The snapshot image and the continuation are identical across the five cells of the desktop boundary (Windows, macOS and Linux on x86_64 and ARM64, macOS on ARM64 only), as the worker sweep found the stepping to be.

Informational, not a hypothesis: the size of the snapshot image per body and the time to take and restore it, which bound a rollback cadence.

## Method

A C17 driver fetches Box2D at tag `v3.2.0` through CMake and compiles the library's shared benchmark scenes from the fetched tree. It runs five of the author's scenes that need no per-step driver (`joint_grid`, `large_pyramid`, `many_pyramids`, `smash`, `tumbler`), at 1/60 s with 4 substeps, through the library's own task system at 1 and 4 workers. For each scene and worker count:

- the uninterrupted run steps 300 times and hashes every body transform after every step (the broadphase query, body sort and FNV-1a 64 hash of the Box2D worker sweep spike), which gives the reference hashes;
- a second world runs the same first 100 steps, takes a snapshot (`b2World_GetSnapshot`, size query then fill), and continues for 200 steps (the first path);
- the same world is then restored in place from the image (`b2World_RestoreFromSnapshot`) and steps 200 times (the second path);
- a new world is built from the image with the same worker count (`b2CreateWorldFromSnapshot`) and steps 200 times (the third path);
- a new world is built from the image with the other worker count (4 for a run at 1, 1 for a run at 4) and steps 200 times (the fourth path).

Every path records whether the restore succeeded, whether the hash right after the restore equals the hash at the snapshot instant, the first step after the snapshot at which its hashes diverge from the reference (or none), its final hash, the size of the image, and the time to take and to restore it. `--self-check` verifies the driver's own logic (hash vectors, two identical runs, an in-place restore and a rebuilt world giving back the snapshot instant). `ci/run-spike.sh box2d-snapshot-continuation <cell>` builds it with the platform's default C compiler and saves `facts.json`, `environment.txt` and the CMake configure output.

Relationship to the external component: Box2D v3.2.0 (MIT) is the subject of the measurement; nothing of it is vendored or reused beyond this spike.

## Environment

Research CI, workflow `research-ci.yml`, five cells (see [`spikes/README.md`](../README.md)), run on demand with `-f spike=box2d-snapshot-continuation`. Release build with the default C compiler of each image, `-ffp-contract=off` on GCC and Clang, Box2D's own CMake flags otherwise; plus a local run on an Apple M5 (10 cores, macOS 26, Apple Clang) outside the matrix.

## Result

Local run of 2026-11-02 on the Apple M5: the 40 paths (5 scenes, 2 worker counts, 4 paths) all restore successfully, give back the hash of the snapshot instant, and reproduce the uninterrupted run at every one of the 200 steps that follow. Snapshot images: 4.3 MB for `tumbler` (2 027 bodies), 8.0 MB for `large_pyramid` (5 051 bodies), 13.0 MB for `joint_grid` (10 000 bodies), 19.2 MB for `smash` (9 601 bodies), 32.8 MB for `many_pyramids` (22 001 bodies), between 1.3 and 2.0 KB per body; taking one costs 0.3 to 7 ms and restoring one 0.1 to 2.5 ms on that machine.

The research CI result on the five cells is recorded here once the run completes; the raw measurements and the environment of each cell are the artifacts of the run.
