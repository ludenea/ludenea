# Changelog

Every change to the public Rust API, the control protocol, the project protocol, the plugin
API, or a file format is recorded here, under [Unreleased](#unreleased), in the commit that
makes it. The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/); the
rules for versions are in the [README](README.md#versioning).

## Formats and protocols

The version each format or protocol writes (Current) and the oldest version this build still
reads. A test checks the Current column against the version constants, so a bump updates
this table in the same commit.

| Format                 | Constant                   | Current | Oldest readable               |
| ---------------------- | -------------------------- | ------- | ----------------------------- |
| Control protocol       | `PROTOCOL_VERSION`         | 3       | 3                             |
| Project protocol       | `PROJECT_PROTOCOL_VERSION` | 2       | 2                             |
| Scene (`.scene.ron`)   | `SCENE_VERSION`            | 2       | 1                             |
| Prefab (`.prefab.ron`) | `PREFAB_VERSION`           | 1       | 1                             |
| Meta (`.meta`)         | `META_VERSION`             | 1       | 1                             |
| Replay (`.replay`)     | `REPLAY_VERSION`           | 1       | 1                             |
| Trace (`.trace`)       | `TRACE_VERSION`            | 1       | 1                             |
| Snapshot (`.snap`)     | `SNAPSHOT_VERSION`         | 2       | 2                             |
| Input log (`.inputs`)  | `INPUT_LOG_VERSION`        | 1       | 1                             |
| Edit log               | `EDIT_LOG_VERSION`         | 2       | 1                             |
| Undo history           | `UNDO_VERSION`             | 3       | the unversioned form before 2 |

Not versioned yet: data assets (`.ron` files holding a value of the game type their meta
names), the project manifest `ludenea.toml`, the branch tree of a session, the stored
session (the editor's copy of each open document's session), and the plugin API.

## [Unreleased]

### Rust API

- `ludenea_app::scene::SCENE_VERSION` is public, like the other format versions.

### Control protocol

No changes yet.

### Project protocol

No changes yet.

### Plugin API

No changes yet.

### Formats

No changes yet.
