//! The families of the same-harness sheet that are judged on states, as
//! data: a want made by a system, several worlds on one loading capability,
//! a mount while worlds exist and an excess over a world's budget. No oracle
//! is read here. Throwaway, never product code.

use asset_loader::Delay;
use prototype_core::table::BudgetUnit;
use prototype_core::table::Policy::Required;

use crate::families::{core, loader};
use crate::play::{Change, Check, FamilyPlan, Move, Scenario, SetName, WorldPlan};
use crate::run::{Content, Family, Settings, content};

pub fn system_wants(settings: &Settings) -> FamilyPlan {
    const CONTENTS: &[Content] =
        &[content("A", 1), content("A", 2), content("F", 1), content("G", 1), content("K", 1), content("M", 1).listing(&["K"]), content("R", 1).listing(&["M"])];
    const AT_THE_NEXT_TICK: &str = "what the system does is applied at the input fixed point of the next tick, before that tick's record";
    const BY_E1: &str = "want A parameters entity:E1 optional";
    let family = Family::new(&["A", "F", "G", "K", "M", "R"], &["A", "K", "R"], CONTENTS, &[], settings);
    let at_the_next_tick = || core(|rules| rules.system_applied_at_the_next_input_fixed_point = true);
    let with_the_record = || core(|rules| rules.system_applied_with_its_ticks_record = true);
    let both_wants: &[&'static str] = &["want A parameters entity:E1 required", "want A parameters entity:E2 optional"];
    let by_system = WorldPlan::new(14)
        .named("by-system")
        .system(2, both_wants)
        .record(3, &["arrival A A.1"])
        .system(5, &["release A entity:E1"])
        .system(7, &["release A entity:E2"])
        .system(10, &["want A parameters entity:E2 optional"])
        .record(12, &["arrival A A.2"]);
    let by_command = WorldPlan::new(14)
        .named("by-command")
        .record(2, both_wants)
        .record(3, &["arrival A A.1"])
        .record(5, &["release A entity:E1"])
        .record(7, &["release A entity:E2"])
        .record(10, &["want A parameters entity:E2 optional"])
        .record(12, &["arrival A A.2"]);
    let scenarios = vec![
        Scenario::scripted("system-wants/entity-wants", by_system)
            .also(by_command)
            .checking(Check::ReportSameState("by-system", "by-command"))
            .control_on(&["by-system"], AT_THE_NEXT_TICK, at_the_next_tick()),
        Scenario::scripted(
            "system-wants/despawn-by-a-system",
            WorldPlan::new(10)
                .system(2, &[BY_E1, "want A parameters entity:E2 optional"])
                .record(3, &["arrival A A.1"])
                .system(5, &["despawn E1"])
                .system(7, &["despawn E2"]),
        )
        .control(AT_THE_NEXT_TICK, at_the_next_tick()),
        Scenario::scripted(
            "system-wants/closure-under-the-world-entry",
            WorldPlan::new(14)
                .system(2, &["want R parameters world optional"])
                .record(4, &["arrival K K.1", "arrival M M.1", "arrival R R.1"])
                .record(5, &["want R parameters source:host optional"])
                .system(7, &["want R parameters world required"])
                .system(9, &["release R world"])
                .record(11, &["release R source:host"]),
        )
        .control(AT_THE_NEXT_TICK, at_the_next_tick()),
        Scenario::scripted(
            "system-wants/arrival-in-the-tick-of-the-want",
            WorldPlan::new(8).record(3, &["arrival A A.1"]).system(3, &[BY_E1]).record(5, &["arrival A A.1"]),
        )
        .control("the want of the system is applied as a command of the record of its tick, before the arrivals", with_the_record()),
        Scenario::scripted(
            "system-wants/want-again-at-the-tick-of-the-fall",
            WorldPlan::new(10).system(2, &[BY_E1]).record(3, &["arrival A A.1"]).system(5, &["release A entity:E1"]).system(6, &[BY_E1]).record(8, &["arrival A A.2"]),
        )
        .control("the want and the release of the system are applied as commands of the record of their tick, before the falls", with_the_record()),
        Scenario::live(
            "system-wants/gate-live",
            &["K.1", "M.1", "R.1", "F.1"],
            WorldPlan::new(12)
                .system(3, &["want R parameters entity:E1 required"])
                .system(6, &["want F parameters entity:E2 required"])
                .system(8, &["want F parameters entity:E2 required"])
                .system(9, &["want F parameters entity:E1 required"]),
        )
        .cut(&["F.1"])
        .control(AT_THE_NEXT_TICK, at_the_next_tick()),
        Scenario::scripted("system-wants/gate-pending", WorldPlan::new(8).system(3, &["want G parameters entity:E1 required"]).step(4).record(4, &["release G entity:E1"]))
            .control(AT_THE_NEXT_TICK, at_the_next_tick()),
    ];
    FamilyPlan {
        name: "system-wants",
        claim: "a want made by a system through its declared access behaves as one made by a command",
        family,
        scenarios,
        oracle: include_str!("../oracles/system-wants.txt"),
    }
}

/// One pattern of records of the sixteen worlds: its name, its records and
/// the content set its worlds are created under.
type Pattern = (&'static str, fn(WorldPlan) -> WorldPlan, SetName);

/// One case of the budget: its name, its world and what moves the content.
type Case = (&'static str, WorldPlan, Option<(u32, Move)>);

fn first_pattern(world: WorldPlan) -> WorldPlan {
    world
        .record(2, &["want P parameters source:host required"])
        .record(4, &["want Q parameters entity:E1 required"])
        .record(7, &["release P source:host"])
        .record(10, &["want J parameters entity:E2 optional"])
}

fn second_pattern(world: WorldPlan) -> WorldPlan {
    world
        .record(2, &["want C parameters entity:E1 optional", "want Q parameters source:host required"])
        .record(5, &["want P parameters entity:E2 optional"])
        .record(8, &["release Q source:host"])
        .record(11, &["release C entity:E1"])
}

pub fn shared_loader(settings: &Settings) -> FamilyPlan {
    const CONTENTS: &[Content] = &[
        content("C", 1),
        content("C", 2),
        content("J", 1),
        content("L", 1),
        content("L", 2),
        content("M", 1).listing(&["C"]),
        content("P", 1).listing(&["J", "M"]),
        content("Q", 1).listing(&["L", "M"]),
    ];
    const SET: &[&str] = &["C.1", "J.1", "L.1", "M.1", "P.1", "Q.1"];
    const ORACLE: &str = "shared-loader/sixteen-worlds";
    let family = Family::new(&["C", "J", "L", "M", "P", "Q"], &["C", "J", "L", "M", "P", "Q"], CONTENTS, &[], settings);
    let patterns: [Pattern; 4] =
        [("first", first_pattern, SetName::None), ("second", second_pattern, SetName::None), ("third", first_pattern, SetName::First), ("fourth", second_pattern, SetName::First)];
    // Sixteen worlds on one instance, listed and stepped in the order
    // `first-0`, `second-0`, `third-0`, `fourth-0`, `first-1`, and so on.
    let sixteen = |name: &str, delay: Delay| {
        let mut worlds = Vec::new();
        for offset in 0..4 {
            for (pattern, records, set) in patterns {
                worlds.push(records(WorldPlan::new(16).named(&format!("{pattern}-{offset}")).under(set).later(offset)));
            }
        }
        let mut worlds = worlds.into_iter();
        let mut scenario = Scenario::live(name, SET, worlds.next().expect("sixteen worlds"));
        for world in worlds {
            scenario = scenario.also(world);
        }
        scenario.oracle = ORACLE.to_string();
        // The later revision is put in place before any world is created.
        scenario.live.as_mut().expect("a live scenario").then = vec!["C.2", "L.2"];
        let delayed = match delay {
            Delay::None => loader(|options| options.delay = Delay::None),
            Delay::ReverseOfTheOrder => loader(|options| options.delay = Delay::ReverseOfTheOrder),
            Delay::Clock => loader(|options| options.delay = Delay::Clock),
        };
        scenario.always(delayed).control(
            "a batch is posted on the queue of every world of the instance, and not on the queue of the world whose want it answers alone",
            loader(|options| options.batches_posted_to_every_world = true),
        )
    };
    let scenarios = vec![
        sixteen(ORACLE, Delay::None),
        sixteen("shared-loader/sixteen-worlds/batches-ready-in-the-reverse-order", Delay::ReverseOfTheOrder),
        sixteen("shared-loader/sixteen-worlds/batches-delayed-from-the-clock", Delay::Clock),
    ];
    FamilyPlan {
        name: "shared-loader",
        claim: "several worlds on one loader each receive their own batches",
        family,
        scenarios,
        oracle: include_str!("../oracles/shared-loader.txt"),
    }
}

pub fn mount(settings: &Settings) -> FamilyPlan {
    const CONTENTS: &[Content] = &[content("O", 1), content("O", 9), content("Q", 9), content("T", 9).listing(&["O"])];
    const BASE: &[&str] = &["O.1"];
    const ONE_PER_TICK: &str = "the arrivals of a mount or of an unmount are posted to a world one per tick, in identifier order, in place of one batch";
    let family = Family::new(&["O", "Q", "T"], &["O", "Q", "T"], CONTENTS, &[], settings);
    // What happens, the same for each world of the first scenario.
    let world = |name: &str| WorldPlan::new(18).named(name).record(7, &["want O parameters source:host required"]).record(9, &["want Q parameters source:host required"]);
    let under_worlds = Scenario::live("mount/package-under-worlds", BASE, world("held").wanting("O", Required))
        .also(world("late"))
        .also(world("early").record(2, &["want Q parameters source:tool optional"]))
        .also(world("released").record(13, &["release O source:host", "release Q source:host"]).record(14, &["want O parameters source:tool optional"]))
        .also(world("under-set").wanting("O", Required).under(SetName::First))
        .moving(5, Move::Mount(vec!["O.9", "Q.9"]))
        .moving(14, Move::Unmount)
        .control_on(&["held", "late", "early", "released"], ONE_PER_TICK, loader(|options| options.moves_posted_one_per_tick = true))
        .control_on(
            &["released"],
            "the batch of a mount or of an unmount holds no arrival for an entry that has no want",
            loader(|options| options.moves_leave_entries_without_a_want = true),
        )
        .control_on(&["under-set"], "the instance's manifest stands in place of the set's", loader(|options| options.current_manifest_for_sets = true));
    let own_asset = Scenario::live("mount/own-asset-with-a-dependency", BASE, WorldPlan::new(12).record(4, &["want T parameters source:host required"]))
        .moving(3, Move::Mount(vec!["O.9", "T.9"]))
        .moving(9, Move::Unmount)
        .control("the arrivals of the unmount are posted one per tick, in identifier order", loader(|options| options.moves_posted_one_per_tick = true));
    FamilyPlan {
        name: "mount",
        claim: "a mount while worlds exist reaches each world as one revision",
        family,
        scenarios: vec![under_worlds, own_asset],
        oracle: include_str!("../oracles/mount.txt"),
    }
}

pub fn budget(settings: &Settings) -> FamilyPlan {
    const CONTENTS: &[Content] = &[
        content("B", 1).sized(9_000),
        content("D", 1).sized(6_000),
        content("K", 1).sized(5_000),
        content("K", 2).sized(11_000),
        content("N", 1).sized(2_000),
        content("R", 1).sized(2_000).listing(&["D"]),
        content("S", 1).sized(1_000),
        content("S", 2).sized(2_000),
        content("V", 1).sized(1_000).listing(&["N", "X"]),
        content("W", 1).sized(1_000).listing(&["N"]),
        content("X", 1).sized(9_000),
    ];
    const FIRST: &[&str] = &["B.1", "D.1", "K.1", "N.1", "R.1", "S.1", "V.1", "W.1", "X.1"];
    const WANT_R: &str = "want R parameters source:host required";
    const WANT_B: &str = "want B parameters source:host optional";
    let family = Family::new(&["B", "D", "K", "N", "R", "S", "V", "W", "X"], &["K", "R", "S", "W"], CONTENTS, &[], settings);
    let world = |to: u32, bytes: u64| WorldPlan::new(to).with_a_budget(bytes);
    let cases: Vec<Case> = vec![
        ("root-and-dependency", world(8, 10_000).wanting("K", Required).record(3, &[WANT_R]), None),
        ("closure-over-the-budget", world(8, 7_000).record(3, &[WANT_R]), None),
        ("small-beside-large", world(8, 10_000).wanting("K", Required).record(3, &[WANT_B, "want S parameters source:host required"]), None),
        ("overlapping-closures", world(8, 10_000).record(3, &["want V parameters source:host optional", "want W parameters source:host required"]), None),
        ("revision-that-grows", world(8, 10_000).wanting("K", Required).wanting("S", Required), Some((4, Move::Revision(vec!["K.2", "S.2"])))),
        (
            "arrival-at-the-tick-of-a-fall",
            world(12, 10_000).record(2, &["want K parameters source:host optional"]).record(5, &["release K source:host", WANT_B]).record(8, &["release B source:host", WANT_B]),
            None,
        ),
    ];
    // The three candidates of what an excess rejects. The candidate on the
    // closure of one want is the one whose arrivals name a root.
    let candidates: [(&str, Vec<Change>); 3] = [
        ("single-arrival", vec![core(|rules| rules.budget_unit = BudgetUnit::SingleArrival)]),
        ("closure-of-a-want", vec![core(|rules| rules.budget_unit = BudgetUnit::ClosureOfAWant), loader(|options| options.roots_marked = true)]),
        ("whole-drain", vec![core(|rules| rules.budget_unit = BudgetUnit::WholeDrain)]),
    ];
    let mut scenarios = Vec::new();
    for (case, world, moved) in &cases {
        for (candidate, changes) in &candidates {
            let mut scenario = Scenario::live(&format!("budget/{case}/{candidate}"), FIRST, world.clone());
            if let Some((before, change)) = moved {
                scenario = scenario.moving(*before, change.clone());
            }
            for change in changes {
                scenario = scenario.always(*change);
            }
            scenario = scenario.control("the budget only reports, and every arrival is applied", core(|rules| rules.budget_only_reports = true));
            if *case == "arrival-at-the-tick-of-a-fall" {
                scenario = scenario.control("the falls before the record", core(|rules| rules.falls_before_record = true));
            }
            scenarios.push(scenario);
        }
    }
    FamilyPlan { name: "budget", claim: "an excess over a world's budget is rejected identically", family, scenarios, oracle: include_str!("../oracles/budget.txt") }
}
