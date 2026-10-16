# 0004. Script gameplay in Rust and Luau

- Status: Accepted
- Date: 2026-10-15

## Context

Rust gives gameplay programmers full performance and determinism through
`ludenea_math`, but its compile times and borrow checker slow down
prototyping, and designers need faster iteration.

The candidates for a scripting layer were Lua, Rhai, WebAssembly components
and Luau. WebAssembly is a runtime rather than a language: studios need one
blessed language with instant iteration and first-class tooling. Luau, the
typed Lua dialect from Roblox, is MIT licensed, sandboxed, gradually typed,
has a language server (`luau-lsp`) and is supported by `mlua`.

A scripting language running inside the deterministic simulation must itself
be deterministic: math functions, table iteration order and garbage
collection must not make two runs diverge.

## Decision

Gameplay is written in two tiers:

- Rust, for gameplay programmers who need full performance;
- Luau, the blessed scripting language for fast iteration and designers.

Luau runs inside the simulation only if a determinism spike succeeds: math
routed to `ludenea_math`, a deterministic table iteration order, garbage
collection unobservable from the simulation, and identical state hashes
across x64, ARM64 and WebAssembly. If the spike fails, Luau is confined to
presentation code and editor tooling.

WebAssembly components are postponed, and considered only for sandboxed
third-party mods if demand appears.

## Consequences

- Teams get one fast-iteration language with mature tooling, and Rust
  remains available everywhere.
- The engine maintains Luau bindings generated from its reflection data.
- The spike result decides where Luau can run; a failure changes the scope
  of this decision and is recorded in a new record.
