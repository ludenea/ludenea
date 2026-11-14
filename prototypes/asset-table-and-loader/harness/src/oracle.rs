//! The oracles written by hand, as the harness reads them, and the comparison
//! of a run with one. Throwaway, never product code.
//!
//! An oracle is a table of states and not a second implementation: per named
//! tick, the elements of the tick's input record, the whole asset table with
//! every want in its stated order, the events, the instances, and what a
//! step names. The harness writes each stated table through the canonical
//! form, in the order the oracle gives, and compares bytes; the state hash
//! it expects for a tick is the hash of those bytes with the workload's hash
//! of a sequential run that reads, per asset, what the stated table gives.

use std::collections::BTreeMap;

use prototype_core::table::{
    Arrival, AssetId, Availability, Command, Event, Instance, Maker, Outcome, Owner, Policy, REASON_ARTIFACT_GONE, REASON_INVALID_PART,
    REASON_UNKNOWN_ASSET, Record, RepulsionTable, Rules, Table, put_entry, put_instances,
};
use reference_workload::{Fnv1a, fnv1a};

use crate::sequential::Switch;

/// The names a family's scenarios and oracles share.
#[derive(Clone, Default)]
pub struct Names {
    pub assets: BTreeMap<String, AssetId>,
    pub hashes: BTreeMap<String, u64>,
    /// The parameters a part gives, by content hash.
    pub tables: BTreeMap<u64, RepulsionTable>,
    pub entities: BTreeMap<String, (u32, u32)>,
    /// The instances in the order they are made, with the policy and the
    /// reload setting of the command that makes each.
    pub instances: Vec<(String, Policy, bool)>,
    /// The assets the entities name, in turn.
    pub pool: Vec<AssetId>,
}

impl Names {
    pub fn asset(&self, name: &str) -> Result<AssetId, String> {
        self.assets.get(name).copied().ok_or_else(|| format!("unknown asset {name}"))
    }

    pub fn hash(&self, name: &str) -> Result<u64, String> {
        self.hashes.get(name).copied().ok_or_else(|| format!("unknown hash {name}"))
    }

    pub fn asset_name(&self, id: AssetId) -> String {
        self.assets.iter().find(|(_, other)| **other == id).map_or_else(|| format!("{id:032x}"), |(name, _)| name.clone())
    }

    pub fn hash_name(&self, hash: u64) -> String {
        self.hashes.iter().find(|(_, other)| **other == hash).map_or_else(|| format!("{hash:016x}"), |(name, _)| name.clone())
    }

    fn maker(&self, kind: &str, name: &str) -> Result<Maker, String> {
        match (kind, name) {
            ("contributor", "base") => Ok(Maker::Contributor(7)),
            ("source", "host") => Ok(Maker::Source(3)),
            ("source", "tool") => Ok(Maker::Source(5)),
            ("instance", name) => {
                let at = self.instances.iter().position(|(other, _, _)| other == name).ok_or_else(|| format!("unknown instance {name}"))?;
                Ok(Maker::Instance(at as u32 + 1))
            }
            _ => Err(format!("unknown maker {kind}:{name}")),
        }
    }

    fn owner(&self, text: &str) -> Result<Owner, String> {
        let (kind, name) = text.split_once(':').ok_or_else(|| format!("an owner is <kind>:<name>, not {text}"))?;
        match kind {
            "entity" => self.entity(name).map(|(slot, generation)| Owner::Entity { slot, generation }),
            "asset" => self.asset(name).map(Owner::Asset),
            _ => self.maker(kind, name).map(Owner::World),
        }
    }

    fn entity(&self, name: &str) -> Result<(u32, u32), String> {
        let name = name.strip_prefix("entity:").unwrap_or(name);
        self.entities.get(name).copied().ok_or_else(|| format!("unknown entity {name}"))
    }

    fn owner_name(&self, owner: &Owner) -> String {
        match owner {
            Owner::World(Maker::Contributor(7)) => "contributor:base".to_string(),
            Owner::World(Maker::Source(3)) => "source:host".to_string(),
            Owner::World(Maker::Source(5)) => "source:tool".to_string(),
            Owner::World(Maker::Instance(index)) => {
                format!("instance:{}", self.instances.get(*index as usize - 1).map_or("?", |(name, _, _)| name.as_str()))
            }
            Owner::World(other) => format!("{other:?}"),
            Owner::Entity { slot, generation } => {
                let name = self.entities.iter().find(|(_, other)| **other == (*slot, *generation)).map(|(name, _)| name.clone());
                format!("entity:{}", name.unwrap_or_else(|| format!("{slot}.{generation}")))
            }
            Owner::Asset(parent) => format!("asset:{}", self.asset_name(*parent)),
        }
    }
}

fn policy(text: &str) -> Result<Policy, String> {
    match text {
        "required" => Ok(Policy::Required),
        "optional" => Ok(Policy::Optional),
        other => Err(format!("unknown policy {other}")),
    }
}

fn policy_name(policy: Policy) -> &'static str {
    if policy == Policy::Required { "required" } else { "optional" }
}

fn reason(text: &str) -> Result<u32, String> {
    match text {
        "unknown-asset" => Ok(REASON_UNKNOWN_ASSET),
        "artifact-gone" => Ok(REASON_ARTIFACT_GONE),
        "invalid-part" => Ok(REASON_INVALID_PART),
        other => Err(format!("unknown reason {other}")),
    }
}

fn reason_name(code: u32) -> &'static str {
    match code {
        REASON_UNKNOWN_ASSET => "unknown-asset",
        REASON_ARTIFACT_GONE => "artifact-gone",
        REASON_INVALID_PART => "invalid-part",
        _ => "another-reason",
    }
}

/// One element of an input record.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Element {
    Command(Command),
    Arrival(Arrival),
}

/// Reads one element, in the words the scenario sheet and the oracles share.
pub fn element(text: &str, names: &Names) -> Result<Element, String> {
    let words: Vec<&str> = text.split_whitespace().collect();
    Ok(match words.as_slice() {
        ["want", asset, owner, level] => Element::Command(Command::Want { asset: names.asset(asset)?, owner: names.owner(owner)?, policy: policy(level)? }),
        ["release", asset, owner] => Element::Command(Command::Release { asset: names.asset(asset)?, owner: names.owner(owner)? }),
        ["despawn", entity] => {
            let (slot, generation) = names.entity(entity)?;
            Element::Command(Command::Despawn { slot, generation })
        }
        ["instantiate", _, asset, level, "reload", reload] => {
            Element::Command(Command::Instantiate { asset: names.asset(asset)?, policy: policy(level)?, reload: *reload == "on" })
        }
        ["arrival", asset, "failed", code] => Element::Arrival(Arrival::failed(names.asset(asset)?, reason(code)?)),
        ["arrival", asset, "failed", code, "marked"] => Element::Arrival(Arrival::failed(names.asset(asset)?, reason(code)?).marked()),
        ["arrival", asset, hash] => Element::Arrival(Arrival::at(names.asset(asset)?, names.hash(hash)?)),
        ["arrival", asset, hash, "marked"] => Element::Arrival(Arrival::at(names.asset(asset)?, names.hash(hash)?).marked()),
        _ => return Err(format!("not an element: {text}")),
    })
}

fn element_text(element: &Element, names: &Names) -> String {
    match element {
        Element::Command(Command::Want { asset, owner, policy }) => {
            format!("want {} {} {}", names.asset_name(*asset), names.owner_name(owner), policy_name(*policy))
        }
        Element::Command(Command::Release { asset, owner }) => format!("release {} {}", names.asset_name(*asset), names.owner_name(owner)),
        Element::Command(Command::Despawn { slot, generation }) => names.owner_name(&Owner::Entity { slot: *slot, generation: *generation }).replace("entity:", "despawn "),
        Element::Command(Command::Instantiate { asset, policy, reload }) => {
            format!("instantiate {} {} reload {}", names.asset_name(*asset), policy_name(*policy), if *reload { "on" } else { "off" })
        }
        Element::Arrival(arrival) => {
            let outcome = match arrival.outcome {
                Outcome::Hash(hash) => names.hash_name(hash),
                Outcome::Failure(code) => format!("failed {}", reason_name(code)),
            };
            format!("arrival {} {outcome}{}", names.asset_name(arrival.asset), if arrival.marked { " marked" } else { "" })
        }
    }
}

fn elements_of(record: &Record) -> Vec<Element> {
    record.commands.iter().map(|command| Element::Command(*command)).chain(record.arrivals.iter().map(|arrival| Element::Arrival(*arrival))).collect()
}

fn event_text(event: &Event, names: &Names) -> String {
    match event {
        Event::Arrived { asset, hash } => format!("arrived {} {}", names.asset_name(*asset), names.hash_name(*hash)),
        Event::HashChanged { asset, from, to } => format!("changed {} {} {}", names.asset_name(*asset), names.hash_name(*from), names.hash_name(*to)),
        Event::Failed { asset, code } => format!("failed {} {}", names.asset_name(*asset), reason_name(*code)),
        Event::NewHistory { assets } => format!("new-history {}", list_text(assets, names)),
    }
}

fn list_text(assets: &[AssetId], names: &Names) -> String {
    assets.iter().map(|asset| names.asset_name(*asset)).collect::<Vec<_>>().join(", ")
}

fn list(text: &str, names: &Names) -> Result<Vec<AssetId>, String> {
    let mut assets = text.split(',').map(str::trim).filter(|name| !name.is_empty()).map(|name| names.asset(name)).collect::<Result<Vec<_>, _>>()?;
    assets.sort_unstable();
    Ok(assets)
}

/// The outcome of a restore.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Verdict {
    Continues,
    NewHistory(Vec<AssetId>),
    Refused(String),
}

type StatedEntry = (AssetId, Availability, Vec<(Owner, Policy)>);

/// The statement of one tick.
#[derive(Clone, Debug, Default)]
pub struct Block {
    pub record: Vec<Element>,
    pub entries: Vec<StatedEntry>,
    pub events: Vec<Event>,
    pub instances: Vec<Instance>,
    pub names_failed: Vec<(AssetId, u32)>,
}

impl Block {
    /// The stated table through the canonical form, in the order stated.
    pub fn table_bytes(&self) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(&(self.entries.len() as u32).to_le_bytes());
        for (id, availability, wants) in &self.entries {
            put_entry(&mut out, *id, *availability, wants);
        }
        put_instances(&mut out, &self.instances);
        out
    }
}

/// What an oracle states of one world of one scenario.
#[derive(Clone, Debug, Default)]
pub struct Expected {
    pub restore: Option<Verdict>,
    /// The steps without waiting that run no tick: the tick each stood
    /// before, and the assets it names as awaited.
    pub blocked: Vec<(u32, Vec<AssetId>)>,
    pub blocks: BTreeMap<u32, Block>,
}

impl Expected {
    /// The block that states the table after `tick`: the last one at or before it.
    pub fn table_at(&self, tick: u32) -> Option<&Block> {
        self.blocks.range(..=tick).next_back().map(|(_, block)| block)
    }

    /// What the sequential oracle of the workload is handed: from which tick
    /// each asset of the pool gives which parameters, and which entities a
    /// stated record despawns. `creation` is the table before the first block.
    pub fn schedule(&self, names: &Names, from: u32, to: u32, creation: &[StatedEntry]) -> (Vec<Switch>, Vec<(u32, usize)>) {
        let mut switches = Vec::new();
        let mut kills = Vec::new();
        let mut held: Vec<Option<RepulsionTable>> = vec![None; names.pool.len()];
        for tick in from..=to {
            let entries = self.table_at(tick).map_or(creation, |block| block.entries.as_slice());
            for (index, asset) in names.pool.iter().enumerate() {
                let table = entries.iter().find(|(id, _, _)| id == asset).and_then(|(_, availability, _)| match availability {
                    Availability::Available(hash) => names.tables.get(hash).copied(),
                    _ => None,
                });
                if table != held[index] || tick == from {
                    switches.push(Switch { tick, asset: index, table });
                    held[index] = table;
                }
            }
            if let Some(block) = self.blocks.get(&tick) {
                for element in &block.record {
                    if let Element::Command(Command::Despawn { slot, .. }) = element {
                        kills.push((tick, *slot as usize));
                    }
                }
            }
        }
        (switches, kills)
    }
}

/// Reads an oracle file: its scenarios, each with its worlds.
pub fn parse(text: &str, names: &Names) -> Result<BTreeMap<String, BTreeMap<String, Expected>>, String> {
    let mut scenarios: BTreeMap<String, BTreeMap<String, Expected>> = BTreeMap::new();
    let (mut scenario, mut world, mut tick) = (String::new(), "world".to_string(), None::<u32>);
    for (number, line) in text.lines().enumerate() {
        let line = line.split('#').next().unwrap_or("").trim();
        if line.is_empty() {
            continue;
        }
        let fail = |reason: String| format!("line {}: {reason}", number + 1);
        let (keyword, rest) = line.split_once(' ').unwrap_or((line, ""));
        let rest = rest.trim();
        if keyword == "scenario" {
            scenario = rest.to_string();
            world = "world".to_string();
            tick = None;
            scenarios.entry(scenario.clone()).or_default();
            continue;
        }
        if keyword == "world" {
            world = rest.to_string();
            tick = None;
            continue;
        }
        let expected = scenarios.get_mut(&scenario).ok_or_else(|| fail("a statement before any scenario".to_string()))?.entry(world.clone()).or_default();
        match keyword {
            "restore" => {
                let (verdict, named) = rest.split_once(' ').unwrap_or((rest, ""));
                expected.restore = Some(match verdict {
                    "continues" => Verdict::Continues,
                    "new-history" => Verdict::NewHistory(list(named, names).map_err(&fail)?),
                    "refused" => Verdict::Refused(named.trim().to_string()),
                    other => return Err(fail(format!("unknown verdict {other}"))),
                });
            }
            "blocked" => {
                let (before, awaited) = rest.split_once(" awaiting").ok_or_else(|| fail("blocked <n> awaiting <assets>".to_string()))?;
                let before = before.trim().parse().map_err(|_| fail(format!("not a tick: {before}")))?;
                expected.blocked.push((before, list(awaited, names).map_err(&fail)?));
            }
            "tick" => {
                let n = rest.parse().map_err(|_| fail(format!("not a tick: {rest}")))?;
                expected.blocks.insert(n, Block::default());
                tick = Some(n);
            }
            _ => {
                let block = tick.and_then(|n| expected.blocks.get_mut(&n)).ok_or_else(|| fail(format!("{keyword} outside a tick block")))?;
                let words: Vec<&str> = rest.split_whitespace().collect();
                match (keyword, words.as_slice()) {
                    ("record", _) => block.record.push(element(rest, names).map_err(&fail)?),
                    ("entry", [asset, "unavailable"]) => block.entries.push((names.asset(asset).map_err(&fail)?, Availability::Unavailable, Vec::new())),
                    ("entry", [asset, "available", hash]) => {
                        block.entries.push((names.asset(asset).map_err(&fail)?, Availability::Available(names.hash(hash).map_err(&fail)?), Vec::new()));
                    }
                    ("entry", [asset, "failed", code]) => {
                        block.entries.push((names.asset(asset).map_err(&fail)?, Availability::Failed(reason(code).map_err(&fail)?), Vec::new()));
                    }
                    ("want", [owner, level]) => {
                        let want = (names.owner(owner).map_err(&fail)?, policy(level).map_err(&fail)?);
                        block.entries.last_mut().ok_or_else(|| fail("a want before any entry".to_string()))?.2.push(want);
                    }
                    ("event", ["arrived", asset, hash]) => {
                        block.events.push(Event::Arrived { asset: names.asset(asset).map_err(&fail)?, hash: names.hash(hash).map_err(&fail)? });
                    }
                    ("event", ["changed", asset, from, to]) => block.events.push(Event::HashChanged {
                        asset: names.asset(asset).map_err(&fail)?,
                        from: names.hash(from).map_err(&fail)?,
                        to: names.hash(to).map_err(&fail)?,
                    }),
                    ("event", ["failed", asset, code]) => {
                        block.events.push(Event::Failed { asset: names.asset(asset).map_err(&fail)?, code: reason(code).map_err(&fail)? });
                    }
                    ("event", ["new-history", ..]) => {
                        let named = rest.strip_prefix("new-history").unwrap_or("");
                        block.events.push(Event::NewHistory { assets: list(named, names).map_err(&fail)? });
                    }
                    ("instance", [name, asset, "instantiations", count, "failed", failed]) => {
                        let (_, policy, reload) =
                            names.instances.iter().find(|(other, _, _)| other == name).ok_or_else(|| fail(format!("unknown instance {name}")))?;
                        block.instances.push(Instance {
                            asset: names.asset(asset).map_err(&fail)?,
                            policy: *policy,
                            reload: *reload,
                            instantiations: count.parse().map_err(|_| fail(format!("not a count: {count}")))?,
                            failed: *failed == "yes",
                        });
                    }
                    ("names-failed", [asset, code]) => block.names_failed.push((names.asset(asset).map_err(&fail)?, reason(code).map_err(&fail)?)),
                    _ => return Err(fail(format!("not a statement: {line}"))),
                }
            }
        }
    }
    Ok(scenarios)
}

/// What a run gave, for one world.
#[derive(Clone, Debug, Default)]
pub struct Observed {
    /// The tick the world starts from: 0, or the tick of the save it was restored from.
    pub first: u32,
    /// The canonical bytes of the table at creation.
    pub creation: Vec<u8>,
    /// The canonical bytes of the table after each tick that ran, from `first + 1`.
    pub tables: Vec<Vec<u8>>,
    /// The state hashes from `first`.
    pub state: Vec<u64>,
    pub log: Vec<Record>,
    pub events: Vec<(u32, Event)>,
    pub blocked: Vec<(u32, Vec<AssetId>)>,
    pub names_failed: Vec<(u32, AssetId, u32)>,
    pub verdict: Option<Verdict>,
    /// The tick the world never ran, where it stayed gated to the end.
    pub stuck: Option<(u32, Vec<AssetId>)>,
}

/// A table's canonical bytes, in words, for a difference a reader arbitrates.
pub fn table_text(bytes: &[u8], names: &Names) -> String {
    let Ok(table) = Table::decode(bytes, Rules::default()) else { return "bytes that do not decode".to_string() };
    let mut text = String::new();
    for (id, entry) in &table.entries {
        let availability = match entry.availability {
            Availability::Unavailable => "unavailable".to_string(),
            Availability::Available(hash) => format!("available {}", names.hash_name(hash)),
            Availability::Failed(code) => format!("failed {}", reason_name(code)),
        };
        let wants: Vec<String> = entry.wants.iter().map(|(owner, policy)| format!("{} {}", names.owner_name(owner), policy_name(*policy))).collect();
        text.push_str(&format!("[{} {availability}: {}] ", names.asset_name(*id), wants.join(", ")));
    }
    for (index, instance) in table.instances.iter().enumerate() {
        let name = names.instances.get(index).map_or("?", |(name, _, _)| name.as_str());
        text.push_str(&format!("[instance {name} {} instantiations {} failed {}] ", names.asset_name(instance.asset), instance.instantiations, instance.failed));
    }
    if text.is_empty() { "an empty table".to_string() } else { text.trim_end().to_string() }
}

/// The state hash of a tick: the workload's hash and the table's, hashed together.
pub fn state_hash(component: u64, table_bytes: &[u8]) -> u64 {
    let mut hasher = Fnv1a::new();
    hasher.write(&component.to_le_bytes());
    hasher.write(&fnv1a(table_bytes).to_le_bytes());
    hasher.finish()
}

/// Compares a run with an oracle, tick by tick, to tick `to`. `sequential`
/// holds the workload's hashes of the sequential run the oracle's tables
/// ask for, from tick 0. Returns every difference in words; none means the
/// run equals the oracle.
pub fn compare(expected: &Expected, observed: &Observed, to: u32, sequential: &[u64], names: &Names) -> Vec<String> {
    let mut differences = Vec::new();
    if let Some((before, awaited)) = &observed.stuck {
        differences.push(format!("the world never ran tick {before}: gated on {}", list_text(awaited, names)));
    }
    if expected.restore != observed.verdict && (expected.restore.is_some() || observed.verdict.is_some()) {
        differences.push(format!("the restore: the oracle states {:?}, the run gave {:?}", expected.restore, observed.verdict));
    }
    if expected.blocked != observed.blocked {
        let text = |blocked: &[(u32, Vec<AssetId>)]| {
            blocked.iter().map(|(before, awaited)| format!("before tick {before} awaiting {}", list_text(awaited, names))).collect::<Vec<_>>().join("; ")
        };
        differences.push(format!("the steps that ran no tick: the oracle states [{}], the run gave [{}]", text(&expected.blocked), text(&observed.blocked)));
    }
    if let Some(block) = expected.blocks.range(..=observed.first).next_back().map(|(_, block)| block)
        && block.table_bytes() != observed.creation
    {
        let (stated, given) = (table_text(&block.table_bytes(), names), table_text(&observed.creation, names));
        differences.push(format!("the table at creation: the oracle states {stated}, the run gave {given}"));
    }
    let last_ran = observed.first + observed.tables.len() as u32;
    for tick in observed.first + 1..=to.min(last_ran) {
        let at = (tick - observed.first) as usize;
        let empty = Block::default();
        let block = expected.blocks.get(&tick).unwrap_or(&empty);
        let record = observed.log.iter().find(|record| record.tick == tick).map(elements_of).unwrap_or_default();
        if record != block.record {
            let text = |elements: &[Element]| elements.iter().map(|element| element_text(element, names)).collect::<Vec<_>>().join("; ");
            differences.push(format!("tick {tick}, the record: the oracle states [{}], the run gave [{}]", text(&block.record), text(&record)));
        }
        let events: Vec<Event> = observed.events.iter().filter(|(at, _)| *at == tick).map(|(_, event)| event.clone()).collect();
        if events != block.events {
            let text = |events: &[Event]| events.iter().map(|event| event_text(event, names)).collect::<Vec<_>>().join("; ");
            differences.push(format!("tick {tick}, the events: the oracle states [{}], the run gave [{}]", text(&block.events), text(&events)));
        }
        let named: Vec<(AssetId, u32)> = observed.names_failed.iter().filter(|(at, _, _)| *at == tick).map(|(_, asset, code)| (*asset, *code)).collect();
        if named != block.names_failed {
            let text = |named: &[(AssetId, u32)]| named.iter().map(|(asset, code)| format!("{} {}", names.asset_name(*asset), reason_name(*code))).collect::<Vec<_>>().join("; ");
            differences.push(format!("tick {tick}, what the step names as failed: the oracle states [{}], the run gave [{}]", text(&block.names_failed), text(&named)));
        }
        let stated = expected.table_at(tick).map_or_else(|| observed.creation.clone(), Block::table_bytes);
        let given = &observed.tables[at - 1];
        if stated != *given {
            differences.push(format!("tick {tick}, the table: the oracle states {}, the run gave {}", table_text(&stated, names), table_text(given, names)));
        }
        let hash = state_hash(sequential[tick as usize], &stated);
        if hash != observed.state[at] {
            differences.push(format!("tick {tick}, the state hash: the oracle gives {hash:016x}, the run gave {:016x}", observed.state[at]));
        }
    }
    differences
}

/// The stated entries of canonical table bytes: what stands before an oracle's first block.
pub fn stated_entries(bytes: &[u8]) -> Vec<StatedEntry> {
    let table = Table::decode(bytes, Rules::default()).expect("a table the build wrote decodes");
    table.entries.into_iter().map(|(id, entry)| (id, entry.availability, entry.wants)).collect()
}
