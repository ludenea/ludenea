# 0006. License the runtime permissively and the editors under AGPLv3

- Status: Accepted
- Date: 2026-10-15

## Context

Studios refuse copyleft licenses in shipped games, and console platform
agreements forbid them in runtimes. MIT OR Apache-2.0 is the Rust norm.

Engine users have also learned to distrust vendors: retroactive runtime fees
and license changes (Unity in 2023, The Machinery in 2022) and relicensing of
open source projects (HashiCorp, Redis) broke that trust. Licenses such as
BSL, SSPL and Elastic License v2 are not open source and led to community
forks.

The editors and servers need protection against closed forks offered as a
service, without giving up the open source label.

## Decision

| Component                                       | License           |
| ----------------------------------------------- | ----------------- |
| Runtime, SDK, LRP specification and client SDKs | MIT OR Apache-2.0 |
| Editors, collaboration server, relay server     | AGPLv3            |

Contributions to AGPLv3 parts require a contributor license agreement, so
those parts can also be offered under a commercial license. The licenses are
chosen once and are not changed later. There is no runtime fee and no
royalty on the runtime.

## Consequences

- Games built with Ludenea can ship under any terms.
- The editors and servers are fully self-hostable, which answers the fear of
  a vendor shutting down.
- Closed forks of the editors offered as a service are blocked by the
  AGPLv3.
- The repository uses two licenses chosen per directory, and external pull
  requests to AGPLv3 parts cannot be merged until the contributor license
  agreement exists.
