//! What the rows judged on a magnitude or on a ceiling share: a median, the
//! arguments of a mode, and where a mode puts what it measured. Throwaway,
//! never product code.
//!
//! Such a row has inputs and no oracle. Its mode prints one JSON document of
//! facts on standard output and a short summary on standard error, and
//! writes its full log to a file. A magnitude is judged on one quiet machine,
//! in the release profile, over at least five repetitions: a mode takes the
//! count of repetitions, and says under which profile and at which size it
//! ran, so that a run made to check that it works is not read as a
//! measurement.

use std::path::PathBuf;
use std::time::Duration;

use reference_workload::{Json, write_platform_and_build};

/// The repetitions of a mode where its arguments name none.
pub const REPETITIONS: usize = 5;

/// The median of a set of durations: the middle one, or the mean of the two
/// middle ones.
pub fn median(durations: &mut [Duration]) -> Duration {
    assert!(!durations.is_empty(), "a median is taken of at least one duration");
    durations.sort_unstable();
    let middle = durations.len() / 2;
    if durations.len().is_multiple_of(2) { (durations[middle - 1] + durations[middle]) / 2 } else { durations[middle] }
}

/// One duration over another.
pub fn ratio(of: Duration, to: Duration) -> f64 {
    of.as_secs_f64() / to.as_secs_f64()
}

/// A ratio written with six digits after the point: a cost under a
/// thousandth of a tick is still a figure.
pub fn write_ratio(out: &mut Json, key: &str, value: f64) {
    out.raw(key, &format!("{value:.6}"));
}

pub fn nanoseconds(duration: Duration) -> u64 {
    duration.as_nanos() as u64
}

/// The arguments of a mode: `[repetitions] [small]`, in either order. The
/// count of repetitions, the stated one where none is given, and whether the
/// mode runs at its small size, which is the size of its self-check and
/// judges nothing.
pub fn arguments(arguments: &[String]) -> Result<(usize, bool), String> {
    let (mut repetitions, mut small) = (REPETITIONS, false);
    for argument in arguments {
        match argument.as_str() {
            "small" => small = true,
            count => match count.parse::<usize>() {
                Ok(count) if count >= 1 => repetitions = count,
                _ => return Err(format!("`{count}` is neither a count of repetitions of at least one nor the word `small`")),
            },
        }
    }
    Ok((repetitions, small))
}

/// The arguments of a mode, or the end of the process with what is wrong
/// with them.
pub fn arguments_or_exit(mode: &str, given: &[String]) -> (usize, bool) {
    arguments(given).unwrap_or_else(|error| {
        eprintln!("{mode} [repetitions] [small]: {error}");
        std::process::exit(2);
    })
}

/// What a mode starts its facts with: its name, the platform and the build,
/// the repetitions, and whether its figures may be read as a measurement.
pub fn head(out: &mut Json, mode: &str, repetitions: usize, small: bool) {
    out.string("prototype", "asset-table-and-loader");
    out.string("mode", mode);
    write_platform_and_build(out);
    out.u64("repetitions", repetitions as u64);
    out.string("size", if small { "small, the size of the self-check" } else { "as the sheet states it" });
    out.bool("profile_and_size_of_a_measurement", measures(small));
}

/// Whether a run is made under the conditions of a measurement the mode can
/// see: the release profile and the size the sheet states. Whether the
/// machine is quiet is for whoever runs the mode to see.
pub fn measures(small: bool) -> bool {
    !small && !cfg!(debug_assertions)
}

/// Under what a mode ran: its repetitions, its profile and its size.
pub fn profile_and_size(repetitions: usize, small: bool) -> String {
    let profile = if cfg!(debug_assertions) { "the unoptimized profile" } else { "the release profile" };
    let size = if small { "its small size" } else { "the size its sheet states" };
    format!("{repetitions} repetitions in {profile} at {size}")
}

/// The sentence the summary of a magnitude opens with.
pub fn conditions(repetitions: usize, small: bool) -> String {
    let read = if measures(small) && repetitions >= REPETITIONS {
        "a measurement where the machine was quiet"
    } else {
        "an indication and no measurement"
    };
    format!("{}: {read}", profile_and_size(repetitions, small))
}

/// Where the full log of a mode is written: beside the facts the run script
/// keeps, where that directory exists, and in the working directory
/// otherwise.
pub fn log_path(mode: &str) -> PathBuf {
    let beside = PathBuf::from("out").join(mode);
    if beside.is_dir() { beside.join("log.txt") } else { PathBuf::from(format!("asset-table-and-loader-{mode}.log")) }
}

/// Ends a mode: the facts on standard output, the summary on standard
/// error, the full log in its file, then the one line the mode closes with.
pub fn finish(mode: &str, facts: Json, summary: &str, log: &str, closing: &str) {
    let path = log_path(mode);
    let written = std::fs::write(&path, format!("{summary}\n{log}"));
    println!("{}", facts.finish());
    eprint!("{summary}");
    match written {
        Ok(()) => eprintln!("full log: {}", path.display()),
        Err(error) => eprintln!("the full log could not be written to {}: {error}", path.display()),
    }
    eprintln!("{closing}");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_median_is_the_middle_duration_or_the_mean_of_the_two_middle_ones() {
        let nanos = |values: &[u64]| values.iter().map(|value| Duration::from_nanos(*value)).collect::<Vec<_>>();
        assert_eq!(median(&mut nanos(&[9, 1, 5])), Duration::from_nanos(5));
        assert_eq!(median(&mut nanos(&[9, 1, 5, 7])), Duration::from_nanos(6));
        assert_eq!(median(&mut nanos(&[4])), Duration::from_nanos(4));
    }

    #[test]
    fn a_mode_reads_its_repetitions_and_its_size_and_refuses_anything_else() {
        let of = |words: &[&str]| arguments(&words.iter().map(|word| (*word).to_string()).collect::<Vec<_>>());
        assert_eq!(of(&[]), Ok((REPETITIONS, false)));
        assert_eq!(of(&["7"]), Ok((7, false)));
        assert_eq!(of(&["small", "2"]), Ok((2, true)));
        assert!(of(&["0"]).is_err());
        assert!(of(&["large"]).is_err());
    }
}
