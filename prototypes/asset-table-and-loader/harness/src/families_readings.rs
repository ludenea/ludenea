//! The families of the stand-in sheet that hold the readings taken which no
//! run had taken, judged on states, as data: the gate and the wants of the
//! first step, the unit of a rejection over a world's budget, the maker of a
//! system's want, the type a want of the core takes and the batch of a
//! restore. Each scenario is the sheet's statement in its words, and each
//! control the rule it changes. No oracle is read here. Throwaway, never
//! product code.

use prototype_core::table::Policy::{Optional, Required};

use crate::families::{core, loader};
use crate::play::{Change, Check, FamilyPlan, Move, Scenario, WorldPlan};
use crate::run::{Content, Family, Settings, content};

// ---------------------------------------------------------------------------
// The gate and the wants of the first step
// ---------------------------------------------------------------------------
//
// In the four families of this part a scenario may ask for several steps
// without waiting in a row, and an element joins the pending record at a
// point of the scenario, whatever tick that record is of. `record(n, ..)` is
// "record of tick n"; `step_in_a_row(n)` and `joins(n, ..)` are what the
// scenario states once tick `n` has run, in the order stated. "And the world
// steps" is the step of the round, as in the earlier sheets.

pub fn pending_ends(settings: &Settings) -> FamilyPlan {
    const CONTENTS: &[Content] = &[content("D", 1), content("G", 1), content("P", 1).listing(&["D"]), content("P", 2)];
    const WHOLE_RECORD: &str =
        "the gate is evaluated on the table with the whole pending record applied, a command that adds a want excepted, so that it sees the wants a pending element ends";
    let family = Family::new(&["D", "G", "P"], &["G", "P"], CONTENTS, &[], settings);
    let world = || WorldPlan::new(8);
    let whole_record = || core(|rules| rules.gate_copies_the_table = true);
    let policy_changes = || core(|rules| rules.gate_sees_pending_policy_changes = true);
    // The opening of the four scenarios on a parent in force, the element
    // under test, and what follows it.
    let on_a_parent_in_force = |name: &str, under_test: &'static str| {
        Scenario::scripted(
            name,
            world()
                .record(2, &["want P parameters source:host optional"])
                .record(3, &["arrival P P.1"])
                .record(4, &["want P parameters source:tool required"])
                .step_in_a_row(4)
                .joins(4, &[under_test])
                .step_in_a_row(4)
                .joins(4, &["arrival D D.1"]),
        )
    };
    let scenarios = vec![
        on_a_parent_in_force("pending-ends/release-of-the-last-required-want", "release P source:tool").control(WHOLE_RECORD, whole_record()),
        on_a_parent_in_force("pending-ends/failure-of-the-parent", "arrival P failed unknown-asset").control(WHOLE_RECORD, whole_record()),
        on_a_parent_in_force("pending-ends/replacement-that-drops-the-dependency", "arrival P P.2").control(WHOLE_RECORD, whole_record()),
        on_a_parent_in_force("pending-ends/policy-change-of-the-parent", "want P parameters source:tool optional").control(
            "the gate applies a pending command that changes the policy of a want, with the recomputation of the wants of its asset's dependencies",
            core(|rules| {
                rules.gate_copies_the_table = true;
                rules.gate_sees_pending_policy_changes = true;
            }),
        ),
        Scenario::scripted(
            "pending-ends/policy-change-on-the-required-asset",
            world()
                .record(2, &["want G parameters source:host required"])
                .step_in_a_row(2)
                .joins(2, &["want G parameters source:host optional"])
                .step_in_a_row(2)
                .joins(2, &["arrival G G.1"]),
        )
        .control("the gate applies a pending command that changes the policy of a want", policy_changes()),
        Scenario::scripted(
            "pending-ends/policy-change-of-a-pending-parent",
            world()
                .record(2, &["want P parameters source:host required"])
                .step_in_a_row(2)
                .joins(2, &["want P parameters source:host optional", "arrival P P.1"])
                .step_in_a_row(2)
                .joins(2, &["arrival D D.1"]),
        )
        .control(
            "the gate applies a pending command that changes the policy of a want, in the policy of the want the core would add from a pending part as elsewhere",
            policy_changes(),
        ),
    ];
    FamilyPlan {
        name: "pending-ends",
        claim: "the gate does not see the wants a pending element ends",
        family,
        scenarios,
        oracle: include_str!("../oracles/pending-ends.txt"),
    }
}

pub fn incomplete_batch(settings: &Settings) -> FamilyPlan {
    const CONTENTS: &[Content] = &[content("D", 1), content("G", 1), content("N", 1), content("P", 1).listing(&["D"]), content("P", 2).listing(&["D", "N"])];
    let family = Family::new(&["D", "G", "N", "P"], &["G", "P"], CONTENTS, &[], settings);
    // Both scenarios open alike.
    let opening = || WorldPlan::new(8).record(2, &["want P parameters source:host required"]).record(3, &["arrival D D.1", "arrival P P.1"]);
    let scenarios = vec![
        Scenario::scripted(
            "incomplete-batch/running-world",
            opening().joins(4, &["arrival P P.2"]).step_in_a_row(4).step_in_a_row(4).joins(4, &["arrival N N.1"]),
        )
        .control(
            "the gate is evaluated on the pending record at every boundary, on a running world as on a gated one",
            core(|rules| rules.gate_read_on_a_world_that_is_not_gated = true),
        )
        .control(
            "a required want that the core adds from the part of an applied arrival gates no boundary",
            core(|rules| rules.core_wants_gate_no_boundary = true),
        ),
        Scenario::scripted(
            "incomplete-batch/gated-world",
            opening()
                .record(4, &["want G parameters source:host required"])
                .step_in_a_row(4)
                .joins(4, &["arrival P P.2"])
                .step_in_a_row(4)
                .joins(4, &["arrival G G.1"])
                .step_in_a_row(4)
                .joins(4, &["arrival N N.1"]),
        )
        .control(
            "the gate leaves out the want the core would add from the part of a pending arrival",
            core(|rules| rules.gate_leaves_out_core_wants = true),
        ),
    ];
    FamilyPlan {
        name: "incomplete-batch",
        claim: "the gate reads a pending record on a gated world alone",
        family,
        scenarios,
        oracle: include_str!("../oracles/incomplete-batch.txt"),
    }
}

pub fn first_step(settings: &Settings) -> FamilyPlan {
    const CONTENTS: &[Content] =
        &[content("K", 1), content("M", 1).listing(&["K"]), content("N", 1), content("P", 1), content("P", 2).listing(&["N"]), content("R", 1).listing(&["M"])];
    const EVERY_ARRIVAL: &str = "the wants of the first step are added from the part of every arrival of the record, whatever wants its asset holds";
    const RELEASE_P: &str = "release P source:host";
    let family = Family::new(&["K", "M", "N", "P", "R"], &["P", "R"], CONTENTS, &[], settings);
    let every_arrival = || core(|rules| rules.first_step_follows_every_arriving_part = true);
    // The three scenarios on `P` open alike.
    let on_p = || WorldPlan::new(8).record(2, &["want P parameters source:host required"]).record(3, &["arrival P P.1"]);
    let scenarios = vec![
        Scenario::scripted("first-step/release-in-the-record-of-the-replacement", on_p().record(5, &[RELEASE_P, "arrival N N.1", "arrival P P.2"]))
            .control(EVERY_ARRIVAL, every_arrival()),
        Scenario::scripted("first-step/replacement-of-an-entry-with-no-want", on_p().record(5, &[RELEASE_P]).record(6, &["arrival N N.1", "arrival P P.2"]))
            .control(EVERY_ARRIVAL, every_arrival()),
        Scenario::scripted(
            "first-step/want-again-in-the-record-of-the-replacement",
            on_p().record(5, &[RELEASE_P]).record(6, &["want P parameters source:tool optional", "arrival N N.1", "arrival P P.2"]),
        )
        .control(
            "the wants of the first step start from an asset that held a want before the commands of the record were applied",
            core(|rules| rules.first_step_starts_from_the_wants_held_before_the_commands = true),
        ),
        Scenario::scripted(
            "first-step/root-fails-while-its-closure-arrives",
            WorldPlan::new(8)
                .record(2, &["want R parameters source:host required"])
                .step_in_a_row(2)
                .joins(2, &["arrival K K.1", "arrival M M.1", "arrival R failed invalid-part"])
                .record(5, &["want R parameters source:tool required"])
                .step_in_a_row(5)
                .joins(5, &["arrival K K.1", "arrival M M.1", "arrival R R.1"]),
        )
        .control(EVERY_ARRIVAL, every_arrival()),
    ];
    FamilyPlan {
        name: "first-step",
        claim: "the wants of the first step start from an asset that holds a want",
        family,
        scenarios,
        oracle: include_str!("../oracles/first-step.txt"),
    }
}

pub fn pending_release(settings: &Settings) -> FamilyPlan {
    const CONTENTS: &[Content] = &[content("D", 1), content("G", 1), content("P", 1).listing(&["D"])];
    const WHATEVER_IS_LEFT: &str = "a pending release answers the asset it names, whatever want is left on it";
    const G_BY_HOST: &str = "want G parameters source:host required";
    let family = Family::new(&["D", "G", "P"], &["G", "P"], CONTENTS, &[], settings);
    let world = || WorldPlan::new(8);
    let whatever_is_left = || core(|rules| rules.gate_takes_a_pending_release_as_an_answer = true);
    // A required want of the host beside an optional one of the tool, one
    // of them released while the world is gated, then the arrival.
    let beside_an_optional_one = |released: &'static str| {
        world()
            .record(2, &[G_BY_HOST, "want G parameters source:tool optional"])
            .step_in_a_row(2)
            .joins(2, &[released])
            .step_in_a_row(2)
            .joins(2, &["arrival G G.1"])
    };
    let scenarios = vec![
        Scenario::scripted(
            "pending-release/one-then-both",
            world()
                .record(2, &[G_BY_HOST, "want G parameters source:tool required"])
                .step_in_a_row(2)
                .joins(2, &["release G source:host"])
                .step_in_a_row(2)
                .joins(2, &["release G source:tool"])
                .step_in_a_row(2),
        )
        .control(WHATEVER_IS_LEFT, whatever_is_left()),
        Scenario::scripted("pending-release/last-required-want-beside-an-optional-one", beside_an_optional_one("release G source:host")).control(
            "a pending release answers a required asset only where it ends the last want of that asset, whatever its policy",
            core(|rules| rules.gate_release_answers_where_no_want_is_left = true),
        ),
        Scenario::scripted("pending-release/optional-want-released", beside_an_optional_one("release G source:tool")).control(WHATEVER_IS_LEFT, whatever_is_left()),
        Scenario::scripted(
            "pending-release/own-want-beside-the-want-of-a-parent",
            world()
                .record(2, &["want P parameters source:host optional"])
                .record(3, &["arrival P P.1"])
                .record(4, &["want P parameters source:tool required", "want D parameters source:host required"])
                .step_in_a_row(4)
                .joins(4, &["release D source:host"])
                .step_in_a_row(4)
                .joins(4, &["arrival D D.1"]),
        )
        .control(WHATEVER_IS_LEFT, whatever_is_left()),
    ];
    FamilyPlan {
        name: "pending-release",
        claim: "a pending release answers a required asset only where it ends its last required want",
        family,
        scenarios,
        oracle: include_str!("../oracles/pending-release.txt"),
    }
}

// ---------------------------------------------------------------------------
// The unit of a rejection over a world's budget
// ---------------------------------------------------------------------------
//
// Every scenario of this part runs under the unit of the design text, the
// group of a root, which is the build's default. Every content has the size
// the sheet gives it, which the domain manifest holds; the stand-in pipeline
// pads the part to it but where a family says otherwise.

/// The reading not taken on the order of the test, a control of the
/// scenarios the sheet names it for.
const IDENTIFIER_ORDER_ALONE: &str = "the groups are tested in the identifier order of their roots alone";

fn identifier_order_alone() -> Change {
    core(|rules| rules.budget_groups_in_identifier_order_alone = true)
}

pub fn group_order(settings: &Settings) -> FamilyPlan {
    const CONTENTS: &[Content] = &[
        content("A", 1).sized(1_000).listing(&["C"]),
        content("B", 1).sized(1_000),
        content("B", 2).sized(4_000),
        content("C", 1).sized(6_000),
        content("D", 1).sized(3_000),
        content("E", 1).sized(4_000),
        content("F", 1).sized(1_000),
        content("G", 1).sized(5_000),
        content("H", 1).sized(3_000),
        content("M", 1).sized(2_000),
        content("M", 2).sized(5_000),
        content("P", 1).sized(1_000).listing(&["M"]),
        content("P", 2).sized(4_000).listing(&["M"]),
        content("Q", 1).sized(1_000).listing(&["T"]),
        content("T", 1).sized(5_000),
    ];
    // The instance's manifest holds the first content of every asset.
    const FIRST: &[&str] = &["A.1", "B.1", "C.1", "D.1", "E.1", "F.1", "G.1", "H.1", "M.1", "P.1", "Q.1", "T.1"];
    // The later revision.
    const LATER: &[&str] = &["B.2", "M.2", "P.2"];
    let family = Family::new(&["A", "B", "C", "D", "E", "F", "G", "H", "M", "P", "Q", "T"], &["A", "E", "M", "Q"], CONTENTS, &[], settings);
    let world = |to: u32, bytes: u64| WorldPlan::new(to).with_a_budget(bytes);
    // A root that is optional in its own right and required as the
    // dependency of another root.
    let as_a_dependency = || world(12, 8_000).record(2, &["want P parameters source:host required"]).record(4, &["want B parameters source:host optional", "want M parameters source:host optional"]);
    let scenarios = vec![
        Scenario::live(
            "group-order/optional-root-first",
            FIRST,
            world(8, 10_000).record(3, &["want A parameters source:host optional", "want Q parameters source:host required"]),
        )
        .control(IDENTIFIER_ORDER_ALONE, identifier_order_alone()),
        Scenario::live(
            "group-order/policy-changed-in-the-record-of-the-arrivals",
            FIRST,
            world(8, 10_000)
                .record(3, &["want A parameters source:host required", "want Q parameters source:host optional"])
                .record(4, &["want A parameters source:host optional", "want Q parameters source:host required"]),
        )
        .control(IDENTIFIER_ORDER_ALONE, identifier_order_alone())
        .control(
            "a root holds a required want where it stood in the required set before the commands of the record were applied",
            core(|rules| rules.budget_required_roots_read_before_the_commands = true),
        ),
        Scenario::live("group-order/required-as-a-dependency", FIRST, as_a_dependency())
            .moving(8, Move::Revision(LATER.to_vec()))
            .control(IDENTIFIER_ORDER_ALONE, identifier_order_alone())
            .control(
                "a root holds a required want where a maker other than the core made one",
                core(|rules| rules.budget_required_roots_by_their_own_wants = true),
            ),
        Scenario::live("group-order/parent-made-optional-in-the-record", FIRST, as_a_dependency().record(8, &["want P parameters source:host optional"]))
            .moving(8, Move::Revision(LATER.to_vec()))
            .control(
                "a want the core added keeps its policy until the arrivals of the record are applied, whatever a command of that record does to the wants of its parent",
                core(|rules| rules.core_wants_keep_their_policy_until_the_arrivals = true),
            ),
        Scenario::live(
            "group-order/each-set-in-identifier-order",
            FIRST,
            world(8, 10_000).record(
                3,
                &[
                    "want H parameters source:host required",
                    "want G parameters source:host required",
                    "want F parameters source:host optional",
                    "want E parameters source:host required",
                    "want D parameters source:host optional",
                ],
            ),
        )
        .control(IDENTIFIER_ORDER_ALONE, identifier_order_alone())
        .control(
            "the groups of each of the two sets are tested in the order in which the wants of their roots were made",
            core(|rules| rules.budget_groups_in_the_order_their_wants_were_made = true),
        ),
    ];
    FamilyPlan {
        name: "group-order",
        claim: "the groups of the roots that hold a required want are tested before the others",
        family,
        scenarios,
        oracle: include_str!("../oracles/group-order.txt"),
    }
}

pub fn unit_sentences(settings: &Settings) -> FamilyPlan {
    const CONTENTS: &[Content] = &[
        content("G", 1).sized(3_000),
        content("G", 2).sized(8_000),
        content("G", 3).sized(4_000),
        content("J", 1).sized(5_000),
        content("J", 2).sized(3_000),
        content("N", 1).sized(6_000),
        content("T", 1).sized(2_000).listing(&["Y"]),
        content("T", 2).sized(3_000).listing(&["Y"]),
        content("X", 1).sized(6_000),
        content("Y", 1).sized(4_000),
        content("Y", 2).sized(3_000),
    ];
    // The instance's manifest holds the first content of every asset when a
    // scenario starts.
    const FIRST: &[&str] = &["G.1", "J.1", "N.1", "T.1", "X.1", "Y.1"];
    const WANT_X: &str = "want X parameters source:host required";
    let family = Family::new(&["G", "J", "N", "T", "X", "Y"], &["G", "J", "N", "X"], CONTENTS, &[], settings);
    // One world per scenario, with a budget of 10 000 bytes at creation; the
    // composition's contributor `base` wants `G` as required at tick 0.
    let world = |to: u32| WorldPlan::new(to).with_a_budget(10_000).wanting("G", Required);
    // A revision that holds `G.2` in place of `G.1` and does not hold `X`.
    let grows_g_without_x = || Move::RevisionWithout(vec!["G.2"], vec!["X"]);
    let scenarios = vec![
        Scenario::live("unit-sentences/room-of-an-entry-that-fails", FIRST, world(8).record(2, &[WANT_X]))
            .moving(6, grows_g_without_x())
            .control(
                "the count holds an entry in force that an arrival to failed of the same record fails",
                core(|rules| rules.budget_count_holds_the_failing_entries = true),
            ),
        Scenario::live("unit-sentences/failed-arrival-for-an-entry-no-want-names", FIRST, world(8).record(2, &[WANT_X]).record(6, &["release X source:host"]))
            .moving(6, grows_g_without_x())
            .control(
                "the room of an entry in force is free wherever the record holds an arrival to failed for it, whether or not that arrival fails it",
                core(|rules| rules.budget_count_leaves_out_every_entry_an_arrival_to_failed_names = true),
            ),
        Scenario::live(
            "unit-sentences/room-of-a-failed-parent",
            FIRST,
            world(10)
                .record(2, &["want T parameters source:host optional"])
                .record(5, &["want N parameters source:host optional"])
                .record(7, &["want N parameters source:tool optional"]),
        )
        .moving(6, Move::RevisionWithout(Vec::new(), vec!["T"]))
        .control(
            "the room of the dependencies of a parent that fails is free at the fixed point that fails it",
            core(|rules| rules.budget_count_leaves_out_the_dependencies_of_a_failing_parent = true),
        ),
        Scenario::live("unit-sentences/smaller-replacement-over-a-lowered-budget", FIRST, world(11).wanting("J", Required).record(3, &["budget 5000"]))
            .moving(6, Move::Revision(vec!["J.2"]))
            .moving(9, Move::Revision(vec!["G.3"]))
            .control(
                "every group is tested against the budget, one that does not raise the count included",
                core(|rules| rules.budget_tests_a_group_that_does_not_raise_the_count = true),
            )
            .control("the budget only reports, and every arrival is applied", core(|rules| rules.budget_only_reports = true)),
        Scenario::live("unit-sentences/group-that-leaves-the-count-as-it-was", FIRST, world(8).wanting("T", Required).record(3, &["budget 5000"]))
            .moving(6, Move::Revision(vec!["T.2", "Y.2"]))
            .control(
                "a group is accepted whatever the count only where it lowers the count",
                core(|rules| rules.budget_accepts_over_the_count_only_a_group_that_lowers_it = true),
            )
            .control(
                "each arrival of a group is judged alone, one that raises the count being rejected where the count stands over the budget",
                core(|rules| rules.budget_judges_each_arrival_of_a_group_alone = true),
            ),
        Scenario::live(
            "unit-sentences/budget-lowered-in-the-record-of-an-arrival",
            FIRST,
            world(6).record(3, &["want N parameters source:host optional"]).record(4, &["budget 8000"]),
        )
        .control(
            "a budget that a record changes is in force from the tick after it",
            core(|rules| rules.budget_event_in_force_from_the_next_tick = true),
        ),
    ];
    FamilyPlan {
        name: "unit-sentences",
        claim: "an entry that fails at the same fixed point leaves its room, and a group that does not raise the count is accepted",
        family,
        scenarios,
        oracle: include_str!("../oracles/unit-sentences.txt"),
    }
}

pub fn sizes(settings: &Settings) -> FamilyPlan {
    // For `H.1` the stand-in pipeline writes a size of 4 000 bytes into the
    // domain manifest and pads the part to 1 000 bytes: the difference
    // stands in for what a prepared form built once adds to its part.
    const CONTENTS: &[Content] = &[
        content("H", 1).sized(1_000).held_as(4_000),
        content("K", 1).sized(5_000),
        content("K", 2).sized(7_000),
        content("K", 3).sized(3_000),
        content("N", 1).sized(2_000),
        content("S", 1).sized(1_000),
        content("W", 1).sized(1_000),
    ];
    // The first revision. The later one holds `K.2` in place of `K.1`.
    const FIRST: &[&str] = &["H.1", "K.1", "N.1", "S.1", "W.1"];
    const SAVER: &str = "sizes/run-and-replay";
    const RECORD_OF_TICK_11: &[&str] = &["release S source:host", "want W parameters source:tool optional"];
    const RECORD_OF_TICK_13: &[&str] = &["want N parameters source:tool optional"];
    const SIZE_LEFT_OUT: &str = "the size left out of the canonical bytes of an entry and of an arrival";
    let family = Family::new(&["H", "K", "N", "S", "W"], &["H", "K", "N", "W"], CONTENTS, &[], settings);
    let size_left_out = || core(|rules| rules.size_left_out_of_the_bytes = true);
    // The composition: its contributor `base` wants `K` as required at tick 0.
    let composition = || WorldPlan::new(15).wanting("K", Required);
    let recorded = composition()
        .named("recorded")
        .with_a_budget(12_000)
        .record(2, &["want H parameters source:host optional", "want S parameters source:host required"])
        .record(4, &["budget 10000"])
        .record(5, &["want N parameters source:host optional"])
        .record(11, RECORD_OF_TICK_11)
        .record(13, RECORD_OF_TICK_13)
        .saved(&[9]);
    // A world that replays the input log of `recorded`, from its creation.
    let replayed = composition().named("replayed").with_a_budget(12_000).replaying_alone("recorded");
    let run_and_replay = Scenario::live(SAVER, FIRST, recorded)
        .also(replayed)
        .moving(8, Move::Revision(vec!["K.2"]))
        .checking(Check::SameState("recorded", "replayed"))
        .control_on(
            &["recorded"],
            "the count is the sum of the lengths of the parts in force, the size an arrival names left aside",
            core(|rules| rules.budget_counts_the_length_of_the_part = true),
        )
        .control_on(
            &["replayed"],
            "the replay applies every arrival to a hash that its log holds and derives no rejection",
            Change::Replay(|rules| rules.no_rejection_derived = true),
        )
        .control_on(
            &["replayed"],
            "the lowering of a budget is no element of the log, and the replay stands under the budget of its creation",
            core(|rules| rules.budget_events_left_out_of_the_log = true),
        )
        .control(SIZE_LEFT_OUT, size_left_out());
    // A reader of the save: into the saved world's composition, naming no
    // content set, stepped to tick 15. Its records of tick 11 and of tick 13
    // hold the commands that `recorded` applies at those ticks, and its
    // scenario gives it no budget.
    let reader = |name: &str, contents: &[&'static str], recorded_content: bool| {
        Scenario::live(name, contents, composition().record(11, RECORD_OF_TICK_11).record(13, RECORD_OF_TICK_13))
            .reading(SAVER, 9, recorded_content, &[])
            .of_the_world("recorded")
    };
    let scenarios = vec![
        run_and_replay,
        reader("sizes/restored-under-the-recorded-content", FIRST, true)
            .checking(Check::ContinuesTheSavedRun)
            .control(
                "the count of a restored table starts from nothing and holds the sizes of the arrivals applied since the restore alone",
                core(|rules| rules.restored_count_starts_from_nothing = true),
            )
            .control(
                "a restored world stands under the budget its saved world was created under",
                core(|rules| rules.save_holds_the_budget_of_creation = true),
            )
            .control(SIZE_LEFT_OUT, size_left_out()),
        reader("sizes/restored-at-a-smaller-hash", &["H.1", "K.3", "N.1", "S.1", "W.1"], false)
            .control(
                "a marked answer at another hash changes the hash of its entry and leaves the size the entry held",
                core(|rules| rules.marked_answer_leaves_the_size = true),
            )
            .control(SIZE_LEFT_OUT, size_left_out()),
    ];
    FamilyPlan {
        name: "sizes",
        claim: "the size an arrival names and an entry holds gives the count",
        family,
        scenarios,
        oracle: include_str!("../oracles/sizes.txt"),
    }
}

// ---------------------------------------------------------------------------
// The maker of a system's want, the type a want of the core takes, the batch
// of a restore
// ---------------------------------------------------------------------------
//
// None of the three families gives a size to its contents.

pub fn system_maker(settings: &Settings) -> FamilyPlan {
    const CONTENTS: &[Content] = &[content("A", 1), content("D", 1), content("F", 1), content("P", 1).listing(&["D"])];
    const LOGIC: &str = "contributor:logic";
    const BASE: &str = "contributor:base";
    const UNDER_THE_CONTRIBUTOR: &str = "the maker of a want a system makes under the world's own entry is the contributor that brings the system";
    let family = Family::new(&["A", "D", "F", "P"], &["A", "P"], CONTENTS, &[], settings);
    // The three systems stand in every schedule, in the order `wanter`,
    // `mender`, `keeper`; a system that a scenario does not name does nothing.
    let world = |to: u32| WorldPlan::new(to).scheduling(&[("wanter", LOGIC), ("mender", BASE), ("keeper", LOGIC)]);
    let under_the_contributor = || core(|rules| rules.system_wants_under_their_contributor = true);
    let scenarios = vec![
        Scenario::scripted(
            "system-maker/two-systems-one-asset",
            world(10)
                .system_of("wanter", LOGIC, 2, &["want A parameters world required"])
                .record(3, &["arrival A A.1"])
                .system_of("keeper", LOGIC, 4, &["want A parameters world optional"])
                .system_of("wanter", LOGIC, 6, &["release A world"])
                .system_of("keeper", LOGIC, 8, &["release A world"]),
        )
        .control(UNDER_THE_CONTRIBUTOR, under_the_contributor()),
        Scenario::scripted(
            "system-maker/system-beside-the-composition",
            world(9)
                .wanting_by(LOGIC, "P", Required)
                .record(1, &["arrival D D.1", "arrival P P.1"])
                .system_of("wanter", LOGIC, 3, &["want P parameters world optional"])
                .system_of("wanter", LOGIC, 6, &["release P world"]),
        )
        .control(UNDER_THE_CONTRIBUTOR, under_the_contributor()),
        Scenario::scripted(
            "system-maker/order-of-the-makers",
            world(6)
                .wanting("D", Optional)
                .wanting_by(LOGIC, "P", Required)
                .wanting_by(LOGIC, "D", Optional)
                .record(1, &["arrival D D.1", "arrival P P.1"])
                .record(
                    2,
                    &[
                        "want D parameters entity:E2 optional",
                        "want D parameters entity:E1 optional",
                        "want D parameters source:twin optional",
                        "want D parameters source:tool optional",
                        "want D parameters source:host optional",
                    ],
                )
                .system_of("wanter", LOGIC, 2, &["want D parameters world optional"])
                .system_of("mender", BASE, 2, &["want D parameters world required"])
                .system_of("keeper", LOGIC, 2, &["want D parameters world optional"])
                .system_of("mender", BASE, 4, &["release D world"])
                .record(5, &["release D source:tool"]),
        )
        .control(UNDER_THE_CONTRIBUTOR, under_the_contributor())
        .control(
            "the wants of systems stand after those of command sources",
            core(|rules| rules.system_wants_after_command_sources = true),
        )
        .control(
            "the wants of systems stand in the order of the identities of the contributors that bring them, then by name",
            core(|rules| rules.system_wants_ordered_by_their_contributor = true),
        ),
        // The content holds `F.1` and never moves; its artifact is cut short
        // when the scenario starts, and whole from before tick 2.
        Scenario::live(
            "system-maker/retry-by-a-system",
            &["F.1"],
            world(8)
                .wanting_by(LOGIC, "F", Required)
                .system_of("wanter", LOGIC, 3, &["want F parameters world optional"])
                .step_unposted(4)
                .system_of("wanter", LOGIC, 6, &["release F world"]),
        )
        .cut(&["F.1"])
        .moving(2, Move::Repair(vec!["F.1"]))
        .control(UNDER_THE_CONTRIBUTOR, under_the_contributor()),
    ];
    FamilyPlan {
        name: "system-maker",
        claim: "a system is the maker of the want it makes",
        family,
        scenarios,
        oracle: include_str!("../oracles/system-maker.txt"),
    }
}

pub fn core_want_type(settings: &Settings) -> FamilyPlan {
    const CONTENTS: &[Content] = &[content("D", 1), content("P", 1).listing(&["D"])];
    // The content holds `D.1` and `P.1` and never moves.
    const ALL: &[&str] = &["D.1", "P.1"];
    const WRONG_FIRST_WANT: &str = "want D second source:host optional";
    const WANT_P: &str = "want P parameters source:tool required";
    const THE_LETTER: &str =
        "a want the core adds states a type only where it is the dependency's first, and the loading capability fails every asset the manifest holds under another type than its entry";
    const STILL_FAILS: &str = "the core's want takes the type, and the loading capability, following the closure, still fails for its type a dependency whose entry the table holds under another type than the manifest's";
    let family = Family::new(&["D", "P"], &["P"], CONTENTS, &[], settings);
    // The composition declares the first and the second type and makes no want.
    let world = || WorldPlan::new(12);
    // The reading not taken: the core's want keeps the type that failed, and
    // the loading capability fails the dependency for its type anew.
    let the_letter = || {
        vec![core(|rules| rules.core_want_keeps_the_type_of_a_failed_entry = true), loader(|options| options.dependency_failed_for_its_type_fails_anew = true)]
    };
    let still_fails = || loader(|options| options.dependency_failed_for_its_type_fails_anew = true);
    let scenarios = vec![
        Scenario::live("core-want-type/first-want-standing", ALL, world().record(2, &[WRONG_FIRST_WANT]).record(5, &[WANT_P]).step_unposted(6))
            .control_of(THE_LETTER, the_letter())
            .control(STILL_FAILS, still_fails()),
        Scenario::live(
            "core-want-type/first-want-released",
            ALL,
            world().record(2, &[WRONG_FIRST_WANT]).record(5, &["release D source:host", WANT_P]).step_unposted(6),
        )
        .control_of(THE_LETTER, the_letter())
        .control(STILL_FAILS, still_fails()),
        Scenario::live(
            "core-want-type/failure-under-a-standing-core-want",
            ALL,
            world().record(2, &[WRONG_FIRST_WANT, WANT_P]).step_unposted(3).record(5, &["want P parameters entity:E1 optional"]),
        )
        .control(
            "an entry that fails for its type while a want of the core stands on it takes, at that failure, the type its parent's part lists, and its load starts again",
            core(|rules| rules.entry_failed_for_its_type_under_a_core_want_takes_the_type = true),
        )
        .control(
            "a second want on a parent in force is a want added on each of its dependencies",
            core(|rules| rules.second_want_on_a_parent_retries_its_dependencies = true),
        ),
        Scenario::live(
            "core-want-type/parent-wanted-again-at-its-fall",
            ALL,
            world().record(2, &[WRONG_FIRST_WANT, WANT_P]).record(6, &["release P source:tool"]).record(7, &[WANT_P]).step_unposted(8),
        )
        .control_of(THE_LETTER, the_letter())
        .control(
            "the wants of dependencies added and recomputed with a batch alone",
            core(|rules| rules.dependency_wants_with_a_batch_alone = true),
        ),
    ];
    FamilyPlan {
        name: "core-want-type",
        claim: "a want the core adds on an entry failed for its type takes the type its parent's part lists",
        family,
        scenarios,
        oracle: include_str!("../oracles/core-want-type.txt"),
    }
}

/// A content at hand: these contents in place of those of the same assets,
/// with the contents of assets the saved world's content does not hold, and
/// the contents of the other assets as they are.
fn at_hand(saved: &[&'static str], changed: &[&'static str]) -> Vec<&'static str> {
    let asset = |name: &str| name.split('.').next().unwrap_or(name).to_string();
    let mut contents: Vec<&'static str> = saved.iter().copied().filter(|name| !changed.iter().any(|other| asset(other) == asset(name))).collect();
    contents.extend_from_slice(changed);
    contents
}

pub fn restore_batch(settings: &Settings) -> FamilyPlan {
    // A part of `P` that holds another list of dependencies differs from
    // `P.1` in that list and in its hash; `Q.2` lists what `Q.1` lists and
    // differs from it in its parameters.
    const CONTENTS: &[Content] = &[
        content("D", 1),
        content("F", 1),
        content("G", 1),
        content("N", 1),
        content("P", 1).listing(&["D"]),
        content("P", 2).listing(&["D", "F"]).with_the_parameters_of(1),
        content("P", 3).listing(&["D", "G"]).with_the_parameters_of(1),
        content("P", 4).listing(&["D", "N"]).with_the_parameters_of(1),
        content("P", 5).listing(&["D", "U"]).with_the_parameters_of(1),
        content("P", 6).listing(&["D", "Z"]).with_the_parameters_of(1),
        content("P", 7).listing(&["D", "X"]).with_the_parameters_of(1),
        content("Q", 1).listing(&["G"]),
        content("Q", 2).listing(&["G"]),
        content("U", 1),
        content("X", 1).listing(&["F"]),
        content("Z", 1),
        content("Z", 2),
    ];
    // The saved world's content: it does not hold `X`.
    const SAVED: &[&str] = &["D.1", "F.1", "G.1", "N.1", "P.1", "Q.1", "U.1", "Z.1"];
    const SAVER: &str = "restore-batch/saved-world";
    const WIDER: &str = "the batch of a restore holds an ordinary arrival for every asset of the closure of an answer that the table does not hold available";
    let family = Family::new(&["D", "F", "G", "N", "P", "Q", "U", "X", "Z"], &["P", "Q"], CONTENTS, &[], settings);
    // The composition's contributor `base` wants `P` and `Q` as required.
    let composition = || WorldPlan::new(12).wanting("P", Required).wanting("Q", Required);
    let wider = || loader(|options| options.restore_answers_complete_the_batch = true);
    // The saved world runs under a scripted availability source, is saved
    // after tick 8 and goes on to tick 12: the uninterrupted run.
    let saved_world = Scenario::scripted(
        SAVER,
        composition()
            .record(1, &["arrival D D.1", "arrival P P.1", "arrival Q Q.1", "arrival G failed invalid-part"])
            .record(2, &["want F parameters source:host optional", "want N parameters source:host optional", "want Z parameters source:host optional"])
            .record(3, &["arrival F failed invalid-part", "arrival N failed invalid-part", "arrival Z Z.1"])
            .record(7, &["want U parameters source:host optional"])
            .record(8, &["release N source:host", "release U source:host", "release Z source:host"])
            .saved(&[8]),
    )
    .control(
        "an entry leaves the table at the fixed point that ends its last want",
        core(|rules| rules.entry_leaves_at_the_fixed_point_that_ends_its_last_want = true),
    );
    // A reader: the save loaded in a new process, as a restore under the
    // content at hand, into the saved world's composition, naming no content
    // set, with a live loading capability over a loose store of its own. Its
    // record is empty but for what the capability posts.
    let reader = |name: &str, changed: &[&'static str]| Scenario::live(name, &at_hand(SAVED, changed), composition()).reading(SAVER, 8, false, &[]);
    let scenarios = vec![
        saved_world,
        reader("restore-batch/recorded-hashes", &[]).checking(Check::ContinuesTheSavedRun).control(WIDER, wider()),
        reader("restore-batch/failed-entry-newly-listed", &["P.2"])
            .control(
                "answers posted for the entries the saved table holds available, and for no other asset",
                loader(|options| options.restore_answers_do_not_follow_the_closure = true),
            )
            .control(WIDER, wider()),
        reader("restore-batch/failed-entry-newly-listed-fails-again", &["P.2"])
            .cut(&["F.1"])
            .control(
                "no failed arrival posted, at a restore, for an entry the save holds as failed",
                loader(|options| options.restore_posts_no_failure_for_a_failed_entry = true),
            )
            .control(
                "an arrival that leaves an entry failed for the reason it held emits no event and is named by no step, whatever want the same record added on the entry",
                core(|rules| rules.failure_for_the_reason_held_at_the_drain_changes_nothing = true),
            ),
        reader("restore-batch/failed-entry-of-another-parent-newly-listed", &["P.3"]).control(
            "an entry the save holds as failed under a want the core added receives nothing, whichever parent owns that want",
            loader(|options| options.restore_leaves_a_failed_entry_under_any_core_want = true),
        ),
        reader("restore-batch/failed-entry-with-no-want-newly-listed", &["P.4"])
            .control(
                "the batch completed for an entry the save holds as failed under a want, and not for one it holds as failed under none",
                loader(|options| options.restore_completes_for_a_failed_entry_under_a_want_alone = true),
            )
            .control(WIDER, wider()),
        reader("restore-batch/unavailable-entry-newly-listed", &["P.5"])
            .control(
                "the batch holds an ordinary arrival for an asset that a want of the core creates or returns to unavailable, and for no entry the save already holds unavailable",
                loader(|options| options.restore_posts_nothing_for_an_unavailable_entry = true),
            )
            .control(WIDER, wider()),
        reader("restore-batch/available-entry-with-no-want-newly-listed", &["P.6", "Z.2"])
            .control(
                "the answer of an entry that held no want when the record was drained changes nothing, a want the core adds from a part of the same record not counted",
                core(|rules| rules.arrival_for_an_entry_unwanted_at_the_drain_changes_nothing = true),
            )
            .control(WIDER, wider()),
        reader("restore-batch/parent-at-another-hash-keeps-its-failed-dependency", &["Q.2"]).control(WIDER, wider()),
        reader("restore-batch/failed-entry-newly-listed-at-depth-two", &["P.7", "X.1"])
            .control(
                "the batch completed one level under an answer alone, the parts of the ordinary arrivals it holds not being followed",
                loader(|options| options.restore_completes_one_level_alone = true),
            )
            .control(WIDER, wider()),
    ];
    FamilyPlan {
        name: "restore-batch",
        claim: "the answers to a restore complete the batch of each closure",
        family,
        scenarios,
        oracle: include_str!("../oracles/restore-batch.txt"),
    }
}
