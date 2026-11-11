# Input records from device events

Throwaway spike of the input management research. It answers one question: when the simulation's only input from a player is one record per tick, built outside the simulation from timestamped device events and carrying held values and cumulative counters, is every press counted exactly once at any ratio of frames to ticks, does a record applied twice or replaced by the previous one stay harmless, do the counters keep a correct baseline across writers and restores, and what do a windowing library and a gamepad library do when they start on a machine with no device?

## Hypotheses

1. **No press is lost or doubled.** A stream of key, stick and mouse events, three presses in ten shorter than a tick, turned into one record per tick, gives the simulation the stream's exact count of presses and releases and its exact sum of relative motion, whether a frame runs four ticks, one tick or none. Refuted by one press, release or unit of motion more or fewer.
2. **A record is idempotent.** Every record applied on two successive ticks, one record in ten missing (the previous one applied again), one in ten rejected for a value that is not finite or for another writer's epoch: the totals at the end still equal the stream's. Refuted by one more or one fewer.
3. **The baseline survives the writer.** A writer whose counters are already running presses nothing when it binds; a writer lost while it holds an action is unbound by a tick input that releases what was held without a release being read; a second writer with unrelated counters presses nothing when it takes the slot; the whole input log replays to the same hash at every tick. Refuted by one press at a bind, one release at the unbind, or one differing hash.
4. **A restore under a live writer.** A world restored to an earlier tick while its writer kept counting. Measured both ways: with nothing done at the restore, and with every slot unbound at the restore so that the writer binds again. The second must reject the stale record and press nothing at the new bind.
5. **Where a press lands.** With event timestamps, the earlier ticks of a frame take the events up to their own end and the last tick takes every event up to the frame's instant; without timestamps the frame's first tick takes them all. Recorded: the tick of application against the tick during which the key went down.
6. **The device layer starts with nothing.** On each cell, a gamepad library and a windowing library's event loop are started and dropped in a child process: the outcome, the gamepads listed and the process's thread count before, during and after. Recorded as facts.

## Method

Everything but the last hypothesis is plain Rust with no dependency beyond the workspace's reference workload crate, used for its hash, its random sequence and its report writer.

- `src/lib.rs` holds the model. A device event is a key going down or up, a stick position, a relative mouse motion or a lost focus, with a time in nanoseconds on the host's clock. The mapping layer, outside the simulation, holds the bindings and the cumulative values: per action a held bit and two 8-bit wrapping counters (presses, releases), a shaped two-axis value, a 32-bit wrapping accumulator of relative motion per axis and an integrated two-axis value. A record is those values and the writer's epoch: 49 bytes with an epoch of 8 bytes.
- The simulation's side is one player slot. A tick's inputs are, in the order applied, an unbind, a bind that carries the writer's values at that moment as the baseline, a record, and a change of the active contexts. A record whose epoch is not the slot's, whose values are not finite, or whose counters moved by more than a bound since the previous applied record, is rejected whole; a tick without an accepted record applies the previous one again. Presses and releases are the differences of the counters; the held bit is state only. Actions outside the slot's active contexts are neutral at application.
- The host loop runs frames of a fixed period (from four ticks per frame to eight frames per tick) or of irregular length, and for each tick due feeds the mapping layer the events assigned to that tick and takes one record.
- The stream: 6 000 ticks of 16 milliseconds, eight keys, a stick report every 8 milliseconds and a mouse report every millisecond, from a fixed seed; what it holds is counted where it is generated.
- The device layer probe runs as a child process (`--probe`), so that a crash inside a platform library is a recorded fact. It uses gilrs 0.11.2 (Apache-2.0 or MIT) and winit 0.30.13 (Apache-2.0), the two libraries being the subject of that measurement, and counts the process's threads through the operating system (the task directory on Linux, a thread snapshot on Windows, the task's thread list on macOS). On Linux the gamepad library links the device manager library, which `setup.sh` installs.

`cargo test` runs the self-check: a tap shorter than a tick is one press and one release, the same record applied twice presses once, and a record of another epoch is rejected.

## Environment

The five cells of the research CI (Windows Server 2025 and Windows 11 ARM64, Ubuntu 24.04 on x86_64 and ARM64, macOS 15 on Apple silicon), Rust 1.99.1, release profile; one developer machine (Windows 11, x86_64) for a local run with an interactive session.

## Result

Measured on 2026-11-11 on the developer machine and on the five cells of the research CI.

On the developer machine every check holds:

- At the seven frame schedules, with and without event timestamps, the simulation's presses, releases and motion equal the stream's (2 586 presses, the busiest action 335, so its 8-bit counter wrapped).
- Every record applied twice, one in ten missing, one in ten not finite and one in ten of another epoch all end on the stream's totals; a counter jump beyond the bound is rejected and presses nothing.
- A writer with 214 presses already counted presses nothing at its bind; the unbind releases the held action with no release read; a second writer presses nothing at its bind; the log of 7 041 ticks, binds and unbind included, replays to the same hash at every tick.
- **A restore with nothing done is not caught by the epoch**: the writer's epoch is still the slot's, the next record is applied, and it presses 132 times where the writer counted 1 in that tick. With every slot unbound at the restore the stale record is rejected and the new bind presses nothing.
- With timestamps a press is applied in the tick during which it happened at every fixed schedule; with irregular frames it is applied in its tick or one tick ahead (the last tick of a frame takes the events up to the frame's instant) and never late. Without timestamps a press is applied up to three ticks ahead of the tick during which it happened when a frame runs four ticks.
- A record costs 121 nanoseconds to build from about 18 events and 7 nanoseconds to apply; 13.8 of its 49 bytes differ from the previous record on average.
- The device layer on this machine: the gamepad library starts with no gamepad and takes the process from 4 threads to 14, of which 13 remain after it is dropped; the windowing library's event loop is created and adds no thread.

On the five cells (research CI run 43019334701) every check holds, and everything that does not depend on the machine is identical to the developer machine's: the totals at the seven schedules, the idempotence variants, the writers, the 132 presses of the restore with nothing done, the placement of the presses (0 at every fixed schedule with timestamps, at most one tick ahead with irregular frames; up to three ticks ahead without timestamps) and the 13.8 bytes that change per record. A record costs 112 to 203 nanoseconds to build and 14 to 23 to apply.

The device layer on the cells, none of which has a gamepad:

| Cell | Gamepad library: threads before, running, after the drop | Windowing library's event loop | Threads before, while it exists, after the drop |
| --- | --- | --- | --- |
| Linux, x86_64 and ARM64 | 1, 3, 3 | fails with an error and no crash: neither display variable is set | 3, 3 |
| macOS, ARM64 | 1, 3, 3 | created | 3, 5, 5 |
| Windows, x86_64 and ARM64 | 4, 10, 9 | created | 9, 9, 9 |

The gamepad library starts on every cell with no gamepad and no error. It creates two threads on Linux and macOS and six on Windows, and on no cell does the count return to its starting value after the library is dropped. The windowing library's event loop cannot exist on a Linux machine with no display server, creates two threads on macOS that outlive it, and none on Windows.
