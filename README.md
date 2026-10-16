# Ludenea

A deterministic game engine and editor suite built in Rust.

Ludenea is in its earliest stage: there is no usable code yet. This repository
will host the engine runtime, its SDK, the Ludenea Remote Protocol and, later,
the editor.

- **Deterministic core.** One input log, keyframes and snapshots power rollback
  netcode, replays, time-travel debugging and reproducible bug reports.
- **Headless first.** The engine is a simulation library; the editor, the
  command line, tests and AI agents are all clients of the same remote
  protocol.
- **Test-native.** A game simulation runs inside `cargo test` in milliseconds,
  with no window, GPU or audio device.

## Architecture decisions

The reasons behind the engine's design are recorded in the
[architecture decision records](docs/adr/README.md).

## Contributing

Issues are welcome. External pull requests cannot be merged yet; see
[CONTRIBUTING.md](CONTRIBUTING.md) for why and for how to report a problem.

## License

The repository uses two licenses, chosen per directory:

| Path                         | License                                                            |
| ---------------------------- | ------------------------------------------------------------------ |
| Everything outside `editor/` | [MIT](LICENSE-MIT) OR [Apache-2.0](LICENSE-APACHE), at your option |
| `editor/`                    | [GNU Affero General Public License v3.0](editor/LICENSE)           |

The runtime, SDK and protocol are permissively licensed so games built with
Ludenea can ship under any terms. There is no runtime fee and no royalty.
