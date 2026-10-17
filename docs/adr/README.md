# Architecture decision records

Each record explains one decision that shapes Ludenea: the context, the
decision itself and its consequences. They exist so contributors can trust
the design and challenge it with full knowledge of why it was chosen.

## Index

| Number                                          | Decision                                                       | Status   |
| ----------------------------------------------- | -------------------------------------------------------------- | -------- |
| [0001](0001-deterministic-core-first.md)        | Build a general-purpose engine around a deterministic core     | Accepted |
| [0002](0002-own-deterministic-ecs.md)           | Write our own deterministic ECS instead of using Bevy ECS      | Accepted |
| [0003](0003-strict-ieee-numeric-model.md)       | Use strict IEEE floating point with discipline                 | Accepted |
| [0004](0004-rust-and-luau-scripting.md)         | Script gameplay in Rust and Luau                               | Accepted |
| [0005](0005-web-editor-over-remote-protocol.md) | Build the editor as a TypeScript web application over LRP      | Accepted |
| [0006](0006-licensing.md)                       | License the runtime permissively and the editors under AGPLv3  | Accepted |
| [0007](0007-public-monorepo.md)                 | Keep the engine in one public monorepo                         | Accepted |
| [0008](0008-hexagonal-core-crates.md)           | Enforce hexagonal dependency rules on the core crates          | Accepted |
| [0009](0009-pinned-stable-toolchain.md)         | Pin a stable Rust toolchain                                    | Accepted |
| [0010](0010-cargo-xtask.md)                     | Use `cargo xtask` as the task runner                           | Accepted |
| [0011](0011-systems-without-unsafe.md)          | Lend world data to systems by column slot, without unsafe code | Proposed |

## Writing a record

1. Copy [template.md](template.md) to `NNNN-short-title.md`, using the next
   free number.
2. Open a pull request with the status `Proposed`; it becomes `Accepted` when
   merged.
3. Add the record to the index above.

A record is never rewritten once accepted. To change a decision, write a new
record and set the old one's status to `Superseded by NNNN`.
