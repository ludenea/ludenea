# Multiplayer test rounds

Throwaway spike of the game testing research. It hosts several peers of an agreed game as worlds of one process, joined to a relay by seeded links, and drives them by rounds as a test fixture would: each participant runs its host's part once in a round, then the links advance by one frame. It measures, instead of assuming, whether the order of the peers in a round changes nothing under two conditions on the link; whether a schedule of stalls, suspensions, losses and clock rates repeats its history; whether the fixture's oracle reports an injected desync at its first divergent tick; whether each world's recording replays alone and a round table places the replayed worlds; and what the fixture's preparation, a round and the link log cost.

It stands on the [lockstep spike](../workload-lockstep/README.md)'s crate (its world of the reference workload, with the input fixed point and the snapshot, and its link settings) and on the [recording round trip spike](../test-recording-round-trip/README.md)'s container and recording. The lockstep spike holds two peers that send each other their inputs, and no recording; so this spike builds, at its size, what its hypotheses need: a relay, a link, a fixture and a recording of a world with several sources of inputs. It is evidence, never product code.

## Layout

- `src/link.rs`: the lockstep spike's link (a latency in frames, a jitter and a loss drawn from a seeded stream) with delivery at its advance, state per direction, a log and a loss rate a schedule replaces for a span of rounds; and, beside it, the form the lockstep spike has, one random stream for both directions drawn from at each send.
- `src/protocol.rs`: the messages, the relay and the peers. A peer sends the relay every input of its own the relay has not acknowledged; the relay merges a tick once it holds every peer's inputs for it, in the order of the peers' identities, and sends each peer the merged records that peer has not acknowledged; a peer steps a tick when it holds the tick's merged record. One peer may roll back instead: it steps ahead inside a window of eight ticks on its own input and the others' last known ones, keeps a ring of snapshots, and restores and steps again when a merged record contradicts a prediction. The relay has no rule for a peer that is gone: it waits.
- `src/fixture.rs`: the fixture (the worlds, the relay, one link per peer), the rounds in a fixed order, in an order shuffled afresh at every round from a seed, or at once on a pool of threads; the schedule, written or drawn from a seed, and its text form; and the oracle.
- `src/recording.rs`: a peer's recording, its replay alone, and the round table.
- `src/main.rs`: the measurements and the report.

## A round

A round is one frame of the test's time. Each peer receives what its link delivered, counts a duration for the round on a clock of its own (one frame by default), steps the ticks its clock has reached and the protocol allows, at most four in a round, and sends. The relay receives, merges and sends. Then every link advances by one frame: it draws, for each message handed to it in the round, a loss and a jitter, and moves what is due in the next round to the receiving end. A peer's clock starts one frame before its first tick: the rounds the connection took are not time the game is late by.

Two things follow from the link's delivery at its advance. A message is received in the round after it was sent at the earliest, so the lockstep spike's setting with no latency means one frame here, on each of the two hops through the relay: beside that spike's figures, where an input delay of 2 absorbed a latency of 1 between two peers that sent to each other, an input delay of 2 here absorbs exactly the two hops of one frame, and 60 ticks take 62 rounds. And a peer never sees, in a round, what another did in that round.

## Hypotheses

Stated in the game testing findings before the spike was built. A hypothesis with no threshold is a figure to record.

1. **The order of the peers.** With links that deliver only when they advance and that hold nothing shared by their two directions, the hashes of every peer at every tick and the hash of the link log are the same for twenty seeded permutations of the order of the peers and with the peers of a round run on separate threads, at 2, 4 and 16 peers through the relay; with a link whose two directions share one random state, a permutation changes them.
2. **The repetition.** Those hashes are one set on the five cells, in both profiles and at two worker budgets.
3. **The schedule.** A schedule that holds a stall, a suspension, a burst of loss, a span of total loss on one link (all a disconnection with its reconnection is to a protocol that holds no connection) and a peer whose clock runs at another rate, and a schedule drawn from a seed under stated rates and replayed from its recorded events, each give the same history at every run.
4. **The oracle.** A desync injected into one peer is reported by the fixture's oracle at its first divergent tick, whatever the order of the peers, with the lockstep peers and with the peer that rolls back, for which the hash compared is the one of the confirmed tick.
5. **The recordings.** Each peer's recording replays alone to the hashes it recorded, and the round table brings the replayed worlds to the hashes recorded for the confirmed ticks of a given round, the peer that rolls back among them.
6. **The fixture's cost**, with no threshold: its preparation, from the creations to a starting point after the first agreed tick, at 2, 4 and 16 peers of 1 000 entities.
7. **The pace**, with no threshold: rounds per second at 2, 4, 16 and 64 peers, in sequence and with the peers of a round run at once.
8. **The link log**, with no threshold: its bytes per message and the cost of keeping it.

## Method

The research CI builds and runs the binary on the five cells in the release profile and in the unoptimized one (`-f profile=dev`); locally it runs the same way (`ci/run-spike.sh multiplayer-test-rounds <cell>`). Every peer's world is the small size of the reference workload, 1 000 entities, with an input delay of 2 ticks and four kicks per peer and tick. The link that draws has a latency of one frame, a jitter of up to two and loses one message in twenty.

**The history of a run** is three hashes: one over every peer's hash at every confirmed tick; one over the round table, which holds for every round and every peer the tick its world had reached, its confirmed tick and the hash it held; and one over the logs of every link.

**The order.** At 2, 4 and 16 peers, at one and two workers per world, 60 ticks over the link that draws: one run in the fixed order (the peers by identity, then the relay), twenty runs each shuffling the participants afresh at every round from its own seed, and one run with the peers of a round run at once. Then the same twenty-one runs with the other form of link, whose two directions share one random stream.

**The schedule.** Four peers, 90 ticks. A written schedule holds a stall, two bursts of loss, two changes of a clock's rate, a suspension and a span of total loss; a drawn one comes from a seed under stated rates, with no event in the last ten rounds of the horizon. Each is run three times, then once more from the text of its events as the record keeps them.

**The oracle.** Four peers, a kick one peer applies at tick 23 and never sends, in seven orders (the fixed one, five shuffles, a round run at once), in three settings: every peer in lockstep; the kick in a lockstep peer while another rolls back; the kick in the peer that rolls back. A fourth case injects nothing.

**The recordings.** One run of four peers under the written schedule, one of which rolls back. Each peer's recording is written as a file, in the recording round trip spike's container and description, in its reference form: the world's creation values, one input record per tick and per source, and the hash of each confirmed tick. Each file is replayed alone, by a function that knows nothing of the other peers, and compared with the hashes it holds. Then for every tenth round of the round table and the last, each world is replayed alone to the confirmed tick the table gives it and compared with the hash recorded for that tick. The recordings, the round table and the last entries of the link logs are published with the measurements.

**The costs.** The preparation is timed five times from the creations to the round in which every peer has confirmed its first tick, over the link with no latency. The pace is the fastest of three runs of 60 ticks at 2, 4, 16 and 64 peers, in turn and at once. The link log is timed on a run of 16 peers with and without it, and on a link alone carrying 200 000 messages of 16 bytes.

**Departures from the specification and readings of it**, each stated here and in the findings:

- "The hashes of every peer at every tick" cannot show the order in a lockstep game whose inputs are drawn from the state: the hash of a tick is fixed by the inputs, whenever the tick is stepped. So the history adds the round table to them, and what a permutation changes, with one random state for both directions, is reported for each of the three hashes.
- "Twenty seeded permutations" are twenty seeds, each shuffling the order afresh at every round, the relay among the participants.
- "On separate threads" is a pool of as many threads as the machine has processors, at most one per peer, with the relay run beside them.
- The link's log holds a message's direction, its sequence, the round it was sent in, its size, and the round it is received in or that it was lost. The lockstep spike's link draws a loss and a jitter only: no message is duplicated, damaged or dropped at a queue's bound, and there is one lane.
- A peer's recording holds a kick the peer applied and never sent as a record of its own, under a source no peer has: without it the recording of the peer that diverged would not replay to its own hashes.

The self-checks (`cargo test -p multiplayer-test-rounds`) verify the link (delivery at its advance, the two forms under the two orders of its ends, a span of total loss, the ring and its hash), the messages, the relay's merge, the fixture (agreement through the relay, the order and the threads, the desync in each setting and order, a peer that rolls back, the two schedules, a stall against a suspension, the preparation's rounds) and the recordings (each alone, the round table, a kick never sent).

## Environment

The five cells of the research CI and a developer machine (Windows 11), in the release and the unoptimized profiles. No third-party library is linked or vendored.

## Result

Measured on 2026-11-12 on the five cells of the research CI in the release profile ([run](https://github.com/ludenea/ludenea/actions/runs/43284528151)) and in the unoptimized one ([run](https://github.com/ludenea/ludenea/actions/runs/43284534815)), and on a developer machine (AMD Ryzen 9 9950X3D, Windows 11) in both. Rust 1.99.1. The raw measurements, the recordings, the round table, the schedules as recorded and the last entries of the link logs are in the runs' artifacts.

1. **The order of the peers: holds, and the hashes at every tick do not show it.** Under the two conditions, the twenty shuffles and the round run at once give the fixed order's history, its three hashes, at 2, 4 and 16 peers and at one and two workers per world, on every machine and in both profiles. With one random state for both directions of a link, each of the twenty shuffles changes the link logs and the round table, and none changes the hash of a tick, at every size and on every machine: the ticks of a lockstep game are fixed by their inputs, and the order of the peers is seen in when they are stepped.

   | Peers | Rounds for 60 ticks | Messages, of which lost | Shuffles with the fixed order's history | A round run at once gives it | With one random state: shuffles that change the link logs, the round table, the tick hashes |
   | --- | --- | --- | --- | --- | --- |
   | 2 | 150 | 600, 32 | 20 of 20 | yes | 20, 20, 0 |
   | 4 | 153 | 1 224, 64 | 20 of 20 | yes | 20, 20, 0 |
   | 16 | 175 | 5 600, 309 | 20 of 20 | yes | 20, 20, 0 |

2. **The repetition: holds.** The three hashes of the fixed order's history are one set on the five cells, in both profiles, at both worker budgets and on the developer machine, 24 reports per size:

   | Peers | Hashes at every tick | Round table | Link logs |
   | --- | --- | --- | --- |
   | 2 | `1f9ec3579e90f541` | `d3db2f31f1b170cf` | `5ae06a7d39fcfba8` |
   | 4 | `24e69849abbfa875` | `abd64058b22cc0fd` | `d1d09c198c602790` |
   | 16 | `7d4bf6729e86b5e5` | `d5518de8266d07ab` | `cc38ae6f37bbcc5a` |

3. **The schedule: holds.** The written schedule of seven events and the schedule of nineteen events drawn from the seed each give one history in three runs, the same from the text of their recorded events, and the same on every machine and in both profiles. Every peer confirms the 90 ticks; the run takes 246 rounds under the written schedule and 267 under the drawn one, against 228 with none; the hashes at every tick are those of the run with no schedule.
4. **The oracle: holds.** The kick applied at tick 23 and never sent is reported at tick 23 in the seven orders of each setting, and in the same round in every order: round 63 with every peer in lockstep, round 60 with the kick in a lockstep peer beside a peer that rolls back, round 61 with the kick in the peer that rolls back. With no kick the oracle reports nothing, the peer that rolls back having rolled back 31 times.
5. **The recordings: hold.** The four recordings, 29 kilobytes each for 90 ticks, replay alone to every hash they hold, the one of the peer that rolled back 46 times among them. For 24 rounds of the 233 in the round table, each of the four worlds replayed alone to the confirmed tick the table gives it holds the hash recorded for that tick: 96 worlds placed, none misplaced, 23 of them worlds that stood ahead of their confirmed tick at that round. The round table is 7.6 kilobytes as text. The recording round trip spike's replay does not read these files: it expects one input record per tick.
6. **The fixture's cost.** The preparation takes three rounds over the link with no latency. From the creations to every peer's first agreed tick, median of five in milliseconds:

   | Peers of 1 000 entities | Release, the five cells | Unoptimized, the five cells | Developer machine, release |
   | --- | --- | --- | --- |
   | 2 | 0.23 to 0.33 | 2.0 to 3.4 | 0.16 |
   | 4 | 0.46 to 0.64 | 3.9 to 6.8 | 0.31 |
   | 16 | 2.0 to 3.2 | 17 to 30 | 1.5 |

7. **The pace**, in rounds per second in release, the peers of a round in turn and at once (on four threads on the Linux and Windows cells, three on the macOS cell):

   | Cell | 2 peers | 4 peers | 16 peers | 64 peers |
   | --- | --- | --- | --- | --- |
   | `linux-x86_64` | 5 465 and 8 040 | 2 844 and 4 686 | 691 and 1 410 | 115 and 274 |
   | `linux-arm64` | 6 426 and 10 360 | 3 213 and 6 662 | 736 and 1 908 | 121 and 363 |
   | `windows-x86_64` | 4 162 and 5 323 | 2 102 and 3 647 | 498 and 1 103 | 88 and 220 |
   | `windows-arm64` | 6 281 and 8 591 | 3 140 and 7 511 | 714 and 2 122 | 118 and 400 |
   | `macos-arm64` | 5 282 and 6 432 | 2 249 and 4 050 | 512 and 1 558 | 91 and 293 |

   In turn a round costs 78 to 126 microseconds per peer at 2 to 16 peers and 129 to 179 at 64. Run at once the peers go 1.2 to 3.4 times faster. Unoptimized, a round in turn costs 650 to 1 350 microseconds per peer at 2 to 16 peers.
8. **The link log.** An entry is 30 bytes; a link's ring of 256 entries is 7 680 bytes; a message carries 750 bytes on average at 16 peers. On a link alone a message costs 123 to 257 nanoseconds without the log and 13 to 30 more with it, in release. A run of 16 peers does not resolve the difference: 91.1 to 137.4 milliseconds with the log and 91.0 to 135.4 without.

Not measured here, and not held by the spike: the loss of a peer, a late join, a leave and a return, for which the relay has no rule; the predicates of progress and of agreement at rest; a trace; an authority with its replicas; a world that leaves the replay promise; physics; the real transport; a recording read on another cell; and a fixture whose worlds carry content, on which the cost of its preparation would turn.
