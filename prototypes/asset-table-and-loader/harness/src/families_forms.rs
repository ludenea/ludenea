//! The families of the stand-in sheet on a prepared form built once and on a
//! dedicated server started from the domain set alone, judged on states, as
//! data. Each scenario is the sheet's statement in its words, and each
//! control the rule it changes. No oracle is read here. Throwaway, never
//! product code.

use prototype_core::forms::Preparer;
use prototype_core::table::AssetType::Second;
use prototype_core::table::Policy::Required;

use crate::families::{core, loader};
use crate::play::{Build, Change, Check, FamilyPlan, Move, Prepared, Scenario, SetName, Tweak, WorldPlan};
use crate::run::{Content, Family, Settings, content};

/// A rule of the stand-in pipeline changed.
fn pipeline(change: fn(&mut asset_pipeline::Options)) -> Change {
    Change::Pipeline(change)
}

// ---------------------------------------------------------------------------
// A prepared form built once
// ---------------------------------------------------------------------------
//
// In this family the composition declares, for the type `second`, one
// prepared form that is built once, prepared by `stamp` at version 1, but
// where a world says that its composition declares none. Every pass of the
// stand-in pipeline over the content runs the preparing action. A world
// whose composition declares the form has the consumer `reader` in its
// schedule.
//
// A reader of a save and a world that replays a recording are each a
// process of their own in the full run: a scenario of the sheet that holds
// two of them is two scenarios here, each with one world, named after the
// scenario and the world and judged by the statement of the sheet's
// scenario.

pub fn prepared_form(settings: &Settings) -> FamilyPlan {
    // The sizes are those of the table of the scenarios under a budget, and
    // the family has one content: every part and every form has the size
    // stated there in every scenario, so that `M.1:built` names one byte
    // string. Where a scenario gives no size, its oracle states none.
    const CONTENTS: &[Content] = &[
        content("A", 1),
        content("K", 1).sized(5_000),
        content("M", 1).of(Second).sized(2_000).with_a_form_of(4_000),
        content("M", 2).of(Second).sized(2_000).with_a_form_of(4_500),
        content("M", 3).of(Second).sized(3_000).with_a_form_of(1_000),
    ];
    // The producer's content. The scenarios under a budget add two later
    // contents of `M`.
    const FIRST: &[&str] = &["A.1", "K.1", "M.1"];
    const RECORDED_RUN: &str = "prepared-form/recorded-run";
    const WANT_OF_M: &[&str] = &["want M second source:host required"];
    const LEFT_OUT: &str = "the hash of a prepared form left out of the arrival and of the entry";
    const FROM_THE_MACHINE: &str = "a prepared form is taken from what the machine built for the part, wherever the machine holds one";
    const PART_ALONE_CONFIRMED: &str = "the confirmation of a restore reads the hash of the canonical part alone";
    const SIZE_OF_THE_PART_ALONE: &str = "the size an arrival names and an entry holds is the size of the canonical part alone";
    let family = Family::new(&["A", "K", "M"], &["A", "K"], CONTENTS, &[], settings).preparing(&[(Second, Preparer::Stamp)]);
    let left_out = || loader(|options| options.forms_not_named = true);
    let from_the_machine = || loader(|options| options.forms_taken_from_the_machine = true);
    let own_form_where_gone = || loader(|options| options.local_form_where_the_named_one_cannot_be_had = true);
    let part_alone_confirmed = || core(|rules| rules.confirmation_reads_the_part_alone = true);
    let size_of_the_part_alone = || loader(|options| options.arrivals_name_the_size_of_the_part_alone = true);
    // The composition of the scenarios that give no size: it declares the
    // form, and its contributor `base` wants `A` as required at tick 0.
    let composition = || WorldPlan::new(6).wanting("A", Required).declaring_the_form(Second, Preparer::Stamp);
    // A world of a run: the record of tick 2 holds the want of `M`.
    let run = |name: &str| composition().named(name).record(2, WANT_OF_M);
    // The stores, beside the producer's store and the machine's own store.
    let both = Prepared::PRODUCER.beside_the_own_forms();
    let without_the_form_built_once = Prepared::PRODUCER.without_the_forms_built_once();

    let recorded_run = Scenario::live(RECORDED_RUN, FIRST, run("no-set").saved(&[4]).recorded())
        .also(run("under-set").under(SetName::First).saved(&[4]).recorded())
        .over(&[Prepared::PRODUCER])
        .control(LEFT_OUT, left_out());
    // A recording replayed in a process beside: the world is made by the
    // same creation, names no content set or is created under the hash of
    // the producer's manifest, and receives its arrivals from the recording.
    let replay = |scenario: &str, world: &'static str, store: Prepared| {
        let plan = if world == "under-set" { composition().named(world).under(SetName::First) } else { composition().named(world) };
        Scenario::live(&format!("{scenario}/{world}"), FIRST, plan).stated_by(scenario).replaying_the_recording(RECORDED_RUN, world).over(&[store])
    };
    // `no-set` replays on an instance whose manifest is the machine's own,
    // `under-set` on one whose manifest is the producer's.
    let replays = |scenario: &str, store: Prepared| [replay(scenario, "no-set", store.under(Build::Own)), replay(scenario, "under-set", store.under(Build::Producer))];
    let beside_the_own_form = replays("prepared-form/replay-beside-the-machine-own-form", both).map(|scenario| scenario.control(FROM_THE_MACHINE, from_the_machine()).control(LEFT_OUT, left_out()));
    let without_the_form = replays("prepared-form/replay-without-the-form-built-once", without_the_form_built_once).map(|scenario| {
        scenario.control("a replay takes the form its machine built for the part where the form a record names cannot be had", own_form_where_gone())
    });
    let own_build = Scenario::live("prepared-form/own-build-names-its-own-form", FIRST, run("no-set"))
        .also(run("under-own-set").under(SetName::First))
        .over(&[Prepared::OWN])
        .checking(Check::ReportAgainstTheRunOf(RECORDED_RUN, 4, &[("no-set", "no-set"), ("under-own-set", "under-set")]))
        .control(
            "the form a machine builds for itself is written in its manifest and held in its store under the hash of the form built once",
            pipeline(|options| options.own_forms_under_the_hash_of_the_form_built_once = true),
        );
    let gone_from_the_store = Scenario::live("prepared-form/form-built-once-gone-from-the-store", FIRST, run("no-set"))
        .also(run("under-set").under(SetName::First))
        .over(&[without_the_form_built_once])
        .control(
            "the loading capability takes the form its machine built for the part, or builds it, where the store holds no bytes under the hash the manifest names",
            own_form_where_gone(),
        );

    // The readers of a save: each loads a save of the recorded run in a new
    // process, into the saved world's composition, and steps to tick 6. A
    // reader's record is empty but for what the capability posts. A reader
    // that reads the save of `under-set` names the content set of the
    // producer's manifest; every other reads the save of `no-set` and names
    // none.
    let reader = |name: &str, store: Prepared, recorded_content: bool| {
        Scenario::live(name, FIRST, composition()).reading(RECORDED_RUN, 4, recorded_content, &[]).of_the_world("no-set").over(&[store])
    };
    let reader_of = |scenario: &str, world: &'static str, store: Prepared| {
        let plan = if world == "under-set" { composition().named(world).under(SetName::First) } else { composition().named(world) };
        Scenario::live(&format!("{scenario}/{world}"), FIRST, plan).stated_by(scenario).reading(RECORDED_RUN, 4, true, &[]).of_the_world(world).over(&[store])
    };
    let confirms = ["no-set", "under-set"]
        .map(|world| reader_of("prepared-form/restore-confirms-the-form", world, both).control(FROM_THE_MACHINE, from_the_machine()).control(LEFT_OUT, left_out()));
    let readers = vec![
        reader("prepared-form/restore-refused-form-gone", without_the_form_built_once, true).control(PART_ALONE_CONFIRMED, part_alone_confirmed()),
        reader("prepared-form/restore-refused-own-build", Prepared::OWN, true).control(PART_ALONE_CONFIRMED, part_alone_confirmed()),
        // The text can be read two ways on this reader. The build confirms
        // an asset where the manifest the restored world stands under holds
        // it at the recorded hashes and the store holds their bytes; the
        // control is the other reading.
        reader("prepared-form/restore-own-manifest-beside-the-form-built-once", both.under(Build::Own), true).control(
            "an asset is confirmed where the store holds bytes under each recorded hash, whatever the manifest the restored world stands under holds",
            loader(|options| options.confirmation_reads_the_store_alone = true),
        ),
        reader("prepared-form/content-at-hand-same-form", Prepared::PRODUCER, false).control(LEFT_OUT, left_out()),
        reader("prepared-form/content-at-hand-own-build", Prepared::OWN, false).control(
            "an answer to a restore is compared with the recorded hash of the canonical part alone",
            core(|rules| rules.answer_compared_with_the_part_alone = true),
        ),
    ];

    // The scenarios under a budget: one instance whose manifest is the
    // producer's, over the producer's store. No world names a content set.
    // In each world the contributor `base` wants `K` as required at tick 0,
    // and the record of tick 3 holds the want of `M`.
    let budgeted = |to: u32, budget: u64| WorldPlan::new(to).wanting("K", Required).with_a_budget(budget).record(3, WANT_OF_M);
    let counted = Scenario::live("prepared-form/counted-with-its-part", FIRST, budgeted(11, 11_000).declaring_the_form(Second, Preparer::Stamp))
        .over(&[Prepared::PRODUCER])
        .moving(6, Move::Revision(vec!["M.2"]))
        .moving(9, Move::Revision(vec!["M.3"]))
        .control(SIZE_OF_THE_PART_ALONE, size_of_the_part_alone())
        .control("the budget only reports, and every arrival is applied", core(|rules| rules.budget_only_reports = true));
    let with_the_form = budgeted(6, 10_000).named("with-the-form").declaring_the_form(Second, Preparer::Stamp);
    // A composition that declares the type `second` and no prepared form
    // for it, and has no consumer `reader`.
    let without_the_form_declared = budgeted(6, 10_000).named("without-the-form").declaring_no_form();
    let over_the_budget = Scenario::live("prepared-form/first-load-over-the-budget-by-its-form", FIRST, with_the_form)
        .also(without_the_form_declared)
        .over(&[Prepared::PRODUCER])
        .control_on(&["with-the-form"], SIZE_OF_THE_PART_ALONE, size_of_the_part_alone())
        .control_on(
            &["without-the-form"],
            "an arrival names every form the domain manifest holds for its asset, with its size, whatever the world's composition declares",
            loader(|options| options.arrivals_name_every_form_of_the_manifest = true),
        );

    let mut scenarios = vec![recorded_run];
    scenarios.extend(beside_the_own_form);
    scenarios.extend(without_the_form);
    scenarios.extend([own_build, gone_from_the_store]);
    scenarios.extend(confirms);
    scenarios.extend(readers);
    scenarios.extend([counted, over_the_budget]);
    FamilyPlan {
        name: "prepared-form",
        claim: "a part or a form built once is never read under another hash",
        family,
        scenarios,
        oracle: include_str!("../oracles/prepared-form.txt"),
    }
}

// ---------------------------------------------------------------------------
// A dedicated server started from the domain set alone
// ---------------------------------------------------------------------------
//
// Two processes over one content: one run of the stand-in pipeline gives one
// domain manifest with its parts and its forms built once, and one
// presentation manifest with its payloads for the one target profile
// `desktop`. The client's process stands over all of it, with the stand-in
// presentation consumer; the server's stands over a selection of its own,
// the domain manifest, the parts and the forms built once, and nothing else.
// Each holds one world, made by the same creation and given the same
// records, on an instance and a loading capability of its own.

pub fn dedicated_server(settings: &Settings) -> FamilyPlan {
    // `T`, `U` and `V` have a payload, and `T` has `U` as its one
    // presentation dependency. No canonical part holds anything of it.
    const CONTENTS: &[Content] = &[
        content("C", 1),
        content("D", 1),
        content("M", 1).of(Second),
        content("S", 1).listing(&["D", "M", "T"]),
        content("T", 1).with_a_payload().presenting(&["U"]),
        content("T", 2).with_a_payload().presenting(&["U"]),
        content("U", 1).with_a_payload(),
        content("V", 1).with_a_payload(),
    ];
    // The first revision. The presentation revision changes the payload of
    // `T`, and the domain revision holds `T.2` in place of `T.1`.
    const FIRST: &[&str] = &["C.1", "D.1", "M.1", "S.1", "T.1", "U.1", "V.1"];
    const SAME_RECORDS: &str = "dedicated-server/same-records";
    const RELEASE_OF_S: &[&str] = &["release S source:host"];
    const WITHOUT_THE_FORMS: &str = "a server's selection holds the canonical parts alone, without the forms built once";
    let family = Family::new(&["C", "D", "M", "S", "T", "U", "V"], &["C", "S", "T", "V"], CONTENTS, &[], settings).preparing(&[(Second, Preparer::Stamp)]).presenting_for("desktop");
    let without_the_forms = || Change::Harness(Tweak::ServerSelectionWithoutTheForms);
    // The composition declares the two types and the form; its contributor
    // `base` wants `C` as required at tick 0.
    let composition = || WorldPlan::new(12).wanting("C", Required).declaring_the_form(Second, Preparer::Stamp);
    let world = |name: &str, set: SetName| {
        composition()
            .named(name)
            .under(set)
            .record(2, &["want S parameters source:host required"])
            .step_unposted(3)
            .record(4, &["want V parameters entity:E1 optional"])
            .record(10, RELEASE_OF_S)
    };
    // The two processes. The payload of `V` is cut short in the client's
    // store from the start; the presentation revision is put in place
    // between tick 6 and tick 7, in the client's process, and the domain
    // revision between tick 8 and tick 9, in both.
    let both = |name: &str, client: WorldPlan, server: WorldPlan| {
        Scenario::live(name, FIRST, client.with_a_presentation_consumer())
            .also(server)
            .on_instances_of_their_own()
            .with_a_server_on(&[1])
            .cut_payloads(&["V.1"])
            .reporting_the_presentation()
            .moving(7, Move::PayloadRevision(vec![("T.1", 2)]))
            .moving(9, Move::Revision(vec!["T.2"]))
            .checking(Check::SameState("client", "server"))
            .checking(Check::ReportSameState("client", "server"))
    };
    let same_records = both(SAME_RECORDS, world("client", SetName::None).saved(&[5]), world("server", SetName::None))
        .control_on(
            &["client"],
            "the loading capability resolves an identifier in the presentation manifest where its instance holds one",
            loader(|options| options.resolves_in_the_presentation_manifest = true),
        )
        .control_on(
            &["client"],
            "the closure of a want follows the presentation dependencies with the domain dependencies",
            loader(|options| options.closure_follows_presentation_dependencies = true),
        )
        .control_on(
            &["client"],
            "an arrival is posted once the payloads of its asset are resident, and is a failure where one cannot be loaded",
            loader(|options| options.arrivals_wait_for_payloads = true),
        )
        .control_on(
            &["client"],
            "a revision changes an asset whose entry in a presentation manifest differs",
            loader(|options| options.presentation_changes_are_revisions = true),
        )
        .control_on(&["server"], WITHOUT_THE_FORMS, without_the_forms());
    // Each world created under the hash of the first revision's domain
    // manifest, which is the content at hand at its creation, and saved
    // after tick 5.
    let under_the_set = both("dedicated-server/under-the-domain-set", world("client", SetName::AtHand).saved(&[5]), world("server", SetName::AtHand).saved(&[5]))
        .checking(Check::SameSetInTheSaves("client", "server"))
        .control(
            "the hash that names a content set covers every manifest its instance holds, the presentation manifest included",
            loader(|options| options.set_hash_covers_every_manifest = true),
        );
    // A reader of the client's save, in a new process over the server's
    // selection of the first revision, under the recorded content, into the
    // saved world's composition, naming no content set.
    let read_by_the_server = Scenario::live("dedicated-server/client-save-read-by-the-server", FIRST, composition().record(10, RELEASE_OF_S))
        .with_a_server_on(&[0])
        .reporting_the_presentation()
        .reading(SAME_RECORDS, 5, true, &[])
        .of_the_world("client")
        .moving(9, Move::Revision(vec!["T.2"]))
        .checking(Check::ContinuesTheSavedRun)
        .control(
            "the confirmation of a restore confirms the presentation payloads of the assets the table holds",
            loader(|options| options.confirmation_confirms_payloads = true),
        )
        .control(WITHOUT_THE_FORMS, without_the_forms());
    FamilyPlan {
        name: "dedicated-server",
        claim: "a dedicated server started from the domain set alone gives the hashes of a client that holds both sets",
        family,
        scenarios: vec![same_records, under_the_set, read_by_the_server],
        oracle: include_str!("../oracles/dedicated-server.txt"),
    }
}
