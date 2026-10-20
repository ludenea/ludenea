//! Data parallelism with deterministic results: work is split into fixed-size chunks and
//! results come back in item order, so the thread count never changes the outcome.

/// A runtime resource that runs closures over slices. Native builds use a private rayon
/// pool when `threads > 1`; WebAssembly builds always run sequentially. Closures must
/// capture only extracted data, never `World`, a `Query`, or a borrow guard.
pub struct Parallel {
    threads: usize,
    #[cfg(not(target_family = "wasm"))]
    pool: Option<rayon::ThreadPool>,
}

/// Why a pool of threads cannot start.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum ParallelError {
    /// The count is 0 or above [`Parallel::MAX_THREADS`].
    #[error(
        "cannot start {requested} threads: outside the supported range; use 1 to {max}",
        max = Parallel::MAX_THREADS
    )]
    OutOfRange { requested: usize },
    /// The system refused to start the threads.
    #[error("cannot start {requested} threads: {reason}; use fewer threads")]
    Refused { requested: usize, reason: String },
}

impl Parallel {
    /// The most threads a pool runs. Results never depend on the count, so the cap only
    /// stops a typo from spending seconds and gigabytes starting threads.
    pub const MAX_THREADS: usize = 256;

    /// A pool of `threads`, from 1 to [`Self::MAX_THREADS`]. One thread runs every closure
    /// on the calling thread, and so does WebAssembly whatever the count.
    pub fn try_new(threads: usize) -> Result<Self, ParallelError> {
        Self::check(threads)?;
        Ok(Self {
            threads,
            #[cfg(not(target_family = "wasm"))]
            pool: if threads > 1 {
                Some(start(threads, rayon::ThreadPoolBuilder::new())?)
            } else {
                None
            },
        })
    }

    /// Refuses, before anything starts, a count [`Self::try_new`] refuses: 0 or above
    /// [`Self::MAX_THREADS`]. The system may still refuse a count this accepts.
    pub fn check(threads: usize) -> Result<(), ParallelError> {
        if (1..=Self::MAX_THREADS).contains(&threads) {
            Ok(())
        } else {
            Err(ParallelError::OutOfRange { requested: threads })
        }
    }

    /// One thread, which needs no pool and so cannot fail.
    pub(crate) fn single() -> Self {
        Self {
            threads: 1,
            #[cfg(not(target_family = "wasm"))]
            pool: None,
        }
    }

    pub fn threads(&self) -> usize {
        self.threads
    }

    /// Runs `f` inside the pool, so the `map` and `for_each_mut` calls it makes skip the
    /// cost of entering the pool each time. Without a pool, runs `f` on the calling thread.
    pub fn install<R: Send>(&self, f: impl FnOnce() -> R + Send) -> R {
        #[cfg(not(target_family = "wasm"))]
        if let Some(pool) = &self.pool {
            return pool.install(f);
        }
        f()
    }

    /// Applies `f` to every item, `chunk` items per task, and returns the results in item
    /// order. Work that fits in one chunk runs on the calling thread.
    ///
    /// # Panics
    /// If `chunk` is 0.
    pub fn map<T: Sync, R: Send>(
        &self,
        items: &[T],
        chunk: usize,
        f: impl Fn(&T) -> R + Sync + Send,
    ) -> Vec<R> {
        #[cfg(not(target_family = "wasm"))]
        if let Some(pool) = self.pool_for(items.len(), chunk) {
            use rayon::prelude::*;
            return pool.install(|| {
                items
                    .par_chunks(chunk)
                    .flat_map_iter(|chunk| chunk.iter().map(&f))
                    .collect()
            });
        }
        items
            .chunks(chunk)
            .flat_map(|chunk| chunk.iter().map(&f))
            .collect()
    }

    /// Applies `f` to every item in place, `chunk` items per task. Work that fits in one
    /// chunk runs on the calling thread.
    ///
    /// # Panics
    /// If `chunk` is 0.
    pub fn for_each_mut<T: Send>(
        &self,
        items: &mut [T],
        chunk: usize,
        f: impl Fn(&mut T) + Sync + Send,
    ) {
        #[cfg(not(target_family = "wasm"))]
        if let Some(pool) = self.pool_for(items.len(), chunk) {
            use rayon::prelude::*;
            return pool.install(|| {
                items
                    .par_chunks_mut(chunk)
                    .for_each(|chunk| chunk.iter_mut().for_each(&f));
            });
        }
        items
            .chunks_mut(chunk)
            .for_each(|chunk| chunk.iter_mut().for_each(&f));
    }

    /// The pool, unless the work fits in one chunk: entering the pool costs more than a
    /// single chunk saves.
    #[cfg(not(target_family = "wasm"))]
    fn pool_for(&self, len: usize, chunk: usize) -> Option<&rayon::ThreadPool> {
        self.pool.as_ref().filter(|_| len > chunk)
    }
}

/// Starts the pool `builder` describes with `threads` threads.
#[cfg(not(target_family = "wasm"))]
fn start(
    threads: usize,
    builder: rayon::ThreadPoolBuilder,
) -> Result<rayon::ThreadPool, ParallelError> {
    builder
        .num_threads(threads)
        .build()
        .map_err(|error| ParallelError::Refused {
            requested: threads,
            reason: error.to_string(),
        })
}

#[cfg(test)]
mod tests {
    use super::{Parallel, ParallelError};

    #[test]
    fn parallel_map_preserves_order_for_any_thread_count() {
        let items: Vec<u64> = (0..1000).collect();
        let results: Vec<Vec<u64>> = [1, 2, 8]
            .into_iter()
            .map(|threads| {
                Parallel::try_new(threads)
                    .unwrap()
                    .map(&items, 7, |item| item * 3 + 1)
            })
            .collect();
        assert_eq!(
            results[0],
            items.iter().map(|item| item * 3 + 1).collect::<Vec<_>>()
        );
        assert_eq!(results[0], results[1]);
        assert_eq!(results[0], results[2]);
    }

    #[test]
    fn for_each_mut_visits_every_item_for_any_thread_count() {
        for threads in [1, 4] {
            let mut items: Vec<u32> = (0..100).collect();
            Parallel::try_new(threads)
                .unwrap()
                .for_each_mut(&mut items, 8, |item| *item *= 2);
            assert_eq!(items, (0..100).map(|item| item * 2).collect::<Vec<_>>());
        }
    }

    #[test]
    fn a_count_outside_1_to_max_threads_is_refused_before_anything_starts() {
        for threads in [0, Parallel::MAX_THREADS + 1, usize::MAX] {
            let error = Parallel::try_new(threads).err().unwrap();
            assert_eq!(error, ParallelError::OutOfRange { requested: threads });
            assert_eq!(
                error.to_string(),
                format!(
                    "cannot start {threads} threads: outside the supported range; use 1 to 256"
                )
            );
        }
        assert_eq!(Parallel::try_new(1).unwrap().threads(), 1);
        // Checked, not started: 256 threads fail under a low process limit.
        assert_eq!(Parallel::check(Parallel::MAX_THREADS), Ok(()));
    }

    #[cfg(not(target_family = "wasm"))]
    #[test]
    fn threads_the_system_refuses_are_an_error() {
        // No system reserves a petabyte of stack for each of two threads.
        let builder = rayon::ThreadPoolBuilder::new().stack_size(1 << 50);
        let Err(ParallelError::Refused { requested, reason }) = super::start(2, builder) else {
            panic!("the system started threads with a petabyte of stack each");
        };
        assert_eq!(requested, 2);
        let message = ParallelError::Refused { requested, reason }.to_string();
        assert!(message.starts_with("cannot start 2 threads: "), "{message}");
        assert!(message.ends_with("; use fewer threads"), "{message}");
    }
}
