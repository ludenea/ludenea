//! What the host reports without being asked: the changes of subscribed entities and
//! resources, computed from per-component hashes, and the introspection record of the
//! ticks run while `Introspection` is subscribed.

use std::collections::{BTreeMap, VecDeque};

use ludenea_app::{SimError, Simulation};
use ludenea_ecs::{
    CommandKind, CommandRecord, Entity, EntityFilter, FilterError, ScheduleObserver, World,
    hash_value,
};

use crate::json::untyped_to_wire;
use crate::protocol::{Event, RecordedEvent, Topic};
use crate::values::{component_value, resource_value};

/// How many of the ticks run while `Introspection` is subscribed stay recorded.
pub const INTROSPECTION_TICKS: usize = 600;

/// How many of the last recorded ticks the system timings average.
const TIMED_TICKS: usize = 60;

/// Component hashes by stable name.
type Hashes = BTreeMap<String, u64>;

/// The subscribed topics, except `Introspection` which the session records, each with
/// the state last pushed.
#[derive(Default)]
pub(crate) struct Subscriptions {
    entities: Vec<EntitiesWatch>,
    entity: Vec<EntityWatch>,
    resources: Option<Hashes>,
}

struct EntitiesWatch {
    text: String,
    filter: EntityFilter,
    /// A hash of the matching entities and their hashes.
    digest: u64,
}

struct EntityWatch {
    entity: Entity,
    /// `None` while the entity is not alive.
    hashes: Option<Hashes>,
}

impl Subscriptions {
    /// Starts watching `topic` from the current state, which is not pushed. Subscribing
    /// again to a watched topic changes nothing.
    pub(crate) fn subscribe(&mut self, topic: &Topic, sim: &Simulation) -> Result<(), FilterError> {
        let world = sim.world();
        match topic {
            Topic::Entities { filter: text } => {
                if !self.entities.iter().any(|watch| watch.text == *text) {
                    let filter = EntityFilter::parse(text, world)?;
                    let (digest, _) = matching(&filter, world);
                    self.entities.push(EntitiesWatch {
                        text: text.clone(),
                        filter,
                        digest,
                    });
                }
            }
            Topic::Entity { entity } => {
                if !self.entity.iter().any(|watch| watch.entity == *entity) {
                    self.entity.push(EntityWatch {
                        entity: *entity,
                        hashes: component_hashes(world, *entity),
                    });
                }
            }
            Topic::Resources => {
                self.resources.get_or_insert_with(|| resource_hashes(world));
            }
            Topic::Introspection => {}
        }
        Ok(())
    }

    pub(crate) fn unsubscribe(&mut self, topic: &Topic) {
        match topic {
            Topic::Entities { filter } => self.entities.retain(|watch| watch.text != *filter),
            Topic::Entity { entity } => self.entity.retain(|watch| watch.entity != *entity),
            Topic::Resources => self.resources = None,
            Topic::Introspection => {}
        }
    }

    /// The events describing what changed since the last call, for every watched topic.
    /// Only the values whose hash changed are converted.
    pub(crate) fn deltas(&mut self, sim: &Simulation) -> Vec<Event> {
        let mut events = Vec::new();
        let world = sim.world();
        for watch in &mut self.entities {
            let (digest, total) = matching(&watch.filter, world);
            if digest != watch.digest {
                watch.digest = digest;
                events.push(Event::EntitiesChanged {
                    filter: watch.text.clone(),
                    total,
                });
            }
        }
        for watch in &mut self.entity {
            let entity = watch.entity;
            let now = component_hashes(world, entity);
            match (watch.hashes.take(), &now) {
                (Some(_), None) => events.push(Event::EntityDespawned { entity }),
                (None, None) => {}
                (before, Some(now)) => {
                    let before = before.unwrap_or_default();
                    let removed: Vec<String> = before
                        .keys()
                        .filter(|name| !now.contains_key(*name))
                        .cloned()
                        .collect();
                    let components: BTreeMap<_, _> = now
                        .iter()
                        .filter(|(name, hash)| before.get(*name) != Some(hash))
                        .map(|(name, _)| {
                            let value = component_value(sim, entity, name).expect("a component");
                            (name.clone(), value)
                        })
                        .collect();
                    if !components.is_empty() || !removed.is_empty() {
                        events.push(Event::EntityChanged {
                            entity,
                            components,
                            removed,
                        });
                    }
                }
            }
            watch.hashes = now;
        }
        if let Some(before) = &mut self.resources {
            let now = resource_hashes(world);
            let resources: BTreeMap<_, _> = now
                .iter()
                .filter(|(name, hash)| before.get(*name) != Some(hash))
                .map(|(name, _)| (name.clone(), resource_value(sim, name)))
                .collect();
            if !resources.is_empty() {
                events.push(Event::ResourcesChanged { resources });
            }
            *before = now;
        }
        events
    }
}

/// A digest of the entities matching `filter` with their hashes, and their count.
// ponytail: hashes every matching entity on each pass; keep per-entity hashes up to date as
// systems write if wide filters on large worlds get slow.
fn matching(filter: &EntityFilter, world: &World) -> (u64, u32) {
    let rows: Vec<(Entity, u64)> = world
        .filter_entities(filter)
        .into_iter()
        .map(|entity| {
            let hash = world.entity_hash(entity).map_or(0, |leaf| leaf.hash);
            (entity, hash)
        })
        .collect();
    let total = u32::try_from(rows.len()).expect("fewer than 2^32 entities");
    (hash_value(&rows), total)
}

fn component_hashes(world: &World, entity: Entity) -> Option<Hashes> {
    world.entity_hash(entity).map(|leaf| {
        leaf.components
            .into_iter()
            .map(|component| (component.name, component.hash))
            .collect()
    })
}

fn resource_hashes(world: &World) -> Hashes {
    world
        .resource_hashes()
        .into_iter()
        .map(|resource| (resource.name, resource.hash))
        .collect()
}

/// One tick run while `Introspection` is subscribed.
#[derive(Default)]
pub(crate) struct TickRecord {
    /// The tick it ended at, and the hash it produced once the session has passed it.
    tick: u64,
    hash: Option<u64>,
    pub(crate) events: Vec<RecordedEvent>,
    /// For each system that issued commands, its index in run order and the commands.
    pub(crate) commands: Vec<(usize, Vec<String>)>,
    /// Microseconds per system in run order; empty where time is not measured.
    micros: Vec<f32>,
}

/// The last `INTROSPECTION_TICKS` ticks run while `Introspection` is subscribed, oldest run
/// first.
#[derive(Default)]
pub(crate) struct Introspection {
    ticks: VecDeque<TickRecord>,
}

impl Introspection {
    /// Steps like `Simulation::step_encoded`, recording the tick. The session passes the
    /// hash the tick produced to `hashed` next, so the world is hashed once per tick.
    pub(crate) fn step(
        &mut self,
        sim: &mut Simulation,
        inputs: Option<&[u8]>,
    ) -> Result<(), SimError> {
        let mut recorder = Recorder::default();
        sim.step_observed(inputs, &mut recorder)?;
        let tick = sim.tick();
        self.ticks.retain(|record| record.tick != tick);
        if self.ticks.len() == INTROSPECTION_TICKS {
            self.ticks.pop_front();
        }
        self.ticks.push_back(TickRecord {
            tick,
            ..recorder.record
        });
        Ok(())
    }

    /// Sets the hash produced by the tick that ended at `tick`, the last one recorded.
    pub(crate) fn hashed(&mut self, tick: u64, hash: u64) {
        if let Some(record) = self.ticks.back_mut()
            && record.tick == tick
        {
            record.hash = Some(hash);
        }
    }

    /// The record of the tick that ended at `tick`, if it is the one that produced `hash`,
    /// the hash the session now holds for it: a replaced future makes older records stale.
    pub(crate) fn tick(&self, tick: u64, hash: Option<u64>) -> Option<&TickRecord> {
        // Keyed on the post-tick hash alone: a replaced future that reaches the very same
        // state at `tick` by other means keeps the old record, whose events and commands
        // may differ. Telling them apart would need the hash of every earlier tick too.
        self.ticks
            .iter()
            .find(|record| record.tick == tick && record.hash.is_some() && record.hash == hash)
    }

    /// The mean time of each of `systems` systems over the last `TIMED_TICKS` records.
    pub(crate) fn mean_micros(&self, systems: usize) -> Vec<Option<f32>> {
        let recent: Vec<&TickRecord> = self.ticks.iter().rev().take(TIMED_TICKS).collect();
        (0..systems)
            .map(|index| {
                let times: Vec<f32> = recent
                    .iter()
                    .filter_map(|record| record.micros.get(index).copied())
                    .collect();
                (!times.is_empty()).then(|| times.iter().sum::<f32>() / times.len() as f32)
            })
            .collect()
    }
}

#[derive(Default)]
struct Recorder {
    record: TickRecord,
    #[cfg(not(target_family = "wasm"))]
    started: Option<std::time::Instant>,
}

impl ScheduleObserver for Recorder {
    // Measuring time is outside the simulation: it never feeds back into it.
    #[allow(clippy::disallowed_methods)]
    fn before_system(&mut self, _: usize) {
        #[cfg(not(target_family = "wasm"))]
        {
            self.started = Some(std::time::Instant::now());
        }
    }

    fn after_system(&mut self, index: usize, commands: &[CommandRecord]) {
        #[cfg(not(target_family = "wasm"))]
        if let Some(started) = self.started.take() {
            self.record
                .micros
                .push(started.elapsed().as_secs_f32() * 1_000_000.0);
        }
        if !commands.is_empty() {
            let commands = commands.iter().map(describe).collect();
            self.record.commands.push((index, commands));
        }
    }

    fn tick_end(&mut self, world: &World) {
        for (event_type, values) in world.events_json() {
            for value in values {
                self.record.events.push(RecordedEvent {
                    event_type: event_type.to_string(),
                    value: untyped_to_wire(value),
                });
            }
        }
    }
}

/// A command as text, such as `"insert ludenea::Body on 12v0"`.
fn describe(record: &CommandRecord) -> String {
    let entity = record
        .entity
        .map(|entity| entity.to_string())
        .unwrap_or_default();
    let component = record.component.unwrap_or_default();
    match record.kind {
        CommandKind::Spawn => format!("spawn {entity}"),
        CommandKind::Despawn => format!("despawn {entity}"),
        CommandKind::Insert => format!("insert {component} on {entity}"),
        CommandKind::Remove => format!("remove {component} from {entity}"),
    }
}
