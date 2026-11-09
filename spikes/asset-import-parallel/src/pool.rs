//! The background pool: worker threads, a ready queue of actions by
//! dependency counting, dynamic claiming by any free worker, parking when
//! nothing is ready. Throwaway, never product code.
//!
//! The pool owns `workers` threads for its whole life. A set of actions is
//! submitted as a batch: an action becomes ready when every action it depends
//! on has completed, any free worker claims the next ready action, a worker
//! with nothing ready parks on the batch's condition variable, and the
//! submitting thread returns when the whole set has completed. The workers
//! then park until the next batch or the pool's drop, which joins them. No
//! priority and no core budget: this spike measures the claiming and the
//! dependency counting alone.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, OnceLock};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use reference_workload::Fnv1a;

/// An action's output, or why it failed.
pub type ActionOutput = Result<Arc<Vec<u8>>, String>;

/// What runs an action: given its index and the outputs of its dependencies
/// in order, returns the output and whether the importer ran (true) or the
/// cache served it (false).
pub trait Execute: Fn(usize, &[Arc<Vec<u8>>]) -> Result<(Arc<Vec<u8>>, bool), String> + Send + Sync + 'static {}

impl<F> Execute for F where F: Fn(usize, &[Arc<Vec<u8>>]) -> Result<(Arc<Vec<u8>>, bool), String> + Send + Sync + 'static {}

/// What the pool reports about one batch.
pub struct BatchReport {
    pub workers: usize,
    pub outputs: Vec<ActionOutput>,
    /// Which actions the importer ran, as the executor said.
    pub imported: Vec<bool>,
    /// The actions in the order they completed.
    pub completion_order: Vec<usize>,
    /// From the submission to the last completion.
    pub wall: Duration,
    /// The sum over workers of the time spent waiting for a ready action
    /// (including the time to notice the batch).
    pub idle: Duration,
}

impl BatchReport {
    pub fn imported_count(&self) -> usize {
        self.imported.iter().filter(|&&imported| imported).count()
    }

    pub fn failures(&self) -> usize {
        self.outputs.iter().filter(|output| output.is_err()).count()
    }

    /// The digest of the completion sequence.
    pub fn completion_order_hash(&self) -> u64 {
        let mut hasher = Fnv1a::new();
        for &action in &self.completion_order {
            hasher.write(&(action as u32).to_le_bytes());
        }
        hasher.finish()
    }

    /// The idle time over the workers' wall time.
    pub fn idle_share(&self) -> f64 {
        let workers_wall = self.wall.as_secs_f64() * self.workers as f64;
        if workers_wall <= 0.0 {
            return 0.0;
        }
        (self.idle.as_secs_f64() / workers_wall).min(1.0)
    }
}

struct BatchState {
    remaining: Vec<usize>,
    ready: VecDeque<usize>,
    completion_order: Vec<usize>,
    /// The workers that entered the batch and those that left it; the
    /// submitting thread waits for both to agree before reading the idle time.
    entered: usize,
    exited: usize,
    /// The instant of the last completion, which caps the idle time of a
    /// worker that noticed the batch late.
    done_at: Option<Instant>,
    /// The sum over workers of the time spent waiting for a ready action.
    idle: Duration,
}

/// One submitted set of actions.
struct Batch {
    dependencies: Vec<Vec<usize>>,
    dependents: Vec<Vec<usize>>,
    execute: Box<dyn Execute>,
    outputs: Vec<OnceLock<ActionOutput>>,
    imported: Vec<AtomicBool>,
    state: Mutex<BatchState>,
    /// Workers park here when nothing is ready; woken by a completion.
    ready_signal: Condvar,
    /// The submitting thread parks here; woken by the last completion and by
    /// the last worker leaving.
    done_signal: Condvar,
    started: Instant,
}

impl Batch {
    fn total(&self) -> usize {
        self.dependencies.len()
    }

    fn is_done(state: &BatchState, total: usize) -> bool {
        state.completion_order.len() == total
    }

    /// Claims the next ready action, parking while nothing is ready and the
    /// batch is not done; `None` once the batch is done.
    fn claim(&self, idle: &mut Duration) -> Option<usize> {
        let mut state = self.state.lock().expect("the batch state is never poisoned");
        loop {
            if let Some(action) = state.ready.pop_front() {
                return Some(action);
            }
            if Self::is_done(&state, self.total()) {
                return None;
            }
            let parked = Instant::now();
            state = self.ready_signal.wait(state).expect("the batch state is never poisoned");
            *idle += parked.elapsed();
        }
    }

    fn run(&self, action: usize) {
        let mut dependency_outputs = Vec::with_capacity(self.dependencies[action].len());
        let mut failed_dependency = None;
        for &dependency in &self.dependencies[action] {
            // The dependency counting invariant: a ready action's dependencies have completed.
            let output = self.outputs[dependency].get().unwrap_or_else(|| panic!("dependency counting broke: action {action} claimed before its dependency {dependency} completed"));
            match output {
                Ok(bytes) => dependency_outputs.push(Arc::clone(bytes)),
                Err(error) => {
                    failed_dependency = Some(format!("dependency {dependency} failed: {error}"));
                    break;
                }
            }
        }
        let output = match failed_dependency {
            Some(error) => Err(error),
            None => (self.execute)(action, &dependency_outputs).map(|(bytes, imported)| {
                self.imported[action].store(imported, Ordering::Relaxed);
                bytes
            }),
        };
        if self.outputs[action].set(output).is_err() {
            panic!("action {action} completed twice");
        }
        let mut state = self.state.lock().expect("the batch state is never poisoned");
        state.completion_order.push(action);
        let mut newly_ready = 0;
        for &dependent in &self.dependents[action] {
            state.remaining[dependent] -= 1;
            if state.remaining[dependent] == 0 {
                state.ready.push_back(dependent);
                newly_ready += 1;
            }
        }
        if Self::is_done(&state, self.total()) {
            state.done_at = Some(Instant::now());
            drop(state);
            self.ready_signal.notify_all();
            self.done_signal.notify_all();
        } else {
            drop(state);
            for _ in 0..newly_ready {
                self.ready_signal.notify_one();
            }
        }
    }
}

struct PoolState {
    shutdown: bool,
    /// The current batch and its number; a worker that finished a batch
    /// waits for a batch with another number.
    batch: Option<(u64, Arc<Batch>)>,
    parked_workers: usize,
}

struct Shared {
    state: Mutex<PoolState>,
    wake: Condvar,
}

/// The pool: `workers` threads that live until the pool is dropped.
pub struct Pool {
    workers: Vec<JoinHandle<()>>,
    shared: Arc<Shared>,
}

impl Pool {
    pub fn new(workers: usize) -> Pool {
        assert!(workers >= 1, "a pool needs at least one worker");
        let shared = Arc::new(Shared { state: Mutex::new(PoolState { shutdown: false, batch: None, parked_workers: 0 }), wake: Condvar::new() });
        let workers = (0..workers)
            .map(|index| {
                let shared = Arc::clone(&shared);
                thread::Builder::new().name(format!("import-worker-{index}")).spawn(move || worker_loop(&shared)).expect("the worker thread spawns")
            })
            .collect();
        Pool { workers, shared }
    }


    /// The workers waiting for a batch.
    #[cfg(test)]
    pub fn parked_workers(&self) -> usize {
        self.shared.state.lock().expect("the pool state is never poisoned").parked_workers
    }

    #[cfg(test)]
    fn weak_shared(&self) -> std::sync::Weak<Shared> {
        Arc::downgrade(&self.shared)
    }

    /// Imports one set: `dependencies[i]` lists the actions `i` depends on
    /// (every index lower or higher, as long as the graph is acyclic).
    /// Returns when every action has completed.
    pub fn import(&self, dependencies: Vec<Vec<usize>>, execute: impl Execute) -> BatchReport {
        let total = dependencies.len();
        let mut dependents = vec![Vec::new(); total];
        for (action, action_dependencies) in dependencies.iter().enumerate() {
            for &dependency in action_dependencies {
                assert!(dependency < total && dependency != action, "action {action} depends on {dependency}");
                dependents[dependency].push(action);
            }
        }
        let remaining: Vec<usize> = dependencies.iter().map(Vec::len).collect();
        let ready: VecDeque<usize> = (0..total).filter(|&action| remaining[action] == 0).collect();
        let batch = Arc::new(Batch {
            dependents,
            execute: Box::new(execute),
            outputs: (0..total).map(|_| OnceLock::new()).collect(),
            imported: (0..total).map(|_| AtomicBool::new(false)).collect(),
            state: Mutex::new(BatchState {
                remaining,
                ready,
                completion_order: Vec::with_capacity(total),
                entered: 0,
                exited: 0,
                done_at: None,
                idle: Duration::ZERO,
            }),
            ready_signal: Condvar::new(),
            done_signal: Condvar::new(),
            started: Instant::now(),
            dependencies,
        });
        let number = BATCH_NUMBER.fetch_add(1, Ordering::Relaxed);
        {
            let mut state = self.shared.state.lock().expect("the pool state is never poisoned");
            state.batch = Some((number, Arc::clone(&batch)));
        }
        self.shared.wake.notify_all();
        let wall = {
            let mut state = batch.state.lock().expect("the batch state is never poisoned");
            while !Batch::is_done(&state, total) {
                state = batch.done_signal.wait(state).expect("the batch state is never poisoned");
            }
            state.done_at.map_or(Duration::ZERO, |done_at| done_at.duration_since(batch.started))
        };
        // No worker enters the batch after this; then wait for those inside to leave.
        {
            let mut state = self.shared.state.lock().expect("the pool state is never poisoned");
            state.batch = None;
        }
        let (completion_order, mut idle, entered) = {
            let mut state = batch.state.lock().expect("the batch state is never poisoned");
            while state.exited < state.entered {
                state = batch.done_signal.wait(state).expect("the batch state is never poisoned");
            }
            (state.completion_order.clone(), state.idle, state.entered)
        };
        // A worker that never noticed the batch was idle for its whole wall.
        idle += wall * (self.workers.len() - entered) as u32;
        let outputs = batch.outputs.iter().map(|slot| slot.get().cloned().unwrap_or_else(|| Err("action never completed".to_string()))).collect();
        let imported = batch.imported.iter().map(|flag| flag.load(Ordering::Relaxed)).collect();
        BatchReport { workers: self.workers.len(), outputs, imported, completion_order, wall, idle }
    }
}

/// Batch numbers, unique across pools so that a worker never mistakes a new
/// batch for the one it just finished.
static BATCH_NUMBER: AtomicU64 = AtomicU64::new(1);

impl Drop for Pool {
    fn drop(&mut self) {
        {
            let mut state = self.shared.state.lock().expect("the pool state is never poisoned");
            state.shutdown = true;
        }
        self.shared.wake.notify_all();
        for worker in self.workers.drain(..) {
            if worker.join().is_err() {
                eprintln!("an import worker panicked");
            }
        }
    }
}

fn worker_loop(shared: &Shared) {
    let mut last_batch = 0;
    loop {
        let batch = {
            let mut state = shared.state.lock().expect("the pool state is never poisoned");
            state.parked_workers += 1;
            let batch = loop {
                if state.shutdown {
                    state.parked_workers -= 1;
                    return;
                }
                match &state.batch {
                    Some((number, batch)) if *number != last_batch => {
                        last_batch = *number;
                        batch.state.lock().expect("the batch state is never poisoned").entered += 1;
                        break Arc::clone(batch);
                    }
                    _ => state = shared.wake.wait(state).expect("the pool state is never poisoned"),
                }
            };
            state.parked_workers -= 1;
            batch
        };
        // The time to notice the batch is idle time of this worker.
        let mut idle = batch.started.elapsed();
        while let Some(action) = batch.claim(&mut idle) {
            batch.run(action);
        }
        let mut state = batch.state.lock().expect("the batch state is never poisoned");
        // A worker that noticed the batch after its last completion was idle for the wall, no more.
        let wall = state.done_at.map_or(idle, |done_at| done_at.duration_since(batch.started));
        state.idle += idle.min(wall);
        state.exited += 1;
        if state.exited == state.entered {
            drop(state);
            batch.done_signal.notify_all();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::thread;
    use std::time::Duration;

    /// A graph of `count` actions where action `i` depends on a few lower
    /// indices drawn from a fixed pattern (no two tests need the same one).
    fn layered_graph(count: usize) -> Vec<Vec<usize>> {
        (0..count)
            .map(|i| {
                let mut dependencies = Vec::new();
                if i >= 1 && i % 3 != 0 {
                    dependencies.push(i - 1);
                }
                if i >= 7 && i % 5 == 0 {
                    dependencies.push(i / 2);
                }
                if i >= 20 && i % 4 == 1 {
                    dependencies.push(i - 20);
                }
                dependencies.sort_unstable();
                dependencies.dedup();
                dependencies
            })
            .collect()
    }

    /// The action's output: its index, then the bytes of its dependencies'
    /// outputs in order, so that a wrong dependency output shows.
    fn concatenating_execute(index: usize, dependencies: &[Arc<Vec<u8>>]) -> Result<(Arc<Vec<u8>>, bool), String> {
        let mut bytes = vec![index as u8];
        for dependency in dependencies {
            bytes.extend_from_slice(dependency);
        }
        Ok((Arc::new(bytes), true))
    }

    fn expected_output(graph: &[Vec<usize>], index: usize) -> Vec<u8> {
        let mut bytes = vec![index as u8];
        for &dependency in &graph[index] {
            bytes.extend_from_slice(&expected_output(graph, dependency));
        }
        bytes
    }

    #[test]
    fn a_dependent_is_never_completed_before_its_dependency_at_any_worker_count() {
        let graph = layered_graph(120);
        for workers in [1, 3, 8] {
            let pool = Pool::new(workers);
            let report = pool.import(graph.clone(), concatenating_execute);
            assert_eq!(report.completion_order.len(), 120, "{workers} workers");
            let mut position = vec![usize::MAX; 120];
            for (step, &action) in report.completion_order.iter().enumerate() {
                assert_eq!(position[action], usize::MAX, "action {action} completed twice");
                position[action] = step;
            }
            for (action, dependencies) in graph.iter().enumerate() {
                for &dependency in dependencies {
                    assert!(position[dependency] < position[action], "{workers} workers: {dependency} after {action}");
                }
            }
            for (action, output) in report.outputs.iter().enumerate() {
                assert_eq!(output.as_ref().unwrap().as_slice(), expected_output(&graph, action), "action {action}");
            }
            assert_eq!(report.imported_count(), 120);
            assert_eq!(report.failures(), 0);
            assert_eq!(report.workers, workers);
            assert!(report.idle_share() >= 0.0 && report.idle_share() <= 1.0, "{}", report.idle_share());
        }
    }

    #[test]
    fn one_worker_completes_in_the_ready_order_so_its_sequence_hash_is_stable() {
        let graph = layered_graph(60);
        let first = Pool::new(1).import(graph.clone(), concatenating_execute);
        let second = Pool::new(1).import(graph, concatenating_execute);
        assert_eq!(first.completion_order, second.completion_order);
        assert_eq!(first.completion_order_hash(), second.completion_order_hash());
        assert_ne!(first.completion_order_hash(), 0);
    }

    #[test]
    fn any_free_worker_claims_the_next_ready_action() {
        // Action 0 is slow and nothing depends on it: with four workers the
        // others complete before it, so the completion order is not the index order.
        let graph: Vec<Vec<usize>> = (0..12).map(|_| Vec::new()).collect();
        let pool = Pool::new(4);
        let report = pool.import(graph, |index, _| {
            thread::sleep(Duration::from_millis(if index == 0 { 80 } else { 2 }));
            Ok((Arc::new(vec![index as u8]), index % 2 == 0))
        });
        assert_eq!(*report.completion_order.last().unwrap(), 0);
        assert_eq!(report.imported_count(), 6, "the executor says which actions it imported");
        assert!(report.wall >= Duration::from_millis(80));
        assert!(report.wall < Duration::from_millis(80 + 11 * 2), "the fast actions overlap the slow one: {:?}", report.wall);
    }

    #[test]
    fn a_failed_action_fails_its_dependents_and_nothing_else() {
        let graph = vec![vec![], vec![], vec![0], vec![1], vec![3], vec![0, 1]];
        let pool = Pool::new(2);
        let report = pool.import(graph, |index, dependencies| {
            if index == 1 {
                return Err("corrupt source".to_string());
            }
            concatenating_execute(index, dependencies)
        });
        assert!(report.outputs[0].is_ok() && report.outputs[2].is_ok());
        assert_eq!(report.outputs[1].as_ref().unwrap_err(), "corrupt source");
        assert!(report.outputs[3].as_ref().unwrap_err().contains("dependency 1 failed: corrupt source"));
        assert!(report.outputs[4].as_ref().unwrap_err().contains("dependency 3 failed"));
        assert!(report.outputs[5].as_ref().unwrap_err().contains("dependency 1 failed"));
        assert_eq!(report.failures(), 4);
        assert_eq!(report.imported_count(), 2);
        assert_eq!(report.completion_order.len(), 6, "a failed action still completes");
    }

    #[test]
    fn the_pool_parks_when_the_set_is_imported_and_exits_when_dropped() {
        let pool = Pool::new(3);
        let weak = pool.weak_shared();
        let graph = layered_graph(30);
        let counter = Arc::new(AtomicUsize::new(0));
        let seen = Arc::clone(&counter);
        pool.import(graph.clone(), move |index, dependencies| {
            seen.fetch_add(1, Ordering::Relaxed);
            concatenating_execute(index, dependencies)
        });
        assert_eq!(counter.load(Ordering::Relaxed), 30);
        let parked_at = std::time::Instant::now();
        while pool.parked_workers() != 3 {
            assert!(parked_at.elapsed() < Duration::from_secs(5), "the workers did not park: {}", pool.parked_workers());
            thread::yield_now();
        }
        // A second set on the same pool runs as well.
        let again = pool.import(graph, concatenating_execute);
        assert_eq!(again.completion_order.len(), 30);
        drop(pool);
        assert!(weak.upgrade().is_none(), "every worker released the shared state, so every worker exited");
    }

    #[test]
    fn an_empty_set_completes_at_once() {
        let report = Pool::new(2).import(Vec::new(), concatenating_execute);
        assert!(report.completion_order.is_empty());
        assert_eq!(report.imported_count(), 0);
        assert_eq!(report.idle_share(), 0.0);
    }
}
