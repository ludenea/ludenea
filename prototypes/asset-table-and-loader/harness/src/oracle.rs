//! The oracles written by hand, as the harness reads them, and the comparison
//! of a run with one. Throwaway, never product code.
//!
//! An oracle is a table of states and not a second implementation: per named
//! tick, the elements of the tick's input record, the whole asset table with
//! every entry's type and every want in its stated order, the events, the
//! instances, and what a step names. The harness writes each stated table
//! through the canonical form, in the order the oracle gives, and compares
//! bytes; the state hash it expects for a tick is the hash of those bytes
//! with the workload's hash of a sequential run that reads, per asset, what
//! the stated table gives.

use std::collections::BTreeMap;

use prototype_core::table::{
    Arrival, AssetId, AssetType, Availability, Command, Event, Instance, Maker, NewHistory, Outcome, Owner, Policy, Record, Refused, RepulsionTable, Rules,
    SystemName, Table, WorldEvent, put_entry, put_instances, reason as listed_reason, reason_named,
};
use prototype_core::world::{Act, Holder};
use reference_workload::{Fnv1a, fnv1a};

use crate::sequential::Switch;

/// The scripted system of the same-harness sheet, `wanter`: its stable name,
/// which the stand-in sheet gives, and the contributor that brings it. A want
/// it makes under the world's own entry is its own: the system is its maker.
pub const SCRIPTED_SYSTEM: &str = "wanter";
pub const SCRIPTED_SYSTEM_CONTRIBUTOR: &str = "contributor:logic";

/// The makers the sheets name, each with its numeric identity. A source and
/// a contributor may share an identity: their kinds tell them apart.
const MAKERS: [(&str, Maker); 5] = [
    ("contributor:base", Maker::Contributor(7)),
    ("contributor:logic", Maker::Contributor(9)),
    ("source:host", Maker::Source(3)),
    ("source:tool", Maker::Source(5)),
    ("source:twin", Maker::Source(7)),
];

/// The names a family's scenarios and oracles share.
#[derive(Clone, Default)]
pub struct Names {
    pub assets: BTreeMap<String, AssetId>,
    pub hashes: BTreeMap<String, u64>,
    /// The size the domain manifest gives each content, by content hash: the
    /// length of its canonical part, no prepared form existing. It is the
    /// size an arrival names and an entry holds where a sheet states none.
    pub sizes: BTreeMap<u64, u64>,
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

    fn owner(&self, text: &str) -> Result<Owner, String> {
        if let Some((_, maker)) = MAKERS.iter().find(|(name, _)| *name == text) {
            return Ok(Owner::World(*maker));
        }
        let (kind, name) = text.split_once(':').ok_or_else(|| format!("an owner is <kind>:<name>, not {text}"))?;
        match kind {
            "entity" => self.entity(name).map(|(slot, generation)| Owner::Entity { slot, generation }),
            "asset" => self.asset(name).map(Owner::Asset),
            // A system is the maker of the want it makes, known by its stable name.
            "system" if !name.is_empty() => Ok(Owner::World(Maker::System(SystemName::new(name)))),
            "instance" => {
                let at = self.instances.iter().position(|(other, _, _)| other == name).ok_or_else(|| format!("unknown instance {name}"))?;
                Ok(Owner::World(Maker::Instance(at as u32 + 1)))
            }
            _ => Err(format!("unknown maker {text}")),
        }
    }

    fn entity(&self, name: &str) -> Result<(u32, u32), String> {
        let name = name.strip_prefix("entity:").unwrap_or(name);
        self.entities.get(name).copied().ok_or_else(|| format!("unknown entity {name}"))
    }

    /// The size the manifest gives a content: what an arrival to its hash
    /// names, and what an entry in force at that hash holds.
    pub fn size(&self, hash: u64) -> u64 {
        self.sizes.get(&hash).copied().unwrap_or(0)
    }

    /// The size in the words of a statement: nothing where it is the size the
    /// manifest gives the content, which a statement need not name.
    fn size_text(&self, hash: u64, size: u64) -> String {
        if size == self.size(hash) { String::new() } else { format!(" size {size}") }
    }

    /// The numeric identity of a contributor the sheets name.
    pub fn contributor(&self, text: &str) -> Result<u64, String> {
        match MAKERS.iter().find(|(name, _)| *name == text) {
            Some((_, Maker::Contributor(identity))) => Ok(*identity),
            _ => Err(format!("unknown contributor {text}")),
        }
    }

    fn owner_name(&self, owner: &Owner) -> String {
        match owner {
            Owner::World(Maker::Instance(index)) => {
                format!("instance:{}", self.instances.get(*index as usize - 1).map_or("?", |(name, _, _)| name.as_str()))
            }
            Owner::World(Maker::System(name)) => format!("system:{}", name.as_str()),
            Owner::World(maker) => MAKERS.iter().find(|(_, other)| other == maker).map_or_else(|| format!("{maker:?}"), |(name, _)| (*name).to_string()),
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

/// An asset type in the words of an oracle: `parameters` for the first
/// synthetic type, `second` for the second, `third` for the one of which no
/// asset exists.
pub fn asset_type(text: &str) -> Result<AssetType, String> {
    match text {
        "parameters" => Ok(AssetType::Parameters),
        "second" => Ok(AssetType::Second),
        "third" => Ok(AssetType::Third),
        other => Err(format!("unknown asset type {other}")),
    }
}

pub fn type_name(asset_type: AssetType) -> &'static str {
    match asset_type {
        AssetType::Parameters => "parameters",
        AssetType::Second => "second",
        AssetType::Third => "third",
    }
}

/// A reason of a failure, by the name the list of reasons gives it.
fn reason(text: &str) -> Result<u32, String> {
    reason_named(text).map(|reason| reason.code).ok_or_else(|| format!("unknown reason {text}"))
}

fn reason_name(code: u32) -> &'static str {
    listed_reason(code).map_or("another-reason", |reason| reason.name)
}

/// A reason of a recorded refusal.
fn refusal(text: &str) -> Result<Refused, String> {
    Refused::ALL.into_iter().find(|refused| refused.name() == text).ok_or_else(|| format!("unknown reason of a refusal {text}"))
}

/// One element of an input record.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Element {
    /// A world event of the host: it sets a field of the configuration.
    Event(WorldEvent),
    Command(Command),
    Arrival(Arrival),
}

/// Reads one element, in the words the scenario sheets and the oracles share.
/// An arrival to a hash names the size the domain manifest gives its
/// content, the length of its part, but where the statement names another:
/// `size <bytes>` stands after the hash, before `marked` and before a root.
pub fn element(text: &str, names: &Names) -> Result<Element, String> {
    let words: Vec<&str> = text.split_whitespace().collect();
    Ok(match words.as_slice() {
        ["want", asset, kind, owner, level] => Element::Command(Command::Want {
            asset: names.asset(asset)?,
            asset_type: asset_type(kind)?,
            owner: names.owner(owner)?,
            policy: policy(level)?,
        }),
        ["budget", bytes] => Element::Event(WorldEvent::Budget(bytes.parse().map_err(|_| format!("not a budget in bytes: {bytes}"))?)),
        ["release", asset, owner] => Element::Command(Command::Release { asset: names.asset(asset)?, owner: names.owner(owner)? }),
        ["despawn", entity] => {
            let (slot, generation) = names.entity(entity)?;
            Element::Command(Command::Despawn { slot, generation })
        }
        ["instantiate", _, asset, kind, level, "reload", reload] => Element::Command(Command::Instantiate {
            asset: names.asset(asset)?,
            asset_type: asset_type(kind)?,
            policy: policy(level)?,
            reload: *reload == "on",
        }),
        ["arrival", asset, "failed", code] => Element::Arrival(Arrival::failed(names.asset(asset)?, reason(code)?)),
        ["arrival", asset, "failed", code, "marked"] => Element::Arrival(Arrival::failed(names.asset(asset)?, reason(code)?).marked()),
        ["arrival", asset, hash, rest @ ..] => {
            let hash = names.hash(hash)?;
            let (size, rest) = match rest {
                ["size", bytes, rest @ ..] => (bytes.parse().map_err(|_| format!("not a size: {bytes}"))?, rest),
                rest => (names.size(hash), rest),
            };
            let arrival = Arrival::at(names.asset(asset)?, hash).sized(size);
            Element::Arrival(match rest {
                [] => arrival,
                ["marked"] => arrival.marked(),
                ["for", root] => arrival.for_root(names.asset(root)?),
                _ => return Err(format!("not an element: {text}")),
            })
        }
        _ => return Err(format!("not an element: {text}")),
    })
}

/// What a system does, in the words of the sheet: an element in which a
/// want under the world's own entry names `world` as its owner. The maker
/// such a want carries is the system itself, which the world gives it.
pub fn system_act(text: &str, names: &Names) -> Result<Act, String> {
    let holder = |text: &str| match text {
        "world" => Ok(Holder::World),
        entity => names.entity(entity).map(|(slot, generation)| Holder::Entity { slot, generation }),
    };
    let words: Vec<&str> = text.split_whitespace().collect();
    Ok(match words.as_slice() {
        ["want", asset, kind, owner, level] => Act::Want { asset: names.asset(asset)?, asset_type: asset_type(kind)?, holder: holder(owner)?, policy: policy(level)? },
        ["release", asset, owner] => Act::Release { asset: names.asset(asset)?, holder: holder(owner)? },
        ["despawn", entity] => {
            let (slot, generation) = names.entity(entity)?;
            Act::Despawn { slot, generation }
        }
        _ => return Err(format!("not what a system does: {text}")),
    })
}

fn element_text(element: &Element, names: &Names) -> String {
    match element {
        Element::Command(Command::Want { asset, asset_type, owner, policy }) => {
            format!("want {} {} {} {}", names.asset_name(*asset), type_name(*asset_type), names.owner_name(owner), policy_name(*policy))
        }
        Element::Event(WorldEvent::Budget(bytes)) => format!("budget {bytes}"),
        Element::Command(Command::Release { asset, owner }) => format!("release {} {}", names.asset_name(*asset), names.owner_name(owner)),
        Element::Command(Command::Despawn { slot, generation }) => names.owner_name(&Owner::Entity { slot: *slot, generation: *generation }).replace("entity:", "despawn "),
        Element::Command(Command::Instantiate { asset, asset_type, policy, reload }) => {
            let reload = if *reload { "on" } else { "off" };
            format!("instantiate {} {} {} reload {reload}", names.asset_name(*asset), type_name(*asset_type), policy_name(*policy))
        }
        Element::Arrival(arrival) => {
            let outcome = match arrival.outcome {
                Outcome::Hash(hash) => format!("{}{}", names.hash_name(hash), names.size_text(hash, arrival.size)),
                Outcome::Failure(code) => format!("failed {}", reason_name(code)),
            };
            let marked = if arrival.marked { " marked" } else { "" };
            let root = arrival.root.map_or_else(String::new, |root| format!(" for {}", names.asset_name(root)));
            format!("arrival {} {outcome}{marked}{root}", names.asset_name(arrival.asset))
        }
    }
}

fn elements_of(record: &Record) -> Vec<Element> {
    // A world event stands at the boundary of its tick, before the commands;
    // the arrivals come last, the availability source being ordered after
    // every other.
    let events = record.events.iter().map(|event| Element::Event(*event));
    let commands = record.commands.iter().map(|command| Element::Command(*command));
    events.chain(commands).chain(record.arrivals.iter().map(|arrival| Element::Arrival(*arrival))).collect()
}

fn new_history_text(named: &NewHistory, names: &Names) -> String {
    let mut text = "new-history".to_string();
    for (word, list) in [("differed", &named.differed), ("unanswered", &named.unanswered), ("dropped", &named.dropped)] {
        if !list.is_empty() {
            text.push_str(&format!(" {word} {}", list_text(list, names)));
        }
    }
    text
}

fn event_text(event: &Event, names: &Names) -> String {
    match event {
        Event::Arrived { asset, hash } => format!("arrived {} {}", names.asset_name(*asset), names.hash_name(*hash)),
        Event::HashChanged { asset, from, to } => format!("changed {} {} {}", names.asset_name(*asset), names.hash_name(*from), names.hash_name(*to)),
        Event::Failed { asset, code } => format!("failed {} {}", names.asset_name(*asset), reason_name(*code)),
        Event::NewHistory(named) => new_history_text(named, names),
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

/// The up to three lists of the event that records a new history, in their
/// order, each left out where it is empty: `differed`, `unanswered`, `dropped`.
fn new_history(text: &str, names: &Names) -> Result<NewHistory, String> {
    let (mut differed, mut unanswered, mut dropped) = (Vec::new(), Vec::new(), Vec::new());
    let mut into = 0;
    for word in text.split_whitespace() {
        match word {
            "differed" => into = 1,
            "unanswered" => into = 2,
            "dropped" => into = 3,
            asset => {
                let id = names.asset(asset.trim_end_matches(','))?;
                match into {
                    1 => differed.push(id),
                    2 => unanswered.push(id),
                    3 => dropped.push(id),
                    _ => return Err(format!("{asset} stands in no list of a new history")),
                }
            }
        }
    }
    for list in [&mut differed, &mut unanswered, &mut dropped] {
        list.sort_unstable();
    }
    Ok(NewHistory { differed, unanswered, dropped })
}

/// The outcome of a restore.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Verdict {
    Continues,
    NewHistory(NewHistory),
    /// Refused by the name of one asset type.
    RefusedType(AssetType),
    /// Refused by two: the type the request names as dropped, then the type
    /// of the kept entry that lists a dependency of it.
    RefusedTypes(AssetType, AssetType),
    /// Refused under the recorded content: the asset and the recorded hash
    /// that could not be confirmed.
    RefusedHash(AssetId, u64),
}

impl Verdict {
    pub fn refused(&self) -> bool {
        !matches!(self, Verdict::Continues | Verdict::NewHistory(_))
    }

    pub fn text(&self, names: &Names) -> String {
        match self {
            Verdict::Continues => "continues".to_string(),
            Verdict::NewHistory(named) => new_history_text(named, names),
            Verdict::RefusedType(refused) => format!("refused type {}", type_name(*refused)),
            Verdict::RefusedTypes(dropped, kept) => format!("refused types {} {}", type_name(*dropped), type_name(*kept)),
            Verdict::RefusedHash(asset, hash) => format!("refused hash {} {}", names.asset_name(*asset), names.hash_name(*hash)),
        }
    }
}

fn verdict(text: &str, names: &Names) -> Result<Verdict, String> {
    let words: Vec<&str> = text.split_whitespace().collect();
    Ok(match words.as_slice() {
        ["continues"] => Verdict::Continues,
        ["new-history", ..] => Verdict::NewHistory(new_history(text.trim_start_matches("new-history"), names)?),
        ["refused", "type", refused] => Verdict::RefusedType(asset_type(refused)?),
        ["refused", "types", dropped, kept] => Verdict::RefusedTypes(asset_type(dropped)?, asset_type(kept)?),
        ["refused", "hash", asset, hash] => Verdict::RefusedHash(names.asset(asset)?, names.hash(hash)?),
        _ => return Err(format!("unknown verdict {text}")),
    })
}

/// One entry as an oracle states it: the asset, the type its entry holds,
/// its availability, the size it holds where it is available, and its wants
/// in the order stated.
pub type StatedEntry = (AssetId, AssetType, Availability, u64, Vec<(Owner, Policy)>);

/// The statement of one tick.
#[derive(Clone, Debug, Default)]
pub struct Block {
    pub record: Vec<Element>,
    /// The elements of the record that are recorded refusals: the place of
    /// each in the record, and the reason.
    pub refusals: Vec<(usize, Refused)>,
    /// The arrivals rejected over the world's budget, in the order applied.
    pub rejected: Vec<(AssetId, u64)>,
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
        for (id, asset_type, availability, size, wants) in &self.entries {
            put_entry(&mut out, *id, *asset_type, *availability, *size, wants);
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

    /// The same statement with every tick number raised by `k`. The table a
    /// creation gives is stated at tick 0 whatever the shift.
    fn shifted(&self, k: u32) -> Expected {
        Expected {
            restore: self.restore.clone(),
            blocked: self.blocked.iter().map(|(before, awaited)| (before + k, awaited.clone())).collect(),
            blocks: self.blocks.iter().map(|(tick, block)| (if *tick == 0 { 0 } else { tick + k }, block.clone())).collect(),
        }
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
                let table = entries.iter().find(|(id, _, _, _, _)| id == asset).and_then(|(_, _, availability, _, _)| match availability {
                    Availability::Available(hash) => names.tables.get(hash).copied(),
                    _ => None,
                });
                if table != held[index] || tick == from {
                    switches.push(Switch { tick, asset: index, table });
                    held[index] = table;
                }
            }
            if let Some(block) = self.blocks.get(&tick) {
                for (index, element) in block.record.iter().enumerate() {
                    // A despawn that is a recorded refusal takes no entity out.
                    let refused = block.refusals.iter().any(|(at, _)| *at == index);
                    if let Element::Command(Command::Despawn { slot, .. }) = element
                        && !refused
                    {
                        kills.push((tick, *slot as usize));
                    }
                }
            }
        }
        (switches, kills)
    }
}

/// What an oracle file states: per scenario, per world.
pub type Stated = BTreeMap<String, BTreeMap<String, Expected>>;

/// Reads an oracle file: its scenarios, each with its worlds.
pub fn parse(text: &str, names: &Names) -> Result<Stated, String> {
    let mut scenarios: Stated = BTreeMap::new();
    // The worlds stated as another world shifted: scenario, world, other, shift.
    let mut shifts: Vec<(String, String, String, u32)> = Vec::new();
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
            let words: Vec<&str> = rest.split_whitespace().collect();
            match words.as_slice() {
                [name] => world = (*name).to_string(),
                [name, "as", other, "shifted", k] => {
                    let k = k.parse().map_err(|_| fail(format!("not a shift: {k}")))?;
                    shifts.push((scenario.clone(), (*name).to_string(), (*other).to_string(), k));
                    world = (*name).to_string();
                }
                _ => return Err(fail(format!("not a world: {rest}"))),
            }
            tick = None;
            continue;
        }
        let expected = scenarios.get_mut(&scenario).ok_or_else(|| fail("a statement before any scenario".to_string()))?.entry(world.clone()).or_default();
        match keyword {
            "restore" => expected.restore = Some(verdict(rest, names).map_err(fail)?),
            "blocked" => {
                let (before, awaited) = rest.split_once(" awaiting").ok_or_else(|| fail("blocked <n> awaiting <assets>".to_string()))?;
                let before = before.trim().parse().map_err(|_| fail(format!("not a tick: {before}")))?;
                expected.blocked.push((before, list(awaited, names).map_err(fail)?));
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
                    ("record", _) => block.record.push(element(rest, names).map_err(fail)?),
                    ("refusal", [named]) => {
                        let at = block.record.len().checked_sub(1).ok_or_else(|| fail("a refusal before any element of the record".to_string()))?;
                        block.refusals.push((at, refusal(named).map_err(fail)?));
                    }
                    ("rejected", [asset, hash]) => block.rejected.push((names.asset(asset).map_err(fail)?, names.hash(hash).map_err(fail)?)),
                    ("entry", [asset, kind, held @ ..]) => {
                        // An entry in force holds the size the manifest gives its
                        // content, but where the statement names another.
                        let (availability, size) = match held {
                            ["unavailable"] => (Availability::Unavailable, 0),
                            ["available", hash] => {
                                let hash = names.hash(hash).map_err(fail)?;
                                (Availability::Available(hash), names.size(hash))
                            }
                            ["available", hash, "size", bytes] => {
                                (Availability::Available(names.hash(hash).map_err(fail)?), bytes.parse().map_err(|_| fail(format!("not a size: {bytes}")))?)
                            }
                            ["failed", code] => (Availability::Failed(reason(code).map_err(fail)?), 0),
                            _ => return Err(fail(format!("not an availability: {line}"))),
                        };
                        block.entries.push((names.asset(asset).map_err(fail)?, asset_type(kind).map_err(fail)?, availability, size, Vec::new()));
                    }
                    ("want", [owner, level]) => {
                        let want = (names.owner(owner).map_err(fail)?, policy(level).map_err(fail)?);
                        block.entries.last_mut().ok_or_else(|| fail("a want before any entry".to_string()))?.4.push(want);
                    }
                    ("event", ["arrived", asset, hash]) => {
                        block.events.push(Event::Arrived { asset: names.asset(asset).map_err(fail)?, hash: names.hash(hash).map_err(fail)? });
                    }
                    ("event", ["changed", asset, from, to]) => block.events.push(Event::HashChanged {
                        asset: names.asset(asset).map_err(fail)?,
                        from: names.hash(from).map_err(fail)?,
                        to: names.hash(to).map_err(fail)?,
                    }),
                    ("event", ["failed", asset, code]) => {
                        block.events.push(Event::Failed { asset: names.asset(asset).map_err(fail)?, code: reason(code).map_err(fail)? });
                    }
                    ("event", ["new-history", ..]) => {
                        let named = rest.strip_prefix("new-history").unwrap_or("");
                        block.events.push(Event::NewHistory(new_history(named, names).map_err(fail)?));
                    }
                    ("instance", [name, asset, "instantiations", count, "failed", failed]) => {
                        let (_, policy, reload) =
                            names.instances.iter().find(|(other, _, _)| other == name).ok_or_else(|| fail(format!("unknown instance {name}")))?;
                        block.instances.push(Instance {
                            asset: names.asset(asset).map_err(fail)?,
                            policy: *policy,
                            reload: *reload,
                            instantiations: count.parse().map_err(|_| fail(format!("not a count: {count}")))?,
                            failed: *failed == "yes",
                        });
                    }
                    ("names-failed", [asset, code]) => block.names_failed.push((names.asset(asset).map_err(fail)?, reason(code).map_err(fail)?)),
                    _ => return Err(fail(format!("not a statement: {line}"))),
                }
            }
        }
    }
    for (scenario, world, other, k) in shifts {
        let worlds = scenarios.get_mut(&scenario).ok_or_else(|| format!("{world}: stated as another world of no scenario"))?;
        let shifted = worlds.get(&other).ok_or_else(|| format!("{scenario}: {world} is stated as {other}, which no statement holds"))?.shifted(k);
        worlds.insert(world, shifted);
    }
    Ok(scenarios)
}

/// Reads an oracle file scenario by scenario, so that a statement the
/// harness cannot read costs its scenario alone: what reads, and one line
/// per scenario that does not, with the line of the file.
pub fn parse_each(text: &str, names: &Names) -> (Stated, Vec<String>) {
    let lines: Vec<&str> = text.lines().collect();
    let starts: Vec<usize> = lines.iter().enumerate().filter(|(_, line)| line.trim_start().starts_with("scenario ")).map(|(at, _)| at).collect();
    let (mut stated, mut unread) = (Stated::new(), Vec::new());
    for (index, start) in starts.iter().enumerate() {
        let end = starts.get(index + 1).copied().unwrap_or(lines.len());
        // The other scenarios are blanked, so that a line keeps its number.
        let alone: Vec<&str> = lines.iter().enumerate().map(|(at, line)| if (*start..end).contains(&at) { *line } else { "" }).collect();
        match parse(&alone.join("\n"), names) {
            Ok(scenarios) => stated.extend(scenarios),
            Err(error) => unread.push(format!("the statement of the {} does not read: {error}", lines[*start].trim())),
        }
    }
    (stated, unread)
}

/// What a run gave, for one world.
#[derive(Clone, Debug, Default)]
pub struct Observed {
    /// The tick the world starts from: 0, or the tick of the save it was restored from.
    pub first: u32,
    /// The canonical bytes of the table at creation.
    pub creation: Vec<u8>,
    /// The entries of the table at creation, read from the bytes that hold
    /// the type of each, which a control may leave out of the others.
    pub creation_entries: Vec<StatedEntry>,
    /// The canonical bytes of the table after each tick that ran, from `first + 1`.
    pub tables: Vec<Vec<u8>>,
    /// The state hashes from `first`.
    pub state: Vec<u64>,
    pub log: Vec<Record>,
    pub events: Vec<(u32, Event)>,
    pub blocked: Vec<(u32, Vec<AssetId>)>,
    pub names_failed: Vec<(u32, AssetId, u32)>,
    /// The recorded refusals: the tick, the place in its record, the reason.
    pub refusals: Vec<(u32, usize, Refused)>,
    /// The arrivals a step named as rejected over the budget.
    pub rejected: Vec<(u32, AssetId, u64)>,
    pub verdict: Option<Verdict>,
    /// The tick the world never ran, where it stayed gated to the end.
    pub stuck: Option<(u32, Vec<AssetId>)>,
}

/// A table's canonical bytes, in words, for a difference a reader arbitrates.
pub fn table_text(bytes: &[u8], names: &Names) -> String {
    let Ok(table) = Table::decode(bytes, Rules::default()) else { return "bytes that are not a table with the type of each entry".to_string() };
    let mut text = String::new();
    for (id, entry) in &table.entries {
        let availability = match entry.availability {
            Availability::Unavailable => "unavailable".to_string(),
            Availability::Available(hash) => format!("available {}{}", names.hash_name(hash), names.size_text(hash, entry.size)),
            Availability::Failed(code) => format!("failed {}", reason_name(code)),
        };
        let wants: Vec<String> = entry.wants.iter().map(|(owner, policy)| format!("{} {}", names.owner_name(owner), policy_name(*policy))).collect();
        text.push_str(&format!("[{} {} {availability}: {}] ", names.asset_name(*id), type_name(entry.asset_type), wants.join(", ")));
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
        let text = |verdict: &Option<Verdict>| verdict.as_ref().map_or_else(|| "none".to_string(), |verdict| verdict.text(names));
        differences.push(format!("the restore: the oracle states [{}], the run gave [{}]", text(&expected.restore), text(&observed.verdict)));
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
        let refusals: Vec<(usize, Refused)> = observed.refusals.iter().filter(|(at, _, _)| *at == tick).map(|(_, index, refused)| (*index, *refused)).collect();
        if refusals != block.refusals {
            let text = |refusals: &[(usize, Refused)]| refusals.iter().map(|(index, refused)| format!("element {} {}", index + 1, refused.name())).collect::<Vec<_>>().join("; ");
            differences.push(format!("tick {tick}, the recorded refusals: the oracle states [{}], the run gave [{}]", text(&block.refusals), text(&refusals)));
        }
        let rejected: Vec<(AssetId, u64)> = observed.rejected.iter().filter(|(at, _, _)| *at == tick).map(|(_, asset, hash)| (*asset, *hash)).collect();
        if rejected != block.rejected {
            let text = |rejected: &[(AssetId, u64)]| rejected.iter().map(|(asset, hash)| format!("{} {}", names.asset_name(*asset), names.hash_name(*hash))).collect::<Vec<_>>().join("; ");
            differences.push(format!("tick {tick}, the arrivals rejected over the budget: the oracle states [{}], the run gave [{}]", text(&block.rejected), text(&rejected)));
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

/// What a run gave for one world, in words, for the log: the verdict of its
/// restore, the steps that ran no tick, the table at creation and, per tick
/// at which the record is not empty, an event is emitted, a step names
/// something or the table changes, what the tick gave.
pub fn observed_text(observed: &Observed, names: &Names) -> Vec<String> {
    let mut lines = Vec::new();
    if let Some(verdict) = &observed.verdict {
        lines.push(format!("restore {}", verdict.text(names)));
    }
    for (before, awaited) in &observed.blocked {
        lines.push(format!("blocked {before} awaiting {}", list_text(awaited, names)));
    }
    if let Some((before, awaited)) = &observed.stuck {
        lines.push(format!("never ran tick {before}: gated on {}", list_text(awaited, names)));
    }
    // A restore that is refused gives no world, and so no table.
    if !observed.creation.is_empty() {
        lines.push(format!("tick {}: {}", observed.first, table_text(&observed.creation, names)));
    }
    let mut held = &observed.creation;
    for (index, table) in observed.tables.iter().enumerate() {
        let tick = observed.first + 1 + index as u32;
        let record = observed.log.iter().find(|record| record.tick == tick).map(elements_of).unwrap_or_default();
        let refusals: Vec<String> = observed.refusals.iter().filter(|(at, _, _)| *at == tick).map(|(_, at, refused)| format!("element {} {}", at + 1, refused.name())).collect();
        let rejected: Vec<String> =
            observed.rejected.iter().filter(|(at, _, _)| *at == tick).map(|(_, asset, hash)| format!("{} {}", names.asset_name(*asset), names.hash_name(*hash))).collect();
        let events: Vec<String> = observed.events.iter().filter(|(at, _)| *at == tick).map(|(_, event)| event_text(event, names)).collect();
        let named: Vec<String> =
            observed.names_failed.iter().filter(|(at, _, _)| *at == tick).map(|(_, asset, code)| format!("{} {}", names.asset_name(*asset), reason_name(*code))).collect();
        if record.is_empty() && refusals.is_empty() && rejected.is_empty() && events.is_empty() && named.is_empty() && table == held {
            continue;
        }
        held = table;
        let mut line = format!("tick {tick}:");
        let lists = [("record", record.iter().map(|element| element_text(element, names)).collect::<Vec<_>>()), ("refusals", refusals), ("rejected", rejected), ("events", events), ("names-failed", named)];
        for (word, list) in lists {
            if !list.is_empty() {
                line.push_str(&format!(" {word} [{}]", list.join("; ")));
            }
        }
        line.push_str(&format!(" table {}", table_text(table, names)));
        lines.push(line);
    }
    lines
}

/// The stated entries of typed table bytes: what stands before an oracle's first block.
pub fn stated_entries(bytes: &[u8]) -> Vec<StatedEntry> {
    let table = Table::decode(bytes, Rules::default()).expect("a table the build wrote decodes");
    table.entries.into_iter().map(|(id, entry)| (id, entry.asset_type, entry.availability, entry.size, entry.wants)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names() -> Names {
        let mut names = Names::default();
        names.assets.insert("A".to_string(), 0x0a);
        names.assets.insert("P".to_string(), 0x30);
        names.hashes.insert("A.1".to_string(), 0x11);
        names.sizes.insert(0x11, 640);
        names.entities.insert("E1".to_string(), (4, 0));
        names.instances.push(("one".to_string(), Policy::Required, true));
        names
    }

    /// The format of an oracle as the harness reads it, on statements
    /// written here: the test reads no oracle file.
    #[test]
    fn a_statement_reads_with_the_type_of_each_entry_and_of_each_want() {
        let names = names();
        let host = Owner::World(Maker::Source(3));
        let want = element("want A second source:host required", &names).expect("an element");
        assert_eq!(want, Element::Command(Command::Want { asset: 0x0a, asset_type: AssetType::Second, owner: host, policy: Policy::Required }));
        assert_eq!(element_text(&want, &names), "want A second source:host required");
        let instantiate = element("instantiate one P parameters optional reload off", &names).expect("an element");
        let stated = Command::Instantiate { asset: 0x30, asset_type: AssetType::Parameters, policy: Policy::Optional, reload: false };
        assert_eq!(instantiate, Element::Command(stated));
        assert!(element("want A source:host required", &names).is_err(), "a want states a type");
        let failed = element("arrival A failed other-type marked", &names).expect("an element");
        assert_eq!(element_text(&failed, &names), "arrival A failed other-type marked");
        let rooted = element("arrival A A.1 for P", &names).expect("an element");
        assert_eq!(rooted, Element::Arrival(Arrival::at(0x0a, 0x11).sized(640).for_root(0x30)), "an arrival names the size the manifest gives its content");
        assert_eq!(element_text(&rooted, &names), "arrival A A.1 for P");
        // A statement may name another size: after the hash, before `marked`.
        let sized = element("arrival A A.1 size 5 marked", &names).expect("an element");
        assert_eq!(sized, Element::Arrival(Arrival::at(0x0a, 0x11).sized(5).marked()));
        assert_eq!(element_text(&sized, &names), "arrival A A.1 size 5 marked");
        assert!(element("arrival A A.1 marked size 5", &names).is_err());
        let stated = parse("scenario s
tick 1
entry A parameters available A.1 size 7
", &names).expect("the statement reads");
        assert_eq!(stated["s"]["world"].blocks[&1].entries[0].3, 7);
        assert_eq!(table_text(&stated["s"]["world"].blocks[&1].table_bytes(), &names), "[A parameters available A.1 size 7: ]");
        let text = "scenario s\ntick 2\nrecord want A parameters entity:E1 optional\nrecord want A third source:twin optional\nrefusal other-type\n\
                    rejected A A.1\nentry A parameters available A.1\nwant entity:E1 optional\nentry P second failed over-budget\n\
                    event arrived A A.1\ninstance one P instantiations 0 failed no\n";
        let stated = parse(text, &names).expect("the statement reads");
        let block = &stated["s"]["world"].blocks[&2];
        let entity = Owner::Entity { slot: 4, generation: 0 };
        assert_eq!(block.entries[0], (0x0a, AssetType::Parameters, Availability::Available(0x11), 640, vec![(entity, Policy::Optional)]), "an entry in force holds the size the manifest gives");
        assert_eq!(block.entries[1].1, AssetType::Second);
        assert_eq!(block.refusals, vec![(1, Refused::AnotherType)]);
        assert_eq!(block.rejected, vec![(0x0a, 0x11)]);
        // The stated table goes through the canonical form the build writes.
        let table = Table::decode(&block.table_bytes(), Rules::default()).expect("the stated table decodes");
        assert_eq!(table.canonical_bytes(), block.table_bytes());
        assert_eq!(stated_entries(&block.table_bytes()), block.entries);
        assert_eq!(table_text(&block.table_bytes(), &names), "[A parameters available A.1: entity:E1 optional] [P second failed over-budget: ] [instance one P instantiations 0 failed false]");
        assert!(parse("scenario s\ntick 1\nentry A available A.1\n", &names).is_err(), "an entry states its type");
    }

    #[test]
    fn a_verdict_an_event_of_a_new_history_and_a_shifted_world_read() {
        let names = names();
        let text = "scenario r\nrestore new-history differed A unanswered A, P dropped P\ntick 9\nevent new-history unanswered P\n\
                    scenario t\nrestore refused types second parameters\nscenario h\nrestore refused hash A A.1\n\
                    scenario w\nworld first\nblocked 3 awaiting A\ntick 0\ntick 4\nentry A parameters unavailable\nworld second as first shifted 2\n";
        let stated = parse(text, &names).expect("the statement reads");
        let both = NewHistory { differed: vec![0x0a], unanswered: vec![0x0a, 0x30], dropped: vec![0x30] };
        assert_eq!(stated["r"]["world"].restore, Some(Verdict::NewHistory(both.clone())));
        assert_eq!(new_history_text(&both, &names), "new-history differed A unanswered A, P dropped P");
        let event = NewHistory { unanswered: vec![0x30], ..NewHistory::default() };
        assert_eq!(stated["r"]["world"].blocks[&9].events, vec![Event::NewHistory(event)]);
        assert_eq!(stated["t"]["world"].restore, Some(Verdict::RefusedTypes(AssetType::Second, AssetType::Parameters)));
        assert_eq!(stated["h"]["world"].restore, Some(Verdict::RefusedHash(0x0a, 0x11)));
        let shifted = &stated["w"]["second"];
        assert_eq!(shifted.blocked, vec![(5, vec![0x0a])]);
        assert_eq!(shifted.blocks.keys().copied().collect::<Vec<_>>(), vec![0, 6]);
        assert_eq!(shifted.blocks[&6].entries.len(), 1);
    }

    #[test]
    fn what_the_system_does_reads_in_the_words_of_the_sheet() {
        let names = names();
        let want = system_act("want A parameters world optional", &names).expect("a want");
        assert_eq!(want, Act::Want { asset: 0x0a, asset_type: AssetType::Parameters, holder: Holder::World, policy: Policy::Optional });
        assert_eq!(system_act("release A entity:E1", &names), Ok(Act::Release { asset: 0x0a, holder: Holder::Entity { slot: 4, generation: 0 } }));
        assert_eq!(system_act("despawn E1", &names), Ok(Act::Despawn { slot: 4, generation: 0 }));
        assert!(system_act("arrival A A.1", &names).is_err(), "a system posts no arrival");
        // A system is the maker of its want, known by its stable name.
        let spawner = Owner::World(Maker::System(SystemName::new("logic/spawner")));
        assert_eq!(names.owner("system:logic/spawner"), Ok(spawner));
        assert_eq!(names.owner_name(&spawner), "system:logic/spawner");
        assert_eq!(names.contributor(SCRIPTED_SYSTEM_CONTRIBUTOR), Ok(9));
        assert_eq!(names.owner("source:twin"), Ok(Owner::World(Maker::Source(7))));
        assert_eq!(names.owner_name(&Owner::World(Maker::Contributor(7))), "contributor:base");
    }

    #[test]
    fn a_statement_that_does_not_read_costs_its_scenario_alone() {
        let names = names();
        let text = "# a comment\nscenario one\ntick 1\nentry A parameters unavailable\nscenario two\ntick 1\nentry A sideways\nscenario three\nrestore continues\n";
        let (stated, unread) = parse_each(text, &names);
        assert_eq!(stated.keys().map(String::as_str).collect::<Vec<_>>(), vec!["one", "three"]);
        assert_eq!(unread.len(), 1);
        assert!(unread[0].contains("scenario two") && unread[0].contains("line 7"), "{}", unread[0]);
        // What a run gave is written in words, a tick that changes nothing left out.
        let observed = Observed { creation: Block::default().table_bytes(), tables: vec![Block::default().table_bytes(); 2], blocked: vec![(1, vec![0x0a])], ..Observed::default() };
        assert_eq!(observed_text(&observed, &names), vec!["blocked 1 awaiting A".to_string(), "tick 0: an empty table".to_string()]);
    }

    /// What the stand-in sheet adds to the format, on statements written
    /// here: the world event that sets a budget, a size after a hash, the
    /// reason of the budget, a system as an owner, several steps that ran no
    /// tick before one tick, and a world stated as another shifted by none.
    #[test]
    fn the_additions_of_the_stand_in_sheet_read() {
        let names = names();
        let budget = element("budget 8000", &names).expect("an element");
        assert_eq!(budget, Element::Event(WorldEvent::Budget(8000)));
        assert_eq!(element_text(&budget, &names), "budget 8000");
        assert!(element("budget", &names).is_err() && element("budget many", &names).is_err());
        let text = "scenario s\nworld recorded\nblocked 3 awaiting A\nblocked 3 awaiting A, P\ntick 3\nrecord budget 8000\nrecord arrival A A.1 size 4000\n\
                    rejected A A.1\nentry A parameters failed over-budget\nwant system:wanter optional\nevent failed A over-budget\nnames-failed A over-budget\n\
                    world replayed as recorded shifted 0\n";
        let stated = parse(text, &names).expect("the statement reads");
        let recorded = &stated["s"]["recorded"];
        assert_eq!(recorded.blocked, vec![(3, vec![0x0a]), (3, vec![0x0a, 0x30])]);
        let block = &recorded.blocks[&3];
        assert_eq!(block.record, vec![budget.clone(), Element::Arrival(Arrival::at(0x0a, 0x11).sized(4000))]);
        assert_eq!(block.entries[0].4, vec![(Owner::World(Maker::System(SystemName::new("wanter"))), Policy::Optional)]);
        assert_eq!(stated["s"]["replayed"].blocks[&3].record, block.record, "a world that replays is stated by the world whose log it replays");
        // Where a record is compared, its world events stand first, then its
        // commands, then its arrivals.
        let host = Owner::World(Maker::Source(3));
        let release = Command::Release { asset: 0x0a, owner: host };
        let record = Record { tick: 3, events: vec![WorldEvent::Budget(8000)], commands: vec![release], arrivals: vec![Arrival::at(0x0a, 0x11).sized(4000)] };
        assert_eq!(elements_of(&record), vec![budget, Element::Command(release), Element::Arrival(Arrival::at(0x0a, 0x11).sized(4000))]);
    }
}
