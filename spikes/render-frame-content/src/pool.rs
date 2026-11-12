//! The frame pool: workers that run the frame-bound tasks of a world
//! (refresh, prepare, recording) outside the tick graph, and the instrument
//! that measures them as the scheduler measures a node. Throwaway, never
//! product code.
//!
//! A pool is created for one world with that world's worker budget. A stage
//! the stepping loop waits for (refresh, prepare) is run by the budget's
//! workers, the stepping thread being one of them; a stage it does not wait
//! for (recording) is run by the pool's own threads alone, one fewer than
//! the budget, or by the stepping thread itself at a budget of one.
//!
//! The skeleton's world spawns its own workers for each step, so this pool
//! is a second set of threads: while a recording overlaps a tick the two
//! sets run at once, which the concurrency model's budget would not allow.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::Instant;

pub type Task = Box<dyn FnOnce() + Send + 'static>;
type Then = Box<dyn FnOnce(StageTime) + Send + 'static>;

/// What a stage cost: the time from its start to its last task's end, the
/// sum of its tasks' durations, and their count.
#[derive(Clone, Copy, Debug, Default)]
pub struct StageTime {
    pub wall_us: f64,
    pub processor_us: f64,
    pub tasks: u64,
}

struct Batch {
    remaining: AtomicUsize,
    processor_ns: AtomicU64,
    tasks: u64,
    started: Instant,
    finished: Mutex<Option<StageTime>>,
    done: Condvar,
    then: Mutex<Option<Then>>,
}

impl Batch {
    fn new(tasks: usize, then: Option<Then>) -> Arc<Batch> {
        Arc::new(Batch {
            remaining: AtomicUsize::new(tasks),
            processor_ns: AtomicU64::new(0),
            tasks: tasks as u64,
            started: Instant::now(),
            finished: Mutex::new(None),
            done: Condvar::new(),
            then: Mutex::new(then),
        })
    }

    fn run(self: &Arc<Batch>, task: Task) {
        let started = Instant::now();
        task();
        self.processor_ns.fetch_add(started.elapsed().as_nanos() as u64, Ordering::Relaxed);
        if self.remaining.fetch_sub(1, Ordering::AcqRel) == 1 {
            let time = StageTime { wall_us: self.started.elapsed().as_secs_f64() * 1e6, processor_us: self.processor_ns.load(Ordering::Relaxed) as f64 / 1e3, tasks: self.tasks };
            let then = self.then.lock().unwrap().take();
            if let Some(then) = then {
                then(time);
            }
            *self.finished.lock().unwrap() = Some(time);
            self.done.notify_all();
        }
    }
}

struct Queue {
    entries: VecDeque<(Arc<Batch>, Task)>,
    closed: bool,
}

struct Shared {
    queue: Mutex<Queue>,
    work: Condvar,
}

pub struct FramePool {
    shared: Arc<Shared>,
    threads: Vec<std::thread::JoinHandle<()>>,
}

impl FramePool {
    /// A pool for a world of this worker budget: one fewer threads than the
    /// budget, the stepping thread being the budget's first worker.
    pub fn new(budget: usize) -> FramePool {
        assert!(budget >= 1, "a world's worker budget is at least its stepping thread");
        let shared = Arc::new(Shared { queue: Mutex::new(Queue { entries: VecDeque::new(), closed: false }), work: Condvar::new() });
        let threads = (1..budget)
            .map(|_| {
                let shared = Arc::clone(&shared);
                std::thread::spawn(move || {
                    loop {
                        let mut queue = shared.queue.lock().unwrap();
                        let (batch, task) = loop {
                            if let Some(entry) = queue.entries.pop_front() {
                                break entry;
                            }
                            if queue.closed {
                                return;
                            }
                            queue = shared.work.wait(queue).unwrap();
                        };
                        drop(queue);
                        batch.run(task);
                    }
                })
            })
            .collect();
        FramePool { shared, threads }
    }

    fn enqueue(&self, tasks: Vec<Task>, then: Option<Then>) -> Arc<Batch> {
        let batch = Batch::new(tasks.len(), then);
        let mut queue = self.shared.queue.lock().unwrap();
        for task in tasks {
            queue.entries.push_back((Arc::clone(&batch), task));
        }
        drop(queue);
        self.shared.work.notify_all();
        batch
    }

    /// Runs a stage the stepping loop waits for: the calling thread works
    /// with the pool's threads until every task has run.
    pub fn run(&self, tasks: Vec<Task>) -> StageTime {
        if tasks.is_empty() {
            return StageTime::default();
        }
        let batch = self.enqueue(tasks, None);
        loop {
            let entry = self.shared.queue.lock().unwrap().entries.pop_front();
            match entry {
                Some((other, task)) => other.run(task),
                None => break,
            }
        }
        let mut finished = batch.finished.lock().unwrap();
        while finished.is_none() {
            finished = batch.done.wait(finished).unwrap();
        }
        finished.unwrap()
    }

    /// Starts a stage the stepping loop does not wait for; `then` runs on the
    /// thread that ends the last task. At a budget of one the calling thread
    /// runs the tasks itself, in order, before this returns.
    pub fn start(&self, tasks: Vec<Task>, then: impl FnOnce(StageTime) + Send + 'static) {
        if tasks.is_empty() {
            then(StageTime::default());
            return;
        }
        if self.threads.is_empty() {
            let batch = Batch::new(tasks.len(), Some(Box::new(then)));
            for task in tasks {
                batch.run(task);
            }
            return;
        }
        self.enqueue(tasks, Some(Box::new(then)));
    }
}

impl Drop for FramePool {
    fn drop(&mut self) {
        self.shared.queue.lock().unwrap().closed = true;
        self.shared.work.notify_all();
        for thread in self.threads.drain(..) {
            let _ = thread.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;

    #[test]
    fn a_waited_stage_runs_every_task_at_every_budget() {
        for budget in [1, 2, 4] {
            let pool = FramePool::new(budget);
            let counter = Arc::new(AtomicU64::new(0));
            let tasks: Vec<Task> = (0..100_u64)
                .map(|value| {
                    let counter = Arc::clone(&counter);
                    Box::new(move || {
                        counter.fetch_add(value, Ordering::Relaxed);
                    }) as Task
                })
                .collect();
            let time = pool.run(tasks);
            assert_eq!(counter.load(Ordering::Relaxed), 4_950);
            assert_eq!(time.tasks, 100);
        }
    }

    #[test]
    fn a_started_stage_calls_back_once_when_its_last_task_ends() {
        for budget in [1, 3] {
            let pool = FramePool::new(budget);
            let (sender, receiver) = mpsc::channel();
            let tasks: Vec<Task> = (0..16).map(|_| Box::new(std::thread::yield_now) as Task).collect();
            pool.start(tasks, move |time| sender.send(time.tasks).unwrap());
            assert_eq!(receiver.recv().unwrap(), 16);
            assert!(receiver.try_recv().is_err());
        }
    }
}
