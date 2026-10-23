//! The per-entity divergence report: two simulations at the same tick, their entity hash
//! trees compared. Each entity that diverges is reported once, at its first divergent tick,
//! with its first differing field and the recorded events and commands that may have caused
//! it. The editor's branch report and SyncTest share it.

use std::collections::BTreeMap;

use ludenea_ecs::diff::entity_subject;
use ludenea_ecs::schema::TypeSchema;
use ludenea_ecs::schema::entities::entity_refs;
use ludenea_ecs::{
    CommandKind, CommandRecord, Difference, Entity, EntityHash, ScheduleObserver, Snapshot,
    SnapshotError, World,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::Simulation;

/// One entity, matched across the branches by identifier, at its first divergent tick.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct EntityDivergence {
    pub entity: Entity,
    pub kind: DivergenceKind,
    pub first_tick: u64,
    /// The first differing component and field of a `Changed` entity.
    pub first_difference: Option<Difference>,
    /// Empty when no recorded event or command links the entity to an already divergent
    /// one, such as a resting contact: touching bodies push each other without an event.
    pub causes: Vec<Cause>,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(rename_all = "snake_case")]
pub enum DivergenceKind {
    Changed,
    OnlyInCurrent,
    OnlyInCompare,
    /// Alive in both with different component sets: likely unrelated entities that got the
    /// same identifier.
    Replaced,
}

/// A cause candidate, in either branch, at the entity's first divergent tick or the tick
/// before. Only events and commands are recorded, so interactions without either (a
/// persistent contact, a system reading another entity) leave no candidate.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Cause {
    /// An event by its stable name that references the entity at `path` and `other`, an
    /// entity already divergent at `tick`.
    Event {
        tick: u64,
        event: String,
        path: String,
        other: Entity,
    },
    /// A command targeting the entity, `kind` being `spawn`, `insert`, `remove`, or
    /// `despawn`, issued by `system`.
    Command {
        tick: u64,
        system: String,
        kind: String,
    },
}

/// The events and commands of the tick that ended at `tick`, in one simulation.
#[derive(Debug, Default)]
pub struct TickRecord {
    pub tick: u64,
    /// Each event by its stable name, as JSON.
    pub events: Vec<(&'static str, Value)>,
    /// Each command with the index of the system that issued it, in run order.
    pub commands: Vec<(usize, CommandRecord)>,
}

/// Records the events and commands of one tick, passed to `Simulation::step_observed`.
#[derive(Default)]
pub struct TickRecorder {
    record: TickRecord,
}

impl TickRecorder {
    /// The record of the observed tick, which ended at `tick`.
    pub fn into_record(self, tick: u64) -> TickRecord {
        TickRecord {
            tick,
            ..self.record
        }
    }
}

impl ScheduleObserver for TickRecorder {
    fn before_system(&mut self, _: usize) {}

    fn after_system(&mut self, index: usize, commands: &[CommandRecord]) {
        self.record
            .commands
            .extend(commands.iter().map(|command| (index, *command)));
    }

    fn tick_end(&mut self, world: &World) {
        for (name, values) in world.events_json() {
            self.record
                .events
                .extend(values.into_iter().map(|value| (name, value)));
        }
    }
}

/// The first simulation compared, called current, then the other, called compare.
const CURRENT: usize = 0;
const COMPARE: usize = 1;

/// Compares two simulations tick after tick and reports each divergent entity once.
pub struct EntityComparison {
    /// Snapshot parts every entity diff shares: its components are empty.
    skeleton: Snapshot,
    first_divergent: Option<u64>,
    /// The first divergent tick of every reported entity, by index and generation.
    divergent: BTreeMap<(u32, u32), (Entity, u64)>,
    /// Problems not yet taken, such as a value that failed to decode for a diff.
    warnings: Vec<String>,
}

impl EntityComparison {
    /// A comparison of simulations of the same app as `skeleton_from`.
    pub fn new(skeleton_from: &Simulation) -> EntityComparison {
        let mut skeleton = skeleton_from.world().snapshot();
        for (_, values) in &mut skeleton.components {
            values.clear();
        }
        EntityComparison {
            skeleton,
            first_divergent: None,
            divergent: BTreeMap::new(),
            warnings: Vec::new(),
        }
    }

    /// The first tick whose world hashes differ, among the ticks compared.
    pub fn first_divergent(&self) -> Option<u64> {
        self.first_divergent
    }

    /// The problems found since the last call.
    pub fn take_warnings(&mut self) -> Vec<String> {
        std::mem::take(&mut self.warnings)
    }

    /// Compares `sims`, both at `tick`, and gives the entities that diverge there for the
    /// first time, ordered by entity. `records` are the records of each simulation's recent
    /// ticks: those of `tick` and the tick before explain a divergence at `tick`. A
    /// divergence at a tick below `explains_from` has no cause, as the ticks recorded before
    /// it ran before what parts the simulations.
    pub fn compare(
        &mut self,
        sims: [&Simulation; 2],
        tick: u64,
        records: [&[TickRecord]; 2],
        explains_from: u64,
        event_schemas: &BTreeMap<&'static str, TypeSchema>,
        systems: &[&'static str],
    ) -> Vec<EntityDivergence> {
        let trees = [sims[CURRENT].hash_tree(), sims[COMPARE].hash_tree()];
        if trees[CURRENT].root == trees[COMPARE].root {
            return Vec::new();
        }
        self.first_divergent.get_or_insert(tick);
        let mut sides: BTreeMap<Entity, [Option<&EntityHash>; 2]> = BTreeMap::new();
        for (side, tree) in trees.iter().enumerate() {
            for leaf in &tree.entities {
                sides.entry(leaf.entity).or_default()[side] = Some(leaf);
            }
        }
        let mut found = Vec::new();
        for (entity, sides) in sides {
            if self.divergent.contains_key(&key(entity)) {
                continue;
            }
            let kind = match sides {
                [Some(current), Some(compare)] if current.hash == compare.hash => continue,
                [Some(current), Some(compare)] => {
                    let names = |leaf: &EntityHash| {
                        leaf.components
                            .iter()
                            .map(|component| component.name.clone())
                            .collect::<Vec<_>>()
                    };
                    if names(current) == names(compare) {
                        DivergenceKind::Changed
                    } else {
                        DivergenceKind::Replaced
                    }
                }
                [Some(_), None] => DivergenceKind::OnlyInCurrent,
                [None, Some(_)] => DivergenceKind::OnlyInCompare,
                [None, None] => unreachable!("listed by a tree"),
            };
            self.divergent.insert(key(entity), (entity, tick));
            found.push((entity, kind));
        }
        let changed: Vec<Entity> = found
            .iter()
            .filter(|(_, kind)| *kind == DivergenceKind::Changed)
            .map(|(entity, _)| *entity)
            .collect();
        let mut differences = self.differences(sims, &changed).unwrap_or_else(|error| {
            self.warnings.push(format!(
                "tick {tick}: the differences could not be listed: {error}"
            ));
            BTreeMap::new()
        });
        let explaining: Vec<&TickRecord> = if tick >= explains_from {
            records
                .into_iter()
                .flatten()
                .filter(|record| (tick.saturating_sub(1)..=tick).contains(&record.tick))
                .collect()
        } else {
            Vec::new()
        };
        found
            .into_iter()
            .map(|(entity, kind)| EntityDivergence {
                entity,
                kind,
                first_tick: tick,
                first_difference: differences.remove(&entity),
                causes: self.causes(entity, &explaining, event_schemas, systems),
            })
            .collect()
    }

    /// The first difference of each of `entities`, through snapshots restricted to them.
    fn differences(
        &self,
        sims: [&Simulation; 2],
        entities: &[Entity],
    ) -> Result<BTreeMap<Entity, Difference>, SnapshotError> {
        if entities.is_empty() {
            return Ok(BTreeMap::new());
        }
        let restricted = |world: &World| Snapshot {
            components: self
                .skeleton
                .components
                .iter()
                .map(|(name, _)| {
                    let values = entities
                        .iter()
                        .filter_map(|&entity| Some((entity, world.component_bytes(entity, name)?)))
                        .collect();
                    (name.clone(), values)
                })
                .collect(),
            ..self.skeleton.clone()
        };
        let world = sims[CURRENT].world();
        let (current, compare) = (restricted(world), restricted(sims[COMPARE].world()));
        let subjects: BTreeMap<String, Entity> = entities
            .iter()
            .map(|&entity| (entity_subject(entity), entity))
            .collect();
        let mut first = BTreeMap::new();
        for difference in world.diff_snapshots(&current, &compare)? {
            if let Some(&entity) = subjects.get(&difference.subject) {
                first.entry(entity).or_insert(difference);
            }
        }
        Ok(first)
    }

    /// The events and commands of `records` that explain why `entity` diverges.
    fn causes(
        &self,
        entity: Entity,
        records: &[&TickRecord],
        event_schemas: &BTreeMap<&'static str, TypeSchema>,
        systems: &[&'static str],
    ) -> Vec<Cause> {
        let mut causes = Vec::new();
        let mut add = |cause: Cause| {
            if !causes.contains(&cause) {
                causes.push(cause);
            }
        };
        for record in records {
            for (name, value) in &record.events {
                let Some(schema) = event_schemas.get(name) else {
                    continue;
                };
                let refs = entity_refs(schema, value);
                let Some(own) = refs
                    .iter()
                    .find(|at| (at.index, at.generation) == key(entity))
                else {
                    continue;
                };
                for at in &refs {
                    match self.divergent.get(&(at.index, at.generation)) {
                        Some(&(other, since)) if other != entity && since <= record.tick => {
                            add(Cause::Event {
                                tick: record.tick,
                                event: name.to_string(),
                                path: own.path.clone(),
                                other,
                            });
                        }
                        _ => {}
                    }
                }
            }
            for (index, command) in &record.commands {
                if command.entity == Some(entity) {
                    add(Cause::Command {
                        tick: record.tick,
                        system: systems
                            .get(*index)
                            .map_or_else(|| format!("system {index}"), |name| name.to_string()),
                        kind: command_kind(command.kind).to_string(),
                    });
                }
            }
        }
        causes
    }
}

fn key(entity: Entity) -> (u32, u32) {
    (entity.index(), entity.generation())
}

fn command_kind(kind: CommandKind) -> &'static str {
    match kind {
        CommandKind::Spawn => "spawn",
        CommandKind::Insert => "insert",
        CommandKind::Remove => "remove",
        CommandKind::Despawn => "despawn",
    }
}
