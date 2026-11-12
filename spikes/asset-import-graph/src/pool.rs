//! The pool of the import graph spike: dependency counting with one central
//! ready set. Throwaway, never product code.
//!
//! An action becomes ready when every action it is known to depend on has
//! completed; any free worker claims the next ready action. An action may
//! end with a request for another action's output: it then waits behind that
//! action and is run again from its start once it has completed. No action
//! adds an action from inside a worker. A cycle among requests fails the
//! actions on it, naming the cycle; an action that asks twice for the same
//! output fails as non-deterministic in its requests; a failed action fails
//! its dependents and nothing else. The pool measures the share of their
//! time the workers spend outside an action, which is what the ready set and
//! its lock cost.

use std::sync::{Condvar, Mutex};
use std::thread;
use std::time::{Duration, Instant};

/// How one run of an action ends.
pub enum Outcome {
    Done,
    /// The output of another action is needed and not there yet.
    Request(usize),
    Failed(String),
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Status {
    Blocked,
    Ready,
    Running,
    Done,
    Failed,
}

struct State {
    status: Vec<Status>,
    /// Dependencies still to complete, per action.
    pending: Vec<usize>,
    dependents: Vec<Vec<usize>>,
    ready: Vec<usize>,
    /// The action each action waits for after a request.
    waiting_on: Vec<Option<usize>>,
    requested: Vec<Vec<usize>>,
    running: usize,
    remaining: usize,
    restarts: usize,
    failures: Vec<(usize, String)>,
}

impl State {
    fn fail(&mut self, action: usize, reason: String) {
        let mut open = vec![(action, reason)];
        while let Some((action, reason)) = open.pop() {
            if matches!(self.status[action], Status::Done | Status::Failed) {
                continue;
            }
            self.status[action] = Status::Failed;
            self.remaining -= 1;
            for dependent in std::mem::take(&mut self.dependents[action]) {
                open.push((dependent, format!("action {action} it depends on failed: {reason}")));
            }
            self.failures.push((action, reason));
        }
    }

    fn complete(&mut self, action: usize) -> usize {
        self.status[action] = Status::Done;
        self.remaining -= 1;
        let mut released = 0;
        for dependent in std::mem::take(&mut self.dependents[action]) {
            self.pending[dependent] -= 1;
            if self.pending[dependent] == 0 && self.status[dependent] == Status::Blocked {
                self.waiting_on[dependent] = None;
                self.status[dependent] = Status::Ready;
                self.ready.push(dependent);
                released += 1;
            }
        }
        released
    }

    /// An action ended with a request for `target`'s output.
    fn request(&mut self, action: usize, target: usize) -> usize {
        self.restarts += 1;
        if self.requested[action].contains(&target) {
            self.fail(action, format!("it asked twice for the output of action {target}: non-deterministic in its requests"));
            return 0;
        }
        self.requested[action].push(target);
        match self.status[target] {
            Status::Done => {
                self.status[action] = Status::Ready;
                self.ready.push(action);
                1
            }
            Status::Failed => {
                self.fail(action, format!("action {target} it asked for failed"));
                0
            }
            _ => {
                // A cycle: the target waits, directly or not, for this action.
                let mut cycle = vec![action, target];
                let mut at = target;
                while let Some(next) = self.waiting_on[at] {
                    cycle.push(next);
                    if next == action {
                        let named = cycle.iter().map(usize::to_string).collect::<Vec<_>>().join(" -> ");
                        for member in cycle {
                            self.fail(member, format!("a cycle of requests: {named}"));
                        }
                        return 0;
                    }
                    at = next;
                }
                self.status[action] = Status::Blocked;
                self.waiting_on[action] = Some(target);
                self.pending[action] = 1;
                self.dependents[target].push(action);
                0
            }
        }
    }
}

/// What a run of a graph gave.
pub struct RunReport {
    pub wall: Duration,
    /// The share of the workers' time spent outside an action.
    pub waiting_share: f64,
    /// The runs that ended with a request.
    pub restarts: usize,
    pub failures: Vec<(usize, String)>,
}

/// Runs every action of a graph on `workers` threads. `dependencies[i]` are
/// the actions `i` is known to depend on; `execute(action, worker)` runs one.
pub fn run(workers: usize, dependencies: &[Vec<usize>], execute: &(dyn Fn(usize, usize) -> Outcome + Sync)) -> RunReport {
    let count = dependencies.len();
    let mut state = State {
        status: vec![Status::Blocked; count],
        pending: dependencies.iter().map(Vec::len).collect(),
        dependents: vec![Vec::new(); count],
        ready: Vec::new(),
        waiting_on: vec![None; count],
        requested: vec![Vec::new(); count],
        running: 0,
        remaining: count,
        restarts: 0,
        failures: Vec::new(),
    };
    for (action, known) in dependencies.iter().enumerate() {
        for dependency in known {
            state.dependents[*dependency].push(action);
        }
    }
    // The ready set is claimed from its end: the lowest index first.
    for action in (0..count).rev() {
        if state.pending[action] == 0 {
            state.status[action] = Status::Ready;
            state.ready.push(action);
        }
    }
    let state = Mutex::new(state);
    let wake = Condvar::new();
    let started = Instant::now();
    let work = |worker: usize| -> (Duration, Duration) {
        let alive = Instant::now();
        let mut executing = Duration::ZERO;
        let mut guard = state.lock().expect("the pool's state is never poisoned");
        loop {
            let Some(action) = guard.ready.pop() else {
                if guard.remaining == 0 {
                    break;
                }
                if guard.running == 0 {
                    // Nothing runs and nothing is ready: what is left waits for itself.
                    let stuck: Vec<usize> = (0..count).filter(|action| guard.status[*action] == Status::Blocked).collect();
                    let named = stuck.iter().map(usize::to_string).collect::<Vec<_>>().join(", ");
                    for action in stuck {
                        guard.fail(action, format!("a cycle among the dependencies of the actions {named}"));
                    }
                    wake.notify_all();
                    break;
                }
                guard = wake.wait(guard).expect("the pool's state is never poisoned");
                continue;
            };
            guard.status[action] = Status::Running;
            guard.running += 1;
            drop(guard);
            let before = Instant::now();
            let outcome = execute(action, worker);
            executing += before.elapsed();
            guard = state.lock().expect("the pool's state is never poisoned");
            guard.running -= 1;
            let released = match outcome {
                Outcome::Done => guard.complete(action),
                Outcome::Request(target) => guard.request(action, target),
                Outcome::Failed(reason) => {
                    guard.fail(action, reason);
                    0
                }
            };
            if guard.remaining == 0 || guard.running == 0 {
                wake.notify_all();
            } else {
                // This worker takes one of the released actions itself.
                for _ in 1..released {
                    wake.notify_one();
                }
            }
        }
        drop(guard);
        (executing, alive.elapsed())
    };
    let times: Vec<(Duration, Duration)> = thread::scope(|scope| {
        let handles: Vec<_> = (1..workers).map(|worker| scope.spawn(move || work(worker))).collect();
        let mut times = vec![work(0)];
        times.extend(handles.into_iter().map(|handle| handle.join().expect("a worker never panics")));
        times
    });
    let wall = started.elapsed();
    let executing: f64 = times.iter().map(|(executing, _)| executing.as_secs_f64()).sum();
    let alive: f64 = times.iter().map(|(_, alive)| alive.as_secs_f64()).sum();
    let state = state.into_inner().expect("the pool's state is never poisoned");
    let mut failures = state.failures;
    failures.sort();
    RunReport { wall, waiting_share: if alive > 0.0 { 1.0 - executing / alive } else { 0.0 }, restarts: state.restarts, failures }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[test]
    fn a_dependent_never_runs_before_its_dependency() {
        for workers in [1, 3, 8] {
            let dependencies: Vec<Vec<usize>> = (0..200).map(|action| if action % 5 == 0 { Vec::new() } else { vec![action - action % 5] }).collect();
            let done: Vec<AtomicUsize> = (0..200).map(|_| AtomicUsize::new(0)).collect();
            let report = run(workers, &dependencies, &|action, _| {
                assert!(dependencies[action].iter().all(|dependency| done[*dependency].load(Ordering::SeqCst) == 1));
                done[action].fetch_add(1, Ordering::SeqCst);
                Outcome::Done
            });
            assert!(report.failures.is_empty() && report.restarts == 0);
            assert!(done.iter().all(|count| count.load(Ordering::SeqCst) == 1));
        }
    }

    #[test]
    fn a_request_runs_the_action_again_once_its_target_is_done() {
        for workers in [1, 4] {
            let dependencies = vec![Vec::new(); 20];
            let runs: Vec<AtomicUsize> = (0..20).map(|_| AtomicUsize::new(0)).collect();
            let done: Vec<AtomicUsize> = (0..20).map(|_| AtomicUsize::new(0)).collect();
            let report = run(workers, &dependencies, &|action, _| {
                runs[action].fetch_add(1, Ordering::SeqCst);
                // Every even action needs the odd one after it.
                if action % 2 == 0 && done[action + 1].load(Ordering::SeqCst) == 0 {
                    return Outcome::Request(action + 1);
                }
                done[action].store(1, Ordering::SeqCst);
                Outcome::Done
            });
            assert!(report.failures.is_empty(), "{:?}", report.failures);
            assert!(done.iter().all(|count| count.load(Ordering::SeqCst) == 1));
            assert_eq!(report.restarts, runs.iter().map(|count| count.load(Ordering::SeqCst)).sum::<usize>() - 20);
        }
    }

    #[test]
    fn a_cycle_of_requests_fails_the_actions_on_it_and_the_run_ends() {
        for workers in [1, 4] {
            let dependencies = vec![Vec::new(), Vec::new(), Vec::new(), vec![0]];
            let report = run(workers, &dependencies, &|action, _| match action {
                0 => Outcome::Request(1),
                1 => Outcome::Request(0),
                _ => Outcome::Done,
            });
            let failed: Vec<usize> = report.failures.iter().map(|(action, _)| *action).collect();
            assert_eq!(failed, [0, 1, 3], "the cycle and its dependent: {:?}", report.failures);
            assert!(report.failures[0].1.contains("cycle") && report.failures[1].1.contains("cycle"));
        }
    }

    #[test]
    fn a_repeated_request_and_a_declared_cycle_fail() {
        let report = run(2, &[Vec::new(), Vec::new()], &|action, _| if action == 0 { Outcome::Request(1) } else { Outcome::Done });
        assert_eq!(report.failures.len(), 1);
        assert!(report.failures[0].1.contains("non-deterministic"), "{:?}", report.failures);
        let report = run(2, &[vec![1], vec![0], Vec::new()], &|_, _| Outcome::Done);
        assert_eq!(report.failures.len(), 2);
        let report = run(2, &[Vec::new(), vec![0]], &|action, _| if action == 0 { Outcome::Failed("no".to_string()) } else { Outcome::Done });
        assert_eq!(report.failures.len(), 2);
    }
}
