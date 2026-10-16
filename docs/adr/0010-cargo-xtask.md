# 0010. Use `cargo xtask` as the task runner

- Status: Accepted
- Date: 2026-10-15

## Context

The repository needs automation beyond `cargo` itself: the boundaries check,
golden hash computation and comparison, the commit format check, and a
single command running everything CI runs. Shell scripts are not portable to
Windows, and tools such as `make` or `just` add a dependency every
contributor must install.

## Decision

Repository automation is a Rust binary in the workspace, `xtask`, invoked
with `cargo xtask <command>` through a cargo alias. `cargo xtask ci` runs
exactly what CI runs.

## Consequences

- Automation is written, tested and linted in Rust like the rest of the
  workspace, and works wherever cargo works.
- Contributors need nothing beyond the pinned toolchain and the tools listed
  in `AGENTS.md`.
- The first `cargo xtask` call compiles the binary, which is slower than a
  script.
