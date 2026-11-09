//! A small persistent worker pool shared by the worlds of one process.
//! Throwaway, never product code.
//!
//! The pool owns helper threads for its whole life. A world that steps offers
//! tickets for its job (one per helper it may use, its worker budget minus the
//! calling thread); an idle helper takes the oldest ticket, works for that job
//! until the job's step ends, then comes back for another ticket. The calling
//! thread is always worker 0 of its own step, so a step never waits for a
//! helper: a pool whose helpers all serve another world leaves the step to its
//! calling thread alone. Before the step returns, the caller withdraws the
//! tickets nobody took and waits for the helpers that joined to leave, so a
//! job is never touched after its step.
//!
//! This is coarser than the persistent pool spike's design, where a worker
//! claims tasks from any active job: here a helper stays with one job for the
//! rest of that job's step. It is the smallest pool that lets two worlds
//! share helpers, which is what the interference measurement needs.

use std::collections::VecDeque;
use std::sync::{Arc, Condvar, Mutex};
use std::thread::{self, JoinHandle};

/// What a pool helper needs from the job it serves.
pub trait Job: Sync {
    /// Called under the pool's lock when a helper takes a ticket of this job.
    fn helper_joined(&self);
    /// Works for the job as worker `worker` until the job's step ends. Its
    /// last access to the job must be the announcement that the helper left.
    fn help(&self, worker: usize);
    /// Helpers that joined and have not left yet.
    fn helpers_active(&self) -> usize;
}

struct Ticket {
    job: *const dyn Job,
    worker: usize,
}

// SAFETY: the pointer is dereferenced by a helper only between the moment it
// takes the ticket under the lock and the moment `help` returns, and the job's
// owner keeps the job alive until both the queue holds no ticket of it and no
// helper is active (see `Pool::retire`).
unsafe impl Send for Ticket {}

#[derive(Default)]
struct Queue {
    tickets: VecDeque<Ticket>,
    stop: bool,
}

#[derive(Default)]
struct Shared {
    queue: Mutex<Queue>,
    available: Condvar,
}

pub struct Pool {
    shared: Arc<Shared>,
    helpers: Vec<JoinHandle<()>>,
}

impl Pool {
    /// A pool of `helpers` threads (a world of `w` workers uses `w - 1`).
    pub fn new(helpers: usize) -> Arc<Pool> {
        let shared = Arc::new(Shared::default());
        let helpers = (0..helpers)
            .map(|index| {
                let shared = shared.clone();
                thread::Builder::new().name(format!("pool-helper-{index}")).spawn(move || helper_loop(&shared)).expect("spawn a pool helper")
            })
            .collect();
        Arc::new(Pool { shared, helpers })
    }

    /// Offers `count` tickets of `job`, for workers 1 to `count`.
    ///
    /// # Safety
    ///
    /// The caller keeps `job` alive and at the same address until it has
    /// called [`Pool::retire`] for it.
    pub unsafe fn offer(&self, job: &dyn Job, count: usize) {
        let count = count.min(self.helpers.len());
        if count == 0 {
            return;
        }
        // SAFETY: the lifetime is erased; the caller's contract keeps the job alive.
        let job: *const dyn Job = unsafe { std::mem::transmute::<&dyn Job, &'static dyn Job>(job) };
        let mut queue = self.shared.queue.lock().unwrap();
        for worker in 1..=count {
            queue.tickets.push_back(Ticket { job, worker });
        }
        drop(queue);
        self.shared.available.notify_all();
    }

    /// Withdraws the tickets of `job` nobody took and waits until every
    /// helper that joined it has left.
    pub fn retire(&self, job: &dyn Job) {
        if self.helpers.is_empty() {
            return;
        }
        let address = std::ptr::from_ref(job).cast::<()>();
        self.shared.queue.lock().unwrap().tickets.retain(|ticket| ticket.job as *const () != address);
        let mut spins = 0_u32;
        while job.helpers_active() > 0 {
            spins += 1;
            if spins % 64 == 0 {
                thread::yield_now();
            } else {
                std::hint::spin_loop();
            }
        }
    }
}

impl Drop for Pool {
    fn drop(&mut self) {
        self.shared.queue.lock().unwrap().stop = true;
        self.shared.available.notify_all();
        for helper in self.helpers.drain(..) {
            let _ = helper.join();
        }
    }
}

fn helper_loop(shared: &Shared) {
    loop {
        let ticket = {
            let mut queue = shared.queue.lock().unwrap();
            loop {
                if queue.stop {
                    return;
                }
                if let Some(ticket) = queue.tickets.pop_front() {
                    // SAFETY: the ticket was in the queue under the lock, so
                    // its job's owner has not retired it yet.
                    unsafe { (*ticket.job).helper_joined() };
                    break ticket;
                }
                queue = shared.available.wait(queue).unwrap();
            }
        };
        // SAFETY: the job's owner waits in `retire` until this helper left.
        unsafe { (*ticket.job).help(ticket.worker) };
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::time::{Duration, Instant};

    #[derive(Default)]
    struct Counting {
        active: AtomicUsize,
        helped: AtomicUsize,
        release: AtomicBool,
    }

    impl Job for Counting {
        fn helper_joined(&self) {
            self.active.fetch_add(1, Ordering::SeqCst);
        }

        fn help(&self, _worker: usize) {
            self.helped.fetch_add(1, Ordering::SeqCst);
            while !self.release.load(Ordering::SeqCst) {
                thread::yield_now();
            }
            self.active.fetch_sub(1, Ordering::SeqCst);
        }

        fn helpers_active(&self) -> usize {
            self.active.load(Ordering::SeqCst)
        }
    }

    fn wait_for(condition: impl Fn() -> bool) {
        let started = Instant::now();
        while !condition() {
            assert!(started.elapsed() < Duration::from_secs(10), "the pool did not react within ten seconds");
            thread::yield_now();
        }
    }

    #[test]
    fn helpers_serve_a_job_and_untaken_tickets_are_withdrawn() {
        let pool = Pool::new(2);
        let first = Counting::default();
        let second = Counting::default();
        // SAFETY: both jobs outlive their retirement below.
        unsafe { pool.offer(&first, 2) };
        wait_for(|| first.helped.load(Ordering::SeqCst) == 2);
        // Every helper serves the first job: the second job's tickets wait.
        unsafe { pool.offer(&second, 2) };
        second.release.store(true, Ordering::SeqCst);
        pool.retire(&second);
        assert_eq!(second.helped.load(Ordering::SeqCst), 0, "no helper was free for the second job");
        first.release.store(true, Ordering::SeqCst);
        pool.retire(&first);
        assert_eq!(first.helpers_active(), 0);
        // The helpers are free again and the withdrawn tickets are gone.
        let third = Counting::default();
        third.release.store(true, Ordering::SeqCst);
        unsafe { pool.offer(&third, 5) };
        wait_for(|| third.helped.load(Ordering::SeqCst) == 2);
        pool.retire(&third);
        assert_eq!(third.helped.load(Ordering::SeqCst), 2, "a job gets at most the pool's helpers");
    }

    #[test]
    fn a_pool_without_helpers_offers_nothing_and_drops_cleanly() {
        let pool = Pool::new(0);
        let job = Counting::default();
        unsafe { pool.offer(&job, 3) };
        pool.retire(&job);
        assert_eq!(job.helped.load(Ordering::SeqCst), 0);
        drop(pool);
        drop(Pool::new(3));
    }
}
