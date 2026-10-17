//! Deterministic system ordering: a topological sort of `before` and `after` constraints
//! with ties broken by registration index.

use std::collections::{BTreeMap, BTreeSet};

use crate::World;
use crate::commands;
use crate::system::{EVENT, IntoSystem, RESOURCE, System};

/// Names a set of systems, for ordering constraints.
pub trait SystemLabel: 'static {
    fn name(&self) -> &'static str;
}

/// A system with its sets and ordering constraints, by label name.
pub struct SystemConfig {
    system: Box<dyn System>,
    sets: Vec<&'static str>,
    after: Vec<&'static str>,
    before: Vec<&'static str>,
}

/// A system or a system configuration, with chainable configuration methods.
pub trait IntoSystemConfig<M>: Sized {
    fn into_config(self) -> SystemConfig;

    fn in_set(self, label: impl SystemLabel) -> SystemConfig {
        let mut config = self.into_config();
        config.sets.push(label.name());
        config
    }

    /// Runs after every system in `label`.
    fn after(self, label: impl SystemLabel) -> SystemConfig {
        let mut config = self.into_config();
        config.after.push(label.name());
        config
    }

    /// Runs before every system in `label`.
    fn before(self, label: impl SystemLabel) -> SystemConfig {
        let mut config = self.into_config();
        config.before.push(label.name());
        config
    }
}

impl<M, S: IntoSystem<M>> IntoSystemConfig<M> for S {
    fn into_config(self) -> SystemConfig {
        SystemConfig {
            system: self.into_system(),
            sets: Vec::new(),
            after: Vec::new(),
            before: Vec::new(),
        }
    }
}

impl IntoSystemConfig<()> for SystemConfig {
    fn into_config(self) -> SystemConfig {
        self
    }
}

/// How to resolve an access conflict of one kind.
fn conflict_hint(kind: &str) -> &'static str {
    match kind {
        RESOURCE => "Use a single ResMut parameter instead of both Res and ResMut.",
        EVENT => {
            "Use either Events or EventWriter for this event in one system, or split the system in two."
        }
        _ => "Merge them into one query, or split the system in two.",
    }
}

#[derive(Debug, thiserror::Error)]
pub enum BuildError {
    #[error(
        "system `{system}` accesses `{name}` in parameters {first} and {second}, and at least one is mutable. {}",
        conflict_hint(kind)
    )]
    AccessConflict {
        system: &'static str,
        /// `"component"`, `"resource"`, `"event"`, or `"runtime resource"`.
        kind: &'static str,
        name: &'static str,
        /// Parameter positions, counted from 1; equal when one query conflicts with itself.
        first: usize,
        second: usize,
    },
    #[error(
        "system `{system}` uses {kind} `{name}`, which is not registered. Register it on the app (component, resource, event, or runtime resource)."
    )]
    Unregistered {
        system: &'static str,
        kind: &'static str,
        name: &'static str,
    },
    #[error(
        "systems {systems:?} form an ordering cycle through their `before` and `after` constraints. Remove one of those constraints."
    )]
    OrderingCycle { systems: Vec<&'static str> },
    #[error("label `{label}` has no systems. Did you forget to add the plugin that provides it?")]
    EmptyLabel { label: &'static str },
}

#[derive(Default)]
pub struct ScheduleBuilder {
    configs: Vec<SystemConfig>,
}

impl ScheduleBuilder {
    pub fn add<M>(&mut self, system: impl IntoSystemConfig<M>) {
        self.configs.push(system.into_config());
    }

    /// Checks every system against `world` and orders them.
    pub fn build(self, world: &World) -> Result<Schedule, BuildError> {
        for config in &self.configs {
            config.system.check(world)?;
        }
        let mut members: BTreeMap<&'static str, Vec<usize>> = BTreeMap::new();
        for (index, config) in self.configs.iter().enumerate() {
            for &set in &config.sets {
                members.entry(set).or_default().push(index);
            }
        }
        // `successors[i]` holds the systems that must run after system `i`.
        let mut successors = vec![BTreeSet::new(); self.configs.len()];
        for (index, config) in self.configs.iter().enumerate() {
            for (labels, runs_after) in [(&config.after, true), (&config.before, false)] {
                for &label in labels {
                    let others = members.get(label).ok_or(BuildError::EmptyLabel { label })?;
                    for &other in others.iter().filter(|&&other| other != index) {
                        let (from, to) = if runs_after {
                            (other, index)
                        } else {
                            (index, other)
                        };
                        successors[from].insert(to);
                    }
                }
            }
        }
        let order = sort(&successors).map_err(|stuck| BuildError::OrderingCycle {
            systems: stuck
                .into_iter()
                .map(|index| self.configs[index].system.name())
                .collect(),
        })?;
        let mut systems: Vec<Option<Box<dyn System>>> = self
            .configs
            .into_iter()
            .map(|config| Some(config.system))
            .collect();
        Ok(Schedule {
            systems: order
                .into_iter()
                .map(|index| systems[index].take().expect("each index is ordered once"))
                .collect(),
        })
    }
}

/// Kahn's algorithm, always taking the lowest ready index. On a cycle, returns the
/// systems on or between cycles, in index order, without those merely blocked by one.
fn sort(successors: &[BTreeSet<usize>]) -> Result<Vec<usize>, Vec<usize>> {
    let mut predecessors = vec![0usize; successors.len()];
    for &to in successors.iter().flatten() {
        predecessors[to] += 1;
    }
    let mut ready: BTreeSet<usize> = (0..successors.len())
        .filter(|&index| predecessors[index] == 0)
        .collect();
    let mut order = Vec::with_capacity(successors.len());
    while let Some(index) = ready.pop_first() {
        order.push(index);
        for &next in &successors[index] {
            predecessors[next] -= 1;
            if predecessors[next] == 0 {
                ready.insert(next);
            }
        }
    }
    if order.len() == successors.len() {
        return Ok(order);
    }
    let mut stuck: BTreeSet<usize> = (0..successors.len())
        .filter(|&index| predecessors[index] > 0)
        .collect();
    // Drop systems with no successor left among the stuck ones until only cycles remain.
    while let Some(&leaf) = stuck
        .iter()
        .find(|&&index| successors[index].is_disjoint(&stuck))
    {
        stuck.remove(&leaf);
    }
    Err(stuck.into_iter().collect())
}

/// Systems in run order.
pub struct Schedule {
    systems: Vec<Box<dyn System>>,
}

impl Schedule {
    /// Runs every system in order, applying its commands right after it returns.
    pub fn run(&mut self, world: &mut World) {
        for system in &mut self.systems {
            system.run(world);
            commands::apply(world);
        }
    }
}
