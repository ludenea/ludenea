//! The Box2D half as the parent sees it: the C program that the spike's
//! setup script builds is found, run as a child process, and its report (one
//! JSON document on its standard output) taken as it is. A missing or
//! failing child is a recorded result, never fatal to the Rapier half.
//! Throwaway, never product code.

use std::env;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use crate::measure::OtherCell;

const PROGRAM: &str = "physics-prepared-geometry-box2d";
const OVERRIDE_VARIABLE: &str = "PREPARED_GEOMETRY_BOX2D";

/// The build directory of the setup script: the unoptimized build of the
/// spike runs the unoptimized build of the C program.
fn build_directory() -> &'static str {
    if cfg!(debug_assertions) { "physics-prepared-geometry-box2d-debug" } else { "physics-prepared-geometry-box2d" }
}

/// The executable: the path the environment names when it names one, else
/// the installed program under the working directory, else under the spikes
/// directory this crate was built in.
fn locate() -> Result<PathBuf, String> {
    if let Some(path) = env::var_os(OVERRIDE_VARIABLE).filter(|value| !value.is_empty()) {
        let path = PathBuf::from(path);
        return if path.is_file() { Ok(path) } else { Err(format!("{OVERRIDE_VARIABLE} names {}, which is not a file", path.display())) };
    }
    let installed = Path::new("build").join(build_directory()).join("install").join("bin").join(format!("{PROGRAM}{}", env::consts::EXE_SUFFIX));
    let candidates = [installed.clone(), Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join(&installed)];
    candidates.iter().find(|candidate| candidate.is_file()).cloned().ok_or_else(|| {
        format!(
            "the Box2D program was not found at {} nor at {}; the spike's setup.sh builds it",
            candidates[0].display(),
            candidates[1].display()
        )
    })
}

/// Whether the text is one JSON object as far as its strings, braces and
/// brackets go: enough to refuse a truncated or foreign output before it is
/// embedded in the report.
pub fn is_one_json_object(text: &str) -> bool {
    let text = text.trim();
    let mut open = Vec::new();
    let (mut in_string, mut escaped, mut closed) = (false, false, false);
    for character in text.chars() {
        if closed {
            return false;
        }
        if in_string {
            match character {
                _ if escaped => escaped = false,
                '\\' => escaped = true,
                '"' => in_string = false,
                _ => {}
            }
            continue;
        }
        match character {
            '"' => in_string = true,
            '{' | '[' => open.push(character),
            '}' | ']' => {
                let expected = if character == '}' { '{' } else { '[' };
                if open.pop() != Some(expected) {
                    return false;
                }
                closed = open.is_empty();
            }
            _ if open.is_empty() => return false,
            _ => {}
        }
    }
    closed && text.starts_with('{')
}

/// Runs the Box2D program and gives its report. The child's progress goes to
/// this process's standard error. Standard C lists no directory, so the
/// names of the other cells' directories follow their root on the command
/// line.
pub fn run(cell: &str, quick: bool, forms_directory: &Path, other_cells_root: &Path, other_cells: &[OtherCell]) -> Result<String, String> {
    let program = locate()?;
    let mut command = Command::new(&program);
    command.arg(cell).arg(forms_directory).arg(other_cells_root).args(other_cells.iter().map(|other| &other.name));
    if quick {
        command.arg("--quick");
    }
    let output = command
        .stdin(Stdio::null())
        .stderr(Stdio::inherit())
        .output()
        .map_err(|error| format!("{} could not be run: {error}", program.display()))?;
    if !output.status.success() {
        return Err(format!("{} ended with {}", program.display(), output.status));
    }
    let report = String::from_utf8(output.stdout).map_err(|error| format!("the Box2D program's output is not UTF-8: {error}"))?;
    if is_one_json_object(&report) { Ok(report) } else { Err(String::from("the Box2D program's output is not one JSON document")) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_json_object_is_recognized() {
        assert!(is_one_json_object("{}"));
        assert!(is_one_json_object(" {\"a\":[1,{\"b\":\"}\\\"]\"}],\"c\":true}\n"));
    }

    #[test]
    fn a_truncated_or_foreign_output_is_refused() {
        for text in ["", "[]", "{\"a\":1", "{\"a\":[1}", "{} trailing", "Box2D: a log line\n{}", "{\"a\":\"unclosed}"] {
            assert!(!is_one_json_object(text), "{text:?}");
        }
    }

    #[test]
    fn the_build_directory_follows_the_profile() {
        assert_eq!(build_directory().ends_with("-debug"), cfg!(debug_assertions));
    }
}
