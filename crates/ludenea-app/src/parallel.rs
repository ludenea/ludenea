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

impl Parallel {
    /// `threads` below 1 counts as 1.
    ///
    /// # Panics
    /// If the native thread pool cannot be created.
    pub fn new(threads: usize) -> Self {
        let threads = threads.max(1);
        Self {
            threads,
            #[cfg(not(target_family = "wasm"))]
            pool: (threads > 1).then(|| {
                rayon::ThreadPoolBuilder::new()
                    .num_threads(threads)
                    .build()
                    .unwrap_or_else(|error| panic!("cannot start {threads} threads: {error}"))
            }),
        }
    }

    pub fn threads(&self) -> usize {
        self.threads
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

#[cfg(test)]
mod tests {
    use super::Parallel;

    #[test]
    fn parallel_map_preserves_order_for_any_thread_count() {
        let items: Vec<u64> = (0..1000).collect();
        let results: Vec<Vec<u64>> = [1, 2, 8]
            .into_iter()
            .map(|threads| Parallel::new(threads).map(&items, 7, |item| item * 3 + 1))
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
            Parallel::new(threads).for_each_mut(&mut items, 8, |item| *item *= 2);
            assert_eq!(items, (0..100).map(|item| item * 2).collect::<Vec<_>>());
        }
    }
}
