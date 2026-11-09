//! The scripted editor client. Throwaway, never product code.
//!
//! It stands for an editor during a drag. It folds every change set into its
//! own copy of the watched component, reads its entities' handles and values
//! from the live-value stream, and, once per delivered tick, sends one `set`
//! command per field of every dragged entity (at most one command per field
//! per tick: it sends only when the delivered tick advanced). When the drag
//! ends it sends one transaction that carries the gesture's final values with
//! the inverse requested, waits for its result, then closes the session. The
//! latency of a `set` command is measured here, from the moment the command
//! is written to the moment the client holds the live-value message whose
//! tick boundary decided it.

use std::collections::VecDeque;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;

use crate::host::SESSION;
use crate::transport::ClientSide;
use crate::wire::{self, ChangeRow, Names, TransactionResult};
use crate::world::{Field, FieldSet, Outcome};

/// A transaction refused for a stale handle is sent again with the handles
/// the live values show, this many times at most.
const TRANSACTION_ATTEMPTS: u32 = 3;

#[derive(Clone)]
pub struct EditorScript {
    pub entity_count: usize,
    /// The entities the drag sets, by identifier index.
    pub dragged: Vec<u32>,
    /// The entities of the live-value subscription, four fields each; the
    /// dragged entities are among them.
    pub live: Vec<u32>,
    /// The drag lasts this many delivered ticks,
    pub drag_ticks: u32,
    /// or until this flag is raised, when there is one.
    pub end_drag: Option<Arc<AtomicBool>>,
}

impl EditorScript {
    /// Ten dragged entities and twenty-five live entities (a hundred fields),
    /// spread over the identifier table.
    pub fn new(entity_count: usize, drag_ticks: u32) -> EditorScript {
        let dragged: Vec<u32> = (0..10).map(|i| (i * entity_count / 10 + entity_count / 100) as u32).collect();
        let mut live = dragged.clone();
        live.extend((0..15).map(|i| (i * entity_count / 15 + entity_count / 40 + 1) as u32));
        let mut distinct = live.clone();
        distinct.sort_unstable();
        distinct.dedup();
        assert_eq!(distinct.len(), live.len(), "the live entities are distinct");
        EditorScript { entity_count, dragged, live, drag_ticks, end_drag: None }
    }
}

/// The velocity the drag asks for, for the `step`-th send and the
/// `entity`-th dragged entity: exact small values, different at every step.
fn dragged_velocity(step: u32, entity: u32) -> (f32, f32) {
    ((((step * 7 + entity * 13) % 49) as f32) - 24.0, (((step * 11 + entity * 5) % 49) as f32) - 24.0)
}

#[derive(Default)]
pub struct ClientReport {
    /// The watched component as the change sets alone describe it.
    pub mirror: Vec<Option<ChangeRow>>,
    /// Per `set` command: milliseconds from the write to the live-value
    /// message of the tick that decided it, and that message's tick minus the
    /// tick the client had seen when it sent.
    pub latencies: Vec<(f32, u32)>,
    pub sets_sent: u64,
    pub set_bytes_strings: u64,
    pub set_bytes_interned: u64,
    pub change_sets: u32,
    pub rows: u64,
    pub live_messages: u32,
    pub live_values: u64,
    pub sequence_in_order: bool,
    pub drag_first_tick: u32,
    pub drag_last_tick: u32,
    pub transactions_sent: u32,
    pub transaction_sets: Vec<FieldSet>,
    pub transaction: Option<TransactionResult>,
    pub final_tick: u32,
    pub ended: bool,
    pub bytes_sent: u64,
    pub bytes_received: u64,
    pub error: Option<String>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Phase {
    Dragging,
    AwaitingResult,
    ResultReceived,
    Closed,
}

struct Editor<'a> {
    script: &'a EditorScript,
    /// The generation of each dragged entity, from the live values.
    handles: Vec<Option<u32>>,
    /// The values last sent for each dragged entity.
    last_values: Vec<Option<(f32, f32)>>,
    pending: VecDeque<(u64, Instant, u32)>,
    phase: Phase,
    request: u64,
    transaction_request: u64,
    sends: u32,
    last_sent_tick: u32,
    last_sequence: u64,
    outgoing: Vec<Vec<u8>>,
}

impl Editor<'_> {
    fn drag_over(&self, report: &ClientReport, tick: u32) -> bool {
        tick - report.drag_first_tick >= self.script.drag_ticks || self.script.end_drag.as_ref().is_some_and(|flag| flag.load(Ordering::SeqCst))
    }

    /// The drag's commands for one delivered tick.
    fn drag(&mut self, report: &mut ClientReport, tick: u32) {
        self.sends += 1;
        let now = Instant::now();
        for (entity, &index) in self.script.dragged.iter().enumerate() {
            let Some(generation) = self.handles[entity] else { continue };
            let (vx, vy) = dragged_velocity(self.sends, entity as u32);
            self.last_values[entity] = Some((vx, vy));
            for (field, value) in [(Field::VelocityX, vx), (Field::VelocityY, vy)] {
                let set = FieldSet { index, generation, field, value };
                self.request += 1;
                let mut message = Vec::new();
                wire::encode_set(&mut message, SESSION, self.request, &set, Names::Strings);
                report.set_bytes_strings += message.len() as u64;
                let mut interned = Vec::new();
                wire::encode_set(&mut interned, SESSION, self.request, &set, Names::Interned);
                report.set_bytes_interned += interned.len() as u64;
                self.outgoing.push(message);
                self.pending.push_back((self.request, now, tick));
                report.sets_sent += 1;
            }
        }
        report.drag_last_tick = tick;
    }

    /// The gesture's end: its final values as one transaction.
    fn close_gesture(&mut self, report: &mut ClientReport) {
        let mut sets = Vec::new();
        for (entity, &index) in self.script.dragged.iter().enumerate() {
            let (Some(generation), Some((vx, vy))) = (self.handles[entity], self.last_values[entity]) else { continue };
            sets.push(FieldSet { index, generation, field: Field::VelocityX, value: vx });
            sets.push(FieldSet { index, generation, field: Field::VelocityY, value: vy });
        }
        self.request += 1;
        self.transaction_request = self.request;
        let mut message = Vec::new();
        wire::encode_transaction(&mut message, SESSION, self.request, &sets, true, Names::Strings);
        self.outgoing.push(message);
        report.transactions_sent += 1;
        report.transaction_sets = sets;
        self.phase = Phase::AwaitingResult;
    }
}

pub fn run(mut link: ClientSide, script: &EditorScript) -> ClientReport {
    let mut report = ClientReport { mirror: vec![None; script.entity_count], sequence_in_order: true, ..ClientReport::default() };
    if let Err(error) = converse(&mut link, script, &mut report) {
        report.error = Some(error);
    }
    report.bytes_sent = link.traffic.sent.load(Ordering::Relaxed);
    report.bytes_received = link.traffic.received.load(Ordering::Relaxed);
    report
}

fn converse(link: &mut ClientSide, script: &EditorScript, report: &mut ClientReport) -> Result<(), String> {
    let mut editor = Editor {
        script,
        handles: vec![None; script.dragged.len()],
        last_values: vec![None; script.dragged.len()],
        pending: VecDeque::new(),
        phase: Phase::Dragging,
        request: 0,
        transaction_request: 0,
        sends: 0,
        last_sent_tick: 0,
        last_sequence: 0,
        outgoing: Vec::new(),
    };
    while let Some(message) = link.receive.receive().map_err(|error| format!("receive failed: {error}"))? {
        match message.first().copied() {
            Some(wire::KIND_CHANGE_SET) => {
                let change_set = wire::decode_change_set(&message, Names::Strings).map_err(|error| error.0)?;
                report.sequence_in_order &= change_set.sequence > editor.last_sequence;
                editor.last_sequence = change_set.sequence;
                report.change_sets += 1;
                report.rows += change_set.rows.len() as u64;
                for row in change_set.rows {
                    let slot = report.mirror.get_mut(row.index as usize).ok_or_else(|| format!("a row for the unknown index {}", row.index))?;
                    *slot = Some(row);
                }
            }
            Some(wire::KIND_LIVE_VALUES) => {
                let received = Instant::now();
                let live = wire::decode_live_values(&message, Names::Strings).map_err(|error| error.0)?;
                report.sequence_in_order &= live.sequence > editor.last_sequence;
                editor.last_sequence = live.sequence;
                report.live_messages += 1;
                report.live_values += live.values.len() as u64;
                for value in &live.values {
                    if let Some(entity) = script.dragged.iter().position(|index| *index == value.index) {
                        editor.handles[entity] = Some(value.generation);
                    }
                }
                while let Some(&(request, sent, seen_tick)) = editor.pending.front() {
                    if request > live.applied_through {
                        break;
                    }
                    editor.pending.pop_front();
                    report.latencies.push(((received - sent).as_secs_f32() * 1_000.0, live.tick - seen_tick));
                }
                if report.live_messages == 1 {
                    report.drag_first_tick = live.tick;
                }
                if live.tick > editor.last_sent_tick {
                    match editor.phase {
                        Phase::Dragging if editor.drag_over(report, live.tick) => editor.close_gesture(report),
                        Phase::Dragging => editor.drag(report, live.tick),
                        Phase::ResultReceived if live.applied_through >= editor.transaction_request => {
                            let mut close = Vec::new();
                            wire::encode_close(&mut close, SESSION);
                            editor.outgoing.push(close);
                            editor.phase = Phase::Closed;
                        }
                        _ => {}
                    }
                    editor.last_sent_tick = live.tick;
                }
            }
            Some(wire::KIND_TRANSACTION_RESULT) => {
                let result = wire::decode_transaction_result(&message, Names::Strings).map_err(|error| error.0)?;
                if result.request != editor.transaction_request {
                    return Err(format!("a result for request {}, the transaction was request {}", result.request, editor.transaction_request));
                }
                let refused = matches!(result.outcome, Outcome::TransactionRejected { .. });
                report.transaction = Some(result);
                // A refused transaction is sent again at the next delivered
                // tick, with the handles the live values show by then.
                editor.phase = if refused && report.transactions_sent < TRANSACTION_ATTEMPTS { Phase::Dragging } else { Phase::ResultReceived };
            }
            Some(wire::KIND_END) => {
                report.final_tick = wire::decode_end(&message).map_err(|error| error.0)?;
                report.ended = true;
                return Ok(());
            }
            other => return Err(format!("unknown message kind {other:?}")),
        }
        if !editor.outgoing.is_empty() {
            link.submit.submit(&editor.outgoing).map_err(|error| format!("submit failed: {error}"))?;
            editor.outgoing.clear();
        }
    }
    Err("the stream ended without its end message".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_script_spreads_distinct_entities_and_the_drag_changes_its_values_at_every_step() {
        for entity_count in [1_000, 10_000] {
            let script = EditorScript::new(entity_count, 600);
            assert_eq!(script.dragged.len(), 10);
            assert_eq!(script.live.len() * 4, 100, "a hundred live fields");
            assert!(script.live.iter().all(|index| (*index as usize) < entity_count));
            assert!(script.dragged.iter().all(|index| script.live.contains(index)));
        }
        for step in 1..200 {
            let (vx, vy) = dragged_velocity(step, 3);
            assert!((-24.0..=24.0).contains(&vx) && (-24.0..=24.0).contains(&vy));
            assert_ne!(dragged_velocity(step + 1, 3), (vx, vy));
        }
    }
}
