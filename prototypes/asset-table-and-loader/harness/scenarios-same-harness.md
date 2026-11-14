# Scenarios of the same-harness stage

This sheet states the inputs of the scenarios of five more claims, run on the harness of the first stage with no stand-in beyond those of that stage. It follows [`scenarios.md`](scenarios.md) for the conventions and for the format of an oracle, and repeats neither. It states no expected result: an oracle is written by hand from the design text, by a writer who has not read the build, and the build is written from the same text by someone who has not read the oracles. Where the two differ, the text arbitrates.

Four of the claims are judged on states, and each has a family and an oracle: a want made by a system (`system-wants`), several worlds on one loading capability (`shared-loader`), a mount while worlds exist (`mount`) and an excess over a world's budget (`budget`). The fifth is a magnitude, the cost of the gate on a large table (`gate-cost`): it has inputs and no oracle.

Every scenario follows the design text as amended after the first stage. Every asset of this sheet is of the first synthetic type, `parameters`.

## Conventions added to those of the first sheet

- **Several worlds on one instance** are stepped in rounds. In a round the host steps every world one tick, in the order the scenario lists the worlds, without waiting between two worlds. Before each round it waits until the loading capability has nothing left to post, so that what the capability posts in answer to a world's tick is in the record of that world's next tick. "Between tick `a` and tick `b`" means after the round of tick `a` and before the round of tick `b`.
- **The scripted system.** In the family `system-wants` the harness adds one system, `wanter`, to the schedule of the stand-in world. It is contributed by `contributor:logic`, whose identity is 9. It runs once in every tick, after the tick's input fixed point and before its structural change fixed point, and it declares access to the wants of the asset table. "System at tick `n`" lists what the system does when it runs in tick `n`, through that access and in the order given: it makes a want, it releases one, or it asks for the despawn of an entity as a system of the workload does. A want it makes under the world's own entry is written `want <asset> <type> world <policy>` in this sheet, and its release `release <asset> world`. Which maker such a want carries, and at which fixed point what the system does is applied, are for the writer and for the builder to read in the text.
- **Sizes and budgets.** In the family `budget` every content has a size in bytes: the size of its canonical part, as the domain manifest gives it. The stand-in pipeline pads a part to the size stated. A world's budget for its asset data is stated in bytes with its scenario and does not change during it.

## Additions to the format of an oracle

Beyond the format of the first sheet, with the asset type in an entry, in a want of a record and in an instantiation as that sheet states them:

- `world <name> as <other> shifted <k>`: the statement of the world `<name>` is the statement of the world `<other>` of the same scenario with every tick number raised by `k`. It stands in place of the tick blocks of `<name>`.
- `rejected <asset> <hash>`: one line per arrival rejected at tick `n` over the world's budget, in the order applied. The result of the step that runs tick `n` names each. The lines stand after the `record` lines of their tick.
- The reason `over-budget`, in an availability, in an event and in a `names-failed` line. It exists for the comparison of the family `budget` alone: the design text names no such reason.
- The record element `arrival <asset> <hash> for <root>`: an arrival that carries the mark of the candidate on the closure of one want, in the family `budget`. `<root>` is an asset. No other candidate and no other family writes the mark.

What the scripted system does has no statement of its own. Where the text makes it an element of no record, the oracle states it by the table of its tick, and a comment says what the system did. A want the system makes under the world's own entry is written with one of the owners of the first sheet, the one the writer reads in the text.

## Family `system-wants`: a want made by a system inside a tick

The claim: a want made by a system through its declared access behaves as one made by a command. The scenarios are those of the asset table spike that turn on wants, its wants and its gate during a run, at the size of this harness, with the wants made by the scripted system. Two more put a want of the system in the tick of an arrival of its asset and in the tick of its asset's fall, where the order of a tick decides.

Assets `A`, `F`, `G`, `K`, `M`, `R`. The pool is `A`, `K`, `R`. The canonical part `R.1` lists `M` as its one dependency and `M.1` lists `K`: a closure of three levels. The table is empty at tick 0. The availability source is scripted but where a scenario says otherwise.

- **`system-wants/entity-wants`**. Two worlds, each run to tick 14.
  - `by-system`. System at tick 2: `want A parameters entity:E1 required`, then `want A parameters entity:E2 optional`. Record of tick 3: the arrival of `A` at `A.1`. System at tick 5: `release A entity:E1`. System at tick 7: `release A entity:E2`. System at tick 10: `want A parameters entity:E2 optional`. Record of tick 12: the arrival of `A` at `A.2`.
  - `by-command`. The same, each want and each release being a command of the record of the tick at which the system makes it in `by-system`. The scripted system stands in the schedule and does nothing.

  The oracle states both worlds. The harness also reports, tick by tick, whether the state hashes of the two worlds are equal. Control, on `by-system`: what the system does is applied at the input fixed point of the next tick, before that tick's record.
- **`system-wants/despawn-by-a-system`**. Run to tick 10. System at tick 2: `want A parameters entity:E1 optional`, then `want A parameters entity:E2 optional`. Record of tick 3: the arrival of `A` at `A.1`. System at tick 5: `despawn E1`. System at tick 7: `despawn E2`. Control: as above.
- **`system-wants/closure-under-the-world-entry`**. Run to tick 14. System at tick 2: `want R parameters world optional`. Record of tick 4: the arrivals of `K` at `K.1`, of `M` at `M.1` and of `R` at `R.1`, one batch. Record of tick 5: `want R parameters source:host optional`. System at tick 7: `want R parameters world required`. System at tick 9: `release R world`. Record of tick 11: `release R source:host`. Control: as above.
- **`system-wants/arrival-in-the-tick-of-the-want`**. Run to tick 8. Record of tick 3: the arrival of `A` at `A.1`. System at tick 3: `want A parameters entity:E1 optional`. Record of tick 5: the arrival of `A` at `A.1`. Control: the want of the system is applied as a command of the record of its tick, before the arrivals.
- **`system-wants/want-again-at-the-tick-of-the-fall`**. Run to tick 10. System at tick 2: `want A parameters entity:E1 optional`. Record of tick 3: the arrival of `A` at `A.1`. System at tick 5: `release A entity:E1`. System at tick 6: `want A parameters entity:E1 optional`. Record of tick 8: the arrival of `A` at `A.2`. Control: the want and the release of the system are applied as commands of the record of their tick, before the falls.
- **`system-wants/gate-live`**. A live loading capability over a loose store written by the stand-in pipeline, whose content holds `K.1`, `M.1`, `R.1`, and `F` at a hash whose artifact is cut short. Run to tick 12. System at tick 3: `want R parameters entity:E1 required`. System at tick 6: `want F parameters entity:E2 required`. System at tick 8: `want F parameters entity:E2 required`, the same want again. System at tick 9: `want F parameters entity:E1 required`. The oracle states what the records hold. Control: what the system does is applied at the input fixed point of the next tick, before that tick's record.
- **`system-wants/gate-pending`**. Run to tick 8. System at tick 3: `want G parameters entity:E1 required`. A step without waiting. Then, pending for tick 4: `release G entity:E1`, a command of `source:host`. A step. Control: as for `gate-live`.

**What the family leaves out of the spike's scenarios.** An instance made by a system, the stand-in instance being made by a command alone. The spike's revision, its table under the hash, its save and its one resident copy, whose wants are the composition's and are made before the first tick. Two systems that make wants in one tick, and so the order of a schedule.

## Family `shared-loader`: several worlds on one loading capability

The claim: several worlds on one loader each receive their own batches.

Assets `C`, `J`, `L`, `M`, `P`, `Q`. The pool is all six. `M.1` lists `C` as its one dependency, `P.1` lists `J` and `M`, and `Q.1` lists `L` and `M`: the closures of `P` and of `Q` overlap on `M` and on `C`. One instance and one live loading capability over a loose store written by the stand-in pipeline.

- The first revision's domain manifest, **the set**, holds `C.1`, `J.1`, `L.1`, `M.1`, `P.1`, `Q.1`.
- **The later revision** holds `C.2` and `L.2` in place of `C.1` and `L.1`, and the four others unchanged. It is put in place before any world is created and is the instance's manifest for the whole scenario. The loose store keeps the artifacts of both revisions.

Sixteen worlds, each with an empty table at tick 0: four patterns, each held by four worlds. The world `<pattern>-<o>`, with `o` from 0 to 3, runs its pattern `o` ticks later: a record the pattern gives to tick `n` is the record of tick `n + o` of that world. The worlds are listed, and stepped, in the order `first-0`, `second-0`, `third-0`, `fourth-0`, `first-1`, and so on to `fourth-3`. Each is stepped to tick 16.

- **Pattern `first`**, naming no set. Record of tick 2: `want P parameters source:host required`. Record of tick 4: `want Q parameters entity:E1 required`. Record of tick 7: `release P source:host`. Record of tick 10: `want J parameters entity:E2 optional`.
- **Pattern `second`**, naming no set. Record of tick 2: `want C parameters entity:E1 optional`, then `want Q parameters source:host required`. Record of tick 5: `want P parameters entity:E2 optional`. Record of tick 8: `release Q source:host`. Record of tick 11: `release C entity:E1`.
- **Pattern `third`**: the records of `first`, in a world created under the set's hash.
- **Pattern `fourth`**: the records of `second`, in a world created under the set's hash.

**The delays.** The scenario is run three times: with no delay; with each batch held back, before it is posted, for 5 milliseconds times the count of worlds listed after the world it answers, so that the batches of one round are ready in the reverse of the order in which their wants left; and with each batch held back for a duration drawn from the clock, between 20 and 150 milliseconds.

This is the scenario **`shared-loader/sixteen-worlds`**. The oracle states the four worlds of offset 0 in full and the twelve others by their shift. Control: a batch is posted on the queue of every world of the instance, and not on the queue of the world whose want it answers alone.

**What the design leaves out.** Sixteen tables that differ in more than a shift and a manifest: four patterns are written by hand, and a fault that needs a fifth is not met. A world seen gated while another steps: the host waits before each round, so no step meets a closed gate, and the tick an answer lands in is a function of the scenario and not of a delay. A revision, a mount or a restore while the sixteen worlds run. Worlds stepped at different rates, or from several threads of the host.

## Family `mount`: a mount while worlds exist

The claim: a mount while worlds exist reaches each world as one revision.

Assets `O`, `Q`, `T`. The pool is all three. One instance and one live loading capability over a loose store written by the stand-in pipeline. **The base**, the instance's domain manifest before a mount and after an unmount, holds `O.1` and holds neither `Q` nor `T`.

**`mount/package-under-worlds`.** The package holds `O.9` and `Q.9` and is mounted with leave to override: one overriding entry and one asset of its own.

Five worlds, listed and stepped in this order, each to tick 18:

- `held`, naming no set: the composition's contributor `base` wants `O` as required at tick 0.
- `late`, naming no set: an empty table at tick 0.
- `early`, naming no set: an empty table at tick 0, and the record of tick 2 holds `want Q parameters source:tool optional`.
- `released`, naming no set: an empty table at tick 0; the record of tick 13 holds `release O source:host`, then `release Q source:host`; and the record of tick 14 holds `want O parameters source:tool optional`.
- `under-set`: as `held`, created under the hash of the base.

What happens, the same for each world:

- between tick 4 and tick 5, the package is mounted;
- record of tick 7: `want O parameters source:host required`;
- record of tick 9: `want Q parameters source:host required`;
- between tick 13 and tick 14, the package is unmounted.

The oracle states the five worlds, each under a `world` statement. Controls: for the four worlds that name no set, the arrivals of a mount or of an unmount are posted to a world one per tick, in identifier order, in place of one batch; for `released`, a second control, in which the batch of a mount or of an unmount holds no arrival for an entry that has no want; for `under-set`, the instance's manifest stands in place of the set's.

**`mount/own-asset-with-a-dependency`.** The package holds `O.9` and `T.9`, whose part lists `O` as its one dependency, and is mounted with leave to override. One world that names no set, with an empty table at tick 0, stepped to tick 12. Between tick 2 and tick 3 the package is mounted. Record of tick 4: `want T parameters source:host required`. Between tick 8 and tick 9 the package is unmounted. Control: the arrivals of the unmount are posted one per tick, in identifier order. A second control, the reading not taken: the wants the core added for the dependencies of a parent that is failed, or whose arrival is rejected, stand while that parent holds a want.

**What the family leaves out.** A mount without leave to override. Two packages mounted in an order. An overriding entry whose part lists a dependency the base does not hold. An instance made from an asset of the package. A world gated when the mount or the unmount lands. A package whose asset is of the second type.

## Family `gate-cost`: the gate evaluated on a large table

The claim: a gate evaluated on a large table costs under a tenth of a tick. It is a magnitude: it has no oracle and no control, and it is judged on one quiet machine, in the release profile, over at least five repetitions.

**The world.** The reference workload at default size, 10 000 entities, at 1, 4 and 8 workers, with a scripted availability source.

**The table.** 2 000 roots `R0000` to `R1999` and 8 000 leaves `L0000` to `L7999`, every one of the type `parameters`. The first content of the root of rank `i` lists the four leaves of ranks `4i` to `4i + 3`; a leaf lists nothing. Unlike the other families, the identifiers are drawn from the fixed stream of the spikes, in no relation to the names. The pool is the sixteen leaves `L0000` to `L0015`. The record of tick 1 holds, for each root, the command `want <root> parameters source:host required`; the record of tick 2 holds the arrivals of the 10 000 assets, one batch. From tick 2 on the table holds 10 000 entries, all available, under 2 000 wants of `source:host` and 8 000 wants the core added. One more asset, `G`, lists nothing and is not in the table.

**The pending record**, in two sizes:

- **small**: the command `release R1000 source:host`, and the arrival of `R0000` at a second content that lists the same four leaves.
- **large**: 100 commands, the releases by `source:host` of its wants of `R1000` to `R1049` and 50 optional wants of `source:tool` on the leaves `L4000` to `L4049`; and 1 000 arrivals, the roots `R0000` to `R0999` each at a second content that lists its four leaves and one leaf more, the first leaf of the next root, which the table holds available.

**The three cases**, each with each size:

- **`gate-cost/open`**: every required asset is answered. The pending record is the one above, assembled after tick 102.
- **`gate-cost/closed`**: the record of tick 103 holds `want G parameters source:host required`, so the world is gated from the boundary after tick 103. The pending record is the one above, and nothing answers `G`.
- **`gate-cost/opens`**: as `closed`, the pending record holding the arrival of `G` as well.

**What is timed.** One evaluation of the gate: from the moment the host asks whether the world may run its next tick, the pending record being assembled, to the verdict, open, or closed with the assets awaited named. The assembly of the pending record, its application and the tick are not in it. The evaluation is made 100 times on the same pending record, which is applied by none of them, and the median is kept.

**Against what.** The median duration of one tick of the same world at the same worker count, over the ticks 3 to 102, each with an empty record and an open gate.

**What is reported**, per case, per size and per worker count, for each repetition: the median evaluation, the median tick and their ratio; and, once, how the build obtains the table it evaluates the gate on, a copy or another form. The row names no worker count and no size of record: the figure the claim is read on here is the largest of the eighteen ratios, and the report gives all of them.

## Family `budget`: an excess over a world's budget

The claim: an excess over a world's budget is rejected identically. The design text leaves open what is rejected and names three candidates: the single arrival, the closure of one want, the whole drain of a tick. It defines none of them, so this sheet does, for the comparison alone, and chooses none. Each case runs once under each candidate, as the scenario `budget/<case>/<candidate>`.

**What the three candidates share.**

- **The count** of a world is the sum of the sizes of the contents its table holds available, an entry that holds no want included.
- **An arrival to a hash is tested where it is applied**: after the commands of its record, after the core has added the wants that the parts of the batch list, and before the falls. An arrival to failed has no size and is not tested.
- **The budget is exceeded** where the count would stand above it; a count equal to the budget is within it. An arrival at another hash of an asset the table holds available counts the new size in place of the former one.
- **An accepted arrival** is applied as any arrival is.
- **A rejected arrival** is not applied, and the result of the step names it. Where the table holds the asset available, the entry stays as it was and no event is emitted. Where it does not, the entry becomes failed with the reason `over-budget`, as by a failed arrival, with the event of a failure.

**The candidates.**

- **`single-arrival`**. Each arrival to a hash is tested alone, in the order applied, which is the order of the asset identifiers, against the count as the arrivals accepted before it leave it. It is accepted where the count with it is within the budget and rejected otherwise.
- **`closure-of-a-want`**. The arrivals that answer one want are accepted together or rejected together. The record gains a mark for it: every arrival to a hash names a **root**, an asset.
  - An arrival posted in answer to a want names the asset that want names.
  - An arrival that stands in the closures of several wants answered between two drains names, of the assets those wants name, the one that stands first in identifier order.
  - An arrival of the batch of a revision, of a mount or of an unmount names the asset the move changed that brought it: itself, or the changed asset whose closure holds it, the first in identifier order where several do.

  The arrivals of one record that name one root are a group. The groups are tested whole, in the identifier order of their roots, each against the count as the groups accepted before it leave it: a group is accepted where the count with all its arrivals is within the budget, and rejected otherwise. Every group is decided before the first arrival is applied; the arrivals are then applied or rejected in identifier order.
- **`whole-drain`**. The arrivals to a hash of one record are one group, accepted where the count with all of them is within the budget and rejected otherwise, every one of them.

**A fourth candidate, `derived-closure`.** The design text now proposes a unit, with a count and a rule on the wants the core adds, and this candidate is that proposal, restated here for the comparison. The three candidates above are as they were: their definitions and their count do not change, and what they share, stated above, stays theirs. The fourth differs from them on the points below and shares the rest. Its scenarios are `budget/<case>/derived-closure`: the six cases below, and two more cases that run under this candidate alone. On 2026-11-14 the founder took this candidate as the unit of a budget and took, for every candidate, the reading that the wants the core adds follow the parts in force: the three others are kept for comparison, each under its own definition and under that reading, a want they add from a part before the test ending at the recomputation where that part is not put in force.

- **No mark.** The record is the one of `single-arrival`: an arrival is written `arrival <asset> <hash>`. The core derives the bounds of a closure itself, from the parts it holds at the fixed point.
- **When, and in which order.** At a fixed point, once the commands of the record are applied, the core derives the groups defined below and decides each of them, before it adds any want from the parts of the batch and before any arrival is applied. It then adds the wants that the parts of the batch list, to any depth, the parts of the rejected arrivals left out. It then goes through the arrivals in identifier order, applying each but the rejected ones; a rejected arrival that fails its asset does so at its own place in that order, which is the place of its event. It then recomputes the wants it added from the parts the table then holds available.
- **The wants the core adds follow the parts in force.** The wants that a rejected part lists are not added. At the recomputation the want of a dependency that the part in force no longer lists ends. A parent that is failed, or that never rose, owns no such want: the wants it owned on its dependencies end at the fixed point that failed it, and a dependency left with no want falls at the next tick as any asset does.
- **The count** of a world is the sum of the sizes of the contents its table holds available, less the entries that fall at the same fixed point whatever the test gives: an entry that holds no want once the commands of the record are applied, whose last want ended during the tick before, and that the closure of no root reaches.
- **Roots, closures and groups.** A root is an asset the table holds under a want the core did not add: a want of an entity or of the world's own entry. The closure of a root is the root with every asset reached from it through the lists of dependencies, read for each asset from the part in force and from the part of its arrival to a hash in the record, from both where it has both; it stops at an asset that has neither. The group of a root is the set of the arrivals to a hash of the record whose asset stands in its closure. An arrival to failed stands in no group, and neither does an arrival at the hash the entry holds, nor an arrival whose asset stands in the closure of no root: each is applied or changes nothing as it would without a budget.
- **The test.** The groups are tested in the identifier order of their roots. A group is accepted where the count stays within the budget once every arrival of the group that no earlier group accepted is counted, an arrival at another hash of an asset the table holds available counting the new size in place of the former one; its arrivals are then part of the count. Otherwise the group is rejected. A count equal to the budget is within it. An arrival that stands in a group is applied where at least one group that holds it is accepted, and rejected where every group that holds it is rejected.
- **A rejected arrival** is not applied, and the result of the step names it. Where the table holds the asset available, the entry stays as it was and no event is emitted. An asset that a rejected part alone lists is named by no want: its rejected arrival leaves no entry, no failure and no event. An asset the table does not hold available and that a want names, the root of a rejected group, an asset wanted in its own right or a dependency that a part in force lists, becomes failed with the reason `over-budget` and the event of a failure. Such a failure is an answer: the gate opens, the step names the asset where a required want names it, and a want added on the entry retries the load.
- **What a rejection leaves.** A rejected first load leaves its root failed, and nothing of its closure in the table but what another accepted group or a part in force holds. A rejected replacement leaves its parent at its former part, with the wants of that part and no trace of what the new part listed.
- **The size is a stand-in.** The design text proposes that an arrival names the size of what it puts in force and that the entry holds it. That is neither built nor written here: the format of an oracle writes no size, the sizes are those of the table below, and the build reads the length of the part, as it does for the three other candidates.

Assets `B`, `D`, `K`, `N`, `R`, `S`, `V`, `W`, `X`. The pool is `K`, `R`, `S`, `W`. A live loading capability over a loose store written by the stand-in pipeline. One world per scenario, which names no set, with a budget of 10 000 bytes but where a case says otherwise.

| Content | Size in bytes | Dependencies its part lists |
| --- | --- | --- |
| `B.1` | 9 000 | none |
| `D.1` | 6 000 | none |
| `K.1` | 5 000 | none |
| `K.2` | 11 000 | none |
| `N.1` | 2 000 | none |
| `R.1` | 2 000 | `D` |
| `R.2` | 2 000 | `D`, `X` |
| `S.1` | 1 000 | none |
| `S.2` | 2 000 | none |
| `V.1` | 1 000 | `N`, `X` |
| `W.1` | 1 000 | `N` |
| `X.1` | 9 000 | none |

The instance's manifest holds the first content of every asset. The cases:

- **`root-and-dependency`**. The composition's contributor `base` wants `K` as required at tick 0. Record of tick 3: `want R parameters source:host required`. Run to tick 8.
- **`closure-over-the-budget`**. A budget of 7 000 bytes, and an empty table at tick 0. Record of tick 3: `want R parameters source:host required`. Run to tick 8.
- **`small-beside-large`**. The contributor `base` wants `K` as required at tick 0. Record of tick 3: `want B parameters source:host optional`, then `want S parameters source:host required`. Run to tick 8.
- **`overlapping-closures`**. An empty table at tick 0. Record of tick 3: `want V parameters source:host optional`, then `want W parameters source:host required`. Run to tick 8.
- **`revision-that-grows`**. The contributor `base` wants `K` and `S` as required at tick 0. Between tick 3 and tick 4 a revision that holds `K.2` and `S.2` in place of `K.1` and `S.1` is put in place. Run to tick 8.
- **`arrival-at-the-tick-of-a-fall`**. An empty table at tick 0. Record of tick 2: `want K parameters source:host optional`. Record of tick 5: `release K source:host`, then `want B parameters source:host optional`. Record of tick 8: `release B source:host`, then `want B parameters source:host optional`. Run to tick 12.

Two more cases run under `derived-closure` alone; the three other candidates do not run them:

- **`replacement-with-a-new-dependency`**. A budget of 10 000 bytes. The contributor `base` wants `R` as required at tick 0. Between tick 3 and tick 4 a revision that holds `R.2` in place of `R.1` is put in place. Run to tick 8.
- **`two-closures-that-fit-alone`**. A budget of 10 000 bytes, and an empty table at tick 0. Record of tick 3: `want B parameters source:host optional`, then `want D parameters source:host required`. Run to tick 8.

Control of every case, under every candidate: the budget only reports, and every arrival is applied. A second control for `arrival-at-the-tick-of-a-fall`: the falls before the record.

Controls under `single-arrival`, `closure-of-a-want` and `whole-drain`. For `closure-over-the-budget` under each of the three, and for `overlapping-closures` and `root-and-dependency` under `closure-of-a-want` and under `whole-drain`, a second control, the reading not taken: the wants the core added for the dependencies of a parent that is failed, or whose arrival is rejected, stand while that parent holds a want.

Controls under `derived-closure`. The control of every case, the budget only reports, is a control of seven of its eight cases: under this candidate `arrival-at-the-tick-of-a-fall` rejects no arrival, so that control cannot differ there and is not named for it.

- `arrival-at-the-tick-of-a-fall`, its one control, in place of the two it has under the three other candidates: the count holds the entries that fall at the same fixed point.
- `root-and-dependency`, `closure-over-the-budget` and `replacement-with-a-new-dependency`, a second control, the same for the three: the wants that a rejected part lists are added before the arrivals are applied.

**What the family leaves out.** A prepared form built once, which the text counts with its part. A budget changed during a run. A restore, under the recorded content or under the content at hand. A world under a named set. A gated world whose pending record gathers several batches. A mark that passes through the queue where a later arrival replaces an earlier one.
