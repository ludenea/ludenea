//! The families of the first scenario sheet, as data: the inputs the sheet
//! states, in its words, and each control as the rule it changes. No oracle
//! is read here. Throwaway, never product code.

use asset_loader::Options;
use prototype_core::table::AssetType::{Parameters, Second, Third};
use prototype_core::table::Policy::Required;
use prototype_core::table::Rules;

use crate::play::{Change, Check, FamilyPlan, Move, Scenario, SetName, Tweak, VariantPlan, WorldPlan};
use crate::run::{Content, Family, Settings, content};

pub fn core(change: fn(&mut Rules)) -> Change {
    Change::Core(change)
}

pub fn loader(change: fn(&mut Options)) -> Change {
    Change::Loader(change)
}

/// The control of every scenario of this sheet whose oracle states a table
/// that holds an entry.
const TYPE_LEFT_OUT: &str = "the entry's type left out of the canonical bytes";
/// The scenarios whose oracle states no table that holds an entry: a reader
/// whose restore is refused, or a table empty at every tick compared. The
/// control cannot differ there, and the sheet does not name it for them.
const NO_ENTRY_STATED: &[&str] = &[
    "restore/recorded-content-refused",
    "restore/another-composition-refused",
    "restore/another-composition-asset-absent",
    "restore/drop-names-a-declared-type",
    "restore/drop-names-a-declared-type-with-no-entry",
    "types/drop-not-named",
    "types/drop-refused-for-a-kept-parent",
    "types/dropped-parent-of-a-kept-dependency",
    "record/want-after-the-despawn-of-its-owner",
];
const STAYS_FAILED: &str = "a failed entry stays failed until the arrival that answers the want added on it";
const BATCH_ALONE: &str = "the wants of dependencies added and recomputed with a batch alone";

/// Adds the control to every scenario the sheet names it for.
fn with_the_type_left_out(scenarios: Vec<Scenario>) -> Vec<Scenario> {
    scenarios
        .into_iter()
        .map(|scenario| {
            if NO_ENTRY_STATED.contains(&scenario.name.as_str()) {
                scenario
            } else {
                scenario.control(TYPE_LEFT_OUT, core(|rules| rules.type_left_out_of_the_bytes = true))
            }
        })
        .collect()
}

/// A content: these contents in place of those of the same assets, and the
/// contents of the other assets as they are.
fn but(base: &[&'static str], changed: &[&'static str]) -> Vec<&'static str> {
    let asset = |name: &str| name.split('.').next().unwrap_or(name).to_string();
    let mut contents: Vec<&'static str> = base.iter().copied().filter(|name| !changed.iter().any(|other| asset(other) == asset(name))).collect();
    contents.extend_from_slice(changed);
    contents
}

/// A content whose manifest does not hold an asset.
fn without(base: &[&'static str], asset: &str) -> Vec<&'static str> {
    base.iter().copied().filter(|name| name.split('.').next() != Some(asset)).collect()
}

/// The reading not taken on the wants the core added under a parent that
/// fails: a control of the scenarios the sheets name it for, in both sheets.
pub const KEPT_UNDER_A_FAILED_PARENT: &str =
    "the wants the core added for the dependencies of a parent that is failed, or whose arrival is rejected, stand while that parent holds a want";

pub fn kept_under_a_failed_parent() -> Change {
    core(|rules| rules.core_wants_kept_under_a_failed_parent = true)
}

/// The readings the build did not take, and the other reading of each place
/// where it had to take one: run on a scenario that differs from its oracle,
/// each alone, so that a difference that is a reading shows as one. A point
/// read two ways and since decided is not among them: the reading not taken
/// is a control of the scenarios the sheets name it for, and must differ.
pub fn readings() -> Vec<VariantPlan> {
    let reading = |name: &str, changes: Vec<Change>| VariantPlan { name: name.to_string(), control: false, changes, worlds: Vec::new() };
    vec![
        reading("reading: a want added on a failed entry leaves it failed until the arrival of the retry", vec![core(|rules| rules.failed_entry_stays_failed_on_a_want = true)]),
        reading("reading: the first want decides an entry's type for good", vec![core(|rules| rules.first_type_holds = true)]),
        reading("reading: the makers under the world's own entry ordered by identity, the kind breaking a tie", vec![core(|rules| rules.makers_by_identity_alone = true)]),
        reading("reading: a despawn of a record ends its entity's wants at the structural change point", vec![core(|rules| rules.despawn_ends_wants_at_the_structural_point = true)]),
        reading("reading: an answer to a restore that fails emits no event", vec![core(|rules| rules.marked_failure_emits_nothing = true)]),
        reading("reading: an answer to a restore that fails emits no event and fails the instance", vec![core(|rules| {
            rules.marked_failure_emits_nothing = true;
            rules.marked_failure_fails_the_instance = true;
        })]),
        reading("reading: the event of a new history stands after the events of its tick's record", vec![core(|rules| rules.restore_event_after_the_record = true)]),
        reading("reading: an entry that holds no want takes the type of a want that states another", vec![core(|rules| rules.entry_without_a_want_takes_a_new_type = true)]),
        reading("reading: a want that replaces an entry's type ends the wants held under the former type", vec![core(|rules| rules.type_replacement_ends_the_former_wants = true)]),
        reading("reading: a type is corrected by a new maker alone", vec![core(|rules| rules.type_corrected_by_a_new_maker_alone = true)]),
        reading("reading: an answer to a restore for an entry no want names never differs", vec![core(|rules| rules.unwanted_answer_never_differs = true)]),
        reading("reading: a restore that awaits no answer records its new history at the restore", vec![core(|rules| rules.restore_event_at_the_restore_where_nothing_is_awaited = true)]),
        reading("reading: a restore into another composition that drops nothing continues", vec![core(|rules| rules.another_composition_without_a_drop_continues = true)]),
        reading("reading: a want added and ended inside one record still leaves the domain", vec![core(|rules| rules.a_want_ended_in_its_record_still_leaves = true)]),
        reading("reading: a dropped type the composition has, with no entry, refuses nothing", vec![core(|rules| rules.dropped_declared_type_without_an_entry_refuses_nothing = true)]),
        reading("reading: under a budget, an arrival no want names counts in its group", vec![core(|rules| rules.budget_counts_unnamed_arrivals = true)]),
        reading("reading: what a system does is applied at the input fixed point of the next tick", vec![core(|rules| rules.system_applied_at_the_next_input_fixed_point = true)]),
        reading("reading: what a system does is applied as commands of the record of its tick", vec![core(|rules| rules.system_applied_with_its_ticks_record = true)]),
        reading("reading: every want is answered, an asset in force included", vec![loader(|options| options.every_want_answered = true)]),
        reading("reading: the answers to a restore do not follow the closure", vec![loader(|options| options.restore_answers_do_not_follow_the_closure = true)]),
        reading("reading: the answers to a restore complete the batch of each closure", vec![loader(|options| options.restore_answers_complete_the_batch = true)]),
        reading("reading: nothing is posted below an asset that fails", vec![loader(|options| options.nothing_posted_below_a_failed_asset = true)]),
        reading("reading: a move of the manifest posts nothing for a failed entry", vec![loader(|options| options.moves_leave_failed_entries = true)]),
        reading("reading: a move that changes the type of an asset in force fails it", vec![loader(|options| options.a_changed_type_fails_an_asset_in_force = true)]),
        reading(
            "reading: the wants the table holds on a pending parent alone give the want the core would add its policy",
            vec![core(|rules| rules.gate_counts_core_wants_from_the_tables_wants_alone = true)],
        ),
        reading("reading: what several systems did in one tick is applied in the order of their stable names", vec![core(|rules| rules.systems_applied_in_name_order = true)]),
        reading(
            "reading: a dependency failed for its type is loaded under the manifest's type below a parent in force too",
            vec![loader(|options| options.type_exception_below_a_parent_in_force = true)],
        ),
        reading(
            "reading: a step without waiting that follows the record of a tick, where steps stand in a row, is the step that runs that tick",
            vec![Change::Harness(Tweak::StepAfterARecordRunsItsTick)],
        ),
        reading(
            "reading: the batch of a revision holds the closure of a changed part that the world's admission check refuses",
            vec![loader(|options| options.closure_of_a_refused_part_posted = true)],
        ),
    ]
}

pub fn order(settings: &Settings) -> FamilyPlan {
    const CONTENTS: &[Content] = &[content("A", 1), content("A", 2), content("B", 1), content("G", 1)];
    let family = Family::new(&["A", "B", "G"], &["A", "B"], CONTENTS, &[], settings);
    let world = || WorldPlan::new(10);
    let arrivals_first = || core(|rules| rules.arrivals_before_commands = true);
    let falls_first = || core(|rules| rules.falls_before_record = true);
    const ARRIVALS_FIRST: &str = "the arrivals of a record applied before its commands";
    const FALLS_FIRST: &str = "the falls applied before the record";
    const WANT_A: &str = "want A parameters source:host required";
    const WANT_G: &str = "want G parameters source:host required";
    const RELEASE_A: &str = "release A source:host";
    let again = || world().record(2, &[WANT_A]).record(5, &[RELEASE_A]).record(6, &[WANT_A]);
    let scenarios = vec![
        Scenario::scripted("order/want-and-arrival", world().record(4, &[WANT_A, "arrival A A.1"])).control(ARRIVALS_FIRST, arrivals_first()),
        Scenario::scripted("order/release-and-arrival", world().record(2, &[WANT_A, "arrival A A.1"]).record(5, &[RELEASE_A, "arrival A A.2"]))
            .control(ARRIVALS_FIRST, arrivals_first()),
        Scenario::scripted("order/want-again-after-release", world().record(2, &[WANT_A, "arrival A A.1"]).record(5, &[RELEASE_A]).record(6, &[WANT_A]))
            .control(FALLS_FIRST, falls_first()),
        Scenario::scripted(
            "order/want-again-after-despawn",
            world().record(2, &["want A parameters entity:E1 required", "arrival A A.1"]).record(5, &["despawn E1"]).record(6, &["want A parameters source:host optional"]),
        )
        .control(FALLS_FIRST, falls_first()),
        Scenario::live("order/want-again-live", &["A.1"], again())
            .control(FALLS_FIRST, falls_first())
            .control("every want answered, an asset in force included", loader(|options| options.every_want_answered = true)),
        Scenario::scripted(
            "order/accumulated-want-and-arrival",
            world().record(3, &[WANT_G]).record(4, &[WANT_A, "arrival A A.1"]).step(4).record(4, &["arrival G G.1"]),
        )
        .control(ARRIVALS_FIRST, arrivals_first()),
        Scenario::scripted(
            "order/accumulated-release-and-arrival",
            world().record(2, &[WANT_A, "arrival A A.1"]).record(3, &[WANT_G]).record(4, &[RELEASE_A, "arrival A A.2"]).step(4).record(4, &["arrival G G.1"]),
        )
        .control(ARRIVALS_FIRST, arrivals_first()),
        Scenario::scripted(
            "order/accumulated-want-again",
            world().record(1, &[WANT_A, "arrival A A.1"]).record(3, &[WANT_G, RELEASE_A]).record(4, &[WANT_A]).step(4).record(4, &["arrival G G.1"]),
        )
        .control(FALLS_FIRST, falls_first()),
        Scenario::scripted("order/gate-release-and-arrival", world().record(3, &[WANT_G]).step(4).record(4, &["release G source:host", "arrival G G.1"]))
            .control(ARRIVALS_FIRST, arrivals_first())
            .control("an entry that never rose leaves at once", core(|rules| rules.unrisen_entry_leaves_at_once = true)),
        Scenario::scripted(
            "order/gate-arrival-and-new-want",
            world().record(3, &[WANT_G]).step(4).record(4, &["arrival G G.1", "want B parameters source:host required"]).step(5).record(5, &["arrival B B.1"]),
        )
        .control("the gate counts a want of the pending record", core(|rules| rules.gate_counts_added_wants = true)),
        Scenario::scripted(
            "order/gate-pending-despawn",
            world().record(3, &["want G parameters entity:E1 required"]).step(4).record(4, &["despawn E1"]).step(4).record(4, &["arrival G G.1"]),
        )
        .control("the gate sees a despawn of the pending record", core(|rules| rules.gate_sees_despawns = true))
        .control(
            "a despawn of a record ends its entity's wants at the tick's structural change point",
            core(|rules| rules.despawn_ends_wants_at_the_structural_point = true),
        ),
    ];
    FamilyPlan {
        name: "order",
        claim: "the order at a tick's input fixed point gives the stated states",
        family,
        scenarios: with_the_type_left_out(scenarios),
        oracle: include_str!("../oracles/order.txt"),
    }
}

pub fn owners(settings: &Settings) -> FamilyPlan {
    const CONTENTS: &[Content] =
        &[content("A", 1), content("B", 1), content("D1", 1), content("D2", 1), content("P", 1).listing(&["D1"]), content("P", 2).listing(&["D2"])];
    let family = Family::new(&["A", "B", "D1", "P", "D2"], &["A", "B", "P"], CONTENTS, &[], settings);
    let world = || WorldPlan::new(12);
    const BY_E1: &str = "want P parameters entity:E1 required";
    const BY_E2: &str = "want P parameters entity:E2 optional";
    let two_wants = |name: &str, wants: &[&'static str]| {
        Scenario::scripted(name, world().record(2, wants).record(3, &["arrival P P.1", "arrival D1 D1.1"]).record(6, &["despawn E1"]).record(9, &["despawn E2"]))
            .control("one want of the dependency per want of its parent", core(|rules| rules.dependency_want_per_parent_want = true))
            .control("the wants of dependencies recomputed with a batch alone", core(|rules| rules.dependency_wants_with_a_batch_alone = true))
    };
    let scenarios = vec![
        two_wants("owners/two-wants-first-order", &[BY_E1, BY_E2]),
        // In the first order the order of making is the total order: the
        // control on the order of making can differ in the second alone.
        two_wants("owners/two-wants-second-order", &[BY_E2, BY_E1])
            .control("the wants of an entry in the order of making", core(|rules| rules.wants_in_making_order = true)),
        Scenario::scripted(
            "owners/parent-changes-dependency",
            world().record(2, &[BY_E1]).record(3, &["arrival P P.1", "arrival D1 D1.1"]).record(6, &["arrival P P.2", "arrival D2 D2.1"]),
        )
        .control("the wants the core added left as they were when the parent's part changes", core(|rules| rules.core_wants_left = true)),
        Scenario::scripted(
            "owners/preload-over-composition",
            world()
                .wanting("A", Required)
                .record(1, &["arrival A A.1"])
                .record(3, &["want A parameters source:host optional", "want B parameters source:host optional"])
                .record(4, &["arrival B B.1"])
                .record(6, &["release A source:host", "release B source:host"]),
        )
        .control("one shared want under the world's own entry", core(|rules| rules.one_want_under_the_world = true))
        .control("the makers of the world's own entry ordered by identity alone, the kind ignored", core(|rules| rules.makers_by_identity_alone = true)),
        Scenario::scripted(
            "owners/one-maker-two-wants",
            world()
                .record(2, &["want A parameters source:host optional"])
                .record(3, &["arrival A A.1"])
                .record(5, &["want A parameters source:host required"])
                .record(7, &["release A source:host"]),
        )
        .control("a second want added for a maker that already holds one", core(|rules| rules.second_want_added = true)),
    ];
    FamilyPlan { name: "owners", claim: "a want's owner orders the table", family, scenarios: with_the_type_left_out(scenarios), oracle: include_str!("../oracles/owners.txt") }
}

pub fn named_set(settings: &Settings) -> FamilyPlan {
    const CONTENTS: &[Content] = &[
        content("K1", 1),
        content("K1", 2),
        content("K2", 1),
        content("K2", 9),
        content("N", 1),
        content("X2", 1),
        content("X2", 2),
        content("X3", 1),
        content("X3", 2),
        content("Y", 1),
        content("Y", 9),
    ];
    const SET: &[&str] = &["K1.1", "K2.1", "X2.1", "X3.1", "Y.1"];
    let family = Family::new(&["K1", "K2", "N", "X2", "X3", "Y"], &["K1", "K2", "X2", "Y"], CONTENTS, &[], settings);
    let composition = |to: u32, name: &str| WorldPlan::new(to).named(name).wanting("K1", Required).wanting("K2", Required);
    let world = |name: &str| {
        composition(22, name)
            .record(9, &["want X2 parameters source:host required"])
            .record(11, &["want Y parameters source:host required"])
            .record(13, &["want N parameters source:host required"])
            .record(15, &["want X3 parameters source:host required"])
    };
    let moved = Scenario::live("named-set/moved-manifest", SET, world("under-set").under(SetName::First))
        .also(world("no-set"))
        .also(world("reference").under(SetName::First).on(1))
        .moving(4, Move::Revision(vec!["K1.2", "X2.2", "X3.2", "N.1"]))
        .moving(6, Move::Remove("X3.1"))
        .moving(7, Move::Mount(vec!["K2.9", "Y.9"]))
        .moving(19, Move::Unmount)
        .checking(Check::SameState("under-set", "reference"))
        .control("the instance's manifest read in place of the set's", loader(|options| options.current_manifest_for_sets = true));
    let recording = Scenario::live("named-set/recording-replays-under-a-set", SET, composition(8, "world").record(4, &["want X2 parameters source:host optional"]))
        .also(composition(8, "replay").under(SetName::First).replaying("world"))
        .checking(Check::SameState("world", "replay"))
        .control("the set's hash placed under the state hash", Change::Harness(Tweak::SetUnderTheHash));
    FamilyPlan {
        name: "named-set",
        claim: "a world under a named content set holds it under a moved manifest",
        family,
        scenarios: with_the_type_left_out(vec![moved, recording]),
        oracle: include_str!("../oracles/named-set.txt"),
    }
}

pub fn restore(settings: &Settings) -> FamilyPlan {
    // `M` is the one more asset the saved world never wanted: the family
    // names none, and it stands last in identifier order.
    const CONTENTS: &[Content] = &[
        content("D1", 1),
        content("D2", 1),
        content("F", 1),
        content("L1", 1),
        content("L1", 2),
        content("L2", 1),
        content("P", 1).listing(&["D1"]),
        content("P", 2).listing(&["D2"]),
        content("T", 1).of(Second),
        content("U", 1),
        content("U", 2),
        content("Z", 1),
        content("Z", 2),
        content("M", 1),
    ];
    const SAVED: &[&str] = &["D1.1", "D2.1", "F.1", "L1.1", "L2.1", "P.1", "T.1", "U.1", "Z.1"];
    const SAVER: &str = "restore/saved-world";
    const RECORD_OF_TICK_8: &[&str] = &["release Z source:host", "want U parameters source:host required"];
    const UNANSWERED_LEFT_OUT: &str = "a want the save holds unanswered left out of what starts a new history";
    const NO_WANT_LEFT: &str = "an entry that holds no want left unanswered at a restore";
    let family = Family::new(&["D1", "D2", "F", "L1", "L2", "P", "T", "U", "Z", "M"], &["L1", "L2", "P", "U"], CONTENTS, &[("one", Required, true)], settings);
    let composition = || WorldPlan::new(16).wanting("L2", Required).wanting("P", Required);
    let saved_world = Scenario::live(
        SAVER,
        SAVED,
        composition()
            .record(
                2,
                &[
                    "instantiate one L1 parameters required reload on",
                    "want Z parameters source:host optional",
                    "want F parameters source:host optional",
                    "want T second source:host optional",
                ],
            )
            .record(8, RECORD_OF_TICK_8)
            .saved(&[5, 8]),
    )
    .cut(&["F.1"]);
    // A reader of the save, under the content at hand, into the saved
    // world's composition, naming no set, but where its scenario says otherwise.
    let reader = |name: &str, contents: &[&'static str], world: WorldPlan| Scenario::live(name, contents, world).cut(&["F.1"]).reading(SAVER, 8, false, &[]);
    let unanswered_left_out = || core(|rules| rules.save_with_an_unanswered_want_continues = true);
    let no_want_left = || core(|rules| rules.restore_leaves_entries_without_a_want = true);
    let first_type_alone = || composition().declaring(&[Parameters]);
    let scenarios = vec![
        saved_world,
        reader("restore/earlier-save-continues", &but(SAVED, &["M.1"]), composition().under(SetName::AtHand).record(8, RECORD_OF_TICK_8))
            .reading(SAVER, 5, false, &[])
            .checking(Check::ContinuesTheSavedRun)
            .control("the hash of the content set under the state hash", Change::Harness(Tweak::SetUnderTheHash)),
        reader("restore/same-content-other-set", &but(SAVED, &["M.1"]), composition().under(SetName::AtHand)).control(UNANSWERED_LEFT_OUT, unanswered_left_out()),
        reader("restore/one-changed", &but(SAVED, &["L1.2"]), composition())
            .control("the answers to the restore posted unmarked", loader(|options| options.answers_unmarked = true)),
        reader("restore/one-absent", &without(SAVED, "L1"), composition()).control(
            "a marked answer that fails treated as a marked answer to a hash, with no event and the instance left as it was",
            core(|rules| rules.marked_failure_emits_nothing = true),
        ),
        Scenario::live("restore/entries-not-available", SAVED, composition().record(11, &["want F parameters source:tool optional"]))
            .reading(SAVER, 8, false, &[])
            .control("the failed entry answered at the restore", Change::Harness(Tweak::AnswerTheFailedEntry("F"))),
        reader("restore/unanswered-at-another-hash", &but(SAVED, &["U.2"]), composition()).control(UNANSWERED_LEFT_OUT, unanswered_left_out()),
        reader("restore/revision-while-waiting", SAVED, composition()).moving(9, Move::Revision(vec!["L1.2"])).control(
            "a revision posted to a world that waits for its answers, which replaces a marked answer in the queue",
            loader(|options| options.revisions_not_held = true),
        ),
        reader("restore/parent-with-another-dependency", &but(SAVED, &["P.2"]), composition()).control(
            "answers posted for the assets the saved table holds and for no other",
            loader(|options| options.restore_answers_do_not_follow_the_closure = true),
        ),
        reader("restore/no-want-entry-wanted-again", &but(SAVED, &["Z.2"]), composition().record(9, &["want Z parameters source:tool optional"]))
            .control(NO_WANT_LEFT, no_want_left()),
        reader("restore/no-want-entry-changed", &but(SAVED, &["Z.2"]), composition()).control(NO_WANT_LEFT, no_want_left()),
        reader("restore/recorded-content-continues", SAVED, composition())
            .reading(SAVER, 8, true, &[])
            .checking(Check::ContinuesTheSavedRun)
            .control("the confirmation made by marked transitions of the first tick's record", Change::Harness(Tweak::AsContentAtHand)),
        reader("restore/recorded-content-refused", &but(SAVED, &["Z.2"]), composition())
            .reading(SAVER, 8, true, &[])
            .control("an entry that holds no want left unconfirmed", no_want_left()),
        reader("restore/another-composition-refused", SAVED, first_type_alone())
            .control("the entries of a type the composition lacks dropped in silence", Change::Harness(Tweak::DropInSilence)),
        reader("restore/another-composition-dropped", SAVED, first_type_alone())
            .reading(SAVER, 8, false, &[Second])
            .control("the event that records the restore names no dropped entry", Change::Harness(Tweak::DropInSilence)),
        reader("restore/another-composition-asset-absent", &without(SAVED, "T"), first_type_alone())
            .control("the type of an entry read from the manifest at hand", Change::Harness(Tweak::TypesFromTheManifest)),
        reader("restore/drop-names-a-type-with-no-entry", SAVED, first_type_alone()).reading(SAVER, 8, false, &[Second, Third]).control(
            "a type named as dropped, of which the table holds no entry, refuses the restore",
            core(|rules| rules.dropped_type_without_an_entry_refuses = true),
        ),
        reader("restore/drop-names-a-declared-type", SAVED, first_type_alone()).reading(SAVER, 8, false, &[Second, Parameters]).control(
            "a type named as dropped that the composition has drops nothing and refuses nothing",
            core(|rules| rules.dropped_declared_type_ignored = true),
        ),
        reader("restore/drop-names-a-declared-type-with-no-entry", SAVED, composition().declaring(&[Parameters, Third]))
            .reading(SAVER, 8, false, &[Second, Third])
            .control(
                "a type named as dropped that the composition has, and of which the table holds no entry, refuses nothing",
                core(|rules| rules.dropped_declared_type_without_an_entry_refuses_nothing = true),
            ),
    ];
    FamilyPlan {
        name: "restore",
        claim: "a save read in another process gives the stated worlds",
        family,
        scenarios: with_the_type_left_out(scenarios),
        oracle: include_str!("../oracles/restore.txt"),
    }
}

pub fn retry(settings: &Settings) -> FamilyPlan {
    const CONTENTS: &[Content] = &[content("A", 1), content("D", 1), content("F", 1), content("P", 1).listing(&["D"])];
    const ALL: &[&str] = &["A.1", "D.1", "F.1", "P.1"];
    const CUT: &[&str] = &["D.1", "F.1"];
    const SAVER: &str = "retry/required-want";
    const WANT_F: &str = "want F parameters source:host optional";
    let family = Family::new(&["A", "D", "F", "P"], &["A", "F", "P"], CONTENTS, &[], settings);
    let world = || WorldPlan::new(12).wanting("A", Required);
    let repaired = |name: &str, world: WorldPlan| Scenario::live(name, ALL, world).cut(CUT).moving(5, Move::Repair(CUT.to_vec()));
    let stays_failed = || core(|rules| rules.failed_entry_stays_failed_on_a_want = true);
    let scenarios = vec![
        repaired("retry/optional-want", world().record(2, &[WANT_F]).record(6, &["want F parameters source:tool optional"])).control(STAYS_FAILED, stays_failed()),
        repaired(SAVER, world().record(2, &[WANT_F]).record(6, &["want F parameters source:tool required"]).step_unposted(7).saved(&[6])).control(STAYS_FAILED, stays_failed()),
        repaired("retry/same-maker-changes-policy", world().record(2, &[WANT_F]).record(6, &["want F parameters source:host required"]).step(7)).control(
            "a want its maker already holds, made again with another policy, counted as a want added",
            core(|rules| rules.policy_change_counts_as_a_want_added = true),
        ),
        repaired("retry/release-and-want-again", world().record(2, &[WANT_F]).record(6, &["release F source:host", WANT_F])).control(STAYS_FAILED, stays_failed()),
        Scenario::live("retry/want-in-the-record-of-the-failure", ALL, world().record(2, &[WANT_F]).record(3, &["want F parameters source:tool optional"]))
            .cut(CUT)
            .control("an arrival that leaves an entry failed for the same reason emits its event", core(|rules| rules.repeated_failure_emits = true)),
        repaired(
            "retry/dependency-of-a-parent-wanted-again",
            world().record(2, &["want P parameters source:host optional"]).record(6, &["release P source:host"]).record(7, &["want P parameters source:host optional"]),
        )
        .control("a want the core adds on a failed entry leaves it failed", core(|rules| rules.core_want_leaves_a_failed_entry_failed = true)),
        Scenario::live("retry/save-under-the-content-at-hand", ALL, world()).reading(SAVER, 6, false, &[]).control(STAYS_FAILED, stays_failed()),
        Scenario::live("retry/save-under-the-recorded-content", ALL, world())
            .reading(SAVER, 6, true, &[])
            .checking(Check::ContinuesTheSavedRun)
            .control(STAYS_FAILED, stays_failed()),
    ];
    FamilyPlan {
        name: "retry",
        claim: "a want added on a failed entry is a new load",
        family,
        scenarios: with_the_type_left_out(scenarios),
        oracle: include_str!("../oracles/retry.txt"),
    }
}

pub fn types(settings: &Settings) -> FamilyPlan {
    // `Q.1` is the part the stand-in pipeline refuses to write: it lists `D`
    // under the second type, where the manifest holds `D` under the first.
    const CONTENTS: &[Content] = &[
        content("A", 1),
        content("D", 1),
        content("F", 1),
        content("K", 1).listing(&["S"]),
        content("P", 1).listing(&["D"]),
        content("Q", 1).listing(&["D"]).listed_as(&[("D", Second)]),
        content("S", 1).of(Second),
        content("W", 1).of(Second).listing(&["D"]),
    ];
    const ALL: &[&str] = &["A.1", "D.1", "F.1", "K.1", "P.1", "Q.1", "S.1", "W.1"];
    const DEPENDENCY_SAVER: &str = "types/dependency-of-another-type";
    const PARENT_SAVER: &str = "types/parent-of-the-second-type";
    const PARENTS_TYPE: &str = "the entry of a dependency takes the type its parent's entry holds";
    let family = Family::new(&["A", "D", "F", "K", "P", "Q", "S", "W"], &["A", "K", "P"], CONTENTS, &[], settings);
    let world = || WorldPlan::new(12);
    let live = |name: &str, world: WorldPlan| Scenario::live(name, ALL, world).cut(&["F.1"]);
    let parents_type = || core(|rules| rules.dependency_takes_its_parents_type = true);
    let dependency = |name: &str| world().named(name).record(2, &["want K parameters source:host required"]).record(6, &["want S parameters source:tool optional"]);
    let scenarios = vec![
        live(
            "types/want-of-another-type",
            world().record(2, &["want A parameters source:host optional", "want A second source:tool optional"]).record(5, &["want A second entity:E1 required"]),
        )
        .control("a want that states another type than its entry holds added as any want", core(|rules| rules.types_not_checked = true)),
        live(
            "types/wrong-first-want",
            world()
                .record(2, &["want A second source:host optional"])
                .record(3, &["want A parameters source:tool optional"])
                .record(5, &["want A parameters source:tool optional"]),
        )
        .control(
            "the type a first want wrote kept for good, a later want of another type being refused whatever the entry's availability",
            core(|rules| rules.first_type_holds = true),
        ),
        live(
            "types/wrong-first-want-corrected-by-its-maker",
            world().record(2, &["want A second source:host optional"]).record(5, &["want A parameters source:host optional"]),
        )
        .control(
            "a want its maker already holds, made again with another type, adds no want and is no new load",
            core(|rules| rules.type_corrected_by_a_new_maker_alone = true),
        ),
        live("types/failed-for-another-reason", world().record(2, &["want F parameters source:host optional"]).record(5, &["want F second source:tool optional"]))
            .control("a want of another type replaces the type of an entry failed for any reason", core(|rules| rules.any_failure_lets_a_type_be_replaced = true)),
        live(DEPENDENCY_SAVER, dependency("world").saved(&[4]))
            .also(dependency("other-order").declaring(&[Second, Parameters]).on(1).unjudged())
            .checking(Check::SameState("world", "other-order"))
            .control(PARENTS_TYPE, parents_type()),
        live(
            "types/core-want-on-a-wrongly-typed-entry",
            world()
                .record(2, &["want D second source:host optional"])
                .record(5, &["want P parameters source:tool required"])
                .step_unposted(6)
                .record(8, &["want D parameters source:tool optional"])
                .step_unposted(9),
        )
        .control(
            "a want the core adds states the type its parent's part lists and is refused where the entry holds another",
            core(|rules| rules.core_wants_are_refused_for_their_type = true),
        ),
        live("types/part-lists-a-contradicted-type", world().record(2, &["want Q parameters source:host required"]))
            .control("the dependencies a part lists not checked against the manifest", loader(|options| options.dependencies_not_checked = true)),
        live(PARENT_SAVER, world().record(2, &["want W second source:host optional"]).saved(&[4])),
        live("types/drop-not-named", world().declaring(&[Parameters])).reading(DEPENDENCY_SAVER, 4, false, &[]).control(PARENTS_TYPE, parents_type()),
        live("types/drop-refused-for-a-kept-parent", world().declaring(&[Parameters])).reading(DEPENDENCY_SAVER, 4, false, &[Second]).control(
            "a type dropped whatever kept entry lists a dependency of it",
            core(|rules| rules.type_dropped_under_a_kept_parent = true),
        ),
        live("types/dropped-parent-of-a-kept-dependency", world().declaring(&[Parameters]))
            .reading(PARENT_SAVER, 4, false, &[Second])
            .control("the want a dropped parent owned left on its dependency", core(|rules| rules.dropped_parents_wants_left = true)),
    ];
    FamilyPlan { name: "types", claim: "an entry holds the asset type of its asset", family, scenarios: with_the_type_left_out(scenarios), oracle: include_str!("../oracles/types.txt") }
}

pub fn dependencies(settings: &Settings) -> FamilyPlan {
    const CONTENTS: &[Content] = &[content("D", 1), content("P", 1).listing(&["D"])];
    const ALL: &[&str] = &["D.1", "P.1"];
    let family = Family::new(&["D", "P"], &["P"], CONTENTS, &[], settings);
    let world = || WorldPlan::new(12);
    let batch_alone = || core(|rules| rules.dependency_wants_with_a_batch_alone = true);
    let scenarios = vec![
        Scenario::live(
            "dependencies/parent-wanted-again-at-its-fall",
            ALL,
            world().record(2, &["want P parameters source:host required"]).record(5, &["release P source:host"]).record(6, &["want P parameters source:tool optional"]),
        )
        .control(BATCH_ALONE, batch_alone()),
        Scenario::live(
            "dependencies/required-want-on-a-parent-in-force",
            ALL,
            world().record(2, &["want P parameters source:host optional"]).record(5, &["want P parameters entity:E1 required"]).record(7, &["despawn E1"]),
        )
        .control(BATCH_ALONE, batch_alone()),
        Scenario::live(
            "dependencies/closure-over-an-available-dependency",
            ALL,
            world().record(2, &["want D parameters source:host optional"]).record(5, &["want P parameters source:tool required"]).step_unposted(6),
        )
        .control(
            "the batch of a want's closure holds an arrival for every asset of the closure, one the table holds available included",
            loader(|options| options.closure_posts_assets_in_force = true),
        ),
    ];
    FamilyPlan {
        name: "dependencies",
        claim: "the wants of dependencies follow their parents where no batch brings them",
        family,
        scenarios: with_the_type_left_out(scenarios),
        oracle: include_str!("../oracles/dependencies.txt"),
    }
}

pub fn revision(settings: &Settings) -> FamilyPlan {
    const CONTENTS: &[Content] = &[
        content("D", 1),
        content("D", 2),
        content("N", 1),
        content("P", 1).listing(&["D", "V"]),
        content("P", 2).listing(&["D", "N", "V"]),
        content("P", 3).listing(&["N"]),
        content("V", 1),
        content("V", 2),
        content("Z", 1),
        content("Z", 2),
    ];
    const FIRST: &[&str] = &["D.1", "N.1", "P.1", "V.1", "Z.1"];
    let family = Family::new(&["D", "N", "P", "V", "Z"], &["P", "Z"], CONTENTS, &[], settings);
    let world = || WorldPlan::new(12);
    let wanting_p = || world().record(2, &["want P parameters source:host required"]);
    let released_z = || world().record(2, &["want Z parameters source:host optional"]).record(5, &["release Z source:host"]);
    let scenarios = vec![
        Scenario::live("revision/parent-gains-a-dependency", FIRST, wanting_p()).moving(6, Move::Revision(vec!["P.2"])).control(
            "the batch of a revision holds an arrival for each asset the revision changes that the table names, and for no other",
            loader(|options| options.moves_without_the_closure = true),
        ),
        Scenario::live("revision/former-dependencies-changed-with-their-parent", FIRST, wanting_p()).moving(6, Move::Revision(vec!["D.2", "V.2", "P.3"])).control(
            "the wants of the dependencies a new part no longer lists ended before any arrival of the batch is applied",
            core(|rules| rules.former_dependency_wants_end_before_the_batch = true),
        )
        .control("the wants of former dependencies end at their parent's arrival", core(|rules| rules.core_wants_recomputed_at_each_arrival = true)),
        Scenario::live("revision/entry-with-no-want-wanted-again", FIRST, released_z().record(6, &["want Z parameters source:tool optional"]))
            .moving(6, Move::Revision(vec!["Z.2"]))
            .control(
                "the batch of a revision holds no arrival for an entry that holds no want",
                loader(|options| options.moves_leave_entries_without_a_want = true),
            ),
        Scenario::live("revision/entry-with-no-want-left", FIRST, released_z())
            .moving(6, Move::Revision(vec!["Z.2"]))
            .control("an arrival applied to an entry that no want names", core(|rules| rules.unnamed_arrivals_applied = true)),
    ];
    FamilyPlan {
        name: "revision",
        claim: "the batch of a revision holds what the revision changes and its closure",
        family,
        scenarios: with_the_type_left_out(scenarios),
        oracle: include_str!("../oracles/revision.txt"),
    }
}

pub fn pending_parent(settings: &Settings) -> FamilyPlan {
    const CONTENTS: &[Content] = &[content("D", 1), content("G", 1), content("P", 1).listing(&["D"])];
    const WANT_G: &str = "want G parameters source:host required";
    let family = Family::new(&["D", "G", "P"], &["G", "P"], CONTENTS, &[], settings);
    let world = || WorldPlan::new(10);
    let scenarios = vec![
        Scenario::scripted(
            "pending-parent/required",
            world().record(3, &["want P parameters source:host required"]).step(4).record(4, &["arrival P P.1"]).step(4).record(4, &["arrival D D.1"]),
        )
        .control("the gate leaves out the want the core would add from the part of a pending arrival", core(|rules| rules.gate_leaves_out_core_wants = true))
        .control("the step names what the state holds unanswered", core(|rules| rules.step_names_what_the_state_holds_unanswered = true)),
        Scenario::scripted(
            "pending-parent/optional",
            world()
                .record(3, &["want P parameters source:host optional", WANT_G])
                .step(4)
                .record(4, &["arrival G G.1", "arrival P P.1"])
                .step(4)
                .record(6, &["arrival D D.1"]),
        )
        .control(
            "the gate counts the want the core would add as required, whatever the wants of its parent",
            core(|rules| rules.gate_counts_core_wants_as_required = true),
        ),
        Scenario::scripted(
            "pending-parent/pending-want-left-out",
            world()
                .record(3, &["want P parameters source:host optional", WANT_G])
                .step(4)
                .record(4, &["arrival G G.1", "arrival P P.1", "want P parameters source:tool required"])
                .step(4)
                .step(5)
                .record(5, &["arrival D D.1"]),
        )
        .control(
            "a pending command that adds a want on the parent counted in the policy of the want the core would add",
            core(|rules| rules.gate_counts_added_wants = true),
        ),
        Scenario::scripted(
            "pending-parent/pending-release-applied",
            world()
                .record(3, &["want P parameters source:host required", "want P parameters source:tool optional", WANT_G])
                .step(4)
                .record(4, &["release P source:host", "arrival G G.1", "arrival P P.1"])
                .step(4)
                .record(6, &["arrival D D.1"]),
        )
        .control(
            "a pending command that releases a want of the parent left out of the policy of the want the core would add",
            core(|rules| rules.gate_leaves_out_releases_for_core_wants = true),
        ),
    ];
    FamilyPlan {
        name: "pending-parent",
        claim: "the gate counts the want the core would add from a pending parent",
        family,
        scenarios: with_the_type_left_out(scenarios),
        oracle: include_str!("../oracles/pending-parent.txt"),
    }
}

pub fn record(settings: &Settings) -> FamilyPlan {
    const CONTENTS: &[Content] = &[content("A", 1)];
    let family = Family::new(&["A", "F"], &["A"], CONTENTS, &[], settings);
    let world = || WorldPlan::new(10);
    let scenarios = vec![
        Scenario::scripted(
            "record/arrival-that-changes-nothing",
            world()
                .record(2, &["want A parameters source:host optional", "want F parameters source:host optional"])
                .record(3, &["arrival A A.1", "arrival F failed invalid-part"])
                .record(5, &["arrival A A.1", "arrival F failed invalid-part"])
                .record(7, &["arrival F failed artifact-gone"]),
        )
        .control("an arrival that leaves an entry as it was emits its event", core(|rules| rules.unchanged_arrival_emits = true)),
        Scenario::scripted(
            "record/want-after-the-despawn-of-its-owner",
            world().record(2, &["despawn E1", "want A parameters entity:E1 required"]).record(3, &["arrival A A.1"]),
        )
        .control(
            "a despawn that stands in a record applied at the structural change point of its tick",
            core(|rules| rules.despawn_ends_wants_at_the_structural_point = true),
        ),
        Scenario::scripted(
            "record/two-makers-of-one-identity",
            world()
                .wanting("A", Required)
                .record(1, &["arrival A A.1"])
                .record(3, &["want A parameters source:twin optional"])
                .record(5, &["release A source:twin"]),
        )
        .control("a want of the world's own entry carries the identity of its maker without its kind", core(|rules| rules.makers_known_by_identity_alone = true)),
    ];
    FamilyPlan {
        name: "record",
        claim: "an element that changes nothing changes nothing, and a maker is known by its kind and its identity",
        family,
        scenarios: with_the_type_left_out(scenarios),
        oracle: include_str!("../oracles/record.txt"),
    }
}
