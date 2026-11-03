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

To be recorded from the research CI run.
