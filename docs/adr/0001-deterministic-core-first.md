# 0001. Build a general-purpose engine around a deterministic core

- Status: Accepted
- Date: 2026-10-15

## Context

Studios shipping competitive multiplayer games on general-purpose engines
repeatedly build the same missing pieces late and at great cost: a
deterministic simulation, cheap snapshot and restore, rollback-safe entity
identity, desync diagnosis and replay recording. NetherRealm reported two to
eight man-years to retrofit rollback into Mortal Kombat X. Photon Quantum
turned that gap into a proprietary product on top of Unity.

A single mechanism covers all of these needs: an input log, periodic
keyframes and a ring buffer of snapshots. With it, any tick can be reached by
restoring the nearest keyframe and re-simulating. That mechanism only works
if determinism is designed in from the first line of code; it cannot be added
later.

Competing on high-end rendering with the largest engines is a decade-scale
effort and cannot be the first product.

## Decision

Ludenea is a general-purpose engine whose first focus is a rollback-capable
deterministic simulation core. The simulation is a headless library: the same
inputs produce the same state on every run, platform, build profile and
thread count.

The first games targeted are small online games for two to four players
(action, party, cooperative) using rollback netcode, where the simulation
state is small and snapshots are cheap. Lockstep and server-authoritative
networking come later on the same deterministic core.

## Consequences

- Rollback netcode, replays, time-travel debugging, regression tests from
  recordings and reproducible bug reports all derive from one mechanism.
- Every simulation crate carries determinism constraints (see
  [0003](0003-strict-ieee-numeric-model.md) and
  [0008](0008-hexagonal-core-crates.md)), and CI must prove determinism on
  every target.
- A deterministic model exposes the full game state to every peer and a
  single floating point bug can end a session; server-authoritative models
  remain needed for games where that is unacceptable.
- The architecture must not prevent high-end rendering, but the first
  releases do not depend on it.
