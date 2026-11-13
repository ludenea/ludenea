//! The calls a world's steps really make to the platform's math library,
//! counted by a debugger where one is at hand (GDB, on Linux).
//!
//! Each world is run again as a child under the debugger, with a breakpoint
//! on every function of the platform's math library a plain `f32` or `f64`
//! method can reach. The child calls a marker when its build ends and its
//! steps begin: the hits counted before it belong to the build, the hits
//! after it to the steps, and the first hit of each function after the
//! marker prints the chain of callers down to the step.
//!
//! The breakpoints are set once the program has reached `main`: several of
//! the library's functions are chosen at load time among variants written
//! for the processor at hand, and a breakpoint set before the choice lands on
//! the code that chooses, which a call never runs again.

use std::env;
use std::fs;
use std::path::Path;
use std::process::{Command, Stdio};

use reference_workload::Json;

use crate::scenes::{KINDS, Kind};

/// The ticks a world steps under the debugger: past the two changes the
/// bodies of several colliders receive.
const TICKS: usize = 320;
const MARKER: &str = "rapier_platform_math_steps_begin";
/// The functions of the platform's math library a breakpoint is set on, with
/// the address this program itself reaches each at once the loader has bound
/// it: a breakpoint by name would land, for a function chosen at load time,
/// on the code that chose.
macro_rules! platform_functions {
    ($($name:ident),* $(,)?) => {
        const FUNCTIONS: &[&str] = &[$(stringify!($name)),*];

        #[cfg(target_os = "linux")]
        mod imported {
            // Only the addresses are taken: the signatures do not matter.
            #[allow(clashing_extern_declarations)]
            unsafe extern "C" {
                $(pub fn $name();)*
            }
        }

        #[cfg(target_os = "linux")]
        fn address(index: usize) -> usize {
            let all: &[unsafe extern "C" fn()] = &[$(imported::$name),*];
            all.get(index).map(|function| *function as usize).unwrap_or(0)
        }

        #[cfg(not(target_os = "linux"))]
        fn address(_index: usize) -> usize {
            0
        }
    };
}

// The single precision functions a method on an `f32` calls, and the double
// precision ones a method on an `f64` would.
platform_functions!(
    sinf, cosf, sincosf, tanf, asinf, acosf, atanf, atan2f, expf, exp2f, logf, log2f, log10f, powf,
    sin, cos, sincos, tan, asin, acos, atan, atan2, exp, exp2, log, log2, log10, pow,
);

/// What the debugger calls, once the program has reached `main`, to learn
/// where to set the breakpoint of the function at an index.
#[unsafe(no_mangle)]
#[inline(never)]
pub extern "C" fn rapier_platform_math_address(index: u32) -> usize {
    address(index as usize)
}
/// Breakpoint 1 is the temporary one on `main`, breakpoint 2 the marker, and
/// the functions follow in order.
const FIRST_FUNCTION_BREAKPOINT: usize = 3;
const BUILD: &str = "===BUILD===";
const STEPS_BEGIN: &str = "===STEPS_BEGIN===";
const END: &str = "===END===";
const CALLERS: &str = "===CALLERS ";
/// The lines of the libraries' sources the reading found a call on, in the
/// versions the build pins: a line that is met while no call is counted is a
/// call the compiler computed.
const LINES: [(&str, &str); 6] = [
    ("the logarithms of the broad phase's tree", "bvh_optimize.rs:26"),
    ("the sine and cosine of half the center of an angular limit", "joint_constraint_helper.rs:64"),
    ("the sine and cosine of one degree, on a convex polyhedron", "convex_polyhedron.rs:873"),
    ("the sine of half a motor's target", "generic_joint_constraint_builder.rs:769"),
    ("the arc cosine of the eigenvalues", "eigen3.rs:101"),
    ("the cosines of the eigenvalues", "eigen3.rs:105"),
];

fn debugger() -> Option<String> {
    if !cfg!(target_os = "linux") {
        return None;
    }
    let output = Command::new("gdb").arg("--version").stdin(Stdio::null()).output().ok()?;
    output.status.success().then(|| String::from_utf8_lossy(&output.stdout).lines().next().unwrap_or_default().to_string())
}

/// The debugger's script. A function's first hit prints its callers and then
/// stops printing; the marker prints the counts so far and lets each function
/// print its callers once more.
fn script() -> String {
    let mut script = String::from("set pagination off\nset confirm off\nset breakpoint pending on\nset print thread-events off\ntbreak main\nrun\nset language c\n");
    script.push_str(&format!("break {MARKER}\n"));
    for index in 0..FUNCTIONS.len() {
        script.push_str(&format!("break *((unsigned long (*)(unsigned int)) rapier_platform_math_address)({index})\n"));
    }
    // A line's breakpoint counts and never stops.
    for (index, (_, line)) in LINES.iter().enumerate() {
        script.push_str(&format!("break {line}\nignore {} 1000000000\n", index + FIRST_FUNCTION_BREAKPOINT + FUNCTIONS.len()));
    }
    script.push_str(&format!("commands 2\nsilent\necho {BUILD}\\n\ninfo breakpoints\n"));
    for index in 0..FUNCTIONS.len() {
        script.push_str(&format!("ignore {} 0\n", index + FIRST_FUNCTION_BREAKPOINT));
    }
    script.push_str(&format!("echo {STEPS_BEGIN}\\n\ncontinue\nend\n"));
    for (index, function) in FUNCTIONS.iter().enumerate() {
        let number = index + FIRST_FUNCTION_BREAKPOINT;
        script.push_str(&format!("commands {number}\nsilent\necho {CALLERS}{function}===\\n\nbacktrace 24\nignore {number} 1000000000\ncontinue\nend\n"));
    }
    script.push_str(&format!("continue\necho {END}\\n\ninfo breakpoints\n"));
    script
}

/// The lines of a log between the line that is `from` and the line that is
/// `to`, or the end. A marker inside a line, as the listing of a breakpoint's
/// commands prints it, is not one.
fn section<'a>(log: &'a str, from: &str, to: Option<&str>) -> Vec<&'a str> {
    log.lines().skip_while(|line| line.trim_end() != from).skip(1).take_while(|line| to.is_none_or(|to| line.trim_end() != to)).collect()
}

/// The hits of each function in one `info breakpoints` listing, by the
/// number of its breakpoint. A breakpoint never hit has no count line.
fn hits(listing: &[&str]) -> Vec<u64> {
    let mut counts = vec![0; FUNCTIONS.len() + LINES.len()];
    let mut current = None;
    for line in listing {
        let first = line.split_whitespace().next().unwrap_or_default();
        if !line.starts_with(char::is_whitespace) && first.chars().all(|character| character.is_ascii_digit()) {
            current = first.parse::<usize>().ok();
        } else if let (Some(number), Some(count)) = (current, line.trim().strip_prefix("breakpoint already hit ")) {
            if let Some(slot) = number.checked_sub(FIRST_FUNCTION_BREAKPOINT).and_then(|index| counts.get_mut(index)) {
                *slot = count.split_whitespace().next().and_then(|count| count.parse().ok()).unwrap_or(0);
            }
        }
    }
    counts
}

/// The first chain of callers printed for each function, innermost first,
/// as the names of the functions alone.
fn callers(steps: &[&str]) -> Vec<(String, String)> {
    let mut chains: Vec<(String, Vec<String>)> = Vec::new();
    let mut reading = false;
    for line in steps {
        if let Some(function) = line.strip_prefix(CALLERS).and_then(|rest| rest.trim_end().strip_suffix("===")) {
            reading = !chains.iter().any(|(known, _)| known == function);
            if reading {
                chains.push((function.to_string(), Vec::new()));
            }
        } else if reading && line.starts_with('#') {
            let frame = line.split_once(" in ").map(|(_, rest)| rest).unwrap_or_else(|| line.split_once(char::is_whitespace).map(|(_, rest)| rest.trim_start()).unwrap_or_default());
            if let (Some(name), Some((_, frames))) = (frame.split(" (").next(), chains.last_mut()) {
                frames.push(name.to_string());
            }
        }
    }
    chains.into_iter().map(|(function, frames)| (function, frames.join(" < "))).collect()
}

fn named(names: &[&str], counts: &[u64]) -> String {
    names.iter().zip(counts).filter(|(_, count)| **count > 0).map(|(function, count)| format!("{function} {count}")).collect::<Vec<_>>().join(", ")
}

pub fn write(out: &mut Json, output: &Path) {
    out.key("reach");
    out.object_start();
    let Some(version) = debugger() else {
        out.bool("measured", false);
        out.string("why", "the calls are counted by GDB on Linux, and this machine has neither");
        out.object_end();
        return;
    };
    out.bool("measured", true);
    out.string("debugger", &version);
    // Keeps the function the debugger calls in the program.
    out.bool("the_program_names_its_functions", (0..FUNCTIONS.len()).all(|index| std::hint::black_box(rapier_platform_math_address(index as u32)) != 0));
    out.u64("ticks", TICKS as u64);
    let directory = output.join("reach");
    let _ = fs::create_dir_all(&directory);
    let script_path = directory.join("breakpoints.gdb");
    let _ = fs::write(&script_path, script());
    out.key("worlds");
    out.array_start();
    for kind in KINDS {
        out.object_start();
        out.string("world", kind.name());
        out.string("call_read_in_the_sources", kind.call());
        match run_under_the_debugger(kind, &script_path) {
            Ok(log) => {
                let _ = fs::write(directory.join(format!("{}.txt", kind.name())), &log);
                let before = hits(&section(&log, BUILD, Some(STEPS_BEGIN)));
                let total = hits(&section(&log, END, None));
                let during: Vec<u64> = total.iter().zip(&before).map(|(total, before)| total.saturating_sub(*before)).collect();
                let described: Vec<String> = LINES.iter().map(|(what, line)| format!("{line} ({what})")).collect();
                let lines: Vec<&str> = described.iter().map(String::as_str).collect();
                let functions = FUNCTIONS.len();
                out.string("calls_during_the_build", &named(FUNCTIONS, &before[..functions]));
                out.string("calls_during_the_steps", &named(FUNCTIONS, &during[..functions]));
                out.u64("calls_during_the_steps_in_all", during[..functions].iter().sum());
                out.string("lines_met_during_the_build", &named(&lines, &before[functions..]));
                out.string("lines_met_during_the_steps", &named(&lines, &during[functions..]));
                out.key("first_callers_during_the_steps");
                out.object_start();
                for (function, chain) in callers(&section(&log, STEPS_BEGIN, Some(END))) {
                    out.string(&function, &chain);
                }
                out.object_end();
            }
            Err(reason) => out.string("error", &reason),
        }
        out.object_end();
    }
    out.array_end();
    out.object_end();
}

fn run_under_the_debugger(kind: Kind, script: &Path) -> Result<String, String> {
    let executable = env::current_exe().map_err(|error| error.to_string())?;
    let output = Command::new("gdb")
        .args(["--batch", "--nx", "-x"])
        .arg(script)
        .arg("--args")
        .arg(executable)
        .args(["--scene", kind.name(), &TICKS.to_string()])
        .stdin(Stdio::null())
        .output()
        .map_err(|error| format!("the debugger could not be started: {error}"))?;
    let log = String::from_utf8_lossy(&output.stdout).into_owned();
    if section(&log, STEPS_BEGIN, None).is_empty() || !log.lines().any(|line| line.trim_end() == END) {
        return Err(format!("the debugger did not follow the run to its end: {}", String::from_utf8_lossy(&output.stderr).split_whitespace().collect::<Vec<_>>().join(" ").chars().take(300).collect::<String>()));
    }
    Ok(log)
}

#[cfg(test)]
mod tests {
    use super::*;

    const LOG: &str = "Temporary breakpoint 1, main ()
===BUILD===
Num     Type           Disp Enb Address            What
2       breakpoint     keep y   0x000055555557a1c0 in rapier_platform_math_steps_begin at src/scenes.rs:300
\tbreakpoint already hit 1 time
        silent
        echo ===BUILD===\\n
3       breakpoint     keep y   0x00007ffff7e4a2b0 <__sinf_fma>
\tbreakpoint already hit 12 times
\tignore next 999999988 hits
        echo ===CALLERS sinf===\\n
4       breakpoint     keep y   0x00007ffff7e4a2f0 <__cosf_fma>
14      breakpoint     keep y   <MULTIPLE>
\tbreakpoint already hit 3 times
14.1                        y   0x00007ffff7e4b000 <log2f>
===STEPS_BEGIN===
===CALLERS sinf===
#0  0x1 in __sinf_fma () from /lib/x86_64-linux-gnu/libm.so.6
#1  0x2 in rapier3d::limit (a=1) at helper.rs:64
#2  step () at pipeline.rs:1
===CALLERS sinf===
#0  0x1 in other () at x.c:1
===END===
Num     Type           Disp Enb Address            What
3       breakpoint     keep y   0x00007ffff7e4a2b0 <__sinf_fma>
\tbreakpoint already hit 40 times
";

    #[test]
    fn the_hits_are_read_by_breakpoint_number_from_a_listing() {
        let before = hits(&section(LOG, BUILD, Some(STEPS_BEGIN)));
        assert_eq!((before[0], before[1], before[11]), (12, 0, 3));
        assert_eq!(before.iter().sum::<u64>(), 15);
        let total = hits(&section(LOG, END, None));
        assert_eq!(total[0], 40);
        assert_eq!(named(FUNCTIONS, &total[..FUNCTIONS.len()]), "sinf 40");
    }

    #[test]
    fn the_callers_are_read_innermost_first_and_once_per_function() {
        let chains = callers(&section(LOG, STEPS_BEGIN, Some(END)));
        assert_eq!(chains, vec![("sinf".to_string(), "__sinf_fma < rapier3d::limit < step".to_string())]);
    }

    #[test]
    fn a_marker_printed_inside_a_listing_starts_no_section() {
        assert_eq!(section(LOG, BUILD, Some(STEPS_BEGIN)).len(), 13);
        assert!(section("echo ===BUILD===\\n\n", BUILD, None).is_empty());
    }

    #[test]
    fn the_script_numbers_its_breakpoints_after_main_and_the_marker() {
        let script = script();
        assert!(script.contains(&format!("commands {}\n", FUNCTIONS.len() + FIRST_FUNCTION_BREAKPOINT - 1)));
        assert!(!script.contains(&format!("commands {}\n", FUNCTIONS.len() + FIRST_FUNCTION_BREAKPOINT)));
        assert!(script.find("tbreak main\nrun\nset language c\n").unwrap() < script.find("rapier_platform_math_address)(0)\n").unwrap());
        assert_eq!(FUNCTIONS.len(), 28);
        assert_eq!((FUNCTIONS[0], FUNCTIONS[11]), ("sinf", "log2f"));
    }
}
