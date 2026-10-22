//! The input queue of one player: the confirmed inputs, the prediction that stands in for the
//! missing ones, and the input each simulated tick last ran with.
//!
//! Ticks come from untrusted peers, so every position is relative to a base tick and is only
//! computed after checking `tick >= base`; no tick arithmetic can overflow or index absolutely.
//! Ticks below the base have been released and cannot be compared any more; each method says what
//! it does for them.

use std::collections::VecDeque;

use thiserror::Error;

/// What [`InputQueue::confirm`] did with an input.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Confirmed {
    /// The tick had no confirmed input yet; it now has.
    New,
    /// The tick already had this exact input (a duplicate, or a tick already released).
    Same,
}

/// Why [`InputQueue::confirm`] refused an input.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub(crate) enum QueueError {
    /// A different input was already confirmed for this tick.
    #[error("tick {tick} is already confirmed with a different input")]
    Conflict { tick: u64 },
    /// The tick is after the next expected one; confirmed inputs arrive in order.
    #[error("confirmed inputs must arrive in order, the next expected tick is {next}")]
    Gap { next: u64 },
}

/// The confirmed inputs of one player from a base tick on, and what each tick last ran with.
#[derive(Debug, Clone)]
pub(crate) struct InputQueue {
    default_input: Vec<u8>,
    /// First tick still held; `confirmed[i]` and `used[i]` belong to tick `base + i`.
    base: u64,
    confirmed: VecDeque<Vec<u8>>,
    /// The last confirmed input ever, kept after release: the prediction.
    last_confirmed: Option<Vec<u8>>,
    /// `None` for a tick not simulated yet. Never longer than the ticks simulated in order.
    used: VecDeque<Option<Vec<u8>>>,
}

impl InputQueue {
    pub(crate) fn new(default_input: Vec<u8>) -> Self {
        Self {
            default_input,
            base: 0,
            confirmed: VecDeque::new(),
            last_confirmed: None,
            used: VecDeque::new(),
        }
    }

    /// Position of `tick` relative to the base, or `None` if it was released.
    fn position(&self, tick: u64) -> Option<usize> {
        usize::try_from(tick.checked_sub(self.base)?).ok()
    }

    /// Confirms the input of `tick`, which must be the next one or an already confirmed one.
    /// A tick below the base was released and cannot be compared: it answers `Same`, a stale
    /// duplicate that changes nothing.
    pub(crate) fn confirm(&mut self, tick: u64, input: Vec<u8>) -> Result<Confirmed, QueueError> {
        let Some(position) = self.position(tick) else {
            return Ok(Confirmed::Same);
        };
        match self.confirmed.get(position) {
            Some(held) if *held == input => Ok(Confirmed::Same),
            Some(_) => Err(QueueError::Conflict { tick }),
            None if position == self.confirmed.len() => {
                self.last_confirmed = Some(input.clone());
                self.confirmed.push_back(input);
                Ok(Confirmed::New)
            }
            None => Err(QueueError::Gap {
                next: self.next_confirmed(),
            }),
        }
    }

    /// The first tick without a confirmed input. Saturates at `u64::MAX` once that last tick is
    /// confirmed, since no later tick exists.
    pub(crate) fn next_confirmed(&self) -> u64 {
        let held = u64::try_from(self.confirmed.len()).unwrap_or(u64::MAX);
        self.base.saturating_add(held)
    }

    /// The confirmed input of `tick`, or the prediction: the last confirmed input, or the default
    /// before any. A released tick answers the prediction too, its input being gone.
    pub(crate) fn input_for(&self, tick: u64) -> Vec<u8> {
        self.position(tick)
            .and_then(|position| self.confirmed.get(position))
            .or(self.last_confirmed.as_ref())
            .unwrap_or(&self.default_input)
            .clone()
    }

    /// Records the input `tick` was simulated with, replacing an earlier record. Ticks are
    /// simulated in order, so a tick below the base (released) or past the next unrecorded one
    /// is ignored: it cannot belong to a rollback and must not grow the queue.
    pub(crate) fn record_used(&mut self, tick: u64, input: &[u8]) {
        let Some(position) = self.position(tick) else {
            return;
        };
        if position < self.used.len() {
            self.used[position] = Some(input.to_vec());
        } else if position == self.used.len() {
            self.used.push_back(Some(input.to_vec()));
        }
    }

    /// The first tick below `below` whose used input differs from its confirmed one. A tick not
    /// simulated, or not confirmed, yet never counts.
    pub(crate) fn first_misprediction(&self, below: u64) -> Option<u64> {
        let limit = usize::try_from(below.saturating_sub(self.base)).unwrap_or(usize::MAX);
        self.confirmed
            .iter()
            .zip(&self.used)
            .take(limit)
            .position(|(confirmed, used)| used.as_ref().is_some_and(|used| used != confirmed))
            // The position indexes a held tick, so `base + position` is a real tick.
            .and_then(|position| self.base.checked_add(u64::try_from(position).ok()?))
    }

    /// Drops what no rollback can need any more: every tick below `tick`, but never a tick that
    /// is not confirmed yet, so the base cannot pass the next expected tick.
    pub(crate) fn release_before(&mut self, tick: u64) {
        let count = self
            .position(tick)
            .map_or(0, |position| position.min(self.confirmed.len()));
        self.confirmed.drain(..count);
        self.used.drain(..count.min(self.used.len()));
        // `count` ticks were held, so the base plus `count` is a tick that exists.
        self.base = self
            .base
            .saturating_add(u64::try_from(count).unwrap_or(u64::MAX));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn queue() -> InputQueue {
        InputQueue::new(vec![0])
    }

    #[test]
    fn confirms_in_order_and_repeats_are_same() {
        let mut q = queue();
        assert_eq!(q.confirm(0, vec![1]), Ok(Confirmed::New));
        assert_eq!(q.confirm(1, vec![2]), Ok(Confirmed::New));
        assert_eq!(q.confirm(0, vec![1]), Ok(Confirmed::Same));
        assert_eq!(q.next_confirmed(), 2);
    }

    #[test]
    fn refuses_a_conflict_and_a_gap_without_changing_anything() {
        let mut q = queue();
        q.confirm(0, vec![1]).unwrap();
        assert_eq!(q.confirm(0, vec![9]), Err(QueueError::Conflict { tick: 0 }));
        assert_eq!(q.confirm(2, vec![9]), Err(QueueError::Gap { next: 1 }));
        assert_eq!(
            q.confirm(u64::MAX, vec![9]),
            Err(QueueError::Gap { next: 1 })
        );
        assert_eq!(q.next_confirmed(), 1);
        assert_eq!(q.input_for(0), vec![1]);
    }

    #[test]
    fn predicts_the_last_confirmed_input_or_the_default() {
        let mut q = queue();
        assert_eq!(q.input_for(5), vec![0]);
        q.confirm(0, vec![1]).unwrap();
        q.confirm(1, vec![2]).unwrap();
        assert_eq!(q.input_for(0), vec![1]);
        assert_eq!(q.input_for(1), vec![2]);
        assert_eq!(q.input_for(7), vec![2]);
        assert_eq!(q.input_for(u64::MAX), vec![2]);
    }

    #[test]
    fn a_misprediction_is_a_used_input_that_differs_from_the_confirmed_one() {
        let mut q = queue();
        q.record_used(0, &[0]);
        q.record_used(1, &[0]);
        q.confirm(0, vec![0]).unwrap();
        assert_eq!(q.first_misprediction(2), None);
        q.confirm(1, vec![5]).unwrap();
        assert_eq!(q.first_misprediction(2), Some(1));
        assert_eq!(q.first_misprediction(1), None);
    }

    #[test]
    fn a_tick_resimulated_after_a_correction_compares_with_its_new_input() {
        let mut q = queue();
        q.record_used(0, &[0]);
        q.confirm(0, vec![5]).unwrap();
        assert_eq!(q.first_misprediction(1), Some(0));
        q.record_used(0, &[5]);
        assert_eq!(q.first_misprediction(1), None);
    }

    #[test]
    fn an_input_confirmed_for_an_unsimulated_tick_is_not_a_misprediction() {
        let mut q = queue();
        q.record_used(0, &[1]);
        q.confirm(0, vec![1]).unwrap();
        q.confirm(1, vec![7]).unwrap();
        assert_eq!(q.first_misprediction(u64::MAX), None);
        assert_eq!(q.first_misprediction(1), None);
    }

    #[test]
    fn release_keeps_the_remaining_ticks_and_the_prediction() {
        let mut q = queue();
        for tick in 0..4 {
            q.record_used(tick, &[0]);
            q.confirm(tick, vec![u8::try_from(tick).unwrap()]).unwrap();
        }
        q.release_before(2);
        assert_eq!(q.next_confirmed(), 4);
        assert_eq!(q.input_for(2), vec![2]);
        assert_eq!(q.first_misprediction(4), Some(2));
        q.release_before(4);
        assert_eq!(q.input_for(9), vec![3]);
        assert_eq!(q.confirm(4, vec![4]), Ok(Confirmed::New));
    }

    #[test]
    fn released_ticks_never_panic_or_change_the_queue() {
        let mut q = queue();
        for tick in 0..3 {
            q.confirm(tick, vec![1]).unwrap();
        }
        q.release_before(3);
        assert_eq!(q.confirm(1, vec![9]), Ok(Confirmed::Same));
        assert_eq!(q.input_for(1), vec![1]);
        q.record_used(1, &[9]);
        assert_eq!(q.first_misprediction(u64::MAX), None);
        assert_eq!(q.next_confirmed(), 3);
    }

    #[test]
    fn release_never_passes_the_next_expected_tick() {
        let mut q = queue();
        q.confirm(0, vec![1]).unwrap();
        q.release_before(u64::MAX);
        assert_eq!(q.next_confirmed(), 1);
        assert_eq!(q.confirm(1, vec![2]), Ok(Confirmed::New));
    }

    #[test]
    fn a_record_past_the_next_unrecorded_tick_is_ignored() {
        let mut q = queue();
        q.record_used(u64::MAX, &[1]);
        q.record_used(5, &[1]);
        q.confirm(0, vec![2]).unwrap();
        assert_eq!(q.first_misprediction(u64::MAX), None);
    }

    /// A queue whose base sits next to `u64::MAX`, reached by releasing, never by allocating.
    fn queue_at(base: u64) -> InputQueue {
        let mut q = queue();
        q.base = base;
        q
    }

    #[test]
    fn ticks_near_the_maximum_do_not_overflow() {
        let mut q = queue_at(u64::MAX - 2);
        for tick in u64::MAX - 2..=u64::MAX {
            q.record_used(tick, &[0]);
            assert_eq!(q.confirm(tick, vec![1]), Ok(Confirmed::New));
        }
        assert_eq!(q.next_confirmed(), u64::MAX);
        assert_eq!(q.confirm(u64::MAX, vec![1]), Ok(Confirmed::Same));
        assert_eq!(
            q.confirm(u64::MAX, vec![2]),
            Err(QueueError::Conflict { tick: u64::MAX })
        );
        assert_eq!(q.first_misprediction(u64::MAX), Some(u64::MAX - 2));
        assert_eq!(q.input_for(u64::MAX), vec![1]);
        q.release_before(u64::MAX);
        assert_eq!(q.first_misprediction(u64::MAX), None);
        assert_eq!(q.first_misprediction(0), None);
        q.release_before(u64::MAX);
        q.release_before(0);
        assert_eq!(q.input_for(0), vec![1]);
    }
}
