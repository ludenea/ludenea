//! The "Formats and protocols" table of `CHANGELOG.md` names every version constant with its
//! current value, so a version bump without its changelog entry fails.

const CHANGELOG: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../CHANGELOG.md");
/// The engine's crates, whose sources declare the version constants.
const CRATES: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/..");
/// The editor's stored session format, whose version is a TypeScript constant.
const STORED_SESSION: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../editor/packages/editor-shell/src/stores/stored-session.ts"
);

#[test]
fn every_format_version_is_in_the_changelog() {
    let mut constants = vec![
        ("PROTOCOL_VERSION", ludenea::control::PROTOCOL_VERSION),
        (
            "PROJECT_PROTOCOL_VERSION",
            ludenea_project::protocol::PROJECT_PROTOCOL_VERSION,
        ),
        ("SCENE_VERSION", ludenea::app::scene::SCENE_VERSION),
        ("PREFAB_VERSION", ludenea::app::PREFAB_VERSION),
        ("META_VERSION", ludenea::app::meta::META_VERSION),
        ("ATLAS_VERSION", ludenea_render::atlas::ATLAS_VERSION),
        ("REPLAY_VERSION", ludenea::app::replay::REPLAY_VERSION),
        ("TRACE_VERSION", ludenea::app::trace::TRACE_VERSION),
        ("SNAPSHOT_VERSION", ludenea::ecs::SNAPSHOT_VERSION),
        ("INPUT_LOG_VERSION", ludenea::app::INPUT_LOG_VERSION),
        ("EDIT_LOG_VERSION", ludenea::app::EDIT_LOG_VERSION),
        ("UNDO_VERSION", ludenea::control::undo::UNDO_VERSION),
        ("STORED_SESSION_VERSION", stored_session_version()),
    ];
    constants.sort();
    let mut listed: Vec<&str> = constants.iter().map(|(name, _)| *name).collect();
    listed.retain(|name| *name != "STORED_SESSION_VERSION");
    assert_eq!(
        listed,
        declared_versions(),
        "the constants listed here (left) and the `pub const *_VERSION` of the crates (right) \
         differ: list the new constant"
    );
    let changelog = std::fs::read_to_string(CHANGELOG)
        .unwrap_or_else(|error| panic!("{CHANGELOG} cannot be read: {error}"));
    assert_eq!(
        documented_versions(&changelog),
        constants,
        "the table of {CHANGELOG} (left) and the version constants (right) differ: \
         update the table and record the change under Unreleased"
    );
}

/// The names of the `pub const <NAME>_VERSION` declared in the sources of the crates, sorted.
fn declared_versions() -> Vec<String> {
    fn scan(directory: &std::path::Path, names: &mut Vec<String>) {
        let entries = std::fs::read_dir(directory)
            .unwrap_or_else(|error| panic!("{} cannot be read: {error}", directory.display()));
        for entry in entries {
            let path = entry.unwrap().path();
            if path.is_dir() {
                scan(&path, names);
            } else if path.extension().is_some_and(|extension| extension == "rs") {
                let source = std::fs::read_to_string(&path).unwrap();
                names.extend(source.lines().filter_map(|line| {
                    let name = line.trim().strip_prefix("pub const ")?.split(':').next()?;
                    name.ends_with("_VERSION").then(|| name.to_string())
                }));
            }
        }
    }
    let mut names = Vec::new();
    scan(std::path::Path::new(CRATES), &mut names);
    names.sort();
    names
}

/// The value of `export const STORED_SESSION_VERSION = <n>;` in the editor's source.
fn stored_session_version() -> u32 {
    let source = std::fs::read_to_string(STORED_SESSION)
        .unwrap_or_else(|error| panic!("{STORED_SESSION} cannot be read: {error}"));
    source
        .lines()
        .find_map(|line| {
            line.strip_prefix("export const STORED_SESSION_VERSION = ")?
                .strip_suffix(';')?
                .parse()
                .ok()
        })
        .unwrap_or_else(|| {
            panic!("{STORED_SESSION} has no line `export const STORED_SESSION_VERSION = <n>;`")
        })
}

/// The constant and current version of each row of the table, sorted by constant; empty
/// without the section. Lines, not `\n`, so a checkout with CRLF line endings reads the same.
fn documented_versions(changelog: &str) -> Vec<(&str, u32)> {
    let mut rows: Vec<(&str, u32)> = changelog
        .lines()
        .skip_while(|line| *line != "## Formats and protocols")
        .skip_while(|line| !line.starts_with('|'))
        .take_while(|line| line.starts_with('|'))
        // The header and its delimiter row.
        .skip(2)
        .map(|row| {
            let cells: Vec<&str> = row.split('|').map(str::trim).collect();
            let [_, _format, constant, current, _oldest, _] = cells[..] else {
                panic!("the row `{row}` does not have the table's four columns");
            };
            let current = current.parse().unwrap_or_else(|_| {
                panic!("the current version `{current}` in the row `{row}` is not a number")
            });
            (constant.trim_matches('`'), current)
        })
        .collect();
    rows.sort();
    rows
}
