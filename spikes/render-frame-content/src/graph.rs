//! The frame graph: one graph per frame, declared and then compiled.
//! Throwaway, never product code.
//!
//! A pass is declared by a function that receives a builder. Each call that
//! creates or imports a resource returns a typed identifier; each call that
//! declares a read or a write returns an access token of that kind, and a
//! pass's body holds the tokens it was given and nothing else, so it cannot
//! name a resource its pass did not declare.
//!
//! Compilation removes the passes whose outputs nothing reads, computes the
//! lifetime of every resource, and keeps the order of declaration, which is
//! the order of submission. A pass may never read a transient that no earlier
//! pass wrote: compilation refuses such a graph.

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
pub struct ResourceId(u32);

/// The right to sample a resource in the pass that declared the read.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Read(ResourceId);

/// The right to render to a resource in the pass that declared the write.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Write(ResourceId);

impl Read {
    pub fn resource(self) -> ResourceId {
        self.0
    }
}

impl Write {
    pub fn resource(self) -> ResourceId {
        self.0
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ResourceKind<H> {
    /// Created by the graph, dead at the end of the frame.
    Transient { width: u32, height: u32 },
    /// Owned outside the graph and brought in for the frame.
    Imported(H),
}

struct Resource<H> {
    kind: ResourceKind<H>,
    /// Read from outside the graph after the frame: a pass that writes it is kept.
    output: bool,
}

struct Pass<B> {
    name: &'static str,
    reads: Vec<ResourceId>,
    writes: Vec<ResourceId>,
    body: B,
}

/// What a pass's declaration function works with.
pub struct PassBuilder<'a, H> {
    resources: &'a mut Vec<Resource<H>>,
    reads: Vec<ResourceId>,
    writes: Vec<ResourceId>,
}

impl<H> PassBuilder<'_, H> {
    /// Creates a transient the pass writes.
    pub fn create(&mut self, width: u32, height: u32) -> Write {
        self.resources.push(Resource { kind: ResourceKind::Transient { width, height }, output: false });
        let id = ResourceId(self.resources.len() as u32 - 1);
        self.writes.push(id);
        Write(id)
    }

    pub fn read(&mut self, resource: ResourceId) -> Read {
        self.reads.push(resource);
        Read(resource)
    }

    pub fn write(&mut self, resource: ResourceId) -> Write {
        self.writes.push(resource);
        Write(resource)
    }
}

pub struct GraphBuilder<H, B> {
    resources: Vec<Resource<H>>,
    passes: Vec<Pass<B>>,
}

impl<H, B> Default for GraphBuilder<H, B> {
    fn default() -> GraphBuilder<H, B> {
        GraphBuilder { resources: Vec::new(), passes: Vec::new() }
    }
}

/// One pass the compilation kept, in the order of declaration.
pub struct CompiledPass<B> {
    #[allow(dead_code, reason = "read by the tests, kept for a diagnosis")]
    pub name: &'static str,
    #[allow(dead_code, reason = "read by the tests, kept for a diagnosis")]
    pub reads: Vec<ResourceId>,
    pub writes: Vec<ResourceId>,
    pub body: B,
}

pub struct CompiledGraph<H, B> {
    pub passes: Vec<CompiledPass<B>>,
    /// The names of the passes removed because nothing reads what they write.
    pub removed: Vec<&'static str>,
    kinds: Vec<ResourceKind<H>>,
    /// Per resource, the first and the last kept pass that names it.
    lifetimes: Vec<Option<(u32, u32)>>,
}

impl<H: Copy, B> GraphBuilder<H, B> {
    pub fn new() -> GraphBuilder<H, B> {
        GraphBuilder::default()
    }

    /// Brings a resource owned outside the graph in for the frame. An output
    /// is read after the frame, by a read-back or by the frames that follow.
    pub fn import(&mut self, handle: H, output: bool) -> ResourceId {
        self.resources.push(Resource { kind: ResourceKind::Imported(handle), output });
        ResourceId(self.resources.len() as u32 - 1)
    }

    /// Declares a pass: `declare` names what the pass creates, reads and
    /// writes, and returns the body to run later.
    pub fn pass(&mut self, name: &'static str, declare: impl FnOnce(&mut PassBuilder<'_, H>) -> B) {
        let mut builder = PassBuilder { resources: &mut self.resources, reads: Vec::new(), writes: Vec::new() };
        let body = declare(&mut builder);
        let (reads, writes) = (builder.reads, builder.writes);
        self.passes.push(Pass { name, reads, writes, body });
    }

    pub fn pass_count(&self) -> usize {
        self.passes.len()
    }

    pub fn resource_count(&self) -> usize {
        self.resources.len()
    }

    /// Compiles the graph. With `remove_unread` off every pass is kept, as
    /// the reference mode has it.
    pub fn compile(self, remove_unread: bool) -> Result<CompiledGraph<H, B>, String> {
        let resource_count = self.resources.len();
        // A transient is read only after a pass wrote it.
        let mut written = vec![false; resource_count];
        for pass in &self.passes {
            for read in &pass.reads {
                if matches!(self.resources[read.0 as usize].kind, ResourceKind::Transient { .. }) && !written[read.0 as usize] {
                    return Err(format!("the pass \"{}\" reads a transient no earlier pass wrote", pass.name));
                }
            }
            for write in &pass.writes {
                written[write.0 as usize] = true;
            }
        }
        // From the last pass to the first: a pass is kept when it writes an
        // output or something a kept pass after it reads.
        let mut needed: Vec<bool> = self.resources.iter().map(|resource| resource.output).collect();
        let mut keep = vec![true; self.passes.len()];
        if remove_unread {
            for (index, pass) in self.passes.iter().enumerate().rev() {
                keep[index] = pass.writes.iter().any(|write| needed[write.0 as usize]);
                if keep[index] {
                    for read in &pass.reads {
                        needed[read.0 as usize] = true;
                    }
                }
            }
        }
        let mut lifetimes: Vec<Option<(u32, u32)>> = vec![None; resource_count];
        let mut passes = Vec::with_capacity(self.passes.len());
        let mut removed = Vec::new();
        for (pass, keep) in self.passes.into_iter().zip(keep) {
            if !keep {
                removed.push(pass.name);
                continue;
            }
            let position = passes.len() as u32;
            for resource in pass.reads.iter().chain(&pass.writes) {
                let lifetime = &mut lifetimes[resource.0 as usize];
                *lifetime = Some((lifetime.map_or(position, |(first, _)| first), position));
            }
            passes.push(CompiledPass { name: pass.name, reads: pass.reads, writes: pass.writes, body: pass.body });
        }
        Ok(CompiledGraph { passes, removed, kinds: self.resources.into_iter().map(|resource| resource.kind).collect(), lifetimes })
    }
}

impl<H: Copy, B> CompiledGraph<H, B> {
    pub fn kind(&self, resource: ResourceId) -> ResourceKind<H> {
        self.kinds[resource.0 as usize]
    }

    /// The first and the last kept pass that names the resource, if any does.
    #[allow(dead_code, reason = "read by the tests, kept for a diagnosis")]
    pub fn lifetime(&self, resource: ResourceId) -> Option<(u32, u32)> {
        self.lifetimes[resource.0 as usize]
    }

    /// The transients a kept pass names, with their sizes, in the order of creation.
    pub fn transients(&self) -> Vec<(ResourceId, u32, u32)> {
        self.kinds
            .iter()
            .enumerate()
            .filter_map(|(index, kind)| match kind {
                ResourceKind::Transient { width, height } if self.lifetimes[index].is_some() => Some((ResourceId(index as u32), *width, *height)),
                _ => None,
            })
            .collect()
    }

    /// Whether the pass at `position` is the first kept pass to name the resource.
    pub fn first_use(&self, resource: ResourceId, position: usize) -> bool {
        self.lifetimes[resource.0 as usize].is_some_and(|(first, _)| first as usize == position)
    }
}

/// What declaring and compiling a synthetic graph found, for the record.
pub struct SyntheticGraph {
    pub passes: usize,
    pub resources: usize,
    pub kept: usize,
    pub removed: usize,
    pub transients_alive: usize,
}

/// Declares and compiles a graph of `passes` passes and twice as many
/// resources: each pass creates two transients and reads up to three that
/// earlier passes wrote; one pass in five feeds nothing, and the last pass
/// writes the one output.
pub fn synthetic(passes: usize) -> SyntheticGraph {
    let mut builder: GraphBuilder<u32, (Vec<Read>, Vec<Write>)> = GraphBuilder::new();
    let output = builder.import(0, true);
    // The transients a later pass may read: those of the passes that feed something.
    let mut readable: Vec<ResourceId> = Vec::new();
    for index in 0..passes {
        let feeds_nothing = index % 5 == 4;
        let last = index == passes - 1;
        builder.pass("synthetic", |pass| {
            let mut reads = Vec::new();
            for step in 1..=3_usize {
                if readable.len() >= step * 2 && (index + step) % 2 == 0 {
                    reads.push(pass.read(readable[readable.len() - step * 2 + step % 2]));
                }
            }
            if last {
                for resource in readable.iter().rev().take(8) {
                    reads.push(pass.read(*resource));
                }
            }
            let mut writes = Vec::new();
            // The import above stands for one of the last pass's two resources.
            writes.push(pass.create(64, 64));
            if last {
                writes.push(pass.write(output));
            } else {
                writes.push(pass.create(64, 64));
            }
            if !feeds_nothing && !last {
                readable.extend(writes.iter().map(|write| write.resource()));
            }
            (reads, writes)
        });
    }
    let (declared, resources) = (builder.pass_count(), builder.resource_count());
    let compiled = builder.compile(true).expect("the synthetic graph reads only what was written");
    SyntheticGraph { passes: declared, resources, kept: compiled.passes.len(), removed: compiled.removed.len(), transients_alive: compiled.transients().len() }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn small(remove_unread: bool) -> CompiledGraph<&'static str, Option<Read>> {
        let mut builder = GraphBuilder::new();
        let target = builder.import("target", true);
        let mut scene = None;
        builder.pass("scene", |pass| {
            scene = Some(pass.create(8, 8).resource());
            None
        });
        builder.pass("mask", |pass| {
            pass.create(8, 8);
            None
        });
        builder.pass("composition", |pass| {
            pass.write(target);
            Some(pass.read(scene.unwrap()))
        });
        builder.compile(remove_unread).unwrap()
    }

    #[test]
    fn a_pass_whose_output_nothing_reads_is_removed_and_the_order_of_declaration_is_kept() {
        let compiled = small(true);
        assert_eq!(compiled.passes.iter().map(|pass| pass.name).collect::<Vec<_>>(), ["scene", "composition"]);
        assert_eq!(compiled.removed, ["mask"]);
        assert_eq!(compiled.transients().len(), 1);
        let scene = compiled.passes[1].body.unwrap().resource();
        assert_eq!(compiled.lifetime(scene), Some((0, 1)));
        assert!(compiled.first_use(scene, 0) && !compiled.first_use(scene, 1));
    }

    #[test]
    fn the_reference_mode_keeps_every_pass() {
        let compiled = small(false);
        assert_eq!(compiled.passes.len(), 3);
        assert_eq!(compiled.transients().len(), 2);
    }

    #[test]
    fn a_read_of_a_transient_nothing_wrote_is_refused() {
        // The identifier of a transient cannot be had before its creation
        // through one builder; it is taken from another graph here.
        let mut other: GraphBuilder<u32, ()> = GraphBuilder::new();
        let mut smuggled = None;
        other.pass("writer", |pass| smuggled = Some(pass.create(8, 8).resource()));
        let mut builder: GraphBuilder<u32, ()> = GraphBuilder::new();
        builder.pass("reader", |pass| {
            pass.read(smuggled.unwrap());
        });
        builder.pass("writer", |pass| {
            pass.create(8, 8);
        });
        assert!(builder.compile(true).is_err());
        assert!(other.compile(true).is_ok());
    }

    #[test]
    fn the_synthetic_graph_has_the_size_asked_and_loses_the_passes_that_feed_nothing() {
        let graph = synthetic(300);
        assert_eq!((graph.passes, graph.resources), (300, 600));
        assert!(graph.removed >= 60, "{} passes removed", graph.removed);
        assert_eq!(graph.kept + graph.removed, 300);
    }
}
