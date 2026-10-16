# 0003. Use strict IEEE floating point with discipline

- Status: Accepted
- Date: 2026-10-15

## Context

A deterministic simulation needs bit-identical numeric results across x64,
ARM64 and WebAssembly. Two approaches exist:

- Fixed point arithmetic, as in Photon Quantum (Q48.16): deterministic by
  construction, but slower, with a limited range and a less familiar
  programming model.
- IEEE floating point with discipline, the approach of Box2D v3, Rapier's
  enhanced determinism mode and Jolt's cross-platform mode: IEEE basic
  operations are exactly specified, and divergence comes from identifiable
  sources such as fused multiply-add, platform math libraries, SIMD width,
  NaN payloads, hash map seeds and thread completion order.

A simulation running on a single thread leaves modern hardware unused, but
thread completion order is itself a source of nondeterminism.

## Decision

Ludenea uses strict IEEE floating point with the following discipline in the
simulation crates:

- no fused multiply-add, explicit or contracted;
- transcendental functions only through portable implementations in
  `ludenea_math`, never the platform's;
- a pinned SIMD width per determinism domain;
- NaNs canonicalized before hashing;
- ordered or fixed-hash containers only, and no wall clock.

Fixed point is available as an opt-in for games that need it.

The simulation is multithreaded, but only through engine primitives whose
results are merged in a canonical order (per-thread buffers merged
deterministically, in the spirit of Box2D's per-thread bit arrays). Results
must not depend on the thread count.

## Consequences

- Developers write ordinary floating point code, at native speed.
- The rules are enforced by Clippy lints in each simulation crate
  (`clippy.toml`), which name the allowed alternative for every forbidden
  method or type. Raw threads stay forbidden until the deterministic
  parallel primitives exist.
- CI computes a golden hash on every target, build profile and thread count;
  changing the reference is a reviewed change of the engine's numeric
  results.
- Dependencies used by the simulation must follow the same discipline, which
  narrows the choice of third-party crates.
