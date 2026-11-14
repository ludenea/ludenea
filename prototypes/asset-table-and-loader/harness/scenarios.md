# Scenarios of the asset table and loader prototype

This sheet states the inputs of every scenario and the format of an oracle. It states no expected result: an oracle is written by hand from the design text, by a writer who has not read the build, and the build is written from the same text by someone who has not read the oracles. Where the two differ, the text arbitrates.

It holds the families of two stages. The first stage ran the families `order`, `owners`, `named-set` and `restore` against the design text as it then stood, and the text was amended on what that stage found. The same-harness stage builds the amended rules before it measures anything new: the oracles of those four families are written again under the text as amended, and the families `retry`, `types`, `dependencies`, `revision`, `pending-parent` and `record` hold the scenarios of the amended rules that no run has taken.

## Conventions

- **Ticks.** A world is created at tick 0, before any tick has run. The input record of tick `n` is applied at the input fixed point at the start of tick `n`. "After tick `n`" is the boundary at the end of tick `n`, where the state hash of tick `n` is taken.
- **Assets** are named by letters. Within a family their identifiers are drawn so that the identifier order is the order in which the family lists them.
- **Hashes** are named `<asset>.<version>`: `A.1` and `A.2` are two contents of the asset `A`, with different parameters.
- **Asset types** are named by the stable name of their schema entry: `parameters` for the first synthetic type, `second` for the second one. `third` names a third type of which no asset exists: only a composition or a request of the family `restore` names it. Every asset is of the first type unless its family says otherwise, and a composition declares the first and the second type unless its scenario says otherwise. A want states an asset type; a want the composition makes at tick 0 states the type of its asset.
- **Entities** `E1` and `E2` are alive from tick 0, are never despawned by the workload itself during a scenario, and `E1` has the smaller identifier.
- **Makers under the world's own entry.** A contributor of the composition is written `contributor:<name>` and the source of a command `source:<name>`. Each has a numeric identity: `contributor:base` is 7, `source:host` is 3, `source:tool` is 5, and `source:twin` is 7, the identity of `contributor:base`. The stand-in for a scene instance is written `instance:<name>`: it makes its want under the world's own entry with an identity of its own, numbered from 1 in the order the instances are made. How the identities of the three kinds compare with one another is for the writer to read in the text.
- **A scripted availability source** places an arrival in the record of a named tick. **A live loading capability** answers from a store; the host waits, before each step, until the capability has nothing left to post, so that what it posts in answer to a tick is in the record of the next tick.
- **A step without waiting** asks the world to run one tick and not to wait at a closed gate.
- **Reasons of a failure**: `unknown-asset` (the manifest the world stands under does not hold the identifier), `other-type` (the manifest holds the asset under another type than the one its entry holds), `artifact-gone` (the store holds no bytes under the hash the manifest names), `invalid-part` (the bytes do not hash to their name or do not validate), `dependencies-differ` (the dependencies the part lists, each with its type, are not those the manifest lists for it).
- **Reasons of a recorded refusal** of a want: `owner-not-alive` (the entity that would own the want is not alive when the command is applied), `other-type` (the want states another type than the one its asset's entry holds).
- **Policies**: `required` and `optional`.
- **A save and its readers.** A save is taken at the boundary after a named tick. A reader loads it in a new process, over a store of its own, by a request that says whether the content must be the recorded one: a restore **under the recorded content**, or a restore **under the content at hand**. A request under the content at hand names the content set the new world stands under, or none. A request into another composition names the asset types it drops, or none.
- **A control** is named beside each scenario of the families the same-harness stage writes or writes again: the same scenario with the mechanism under test deliberately changed, which must differ from the oracle. It is stated in one sentence, as the rule changed. Where a control is named as the reading not taken, the text was read two ways on that point and the founder took the other reading on 2026-11-14: the oracle stands under the reading taken.
- Every scenario runs at 1, 4 and 8 workers. Every entity of the workload names one asset of the family's pool, in turn, and reads its repulsion parameters from it; an asset that is not available reads the declared default.

## The format of an oracle

One text file per family, in `oracles/`. Blank lines and lines that start with `#` are ignored. A statement is one line:

```text
scenario <name>
world <name>                       optional; one world named "world" otherwise
blocked <n> awaiting <asset>, ...  a step without waiting before tick n runs no tick and names these
tick <n>                           opens the statement of tick n
record <element>                   one line per element of the record of tick n, in the order applied
refusal <reason>                   the record element above is a recorded refusal, which changes nothing
entry <asset> <type> <availability>  one line per entry of the table after tick n, in the table's order
want <owner> <policy>              the wants of the entry above, in the table's order
event <event>                      one line per event emitted at tick n, in order
instance <name> <asset> instantiations <count> failed <yes|no>
names-failed <asset> <reason>      the step that runs tick n names this required asset as failed
restore <verdict>                  for a restored world: the outcome of the restore, right after the scenario line
```

- `<type>` is `parameters`, `second` or `third`: the asset type an entry holds, or the one a want states.
- `<availability>` is `unavailable`, `available <hash>` or `failed <reason>`.
- `<owner>` is `contributor:<name>`, `source:<name>`, `instance:<name>`, `entity:E1` or `asset:<parent>` for a want the core added.
- `<element>` is `want <asset> <type> <owner> <policy>`, `release <asset> <owner>`, `despawn <entity>`, `instantiate <name> <asset> <type> <policy> reload <on|off>`, `arrival <asset> <hash>`, `arrival <asset> failed <reason>`, either arrival followed by `marked` where it answers a restore.
- `<reason>` after `refusal` is a reason of a recorded refusal; everywhere else it is a reason of a failure.
- `<event>` is `arrived <asset> <hash>`, `changed <asset> <from> <to>`, `failed <asset> <reason>` or, for the one event that records a restore that starts a new history, `new-history` followed by up to three lists, in this order and each left out where it is empty: `differed <asset>, ...` (each asset whose answer differed from the recorded hash), `unanswered <asset>, ...` (the asset of each want the save held unanswered), `dropped <asset>, ...` (each entry the request dropped). Each list is in the order of the asset identifiers.
- `<verdict>` is `continues`; `new-history` with the lists of the event above; `refused type <type>` for a restore refused by the name of one asset type; `refused types <type> <type>` for one refused by two, the type the request names as dropped and then the type of the kept entry that lists a dependency of it; or `refused hash <asset> <hash>` for a restore under the recorded content whose confirmation fails, with the recorded hash that could not be confirmed.

A tick block states the whole table after that tick, every entry and every want, and every instance. A block with no `entry` line states an empty table. A `tick 0` block states the table the creation gives, with an empty record and no event. A tick that has no block has an empty record, no event, and the table and instances of the block before it; before the first block the table is the one the scenario's creation gives. So every tick at which the record is not empty, an event is emitted or the table changes needs a block. The order of the `entry` lines and of the `want` lines is part of the statement: the harness writes them through the canonical form in the order given and compares bytes.

Where the text gives two behaviors, or none, the writer takes one reading, writes the oracle under it, and records the sentences involved in a note beside the oracle.

## Family `order`: the order at a tick's input fixed point

Assets `A`, `B`, `G`. The pool is `A`, `B`. No canonical part of this family lists a dependency. The table is empty at tick 0. The availability source is scripted. Each scenario runs to tick 10.

Controls, each the rule changed:

- `order/want-and-arrival`, `order/release-and-arrival`, `order/accumulated-want-and-arrival`, `order/accumulated-release-and-arrival`, `order/gate-release-and-arrival`: the arrivals of a record applied before its commands.
- `order/want-again-after-release`, `order/want-again-after-despawn`, `order/accumulated-want-again`, `order/want-again-live`: the falls applied before the record.
- `order/want-again-live`, a second control: every want answered, an asset in force included.
- `order/gate-release-and-arrival`, a second control: an entry that never rose leaves at once.
- `order/gate-arrival-and-new-want`: the gate counts a want of the pending record.
- `order/gate-pending-despawn`: the gate sees a despawn of the pending record; and a second control, a despawn of a record ends its entity's wants at the tick's structural change point.
- Every scenario of every family whose oracle states a table that holds an entry: the entry's type left out of the canonical bytes. Nine scenarios state no such table, a reader whose restore is refused or a table that is empty at every tick compared, and the control cannot differ there: `restore/recorded-content-refused`, `restore/another-composition-refused`, `restore/another-composition-asset-absent`, `restore/drop-names-a-declared-type`, `restore/drop-names-a-declared-type-with-no-entry`, `types/drop-not-named`, `types/drop-refused-for-a-kept-parent`, `types/dropped-parent-of-a-kept-dependency` and `record/want-after-the-despawn-of-its-owner`. The sheet named it for them before the first run, which showed the nine controls equal to their oracles.

- **`order/want-and-arrival`**. Record of tick 4: `want A parameters source:host required`, and the arrival of `A` at `A.1`.
- **`order/release-and-arrival`**. Record of tick 2: `want A parameters source:host required`, and the arrival of `A` at `A.1`. Record of tick 5: `release A source:host`, and an arrival of `A` at `A.2`.
- **`order/want-again-after-release`**. Record of tick 2 as above. Record of tick 5: `release A source:host`. Record of tick 6: `want A parameters source:host required`. No arrival is scripted after tick 2.
- **`order/want-again-after-despawn`**. Record of tick 2: `want A parameters entity:E1 required`, and the arrival of `A` at `A.1`. Record of tick 5: `despawn E1`. Record of tick 6: `want A parameters source:host optional`. No arrival is scripted after tick 2.
- **`order/want-again-live`**. As `order/want-again-after-release`, with a live loading capability in place of the script, over a store that holds `A` at `A.1`. The oracle states what the records of the ticks after tick 6 hold.

**A record that accumulated.** The world is gated on `G`, and the record under test waits as its pending record.

- **`order/accumulated-want-and-arrival`**. Record of tick 3: `want G parameters source:host required`. Then, pending for tick 4: `want A parameters source:host required` and the arrival of `A` at `A.1`. A step without waiting. Then the arrival of `G` at `G.1` joins the pending record, and the world steps.
- **`order/accumulated-release-and-arrival`**. Record of tick 2: `want A parameters source:host required`, and the arrival of `A` at `A.1`. Record of tick 3: `want G parameters source:host required`. Pending for tick 4: `release A source:host` and an arrival of `A` at `A.2`. A step without waiting. Then the arrival of `G` at `G.1` joins, and the world steps.
- **`order/accumulated-want-again`**. Record of tick 1: `want A parameters source:host required`, and the arrival of `A` at `A.1`. Record of tick 3: `want G parameters source:host required`, then `release A source:host`. Pending for tick 4: `want A parameters source:host required`. A step without waiting. Then the arrival of `G` at `G.1` joins, and the world steps.

**The gate on the record under test.** The world is gated on one required asset.

- **`order/gate-release-and-arrival`**. Record of tick 3: `want G parameters source:host required`. A step without waiting. Pending for tick 4: `release G source:host` and an arrival of `G` at `G.1`. A step.
- **`order/gate-arrival-and-new-want`**. Record of tick 3: `want G parameters source:host required`. A step without waiting. Pending for tick 4: the arrival of `G` at `G.1` and `want B parameters source:host required`. A step, then a step without waiting. Then the arrival of `B` at `B.1` is posted, and the world steps.
- **`order/gate-pending-despawn`**. Record of tick 3: `want G parameters entity:E1 required`. A step without waiting. Pending for tick 4: `despawn E1`. A step without waiting. Then the arrival of `G` at `G.1` joins the pending record, and the world steps.

## Family `owners`: the owners of wants

Assets `A`, `B`, `D1`, `P`, `D2`. The pool is `A`, `B`, `P`. The canonical part `P.1` lists `D1`, under the first type, as its one dependency; `P.2` lists `D2`, under the first type; `D1.1` and `D2.1` list none. The availability source is scripted. Each scenario runs to tick 12.

Controls, each the rule changed:

- `owners/two-wants-first-order`, `owners/two-wants-second-order`: one want of the dependency per want of its parent; and a second control, the wants of dependencies recomputed with a batch alone.
- `owners/two-wants-second-order`, a third control: the wants of an entry in the order of making. In the first order the order of making is the total order, so the control cannot differ there; the sheet named it for both before the first run, which showed it equal to the oracle in the first.
- `owners/parent-changes-dependency`: the wants the core added left as they were when the parent's part changes.
- `owners/preload-over-composition`: one shared want under the world's own entry; and a second control, the makers of the world's own entry ordered by identity alone, the kind ignored.
- `owners/one-maker-two-wants`: a second want added for a maker that already holds one.

- **`owners/two-wants-first-order`**. Record of tick 2: `want P parameters entity:E1 required`, then `want P parameters entity:E2 optional`. Record of tick 3: the arrivals of `P` at `P.1` and of `D1` at `D1.1`, one batch. Record of tick 6: `despawn E1`. Record of tick 9: `despawn E2`.
- **`owners/two-wants-second-order`**. The same, the two commands of tick 2 in the other order.
- **`owners/parent-changes-dependency`**. Record of tick 2: `want P parameters entity:E1 required`. Record of tick 3: the arrivals of `P` at `P.1` and of `D1` at `D1.1`. Record of tick 6: the arrivals of `P` at `P.2` and of `D2` at `D2.1`, one batch.
- **`owners/preload-over-composition`**. The composition's contributor `base` wants `A` as required: the want is in the table at tick 0. Record of tick 1: the arrival of `A` at `A.1`. Record of tick 3: `want A parameters source:host optional` and `want B parameters source:host optional`, a host's preload. Record of tick 4: the arrival of `B` at `B.1`. Record of tick 6: `release A source:host` and `release B source:host`.
- **`owners/one-maker-two-wants`**. Record of tick 2: `want A parameters source:host optional`. Record of tick 3: the arrival of `A` at `A.1`. Record of tick 5: `want A parameters source:host required`. Record of tick 7: `release A source:host`.

## Family `named-set`: a named content set under a moved manifest

Assets `K1`, `K2`, `N`, `X2`, `X3`, `Y`. The pool is `K1`, `K2`, `X2`, `Y`. No canonical part of this family lists a dependency, and each manifest holds each asset under the first type. A live loading capability over a loose store written by the stand-in pipeline.

Controls, each the rule changed: for `named-set/moved-manifest`, the instance's manifest read in place of the set's; for `named-set/recording-replays-under-a-set`, the set's hash placed under the state hash.

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
- record of tick 9: `want X2 parameters source:host required`;
- record of tick 11: `want Y parameters source:host required`;
- record of tick 13: `want N parameters source:host required`;
- record of tick 15: `want X3 parameters source:host required`;
- between tick 18 and tick 19, the package is unmounted.

This is the scenario **`named-set/moved-manifest`**. The oracle states the three worlds, each under a `world` statement. For `under-set` it states the table tick by tick; the harness also compares its state hashes with those of `reference`.

- **`named-set/recording-replays-under-a-set`**. Over the first revision alone, which never moves: a world that names no set, with the composition above; record of tick 4: `want X2 parameters source:host optional`. Its input log is then replayed in a world created under the set's hash. Each world runs to tick 8. The oracle states the one table both must give, tick by tick; the harness compares the state hashes of the two worlds and reads the set's hash in the header of a save of the second.

## Family `restore`: a save read in another process

Assets `D1`, `D2`, `F`, `L1`, `L2`, `P`, `T`, `U`, `Z`. The pool is `L1`, `L2`, `P`, `U`. `T` is of the second asset type; every other asset is of the first. `P.1` lists `D1` as its one dependency. A live loading capability over a loose store written by the stand-in pipeline.

**The saved world.** The content it runs under holds `D1.1`, `D2.1`, `L1.1`, `L2.1`, `P.1`, `T.1`, `U.1`, `Z.1`, and holds `F` at `F.1`, whose artifact is cut short. The world names no content set. Its composition declares the first and the second asset type, and its contributor `base` wants `L2` and `P` as required at tick 0.

- Record of tick 2: `instantiate one L1 parameters required reload on`, `want Z parameters source:host optional`, `want F parameters source:host optional`, `want T second source:host optional`.
- Record of tick 8: `release Z source:host`, then `want U parameters source:host required`.
- The world is saved twice: after tick 5, **the earlier save**, whose table holds no unanswered want and no entry without a want, and after tick 8, **the save**. The process that saved it goes on to tick 16: this is the uninterrupted run, the scenario **`restore/saved-world`**, whose oracle states the table from tick 0 to tick 16. Control: the entries of the table written without their asset type.

**The readers.** Each loads the save in a new process, as a restore under the content at hand but where it says under the recorded content, into the saved world's composition but where it names another, names no content set but where it says so, and steps to tick 16. A reader's record is empty but for what the capability posts and for the commands its scenario states. The oracle states each reader's verdict before its first tick, what a step without waiting names before the answers, and its tables from its first tick on, which is tick 9 but for the reader of the earlier save.

Readers under the content at hand, into the saved world's composition:

- **`restore/earlier-save-continues`**. It reads the earlier save: its first tick is tick 6. The content at hand is the saved world's, with one more asset the saved world never wanted, and the reader names that content's set. Its record of tick 8 holds the two commands of the saved world's record of tick 8. The harness also compares its state hashes with those of the uninterrupted run, from tick 6 on. Control: the hash of the content set under the state hash.
- **`restore/same-content-other-set`**. The content at hand is the saved world's, with one more asset the saved world never wanted, and the reader names that content's set. Control: a want the save holds unanswered left out of what starts a new history.
- **`restore/one-changed`**. The content at hand holds `L1.2` in place of `L1.1`. Control: the answers to the restore posted unmarked.
- **`restore/one-absent`**. The manifest of the content at hand does not hold `L1`. Control: a marked answer that fails treated as a marked answer to a hash, with no event and the instance left as it was.
- **`restore/entries-not-available`**. The content at hand is the saved world's, but that the artifact of `F.1` is whole from the start. Record of tick 11: `want F parameters source:tool optional`. Control: the failed entry answered at the restore.
- **`restore/unanswered-at-another-hash`**. The content at hand holds `U.2` in place of `U.1`. Control: a want the save holds unanswered left out of what starts a new history.
- **`restore/revision-while-waiting`**. The content at hand is the saved world's when the answers are posted. Before the reader's first tick, a revision that holds `L1.2` in place of `L1.1` is put in place. Control: a revision posted to a world that waits for its answers, which replaces a marked answer in the queue.
- **`restore/parent-with-another-dependency`**. The content at hand holds `P.2`, which lists `D2` as its one dependency, in place of `P.1`. Control: answers posted for the assets the saved table holds and for no other.
- **`restore/no-want-entry-wanted-again`**. The content at hand holds `Z.2` in place of `Z.1`. The reader's record of tick 9 holds `want Z parameters source:tool optional`. Control: an entry that holds no want left unanswered at a restore.
- **`restore/no-want-entry-changed`**. The content at hand holds `Z.2` in place of `Z.1`, and no command names `Z`. Control: an entry that holds no want left unanswered at a restore.

Readers under the recorded content, into the saved world's composition:

- **`restore/recorded-content-continues`**. The content at hand is the saved world's, and the request states that the content is the recorded one. The harness also compares its state hashes with those of the uninterrupted run, from tick 9 on. Control: the confirmation made by marked transitions of the first tick's record.
- **`restore/recorded-content-refused`**. The content at hand holds `Z.2` in place of `Z.1`, and the request states that the content is the recorded one. Control: an entry that holds no want left unconfirmed.

Readers under the content at hand, into another composition. The content at hand is the saved world's but where a reader says otherwise:

- **`restore/another-composition-refused`**. The composition at hand declares the first asset type alone. The request names no type as dropped. Control: the entries of a type the composition lacks dropped in silence.
- **`restore/another-composition-dropped`**. The composition at hand declares the first asset type alone. The request names `second` as dropped. Control: the event that records the restore names no dropped entry.
- **`restore/another-composition-asset-absent`**. The composition at hand declares the first asset type alone, the manifest of the content at hand does not hold `T`, and the request names no type as dropped. Control: the type of an entry read from the manifest at hand.
- **`restore/drop-names-a-type-with-no-entry`**. The composition at hand declares the first asset type alone. The request names `second` and `third` as dropped. Control: a type named as dropped, of which the table holds no entry, refuses the restore.
- **`restore/drop-names-a-declared-type`**. The composition at hand declares the first asset type alone. The request names `second` and `parameters` as dropped. Control: a type named as dropped that the composition has drops nothing and refuses nothing.
- **`restore/drop-names-a-declared-type-with-no-entry`**. The composition at hand declares the first asset type and `third`. The request names `second` and `third` as dropped. Control: a type named as dropped that the composition has, and of which the table holds no entry, refuses nothing.

## Family `retry`: a want added on a failed entry

Assets `A`, `D`, `F`, `P`. The pool is `A`, `F`, `P`. `P.1` lists `D` as its one dependency. A live loading capability over a loose store written by the stand-in pipeline. The content holds `A.1`, `D.1`, `F.1`, `P.1` and never moves: no revision is put in place. When a scenario starts, the artifacts of `D.1` and `F.1` are cut short. **The repair** puts the whole artifacts of `D.1` and `F.1` in the store, under the same hashes. The composition's contributor `base` wants `A` as required at tick 0. Each scenario runs to tick 12.

Scenarios on `F`. But where one says otherwise: record of tick 2: `want F parameters source:host optional`; the repair between tick 4 and tick 5.

- **`retry/optional-want`**. Record of tick 6: `want F parameters source:tool optional`. Control: a failed entry stays failed until the arrival that answers the want added on it.
- **`retry/required-want`**. Record of tick 6: `want F parameters source:tool required`. Then a step without waiting, before the capability has posted. The world is saved after tick 6: **the save** of this family. Control: a failed entry stays failed until the arrival that answers the want added on it.
- **`retry/same-maker-changes-policy`**. Record of tick 6: `want F parameters source:host required`. Then a step without waiting. Control: a want its maker already holds, made again with another policy, counted as a want added.
- **`retry/release-and-want-again`**. Record of tick 6: `release F source:host`, then `want F parameters source:host optional`. Control: a failed entry stays failed until the arrival that answers the want added on it.
- **`retry/want-in-the-record-of-the-failure`**. No repair is made. The record of tick 3 holds `want F parameters source:tool optional`, with what the capability posted. Control: an arrival that leaves an entry failed for the same reason emits its event.

A scenario on `P` and its dependency, in which no command names `F`:

- **`retry/dependency-of-a-parent-wanted-again`**. Record of tick 2: `want P parameters source:host optional`. The repair between tick 4 and tick 5. Record of tick 6: `release P source:host`. Record of tick 7: `want P parameters source:host optional`. Control: a want the core adds on a failed entry leaves it failed.

Readers of the save, each over the repaired store, into the saved world's composition, naming no content set, stepped to tick 12:

- **`retry/save-under-the-content-at-hand`**. A restore under the content at hand. Control: a failed entry stays failed until the arrival that answers the want added on it, so that the save holds the entry failed.
- **`retry/save-under-the-recorded-content`**. A restore under the recorded content. The harness also compares its state hashes with those of `retry/required-want`, from tick 7 on. Control: the same.

## Family `types`: the asset type an entry holds

Assets `A`, `D`, `F`, `K`, `P`, `Q`, `S`, `W`. The pool is `A`, `K`, `P`. `S` and `W` are of the second asset type; every other asset is of the first. `K.1` lists `S`, `P.1` lists `D` and `W.1` lists `D`, each as its one dependency and under the type the manifest holds it under. A live loading capability over a loose store. The content holds one hash of each asset and never moves; the artifact of `F.1` is cut short. One part is not what the stand-in pipeline writes: `Q.1` lists `D` as its one dependency under the second type, where the manifest holds `D` under the first type and lists it under the first type for `Q`. The stand-in pipeline refuses such a part, so the harness puts it in place itself. The composition declares the first and the second type and makes no want: the table is empty at tick 0. Each scenario runs to tick 12.

- **`types/want-of-another-type`**. Record of tick 2: `want A parameters source:host optional`, then `want A second source:tool optional`. Record of tick 5: `want A second entity:E1 required`. Control: a want that states another type than its entry holds added as any want.
- **`types/wrong-first-want`**. Record of tick 2: `want A second source:host optional`. The record of tick 3 holds `want A parameters source:tool optional`, with what the capability posted. Record of tick 5: `want A parameters source:tool optional`. Control: the type a first want wrote kept for good, a later want of another type being refused whatever the entry's availability.
- **`types/wrong-first-want-corrected-by-its-maker`**. Record of tick 2: `want A second source:host optional`. Record of tick 5: `want A parameters source:host optional`. Control: a want its maker already holds, made again with another type, adds no want and is no new load.
- **`types/failed-for-another-reason`**. Record of tick 2: `want F parameters source:host optional`. Record of tick 5: `want F second source:tool optional`. Control: a want of another type replaces the type of an entry failed for any reason.
- **`types/dependency-of-another-type`**. Record of tick 2: `want K parameters source:host required`. Record of tick 6: `want S parameters source:tool optional`. The world is saved after tick 4. The harness also runs the scenario in a world whose composition declares the two types in the other order, and compares the state hashes of the two worlds. Control: the entry of a dependency takes the type its parent's entry holds.
- **`types/core-want-on-a-wrongly-typed-entry`**. Record of tick 2: `want D second source:host optional`. Record of tick 5: `want P parameters source:tool required`, then a step without waiting, before the capability has posted. Record of tick 8: `want D parameters source:tool optional`, then a step without waiting, before the capability has posted. Control: a want the core adds states the type its parent's part lists and is refused where the entry holds another.
- **`types/part-lists-a-contradicted-type`**. Record of tick 2: `want Q parameters source:host required`. Control: the dependencies a part lists not checked against the manifest.
- **`types/parent-of-the-second-type`**. Record of tick 2: `want W second source:host optional`. The world is saved after tick 4. Control: the entries of the table written without their asset type.

Readers of the save of `types/dependency-of-another-type`, each under the content at hand, which is the saved world's, into a composition that declares the first asset type alone:

- **`types/drop-not-named`**. The request names no type as dropped. Control: the entry of a dependency takes the type its parent's entry holds.
- **`types/drop-refused-for-a-kept-parent`**. The request names `second` as dropped. Control: a type dropped whatever kept entry lists a dependency of it.

A reader of the save of `types/parent-of-the-second-type`, under the content at hand, which is the saved world's, into a composition that declares the first asset type alone, naming no content set, stepped to tick 12:

- **`types/dropped-parent-of-a-kept-dependency`**. The request names `second` as dropped. Control: the want a dropped parent owned left on its dependency.

## Family `dependencies`: the wants of dependencies where no batch brings them

Assets `D`, `P`. The pool is `P`. `P.1` lists `D` as its one dependency. A live loading capability over a loose store written by the stand-in pipeline; the content holds `D.1` and `P.1` and never moves. The table is empty at tick 0. Each scenario runs to tick 12, and its oracle states what the records hold after the last command.

- **`dependencies/parent-wanted-again-at-its-fall`**. Record of tick 2: `want P parameters source:host required`. Record of tick 5: `release P source:host`. Record of tick 6: `want P parameters source:tool optional`. Control: the wants of dependencies added and recomputed with a batch alone.
- **`dependencies/required-want-on-a-parent-in-force`**. Record of tick 2: `want P parameters source:host optional`. Record of tick 5: `want P parameters entity:E1 required`. Record of tick 7: `despawn E1`. Control: the wants of dependencies added and recomputed with a batch alone.
- **`dependencies/closure-over-an-available-dependency`**. Record of tick 2: `want D parameters source:host optional`. Record of tick 5: `want P parameters source:tool required`, then a step without waiting, before the capability has posted. Control: the batch of a want's closure holds an arrival for every asset of the closure, one the table holds available included.

## Family `revision`: what the batch of a revision holds

Assets `D`, `N`, `P`, `V`, `Z`. The pool is `P`, `Z`. A live loading capability over a loose store written by the stand-in pipeline. The world names no content set. The first revision holds `D.1`, `N.1`, `P.1`, `V.1`, `Z.1`; `P.1` lists `D` and `V` as its two dependencies. Each scenario names its later revision, which is put in place between tick 5 and tick 6 and holds what the first holds but where it says otherwise. The table is empty at tick 0. Each scenario runs to tick 12.

- **`revision/parent-gains-a-dependency`**. Record of tick 2: `want P parameters source:host required`. The later revision holds `P.2` in place of `P.1`; `P.2` lists `D`, `N` and `V`. Control: the batch of a revision holds an arrival for each asset the revision changes that the table names, and for no other.
- **`revision/former-dependencies-changed-with-their-parent`**. Record of tick 2: `want P parameters source:host required`. The later revision holds `D.2`, `V.2` and `P.3` in place of `D.1`, `V.1` and `P.1`; `P.3` lists `N` alone. Control: the wants of the dependencies a new part no longer lists ended before any arrival of the batch is applied. A second control, the reading not taken: the wants of former dependencies end at their parent's arrival.
- **`revision/entry-with-no-want-wanted-again`**. Record of tick 2: `want Z parameters source:host optional`. Record of tick 5: `release Z source:host`. The later revision holds `Z.2` in place of `Z.1`. Record of tick 6: `want Z parameters source:tool optional`, with what the capability posted. Control: the batch of a revision holds no arrival for an entry that holds no want.
- **`revision/entry-with-no-want-left`**. The same, without the command of tick 6. Control: an arrival applied to an entry that no want names.

## Family `pending-parent`: the gate over a parent pending without its dependency

Assets `D`, `G`, `P`. The pool is `G`, `P`. `P.1` lists `D` as its one dependency. The table is empty at tick 0. The availability source is scripted, and it posts here what the loading capability never posts: a parent without the assets of its closure. Each scenario runs to tick 10.

- **`pending-parent/required`**. Record of tick 3: `want P parameters source:host required`. A step without waiting. Then the arrival of `P` at `P.1` joins the pending record of tick 4, with no arrival of `D`. A step without waiting. Then the arrival of `D` at `D.1` joins the pending record, and the world steps. Control: the gate leaves out the want the core would add from the part of a pending arrival. A second control, the reading not taken: the step names what the state holds unanswered.
- **`pending-parent/optional`**. Record of tick 3: `want P parameters source:host optional`, then `want G parameters source:host required`. A step without waiting. Then the arrivals of `G` at `G.1` and of `P` at `P.1` join the pending record of tick 4, with no arrival of `D`. A step without waiting. Record of tick 6: the arrival of `D` at `D.1`. Control: the gate counts the want the core would add as required, whatever the wants of its parent.
- **`pending-parent/pending-want-left-out`**. As `pending-parent/optional` to the arrivals that join, the pending record of tick 4 also holding `want P parameters source:tool required`. A step without waiting. After tick 4, a step without waiting. Then the arrival of `D` at `D.1` joins the pending record of tick 5, and the world steps. Control: a pending command that adds a want on the parent counted in the policy of the want the core would add.
- **`pending-parent/pending-release-applied`**. Record of tick 3: `want P parameters source:host required`, `want P parameters source:tool optional`, then `want G parameters source:host required`. A step without waiting. Then the pending record of tick 4 gains `release P source:host` and the arrivals of `G` at `G.1` and of `P` at `P.1`, with no arrival of `D`. A step without waiting. Record of tick 6: the arrival of `D` at `D.1`. Control: a pending command that releases a want of the parent left out of the policy of the want the core would add.

## Family `record`: elements of a record that change nothing, and the kind of a maker

Assets `A`, `F`. The pool is `A`. The availability source is scripted. The table is empty at tick 0 but where a scenario says otherwise. Each scenario runs to tick 10.

- **`record/arrival-that-changes-nothing`**. Record of tick 2: `want A parameters source:host optional`, then `want F parameters source:host optional`. Record of tick 3: the arrival of `A` at `A.1` and an arrival of `F` failed for `invalid-part`. Record of tick 5: the same two arrivals again. Record of tick 7: an arrival of `F` failed for `artifact-gone`. Control: an arrival that leaves an entry as it was emits its event.
- **`record/want-after-the-despawn-of-its-owner`**. Record of tick 2: `despawn E1`, then `want A parameters entity:E1 required`. Record of tick 3: the arrival of `A` at `A.1`. Control: a despawn that stands in a record applied at the structural change point of its tick.
- **`record/two-makers-of-one-identity`**. The composition's contributor `base` wants `A` as required at tick 0. Record of tick 1: the arrival of `A` at `A.1`. Record of tick 3: `want A parameters source:twin optional`. Record of tick 5: `release A source:twin`. Control: a want of the world's own entry carries the identity of its maker without its kind.
