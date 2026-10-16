# 0002. Write our own deterministic ECS instead of using Bevy ECS

- Status: Accepted
- Date: 2026-10-15

## Context

Bevy ECS is the most mature entity component system in Rust and would save
a large amount of initial work. It is, however, designed for a general
real-time engine, not for rollback:

- Its entity allocator reuses identifiers from free lists in an order that is
  not part of any snapshot, so restoring a world does not yield identical
  identifiers. Rollback libraries for Bevy add a separate identity layer to
  work around this.
- Its scheduler tolerates ambiguous ordering between systems; Bevy added a
  shuffle seed to expose the resulting bugs rather than forbid them.
- Its storage is not laid out to be snapshotted and restored with a memory
  copy.
- Its core models (hierarchy, events, rendering) changed significantly in
  recent releases, and it is still before 1.0.

## Decision

Ludenea writes its own ECS, `ludenea_ecs`, kept small and deterministic by
design:

- storage that can be snapshotted and restored with a memory copy;
- an entity allocator that is part of the snapshot, so rollback restores
  identical identifiers;
- a total order between systems with conflicting data access, where an
  ambiguous order is an error.

Compatibility with the Bevy Remote Protocol remains possible as a bridge for
tools, without depending on Bevy's runtime.

## Consequences

- Determinism and rollback are properties of the ECS itself, not layers on
  top of it.
- Ludenea controls the pace of change of its core data model.
- The ECS, its scheduler and its query engine must be written, tested and
  optimized in-house, and the Bevy plugin ecosystem cannot be reused
  directly.
