//! The scenarios of two claims: the order at a tick's input fixed point, and
//! the owners of wants. Throwaway, never product code.
//!
//! Each scenario is the scenario sheet's statement as data: the wants the
//! composition makes at creation, and what stands before each tick, in the
//! sheet's own words. Its availability source is scripted, but for the one
//! scenario that asks what a live loading capability posts.

use asset_loader::{Attachment, Loader, Options};
use prototype_core::table::{Policy, Rules};
use prototype_core::world::{Links, World};

use crate::oracle::{Expected, parse};
use crate::run::{Act, BASE, Claim, Content, Driven, Family, Outcome, Settings, Variant, config, content, import, judge, late_extract, replay, scripted_world};

/// A rule changed on purpose: of the core, or of the loading capability.
#[derive(Clone, Copy)]
pub enum Change {
    Core(fn(&mut Rules)),
    Loader(fn(&mut Options)),
}

pub struct Scenario {
    pub name: &'static str,
    /// The wants the composition's contributor makes at creation.
    pub initial: &'static [(&'static str, Policy)],
    /// What stands before a tick: an element, or a step without waiting.
    pub acts: &'static [(u32, &'static str)],
    pub to: u32,
    /// A live loading capability over a store that holds these contents.
    pub live: Option<&'static [&'static str]>,
    /// The controls (which must differ from the oracle) and the readings.
    pub variants: &'static [(&'static str, bool, Change)],
}

const WITHOUT_WAITING: &str = "step-without-waiting";

const ARRIVALS_FIRST: (&str, bool, Change) = ("the arrivals before the commands", true, Change::Core(|rules| rules.arrivals_before_commands = true));
const FALLS_FIRST: (&str, bool, Change) = ("the falls before the record", true, Change::Core(|rules| rules.falls_before_record = true));
const DESPAWN_LATE: (&str, bool, Change) = (
    "reading: a despawn ends its wants at the structural change point of its tick",
    false,
    Change::Core(|rules| rules.despawn_ends_wants_at_the_structural_point = true),
);

pub const ORDER: &[Scenario] = &[
    Scenario {
        name: "order/want-and-arrival",
        initial: &[],
        acts: &[(4, "want A source:host required"), (4, "arrival A A.1")],
        to: 10,
        live: None,
        variants: &[ARRIVALS_FIRST],
    },
    Scenario {
        name: "order/release-and-arrival",
        initial: &[],
        acts: &[(2, "want A source:host required"), (2, "arrival A A.1"), (5, "release A source:host"), (5, "arrival A A.2")],
        to: 10,
        live: None,
        variants: &[ARRIVALS_FIRST],
    },
    Scenario {
        name: "order/want-again-after-release",
        initial: &[],
        acts: &[(2, "want A source:host required"), (2, "arrival A A.1"), (5, "release A source:host"), (6, "want A source:host required")],
        to: 10,
        live: None,
        variants: &[FALLS_FIRST],
    },
    Scenario {
        name: "order/want-again-after-despawn",
        initial: &[],
        acts: &[(2, "want A entity:E1 required"), (2, "arrival A A.1"), (5, "despawn E1"), (6, "want A source:host optional")],
        to: 10,
        live: None,
        variants: &[FALLS_FIRST, DESPAWN_LATE],
    },
    Scenario {
        name: "order/want-again-live",
        initial: &[],
        acts: &[(2, "want A source:host required"), (5, "release A source:host"), (6, "want A source:host required")],
        to: 10,
        live: Some(&["A.1"]),
        variants: &[(
            "reading: a new want of an asset in force is answered by no arrival",
            false,
            Change::Loader(|options| options.a_want_of_an_asset_in_force_is_not_answered = true),
        )],
    },
    Scenario {
        name: "order/accumulated-want-and-arrival",
        initial: &[],
        acts: &[
            (3, "want G source:host required"),
            (4, "want A source:host required"),
            (4, "arrival A A.1"),
            (4, WITHOUT_WAITING),
            (4, "arrival G G.1"),
        ],
        to: 10,
        live: None,
        variants: &[ARRIVALS_FIRST],
    },
    Scenario {
        name: "order/accumulated-release-and-arrival",
        initial: &[],
        acts: &[
            (2, "want A source:host required"),
            (2, "arrival A A.1"),
            (3, "want G source:host required"),
            (4, "release A source:host"),
            (4, "arrival A A.2"),
            (4, WITHOUT_WAITING),
            (4, "arrival G G.1"),
        ],
        to: 10,
        live: None,
        variants: &[ARRIVALS_FIRST],
    },
    Scenario {
        name: "order/accumulated-want-again",
        initial: &[],
        acts: &[
            (1, "want A source:host required"),
            (1, "arrival A A.1"),
            (3, "want G source:host required"),
            (3, "release A source:host"),
            (4, "want A source:host required"),
            (4, WITHOUT_WAITING),
            (4, "arrival G G.1"),
        ],
        to: 10,
        live: None,
        variants: &[FALLS_FIRST],
    },
    Scenario {
        name: "order/gate-release-and-arrival",
        initial: &[],
        acts: &[(3, "want G source:host required"), (4, WITHOUT_WAITING), (4, "release G source:host"), (4, "arrival G G.1")],
        to: 10,
        live: None,
        variants: &[ARRIVALS_FIRST],
    },
    Scenario {
        name: "order/gate-arrival-and-new-want",
        initial: &[],
        acts: &[
            (3, "want G source:host required"),
            (4, WITHOUT_WAITING),
            (4, "arrival G G.1"),
            (4, "want B source:host required"),
            (5, WITHOUT_WAITING),
            (5, "arrival B B.1"),
        ],
        to: 10,
        live: None,
        variants: &[("the gate counts a pending command that adds a want", true, Change::Core(|rules| rules.gate_counts_added_wants = true))],
    },
    Scenario {
        name: "order/gate-pending-despawn",
        initial: &[],
        acts: &[(3, "want G entity:E1 required"), (4, WITHOUT_WAITING), (4, "despawn E1"), (4, WITHOUT_WAITING), (4, "arrival G G.1")],
        to: 10,
        live: None,
        variants: &[("the gate sees a pending despawn", true, Change::Core(|rules| rules.gate_sees_despawns = true)), DESPAWN_LATE],
    },
];

const PER_PARENT_WANT: (&str, bool, Change) =
    ("one want of the dependency per want of the parent", true, Change::Core(|rules| rules.dependency_want_per_parent_want = true));

pub const OWNERS: &[Scenario] = &[
    Scenario {
        name: "owners/two-wants-first-order",
        initial: &[],
        acts: &[
            (2, "want P entity:E1 required"),
            (2, "want P entity:E2 optional"),
            (3, "arrival P P.1"),
            (3, "arrival D1 D1.1"),
            (6, "despawn E1"),
            (9, "despawn E2"),
        ],
        to: 12,
        live: None,
        variants: &[PER_PARENT_WANT],
    },
    Scenario {
        name: "owners/two-wants-second-order",
        initial: &[],
        acts: &[
            (2, "want P entity:E2 optional"),
            (2, "want P entity:E1 required"),
            (3, "arrival P P.1"),
            (3, "arrival D1 D1.1"),
            (6, "despawn E1"),
            (9, "despawn E2"),
        ],
        to: 12,
        live: None,
        variants: &[("the wants in the order of making", true, Change::Core(|rules| rules.wants_in_making_order = true)), PER_PARENT_WANT],
    },
    Scenario {
        name: "owners/parent-changes-dependency",
        initial: &[],
        acts: &[(2, "want P entity:E1 required"), (3, "arrival P P.1"), (3, "arrival D1 D1.1"), (6, "arrival P P.2"), (6, "arrival D2 D2.1")],
        to: 12,
        live: None,
        variants: &[("the core's wants left as they were", true, Change::Core(|rules| rules.core_wants_left = true))],
    },
    Scenario {
        name: "owners/preload-over-composition",
        initial: &[("A", Policy::Required)],
        acts: &[
            (1, "arrival A A.1"),
            (3, "want A source:host optional"),
            (3, "want B source:host optional"),
            (4, "arrival B B.1"),
            (6, "release A source:host"),
            (6, "release B source:host"),
        ],
        to: 12,
        live: None,
        variants: &[
            ("one shared want under the world's own entry", true, Change::Core(|rules| rules.one_want_under_the_world = true)),
            ("reading: the makers ordered by their identity alone", false, Change::Core(|rules| rules.makers_by_identity_alone = true)),
        ],
    },
    Scenario {
        name: "owners/one-maker-two-wants",
        initial: &[],
        acts: &[(2, "want A source:host optional"), (3, "arrival A A.1"), (5, "want A source:host required"), (7, "release A source:host")],
        to: 12,
        live: None,
        variants: &[("a second want added beside the first", true, Change::Core(|rules| rules.second_want_added = true))],
    },
];

pub fn order_family(settings: &Settings) -> Family {
    Family::new(&["A", "B", "G"], &["A", "B"], &[content("A", 1), content("A", 2), content("B", 1), content("G", 1)], &[], settings)
}

pub fn owners_family(settings: &Settings) -> Family {
    let contents = [
        content("A", 1),
        content("B", 1),
        content("D1", 1),
        content("D2", 1),
        Content { asset: "P", version: 1, asset_type: 1, dependencies: &["D1"] },
        Content { asset: "P", version: 2, asset_type: 1, dependencies: &["D2"] },
    ];
    Family::new(&["A", "B", "D1", "P", "D2"], &["A", "B", "P"], &contents, &[], settings)
}

fn acts(scenario: &Scenario) -> Vec<(u32, Act)> {
    scenario.acts.iter().map(|(before, text)| (*before, if *text == WITHOUT_WAITING { Act::StepWithoutWaiting } else { Act::Post(text) })).collect()
}

/// Runs one scenario at one worker count under the given rules and returns
/// what it observed, with a replay of its log at another worker count.
fn run_one(settings: &Settings, family: &Family, scenario: &Scenario, workers: usize, rules: Rules, options: Options) -> (Driven, Option<bool>) {
    let names = &family.names;
    let wants: Vec<_> = scenario.initial.iter().map(|(asset, policy)| (names.assets[*asset], *policy)).collect();
    let configured = |workers: usize| {
        let mut config = config(settings, workers, names).wanting(BASE, &wants);
        config.rules = rules;
        config
    };
    let other = if workers == 1 { 4 } else { 1 };
    match scenario.live {
        None => {
            let residents = family.residents();
            let (world, queue) = scripted_world(configured(workers), &residents, Vec::new());
            let mut driven = Driven::new(world, queue);
            driven.drive(scenario.to, &acts(scenario), names, &|| ());
            let replays = driven.observed.stuck.is_none().then(|| {
                replay(&driven.observed.log, scenario.to, |records| scripted_world(configured(other), &residents, records).0) == driven.observed.state
            });
            (driven, replays)
        }
        Some(contents) => {
            let directory = settings.directory(&format!("{}-{workers}", scenario.name));
            family.write_sources(&directory.join("content"), contents);
            import(&directory.join("content"), &directory.join("store"));
            let loader = Loader::start(&directory.join("store"), options).expect("the loading capability starts");
            let (extract, slot) = late_extract();
            let mut links = Links::new(loader.residents());
            links.extract = Some(extract);
            let queue = links.queue.clone();
            let world = World::new(configured(workers), Vec::new(), links);
            let attachment = Attachment { queue: queue.clone(), set: None, unanswered: world.unanswered(), named: world.named(), restore: Vec::new() };
            let _ = slot.set(loader.attach(attachment).expect("the world is attached"));
            let mut driven = Driven::new(world, queue);
            driven.drive(scenario.to, &acts(scenario), names, &|| loader.settle());
            let residents = loader.residents();
            let replays = driven.observed.stuck.is_none().then(|| {
                let world = |records| {
                    let links = Links::new(residents.clone());
                    World::new(configured(other), records, links)
                };
                replay(&driven.observed.log, scenario.to, world) == driven.observed.state
            });
            (driven, replays)
        }
    }
}

/// Runs the scenarios of one claim: each at every worker count against its
/// oracle, then each of its controls and readings at one worker count.
pub fn run(name: &'static str, settings: &Settings, family: &Family, scenarios: &[Scenario], oracles: &str) -> Claim {
    let mut claim = Claim { name, ..Claim::default() };
    let stated = parse(oracles, &family.names).unwrap_or_else(|error| panic!("the oracle of {name} does not read: {error}"));
    let nothing = Expected::default();
    for scenario in scenarios {
        let expected = stated.get(scenario.name).and_then(|worlds| worlds.get("world")).unwrap_or(&nothing);
        if !stated.contains_key(scenario.name) {
            claim.notes.push(format!("{}: no oracle states this scenario", scenario.name));
        }
        for &workers in &settings.workers {
            let (driven, replays) = run_one(settings, family, scenario, workers, Rules::default(), Options::default());
            let differences = judge(settings, &family.names, expected, &driven.observed, None, scenario.to);
            let mut outcome = Outcome::new(scenario.name, "world", workers, &driven.observed, differences);
            outcome.replays = replays;
            claim.outcomes.push(outcome);
        }
        for (variant, control, change) in scenario.variants {
            let (mut rules, mut options) = (Rules::default(), Options::default());
            match change {
                Change::Core(change) => change(&mut rules),
                Change::Loader(change) => change(&mut options),
            }
            let (driven, _) = run_one(settings, family, scenario, 4, rules, options);
            let differences = judge(settings, &family.names, expected, &driven.observed, None, scenario.to);
            claim.variants.push(Variant {
                scenario: scenario.name.to_string(),
                name: variant,
                control: *control,
                differs: !differences.is_empty(),
                first_difference: differences.first().cloned().unwrap_or_default(),
            });
        }
    }
    claim
}
