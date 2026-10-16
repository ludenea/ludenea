# 0009. Pin a stable Rust toolchain

- Status: Accepted
- Date: 2026-10-15

## Context

The engine's numeric results are checked by golden hashes. A compiler change
can, in rare cases, change code generation, lints or formatting, and an
unpinned toolchain makes CI results depend on the day they run. Nightly
features would also expose the engine to changes it cannot control.

## Decision

The toolchain is pinned to an exact stable release in `rust-toolchain.toml`,
with its components and targets. Nightly is never used. The minimum
supported Rust version declared in `Cargo.toml` matches the pinned release.
Upgrading the toolchain is a dedicated change that must pass the full CI,
golden hashes included.

## Consequences

- Every contributor and every CI run builds with the same compiler.
- A toolchain upgrade is a deliberate, reviewable change.
- Features available only on nightly are out of reach until they are
  stabilized.
