# 0007. Keep the engine in one public monorepo

- Status: Accepted
- Date: 2026-10-15

## Context

The runtime, the SDK, the protocol and the editor evolve together: a change
to the protocol touches the engine, the generated clients and the editor at
once. Splitting them across repositories would turn each such change into
several coordinated releases.

## Decision

The engine lives in one public repository on GitHub, `ludenea/ludenea`: the
Rust workspace, the protocol and the editor, with licenses chosen per
directory (see [0006](0006-licensing.md)).

## Consequences

- A cross-cutting change is one pull request, tested by one CI run.
- Contributors find the whole open source project in one place.
- CI must handle several languages and licenses in one repository.
