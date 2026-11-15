//! The families of the stand-in sheet that a second reading of the design
//! text added, judged on states, as data: an arrival for an entry with no
//! want that a want of the first step names, the gate's count through the
//! pending parts to any depth, and the count of a budget over an entry in
//! force that holds no want. Each scenario is the sheet's statement in its
//! words, and each control the rule it changes. No oracle is read here.
//! Throwaway, never product code.

use prototype_core::table::Policy::Required;

use crate::families::core;
use crate::play::{Change, FamilyPlan, Move, Scenario, WorldPlan};
use crate::run::{Content, Family, Settings, content};

// ---------------------------------------------------------------------------
// The first step and the gate, after a second reading of the design text
// ---------------------------------------------------------------------------
//
// Neither family of this part gives a size to its contents.

// Every world of this family stands on a live loading capability over a
// loose store written by the stand-in pipeline: the records under test are
// the batch of a revision and the answers to a restore, each what the
// capability posts. A later revision is put in place before the round of
// the tick named: "between tick 5 and tick 6" is before the round of tick 6.
pub fn first_step_arrival(settings: &Settings) -> FamilyPlan {
    const CONTENTS: &[Content] = &[
        content("B", 1),
        content("B", 2),
        content("M", 1).listing(&["Z"]),
        content("P", 1),
        content("P", 2).listing(&["B", "Z"]),
        content("P", 3).listing(&["M"]),
        content("Z", 1),
        content("Z", 2),
    ];
    // The first revision.
    const FIRST: &[&str] = &["B.1", "M.1", "P.1", "Z.1"];
    // The later revision, or the content at hand, of the scenarios that
    // change the parent with the two entries, the two entries alone, and
    // the parent to a part that lists them two levels down.
    const WITH_THE_PARENT: &[&str] = &["B.2", "P.2", "Z.2"];
    const WITHOUT_THE_PARENT: &[&str] = &["B.2", "Z.2"];
    const AT_DEPTH_TWO: &[&str] = &["P.3", "Z.2"];
    const SAVER: &str = "first-step-arrival/saved-world";
    const AT_ITS_ARRIVAL: &str =
        "the wants that a part lists are added when the arrival of that part is applied, in identifier order, and not before the first arrival of the record";
    let family = Family::new(&["B", "M", "P", "Z"], &["P"], CONTENTS, &[], settings);
    let at_its_arrival = || core(|rules| rules.wants_of_a_part_added_where_its_arrival_is_applied = true);
    // The opening, shared by every world of the family that is not a
    // reader. No command names `M`.
    let opening = || {
        WorldPlan::new(10)
            .record(2, &["want P parameters source:host required", "want B parameters source:tool optional", "want Z parameters source:tool optional"])
            .record(5, &["release B source:tool", "release Z source:tool"])
    };
    let on_a_revision = |name: &str, before: u32, later: &[&'static str]| Scenario::live(name, FIRST, opening()).moving(before, Move::Revision(later.to_vec()));
    // A reader: the save loaded in a new process, as a restore under the
    // content at hand, into the saved world's composition, naming no content
    // set, with a live loading capability over a loose store of its own,
    // stepped to tick 10. Its record is empty but for what the capability
    // posts. The content at hand holds these contents in place of those of
    // the same assets in the first revision.
    let reader = |name: &str, changed: &[&'static str]| {
        let unchanged = |held: &&&'static str| !changed.iter().any(|other| other.split('.').next() == held.split('.').next());
        let at_hand: Vec<&'static str> = FIRST.iter().filter(unchanged).chain(changed).copied().collect();
        Scenario::live(name, &at_hand, WorldPlan::new(10)).reading(SAVER, 5, false, &[])
    };
    let scenarios = vec![
        on_a_revision("first-step-arrival/revision-with-the-parent", 6, WITH_THE_PARENT)
            .control(
                "an arrival for an entry that holds no want once the commands of its record are applied changes nothing, whatever want the core adds at the first step",
                core(|rules| rules.arrival_for_an_entry_unwanted_after_the_commands_changes_nothing = true),
            )
            .control(AT_ITS_ARRIVAL, at_its_arrival()),
        on_a_revision("first-step-arrival/revision-without-the-parent", 6, WITHOUT_THE_PARENT)
            .control("an arrival is applied to an entry that no want names", core(|rules| rules.unnamed_arrivals_applied = true)),
        on_a_revision("first-step-arrival/revision-at-depth-two", 6, AT_DEPTH_TWO).control(
            "the first step adds the wants one level under an asset that holds a want and no further, the wants of the levels below being added once the arrivals are applied",
            core(|rules| rules.first_step_adds_one_level_alone = true),
        ),
        // The later revision is put in place between tick 4 and tick 5: its
        // batch stands in the record that holds the two releases.
        on_a_revision("first-step-arrival/revision-in-the-record-of-the-release", 5, WITH_THE_PARENT).control(
            "an arrival of an asset whose last want a command of the same record released changes nothing, whatever want the core adds at the first step",
            core(|rules| rules.arrival_for_an_asset_released_in_its_record_changes_nothing = true),
        ),
        // The saved world: the opening, with no later revision, saved after
        // tick 5. The process that saved it goes on to tick 10: the
        // uninterrupted run.
        Scenario::live(SAVER, FIRST, opening().saved(&[5])).control(
            "an entry leaves the table at the fixed point that ends its last want",
            core(|rules| rules.entry_leaves_at_the_fixed_point_that_ends_its_last_want = true),
        ),
        reader("first-step-arrival/restore-with-the-parent", WITH_THE_PARENT)
            .control(
                "the marked answer of an entry that the save holds with no want changes nothing, whatever want the core adds at the first step",
                core(|rules| rules.arrival_for_an_entry_unwanted_at_the_drain_changes_nothing = true),
            )
            .control(AT_ITS_ARRIVAL, at_its_arrival()),
        reader("first-step-arrival/restore-without-the-parent", WITHOUT_THE_PARENT).control(
            "an answer that is not applied does not count as an answer that differed",
            core(|rules| rules.unwanted_answer_never_differs = true),
        ),
        reader("first-step-arrival/restore-at-depth-two", AT_DEPTH_TWO).control(
            "the marked answer of an entry with no want is applied only where the want that names it is added from the part of a marked answer, a want added from the part of an ordinary arrival not counted",
            core(|rules| rules.marked_answer_named_from_the_part_of_an_answer_alone = true),
        ),
    ];
    FamilyPlan {
        name: "first-step-arrival",
        claim: "an arrival for an entry with no want is applied where a want of the first step names the asset",
        family,
        scenarios,
        oracle: include_str!("../oracles/first-step-arrival.txt"),
    }
}

/// The design text as it read before its second reading, a control of two
/// scenarios of the gate's count: the policy of a pending parent is the one
/// that the wants the table holds give it.
fn the_tables_wants_alone() -> Change {
    core(|rules| rules.gate_counts_core_wants_from_the_tables_wants_alone = true)
}

// A scenario of this family asks for several steps without waiting in a
// row, and an element joins the pending record at a point of the scenario,
// whatever tick that record is of: `record(n, ..)` is "record of tick n";
// `step_in_a_row(n)` and `joins(n, ..)` are what the scenario states once
// tick `n` has run, in the order stated. "And the world steps" is the step
// of the round.
pub fn pending_depth(settings: &Settings) -> FamilyPlan {
    const CONTENTS: &[Content] = &[content("G", 1), content("K", 1), content("M", 1).listing(&["K"]), content("R", 1).listing(&["M"]), content("S", 1).listing(&["R"])];
    const WANT_R: &str = "want R parameters source:host required";
    const ARRIVAL_OF_K: &[&str] = &["arrival K K.1"];
    // The batch of `R` without the arrival of its second level.
    const WITHOUT_K: &[&str] = &["arrival M M.1", "arrival R R.1"];
    let family = Family::new(&["G", "K", "M", "R", "S"], &["G", "K", "R"], CONTENTS, &[], settings);
    let world = || WorldPlan::new(8);
    let scenarios = vec![
        Scenario::scripted(
            "pending-depth/second-level-missing",
            world().record(2, &[WANT_R]).step_in_a_row(2).joins(2, WITHOUT_K).step_in_a_row(2).joins(2, ARRIVAL_OF_K),
        )
        .control(
            "the design text as it read before its second reading: the policy of a pending parent is the one that the wants the table holds give it, and a want the core would add on that parent from another pending part gives it none",
            the_tables_wants_alone(),
        ),
        Scenario::scripted(
            "pending-depth/complete-batch",
            world().record(2, &[WANT_R]).step_in_a_row(2).joins(2, &["arrival K K.1", "arrival M M.1", "arrival R R.1"]).step_in_a_row(2),
        )
        .control(
            "a want the core would add from the part of a pending parent that the table holds under no want is answered by an entry the table holds available alone, and by no pending arrival",
            core(|rules| rules.gate_answers_under_an_unwanted_pending_parent_from_the_table_alone = true),
        ),
        Scenario::scripted(
            "pending-depth/second-level-in-force",
            world()
                .record(2, &["want K parameters source:tool optional"])
                .record(3, ARRIVAL_OF_K)
                .record(4, &[WANT_R])
                .step_in_a_row(4)
                .joins(4, WITHOUT_K)
                .step_in_a_row(4),
        )
        .control(
            "a want the core would add from the part of a pending parent that the table holds under no want is answered by a pending arrival alone, and by no entry the table holds available",
            core(|rules| rules.gate_answers_under_an_unwanted_pending_parent_from_the_pending_arrivals_alone = true),
        ),
        Scenario::scripted(
            "pending-depth/parent-under-an-optional-want-of-its-own",
            world()
                .record(2, &[WANT_R, "want M parameters source:tool optional"])
                .step_in_a_row(2)
                .joins(2, WITHOUT_K)
                .step_in_a_row(2)
                .joins(2, ARRIVAL_OF_K),
        )
        .control(
            "the design text as it read before its second reading: the policy of a pending parent is the one that the wants the table holds give it, and a want the core would add on that parent from another pending part is not counted in it",
            the_tables_wants_alone(),
        ),
        Scenario::scripted(
            "pending-depth/optional-root",
            world()
                .record(2, &["want R parameters source:host optional", "want G parameters source:host required"])
                .step_in_a_row(2)
                .joins(2, &["arrival G G.1", "arrival M M.1", "arrival R R.1"])
                .step_in_a_row(2)
                .joins(2, ARRIVAL_OF_K),
        )
        .control(
            "a want the core would add from the part of a pending parent that the table holds under no want is counted as required, whatever the policy of the want that parent would take",
            core(|rules| rules.gate_counts_as_required_under_an_unwanted_pending_parent = true),
        ),
        Scenario::scripted(
            "pending-depth/third-level-missing",
            world()
                .record(2, &["want S parameters source:host required"])
                .step_in_a_row(2)
                .joins(2, &["arrival M M.1", "arrival R R.1", "arrival S S.1"])
                .step_in_a_row(2)
                .joins(2, ARRIVAL_OF_K),
        )
        .control(
            "a pending parent takes the policy of the want the core would add on it only where that want comes from the part of an asset that the table holds under a want, so that the count follows the pending parts two levels down and no further",
            core(|rules| rules.gate_follows_the_pending_parts_two_levels_alone = true),
        ),
    ];
    FamilyPlan {
        name: "pending-depth",
        claim: "the gate counts a want the core would add through the pending parts to any depth",
        family,
        scenarios,
        oracle: include_str!("../oracles/pending-depth.txt"),
    }
}

// ---------------------------------------------------------------------------
// The count of a budget, after a second reading of the design text
// ---------------------------------------------------------------------------
//
// The family runs under the unit of the design text, the group of a root,
// which is the build's default. Every content has the size the sheet gives
// it, which each scripted arrival to a hash names.

pub fn count_without_want(settings: &Settings) -> FamilyPlan {
    const CONTENTS: &[Content] =
        &[content("B", 1).sized(2_000), content("D", 1).sized(4_000), content("N", 1).sized(5_000), content("P", 1).sized(2_500).listing(&["D"])];
    const WANT_N: &str = "want N parameters source:host optional";
    const WANT_P: &str = "want P parameters source:host optional";
    const RELEASE_D: &str = "release D source:host";
    // An arrival to failed for an asset in force: the script's, which stands
    // for an asset that leaves the instance's manifest.
    const D_FAILED: &str = "arrival D failed unknown-asset";
    // The three arrivals of the record under test, one batch.
    const THREE_ARRIVALS: &[&str] = &[D_FAILED, "arrival N N.1", "arrival P P.1"];
    const AS_BEFORE: &str =
        "the sentence as the design text had it before: the count leaves out an entry in force wherever the record holds an arrival to failed for it, whether or not the entry holds a want";
    let family = Family::new(&["B", "D", "N", "P"], &["B", "D", "N", "P"], CONTENTS, &[], settings);
    let as_before = || core(|rules| rules.budget_count_leaves_out_every_entry_an_arrival_to_failed_names = true);
    // One world per scenario, with the budget its scenario states; the
    // composition's contributor `base` wants `B` as required at tick 0.
    let world = |bytes: u64| WorldPlan::new(8).with_a_budget(bytes).wanting("B", Required).record(1, &["arrival B B.1"]);
    // The opening of five scenarios, the record of tick 5 being given: `D`
    // is the entry, in force under the host's optional want from tick 3.
    let opening = |bytes: u64, record_of_tick_5: &[&'static str]| {
        world(bytes).record(2, &["want D parameters source:host optional"]).record(3, &["arrival D D.1"]).record(5, record_of_tick_5)
    };
    // The record under test releases the last want of the entry.
    let released_in_the_record = |name: &str, bytes: u64| {
        Scenario::scripted(name, opening(bytes, &[WANT_N, WANT_P]).record(6, &[RELEASE_D]).record(6, THREE_ARRIVALS)).control(AS_BEFORE, as_before())
    };
    let scenarios = vec![
        released_in_the_record("count-without-want/first-load-fits-beside-the-entry", 9_000),
        released_in_the_record("count-without-want/first-load-does-not-fit-beside-the-entry", 8_000),
        released_in_the_record("count-without-want/both-fit-without-the-entry", 10_000),
        // No command of the record under test names the entry.
        Scenario::scripted("count-without-want/entry-keeps-its-want", opening(9_000, &[WANT_N, WANT_P]).record(6, THREE_ARRIVALS)).control(
            "the count holds an entry in force that an arrival to failed of the same record fails",
            core(|rules| rules.budget_count_holds_the_failing_entries = true),
        ),
        Scenario::scripted(
            "count-without-want/entry-under-the-want-of-a-parent",
            world(10_000).record(2, &[WANT_P]).record(3, &["arrival D D.1", "arrival P P.1"]).record(5, &[WANT_N]).record(6, &[D_FAILED, "arrival N N.1"]),
        )
        .control(
            "the count leaves out an entry that an arrival to failed fails only where a want that the core did not add names it",
            core(|rules| rules.budget_count_leaves_out_a_failing_entry_under_an_own_want_alone = true),
        ),
        // The record before the one under test releases the last want of
        // the entry.
        Scenario::scripted("count-without-want/entry-due-to-fall", opening(9_000, &[RELEASE_D, WANT_N, WANT_P]).record(6, THREE_ARRIVALS))
            .control(AS_BEFORE, as_before())
            .control(
                "the other reading of what the count leaves out: an entry due to fall whose arrival to failed the record holds is left out of the count, whatever closure reaches it",
                core(|rules| rules.budget_count_leaves_out_an_entry_due_to_fall_that_an_arrival_to_failed_names = true),
            ),
    ];
    FamilyPlan {
        name: "count-without-want",
        claim: "an entry in force that holds no want stays counted",
        family,
        scenarios,
        oracle: include_str!("../oracles/count-without-want.txt"),
    }
}

#[cfg(test)]
mod tests {
    use prototype_core::world::Size;

    /// Every control of the families a second reading of the design text
    /// added gives another run than its scenario's own: one that gave that
    /// very run could not differ from an oracle the scenario equals. The
    /// comparison is with the scenario's run, and no oracle.
    #[test]
    fn every_control_of_the_families_of_the_second_reading_gives_another_run() {
        const FAMILIES: [(&str, usize); 3] = [("first-step-arrival", 10), ("pending-depth", 6), ("count-without-want", 7)];
        let mut settings = crate::settings("second-reading");
        settings.size = Size::Small;
        settings.workers = vec![1, 4];
        let only: Vec<String> = FAMILIES.iter().map(|(family, _)| (*family).to_string()).collect();
        let (log, clean) = crate::drive(&settings, &only, false);
        let _ = std::fs::remove_dir_all(&settings.scratch);
        assert!(clean, "{log}");
        for (family, count) in FAMILIES {
            let of_the_family = |line: &&str| line.contains(&format!("note: {family}/"));
            let controls: Vec<&str> = log.lines().filter(|line| line.contains("] gives another run: ") || line.contains("GIVES THE RUN OF THE SCENARIO ITSELF")).filter(of_the_family).collect();
            let unchanged: Vec<&&str> = controls.iter().filter(|line| line.contains("GIVES THE RUN OF THE SCENARIO ITSELF")).collect();
            assert!(controls.len() == count && unchanged.is_empty(), "{family}: {} controls driven; those that give their scenario's own run: {unchanged:#?}", controls.len());
        }
    }
}
