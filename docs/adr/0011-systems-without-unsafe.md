# 0011. Lend world data to systems by column slot, without unsafe code

- Status: Accepted
- Date: 2026-10-17

## Context

Systems are the main way gameplay code touches the world, so their
ergonomics shape the developer experience of the engine. Bevy lets a system
be a plain function whose typed parameters (`Query`, `Res`, `ResMut`,
`Commands`) declare what it accesses, and several queries can write
different data in the same system. Bevy gets there through
`UnsafeWorldCell`, which hands out aliasing pointers that the scheduler
promises never to overlap.

The workspace denies `unsafe_code`. The options considered were:

- declaring the access of a system by hand and checking it at run time,
  which writes every access twice;
- one query per system, which is too restrictive for gameplay code;
- function systems with typed parameters, which need several mutable
  borrows of one world at the same time.

## Decision

Ludenea uses function systems with typed parameters and lends the world to
them by column slot, in safe Rust.

Before each system run, the schedule takes the world mutably once and
splits it into disjoint borrows: one slot per column of every archetype and
one per resource. Each parameter takes the slots it declared. A written
column is taken out of its slot, once; a read column is turned into a
shared borrow left in place, so several parameters can read it. When the
schedule is built, the parameters of each system are checked against each
other: two parameters writing the same component in archetypes that can
overlap, given their `With` and `Without` filters, make the system panic. Taking a slot
therefore never fails at run time.

The same declared access lets the schedule reject two conflicting systems
left unordered, which keeps the execution order deterministic and explicit.

## Consequences

- Gameplay code reads like Bevy: two queries writing the same component
  through disjoint filters, or a query reading other entities while another
  writes, need no workaround.
- No `unsafe` block exists in the engine for system access, so a mistake in
  the scheduler cannot cause undefined behaviour, only a panic.
- Every system run builds a table of slots over every archetype, a cost
  linear in the number of columns. It is accepted until the benchmark
  harness shows it; lending only the archetypes a system matches is the
  planned upgrade.
- A system has at most one `Commands` parameter, and writes to overlapping
  archetypes in one system are rejected, as in Bevy.
- Systems take their parameters by value, so the pedantic Clippy lint
  `needless_pass_by_value` is allowed for the whole workspace.
- Running systems in parallel would need the world split across threads.
  It is not planned: parallelism happens inside systems, through
  primitives with a deterministic merge. Reopening that choice would
  reopen this decision.
