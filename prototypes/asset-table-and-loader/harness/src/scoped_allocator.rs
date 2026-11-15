//! A counting allocator scoped to threads: the allocator of the harness.
//! Throwaway, never product code.
//!
//! It forwards every call to the counting allocator of the first check, so
//! that the asset table spike's scenarios keep their counters, and adds one
//! measure: while a scope is open, for the threads that entered it, the
//! largest single allocation and the largest count of bytes held allocated
//! at one moment. A block counts as held from the moment a thread of the
//! scope allocates it to the moment any thread frees it: a block allocated
//! before the scope, or by a thread outside it, is never counted, whoever
//! frees it. A block that grows in place or moves is counted as its new size
//! allocated, then its former size freed, which is the most that may be held
//! at that moment.
//!
//! One scope is open at a time: a second one waits for the first to close.
//! A thread enters the scope that is open by calling `enter`; the loading
//! capability calls it, through the watch of a mount, on each of its threads
//! that reads, expands, verifies or validates an artifact.

use std::alloc::{GlobalAlloc, Layout};
use std::cell::Cell;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Mutex, MutexGuard};

use spike_check::Counting;

/// The allocator of the harness.
pub struct Scoped;

/// How many blocks one scope can hold track of at one moment, freed ones
/// included until the scope closes.
const SLOTS: usize = 1 << 14;
/// A slot that never held a block, and one whose block was freed.
const EMPTY: usize = 0;
const FREED: usize = 1;

/// The scope that is open, by its number; zero where none is.
static OPEN: AtomicU64 = AtomicU64::new(0);
static NEXT: AtomicU64 = AtomicU64::new(1);
static LARGEST: AtomicUsize = AtomicUsize::new(0);
static HELD: AtomicUsize = AtomicUsize::new(0);
static HELD_AT_MOST: AtomicUsize = AtomicUsize::new(0);
/// More blocks were allocated in a scope than it can hold track of: what it
/// measured is then no account of what was held.
static OVERFLOWED: AtomicBool = AtomicBool::new(false);
/// The blocks the threads of the scope allocated and that are not freed, by
/// address, with their size: an open-addressed table that no call resizes.
static BLOCKS: [AtomicUsize; SLOTS] = [const { AtomicUsize::new(EMPTY) }; SLOTS];
static SIZES: [AtomicUsize; SLOTS] = [const { AtomicUsize::new(0) }; SLOTS];
/// One scope at a time.
static ONE_AT_A_TIME: Mutex<()> = Mutex::new(());

thread_local! {
    /// The number of the scope this thread entered last; zero where none.
    static ENTERED: Cell<u64> = const { Cell::new(0) };
}

/// What a scope measured.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Measured {
    /// The largest single allocation a thread of the scope made, in bytes.
    pub largest: usize,
    /// The largest count of bytes that the blocks allocated by the threads
    /// of the scope held at one moment.
    pub held_at_most: usize,
    /// Whether every block was tracked: false where the scope allocated
    /// more blocks than it can hold track of.
    pub complete: bool,
}

/// An open scope. It closes when it is dropped.
pub struct Scope {
    _one_at_a_time: MutexGuard<'static, ()>,
}

/// Opens a scope: from now on the allocations of the threads that enter it
/// are measured, until it closes.
pub fn open() -> Scope {
    let one_at_a_time = ONE_AT_A_TIME.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    for (block, size) in BLOCKS.iter().zip(&SIZES) {
        block.store(EMPTY, Ordering::Relaxed);
        size.store(0, Ordering::Relaxed);
    }
    LARGEST.store(0, Ordering::Relaxed);
    HELD.store(0, Ordering::Relaxed);
    HELD_AT_MOST.store(0, Ordering::Relaxed);
    OVERFLOWED.store(false, Ordering::Relaxed);
    OPEN.store(NEXT.fetch_add(1, Ordering::SeqCst), Ordering::SeqCst);
    Scope { _one_at_a_time: one_at_a_time }
}

impl Scope {
    /// Closes the scope and gives what it measured.
    pub fn close(self) -> Measured {
        OPEN.store(0, Ordering::SeqCst);
        Measured { largest: LARGEST.load(Ordering::SeqCst), held_at_most: HELD_AT_MOST.load(Ordering::SeqCst), complete: !OVERFLOWED.load(Ordering::SeqCst) }
    }
}

impl Drop for Scope {
    fn drop(&mut self) {
        OPEN.store(0, Ordering::SeqCst);
    }
}

/// The calling thread enters the scope that is open, where one is: its
/// allocations are measured from now on, until that scope closes.
pub fn enter() {
    ENTERED.with(|entered| entered.set(OPEN.load(Ordering::SeqCst)));
}

/// Whether the calling thread is in the scope that is open.
fn in_the_scope() -> bool {
    let open = OPEN.load(Ordering::Relaxed);
    open != 0 && ENTERED.try_with(|entered| entered.get() == open).unwrap_or(false)
}

fn slot_of(address: usize) -> usize {
    ((address >> 3) as u64).wrapping_mul(0x9e37_79b9_7f4a_7c15) as usize >> (usize::BITS - SLOTS.trailing_zeros())
}

/// Holds track of a block a thread of the scope allocated.
fn track(address: usize, size: usize) {
    let mut at = slot_of(address);
    for _ in 0..SLOTS {
        let held_there = BLOCKS[at].load(Ordering::Relaxed);
        if (held_there == EMPTY || held_there == FREED) && BLOCKS[at].compare_exchange(held_there, address, Ordering::AcqRel, Ordering::Relaxed).is_ok() {
            SIZES[at].store(size, Ordering::Release);
            return;
        }
        at = (at + 1) % SLOTS;
    }
    OVERFLOWED.store(true, Ordering::Relaxed);
}

/// Stops holding track of the block at an address, where a thread of the
/// scope allocated it, and gives its size.
fn forget(address: usize) -> Option<usize> {
    let mut at = slot_of(address);
    for _ in 0..SLOTS {
        let held_there = BLOCKS[at].load(Ordering::Acquire);
        if held_there == EMPTY {
            return None;
        }
        if held_there == address && BLOCKS[at].compare_exchange(address, FREED, Ordering::AcqRel, Ordering::Relaxed).is_ok() {
            return Some(SIZES[at].load(Ordering::Acquire));
        }
        at = (at + 1) % SLOTS;
    }
    None
}

/// A thread of the scope allocated a block: it is held from now on.
fn allocated(address: usize, size: usize) {
    LARGEST.fetch_max(size, Ordering::Relaxed);
    let held = HELD.fetch_add(size, Ordering::Relaxed) + size;
    HELD_AT_MOST.fetch_max(held, Ordering::Relaxed);
    track(address, size);
}

// SAFETY: every call is forwarded unchanged to the counting allocator of
// the first check, which forwards it to the system allocator; what is added
// reads and writes atomics and one thread-local cell that has no destructor,
// and allocates nothing.
unsafe impl GlobalAlloc for Scoped {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let block = unsafe { Counting.alloc(layout) };
        if !block.is_null() && in_the_scope() {
            allocated(block as usize, layout.size());
        }
        block
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        let block = unsafe { Counting.alloc_zeroed(layout) };
        if !block.is_null() && in_the_scope() {
            allocated(block as usize, layout.size());
        }
        block
    }

    unsafe fn dealloc(&self, block: *mut u8, layout: Layout) {
        // A block is held until any thread frees it.
        if OPEN.load(Ordering::Relaxed) != 0
            && let Some(size) = forget(block as usize)
        {
            HELD.fetch_sub(size, Ordering::Relaxed);
        }
        unsafe { Counting.dealloc(block, layout) }
    }

    unsafe fn realloc(&self, block: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        // The former block is forgotten before the system allocator may
        // give its address to another thread.
        let former = if OPEN.load(Ordering::Relaxed) != 0 { forget(block as usize) } else { None };
        let moved = unsafe { Counting.realloc(block, layout, new_size) };
        if moved.is_null() {
            // The former block stands as it was.
            if let Some(size) = former {
                track(block as usize, size);
            }
            return moved;
        }
        // The new size allocated, then the former size freed: the most
        // that may be held at that moment.
        if in_the_scope() {
            allocated(moved as usize, new_size);
        }
        if let Some(size) = former {
            HELD.fetch_sub(size, Ordering::Relaxed);
        }
        moved
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_scope_measures_what_the_threads_that_entered_it_allocate_and_nothing_else() {
        // Allocated before the scope, and freed inside it: never counted.
        let before = vec![0_u8; 300_000];
        let scope = open();
        // Allocated inside the scope by a thread that did not enter it.
        let outside = vec![0_u8; 200_000];
        enter();
        drop(before);
        // Two blocks held together, the larger of 100 000 bytes.
        let first = vec![1_u8; 100_000];
        let second = vec![2_u8; 50_000];
        // A thread that does not enter the scope is not measured, and it
        // frees a block of the scope, which is then held no longer.
        std::thread::spawn(move || {
            let _unseen = vec![3_u8; 400_000];
            drop(second);
        })
        .join()
        .expect("the thread ends");
        // A third block: the first two were never held with it together
        // beyond the first.
        let third = vec![4_u8; 60_000];
        let measured = scope.close();
        drop((first, third, outside));
        assert!(measured.complete);
        assert_eq!(measured.largest, 100_000, "the largest single allocation of the threads that entered");
        assert!((160_000..200_000).contains(&measured.held_at_most), "the first with the third, and the little this test allocates beside them: {measured:?}");
        // After the scope closed, nothing is measured, and a thread that
        // entered a former scope is not in the next one.
        let next = open();
        let unseen = vec![5_u8; 700_000];
        assert_eq!(next.close(), Measured { largest: 0, held_at_most: 0, complete: true });
        drop(unseen);
    }

    #[test]
    fn a_block_that_grows_is_counted_as_its_new_size_before_its_former_one_is_freed() {
        let scope = open();
        enter();
        let mut grown: Vec<u8> = Vec::with_capacity(10_000);
        grown.extend(std::iter::repeat_n(7_u8, 10_000));
        grown.reserve_exact(30_000);
        let measured = scope.close();
        assert_eq!(grown.capacity(), 40_000);
        assert_eq!(measured.largest, 40_000);
        assert!((50_000..60_000).contains(&measured.held_at_most), "the new block and the former one, for a moment: {measured:?}");
    }
}
