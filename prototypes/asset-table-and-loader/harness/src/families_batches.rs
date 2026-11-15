//! The families of the stand-in sheet's part on a batch against the state of
//! the last tick and on a replacement that fails, judged on states, as data.
//! Both run with a live loading capability over a loose store written by the
//! stand-in pipeline. Each scenario is the sheet's statement in its words,
//! and each control the rule it changes. No oracle is read here. Throwaway,
//! never product code.
//!
//! In both families "a save" writes, deletes or writes again the sources it
//! names; one pass of the stand-in pipeline then runs over the content root,
//! and what that pass gives is put in place on the instance. `moving(b, ..)`
//! is "between tick `b - 1` and tick `b`": after the tick before has run and
//! before the host waits for the capability ahead of tick `b`. Where a world
//! takes a step without waiting before the capability has posted, what moves
//! the content of its instance moves it after that step. The harness reports
//! beside the oracle, and compares with nothing, what the pipeline gave after
//! each save and what the loading capability obtained for a world and did not
//! post to it.

use crate::families::{kept_under_a_failed_parent, loader};
use crate::play::{Change, FamilyPlan, Move, Save, Scenario, WorldPlan};
use crate::run::{Content, Family, Settings, content};

/// A rule of the stand-in pipeline changed: what a pass does where an action
/// fails.
fn pipeline(change: fn(&mut asset_pipeline::Options)) -> Change {
    Change::Pipeline(change)
}

// ---------------------------------------------------------------------------
// A batch complete against the state at the end of the last tick
// ---------------------------------------------------------------------------
//
// Each scenario has two worlds, each on an instance of its own, with its own
// content root, its own store and its own capability. `running` is the
// scenario as stated. `gated` is the same scenario with what its paragraph
// adds, so that the batch under test joins the pending record of a world
// whose gate is closed.

pub fn batch_state(settings: &Settings) -> FamilyPlan {
    const CONTENTS: &[Content] = &[
        content("D", 1).sized(6_000),
        content("G", 1).sized(1_000),
        content("K", 1).sized(3_000),
        content("P", 1).sized(2_000).listing(&["D"]),
        content("P", 2).sized(2_000).listing(&["D"]),
        content("P", 3).sized(2_000).listing(&["D", "X"]),
        content("P", 4).sized(2_000).listing(&["D", "X"]),
        content("P", 5).sized(2_000),
        content("X", 1).sized(1_500),
    ];
    // The first revision.
    const FIRST: &[&str] = &["D.1", "G.1", "K.1", "P.1", "X.1"];
    const BOTH: &[&str] = &["running", "gated"];
    const WHAT_IT_POSTED: &str = "the loading capability builds a batch against what it posted to the world, an asset it posted to a hash counting as held available at that hash until it posts that asset again, in place of the table extracted at the end of the last tick the world ran";
    const WANT_G: &str = "want G parameters source:host required";
    let family = Family::new(&["D", "G", "K", "P", "X"], &["D", "P"], CONTENTS, &[], settings);
    let what_it_posted = || loader(|options| options.trusts_what_it_posted = true);
    let world = |to: u32, name: &str| WorldPlan::new(to).named(name);
    // A revision put in place: the save that gives it.
    let revision = |contents: &[&'static str]| Move::Save(Save::of(contents));
    // The two worlds of a scenario, each on an instance of its own, with the
    // control of every scenario of the family, on both worlds.
    let scenario = |name: &str, running: WorldPlan, gated: WorldPlan| {
        Scenario::live(name, FIRST, running).also(gated).on_instances_of_their_own().reported().control_on(BOTH, WHAT_IT_POSTED, what_it_posted())
    };
    let scenarios = vec![
        // An arrival rejected over the budget, the room freed, then a later
        // revision of the same asset.
        {
            let stated = |name: &str| {
                world(10, name)
                    .with_a_budget(12_000)
                    .record(1, &["want K parameters source:host optional", "want P parameters source:host required"])
                    .record(5, &["release K source:host"])
            };
            scenario("batch-state/revision-after-a-rejected-replacement", stated("running"), stated("gated").record(7, &[WANT_G]).step_unposted(8))
                .moving(4, revision(&["P.3"]))
                .moving(8, revision(&["P.4"]))
        },
        {
            let stated = |name: &str| {
                world(10, name)
                    .with_a_budget(10_000)
                    .record(1, &["want K parameters source:host optional"])
                    .record(3, &["want P parameters source:host required"])
                    .record(5, &["release K source:host"])
            };
            scenario("batch-state/revision-after-a-rejected-first-load", stated("running"), stated("gated").record(7, &[WANT_G]).step_unposted(8))
                .moving(8, revision(&["P.2"]))
        },
        // A dependency that left the table when its parent failed, the
        // parent being retried.
        {
            let stated = |name: &str, retry: &'static str| world(10, name).record(1, &["want P parameters source:host optional"]).record(7, &[retry]);
            scenario(
                "batch-state/retry-of-a-parent-that-left-the-manifest",
                stated("running", "want P parameters source:tool optional"),
                stated("gated", "want P parameters source:tool required").step_unposted(8),
            )
            // The source of `P` is deleted, then saved again as it was.
            .moving(4, Move::Save(Save::deleting(&["P"])))
            .moving(8, revision(&["P.1"]))
            .control("the wants the core added for the dependencies of a parent that is failed stand while that parent holds a want", kept_under_a_failed_parent())
        },
        {
            let stated = |name: &str, retry: &'static str| world(8, name).record(1, &["want P parameters source:host optional"]).record(5, &[retry]);
            scenario(
                "batch-state/retry-of-a-root-that-never-rose",
                stated("running", "want P parameters source:tool optional"),
                stated("gated", "want P parameters source:tool required").step_unposted(6),
            )
            // When the scenario starts, the artifact of `P.1` is cut short;
            // the repair puts the whole artifact in the store.
            .cut(&["P.1"])
            .moving(4, Move::Repair(vec!["P.1"]))
        },
        // A former dependency that a later revision lists again.
        {
            let stated = |name: &str| world(8, name).record(1, &["want P parameters source:host required"]);
            scenario("batch-state/former-dependency-listed-again", stated("running"), stated("gated").record(5, &[WANT_G]).step_unposted(6))
                .moving(4, revision(&["P.5"]))
                .moving(6, revision(&["P.2"]))
                .control(
                    "the loading capability builds the batch of a revision against the table extracted one tick before the last",
                    loader(|options| options.moves_built_against_the_table_before_the_last = true),
                )
        },
    ];
    FamilyPlan {
        name: "batch-state",
        claim: "a batch is complete against the state at the end of the last tick before the drain",
        family,
        scenarios,
        oracle: include_str!("../oracles/batch-state.txt"),
    }
}

// ---------------------------------------------------------------------------
// A replacement that fails leaves what is in force
// ---------------------------------------------------------------------------
//
// The worlds of a scenario run on one instance, with one content root, one
// store and one capability, and are stepped in rounds, in the order listed.
// Two stand-ins exist for the comparison alone. A source saved with an error
// holds the word `error` where the importer reads the first parameter: the
// canonical action of its asset fails, with a diagnostic that names the
// source, and every other action of the pass runs. A composition that
// carries the admission check gives it to the entry of the type of
// parameters, with the bound: the check refuses a part whose first parameter
// is greater than the bound, which the family states of two contents.

pub fn failed_replacement(settings: &Settings) -> FamilyPlan {
    const CONTENTS: &[Content] = &[
        content("A", 1),
        content("A", 2),
        content("B", 1),
        content("B", 2).above_the_bound(),
        content("B", 3),
        content("N", 1),
        content("P", 1).listing(&["B"]),
        content("P", 2).listing(&["B"]),
        content("P", 3).listing(&["B", "N"]).above_the_bound(),
        content("P", 4).listing(&["B", "N"]),
    ];
    const WANT_A_REQUIRED: &str = "want A parameters source:host required";
    const WANT_A_OPTIONAL: &str = "want A parameters source:host optional";
    const WANT_B_REQUIRED: &str = "want B parameters source:host required";
    const WANT_B_OPTIONAL: &str = "want B parameters source:host optional";
    const WANT_P_REQUIRED: &str = "want P parameters source:host required";
    const HELD_BACK: &str = "an action that fails holds the revision back, the pipeline putting no revision in place until every action of a pass has succeeded";
    const NO_CHECK: &str = "no admission check is run";
    let family = Family::new(&["A", "B", "N", "P"], &["A", "B", "P"], CONTENTS, &[], settings);
    // Each world is stepped to tick 9.
    let world = |name: &str| WorldPlan::new(9).named(name);
    let held_back = || pipeline(|options| options.failed_action_holds_the_revision_back = true);
    let no_check = || loader(|options| options.no_admission_check_run = true);
    // A save that writes the sources of these contents.
    let saved_as = |contents: &[&'static str]| Move::Save(Save::of(contents));
    let scenarios = vec![
        // A source saved with an error, then corrected. No composition
        // carries the check. The content root holds the sources of `A` and
        // of `B`.
        Scenario::live("failed-replacement/source-saved-with-an-error", &["A.1", "B.1"], world("required").record(1, &[WANT_A_REQUIRED, WANT_B_OPTIONAL]))
            .also(world("optional").record(1, &[WANT_A_OPTIONAL, WANT_B_OPTIONAL]))
            .also(world("late").record(5, &[WANT_A_REQUIRED]))
            .reported()
            // The erroneous save: the source of `A` is saved with an error,
            // and the source of `B` is saved as `B.2`.
            .moving(4, Move::Save(Save::of(&["B.2"]).with_an_error(&["A.2"])))
            // The correction: the source of `A` is saved again, without the
            // error, as `A.2`.
            .moving(7, saved_as(&["A.2"]))
            .control(
                "the pipeline writes a revision without an entry for an asset whose action failed, in place of its former entry",
                pipeline(|options| options.failed_action_leaves_no_entry = true),
            )
            .control_on(&["required", "optional"], HELD_BACK, held_back()),
        // The content root holds the source of `B` alone.
        Scenario::live("failed-replacement/first-import-saved-with-an-error", &["B.1"], world("world").record(1, &[WANT_B_OPTIONAL]).record(5, &["want N parameters source:host required"]))
            .reported()
            // The source of `N` is saved for the first time, with an error,
            // and the source of `B` is saved as `B.2`.
            .moving(4, Move::Save(Save::of(&["B.2"]).with_an_error(&["N.1"])))
            // The source of `N` is saved again, without the error, as `N.1`.
            .moving(7, saved_as(&["N.1"]))
            .control("the batch of a revision holds no arrival for an entry that is failed", loader(|options| options.moves_leave_failed_entries = true))
            .control(HELD_BACK, held_back()),
        // A revision of which a world's admission check refuses one asset.
        Scenario::live(
            "failed-replacement/revision-of-two-with-one-refused",
            &["A.1", "B.1"],
            world("checked").carrying_the_check().record(1, &[WANT_A_REQUIRED, WANT_B_REQUIRED]),
        )
        .also(world("unchecked").record(1, &[WANT_A_REQUIRED, WANT_B_REQUIRED]))
        .also(world("late").carrying_the_check().record(5, &[WANT_B_REQUIRED]))
        .reported()
        .moving(4, saved_as(&["A.2", "B.2"]))
        .moving(7, saved_as(&["B.3"]))
        .control_on(&["checked", "late"], NO_CHECK, no_check())
        .control_on(
            &["unchecked"],
            "a part that one world of the instance refuses is posted to no world of that instance",
            loader(|options| options.a_part_one_world_refuses_is_posted_to_no_world = true),
        )
        .control_on(
            &["checked"],
            "the batch of a revision is held back while the world refuses one of its assets",
            loader(|options| options.move_held_back_while_an_asset_is_refused = true),
        )
        .control_on(
            &["checked"],
            "a replacement the world refuses is posted to it as a failure",
            loader(|options| options.refused_replacement_posted_as_a_failure = true),
        ),
        Scenario::live("failed-replacement/dependent-of-a-refused-asset", &["B.1", "P.1"], world("world").carrying_the_check().record(1, &[WANT_P_REQUIRED]))
            .reported()
            .moving(4, saved_as(&["B.2", "P.2"]))
            .moving(7, saved_as(&["B.3"]))
            .control(
                "an asset whose part lists a refused asset is withheld with it",
                loader(|options| options.dependent_of_a_refused_asset_withheld = true),
            ),
        Scenario::live(
            "failed-replacement/refused-parent-with-a-new-dependency",
            &["A.1", "B.1", "N.1", "P.1"],
            world("world").carrying_the_check().record(1, &[WANT_A_OPTIONAL, WANT_P_REQUIRED]),
        )
        .reported()
        .moving(4, saved_as(&["A.2", "P.3"]))
        .moving(7, saved_as(&["P.4"]))
        .control(NO_CHECK, no_check()),
    ];
    FamilyPlan {
        name: "failed-replacement",
        claim: "a replacement that fails leaves what is in force, and the rest of its revision is applied",
        family,
        scenarios,
        oracle: include_str!("../oracles/failed-replacement.txt"),
    }
}
