# Scenarios of the stage of the four claims

This sheet states the inputs of every scenario and the format of an oracle. It states no expected result: an oracle is written by hand from the design text, by a writer who has not read the build, and the build is written from the same text by someone who has not read the oracles. Where the two differ, the text arbitrates.

## Conventions

- **Ticks.** A world is created at tick 0, before any tick has run. The input record of tick `n` is applied at the input fixed point at the start of tick `n`. "After tick `n`" is the boundary at the end of tick `n`, where the state hash of tick `n` is taken.
- **Assets** are named by letters. Within a family their identifiers are drawn so that the identifier order is the order in which the family lists them.
- **Hashes** are named `<asset>.<version>`: `A.1` and `A.2` are two contents of the asset `A`, with different parameters.
- **Entities** `E1` and `E2` are alive from tick 0, are never despawned by the workload itself during a scenario, and `E1` has the smaller identifier.
- **Makers under the world's own entry.** A contributor of the composition is written `contributor:<name>` and the source of a command `source:<name>`. Each has a numeric identity: `contributor:base` is 7, `source:host` is 3, `source:tool` is 5. The stand-in for a scene instance is written `instance:<name>`: it makes its want under the world's own entry with an identity of its own, numbered from 1 in the order the instances are made. How the identities of the three kinds compare with one another is for the writer to read in the text.
- **A scripted availability source** places an arrival in the record of a named tick. **A live loading capability** answers from a store; the host waits, before each step, until the capability has nothing left to post, so that what it posts in answer to a tick is in the record of the next tick.
- **A step without waiting** asks the world to run one tick and not to wait at a closed gate.
- **Reasons of a failure**: `unknown-asset` (the manifest the world stands under does not hold the identifier), `artifact-gone` (the store holds no bytes under the hash the manifest names), `invalid-part` (the bytes do not hash to their name or do not validate).
- **Policies**: `required` and `optional`.
- Every scenario runs at 1, 4 and 8 workers. Every entity of the workload names one asset of the family's pool, in turn, and reads its repulsion parameters from it; an asset that is not available reads the declared default.

## The format of an oracle

One text file per family, in `oracles/`. Blank lines and lines that start with `#` are ignored. A statement is one line:

```text
scenario <name>
world <name>                       optional; one world named "world" otherwise
blocked <n> awaiting <asset>, ...  a step without waiting before tick n runs no tick and names these
tick <n>                           opens the statement of tick n
record <element>                   one line per element of the record of tick n, in the order applied
entry <asset> <availability>       one line per entry of the table after tick n, in the table's order
want <owner> <policy>              the wants of the entry above, in the table's order
event <event>                      one line per event emitted at tick n, in order
instance <name> <asset> instantiations <count> failed <yes|no>
names-failed <asset> <reason>      the step that runs tick n names this required asset as failed
restore <verdict>                  for a restored world: the outcome of the restore, right after the scenario line
```

- `<availability>` is `unavailable`, `available <hash>` or `failed <reason>`.
- `<owner>` is `contributor:<name>`, `source:<name>`, `instance:<name>`, `entity:E1` or `asset:<parent>` for a want the core added.
- `<element>` is `want <asset> <owner> <policy>`, `release <asset> <owner>`, `despawn <entity>`, `instantiate <name> <asset> <policy> reload <on|off>`, `arrival <asset> <hash>`, `arrival <asset> failed <reason>`, either arrival followed by `marked` where it answers a restore.
- `<event>` is `arrived <asset> <hash>`, `changed <asset> <from> <to>`, `failed <asset> <reason>` or `new-history <asset>, ...` for the event that records a restore whose answers differed.
- `<verdict>` is `continues`, `new-history <asset>, ...` or `refused <name>`.

A tick block states the whole table after that tick, every entry and every want, and every instance. A tick that has no block has an empty record, no event, and the table and instances of the block before it; before the first block the table is the one the scenario's creation gives. So every tick at which the record is not empty, an event is emitted or the table changes needs a block. The order of the `entry` lines and of the `want` lines is part of the statement: the harness writes them through the canonical form in the order given and compares bytes.

Where the text gives two behaviors, or none, the writer takes one reading, writes the oracle under it, and records the sentences involved in a note beside the oracle.

## Family `order`: the order at a tick's input fixed point

Assets `A`, `B`, `G`. The pool is `A`, `B`. The table is empty at tick 0. The availability source is scripted. Each scenario runs to tick 10.

- **`order/want-and-arrival`**. Record of tick 4: `want A source:host required`, and the arrival of `A` at `A.1`.
- **`order/release-and-arrival`**. Record of tick 2: `want A source:host required`, and the arrival of `A` at `A.1`. Record of tick 5: `release A source:host`, and an arrival of `A` at `A.2`.
- **`order/want-again-after-release`**. Record of tick 2 as above. Record of tick 5: `release A source:host`. Record of tick 6: `want A source:host required`. No arrival is scripted after tick 2.
- **`order/want-again-after-despawn`**. Record of tick 2: `want A entity:E1 required`, and the arrival of `A` at `A.1`. Record of tick 5: `despawn E1`. Record of tick 6: `want A source:host optional`. No arrival is scripted after tick 2.
- **`order/want-again-live`**. As `order/want-again-after-release`, with a live loading capability in place of the script, over a store that holds `A` at `A.1`. The oracle states what the records of the ticks after tick 6 hold.

**A record that accumulated.** The world is gated on `G`, and the record under test waits as its pending record.

- **`order/accumulated-want-and-arrival`**. Record of tick 3: `want G source:host required`. Then, pending for tick 4: `want A source:host required` and the arrival of `A` at `A.1`. A step without waiting. Then the arrival of `G` at `G.1` joins the pending record, and the world steps.
- **`order/accumulated-release-and-arrival`**. Record of tick 2: `want A source:host required`, and the arrival of `A` at `A.1`. Record of tick 3: `want G source:host required`. Pending for tick 4: `release A source:host` and an arrival of `A` at `A.2`. A step without waiting. Then the arrival of `G` at `G.1` joins, and the world steps.
- **`order/accumulated-want-again`**. Record of tick 1: `want A source:host required`, and the arrival of `A` at `A.1`. Record of tick 3: `want G source:host required`, then `release A source:host`. Pending for tick 4: `want A source:host required`. A step without waiting. Then the arrival of `G` at `G.1` joins, and the world steps.

**The gate on the record under test.** The world is gated on one required asset.

- **`order/gate-release-and-arrival`**. Record of tick 3: `want G source:host required`. A step without waiting. Pending for tick 4: `release G source:host` and an arrival of `G` at `G.1`. A step.
- **`order/gate-arrival-and-new-want`**. Record of tick 3: `want G source:host required`. A step without waiting. Pending for tick 4: the arrival of `G` at `G.1` and `want B source:host required`. A step, then a step without waiting. Then the arrival of `B` at `B.1` is posted, and the world steps.
- **`order/gate-pending-despawn`**. Record of tick 3: `want G entity:E1 required`. A step without waiting. Pending for tick 4: `despawn E1`. A step without waiting. Then the arrival of `G` at `G.1` joins the pending record, and the world steps.

## Family `owners`: the owners of wants

Assets `A`, `B`, `D1`, `P`, `D2`. The pool is `A`, `B`, `P`. The canonical part `P.1` lists `D1` as its one dependency; `P.2` lists `D2`. The availability source is scripted. Each scenario runs to tick 12.

- **`owners/two-wants-first-order`**. Record of tick 2: `want P entity:E1 required`, then `want P entity:E2 optional`. Record of tick 3: the arrivals of `P` at `P.1` and of `D1` at `D1.1`, one batch. Record of tick 6: `despawn E1`. Record of tick 9: `despawn E2`.
- **`owners/two-wants-second-order`**. The same, the two commands of tick 2 in the other order.
- **`owners/parent-changes-dependency`**. Record of tick 2: `want P entity:E1 required`. Record of tick 3: the arrivals of `P` at `P.1` and of `D1` at `D1.1`. Record of tick 6: the arrivals of `P` at `P.2` and of `D2` at `D2.1`, one batch.
- **`owners/preload-over-composition`**. The composition's contributor `base` wants `A` as required: the want is in the table at tick 0. Record of tick 1: the arrival of `A` at `A.1`. Record of tick 3: `want A source:host optional` and `want B source:host optional`, a host's preload. Record of tick 4: the arrival of `B` at `B.1`. Record of tick 6: `release A source:host` and `release B source:host`.
- **`owners/one-maker-two-wants`**. Record of tick 2: `want A source:host optional`. Record of tick 3: the arrival of `A` at `A.1`. Record of tick 5: `want A source:host required`. Record of tick 7: `release A source:host`.

## Family `named-set`: a named content set under a moved manifest

Assets `K1`, `K2`, `N`, `X2`, `X3`, `Y`. The pool is `K1`, `K2`, `X2`, `Y`. A live loading capability over a loose store written by the stand-in pipeline.

- The first revision's domain manifest, **the set**, holds `K1.1`, `K2.1`, `X2.1`, `X3.1`, `Y.1` and does not hold `N`.
- **The later revision** holds `K1.2`, `K2.1`, `X2.2`, `X3.2`, `Y.1` and `N.1`.
- **The package** holds `K2.9` and `Y.9` and is mounted with leave to override.

Three worlds, each with the composition's contributor `base` wanting `K1` and `K2` as required at tick 0:

- `under-set`, on the instance, created under the set's hash;
- `no-set`, on the same instance, naming no set;
- `reference`, on a second instance whose manifest stays the set, created under the set's hash.

What happens, the same for each world, each world stepped to tick 22:

- between tick 3 and tick 4, the later revision is put in place on the instance (not on the second instance);
- between tick 5 and tick 6, the artifact of `X3.1` is removed from the loose store of both instances; no process has loaded it;
- between tick 6 and tick 7, the package is mounted on the instance;
- record of tick 9: `want X2 source:host required`;
- record of tick 11: `want Y source:host required`;
- record of tick 13: `want N source:host required`;
- record of tick 15: `want X3 source:host required`;
- between tick 18 and tick 19, the package is unmounted.

This is the scenario **`named-set/moved-manifest`**. The oracle states the three worlds, each under a `world` statement. For `under-set` it states the table tick by tick; the harness also compares its state hashes with those of `reference`.

- **`named-set/recording-replays-under-a-set`**. Over the first revision alone, which never moves: a world that names no set, with the composition above; record of tick 4: `want X2 source:host optional`. Its input log is then replayed in a world created under the set's hash. The oracle states the one table both must give, tick by tick; the harness compares the state hashes of the two worlds and reads the set's hash in the header of a save of the second.

## Family `restore`: a restore under the content at hand

Assets `D1`, `D2`, `F`, `L1`, `L2`, `P`, `T`, `U`, `Z`. The pool is `L1`, `L2`, `P`, `U`. `T` is of a second asset type; every other asset is of the first. `P.1` lists `D1` as its one dependency. A live loading capability over a loose store written by the stand-in pipeline.

**The saved world.** The content it runs under holds `D1.1`, `D2.1`, `L1.1`, `L2.1`, `P.1`, `T.1`, `U.1`, `Z.1`, and holds `F` at a hash whose artifact is cut short. The world names no content set. Its composition declares both asset types, and its contributor `base` wants `L2` and `P` as required at tick 0.

- Record of tick 2: `instantiate one L1 required reload on`, `want Z source:host optional`, `want F source:host optional`, `want T source:host optional`.
- Record of tick 8: `release Z source:host`, then `want U source:host required`.
- The world is saved after tick 8. The process that saved it goes on to tick 16: this is the uninterrupted run, the scenario **`restore/saved-world`**, whose oracle states the table from tick 0 to tick 16.

**The readers.** Each loads the save in a new process, as a restore under the content at hand, names no content set but where it says so, and steps to tick 16. The oracle states each reader's verdict before its first tick, what a step without waiting names before the answers, and its tables from tick 9 on.

- **`restore/same-content-other-set`**. The content at hand is the saved world's, with one more asset the saved world never wanted, and the reader names that content's set.
- **`restore/one-changed`**. The content at hand holds `L1.2` in place of `L1.1`.
- **`restore/one-absent`**. The manifest of the content at hand does not hold `L1`.
- **`restore/entries-not-available`**. The content at hand is the saved world's. After tick 10, record of tick 11: `want F source:tool optional`, the content at hand then holding `F` at `F.1` with a whole artifact.
- **`restore/unanswered-at-another-hash`**. The content at hand holds `U.2` in place of `U.1`.
- **`restore/revision-while-waiting`**. The content at hand is the saved world's when the answers are posted. Before the reader's first tick, a revision that holds `L1.2` in place of `L1.1` is put in place.
- **`restore/parent-with-another-dependency`**. The content at hand holds `P.2`, which lists `D2` as its one dependency, in place of `P.1`.
- **`restore/another-composition-refused`**. The content at hand is the saved world's. The composition at hand declares the first asset type alone. The request names nothing as dropped.
- **`restore/another-composition-dropped`**. The same, and the request names as dropped what the text has it name.
