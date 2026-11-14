# Scenarios of the stand-in stage

This sheet states the inputs of the scenarios of the third and last stage. It follows [`scenarios.md`](scenarios.md) and [`scenarios-same-harness.md`](scenarios-same-harness.md) for the conventions and for the format of an oracle, and repeats neither. It states no expected result: an oracle is written by hand from the design text, by a writer who has not read the build, and the build is written from the same text by someone who has not read the oracles. Where the two differ, the text arbitrates.

The stage holds two kinds of scenario.

- **The readings taken on 2026-11-14 that no run had taken.** The design text was amended on what the same-harness stage found, and several of its rules were then read one way where the build read them another way or not at all. Each has a family here, its oracle written first, and the reading not taken as a control.
- **Five more claims of the design text, each with one stand-in more**: a replacement that fails, a prepared form built once, a dedicated server started from the domain set alone, a decoder held to the host's ceiling, and work at load above one millisecond.

Rows judged on a magnitude or on a ceiling have inputs and no oracle: the cost of the gate in its incremental form (`gate-form`), the cost of deriving the groups of a budget (`groups-cost`), the decoder's ceiling (`decoder-ceiling`) and the bound of one millisecond (`load-bound`).

## Conventions of the stage

- **Sizes.** The design text as amended has an arrival name the size of what it puts in force and an entry hold it, in the canonical bytes. Where a family gives a size for each content, its oracle states it: `entry <asset> <type> available <hash> size <bytes>`, and the record element `arrival <asset> <hash> size <bytes>`, followed by `marked` where it answers a restore. Where a family gives no size, its oracle states none: the harness then writes the size the domain manifest gives, the length of the canonical part, and that size is judged by no oracle. The fourteen families of the two earlier sheets give no size but the eight worlds of `budget/<case>/derived-closure`, which were written again with it.
- **Oracles written again.** Two earlier oracles were written again by hand before this stage ran, and for no other reason than a reading taken since: the eight worlds of `budget/<case>/derived-closure`, for the size and, in one of them, for the order in which the groups of a budget are tested; and the worlds of `system-wants` in which the scripted system makes a want under the world's own entry, for the maker of that want.

## Part: the gate and the wants of the first step

### Conventions added

These hold for the families `pending-ends`, `incomplete-batch`, `first-step` and `pending-release`.

- **The pending record** is the record of the next tick the world has not run. Where a scenario says that an element joins the pending record, the element is added to that record at that point of the scenario, whatever tick the record is of and whether or not the world's gate is closed: the oracle states the tick, and under a control it may be another. A record the scenario gives by its tick number, "record of tick `n`", is the record of that tick whatever happened before.
- **Steps in a row.** A scenario may ask for several steps without waiting in a row. Each one either runs one tick or runs none and names what it awaits. The next statement of the scenario applies to the world as that step left it.
- **A change of policy** is written as the command the design text gives it: a want by a maker that already holds a want of the asset, stating the other policy. The text has a second want from one maker change the policy of the want it holds and add none, so the sheet has no other element for it, and a record states it as a `want` element. The earlier families `owners` and `retry` each hold one such command without naming it.
- **Sizes.** The four families give no size for a content. Their oracles state none, in an entry or in an arrival: the harness writes the size the domain manifest gives, the length of the canonical part, and that size is not judged.
- **How long a scenario runs.** Every scenario of the four families holds a closed gate. Each runs to tick 8, which is at least two ticks after the last tick its gate holds back.

### Additions to the format of an oracle

None. Where a scenario asks for several steps without waiting before one tick, the oracle holds one `blocked` line for each step that ran no tick, in the order of the steps, as the family `order` already does. A step without waiting that runs its tick has no line of its own: the oracle states the tick.

### Family `pending-ends`: the gate does not see the wants a pending element ends

The claim: the gate of a gated world does not see the wants that a pending element would end through the recomputation of the wants the core added.

Assets `D`, `G`, `P`. The pool is `G`, `P`. `P.1` lists `D` as its one dependency; `P.2` lists none; `D.1` and `G.1` list none. The table is empty at tick 0. The availability source is scripted, and it posts here what the loading capability never posts: a parent without the asset its part lists. Each scenario runs to tick 8.

**The opening** of the four scenarios on a parent in force. Record of tick 2: `want P parameters source:host optional`. Record of tick 3: the arrival of `P` at `P.1`, with no arrival of `D`. Record of tick 4: `want P parameters source:tool required`. A step without waiting. Then **the element under test** joins the pending record. A step without waiting. Then the arrival of `D` at `D.1` joins the pending record, and the world steps.

- **`pending-ends/release-of-the-last-required-want`**. The element under test: `release P source:tool`. Control, the reading not taken: the gate is evaluated on the table with the whole pending record applied, a command that adds a want excepted, so that it sees the wants a pending element ends.
- **`pending-ends/failure-of-the-parent`**. The element under test: an arrival of `P` failed for `unknown-asset`. Control: the same reading not taken.
- **`pending-ends/replacement-that-drops-the-dependency`**. The element under test: the arrival of `P` at `P.2`. Control: the same reading not taken.
- **`pending-ends/policy-change-of-the-parent`**. The element under test: `want P parameters source:tool optional`, a change of policy. Control: the gate applies a pending command that changes the policy of a want, with the recomputation of the wants of its asset's dependencies.

Two scenarios without the opening, on a change of policy where no want stands yet on a dependency:

- **`pending-ends/policy-change-on-the-required-asset`**. Record of tick 2: `want G parameters source:host required`. A step without waiting. Then `want G parameters source:host optional`, a change of policy, joins the pending record. A step without waiting. Then the arrival of `G` at `G.1` joins the pending record, and the world steps. Control: the gate applies a pending command that changes the policy of a want.
- **`pending-ends/policy-change-of-a-pending-parent`**. Record of tick 2: `want P parameters source:host required`. A step without waiting. Then `want P parameters source:host optional`, a change of policy, and the arrival of `P` at `P.1` join the pending record, with no arrival of `D`. A step without waiting. Then the arrival of `D` at `D.1` joins the pending record, and the world steps. Control: the gate applies a pending command that changes the policy of a want, in the policy of the want the core would add from a pending part as elsewhere.

**What the family leaves out.** The release of a parent's only want, after which the dependency's own arrival changes nothing: its tables are those of the reading not taken at every tick, the two differing by a record and by what a step answers alone. A pending despawn of an entity that owns the parent's required want, which the family `order` runs on an asset wanted in its own right. A parent at depth, whose pending element would end wants through two levels. A pending replacement that drops one dependency and lists a new one. Two pending elements on one parent, a release beside a replacement. A pending change of policy from optional to required. A dependency whose own answer is a failure. A live loading capability: the state every scenario starts from, a parent in force beside a dependency that is unanswered, is posted by a script. A restored world. The cost of the gate.

### Family `incomplete-batch`: the gate reads a pending record on a gated world alone

The claim: the gate reads a pending record on a gated world alone.

Assets `D`, `G`, `N`, `P`. The pool is `G`, `P`. `P.1` lists `D` as its one dependency; `P.2` lists `D` and `N`; `D.1`, `G.1` and `N.1` list none. The table is empty at tick 0. The availability source is scripted, and it posts here what the loading capability never posts: the replacement of a parent without the new dependency its part lists. Each scenario runs to tick 8.

Both scenarios open alike. Record of tick 2: `want P parameters source:host required`. Record of tick 3: the arrivals of `D` at `D.1` and of `P` at `P.1`, one batch.

- **`incomplete-batch/running-world`**. Once tick 4 has run, the arrival of `P` at `P.2` joins the pending record, with no arrival of `N`. A step without waiting, then a second step without waiting. Then the arrival of `N` at `N.1` joins the pending record, and the world steps. Control, the reading not taken: the gate is evaluated on the pending record at every boundary, on a running world as on a gated one. A second control: a required want that the core adds from the part of an applied arrival gates no boundary.
- **`incomplete-batch/gated-world`**. Record of tick 4: `want G parameters source:host required`. A step without waiting. Then the arrival of `P` at `P.2` joins the pending record, with no arrival of `N`. A step without waiting. Then the arrival of `G` at `G.1` joins the pending record. A step without waiting. Then the arrival of `N` at `N.1` joins the pending record, and the world steps. Control: the gate leaves out the want the core would add from the part of a pending arrival.

**What the family leaves out.** A live loading capability, and with it the batch built while a tick runs. A parent under an optional want alone, on which no want the core adds gates. The first load of a required parent, which gates its world before any batch and is the earlier family `pending-parent`. A batch that holds a parent and the asset under it without the asset one level further down. Two incomplete batches in a row. A new dependency whose answer is a failure. A world under a budget. A restored world, which is gated whatever its state holds.

### Family `first-step`: the wants of the first step start from an asset that holds a want

The claim: over the arrivals of one record, the wants the core adds before it applies them start from the part of an asset that holds a want, and the part of an asset no want names adds nothing.

Assets `K`, `M`, `N`, `P`, `R`. The pool is `P`, `R`. `P.1` lists no dependency and `P.2` lists `N` as its one dependency. `R.1` lists `M` as its one dependency and `M.1` lists `K`: a closure of three levels. `K.1` and `N.1` list none. The table is empty at tick 0. The availability source is scripted. Each scenario runs to tick 8.

Three scenarios on `P` open alike. Record of tick 2: `want P parameters source:host required`. Record of tick 3: the arrival of `P` at `P.1`.

- **`first-step/release-in-the-record-of-the-replacement`**. Record of tick 5: `release P source:host`, and the arrivals of `N` at `N.1` and of `P` at `P.2`, one batch. Control: the wants of the first step are added from the part of every arrival of the record, whatever wants its asset holds.
- **`first-step/replacement-of-an-entry-with-no-want`**. Record of tick 5: `release P source:host`. Record of tick 6: the arrivals of `N` at `N.1` and of `P` at `P.2`, one batch. Control: the same.
- **`first-step/want-again-in-the-record-of-the-replacement`**. Record of tick 5: `release P source:host`. Record of tick 6: `want P parameters source:tool optional`, and the arrivals of `N` at `N.1` and of `P` at `P.2`, one batch. Control: the wants of the first step start from an asset that held a want before the commands of the record were applied.

One scenario on `R`:

- **`first-step/root-fails-while-its-closure-arrives`**. Record of tick 2: `want R parameters source:host required`. A step without waiting. Then the arrivals of `K` at `K.1` and of `M` at `M.1` and an arrival of `R` failed for `invalid-part` join the pending record, one batch, and the world steps. Record of tick 5: `want R parameters source:tool required`. A step without waiting. Then the arrivals of `K` at `K.1`, of `M` at `M.1` and of `R` at `R.1` join the pending record, one batch, and the world steps. Control: the wants of the first step are added from the part of every arrival of the record, whatever wants its asset holds.

**What the family leaves out.** A live loading capability, and so the batch of a revision or of a closure that brings these records. An entry with no want that a part of the record lists as a new dependency of a parent that holds a want. A root that fails over a closure of one level, where a failed arrival brings no part and no reading of the first step gives another state; the family `types` holds such a record. An asset in the middle of a closure that fails while the root and the leaf arrive. A parent whose last want a despawn of the record ends. A world under a budget, where the parts of the rejected arrivals are left out of the first step. A restore.

### Family `pending-release`: a pending release answers a required asset only where it ends its last required want

The claim: on a gated world, a pending release answers the required asset it names only where no required want is left on that asset once the release is applied.

Assets `D`, `G`, `P`. The pool is `G`, `P`. `P.1` lists `D` as its one dependency; `D.1` and `G.1` list none. The table is empty at tick 0. The availability source is scripted, and in the last scenario it posts a parent without the asset its part lists. Each scenario runs to tick 8.

- **`pending-release/one-then-both`**. Record of tick 2: `want G parameters source:host required`, then `want G parameters source:tool required`. A step without waiting. Then `release G source:host` joins the pending record. A step without waiting. Then `release G source:tool` joins the pending record. A step without waiting. No arrival of `G` is scripted. Control, the reading not taken: a pending release answers the asset it names, whatever want is left on it.
- **`pending-release/last-required-want-beside-an-optional-one`**. Record of tick 2: `want G parameters source:host required`, then `want G parameters source:tool optional`. A step without waiting. Then `release G source:host` joins the pending record. A step without waiting. Then the arrival of `G` at `G.1` joins the pending record, and the world steps. Control: a pending release answers a required asset only where it ends the last want of that asset, whatever its policy.
- **`pending-release/optional-want-released`**. Record of tick 2: `want G parameters source:host required`, then `want G parameters source:tool optional`. A step without waiting. Then `release G source:tool` joins the pending record. A step without waiting. Then the arrival of `G` at `G.1` joins the pending record, and the world steps. Control, the reading not taken: a pending release answers the asset it names, whatever want is left on it.
- **`pending-release/own-want-beside-the-want-of-a-parent`**. Record of tick 2: `want P parameters source:host optional`. Record of tick 3: the arrival of `P` at `P.1`, with no arrival of `D`. Record of tick 4: `want P parameters source:tool required`, then `want D parameters source:host required`. A step without waiting. Then `release D source:host` joins the pending record. A step without waiting. Then the arrival of `D` at `D.1` joins the pending record, and the world steps. Control, the reading not taken: a pending release answers the asset it names, whatever want is left on it.

**What the family leaves out.** A required want of an entity beside a required want of the world's own entry, and the despawn of that entity, which the gate does not see. A release followed, in the same pending record, by a want of the same maker. A release of the dependency's own want beside a release of its parent's last required want. A required want of a contributor of the composition, which no command releases. A release made by a system, which is no element of a record. An asset that is failed and wanted again while the world is gated. A restored world.

## Part: the unit of a rejection over a world's budget

This part of the sheet states the inputs of three families judged on states, `group-order`, `unit-sentences` and `sizes`; the inputs of one magnitude, `groups-cost`; and what changes for the eight worlds of the family `budget` that are written again. It follows [`scenarios.md`](scenarios.md) and [`scenarios-same-harness.md`](scenarios-same-harness.md) for the conventions and for the format of an oracle, and repeats neither. It states no expected result.

The three families hold what no run has taken of the unit of a rejection over a world's budget. The design text now defines that unit, the group of a root, and names no candidate any more: every scenario of this part runs under the unit of the text, which the earlier sheet restated as the candidate `derived-closure`, with what the text has gained since. This part restates none of it. The order in which the groups are tested, what the count leaves out, the group that does not raise the count and the size are for the writer and for the builder to read in the text. Every asset of this part is of the first synthetic type, `parameters`.

### Conventions added

- **Sizes.** Every family of this part gives a size in bytes for each content: the size the domain manifest gives for it. The stand-in pipeline pads a part to the size stated, but where a family says otherwise. The oracles of this part state the size each arrival to a hash names and the size each available entry holds.
- **Budgets.** A world's budget for its asset data is stated in bytes with its scenario: it is the budget the world is created under.
- **A budget lowered.** Between two steps the host lowers a world's budget by a world event, an element of the input record of the next tick, issued by `source:host` and written `budget <bytes>`. "Record of tick `n`: `budget 5000`" means that the host lowers the budget to 5 000 bytes between tick `n - 1` and tick `n`. No record of this part holds both a world event and a command, so the place of a world event among the commands of its record is not judged. Which budget the arrivals of the record that holds the event are tested against, and what a lowering does to a table whose count stands over the new budget, are for the writer and for the builder to read in the text.
- **A revision that takes an asset out.** "A revision that does not hold `X`" is a revision whose domain manifest holds no entry for `X`: the asset leaves the instance's manifest, as by a deletion.
- **A world that replays.** It is created as the world that wrote the input log was, with the same composition and the same budget at creation, in a new process, with no loading capability and no scripted source. Its records are the log's, read back from its bytes, and its store holds every part the log names. It runs at another worker count than the world that wrote the log.
- **A reader of a save** is as in the family `restore` of the first sheet: a new process, a store of its own, a request that says whether the content must be the recorded one. A reader's scenario gives it no budget: which budget a restored world stands under is for the writer and for the builder to read in the text.

### Additions to the format of an oracle

Beyond the formats of the two earlier sheets, the `rejected` line and the statement `world <name> as <other> shifted <k>` of the second one included:

- `entry <asset> <type> available <hash> size <bytes>`: an available entry states the size it holds. An entry that is unavailable or failed states none.
- The record element `arrival <asset> <hash> size <bytes>`, followed by `marked` where it answers a restore: an arrival to a hash states the size it names. An arrival to failed states none. A `rejected` line names the asset and the hash, as before, and no size.
- The record element `budget <bytes>`: the world event that sets the world's budget for its asset data. A tick whose record holds one needs a block, as any tick whose record is not empty does.
- The reason `over-budget`, in an availability, in an event and in a `names-failed` line, as in the earlier sheet. It is the reason of the budget's own that the design text now gives the failure of a rejected first load. The text gives that reason no name, and this sheet keeps the name of the earlier one.
- `world <name> as <other> shifted 0` states a world that replays the log of the world `<other>` of the same scenario.

### Family `group-order`: the order in which the groups are tested

The claim: the groups of the roots that hold a required want are tested before the others.

Assets `A`, `B`, `C`, `D`, `E`, `F`, `G`, `H`, `M`, `P`, `Q`, `T`. The pool is `A`, `E`, `M`, `Q`. A live loading capability over a loose store written by the stand-in pipeline. One world per scenario, which names no set, with an empty table at tick 0 and a budget of 10 000 bytes but where a scenario says otherwise.

| Content | Size in bytes | Dependencies its part lists |
| --- | --- | --- |
| `A.1` | 1 000 | `C` |
| `B.1` | 1 000 | none |
| `B.2` | 4 000 | none |
| `C.1` | 6 000 | none |
| `D.1` | 3 000 | none |
| `E.1` | 4 000 | none |
| `F.1` | 1 000 | none |
| `G.1` | 5 000 | none |
| `H.1` | 3 000 | none |
| `M.1` | 2 000 | none |
| `M.2` | 5 000 | none |
| `P.1` | 1 000 | `M` |
| `P.2` | 4 000 | `M` |
| `Q.1` | 1 000 | `T` |
| `T.1` | 5 000 | none |

The instance's manifest holds the first content of every asset. **The later revision** holds `B.2`, `M.2` and `P.2` in place of `B.1`, `M.1` and `P.1`, and the rest unchanged.

Two closures that each fit alone, the optional root standing first in identifier order:

- **`group-order/optional-root-first`**. Record of tick 3: `want A parameters source:host optional`, then `want Q parameters source:host required`. Run to tick 8. Control, the reading not taken: the groups are tested in the identifier order of their roots alone.
- **`group-order/policy-changed-in-the-record-of-the-arrivals`**. Record of tick 3: `want A parameters source:host required`, then `want Q parameters source:host optional`. The record of tick 4 holds `want A parameters source:host optional`, then `want Q parameters source:host required`, with what the capability posted. Run to tick 8. Control, the reading not taken: the groups are tested in the identifier order of their roots alone. A second control: a root holds a required want where it stood in the required set before the commands of the record were applied.

A root that is optional in its own right and required as the dependency of another root:

- **`group-order/required-as-a-dependency`**. A budget of 8 000 bytes. Record of tick 2: `want P parameters source:host required`. Record of tick 4: `want B parameters source:host optional`, then `want M parameters source:host optional`. Between tick 7 and tick 8 the later revision is put in place. Run to tick 12. Control, the reading not taken: the groups are tested in the identifier order of their roots alone. A second control: a root holds a required want where a maker other than the core made one.
- **`group-order/parent-made-optional-in-the-record`**. As `group-order/required-as-a-dependency`, but that the record of tick 8 holds `want P parameters source:host optional`, with what the capability posted. Run to tick 12. Control: a want the core added keeps its policy until the arrivals of the record are applied, whatever a command of that record does to the wants of its parent.

Five roots with no dependency, their wants made in the reverse of their identifier order:

- **`group-order/each-set-in-identifier-order`**. Record of tick 3, in this order: `want H parameters source:host required`, `want G parameters source:host required`, `want F parameters source:host optional`, `want E parameters source:host required`, `want D parameters source:host optional`. Run to tick 8. Control, the reading not taken: the groups are tested in the identifier order of their roots alone. A second control: the groups of each of the two sets are tested in the order in which the wants of their roots were made.

**What the family leaves out.** A root that a part of the same record would make required, its parent arriving with it: the want the core adds from that part is not in the table when the groups are tested, the arrival of such a root stands in the group of its parent's root as well, and no state of this family turns on it. A step without waiting on a gated world whose pending batch holds a group that the budget rejects: whether the gate applies the budget to a pending batch is open in the design text. Two roots under the two policies that share a dependency, which the case `overlapping-closures` of the earlier family holds. A required want of an entity or of a contributor in the record under test. A root required through several levels of dependencies. A prepared form built once.

### Family `unit-sentences`: an entry that fails at the same fixed point, and a group that does not raise the count

The claim: the two sentences of the unit that no run has taken give the stated states. The first is on the count, which leaves out an entry in force that an arrival to failed of the same record fails. The second is on the test: a group that does not raise the count is accepted whatever the count.

Assets `G`, `J`, `N`, `T`, `X`, `Y`. The pool is `G`, `J`, `N`, `X`. A live loading capability over a loose store written by the stand-in pipeline. One world per scenario, which names no set, with a budget of 10 000 bytes at creation. The composition's contributor `base` wants `G` as required at tick 0, alone but where a scenario says otherwise.

| Content | Size in bytes | Dependencies its part lists |
| --- | --- | --- |
| `G.1` | 3 000 | none |
| `G.2` | 8 000 | none |
| `G.3` | 4 000 | none |
| `J.1` | 5 000 | none |
| `J.2` | 3 000 | none |
| `N.1` | 6 000 | none |
| `T.1` | 2 000 | `Y` |
| `T.2` | 3 000 | `Y` |
| `X.1` | 6 000 | none |
| `Y.1` | 4 000 | none |
| `Y.2` | 3 000 | none |

The instance's manifest holds the first content of every asset when a scenario starts. Each scenario names the revisions put in place; a revision holds what the manifest held before it but where the scenario says otherwise.

An arrival to failed for an asset in force, with an arrival to a hash:

- **`unit-sentences/room-of-an-entry-that-fails`**. Record of tick 2: `want X parameters source:host required`. Between tick 5 and tick 6 a revision is put in place that holds `G.2` in place of `G.1` and does not hold `X`. Run to tick 8. Control: the count holds an entry in force that an arrival to failed of the same record fails.
- **`unit-sentences/failed-arrival-for-an-entry-no-want-names`**. As `unit-sentences/room-of-an-entry-that-fails`, but that the record of tick 6 holds `release X source:host`, with what the capability posted. Run to tick 8. Control: the room of an entry in force is free wherever the record holds an arrival to failed for it, whether or not that arrival fails it.
- **`unit-sentences/room-of-a-failed-parent`**. Record of tick 2: `want T parameters source:host optional`. Record of tick 5: `want N parameters source:host optional`. Between tick 5 and tick 6 a revision that does not hold `T` is put in place. Record of tick 7: `want N parameters source:tool optional`. Run to tick 10. Control: the room of the dependencies of a parent that fails is free at the fixed point that fails it.

A world whose count stands over a lowered budget:

- **`unit-sentences/smaller-replacement-over-a-lowered-budget`**. The contributor `base` wants `G` and `J` as required at tick 0. Record of tick 3: `budget 5000`. Between tick 5 and tick 6 a revision that holds `J.2` in place of `J.1` is put in place. Between tick 8 and tick 9 a revision that holds `G.3` in place of `G.1` is put in place. Run to tick 11. Control: every group is tested against the budget, one that does not raise the count included. A second control: the budget only reports, and every arrival is applied.
- **`unit-sentences/group-that-leaves-the-count-as-it-was`**. The contributor `base` wants `G` and `T` as required at tick 0. Record of tick 3: `budget 5000`. Between tick 5 and tick 6 a revision that holds `T.2` and `Y.2` in place of `T.1` and `Y.1` is put in place. Run to tick 8. Control: a group is accepted whatever the count only where it lowers the count. A second control: each arrival of a group is judged alone, one that raises the count being rejected where the count stands over the budget.

The record in which a budget is lowered:

- **`unit-sentences/budget-lowered-in-the-record-of-an-arrival`**. Record of tick 3: `want N parameters source:host optional`. The record of tick 4 holds `budget 8000`, with what the capability posted. Run to tick 6. Control: a budget that a record changes is in force from the tick after it.

**What the family leaves out.** A count that stands over its budget by a restore. A lowering that takes something out of force: what a budget lowered under the count does to what is in force is open in the design text. A record that holds both a lowering and a command. A budget raised, and a rejected replacement offered again once there is room, which the design text leaves open. An asset in force that fails by an unmount, which the earlier family `mount` holds without a budget. A parent that fails while another root holds its dependency. A step without waiting. A prepared form built once.

### Family `sizes`: the size an arrival names and an entry holds

The claim: the size an arrival names and an entry holds gives the count. The family holds a size that the length of the part does not give, a run replayed from its input log, and a table saved and restored. The eight worlds of the earlier family that state the size are in the last section of this part.

Assets `H`, `K`, `N`, `S`, `W`. The pool is `H`, `K`, `N`, `W`. No canonical part of this family lists a dependency. A live loading capability over a loose store written by the stand-in pipeline, but for the world that replays.

| Content | Size in bytes | Length of its part in bytes |
| --- | --- | --- |
| `H.1` | 4 000 | 1 000 |
| `K.1` | 5 000 | 5 000 |
| `K.2` | 7 000 | 7 000 |
| `K.3` | 3 000 | 3 000 |
| `N.1` | 2 000 | 2 000 |
| `S.1` | 1 000 | 1 000 |
| `W.1` | 1 000 | 1 000 |

**A size that is not the length of the part.** The size of a content is the one the domain manifest holds for it. For `H.1` the stand-in pipeline writes a size of 4 000 bytes into the domain manifest and pads the part to 1 000 bytes. The difference stands in for what a prepared form built once adds to the size of its part; this family builds no such form. The loading capability names the size the manifest holds and compares it with nothing.

**The first revision** holds `H.1`, `K.1`, `N.1`, `S.1` and `W.1`. **The later revision** holds `K.2` in place of `K.1`. `K.3` is in neither: one reader's content at hand holds it.

**`sizes/run-and-replay`.** Two worlds, each run to tick 15.

- `recorded`, on an instance whose manifest is the first revision's, naming no set, with a budget of 12 000 bytes at creation. The composition's contributor `base` wants `K` as required at tick 0.
  - Record of tick 2: `want H parameters source:host optional`, then `want S parameters source:host required`.
  - Record of tick 4: `budget 10000`.
  - Record of tick 5: `want N parameters source:host optional`.
  - Between tick 7 and tick 8 the later revision is put in place.
  - The world is saved after tick 9: **the save** of this family.
  - Record of tick 11: `release S source:host`, then `want W parameters source:tool optional`.
  - Record of tick 13: `want N parameters source:tool optional`.
- `replayed`: a world that replays the input log of `recorded`, from its creation to tick 15.

The oracle states `recorded` tick by tick and `replayed` by it. The harness also compares the state hashes of the two worlds, tick by tick. Control, on `recorded`: the count is the sum of the lengths of the parts in force, the size an arrival names left aside. Control, on `replayed`: the replay applies every arrival to a hash that its log holds and derives no rejection. A second control, on `replayed`: the lowering of a budget is no element of the log, and the replay stands under the budget of its creation.

Readers of the save, each into the saved world's composition, naming no content set, stepped to tick 15. Each reader's record of tick 11 and record of tick 13 hold the commands that `recorded` applies at those ticks. The oracle states each reader's verdict and its tables from its first tick on.

- **`sizes/restored-under-the-recorded-content`**. A restore under the recorded content. The content at hand is the first revision's. The harness also compares its state hashes with those of `recorded`, from tick 10 on. Control: the count of a restored table starts from nothing and holds the sizes of the arrivals applied since the restore alone. A second control: a restored world stands under the budget its saved world was created under.
- **`sizes/restored-at-a-smaller-hash`**. A restore under the content at hand, which holds `K.3` in place of `K.1` and is the first revision's otherwise. The reader's store holds no artifact of `K.1`. Control: a marked answer at another hash changes the hash of its entry and leaves the size the entry held.

Control of the three scenarios of the family: the size left out of the canonical bytes of an entry and of an arrival.

**What the family leaves out.** A prepared form built once, counted with its part, which another part of this sheet holds. A reader whose content at hand holds a larger hash than the saved one, which the budget would reject: the design text leaves open what such a rejection does at a restore. A reader under the recorded content whose manifest holds the replacement that the saved world rejected. A save that holds an entry with no want or an unanswered want. A restore into another composition, and a world under a named set. A manifest whose size is smaller than its part, and a loading capability that checks a size against the bytes it read.

### Family `groups-cost`: the groups derived on a large table

The claim: the groups of a rejection over a budget are derived at a cost that follows the record, and not the table. It is a magnitude: it has no oracle and no control, and it is judged on one quiet machine, in the release profile, over at least five repetitions.

**The world.** The reference workload at default size, 10 000 entities, at 1, 4 and 8 workers, with a scripted availability source and a budget of 100 000 000 bytes, which no record of the family reaches: every group is accepted, and the family times how the groups are derived and decided, whatever the decision.

**The two tables.**

- **`large`**: the table of the family `gate-cost`. 2 000 roots `R0000` to `R1999` and 8 000 leaves `L0000` to `L7999`; the first content of the root of rank `i` lists the four leaves of ranks `4i` to `4i + 3`, and a leaf lists nothing; the identifiers are drawn from the fixed stream of the spikes, in no relation to the names; the pool is the sixteen leaves `L0000` to `L0015`. Every first content has a size of 1 000 bytes. The record of tick 1 holds, for each root, the command `want <root> parameters source:host required`; the record of tick 2 holds the arrivals of the 10 000 assets, one batch, each naming 1 000 bytes. From tick 2 on the table holds 10 000 available entries.
- **`small`**: the roots `R0000` to `R0199` and the leaves `L0000` to `L0799` of the large table, with the identifiers and the contents they have there, filled the same way. From tick 2 on the table holds 1 000 available entries.

**The two records**, each the record of tick 103, assembled after tick 102. Each is the same record, byte for byte, on the two tables.

- **`one`**: the arrival of the leaf `L0000` at a second content of 1 100 bytes, which lists nothing.
- **`thousand`**: 1 000 arrivals, the roots `R0000` to `R0199` and the leaves `L0000` to `L0799`, each at a second content of 1 100 bytes. The second content of the root of rank `i` lists its four leaves and one leaf more, the first leaf of the root of rank `i + 1`, and for the root of rank 199 the leaf `L0000`. The second content of a leaf lists nothing.

**What is timed.** The derivation and the decision of the groups for the record: from the moment the commands of the record are applied, the record holding none, to the moment every group is decided, before any want is added from a part and before any arrival is applied. It is made 100 times on the same record and the same table, which none of them changes, and the median is kept. Adding the wants, applying the arrivals, recomputing the wants and the tick are not in it.

**Against what.** The median duration of one tick of the same world at the same worker count, over the ticks 3 to 102, each with an empty record.

**What is reported**, per table, per record and per worker count, for each repetition: the median derivation, the median tick and their ratio; and the count of roots whose closure one derivation read. Beside them, once per repetition, the duration of the input fixed point of tick 103, which applies the record, under the budget and in a twin world created with no budget. And, once, how the build derives the groups: from every root of the table downwards, or from the arrivals upwards.

The figure the claim is read on is, for each record and each worker count, the median derivation on the large table over the median derivation on the small one. The record is the same on both tables: a cost that follows the record leaves that figure near one, and a cost that follows the table brings it near ten, which is the ratio of the two tables. The claim names no bound, so this sheet names none: the report gives the six figures of each repetition.

**What the family leaves out.** A record whose groups are rejected. A closure of more than two levels, and a leaf that many roots reach. A table whose roots hold optional wants. The cost that the unit adds to the application of the arrivals, which the two durations of the fixed point show without separating it.

### The eight worlds of the unit in the family `budget`, written again

The earlier sheet ran eight scenarios `budget/<case>/derived-closure` under its restatement of the unit that the design text then proposed. Their inputs do not change: the assets, the table of sizes, the cases, the budgets and the records are those of that sheet. Two things change, and the eight oracles are written again for both.

- **The size.** That sheet said the size was a stand-in, read by the build from the length of the part and written in no oracle. Each arrival to a hash now names the size the domain manifest gives, each available entry holds it, and the eight oracles state both, in the format this part adds. The sizes are those of that sheet's table.
- **The order of the test.** That sheet's restatement tested the groups in the identifier order of their roots. The design text now states the order, and the eight worlds run under it.

The definitions and the oracles of the three other candidates stay as they are. Their oracles state no size, and the size is not judged there.

The controls that the earlier sheet names for the eight scenarios stand. One is added, for `budget/two-closures-that-fit-alone/derived-closure`: the reading not taken, the groups tested in the identifier order of their roots alone.

## Part: the maker of a system's want, the type a want of the core takes, the batch of a restore

This part holds three families: a system as the maker of the want it makes (`system-maker`), the type a want of the core takes on an entry failed for its type (`core-want-type`), and the batch that the answers to a restore complete (`restore-batch`). It follows the two earlier sheets for the conventions and for the format of an oracle, and repeats neither. It states no expected result.

None of the three families gives a size to its contents: their oracles state none, and the size the harness writes is not judged.

### Conventions added

- **Three scripted systems.** In the family `system-maker` the harness adds three systems to the schedule of the stand-in world: `wanter` and `keeper`, both contributed by `contributor:logic`, whose identity is 9, and `mender`, contributed by `contributor:base`, whose identity is 7. `wanter` is the scripted system of the earlier sheet, and what that sheet says of it stands for the three: each runs once in every tick, after the tick's input fixed point and before its structural change fixed point, and each declares access to the wants of the asset table. Their order in the schedule is `wanter`, `mender`, `keeper`. The composition of every scenario of the family holds the two contributors, so the three systems stand in every schedule; a system that a scenario does not name does nothing. "System `<name>` at tick `n`" lists what that system does when it runs in tick `n`, in the order given. A want it makes under the world's own entry is written `want <asset> <type> world <policy>` in this sheet, and its release `release <asset> world`.
- **The stable name of a system** is the name written here: `keeper`, `mender`, `wanter`. The three names have the same length and differ at their first letter, so that the order of two of them is the same whether their characters or their canonical bytes are compared: `keeper`, then `mender`, then `wanter`. The order of the names is not the order of the schedule, and it is not the order of the identities of the contributors that bring them.
- **What the design text says of a system's want.** A system is the maker of the want it makes under the world's own entry: a third kind of maker beside the contributors of the composition and the sources of commands, whose identity is the system's stable name. In the total order of the wants of an entry, the wants of the world's own entry stand first: those of contributors, then those of systems, then those of command sources, each kind by its identity. The wants of entities and the wants the core added follow, as the first sheet has them.
- **A scripted world saved, live readers.** In the family `restore-batch` the saved world runs under a scripted availability source, so that its table at the save is a function of its script, an entry that no arrival ever answered included. Each reader is a process of its own with a live loading capability over a loose store of its own.
- **A step without waiting before the capability has posted**, as the first sheet uses it: the host asks for one tick, without waiting at a closed gate, before it lets the live loading capability post what the tick before made it load.

### Additions to the format of an oracle

- `<owner>` may be `system:<name>`: a want a system makes under the world's own entry, `<name>` being the stable name of the system. It stands in an entry's wants at the place the design text gives the wants of systems. In the family `system-wants` of the earlier sheet, the want the scripted system makes under the world's own entry is now written `system:wanter`: this replaces the sentence of that sheet under which such a want was written with one of the owners of the first sheet.

What a system does has no statement of its own, as in the earlier sheet: the oracle states it by the table of its tick, and a comment says what the system did.

### Family `system-maker`: a system is the maker of the want it makes

The claim: a system is the maker of the want it makes under the world's own entry. Two systems of one contributor hold two wants, and the release of one leaves the other; a system's want of an asset the composition requires for its module is its own and changes nothing of the composition's; the wants of an entry stand in the order of the three kinds of maker.

Assets `A`, `D`, `F`, `P`. The pool is `A`, `P`. The canonical part `P.1` lists `D` as its one dependency; no other part lists one. The table is empty at tick 0 but where a scenario gives the composition a want. The availability source is scripted but where a scenario says otherwise.

Controls, each the rule changed:

- Every scenario of the family, the reading not taken: the maker of a want a system makes under the world's own entry is the contributor that brings the system.
- `system-maker/order-of-the-makers`, a second control: the wants of systems stand after those of command sources. A third control: the wants of systems stand in the order of the identities of the contributors that bring them, then by name.

The scenarios:

- **`system-maker/two-systems-one-asset`**. Run to tick 10. System `wanter` at tick 2: `want A parameters world required`. Record of tick 3: the arrival of `A` at `A.1`. System `keeper` at tick 4: `want A parameters world optional`. System `wanter` at tick 6: `release A world`. System `keeper` at tick 8: `release A world`.
- **`system-maker/system-beside-the-composition`**. Run to tick 9. The composition's contributor `logic` wants `P` as required: the want is in the table at tick 0. Record of tick 1: the arrivals of `D` at `D.1` and of `P` at `P.1`, one batch. System `wanter` at tick 3: `want P parameters world optional`. System `wanter` at tick 6: `release P world`.
- **`system-maker/order-of-the-makers`**. Run to tick 6. The composition's contributor `base` wants `D` as optional, and its contributor `logic` wants `P` as required and `D` as optional: the three wants are in the table at tick 0. Record of tick 1: the arrivals of `D` at `D.1` and of `P` at `P.1`, one batch. Record of tick 2, in this order: `want D parameters entity:E2 optional`, `want D parameters entity:E1 optional`, `want D parameters source:twin optional`, `want D parameters source:tool optional`, `want D parameters source:host optional`. At tick 2 the three systems each make a want: system `wanter`, `want D parameters world optional`; system `mender`, `want D parameters world required`; system `keeper`, `want D parameters world optional`. System `mender` at tick 4: `release D world`. Record of tick 5: `release D source:tool`.
- **`system-maker/retry-by-a-system`**. Run to tick 8. A live loading capability over a loose store written by the stand-in pipeline, whose content holds `F.1` and never moves. When the scenario starts the artifact of `F.1` is cut short; between tick 1 and tick 2 the whole artifact is put in the store, under the same hash. The composition's contributor `logic` wants `F` as required: the want is in the table at tick 0. System `wanter` at tick 3: `want F parameters world optional`. Then a step without waiting, before the capability has posted. System `wanter` at tick 6: `release F world`. The oracle states what the records hold.

**What the family leaves out.** A want of a system that would be refused, a want of another type than its entry holds for example: a refusal of a system's want has no record to stand in. A release by a system that holds no want of the asset it names. Two systems that make the first want of one asset under two types in one tick, where the order of the schedule would decide the type of the entry. Two stable names of unequal lengths, on which a comparison of characters and a comparison of canonical bytes may differ. A save that holds the want of a system: read into its own composition, and read into a composition that no longer holds the system. A want a system makes for an entity, whose owner is the entity and which carries no maker: the earlier family `system-wants` holds those.

### Family `core-want-type`: the type a want of the core takes on an entry failed for its type

The claim: a want the core adds on an entry failed for the reason of another type takes the type its parent's part lists, replaces the entry's type and is the new load, whatever other want stands on the entry; and the loading capability, following the closure that brings that want, does not fail the dependency for its type and loads it under the manifest's type. The family also states the case the rule does not reach: an entry that fails for its type while a want of the core already stands on it.

Assets `D`, `P`. The pool is `P`. Both are of the first asset type: the manifest holds each under `parameters`, and `P.1` lists `D` as its one dependency under `parameters`. A live loading capability over a loose store written by the stand-in pipeline; the content holds `D.1` and `P.1` and never moves. The composition declares the first and the second type and makes no want: the table is empty at tick 0. Each scenario runs to tick 12, and its oracle states what the records hold.

Controls, each the rule changed:

- `core-want-type/first-want-standing`, `core-want-type/first-want-released`, `core-want-type/parent-wanted-again-at-its-fall`, the reading not taken: a want the core adds states a type only where it is the dependency's first, and the loading capability fails every asset the manifest holds under another type than its entry.
- `core-want-type/first-want-standing`, `core-want-type/first-want-released`, a second control: the core's want takes the type, and the loading capability, following the closure, still fails for its type a dependency whose entry the table holds under another type than the manifest's.
- `core-want-type/parent-wanted-again-at-its-fall`, a second control: the wants of dependencies added and recomputed with a batch alone.
- `core-want-type/failure-under-a-standing-core-want`: an entry that fails for its type while a want of the core stands on it takes, at that failure, the type its parent's part lists, and its load starts again. A second control: a second want on a parent in force is a want added on each of its dependencies.

The scenarios:

- **`core-want-type/first-want-standing`**. Record of tick 2: `want D second source:host optional`. Record of tick 5: `want P parameters source:tool required`, then a step without waiting, before the capability has posted.
- **`core-want-type/first-want-released`**. Record of tick 2: `want D second source:host optional`. Record of tick 5: `release D source:host`, then `want P parameters source:tool required`, then a step without waiting, before the capability has posted.
- **`core-want-type/failure-under-a-standing-core-want`**. Record of tick 2: `want D second source:host optional`, then `want P parameters source:tool required`, then a step without waiting, before the capability has posted. Record of tick 5: `want P parameters entity:E1 optional`.
- **`core-want-type/parent-wanted-again-at-its-fall`**. Record of tick 2: `want D second source:host optional`, then `want P parameters source:tool required`. Record of tick 6: `release P source:tool`. Record of tick 7: `want P parameters source:tool required`, then a step without waiting, before the capability has posted.

**What the family leaves out.** The repair of such an entry by a command that states the manifest's type, which the family `types` of the first sheet holds. An entry failed for its type under the wants of two parents. A wrong first want that stands in the record of the batch that brings the core's want, the arrival of that batch being to a hash. A revision that moves the type the manifest holds an asset under. A scripted source that posts a parent without its dependency. A save that holds an entry failed for its type, restored under content whose part newly lists it. A want of the core added on an entry failed for another reason than its type, which the family `retry` of the first sheet holds.

### Family `restore-batch`: the answers to a restore complete the batch of each closure

The claim: at a restore under the content at hand, the loading capability posts with the marked answers, as an ordinary arrival, each asset that a want the core adds from a part at hand creates or returns to unavailable, and an entry the save holds unavailable under no want that such a part lists; an entry the save holds as failed under a want that stands, on which the parts at hand add no want, receives nothing. The two refutations are a restored world that never runs its first tick and a continued run whose failed entry is answered again.

Assets `D`, `F`, `G`, `N`, `P`, `Q`, `U`, `X`, `Z`. The pool is `P`, `Q`. Every asset is of the first type.

**The saved world.** Its content holds `D.1`, `F.1`, `G.1`, `N.1`, `P.1`, `Q.1`, `U.1`, `Z.1`: `P.1` lists `D` as its one dependency, `Q.1` lists `G` as its one dependency, and no other part lists one. It does not hold `X`. The world names no content set. Its composition's contributor `base` wants `P` and `Q` as required: the two wants are in the table at tick 0. Its availability source is scripted.

- Record of tick 1: the arrivals of `D` at `D.1`, of `P` at `P.1` and of `Q` at `Q.1`, and an arrival of `G` failed for `invalid-part`, one batch.
- Record of tick 2: `want F parameters source:host optional`, `want N parameters source:host optional`, `want Z parameters source:host optional`.
- Record of tick 3: an arrival of `F` failed for `invalid-part`, an arrival of `N` failed for `invalid-part`, and the arrival of `Z` at `Z.1`.
- Record of tick 7: `want U parameters source:host optional`. No arrival of `U` is scripted.
- Record of tick 8: `release N source:host`, `release U source:host`, `release Z source:host`.
- The world is saved after tick 8: **the save**. The process that saved it goes on to tick 12, with no further command and no further arrival: this is the uninterrupted run, the scenario **`restore-batch/saved-world`**, whose oracle states the table from tick 0 to tick 12. Control: an entry leaves the table at the fixed point that ends its last want.

**The readers.** Each loads the save in a new process, as a restore under the content at hand, into the saved world's composition, naming no content set, with a live loading capability over a loose store of its own written by the stand-in pipeline, and steps to tick 12. The content at hand is the saved world's but where a reader says otherwise, and every artifact of it is whole but where a reader says otherwise: an asset the save holds as failed would load. A part named below as holding another list of dependencies differs from `P.1`, or from `Q.1`, in that list and in its hash. A reader's record is empty but for what the capability posts. The oracle states each reader's verdict before its first tick, what a step without waiting names before the answers are posted, and its tables from its first tick on, which is tick 9.

- **`restore-batch/recorded-hashes`**. The content at hand is the saved world's. The harness also compares its state hashes with those of the uninterrupted run, from tick 9 on. Control, the reading not taken: the batch of a restore holds an ordinary arrival for every asset of the closure of an answer that the table does not hold available.
- **`restore-batch/failed-entry-newly-listed`**. The content at hand holds `P.2` in place of `P.1`; `P.2` lists `D` and `F`. Control: answers posted for the entries the saved table holds available, and for no other asset. A second control, the reading not taken: as above.
- **`restore-batch/failed-entry-newly-listed-fails-again`**. The same content at hand, the artifact of `F.1` being cut short in the reader's store. Control: no failed arrival posted, at a restore, for an entry the save holds as failed. A second control: an arrival that leaves an entry failed for the reason it held emits no event and is named by no step, whatever want the same record added on the entry.
- **`restore-batch/failed-entry-of-another-parent-newly-listed`**. The content at hand holds `P.3` in place of `P.1`; `P.3` lists `D` and `G`. Control: an entry the save holds as failed under a want the core added receives nothing, whichever parent owns that want.
- **`restore-batch/failed-entry-with-no-want-newly-listed`**. The content at hand holds `P.4` in place of `P.1`; `P.4` lists `D` and `N`. Control: the batch completed for an entry the save holds as failed under a want, and not for one it holds as failed under none. A second control, the reading not taken: as above.
- **`restore-batch/unavailable-entry-newly-listed`**. The content at hand holds `P.5` in place of `P.1`; `P.5` lists `D` and `U`. Control: the batch holds an ordinary arrival for an asset that a want of the core creates or returns to unavailable, and for no entry the save already holds unavailable. A second control, the reading not taken: as above.
- **`restore-batch/available-entry-with-no-want-newly-listed`**. The content at hand holds `P.6` in place of `P.1`, and `Z.2` in place of `Z.1`; `P.6` lists `D` and `Z`. Control: the answer of an entry that held no want when the record was drained changes nothing, a want the core adds from a part of the same record not counted. A second control, the reading not taken: as above.
- **`restore-batch/parent-at-another-hash-keeps-its-failed-dependency`**. The content at hand holds `Q.2` in place of `Q.1`; `Q.2` lists `G`, as `Q.1` does, and differs from it in its parameters. Control, the reading not taken: as above.
- **`restore-batch/failed-entry-newly-listed-at-depth-two`**. The content at hand holds `P.7` in place of `P.1`, and `X.1`, an asset the saved world's content does not hold; `P.7` lists `D` and `X`, and `X.1` lists `F` as its one dependency. Control: the batch completed one level under an answer alone, the parts of the ordinary arrivals it holds not being followed. A second control, the reading not taken: as above.

**What the family leaves out.** A part at hand that newly lists, under an answer, an asset the saved table does not name: the family `restore` of the first sheet holds it. An entry the save holds unanswered under a want, or available under a want, that a part at hand newly lists. A save that holds an unanswered want, of which every reader starts a new history whatever its batch holds. A reader whose first record holds a command. A restore under the recorded content, which answers nothing. A restore into another composition. A budget, under which an ordinary arrival of the batch may be rejected. A revision that lands while a reader waits for its answers.

## Part: a batch against the state of the last tick, and a replacement that fails

This part of the sheet states the inputs of two families. Both run with a live loading capability over a loose store written by the stand-in pipeline. The part follows [`scenarios.md`](scenarios.md) and [`scenarios-same-harness.md`](scenarios-same-harness.md) for the conventions and for the format of an oracle, and repeats neither. It states no expected result: an oracle is written by hand from the design text, by a writer who has not read the build, and the build is written from the same text by someone who has not read the oracles. Where the two differ, the text arbitrates.

### Conventions added

- **Sizes.** In the family `batch-state` every content has a size in bytes: the size of its canonical part, as the domain manifest gives it. The stand-in pipeline pads a part to the size stated. No content of the family has a prepared form, so the size an arrival names is the size of its part. The oracle of that family states the size of every arrival to a hash and of every entry in force. The family `failed-replacement` gives no size: its oracle states none, the harness writes the size the domain manifest gives, and that size is not judged.
- **A budget.** A world's budget for its asset data is stated in bytes with its scenario and does not change during it. A scenario that states no budget has none. The unit of a rejection is the one the design text has taken, the group of a root, as the candidate `derived-closure` of the same-harness sheet restates it, with the order the text has taken since: the groups of the roots that hold a required want are tested before the others, each of the two sets in the identifier order of its roots.
- **The content root and a save.** The stand-in pipeline imports a content root of small text sources, one per asset. A source states its asset's type, its parameters, its dependencies and, where the family gives sizes, a size. **A save** writes, deletes or writes again the sources it names. The pipeline then runs one pass over the root, in the manner of a development host, and what that pass gives is put in place on the instance. A source **saved again as it was** has the bytes it had before. Where a scenario says that a revision is put in place and names what it holds, the save is the one that gives that revision. "Between tick `a` and tick `b`" is, for one world, after tick `a` has run and before the host waits for the capability ahead of tick `b`.
- **A source saved with an error.** For the comparison alone: its text holds the word `error` where the importer reads the first parameter. The importer reads no number there, and the canonical action of that asset fails, with a diagnostic that names the source. Every other action of the pass runs. What the pass writes, and what a world receives of it, are for the writer and for the builder to read in the text.
- **The admission check.** The design text lets the schema entry of an asset type carry an admission check, which the capability that declares the type supplies. No type of the prototype has one, so this sheet defines one for the comparison alone. A composition that **carries the check** gives it to the entry of the type `parameters`, with one number, **the bound**. The check reads one canonical part of that type and nothing else: neither the table, the manifest, the store nor another part. It refuses the part where the first parameter the type's layout declares is greater than the bound, and accepts it otherwise. The bound stands inside the values the type's validator accepts, so a content the check refuses is a valid part. Where and when the check runs, what a refusal leaves in a world and what it leaves outside are for the writer and for the builder to read in the text.
- **A held capability.** "A step without waiting, before the capability has posted" is made while the harness holds what the capability posts: nothing has joined the pending record when the step is answered. The harness then lets the capability go and waits, as before every step, until it has nothing left to post.
- **The worlds of one scenario.** In `batch-state` each world of a scenario runs on an instance of its own, with its own content root, its own store and its own capability. In `failed-replacement` the worlds of a scenario run on one instance, with one content root, one store and one capability, and are stepped in rounds as the same-harness sheet states.
- **What the harness reports beside the oracle.** Of the stand-in pipeline, after each save: whether its pass put a revision in place; the content under which the domain manifest of that revision holds each asset, an asset it does not hold being named as absent; and each action that failed, with its source and its diagnostic. Of the loading capability, for each world: each part that was obtained for that world and not posted to it, with the asset, the content and the cause. Neither report is an element of an input record, and neither is compared with the oracle, which states what each world receives.

### Additions to the format of an oracle

Beyond the format of the two earlier sheets:

- **The size.** Where a family gives sizes, an arrival to a hash is written `arrival <asset> <hash> size <bytes>`, followed by `marked` where it answers a restore, and an entry in force is written `entry <asset> <type> available <hash> size <bytes>`. An arrival to failed, an entry that is unavailable or failed, an event and a `rejected` line state no size. Where a family gives none, both lines are written as the earlier sheets write them.
- **The statement `rejected <asset> <hash>` and the reason `over-budget`** of the same-harness sheet are used by the family `batch-state` as that sheet states them. The reason stands for the reason of the budget's own that the design text now gives to the failure of a rejected first load.
- **The reason `not-admitted`**: the admission check of the world's composition refuses the part, on a first load. The design text lists this reason among the reasons of a failure and gives it no name.

### Family `batch-state`: a batch complete against the state at the end of the last tick

The claim: a batch is complete against the state at the end of the last tick before the drain. It is stated of three situations, each on a running world and on a gated one: a world that rejects an arrival over its budget, frees the room and receives a later revision of the same asset; a dependency that left the table when its parent failed, the parent being retried; and a former dependency that a later revision lists again. The first two are each written twice, since each can be reached in two ways.

Assets `D`, `G`, `K`, `P`, `X`. The pool is `D`, `P`. A live loading capability over a loose store written by the stand-in pipeline. No world names a content set. The table is empty at tick 0.

| Content | Size in bytes | Dependencies its part lists |
| --- | --- | --- |
| `D.1` | 6 000 | none |
| `G.1` | 1 000 | none |
| `K.1` | 3 000 | none |
| `P.1` | 2 000 | `D` |
| `P.2` | 2 000 | `D` |
| `P.3` | 2 000 | `D`, `X` |
| `P.4` | 2 000 | `D`, `X` |
| `P.5` | 2 000 | none |
| `X.1` | 1 500 | none |

The first revision holds `D.1`, `G.1`, `K.1`, `P.1` and `X.1`. Each scenario names its later revisions; each is put in place between the two ticks named and holds what the revision before it holds but where the scenario says otherwise.

Each scenario has two worlds. `running` is the scenario as stated. `gated` is the same scenario with what its paragraph adds, so that the batch under test joins the pending record of a world whose gate is closed. The oracle states both, and states what the records hold: with a live capability the content of each batch is part of the statement.

Controls, each the rule changed:

- Every scenario of the family, on both worlds: the loading capability builds a batch against what it posted to the world, an asset it posted to a hash counting as held available at that hash until it posts that asset again, in place of the table extracted at the end of the last tick the world ran.
- `batch-state/former-dependency-listed-again`, a second control: the loading capability builds the batch of a revision against the table extracted one tick before the last.
- `batch-state/retry-of-a-parent-that-left-the-manifest`, a second control, the reading not taken: the wants the core added for the dependencies of a parent that is failed stand while that parent holds a want.

**An arrival rejected over the budget, the room freed, then a later revision of the same asset.**

- **`batch-state/revision-after-a-rejected-replacement`**. A budget of 12 000 bytes. Record of tick 1: `want K parameters source:host optional`, then `want P parameters source:host required`. Between tick 3 and tick 4 the second revision is put in place: it holds `P.3` in place of `P.1`. Record of tick 5: `release K source:host`. Between tick 7 and tick 8 the third revision is put in place: it holds `P.4` in place of `P.3`. Run to tick 10.
  - `gated`: the record of tick 7 holds `want G parameters source:host required`. After tick 7, a step without waiting, before the capability has posted and before the third revision is put in place. Then the third revision is put in place, and the world steps.
- **`batch-state/revision-after-a-rejected-first-load`**. A budget of 10 000 bytes. Record of tick 1: `want K parameters source:host optional`. Record of tick 3: `want P parameters source:host required`. Record of tick 5: `release K source:host`. Between tick 7 and tick 8 the second revision is put in place: it holds `P.2` in place of `P.1`. Run to tick 10.
  - `gated`: the record of tick 7 holds `want G parameters source:host required`. After tick 7, a step without waiting, before the capability has posted and before the second revision is put in place. Then the second revision is put in place, and the world steps.

**A dependency that left the table when its parent failed, the parent being retried.**

- **`batch-state/retry-of-a-parent-that-left-the-manifest`**. No budget. Record of tick 1: `want P parameters source:host optional`. Between tick 3 and tick 4 the source of `P` is deleted: the second revision holds what the first holds, without `P`. Record of tick 7: `want P parameters source:tool optional`. Between tick 7 and tick 8 the source of `P` is saved again as it was: the third revision holds `P.1` again. Run to tick 10.
  - `gated`: the want of tick 7 is `want P parameters source:tool required`. After tick 7, a step without waiting, before the capability has posted and before the source of `P` is saved again. Then the source is saved again, and the world steps.
- **`batch-state/retry-of-a-root-that-never-rose`**. No budget, and no revision after the first. When the scenario starts, the artifact of `P.1` is cut short. **The repair**, between tick 3 and tick 4, puts the whole artifact of `P.1` in the store, under the same hash. Record of tick 1: `want P parameters source:host optional`. Record of tick 5: `want P parameters source:tool optional`. Run to tick 8.
  - `gated`: the want of tick 5 is `want P parameters source:tool required`. After tick 5, a step without waiting, before the capability has posted. Then the world steps.

**A former dependency that a later revision lists again.**

- **`batch-state/former-dependency-listed-again`**. No budget. Record of tick 1: `want P parameters source:host required`. Between tick 3 and tick 4 the second revision is put in place: it holds `P.5` in place of `P.1`. Between tick 5 and tick 6 the third revision is put in place: it holds `P.2` in place of `P.5`. Run to tick 8.
  - `gated`: the record of tick 5 holds `want G parameters source:host required`. After tick 5, a step without waiting, before the capability has posted and before the third revision is put in place. Then the third revision is put in place, and the world steps.

**What the family leaves out.** The batch of a mount, of an unmount and of the answers to a restore, and a source that answers in the capability's place. A batch built for a world that has run no tick, whose state a query gives. A batch built while a tick runs: the host waits before every step, so every batch is built between two ticks. Two worlds on one capability. A prepared form built once, whose size an arrival counts with its part. A gated world whose pending batch its budget rejects. A rejected replacement offered again with no later revision of its asset. A parent that fails after it was in force and is retried by a want with no revision between the retry and its answer. A world under a named set.

### Family `failed-replacement`: a replacement that fails leaves what is in force

The claim: a replacement that fails leaves what is in force, and the rest of its revision is applied. Two ways a new version fails are run: a source saved with an error, and a part that a world's admission check refuses. Three scenarios stand beside them for what the same rules give at their edges: an asset whose part lists a refused asset, a refused part that lists a new dependency, and a source that fails at its first import.

Assets `A`, `B`, `N`, `P`. The pool is `A`, `B`, `P`. A live loading capability over a loose store written by the stand-in pipeline. The family gives no size and no budget. No world names a content set. The table is empty at tick 0.

| Content | Dependencies its part lists | Against the bound of the admission check |
| --- | --- | --- |
| `A.1` | none | within |
| `A.2` | none | within |
| `B.1` | none | within |
| `B.2` | none | above |
| `B.3` | none | within |
| `N.1` | none | within |
| `P.1` | `B` | within |
| `P.2` | `B` | within |
| `P.3` | `B`, `N` | above |
| `P.4` | `B`, `N` | within |

Each scenario names the sources its content root holds when it starts, each with its first content, and its saves. A source "saved as `B.2`" is saved with the parameters and the dependencies of that content. Each world is stepped to tick 9.

Controls, each the rule changed:

- `failed-replacement/source-saved-with-an-error`: the pipeline writes a revision without an entry for an asset whose action failed, in place of its former entry. A second control, named for the worlds `required` and `optional`, since it cannot differ on the world `late`: an action that fails holds the revision back, the pipeline putting no revision in place until every action of a pass has succeeded.
- `failed-replacement/revision-of-two-with-one-refused`: no admission check is run, on the worlds `checked` and `late`; it cannot differ on `unchecked`. A second control, on `unchecked`: a part that one world of the instance refuses is posted to no world of that instance. A third, on `checked`: the batch of a revision is held back while the world refuses one of its assets. A fourth, on `checked`: a replacement the world refuses is posted to it as a failure.
- `failed-replacement/dependent-of-a-refused-asset`: an asset whose part lists a refused asset is withheld with it.
- `failed-replacement/refused-parent-with-a-new-dependency`: no admission check is run.
- `failed-replacement/first-import-saved-with-an-error`: the batch of a revision holds no arrival for an entry that is failed. A second control: an action that fails holds the revision back.

**A source saved with an error, then corrected.**

- **`failed-replacement/source-saved-with-an-error`**. No composition carries the check. The content root holds the sources of `A` and of `B`. Three worlds, listed and stepped in this order:
  - `required`. Record of tick 1: `want A parameters source:host required`, then `want B parameters source:host optional`.
  - `optional`. Record of tick 1: `want A parameters source:host optional`, then `want B parameters source:host optional`.
  - `late`. Record of tick 5: `want A parameters source:host required`.

  Between tick 3 and tick 4, **the erroneous save**: the source of `A` is saved with an error, and the source of `B` is saved as `B.2`. Between tick 6 and tick 7, **the correction**: the source of `A` is saved again, without the error, as `A.2`.
- **`failed-replacement/first-import-saved-with-an-error`**. No composition carries the check. The content root holds the source of `B` alone. One world. Record of tick 1: `want B parameters source:host optional`. Between tick 3 and tick 4: the source of `N` is saved for the first time, with an error, and the source of `B` is saved as `B.2`. Record of tick 5: `want N parameters source:host required`. Between tick 6 and tick 7: the source of `N` is saved again, without the error, as `N.1`.

**A revision of which a world's admission check refuses one asset.**

- **`failed-replacement/revision-of-two-with-one-refused`**. The content root holds the sources of `A` and of `B`. Three worlds, listed and stepped in this order:
  - `checked`, whose composition carries the check. Record of tick 1: `want A parameters source:host required`, then `want B parameters source:host required`.
  - `unchecked`, whose composition does not carry it. The same record of tick 1.
  - `late`, whose composition carries the check. Record of tick 5: `want B parameters source:host required`.

  Between tick 3 and tick 4: the sources of `A` and of `B` are saved as `A.2` and as `B.2`. Between tick 6 and tick 7: the source of `B` is saved as `B.3`.
- **`failed-replacement/dependent-of-a-refused-asset`**. The content root holds the sources of `B` and of `P`. One world, whose composition carries the check. Record of tick 1: `want P parameters source:host required`. Between tick 3 and tick 4: the sources of `B` and of `P` are saved as `B.2` and as `P.2`. Between tick 6 and tick 7: the source of `B` is saved as `B.3`.
- **`failed-replacement/refused-parent-with-a-new-dependency`**. The content root holds the sources of `A`, of `B`, of `N` and of `P`. One world, whose composition carries the check. Record of tick 1: `want A parameters source:host optional`, then `want P parameters source:host required`. Between tick 3 and tick 4: the sources of `A` and of `P` are saved as `A.2` and as `P.3`. Between tick 6 and tick 7: the source of `P` is saved as `P.4`.

The oracle states what each world receives. The clause of the claim on a revision that was not written is observed on the stand-in pipeline, by the report the harness makes beside the oracle after each save.

**What the family leaves out.** A new part that fails its verification or its validation, a damaged artifact under a new hash for example, and a prepared form whose preparation fails. An action that could not run because another failed: no source of the family is saved with an error while a source that lists its asset is saved in the same pass. A hermetic pass. A pass in which an action fails and nothing else changes. A refused version offered again with no later save of its source. A refused replacement of an entry that holds no want. A refusal under a budget, and a refusal at a restore. An admission check that reads more than one part, and an asset type that carries code. A world under a named set. The time from a save to the tick that applies its batch.

## Part: prepared forms, a dedicated server and three magnitudes

This part states the inputs of five families. Two are judged on states and each has an oracle: a prepared form built once (`prepared-form`) and a dedicated server beside a client (`dedicated-server`). Three are judged on a magnitude or on a ceiling and have inputs and no oracle: the incremental form of the gate (`gate-form`), a decoder at load under the host's ceiling (`decoder-ceiling`) and the bound above which work at load leaves the executor (`load-bound`).

It follows [`scenarios.md`](scenarios.md) and [`scenarios-same-harness.md`](scenarios-same-harness.md) for the conventions and for the format of an oracle, and repeats neither. It states no expected result: an oracle is written by hand from the design text, by a writer who has not read the build, and the build is written from the same text by someone who has not read the oracles. Where the two differ, the text arbitrates.

**Sizes.** The scenarios of `prepared-form` that run under a budget give a size for each content, and the oracle states the size of each arrival to a hash and of each available entry. The other scenarios of `prepared-form` and the family `dedicated-server` give no size, and their oracles state none: the harness writes there the size the domain manifest gives, which is not judged.

### Conventions added

- **A prepared form built once.** In the families `prepared-form` and `dedicated-server` the composition declares, for the asset type `second`, one prepared form that is built once, prepared by `stamp` at version 1, but where a world says that its composition declares none. An asset of the type `parameters` has no prepared form. Every store of these two families is one its host trusts.
- **The stand-in preparing function.** `stamp` at version 1 takes the canonical bytes of a part and a platform name. Its output is the UTF-8 bytes of the text `stamp 1`, one space, the UTF-8 bytes of the platform name, one zero byte, then the content hash of the part; where a family states a size for the form, zero bytes are appended up to that size. The stand-in pipeline runs it as a preparing action, once per part of the type `second`, and writes the hash and the size of its output in the domain manifest, beside the hash and the size of the part, under the name `stamp` and the version 1.
- **The content built once, and a machine's own build.** The content of a family is prepared under the fixed platform name `producer`, on whatever machine runs the pipeline: this is **the producer's content**, and its domain manifest is **the producer's manifest**. Every machine that runs the harness has a local platform name of its own, its operating system and its processor architecture as its toolchain names them, which is never `producer`. **The machine's own build** is the same sources prepared under that local name: the same parts under the same hashes, another form for each part of the type `second`, and so another domain manifest, **the machine's own manifest**, whose hash is not the producer's. A form a machine builds for itself is held under its own hash and never under the hash of the form built once.
- **The hashes of forms** are named after the part and after who built the form. `M.1:built` is the form built once from the part `M.1`, the producer's. `M.1:local` is the form the machine that runs the harness builds for itself from the same part: the harness computes it on each machine, and by construction it differs from `M.1:built` on every machine.
- **Stores of the family `prepared-form`.** **The producer's store** holds the parts and the forms built once of the producer's content, with the producer's manifest. **The machine's own store** holds the same parts and the machine's own forms, with the machine's own manifest, and holds no bytes under the hash of a form built once. A scenario may ask for a store that holds the artifacts of both, or for the parts with the machine's own forms and none built once. Wherever a store holds a machine's own form, the harness also places that form where the build keeps the forms a machine builds for itself, if it keeps such a place, under the key of the part's hash and of `stamp` at version 1.
- **The consumer of a form.** The harness adds one system, `reader`, to the schedule of a world whose composition declares the form. It runs once in every tick, after the tick's input fixed point. For each entry of the asset table that is available and whose type has a declared form, in the table's order, it asks for the prepared form of that asset as a system of the declaring capability would read it inside the domain, and the harness records the content hash of the bytes it is handed, computed over those bytes and not read from the table. It writes nothing to the world.
- **A replay in a process beside.** A recording is the input log of a world. It is replayed in a new process, over a store its scenario states, in a world made by the same creation, which names no content set or is created under the one its scenario states. The replaying world receives its arrivals from the recording and from no loading capability; its instance holds the manifest its scenario states.
- **Sizes under a budget.** Where a scenario of `prepared-form` runs under a budget, every part and every form has a size in bytes, stated in a table, and the stand-in pipeline pads each to that size. A world's budget for its asset data is stated in bytes with its scenario and does not change during it. The reason of a failure that the core derives from a rejection over a budget is written `over-budget`, as in the sheet of the same-harness stage.
- **Two selections of one content.** In the family `dedicated-server` one run of the stand-in pipeline gives one domain manifest with its parts and its forms built once, and one presentation manifest with its payloads for the one target profile `desktop`. **The server's selection** is the domain manifest, the parts and the forms built once, and nothing else. **The client's selection** is the server's with the presentation manifest and its payloads. Each process has an instance and a live loading capability of its own over a loose store that holds its selection.
- **The stand-in presentation manifest** is one more document under the root of the content, with a hash of its own. It holds an entry for each asset that has a payload, and none for an asset that has no payload. An entry holds one variant, `full`, with the hash and the size of its payload and no layout, and the list of the asset's presentation dependencies, by identifier. The payload of an asset is the UTF-8 bytes of the text `payload`, one space, the name of the part it was made for (`T.1`), one space, and a revision number of the payload that starts at 1, padded with zero bytes to 256 bytes. No canonical part and no domain manifest holds anything of a payload.
- **The stand-in presentation consumer** stands in the client's process and in no world. After each tick it reads the extracted state and, for each asset the table holds available for which the presentation manifest holds an entry, it asks the loading capability for the payload of the variant `full` of that asset and of each of its presentation dependencies. It makes no want and writes nothing to a world. A payload that cannot be loaded is reported to it, outside the domain.

### Additions to the format of an oracle

Beyond the formats of the two earlier sheets:

- `form <form-hash>` after the hash of a part, in an availability and in an arrival: `entry <asset> <type> available <hash> form <form-hash>` and the record element `arrival <asset> <hash> form <form-hash>`. It states the hash of the prepared form built once that the arrival names and that the entry holds beside the hash of its part. It stands only where the world's composition declares a form built once for the asset's type; no world of this part has more than one such form for a type. `<form-hash>` is a name of the convention above, `<asset>.<version>:built` or `<asset>.<version>:local`.
- `size <bytes>` after the hash of a part and after the `form` it may have, in an availability and in an arrival to a hash, in the scenarios that give sizes: `entry <asset> <type> available <hash> form <form-hash> size <bytes>` and `arrival <asset> <hash> form <form-hash> size <bytes>`. An arrival that answers a restore ends with `marked`, after its form and its size. An arrival to failed, an entry that is failed and an entry that is unavailable state no form and no size.
- `handed <asset> <form-hash>`: one line per entry that is available after tick `n` and whose type has a declared form, in the table's order, after the last `entry` and `want` line of the block and before its `event` lines. It states the bytes the consumer of the form was handed at tick `n`, by their hash. The harness compares it with what it recorded, as it compares a verdict. A block of a world whose composition declares a form states every such line, as it states every entry, and a tick that has no block has the `handed` lines of the block before it, as it has its table.
- `replay <verdict>`: for a world that replays a recording, the outcome of the replay. `<verdict>` is `runs`, or `refused <n> hash <hash>` for a replay refused at the record of tick `n`, naming a hash that cannot be had. A refused replay states the ticks before `n` and none from `n` on.
- A `restore` line may name the hash of a form: `restore refused hash <asset> <form-hash>` is a restore under the recorded content refused for the form of that asset, naming the recorded hash of the form.
- Where a scenario has several worlds, a `restore` line or a `replay` line stands right after the `world` line of the world it speaks of. A world stated `as` another has that world's `restore` or `replay` line and its `blocked` lines with its tick blocks.
- `rejected <asset> <hash>` names the hash of the part of the rejected arrival, as in the sheet of the same-harness stage, whatever form that arrival names.

### Family `prepared-form`: a form built once is never read under another hash

The claim: a part or a form built once is never read under another hash. A world whose table names a form built once is replayed, and its save is restored, on a machine that holds a form of the same asset that it built for itself, with a named content set and without one. The family adds the hash of a form in the confirmation of a restore under the recorded content, and the size of a form counted with its part under a world's budget.

Assets `A`, `K`, `M`. `M` is of the type `second`, which has the form; `A` and `K` are of the first type. The pool is `A`, `K`. No canonical part of this family lists a dependency. The producer's content holds `A.1`, `K.1` and `M.1` with its form `M.1:built`; the scenarios under a budget add two later contents of `M`. The machine's own build holds `A.1`, `K.1` and `M.1` with the form `M.1:local`.

**What the oracle states of a form, and what the harness reports beside it.** The workload reads its parameters from canonical parts and reads nothing of a form. The oracle states a form where the design text places one in what a world observes: in the arrival of its asset, as an element of the input record; in the entry, beside the hash of the part, so in the canonical bytes, under the state hash and in a save; in the verdict of a restore; and in the verdict of a replay. Whether an event names a form is for the writer and for the builder to read in the text. Beside the oracle the harness reports, for every tick and for every available entry whose type has a declared form, the hash of the bytes the consumer `reader` was handed: the oracle states that report by its `handed` lines, and a build that hands the consumer other bytes than the ones the entry names differs from the oracle on those lines and nowhere in the table. The harness also reports, for each replay that runs, whether its state hashes equal the recorded world's, tick by tick, and, for `prepared-form/own-build-names-its-own-form`, whether the hash of the machine's own manifest is the producer's and whether the state hashes of its worlds equal those of `prepared-form/recorded-run`.

#### Scenarios that give no size

In each world of these scenarios the composition's contributor `base` wants `A` as required at tick 0, and the record of tick 2 holds `want M second source:host required`. The loading capability is live, but in a replay. Each world runs to tick 6.

- **`prepared-form/recorded-run`**. One instance whose manifest is the producer's, over the producer's store. Two worlds, stepped in rounds in this order: `no-set`, which names no content set, and `under-set`, created under the hash of the producer's manifest. Each world is saved after tick 4: **the save** of that world. The input log of each world is **the recording** of that world. Control: the hash of a prepared form left out of the arrival and of the entry.
- **`prepared-form/replay-beside-the-machine-own-form`**. Each recording is replayed in a process beside, over a store that holds the artifacts of the producer's store and, beside them, the machine's own form `M.1:local`. World `no-set` replays the recording of `no-set` in a world that names no set, on an instance whose manifest is the machine's own. World `under-set` replays the recording of `under-set` in a world created under the hash of the producer's manifest, on an instance whose manifest is the producer's. Control: a prepared form is taken from what the machine built for the part, wherever the machine holds one. A second control: the hash of a prepared form left out of the arrival and of the entry.
- **`prepared-form/replay-without-the-form-built-once`**. As the scenario above, each store holding the parts and the machine's own form `M.1:local`, and no bytes under `M.1:built`. Control: a replay takes the form its machine built for the part where the form a record names cannot be had.
- **`prepared-form/own-build-names-its-own-form`**. Not a replay: a run on an instance whose manifest is the machine's own, over the machine's own store, with a live loading capability. Two worlds, stepped in rounds in this order: `no-set`, which names no set, and `under-own-set`, created under the hash of the machine's own manifest. Control: the form a machine builds for itself is written in its manifest and held in its store under the hash of the form built once.
- **`prepared-form/form-built-once-gone-from-the-store`**. Not a replay: a run on an instance whose manifest is the producer's, over a store that holds the parts and the machine's own form `M.1:local`, and no bytes under `M.1:built`, with a live loading capability. Two worlds, stepped in rounds in this order: `no-set`, and `under-set`, created under the hash of the producer's manifest. Control: the loading capability takes the form its machine built for the part, or builds it, where the store holds no bytes under the hash the manifest names.

**Readers of a save.** Each loads a save of `prepared-form/recorded-run` in a new process, into the saved world's composition, and steps to tick 6; its first tick is tick 5. A reader's record is empty but for what the capability posts. But where a reader says otherwise it reads the save of `no-set` and names no content set. For a reader under the content at hand the oracle states what a step without waiting names before the answers.

- **`prepared-form/restore-confirms-the-form`**. Under the recorded content, on an instance whose manifest is the producer's, over a store that holds the artifacts of the producer's store and the machine's own form `M.1:local` beside them. Two readers, each a process of its own: `no-set`, as above, and `under-set`, which reads the save of `under-set` and names the content set of the producer's manifest. Control: a prepared form is taken from what the machine built for the part, wherever the machine holds one. A second control: the hash of a prepared form left out of the arrival and of the entry.
- **`prepared-form/restore-refused-form-gone`**. Under the recorded content, on an instance whose manifest is the producer's, over a store that holds the parts and the machine's own form `M.1:local`, and no bytes under `M.1:built`. Control: the confirmation of a restore reads the hash of the canonical part alone.
- **`prepared-form/restore-refused-own-build`**. Under the recorded content, on an instance whose manifest is the machine's own, over the machine's own store. Control: the confirmation of a restore reads the hash of the canonical part alone.
- **`prepared-form/restore-own-manifest-beside-the-form-built-once`**. Under the recorded content, on an instance whose manifest is the machine's own, over a store that holds the artifacts of the producer's store and the machine's own form `M.1:local` beside them: the bytes of the form built once are in the store, and the manifest the reader stands under holds another form for `M`. The design text can be read two ways on this reader: an asset is confirmed where the manifest the restored world stands under holds it at the recorded hashes and the store holds their bytes; or an asset is confirmed where the store holds bytes under each recorded hash, whatever that manifest holds. Which of the two the text gives is for the writer and for the builder to read. Control: the other of the two readings.
- **`prepared-form/content-at-hand-same-form`**. Under the content at hand, on an instance whose manifest is the producer's, over the producer's store. Control: the hash of a prepared form left out of the arrival and of the entry.
- **`prepared-form/content-at-hand-own-build`**. Under the content at hand, on an instance whose manifest is the machine's own, over the machine's own store. Control: an answer to a restore is compared with the recorded hash of the canonical part alone.

#### Scenarios under a budget

One instance whose manifest is the producer's, over the producer's store, with a live loading capability. No world names a content set. The oracle states a size in each arrival to a hash and in each available entry.

| Content | Size of the part | Its form built once | Size of the form |
| --- | --- | --- | --- |
| `K.1` | 5 000 | none | |
| `M.1` | 2 000 | `M.1:built` | 4 000 |
| `M.2` | 2 000 | `M.2:built` | 4 500 |
| `M.3` | 3 000 | `M.3:built` | 1 000 |

- **`prepared-form/counted-with-its-part`**. One world with a budget of 11 000 bytes. The composition's contributor `base` wants `K` as required at tick 0. Record of tick 3: `want M second source:host required`. Between tick 5 and tick 6 a revision that holds `M.2`, with its form, in place of `M.1` is put in place. Between tick 8 and tick 9 a revision that holds `M.3`, with its form, in place of `M.2` is put in place. Run to tick 11. Control: the size an arrival names and an entry holds is the size of the canonical part alone. A second control: the budget only reports, and every arrival is applied.
- **`prepared-form/first-load-over-the-budget-by-its-form`**. Two worlds on the one instance, stepped in rounds in this order, each with a budget of 10 000 bytes: `with-the-form`, whose composition declares the form, and `without-the-form`, whose composition declares the type `second` and no prepared form for it, and has no consumer `reader`. In each, the contributor `base` wants `K` as required at tick 0, and the record of tick 3 holds `want M second source:host required`. Run to tick 6. Control for `with-the-form`: the size an arrival names and an entry holds is the size of the canonical part alone. Control for `without-the-form`: an arrival names every form the domain manifest holds for its asset, with its size, whatever the world's composition declares.

**What the family leaves out.** A revision that changes a form and leaves its part as it was, as a new version of what prepares it gives: the design text has no sentence on whether such a revision changes the asset, nor on what the event of such an arrival names. A type with two forms built once, the text giving no order for their hashes in an entry or in an arrival. A restore into another composition whose preparer is absent or at another version, where the text has the form obtained anew and names no transition that would carry it. A form built once read from a store its host does not trust, with a validator of the declaring capability and without one. A world created under the producer's content set on an instance whose own manifest was never that set. A peer that joins. A recording that embeds its parts and its forms. A form that is the same computation on every cell, which is a cache of the machine and stands in no table. A parent whose part lists an asset that has a form, which the family `dedicated-server` holds. A table saved and restored under a budget, and a run under a budget replayed on another machine.

### Family `dedicated-server`: a server started from the domain set alone

The claim: a dedicated server started from the domain set alone gives the hashes of a client that holds both sets. Two processes over one content, one with no presentation manifest: each holds one world made by the same creation and given the same records.

Assets `C`, `D`, `M`, `S`, `T`, `U`, `V`. `M` is of the type `second`, with its form built once; every other asset is of the first type. The pool is `C`, `S`, `T`, `V`. The canonical part `S.1` lists three dependencies: `D` under the first type, `M` under the type `second` and `T` under the first type. No other part lists a dependency. This family gives no size.

**The content**, one run of the stand-in pipeline for each revision:

- **The first revision.** Its domain manifest holds `C.1`, `D.1`, `M.1` with its form `M.1:built`, `S.1`, `T.1`, `U.1` and `V.1`. Its presentation manifest holds three entries: `T`, with its payload and with `U` as its one presentation dependency; `U`, with its payload; and `V`, with its payload, whose artifact is cut short in the client's store from the start. `C`, `D`, `M` and `S` have no payload and no entry.
- **The presentation revision** changes the payload of `T`, whose revision number becomes 2, and changes no canonical part: its domain manifest is byte for byte the first revision's. It is put in place between tick 6 and tick 7, in the client's process; the server's selection holds nothing that it changes, and nothing is put in place there.
- **The domain revision** holds `T.2` in place of `T.1`, with the payload made for `T.2` in the client's selection. It is put in place between tick 8 and tick 9, in both processes.

**The two worlds**, `client` in the client's process and `server` in the server's, each with the same composition, which declares the two types and the form. In each, the composition's contributor `base` wants `C` as required at tick 0, and:

- record of tick 2: `want S parameters source:host required`, then a step without waiting, before the capability has posted;
- record of tick 4: `want V parameters entity:E1 optional`;
- record of tick 10: `release S source:host`.

Each world runs to tick 12. The oracle states the world `client` in full, and the world `server` has the same statement, written `world server as client shifted 0`. The harness compares the state hashes of the two worlds, tick by tick.

- **`dedicated-server/same-records`**. The two worlds name no content set. Controls, each the rule changed in the loading capability of both processes, which changes the world of one of them alone:
  - the loading capability resolves an identifier in the presentation manifest where its instance holds one;
  - the closure of a want follows the presentation dependencies with the domain dependencies;
  - an arrival is posted once the payloads of its asset are resident, and is a failure where one cannot be loaded;
  - a revision changes an asset whose entry in a presentation manifest differs;
  - a server's selection holds the canonical parts alone, without the forms built once.
- **`dedicated-server/under-the-domain-set`**. The same, each world created under the hash of the first revision's domain manifest. Each world is saved after tick 5, and the harness reads the hash of the content set in the header of each save and compares the two. Control: the hash that names a content set covers every manifest its instance holds, the presentation manifest included.
- **`dedicated-server/client-save-read-by-the-server`**. The world `client` of `dedicated-server/same-records` is saved after tick 5. A reader loads that save in a new process over the server's selection of the first revision, under the recorded content, into the saved world's composition, naming no content set. Its first tick is tick 6. The domain revision is put in place between tick 8 and tick 9, the reader's record of tick 10 holds `release S source:host`, and it steps to tick 12. The harness also compares its state hashes with those of the world `client` of `dedicated-server/same-records`, from tick 6 on. Control: the confirmation of a restore confirms the presentation payloads of the assets the table holds. A second control: a server's selection holds the canonical parts alone, without the forms built once.

**What the harness reports beside the oracle.** Per process: how many times the loading capability read a presentation manifest and a payload; for the client, the payloads its presentation consumer received and the ones it was refused, with the tick of each; and, tick by tick, whether the state hashes of `client` and of `server` are equal.

**What the family leaves out.** A server's save read by a client. A client that holds two target profiles, or an asset with two variants. A payload read by ranges. A host that paces on the readiness of its payloads, as a loading screen does. A budget, and so the size of a payload held outside a world's count. The host manifest. A server whose composition lacks a capability that the client's has and that declares a prepared form. A package in place of a loose store, and a mount. Two machines: the two processes of a run stand on one machine, and what two platforms add is the run's, not the sheet's.

### Family `gate-form`: the incremental form of the gate

The claim: the incremental form of the gate costs under a tenth of a tick. It is a magnitude: it has no oracle and no control, and it is judged on one quiet machine, in the release profile, over at least five repetitions.

**The form**, as the design text has it. The table keeps the set of the required assets it holds unanswered, brought up to date wherever a want or an availability changes, and the gate reads the pending elements against that set. The same form keeps the count of the entries a restore still awaits an answer for. Neither is part of the canonical bytes.

**The world and the table** are those of the family `gate-cost` of the sheet of the same-harness stage, so that the figures compare: the reference workload at default size, 10 000 entities, at 1, 4 and 8 workers, with a scripted availability source; 2 000 roots and 8 000 leaves, every one available from tick 2 on; the asset `G` outside the table.

**The tick it is measured against** is the one of `gate-cost`: the median duration of one tick of that world at the same worker count, over the ticks 3 to 102, each with an empty record and an open gate.

**First measurement: one evaluation on the large table.** The three cases of `gate-cost`, each with its two sizes of pending record, under the names `gate-form/open`, `gate-form/closed` and `gate-form/opens`. What is timed is what `gate-cost` times: one evaluation, from the moment the host asks whether the world may run its next tick, the pending record being assembled, to the verdict, open, or closed with the assets awaited named. It is made 100 times on the same pending record, which none of them applies, and the median is kept. In the case `open` the world is not gated, and by the design text such a world steps without reading the record of its next tick: what is timed there is what the host does to learn that the world may step, and the report says whether it read the pending record.

**Second measurement: one evaluation on a restored table that awaits its answers.** The world above is saved after tick 102. A reader loads the save in a new process, under the content at hand, which is the saved world's, naming no content set, with a scripted availability source in place of a loading capability, so that the pending record is what a case states. The restored world has run no tick and awaits an answer for each of its 10 000 available entries. Four cases, by what the pending record of its first tick holds, every answer being marked and at the recorded hash:

- **`gate-form/restore-nothing-pending`**: nothing.
- **`gate-form/restore-one-closure-pending`**: the answers of `R0000` and of its four leaves.
- **`gate-form/restore-half-pending`**: the answers of the roots `R0000` to `R0999` and of their 4 000 leaves.
- **`gate-form/restore-all-pending`**: the 10 000 answers.

Two durations are timed apart, each 100 times on the same pending record, which none of them applies, the median kept: the verdict alone, open or closed, with the count of the entries still awaited; and the whole answer of a step without waiting, which also names the assets awaited. Each is set against the median tick of the first measurement at the same worker count, the restored world having run no tick.

**Third measurement: the cost the form moves to every change of a want or of an availability.** A run of its own, from the table of tick 102: the world above, running and never gated, from tick 103 to tick 302. The content gains 2 000 assets `X0000` to `X1999` of the type `parameters`, whose parts list nothing and which the table does not hold at tick 102, and a second content, with other parameters and no dependency, for each leaf of ranks 400 to 3 999. With `j` the tick number less 103, the record of each of these ticks holds, in this order:

- for the 100 assets `X` of ranks `100 (j mod 20)` to `100 (j mod 20) + 99`: `want <asset> parameters source:host required`;
- for the 100 assets `X` wanted by the record of ten ticks before, where there is one: `release <asset> source:host`;
- for the 20 roots of ranks `1000 + 20 (j mod 50)` to `1000 + 20 (j mod 50) + 19`: `release <root> source:host`;
- for the 20 roots released by the record of the tick before, where there is one: `want <root> parameters source:host required`;
- for the 100 roots `R0000` to `R0099`: `want <root> parameters source:host optional` where `j` is even and `want <root> parameters source:host required` where `j` is odd, each a change of the policy of a want its maker holds;
- the arrivals of the 100 assets `X` wanted by this record, each at its one content;
- the arrivals of the 100 leaves of ranks `400 + 100 (j mod 36)` to `400 + 100 (j mod 36) + 99`, each at its second content where the table holds it at its first, and at its first where the table holds it at its second.

Every required want of such a record is made on an asset whose arrival stands in the same record, or on an asset the table holds available, so the world's state holds every required asset answered at every boundary. The same 200 ticks are run twice: with the form, and in a variant of the build that keeps neither the set nor the count, whose gate the host does not ask. What is timed is each whole tick from tick 123 to tick 302, and the median is kept for each of the two runs.

**What is reported**, per worker count and for each repetition. For the first measurement: per case and per size, the median evaluation, the median tick and their ratio. For the second: per case, the count of pending elements, the count of entries still awaited, the median of the verdict alone, the median of the whole answer, and the ratio of each to the median tick; and, once per reader, the time it takes to derive the set and the count from the restored table, which is no evaluation. For the third: the median tick with the form, the median tick without it, their difference, that difference as a share of the median tick of the ticks 3 to 102, and that difference divided by the count of changes of a want and of an availability made in one tick, the wants the core adds and ends included, which the build counts; and whether the state hashes of the two runs are equal at every tick. Once: how the build keeps the set and the count, and what its gate reads.

**The refutation**, in the row's words: a tenth of that tick or more for one evaluation. The cost moved to the changes is reported with it, and no bound is stated for it. The row names no worker count and no size of pending record: the report gives every ratio of the first two measurements and names the largest, and it says of each case of the second how many elements its pending record holds, so that a cost that follows the pending record is told from one that follows the table.

**What the family leaves out.** A restored table whose pending record holds an answer at another hash, a failure, or the ordinary arrival of a dependency a part at hand newly lists. A restored table that also holds a required want unanswered. A restore in place, which derives the set and the count again each time. A table larger than ten thousand entries. A world that is gated while its wants change.

### Family `decoder-ceiling`: a decoder at load under the host's ceiling

The claim: a decoder at load holds to the host's ceiling. It is judged on a ceiling and on the kind of a transition: it has no oracle and no control, and it runs on one machine, in the release profile, over at least five repetitions.

**The stand-in block codec**, `runs`, is the prototype's own. An artifact stored in a package is cut into blocks of at most 65 536 expanded bytes. A block is, in this order: its declared length, the count of bytes it expands to, as a 32-bit little-endian integer; its stored length, the count of bytes of the run stream that follows, as a 32-bit little-endian integer; then the run stream, a sequence of pairs, each a count from 1 to 65 535 as a 16-bit little-endian integer followed by one byte, which expands to that many copies of that byte. The honest writer of the stand-in emits, for each block, the runs of equal bytes of its input, in order, and declares the length they expand to.

**The stand-in package** is one file: for each of its assets, the slice of the domain manifest (the identifier, the type, the hash and the size of the canonical part); an index that gives, for each artifact, its hash, its declared size and the place of each of its blocks; and the blocks. It holds no presentation manifest and no asset of a type that carries code.

**The ceiling** is a setting of the mount: 1 048 576 bytes, the most the decoder may hold allocated to expand one artifact.

**The world.** One world that names no content set, with an empty table at tick 0 and a live loading capability over a loose store its host trusts, which holds nothing the scenario wants. Between tick 1 and tick 2 the package is mounted as untrusted, without leave to override. It holds three assets of the type `parameters`, none of which lists a dependency:

- `W`: an honest part padded by the stand-in pipeline to 65 536 bytes, declared so everywhere. It is the reference, a load the ceiling allows.
- `Y`: the manifest slice, the index and the header of its one block each declare 4 096 bytes, and the hash the slice names is the hash of an honest part of that size, which the package does not hold; the run stream of the block is 128 pairs, each of the count 65 535 and of the byte zero, which is 384 stored bytes that expand to 8 388 480 bytes. This is the payload that expands beyond what it declares.
- `Z`: an honest part padded with zero bytes to 4 194 304 bytes, four times the ceiling; the manifest slice and the index declare that size, and each of its 64 blocks honestly declares 65 536 bytes. This is the canonical part and the manifest that declare a size above the ceiling.

Record of tick 3: `want W parameters source:host required`. Record of tick 5: `want Y parameters source:host required`. Record of tick 7: `want Z parameters source:host required`. The world is stepped to tick 10. The three loads are the cases **`decoder-ceiling/within`**, **`decoder-ceiling/expands-beyond-its-declaration`** and **`decoder-ceiling/declared-above-the-ceiling`**.

**What is measured.** For each load, through a counting allocator of the harness scoped to the threads that read, expand, verify and validate the artifact, from the first read to the posting of the arrival: the largest single allocation, and the largest count of bytes held allocated at one moment. And the transition the world receives for the asset, in the record of the tick after its want: its kind, to a hash or failed, and the reason the build gives; whether the step names the asset; and whether the world then steps.

**What is reported**, per case and per repetition: the two counts of bytes beside the ceiling, the transition and its reason. The design text gives a list of the reasons of a failure, and none is the ceiling's own: "the bytes do not verify or do not validate" is the nearest for a block that expands beyond what it declares, and nothing of the list fits a declared size above the ceiling. The sheet asks for no reason, and the report gives the one the build names in each of the two cases.

**The refutation**, in the row's words: an allocation above the ceiling, or a load that is not a failed transition. It is read on the cases of `Y` and of `Z`. The case of `W` shows the same measurement on a load the ceiling allows.

**What the family leaves out.** A presentation payload that expands beyond what it declares, which gives a world no transition. A block whose header declares more than the index and the manifest do. A ceiling set on the instance and not on the mount, and a package its host trusts. A replacement of an asset in force that the ceiling stops. A prepared form built once brought by the untrusted package. A decoder under a fuzzed input.

### Family `load-bound`: the bound above which work at load leaves the executor

The claim: work at load above one millisecond belongs on the background pool. It is a magnitude: it has no oracle and no control, and it is judged on one quiet machine, in the release profile, over at least five repetitions. The figures are the stand-in executor's and move with a real one.

**The stand-ins.**

- **The executor**: two file threads behind the store port, which serve its requests in the order received. A request reads one artifact whole from the loose store. The work at load that the bound leaves on the executor runs on the file thread that read the artifact, before that thread takes its next request.
- **The background pool**: four worker threads that run actions in the order submitted, but where the build's pool has an order of its own, which the report then states. A piece of work at load above the bound is one action.
- **The bound** is a setting of the instance: 0.1, 1 or 10 milliseconds. A bound is a duration, and a piece of work is placed before it runs, so the stand-in estimates: when the instance starts it measures, on a buffer of 1 048 576 bytes, its rate in bytes per millisecond for each kind of work at load of this family (verifying a hash, validating a part, building the form below), and the estimate of a piece of work is its size divided by that rate. A piece of work whose estimate is above the bound is an action of the pool; any other stays on the file thread.
- **The form built at load.** For this family alone the composition declares, for the type `second`, one prepared form that is the same computation on every cell, which the loading capability builds where the machine's cache of forms does not hold it, and no form built once. Its stand-in, `grind` at version 1, takes the canonical bytes of the part and returns as many bytes: four passes, each replacing every byte by the sum, modulo 256, of the bytes up to and including it in the output of the pass before, the first pass reading the part. The machine's cache of forms is empty when a repetition starts.
- **The saturating import**: a stream of synthetic import actions, each one computation of about 2 milliseconds (passes of the content hash over a buffer of 1 048 576 bytes, their count calibrated when the instance starts), submitted to the pool so that at least eight actions are queued behind the running ones at every moment of a measurement. With the pool idle none is submitted.

**The content**, on a loose store, each part padded by the stand-in pipeline to the size stated:

| Content | Type | Size in bytes | Dependencies its part lists |
| --- | --- | --- | --- |
| `R.1` | `parameters` | 1 024 | `B` under the first type, `F` under `second` |
| `B.1` | `parameters` | 33 554 432 | none |
| `F.1` | `second` | 8 388 608 | none |
| `S000.1` to `S199.1` | `parameters` | 1 024 each | none |

`B.1` is the large part, and `F.1` the part whose form is built at load, of the same size as its part. The 200 small assets stand outside the closure of `R`.

**The world.** One world that names no content set, with an empty table at tick 0 and a live loading capability. Record of tick 1: `want R parameters source:host required`. The world is gated from the boundary after tick 1, and it runs tick 2 once the batch of the closure of `R` is posted. The world's worker count changes nothing of what is timed, the world being gated while it is timed: the family runs at 4 workers.

**The six cases**: the bound at 0.1, at 1 and at 10 milliseconds, each with the pool idle and with the pool saturated, under the names `load-bound/tenth-of-a-millisecond-idle`, `load-bound/tenth-of-a-millisecond-saturated`, `load-bound/one-millisecond-idle`, `load-bound/one-millisecond-saturated`, `load-bound/ten-milliseconds-idle` and `load-bound/ten-milliseconds-saturated`. Each repetition of a case is a new process, with an empty resident store and an empty cache of forms.

**What is measured.**

- **The latency of the executor's reads behind a computation.** From the end of tick 1 until the batch is posted, the harness sends the store port one read request of a small artifact every 250 microseconds, the artifacts of `S000` to `S199` in turn. For each request it records the wait, from the request to the moment a file thread starts the read, and, of that wait, the time during which every file thread was running a computation and not a read: the delay behind a computation.
- **The wall time the gated world waits**: from the end of tick 1, when the want leaves the domain, to the moment the batch of the closure of `R` stands on the world's queue.

**What is reported**, per case and per repetition: the largest and the 99th percentile of the delay behind a computation, and of the whole wait; the wall time the world waited; for each bound, the ratio of the wall time with the pool saturated to the wall time with the pool idle; and which pieces of work ran on a file thread and which as an action, each with its estimate and its measured duration. Once per machine: the duration, measured alone on an idle machine, of each large piece of work, which are the verification and the validation of `B.1`, the verification and the validation of `F.1` and the building of its form. The sizes are chosen so that each of these costs several milliseconds; where one of them measures under two milliseconds on the machine of the run, the report says so, the bound of one millisecond being then not shown by it.

**The refutation**, in the row's words: at one millisecond, a read delayed by more than one millisecond behind a computation, or a gated world that waits more than twice the time it waits with the pool idle. The cases at 0.1 and at 10 milliseconds are reported beside it and judged by nothing.

**What the family leaves out.** Another count of file threads or of pool workers. A pool that serves a load before an import, or in any order but the one submitted. A real import in place of the synthetic actions. A form built once, which is read and verified and never built at load. A compressed block expanded at load. A payload read by ranges. Several worlds that wait on one loading capability.
