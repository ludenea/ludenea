//! What the host reports without being asked: the changes of subscribed entities and
//! resources, computed from per-component and per-resource hashes, the render frames of
//! the ticks played while `RenderFrames` is subscribed, and the introspection record of the
//! ticks run while `Introspection` is subscribed.

use std::collections::{BTreeMap, BTreeSet, VecDeque};

use ludenea_app::{SimError, Simulation};
use ludenea_draw::frame::RenderFrame;
use ludenea_ecs::{
    CommandKind, CommandRecord, Entity, EntityFilter, FilterError, ScheduleObserver, World,
    hash_value,
};

use crate::json::untyped_to_wire;
use crate::protocol::{Event, RecordedEvent, Topic};
use crate::values::{component_value, resource_value};

/// How many of the ticks run while `Introspection` is subscribed stay recorded.
pub const INTROSPECTION_TICKS: usize = 600;

/// How often, at most, a subscribed resource is pushed while playing: a resource such as the
/// contact cache changes every tick, and its whole value is converted and sent each push.
pub const RESOURCE_PUSHES_PER_SECOND: u32 = 10;

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
    resources: Vec<ResourceWatch>,
    /// Seconds played since resources were last checked for changes.
    since_resources: f64,
    /// Whether `RenderFrames` is subscribed.
    render_frames: bool,
    /// The debug layers of the pushed frames.
    pushed_layers: BTreeSet<String>,
    /// The extraction failures already reported, each reported once. They grow with the
    /// console lines reported, one per failing tick at most.
    failed_frames: BTreeSet<String>,
}

struct ResourceWatch {
    name: String,
    /// `None` until its value is first pushed.
    hash: Option<u64>,
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
    /// Starts watching `topic` from the current state, which is not pushed (except for a
    /// resource). Subscribing again to a watched topic changes nothing else.
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
            // Unlike the others, the current value is pushed, again on a second subscription.
            Topic::Resource { name } => {
                match self.resources.iter_mut().find(|watch| watch.name == *name) {
                    Some(watch) => watch.hash = None,
                    None => self.resources.push(ResourceWatch {
                        name: name.clone(),
                        hash: None,
                    }),
                }
            }
            Topic::RenderFrames => self.render_frames = true,
            Topic::Introspection | Topic::Ghost | Topic::Divergence => {}
        }
        Ok(())
    }

    pub(crate) fn unsubscribe(&mut self, topic: &Topic) {
        match topic {
            Topic::Entities { filter } => self.entities.retain(|watch| watch.text != *filter),
            Topic::Entity { entity } => self.entity.retain(|watch| watch.entity != *entity),
            Topic::Resource { name } => self.resources.retain(|watch| watch.name != *name),
            Topic::RenderFrames => self.render_frames = false,
            Topic::Introspection | Topic::Ghost | Topic::Divergence => {}
        }
    }

    /// Whether the frames of the ticks played are pushed.
    pub(crate) fn pushes_frames(&self) -> bool {
        self.render_frames
    }

    pub(crate) fn set_pushed_layers(&mut self, debug_layers: Vec<String>) {
        self.pushed_layers = debug_layers.into_iter().collect();
    }

    /// The frame of the current tick with the pushed debug layers. On failure, the console
    /// error that reports it, `None` when the same failure was reported already.
    pub(crate) fn extract_frame(
        &mut self,
        sim: &Simulation,
    ) -> Result<RenderFrame, Option<String>> {
        sim.render_frame(&self.pushed_layers).map_err(|error| {
            let message = format!("render frame of tick {}: {error}", sim.tick());
            self.failed_frames
                .insert(message.clone())
                .then_some(message)
        })
    }

    /// Counts `seconds` of play toward the next check of the subscribed resources.
    pub(crate) fn played(&mut self, seconds: f64) {
        self.since_resources += seconds;
    }

    /// The events describing what changed since the last call, for every watched topic.
    /// Only the values whose hash changed are converted. While `playing`, resources are
    /// checked at most `RESOURCE_PUSHES_PER_SECOND` times a second of play; once paused, at
    /// every call, so the last value played is pushed.
    pub(crate) fn deltas(&mut self, sim: &Simulation, playing: bool) -> Vec<Event> {
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
        let due = !playing || self.since_resources >= 1.0 / f64::from(RESOURCE_PUSHES_PER_SECOND);
        if due && !self.resources.is_empty() {
            self.since_resources = 0.0;
            let mut resources = BTreeMap::new();
            for watch in &mut self.resources {
                let hash = world.resource_hash(&watch.name);
                if hash.is_some() && hash != watch.hash {
                    watch.hash = hash;
                    resources.insert(watch.name.clone(), resource_value(sim, &watch.name));
                }
            }
            if !resources.is_empty() {
                events.push(Event::ResourcesChanged { resources });
            }
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
    // Only native builds read the clock, so only they expect the lint.
    #[cfg_attr(
        not(target_family = "wasm"),
        expect(
            clippy::disallowed_methods,
            reason = "profiling measures wall-clock time, which never feeds back into the simulation"
        )
    )]
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
