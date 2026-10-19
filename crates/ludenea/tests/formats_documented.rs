//! The "Formats and protocols" table of `CHANGELOG.md` names every version constant with its
//! current value, so a version bump without its changelog entry fails.

const CHANGELOG: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../CHANGELOG.md");

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
        ("REPLAY_VERSION", ludenea::app::replay::REPLAY_VERSION),
        ("TRACE_VERSION", ludenea::app::trace::TRACE_VERSION),
        ("SNAPSHOT_VERSION", ludenea::ecs::SNAPSHOT_VERSION),
        ("INPUT_LOG_VERSION", ludenea::app::INPUT_LOG_VERSION),
        ("EDIT_LOG_VERSION", ludenea::app::EDIT_LOG_VERSION),
        ("UNDO_VERSION", ludenea::control::undo::UNDO_VERSION),
    ];
    constants.sort();
    let changelog = std::fs::read_to_string(CHANGELOG)
        .unwrap_or_else(|error| panic!("{CHANGELOG} cannot be read: {error}"));
    assert_eq!(
        documented_versions(&changelog),
        constants,
        "the table of {CHANGELOG} (left) and the version constants (right) differ: \
         update the table and record the change under Unreleased"
    );
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
