# 0008. Enforce hexagonal dependency rules on the core crates

- Status: Accepted
- Date: 2026-10-15

## Context

The simulation must run deterministically and headless, inside `cargo test`,
with no window, GPU, audio device or network. A single dependency doing I/O,
reading the wall clock or reading operating system randomness, even
transitively, breaks both properties. Code review alone does not catch a
transitive dependency.

## Decision

The workspace follows a hexagonal architecture. Core crates hold the
simulation and its domain; adapter crates connect it to the outside world
(protocol server, transports, rendering, persistence).

Core crates never depend, directly or transitively, on:

- adapter crates;
- crates performing I/O or providing an async runtime;
- crates reading operating system randomness.

`cargo xtask boundaries` checks these rules on the dependency graph and runs
in CI. The rules and the lists of adapter and forbidden crates live in
`xtask/src/boundaries.rs`.

## Consequences

- The simulation stays testable in milliseconds and portable to every
  target, including WebAssembly.
- Adding a dependency to a core crate can fail CI even when the crate itself
  is harmless, if one of its dependencies is forbidden.
- A new adapter crate must be declared in the boundaries rules.
- The forbidden crate lists are maintained by hand and can miss a new crate
  doing I/O; the determinism lints and golden hashes are the second line of
  defense.
