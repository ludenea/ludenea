//! Enforces the hexagonal boundary: core crates stay free of I/O.

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::fmt;

use cargo_metadata::{CargoOpt, DependencyKind, MetadataCommand};

/// Crates that talk to the outside world; core crates must never reach them.
const ADAPTER_CRATES: &[&str] = &[
    "ludenea_protocol",
    "ludenea_server",
    "ludenea_reflect",
    "ludenea_timeline",
];

/// Crates performing I/O or providing an async runtime.
const IO_CRATES: &[&str] = &["async-std", "hyper", "mio", "reqwest", "smol", "tokio"];

/// Crates reading randomness from the operating system, which differs on
/// every run.
const OS_RANDOMNESS_CRATES: &[&str] = &["getrandom"];

/// Each rule forbids core crates from reaching any of its crates.
const RULES: &[(&[&str], &str)] = &[
    (
        ADAPTER_CRATES,
        "core crates must not depend on adapter crates",
    ),
    (
        IO_CRATES,
        "core crates must not perform I/O or depend on an async runtime",
    ),
    (
        OS_RANDOMNESS_CRATES,
        "core crates must not read OS randomness, seed a deterministic generator instead",
    ),
];

/// Workspace members that are not core crates.
const NON_CORE_MEMBERS: &[&str] = &["xtask"];

/// Package name to the names of its normal (non-dev, non-build) dependencies.
pub type Graph = BTreeMap<String, BTreeSet<String>>;

#[derive(Debug, PartialEq, Eq)]
pub struct Violation {
    /// From the core crate to the forbidden crate.
    pub path: Vec<String>,
    pub rule: &'static str,
}

impl fmt::Display for Violation {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let (core, rest) = self.path.split_first().expect("a path is never empty");
        let (forbidden, through) = rest.split_last().expect("a path has two ends");
        write!(formatter, "{core} depends on {forbidden}")?;
        if !through.is_empty() {
            write!(formatter, " through {}", through.join(", "))?;
        }
        write!(formatter, ": {}", self.rule)
    }
}

/// Reads the dependency graph of the current workspace, with every feature
/// enabled so that optional dependencies are checked too.
pub fn load() -> Result<(Graph, Vec<String>), cargo_metadata::Error> {
    let metadata = MetadataCommand::new()
        .features(CargoOpt::AllFeatures)
        .exec()?;
    let name_of = |id| metadata[id].name.to_string();
    let mut graph = Graph::new();
    for node in metadata.resolve.iter().flat_map(|resolve| &resolve.nodes) {
        let dependencies = graph.entry(name_of(&node.id)).or_default();
        for dependency in &node.deps {
            if dependency
                .dep_kinds
                .iter()
                .any(|info| info.kind == DependencyKind::Normal)
            {
                dependencies.insert(name_of(&dependency.pkg));
            }
        }
    }
    let members = metadata.workspace_members.iter().map(name_of).collect();
    Ok((graph, members))
}

/// Lists every forbidden crate reachable from a core crate, which is any
/// workspace member that is neither an adapter crate nor tooling.
pub fn violations(graph: &Graph, workspace_members: &[String]) -> Vec<Violation> {
    let is_core = |name: &str| !ADAPTER_CRATES.contains(&name) && !NON_CORE_MEMBERS.contains(&name);
    workspace_members
        .iter()
        .filter(|member| is_core(member))
        .flat_map(|core| violations_from(graph, core))
        .collect()
}

/// Breadth-first walk from `core`, stopping at each forbidden crate.
fn violations_from(graph: &Graph, core: &str) -> Vec<Violation> {
    let mut parents: BTreeMap<&str, &str> = BTreeMap::new();
    let mut queue = VecDeque::from([core]);
    let mut found = Vec::new();
    while let Some(package) = queue.pop_front() {
        for dependency in graph.get(package).into_iter().flatten() {
            if dependency == core || parents.contains_key(dependency.as_str()) {
                continue;
            }
            parents.insert(dependency, package);
            if let Some((_, rule)) = RULES
                .iter()
                .find(|(crates, _)| crates.contains(&dependency.as_str()))
            {
                found.push(Violation {
                    path: path_to(&parents, core, dependency),
                    rule,
                });
            } else {
                queue.push_back(dependency);
            }
        }
    }
    found
}

fn path_to(parents: &BTreeMap<&str, &str>, core: &str, target: &str) -> Vec<String> {
    let mut path = vec![target.to_owned()];
    let mut current = target;
    while current != core {
        current = parents[current];
        path.push(current.to_owned());
    }
    path.reverse();
    path
}

#[cfg(test)]
mod tests {
    use super::*;

    fn graph(edges: &[(&str, &[&str])]) -> Graph {
        edges
            .iter()
            .map(|(package, dependencies)| {
                let dependencies = dependencies.iter().map(ToString::to_string).collect();
                ((*package).to_owned(), dependencies)
            })
            .collect()
    }

    fn members(names: &[&str]) -> Vec<String> {
        names.iter().map(ToString::to_string).collect()
    }

    #[test]
    fn a_core_crate_without_forbidden_dependencies_passes() {
        let graph = graph(&[("ludenea_sim", &["ludenea_math"]), ("ludenea_math", &[])]);

        assert_eq!(
            violations(&graph, &members(&["ludenea_sim", "ludenea_math"])),
            []
        );
    }

    #[test]
    fn a_core_crate_depending_on_an_async_runtime_is_reported() {
        let graph = graph(&[("ludenea_sim", &["tokio"])]);

        let found = violations(&graph, &members(&["ludenea_sim"]));

        assert_eq!(
            found.iter().map(ToString::to_string).collect::<Vec<_>>(),
            [
                "ludenea_sim depends on tokio: core crates must not perform I/O or depend on an async runtime"
            ]
        );
    }

    #[test]
    fn a_transitive_forbidden_dependency_is_reported_with_its_path() {
        let graph = graph(&[("ludenea_sim", &["helper"]), ("helper", &["mio"])]);

        let found = violations(&graph, &members(&["ludenea_sim"]));

        assert_eq!(
            found.iter().map(ToString::to_string).collect::<Vec<_>>(),
            [
                "ludenea_sim depends on mio through helper: core crates must not perform I/O or depend on an async runtime"
            ]
        );
    }

    #[test]
    fn a_core_crate_reaching_os_randomness_is_reported() {
        let graph = graph(&[("ludenea_sim", &["rand"]), ("rand", &["getrandom"])]);

        let found = violations(&graph, &members(&["ludenea_sim"]));

        assert_eq!(
            found.iter().map(ToString::to_string).collect::<Vec<_>>(),
            [
                "ludenea_sim depends on getrandom through rand: core crates must not read OS randomness, seed a deterministic generator instead"
            ]
        );
    }

    #[test]
    fn a_core_crate_depending_on_an_adapter_crate_is_reported() {
        let graph = graph(&[
            ("ludenea_ecs", &["ludenea_server"]),
            ("ludenea_server", &["tokio"]),
        ]);

        let found = violations(&graph, &members(&["ludenea_ecs", "ludenea_server"]));

        assert_eq!(
            found.iter().map(ToString::to_string).collect::<Vec<_>>(),
            [
                "ludenea_ecs depends on ludenea_server: core crates must not depend on adapter crates"
            ]
        );
    }

    #[test]
    fn adapter_crates_and_xtask_may_perform_io() {
        let graph = graph(&[("ludenea_server", &["tokio"]), ("xtask", &["tokio"])]);

        assert_eq!(
            violations(&graph, &members(&["ludenea_server", "xtask"])),
            []
        );
    }
}
