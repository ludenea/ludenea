//! The input queue of one player: the confirmed inputs, the prediction that stands in for the
//! missing ones, and the input each simulated tick last ran with.
//!
//! Ticks come from untrusted peers, so every position is relative to a base tick. A tick is
//! compared with the base in `u64` first; only an offset already bounded by the held length is
//! converted to `usize`, so a far tick is never mistaken for a released one on a 32-bit target.
//! Ticks below the base have been released and cannot be compared any more; each method says what
//! it does for them.

use std::collections::VecDeque;

use thiserror::Error;

/// What [`InputQueue::confirm`] did with an input.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Confirmed {
    /// The tick had no confirmed input yet; it now has.
    New,
    /// The tick holds this exact input already, verified equal (a duplicate).
    Same,
    /// The tick was released: its input is gone, so nothing was compared and nothing changed. An
    /// honest peer's reordered or repeated resend lands here; it is not a violation.
    Released,
}

/// Why a call on an [`InputQueue`] was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub(crate) enum QueueError {
    /// A different input was already confirmed for this tick.
    #[error("tick {tick} is already confirmed with a different input")]
    Conflict { tick: u64 },
    /// The tick is after the next expected one; confirmed inputs arrive in order.
    #[error("confirmed inputs must arrive in order, the next expected tick is {next}")]
    Gap { next: u64 },
    /// A simulated tick was recorded out of order or after its release. These are local ticks, so
    /// this is a session bug, and ignoring it would hide a misprediction. `next` is `None`
    /// when tick `u64::MAX` is recorded and no tick can follow.
    #[error("tick {tick} was recorded as simulated, expected {}", describe_next(*next))]
    NotNextSimulated { tick: u64, next: Option<u64> },
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
    /// The input each simulated tick last ran with, recorded in order from the base.
    used: VecDeque<Vec<u8>>,
}

/// Words for the next expected tick of a [`QueueError::NotNextSimulated`].
fn describe_next(next: Option<u64>) -> String {
    next.map_or_else(
        || "no tick, the last one is recorded".to_owned(),
        |tick| format!("tick {tick}"),
    )
}

/// Number of held entries as a `u64`; `usize` never exceeds it on a supported target.
fn as_offset(len: usize) -> u64 {
    u64::try_from(len).unwrap_or(u64::MAX)
}

impl InputQueue {
    /// An empty queue starting at tick 0, so the first simulated tick must be tick 0.
    pub(crate) fn new(default_input: Vec<u8>) -> Self {
        Self {
            default_input,
            base: 0,
            confirmed: VecDeque::new(),
            last_confirmed: None,
            used: VecDeque::new(),
        }
    }

    /// Index of `tick` in a queue holding `len` entries, `None` if it lies before the base or
    /// past the end. The comparison is done in `u64`; the conversion only sees an offset below
    /// `len`, which fits in `usize`.
    fn index(&self, tick: u64, len: usize) -> Option<usize> {
        let offset = tick.checked_sub(self.base)?;
        if offset >= as_offset(len) {
            return None;
        }
        usize::try_from(offset).ok()
    }

    /// Confirms the input of `tick`, which must be the next one or an already confirmed one. A
    /// released tick answers [`Confirmed::Released`].
    pub(crate) fn confirm(&mut self, tick: u64, input: Vec<u8>) -> Result<Confirmed, QueueError> {
        let Some(offset) = tick.checked_sub(self.base) else {
            return Ok(Confirmed::Released);
        };
        if let Some(held) = self
            .index(tick, self.confirmed.len())
            .and_then(|index| self.confirmed.get(index))
        {
            return if *held == input {
                Ok(Confirmed::Same)
            } else {
                Err(QueueError::Conflict { tick })
            };
        }
        let held = as_offset(self.confirmed.len());
        if offset > held {
            // The tick is past `base + held`, so this sum cannot overflow.
            return Err(QueueError::Gap {
                next: self.base.saturating_add(held),
            });
        }
        self.last_confirmed = Some(input.clone());
        self.confirmed.push_back(input);
        Ok(Confirmed::New)
    }

    /// The first tick without a confirmed input, or `None` once tick `u64::MAX` is confirmed:
    /// no tick follows it, so a caller acknowledging "everything below the next tick" must read
    /// `None` as "everything".
    pub(crate) fn next_confirmed(&self) -> Option<u64> {
        self.base.checked_add(as_offset(self.confirmed.len()))
    }

    /// The confirmed input of `tick`, or the prediction: the last confirmed input, or the default
    /// before any. `None` for a released tick, whose input is gone and which has no prediction.
    pub(crate) fn input_for(&self, tick: u64) -> Option<Vec<u8>> {
        if tick < self.base {
            return None;
        }
        Some(
            self.index(tick, self.confirmed.len())
                .and_then(|index| self.confirmed.get(index))
                .or(self.last_confirmed.as_ref())
                .unwrap_or(&self.default_input)
                .clone(),
        )
    }

    /// Records the input `tick` was simulated with, replacing an earlier record. Ticks are
    /// simulated in order from the base, so only a recorded tick or the next one is accepted.
    pub(crate) fn record_used(&mut self, tick: u64, input: &[u8]) -> Result<(), QueueError> {
        if let Some(index) = self.index(tick, self.used.len()) {
            self.used[index] = input.to_vec();
            return Ok(());
        }
        let recorded = as_offset(self.used.len());
        if tick.checked_sub(self.base) == Some(recorded) {
            self.used.push_back(input.to_vec());
            return Ok(());
        }
        Err(QueueError::NotNextSimulated {
            tick,
            next: self.base.checked_add(recorded),
        })
    }

    /// The first tick below `below` whose used input differs from its confirmed one. A tick not
    /// simulated, or not confirmed, yet never counts.
    pub(crate) fn first_misprediction(&self, below: u64) -> Option<u64> {
        let limit = usize::try_from(below.saturating_sub(self.base)).unwrap_or(usize::MAX);
        let position = self
            .confirmed
            .iter()
            .zip(&self.used)
            .take(limit)
            .position(|(confirmed, used)| used != confirmed)?;
        // The position indexes a held tick, so `base + position` is a real tick.
        self.base.checked_add(as_offset(position))
    }

    /// Drops what no rollback can need any more: every tick below `tick`, but never a tick that
    /// is not both confirmed and simulated yet. Call [`Self::first_misprediction`] first: a
    /// released tick can no longer be found wrong.
    pub(crate) fn release_before(&mut self, tick: u64) {
        let held = as_offset(self.confirmed.len().min(self.used.len()));
        let count = tick.saturating_sub(self.base).min(held);
        // `count` is at most the held length, so it fits in `usize`.
        let count = usize::try_from(count).unwrap_or(0);
        self.confirmed.drain(..count);
        self.used.drain(..count);
        self.base = self.base.saturating_add(as_offset(count));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn queue() -> InputQueue {
        InputQueue::new(vec![0])
    }

    fn byte(tick: u64) -> Vec<u8> {
        vec![u8::try_from(tick % 251).unwrap()]
    }

    /// A queue confirmed and simulated for `0..confirmed` and `0..simulated`, each tick `t`
    /// confirmed with `byte(t)` and simulated with `byte(t)` too.
    fn queue_with(confirmed: u64, simulated: u64) -> InputQueue {
        let mut q = queue();
        for tick in 0..confirmed {
            q.confirm(tick, byte(tick)).unwrap();
        }
        for tick in 0..simulated {
            q.record_used(tick, &byte(tick)).unwrap();
        }
        q
    }

    /// A queue whose base sits at `base`, set directly: reaching it by releasing would need
    /// `base` confirmed ticks.
    fn queue_at(base: u64) -> InputQueue {
        let mut q = queue();
        q.base = base;
        q
    }

    #[test]
    fn confirms_in_order_and_repeats_are_same() {
        let mut q = queue();
        assert_eq!(q.confirm(0, vec![1]), Ok(Confirmed::New));
        assert_eq!(q.confirm(1, vec![2]), Ok(Confirmed::New));
        assert_eq!(q.confirm(0, vec![1]), Ok(Confirmed::Same));
        assert_eq!(q.next_confirmed(), Some(2));
    }

    #[test]
    fn refuses_a_conflict_and_a_gap_without_changing_anything() {
        let mut q = queue();
        q.confirm(0, vec![1]).unwrap();
        assert_eq!(q.confirm(0, vec![9]), Err(QueueError::Conflict { tick: 0 }));
        for tick in [2, 1 << 32, (1 << 32) + 1, u64::MAX - 1, u64::MAX] {
            assert_eq!(q.confirm(tick, vec![9]), Err(QueueError::Gap { next: 1 }));
        }
        assert_eq!(q.next_confirmed(), Some(1));
        assert_eq!(q.input_for(0), Some(vec![1]));
    }

    #[test]
    fn predicts_the_last_confirmed_input_or_the_default() {
        let mut q = queue();
        assert_eq!(q.input_for(5), Some(vec![0]));
        q.confirm(0, vec![1]).unwrap();
        q.confirm(1, vec![2]).unwrap();
        assert_eq!(q.input_for(0), Some(vec![1]));
        assert_eq!(q.input_for(1), Some(vec![2]));
        for tick in [7, 1 << 32, u64::MAX] {
            assert_eq!(q.input_for(tick), Some(vec![2]));
        }
    }

    #[test]
    fn a_misprediction_is_a_used_input_that_differs_from_the_confirmed_one() {
        let mut q = queue();
        q.record_used(0, &[0]).unwrap();
        q.record_used(1, &[0]).unwrap();
        q.confirm(0, vec![0]).unwrap();
        assert_eq!(q.first_misprediction(2), None);
        q.confirm(1, vec![5]).unwrap();
        assert_eq!(q.first_misprediction(2), Some(1));
        assert_eq!(q.first_misprediction(1), None);
    }

    #[test]
    fn a_tick_resimulated_after_a_correction_compares_with_its_new_input() {
        let mut q = queue();
        q.record_used(0, &[0]).unwrap();
        q.confirm(0, vec![5]).unwrap();
        assert_eq!(q.first_misprediction(1), Some(0));
        q.record_used(0, &[5]).unwrap();
        assert_eq!(q.first_misprediction(1), None);
    }

    #[test]
    fn an_input_confirmed_for_an_unsimulated_tick_is_not_a_misprediction() {
        let mut q = queue();
        q.record_used(0, &[1]).unwrap();
        q.confirm(0, vec![1]).unwrap();
        q.confirm(1, vec![7]).unwrap();
        assert_eq!(q.first_misprediction(u64::MAX), None);
        assert_eq!(q.first_misprediction(1), None);
    }

    #[test]
    fn recording_must_start_at_the_base_and_follow_in_order() {
        let mut q = queue();
        assert_eq!(
            q.record_used(1, &[0]),
            Err(QueueError::NotNextSimulated {
                tick: 1,
                next: Some(0)
            })
        );
        q.record_used(0, &[0]).unwrap();
        for tick in [2, 1 << 32, u64::MAX] {
            assert_eq!(
                q.record_used(tick, &[0]),
                Err(QueueError::NotNextSimulated {
                    tick,
                    next: Some(1)
                })
            );
        }
        assert_eq!(q.record_used(1, &[0]), Ok(()));
    }

    #[test]
    fn release_keeps_the_remaining_ticks_and_the_prediction() {
        let mut q = queue_with(4, 4);
        q.record_used(2, &[99]).unwrap();
        q.release_before(2);
        assert_eq!(q.next_confirmed(), Some(4));
        assert_eq!(q.input_for(2), Some(byte(2)));
        assert_eq!(q.first_misprediction(4), Some(2));
        q.release_before(4);
        assert_eq!(q.input_for(9), Some(byte(3)));
        assert_eq!(q.confirm(4, byte(4)), Ok(Confirmed::New));
        assert_eq!(q.record_used(4, &byte(4)), Ok(()));
    }

    #[test]
    fn a_released_tick_is_reported_not_guessed() {
        let mut q = queue_with(3, 3);
        q.release_before(3);
        assert_eq!(q.confirm(1, vec![9]), Ok(Confirmed::Released));
        assert_eq!(q.input_for(1), None);
        assert_eq!(
            q.record_used(1, &[9]),
            Err(QueueError::NotNextSimulated {
                tick: 1,
                next: Some(3)
            })
        );
        assert_eq!(q.first_misprediction(u64::MAX), None);
        assert_eq!(q.next_confirmed(), Some(3));
    }

    #[test]
    fn release_never_passes_the_next_expected_tick() {
        let mut q = queue_with(1, 1);
        q.release_before(u64::MAX);
        assert_eq!(q.next_confirmed(), Some(1));
        assert_eq!(q.confirm(1, vec![2]), Ok(Confirmed::New));
    }

    #[test]
    fn release_never_passes_a_confirmed_tick_that_was_not_simulated() {
        let mut q = queue_with(10, 4);
        q.release_before(8);
        assert_eq!(q.input_for(4), Some(byte(4)));
        assert_eq!(q.input_for(3), None);
        q.record_used(4, &[99]).unwrap();
        assert_eq!(q.first_misprediction(5), Some(4));
    }

    #[test]
    fn ticks_near_the_maximum_do_not_overflow() {
        let mut q = queue_at(u64::MAX - 2);
        for tick in u64::MAX - 2..=u64::MAX {
            q.record_used(tick, &[0]).unwrap();
            assert_eq!(q.confirm(tick, vec![1]), Ok(Confirmed::New));
        }
        assert_eq!(q.confirm(u64::MAX, vec![1]), Ok(Confirmed::Same));
        assert_eq!(
            q.confirm(u64::MAX, vec![2]),
            Err(QueueError::Conflict { tick: u64::MAX })
        );
        assert_eq!(q.first_misprediction(u64::MAX), Some(u64::MAX - 2));
        assert_eq!(q.input_for(u64::MAX), Some(vec![1]));
        q.record_used(u64::MAX, &[1]).unwrap();
        q.release_before(u64::MAX);
        assert_eq!(q.first_misprediction(u64::MAX), None);
        assert_eq!(q.first_misprediction(0), None);
        q.release_before(0);
        assert_eq!(q.input_for(0), None);
    }

    #[test]
    fn nothing_is_next_once_the_last_tick_is_confirmed() {
        let mut q = queue_at(u64::MAX);
        assert_eq!(q.next_confirmed(), Some(u64::MAX));
        q.confirm(u64::MAX, vec![1]).unwrap();
        assert_eq!(q.next_confirmed(), None);
        q.release_before(u64::MAX);
        assert_eq!(q.next_confirmed(), None);
    }

    #[test]
    fn nothing_is_expected_after_the_last_tick_is_recorded() {
        let mut q = queue_at(u64::MAX);
        q.record_used(u64::MAX, &[1]).unwrap();
        assert_eq!(
            q.record_used(0, &[1]),
            Err(QueueError::NotNextSimulated {
                tick: 0,
                next: None
            })
        );
        assert_eq!(q.record_used(u64::MAX, &[2]), Ok(()));
    }
}
