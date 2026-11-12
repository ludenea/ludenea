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

Not measured yet.
