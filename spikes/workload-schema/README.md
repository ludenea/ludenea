# Reference workload schema

Throwaway spike of the ECS research. It describes the [reference workload](../reference-workload/README.md)'s components, identifiers and globals by a schema served as data, lets a client that knows nothing of the storage read the state through that description alone and recompute the workload's tick hash from its view, and measures, instead of assuming, whether that hash equals the storage's hash at every tick, what serving and reading the state through the schema costs, and whether one compatible schema change (a field added with a default) leaves an old client reading correctly while a new client reads the field, including for a state saved before the change and loaded after it.

## Layout

- `src/contract.rs`: the contract description and its encodings. A description is data: named structure and enumeration types with stable names and versions, fields with types, optional defaults and documentation, the component types with the structural operations they admit, the globals, and the commands, queries and events. Values are encoded by their type in a canonical byte order (little-endian, fields in description order, a count before a sequence, a presence byte before an optional). The description itself is encoded in a fixed bootstrap format, the only thing a client must know before reading anything. The module names nothing of the storage.
- `src/client.rs`: the client. It imports the contract format only, never the workload crate. At session start it reads the description and resolves its script against it: the names of the structures and fields it wants (`world_state`, `identifier_slot`, `entity`, `position`, `velocity`, `mass` and their fields) with the types it expects, every other field being skipped by its described type. It then reads one `world_state` per tick boundary and recomputes the tick hash with its own FNV-1a over the identifier slots in order (generation, alive, and for an alive slot the five values of its entity), then the energy and the stream state. Script version 1 knows the five specified values; script version 2 also reads the field the compatible change added.
- `src/main.rs`: the server and the scenarios. The server's storage is the reference implementation's (identifier-indexed slots as structures of arrays), stepped by the sequential reference, with a `spin` column when the schema version has the added field. It serves the description's bytes and the `world_state` bytes (the identifier table, the alive entities in canonical order with their handle and components, the globals), and loads a `world_state` saved under one schema version into a server at another, a field the target has and the source lacks taking the target's default.

## Hypotheses

With the reference workload, version 1, default size (10 000 entities, 1 000 ticks), wander system off, compiled by one pinned Rust toolchain at default target features:

1. The schema is complete and the client never touches the storage: at every tick boundary from 0 to 1 000, the hash the client recomputes from the described values equals the storage's hash, and the client's run hash equals the sequential reference's, on every cell; every entity's handle matches its identifier slot's index and generation.
2. A field added to one component with a default (`spin` on the velocity, schema version 2) is a compatible change: the version 1 script reads a version 2 server and recomputes the same hashes, skipping the field by its described type; the version 2 script reads the field, and the hash of its values equals the storage's hash of the column; a `world_state` saved under version 1 at tick 500 loads into a version 2 server with the default in the added field, and both scripts then read it to tick 1 000 with the reference's hashes.
3. The breaking direction is named: the version 2 script against a version 1 description is refused with an error that names the missing field, and a field whose type differs from the script's expectation is refused naming the field and both types.
4. Serving the whole state through the schema costs a fraction of the tick: encoding the `world_state` (about 330 KB for 10 000 entities) and the client's read with its hash each cost under one sequential tick, measured per cell.

## Method

Three readings over 1 000 ticks (schema version 1 with script 1, version 2 with script 1, version 2 with script 2), each comparing at every tick the client's hash with the storage's and, at the end, the client's run hash with the sequential reference's; the load scenario (a version 1 save at tick 500 loaded under version 2, then read by both scripts to tick 1 000); the breaking direction (script 2 against description 1). The server's `world_state` encoder is written by hand for speed, and the self-check verifies that it equals the generic encoding of the same value tree by the description. Every reading reports the hashes, the first mismatching tick if any, the state and description bytes, and the mean microseconds of the encoding, of the client's read with its hash, of the storage's own hash and of the sequential tick.

The self-checks (`cargo test -p workload-schema`) verify on the small size that the description survives a round trip and rejects damage with a reason, that values encode and decode by type and that skipping matches decoding, that the hand encoder equals the generic one (with dead slots), that the client recomputes the storage hash at every tick for every version and script with handles consistent, that a version 1 save loads under version 2 with the default and keeps the reference hashes while respawned slots get the field's value, that the new script is refused by the old description naming the field, and that the report runs.

## Environment

Research CI, workflow `research-ci.yml`, five cells (see [`spikes/README.md`](../README.md)), run on demand with `-f spike=workload-schema`. Rust toolchain pinned in [`rust-toolchain.toml`](../rust-toolchain.toml), release profile, default target features of each target; plus a local run on an Apple M5 (10 cores, macOS 26) outside the matrix. The spike is sequential: timings compare per processor only.

## Result

Research CI [run](https://github.com/ludenea/ludenea/actions/runs/41692282727) of 2026-11-03 on four cells (AMD EPYC 9V45 on Linux x86_64, Neoverse N2 on Linux ARM64, AMD EPYC 7763 on Windows x86_64, Cobalt 100 on Windows ARM64), the macOS cell cancelled by the platform for capacity and re-run on demand the same day ([run](https://github.com/ludenea/ludenea/actions/runs/41695541304), a virtual Apple M1 with 3 CPUs), and a local run the same day on an Apple M5 (10 cores, macOS 26) outside the matrix. Every check holds on every cell. Mean microseconds per tick boundary over 1 000 ticks of the default size, wander system off, for the three readings (schema version 1 read by script 1, version 2 read by script 1, version 2 read by script 2):

| Cell | Machine | Encode the state | Client read with its hash | Storage hash | Sequential tick |
| --- | --- | --- | --- | --- | --- |
| `linux-x86_64` | AMD EPYC 9V45 | 47 to 53 | 458 to 476 | 229 to 234 | 1 230 to 1 250 |
| `linux-arm64` | Neoverse N2 | 60 to 67 | 593 to 617 | 224 to 225 | 1 116 to 1 126 |
| `windows-x86_64` | AMD EPYC 7763 | 91 to 108 | 735 to 892 | 313 | 1 606 to 1 612 |
| `windows-arm64` | Cobalt 100 | 116 to 127 | 698 to 737 | 225 | 1 118 to 1 130 |
| `macos-arm64` | Apple M1 virtual, 3 CPUs | 71 to 87 | 922 to 1 000 | 411 to 429 | 1 583 to 1 631 |
| local | Apple M5 | 36 to 40 | 380 to 402 | 224 to 228 | 757 to 772 |

- Hypothesis 1 holds: at every one of the 1 001 tick boundaries of every reading on every cell, the hash the client recomputes from the described values equals the storage's hash (zero mismatching ticks), every entity's handle matches its identifier slot (zero inconsistent ticks), and the client's run hash equals the sequential reference's `0x9f50cc4763ae733c`. The `world_state` weighs 330 024 bytes for 10 000 entities under version 1 (33 bytes per entity: 5 for the identifier slot, 8 for the handle, 20 for the three components) and 370 024 under version 2; the description weighs 1 206 and 1 287 bytes.
- Hypothesis 2 holds: the version 1 script reads the version 2 server with the same hashes, skipping `spin` by its described type; the version 2 script reads `spin` and its hash of the column equals the storage's on every tick; the version 1 save at tick 500 loads under version 2 with the default in the added field (the hash at load equals the saved and the reference, the column is all zeros), and both scripts then read the loaded server to tick 1 000 with the reference's hashes on every cell.
- Hypothesis 3 holds: the version 2 script against the version 1 description is refused with "field `spin` is not in `velocity`" on every cell; the self-check refuses a field of another type naming the field and both types.
- Hypothesis 4 holds: encoding the whole state costs 47 to 127 microseconds on the cells (4 to 8 percent of a sequential tick), the client's read with its hash 458 to 1 000 microseconds (37 to 61 percent of a sequential tick, 1.6 to 3.1 times the storage's own hash, since the client decodes 330 KB and hashes 210 KB of them through a plan resolved by name), both under one tick on every cell; the version 2 rows cost the version 1 script 2 to 6 percent more to skip.

The raw measurements and the environment of each cell are the artifacts of the runs. The result holds for the reference workload version 1 at this toolchain and on these images.
