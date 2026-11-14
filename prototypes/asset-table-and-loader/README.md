# Asset table and loader

Throwaway prototype of the assets research. Never product code, and never a first version of the engine.

It builds, from the text of a design alone, a world's **asset table** and the **loading capability** that answers it, and checks that what was built gives the states the text says. An entity names an asset by a 128-bit identifier and asks for nothing; what a world loads, it loads because a **want** says so: a record of the table that names an asset, an owner and a policy (required or optional). An asset becomes available, or failed, by an **arrival** applied at a tick's input fixed point; it falls when no want names it; and a world does not step past a tick boundary while a required asset is unanswered (the **gate**). The loading capability resolves each want in the manifest the world stands under, the instance's current one or the manifest of a **content set** the world names, and posts the arrivals of a want's closure as one batch.

Two spikes measured the parts one by one. This prototype exists because what a reading of such a design gets wrong is how its mechanisms meet: the table with the gate, the loader with a content set, a restore with a revision.

## What it stands on

- The [asset table spike](../../spikes/workload-asset-table/README.md): its table, its world on the reference workload, its saved world read by another process. **Copied**, then changed where the design text states a rule the spike did not hold; the spike stays as measured. The prototype's first check is that spike's scenarios run again, with their oracles, on the changed table and world (`spike-check/`).
- The [import graph spike](../../spikes/asset-import-graph/README.md): its artifact cache, its action keyed by what it reads, its domain manifest, its revision put in place by one rename, its package and its mount with and without leave to override. **Copied**, without its sources and importers: the world this prototype steps reads the canonical part of the asset table spike, so the stand-in pipeline has one importer of its own, for a small text source. Nothing of the pipeline is measured here.
- The [reference workload](../../spikes/reference-workload/README.md), version 1, the repulsion parameters read through an asset as the asset table spike read them.

Standard library and the reference workload crate alone.

**Stand-ins for what does not exist.** The engine's world, its systems and its canonical form: the reference workload's world. The composition and its contributors: a list of the harness. Asset types: one synthetic type of parameters, and a second one so that a composition can lack a type. A content hash: the 64-bit function of the spikes. The I/O executor: two plain file threads behind the store port. The step of a control surface: a function call. A scene instance: a record of the harness that counts instantiations and makes its want under the world's own entry.

## Layout

One Cargo workspace whose crates follow the placement the design gives the subsystem. The harness records the dependency graph and checks it: the table has no edge to the loading capability, and the loading capability none to the pipeline.

- `core/`: the asset identity core inside the stand-in world. `table.rs` holds the table, the wants in their total order, the input record, the gate and the canonical bytes; `world.rs` the world, its input fixed point, its save and its two restores. It reads no file and reaches a part through an extension point. Every rule a scenario's control changes on purpose, and every place where the text was read one way among several, is a field of `Rules`.
- `store/`: the content store library. The formats the loading capability and the pipeline share and nothing else: the domain manifest, the root document of a revision, the package. No thread, no I/O.
- `loader/`: the loading capability. The resident store, the store port and its file threads, the manifest resolved per world, the batches, the moves of the manifest (a revision, a mount, an unmount), the answers to a restore.
- `pipeline/`: the stand-in pipeline.
- `spike-check/`: the asset table spike's scenarios, copied and ported.
- `harness/`: the scenarios, the oracles and the report. [`harness/scenarios.md`](harness/scenarios.md) states the inputs of every scenario and the format of an oracle; `harness/oracles/` holds the oracles.

## Hypothesis

Stated before the run. The asset table and the loading capability can be built from the design text alone, and so built they give the states the text says: every scenario equals an oracle written by hand from the same text, at 1, 4 and 8 workers, in release and unoptimized builds.

Two kinds of result refute it, and each is a finding about the text and not a failure of the build:

- **A differing result**: a state hash, the canonical bytes of a table, a list of events or a step's result that differs from the oracle's, once the difference is traced to the text and not to a fault of the build.
- **A rule that cannot be built as stated**: two sentences that give two behaviors, a case no sentence covers, a rule that needs a fact the text places elsewhere.

A build that equals its oracles shows that the scenarios run hold, on stand-ins. It does not show that no other mechanism is wrong.

## The four claims of the first stage

| Claim | Scenarios | Refuted by |
| --- | --- | --- |
| The order at a tick's input fixed point gives the stated states: the record first, its commands before its arrivals, then the falls | A want and the arrival of its asset in one record; a release and an arrival at a new hash in one record; a want made again at the tick of a fall, after a release and after a despawn; each of those records as the pending record of a gated world; three pending records on a world gated on one required asset | One state hash that differs from the oracle's |
| A want's owner orders the table | A parent wanted by two entities, one required and one optional, with one dependency, the two wants made in one order and in the other; a parent whose hash changes to a part with another dependency; a host's preload over an asset the composition requires, then released; one maker that wants one asset twice | One state hash that differs, or an asset that falls while a maker still wants it |
| A named content set holds under a moved manifest | A world created under a set beside a world that names none, and the same world under the same set on a second instance whose manifest never moves; a revision, the removal of an artifact, a mount with leave to override and an unmount moved under them; four required wants made meanwhile; a recording made with no set replayed under a set | A want answered at a hash outside the set, or a gate that does not open |
| A history is never continued under other content, on a restore under the content at hand | One world saved with available assets, a failed entry, an unanswered required want and an entry with no want; nine readers, each a process of its own over a store of its own: the same content under another set, one asset changed, one asset absent, the entries that are not available under a want, the unanswered want answered at another hash, a revision that lands while the world waits, a parent whose part names another dependency, another composition with and without the drop named | A first tick that runs before every answer, an event or a new instantiation on a marked answer, a marked answer replaced by a revision, a continued run whose hashes differ from the uninterrupted run's, or one state hash that differs from the oracle's |

## Method

- **Two readings of one text.** The oracles were written by hand from the design text, by writers who had read neither this build nor the code of the spikes it stands on, and were in place before the scenario each judges first ran. The build was written from the same text by someone who had not read the oracles. Where the two differ, the text arbitrates, with three outcomes: the build misread, the oracle misread, or the text allows both.
- **An oracle is a table of states and not a second implementation**: per named tick, the elements of the tick's input record, the whole asset table with every want in its stated order, the events, the instances, and what a step names. The harness writes each stated table through the canonical form, in the order the oracle gives, and compares bytes; the state hash it expects for a tick is the hash of those bytes with the workload's hash of a sequential run that reads, per asset, what the stated table gives.
- **Each scenario has a control**: the same scenario with the mechanism under test deliberately changed (the arrivals before the commands, the falls before the record, one shared want under the world's entry, the instance's manifest in place of the set's, the answers to a restore posted unmarked, and so on) must differ from the oracle. A control that equals the oracle is a fault of the scenario.
- **A reading is run both ways.** Where the text gives two behaviors, or none, the build takes one reading and the harness runs the other as well, so that a difference between the build and an oracle that comes from a reading alone is shown to be that and nothing else.
- **A replay of each run.** The input log of every scenario, read back from its bytes, replays the run at another worker count to the same state hashes.
- **An enumeration of small records.** Every record of at most four elements over two assets and two owners, applied to the table alone from five starting tables and followed by one empty tick, is checked against invariants the text states: no asset falls while a want names it; no entry stays without a want past one tick boundary; the canonical bytes do not depend on the order in which wants were made; a table read back from its bytes is byte-identical.
- A live loading capability answers on a thread of its own; the host waits, before each step, until it has nothing left to post, so that the tick an answer lands in is a function of the scenario.

A run prints one JSON document of facts on standard output and a short summary on standard error, and writes its full log, every difference with an oracle in words, beside its facts. The self-checks (`cargo test --workspace`) run every scenario on the small size, the readers of a save inside the test process, and the self-checks of each crate.

## Environment

The research CI, workflow `research-ci.yml`, five cells (see [`spikes/README.md`](../../spikes/README.md)), run on demand with `-f spike=asset-table-and-loader`, in the release profile (reference workload at default size, 10 000 entities) and in the unoptimized profile (`-f profile=dev`, the small size, 1 000 entities), which judges identity and no magnitude. One quiet machine outside the matrix (an AMD Ryzen 9 9950X3D, 16 cores and 32 threads, Windows 11). Rust toolchain pinned in [`spikes/rust-toolchain.toml`](../../spikes/rust-toolchain.toml), default target features. Worker counts 1, 4 and 8.

## Result

**The first stage, on one machine.** Measured on 2026-11-14 on the quiet machine alone, three times in the release profile (default size, a run of 36 seconds) and once in the unoptimized profile (small size). Not measured on the five cells yet. Every run gives the same verdicts, and the three release runs give the same run hash for each of the thirty scenario worlds, the scenarios with a live loading capability included.

- **The first check holds**: the six scenarios of the asset table spike that are run again (wants, the gate during a run, a revision as one batch, the table under the hash, a save read by another process, one resident copy) hold on the changed table and world, each reader of the save a process of its own. Two things were adapted and no check was weakened: the count of reload events on the answers to a restore counts arrivals and changes of hash, the event that records a new history being a kind the spike did not have; and the readers of the save run under the reading the spike measured for a failed answer to a restore (an event, and a failed instance), which the prototype keeps as one reading among three.
- **The order at a tick's input fixed point**: the eleven scenarios equal their oracle at 1, 4 and 8 workers, tick by tick (the record, the table's bytes, the events, what a step names, the state hash), each log replays at another worker count, and the ten controls differ.
- **The owners of wants**: four scenarios of five equal their oracle and the six controls differ. The fifth, the preload over the composition, differs at three ticks in the order of two wants of one entry, one made by a contributor and one by the source of a command: the build ordered the kinds, the oracle the numeric identities. Run under the oracle's reading, the build equals it at every tick.
- **A named content set under a moved manifest**: the world under the set, the world that names none and the world on the second instance each equal their oracle, the world under the set has the state hashes of the world whose manifest never moved (run hash `0xcd6756fc78f64816` for both), a recording made with no set replays under the set to the same hashes with the set's hash in the header of a save, and the two controls differ.
- **A restore under the content at hand**: the saved world and five readers equal their oracle; four readers differ from it by a reading alone, and equal it once run under the oracle's reading (where the event that records a new history stands among the events of its tick, in four readers; whether a failed answer to a restore fails the instance made from the asset, in one). The nine controls differ. Two results are not of that kind:
  - **A reader that never ran.** Where the part at hand of a parent lists another dependency, answers posted for the assets the saved table holds and for nothing else leave the world gated for ever: the gate, evaluated with the pending answers applied, counts the want of the new dependency, nothing is applied while the gate is closed, and so that want never leaves the world to be answered. The build therefore posts the new dependency with the answers, as an ordinary arrival, which is also what the oracle's writer read.
  - **A continued run that differs.** Where the saved table holds a required want unanswered and the content at hand answers it at another hash than the run that went on received, no answer to the restore differs, the restore is not recorded as the start of a new history, and the state hashes differ from the uninterrupted run's from the first tick.
- **The enumeration of small records**: 13 120 records, 65 600 applications, no invariant broken. 1 293 applications reach a case the rule on the fall reads two ways: an entry with no want, wanted and released again inside one record.

**Where the text was read.** The builder and the two writers of oracles recorded, each alone, the places where the text gave two behaviors or none. Those that a run could tell apart were run both ways: which wants a loading capability answers (each new want, or each asset that is unanswered: the input record differs, the state does not); where a despawn of a record ends its entity's wants (at its place in the record, or at the structural change point of its tick: the state differs when the record also holds the arrival of the asset); how two makers of different kinds compare; what a failed answer to a restore does; where the event of a restore stands; whether the answers to a restore follow the closure of the parts at hand.

The result holds for the reference workload version 1 at this toolchain, on this machine, on the stand-ins named above.
