//! The messages of the paced editor session. Throwaway, never product code.
//!
//! Every message is a kind, the session, and a body whose names are stable
//! names. A stable name travels as a string (a 16-bit length and its bytes)
//! or, under the interned names capability, as a 16-bit integer the session
//! assigned to it: every encoder takes the choice as a parameter, so that the
//! same message is measured both ways. The rows of a state stream (a change
//! set, a full-state read) are encoded in one of two layouts: canonical bytes
//! with the field names once in the message's header (the data plane's
//! form), or name-tagged fields in every row (the control plane's framing
//! applied to rows).

use crate::world::{Command, Field, FieldSet, Outcome, Rejection};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Names {
    Strings,
    Interned,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Rows {
    Canonical,
    Tagged,
}

/// The stable names of the session's contract. Under the interned names
/// capability a name travels as its position in this table.
pub const STABLE_NAMES: [&str; 26] = [
    "change_set",
    "live_values",
    "full_state",
    "set",
    "transaction",
    "entity",
    "field",
    "value",
    "want_inverse",
    "sets",
    "sampled_motion",
    "sampled_motion.position_x",
    "sampled_motion.position_y",
    "sampled_motion.velocity_x",
    "sampled_motion.velocity_y",
    "identity.generation",
    "identity.alive",
    "position.x",
    "position.y",
    "velocity.x",
    "velocity.y",
    "mass.value",
    "world.energy",
    "world.random_stream",
    "inverse",
    "rejected",
];

/// The fields of the watched component, in row order.
pub const WATCHED_FIELDS: [&str; 4] =
    ["sampled_motion.position_x", "sampled_motion.position_y", "sampled_motion.velocity_x", "sampled_motion.velocity_y"];

/// The fields of a full-state row after the generation and the alive flag.
pub const STATE_FIELDS: [&str; 9] = [
    "position.x",
    "position.y",
    "velocity.x",
    "velocity.y",
    "mass.value",
    "sampled_motion.position_x",
    "sampled_motion.position_y",
    "sampled_motion.velocity_x",
    "sampled_motion.velocity_y",
];

pub const KIND_CHANGE_SET: u8 = 1;
pub const KIND_LIVE_VALUES: u8 = 2;
pub const KIND_TRANSACTION_RESULT: u8 = 3;
pub const KIND_END: u8 = 4;
pub const KIND_FULL_STATE: u8 = 5;
pub const KIND_SET: u8 = 16;
pub const KIND_TRANSACTION: u8 = 17;
pub const KIND_CLOSE: u8 = 18;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DecodeError(pub String);

impl std::fmt::Display for DecodeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

type Decoded<T> = Result<T, DecodeError>;

// ---------------------------------------------------------------------------
// Primitives
// ---------------------------------------------------------------------------

fn put_u16(out: &mut Vec<u8>, v: u16) {
    out.extend_from_slice(&v.to_le_bytes());
}

fn put_u32(out: &mut Vec<u8>, v: u32) {
    out.extend_from_slice(&v.to_le_bytes());
}

fn put_u64(out: &mut Vec<u8>, v: u64) {
    out.extend_from_slice(&v.to_le_bytes());
}

fn put_f32(out: &mut Vec<u8>, v: f32) {
    out.extend_from_slice(&v.to_le_bytes());
}

fn put_name(out: &mut Vec<u8>, names: Names, name: &str) {
    match names {
        Names::Strings => {
            put_u16(out, name.len() as u16);
            out.extend_from_slice(name.as_bytes());
        }
        Names::Interned => {
            let id = STABLE_NAMES.iter().position(|known| *known == name).expect("a stable name of the contract");
            put_u16(out, id as u16);
        }
    }
}

pub fn name_size(names: Names, name: &str) -> usize {
    match names {
        Names::Strings => 2 + name.len(),
        Names::Interned => 2,
    }
}

pub struct Reader<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl<'a> Reader<'a> {
    pub fn new(bytes: &'a [u8]) -> Reader<'a> {
        Reader { bytes, at: 0 }
    }

    fn take(&mut self, len: usize) -> Decoded<&'a [u8]> {
        let end = self.at.checked_add(len).filter(|end| *end <= self.bytes.len());
        let Some(end) = end else {
            return Err(DecodeError(format!("message truncated at byte {} of {}", self.at, self.bytes.len())));
        };
        let slice = &self.bytes[self.at..end];
        self.at = end;
        Ok(slice)
    }

    pub fn u8(&mut self) -> Decoded<u8> {
        Ok(self.take(1)?[0])
    }

    fn u16(&mut self) -> Decoded<u16> {
        Ok(u16::from_le_bytes(self.take(2)?.try_into().expect("two bytes")))
    }

    fn u32(&mut self) -> Decoded<u32> {
        Ok(u32::from_le_bytes(self.take(4)?.try_into().expect("four bytes")))
    }

    fn u64(&mut self) -> Decoded<u64> {
        Ok(u64::from_le_bytes(self.take(8)?.try_into().expect("eight bytes")))
    }

    fn f32(&mut self) -> Decoded<f32> {
        Ok(f32::from_le_bytes(self.take(4)?.try_into().expect("four bytes")))
    }

    /// A stable name, resolved against the contract: an unknown name is an
    /// error that names it.
    fn name(&mut self, names: Names) -> Decoded<&'static str> {
        match names {
            Names::Strings => {
                let len = self.u16()? as usize;
                let text = std::str::from_utf8(self.take(len)?).map_err(|_| DecodeError("a name is not UTF-8".to_string()))?;
                STABLE_NAMES.iter().copied().find(|known| *known == text).ok_or_else(|| DecodeError(format!("no stable name `{text}` in the contract")))
            }
            Names::Interned => {
                let id = self.u16()? as usize;
                STABLE_NAMES.get(id).copied().ok_or_else(|| DecodeError(format!("no interned name {id} in the session")))
            }
        }
    }

    fn expect_name(&mut self, names: Names, expected: &str) -> Decoded<()> {
        let found = self.name(names)?;
        if found == expected { Ok(()) } else { Err(DecodeError(format!("expected `{expected}`, found `{found}`"))) }
    }

    fn finish(&self) -> Decoded<()> {
        if self.at == self.bytes.len() { Ok(()) } else { Err(DecodeError(format!("{} bytes after the message", self.bytes.len() - self.at))) }
    }
}

// ---------------------------------------------------------------------------
// The change set
// ---------------------------------------------------------------------------

/// One changed entity of the watched component: its handle at the time the
/// component was written, and the component's current values.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ChangeRow {
    pub index: u32,
    pub generation: u32,
    pub values: [f32; 4],
}

#[derive(Clone, Debug, PartialEq)]
pub struct ChangeSet {
    pub session: u32,
    pub sequence: u64,
    /// The changes are those of the ticks after `from_tick` up to `to_tick`.
    pub from_tick: u32,
    pub to_tick: u32,
    pub rows: Vec<ChangeRow>,
}

#[allow(clippy::too_many_arguments)]
pub fn encode_change_set(out: &mut Vec<u8>, session: u32, sequence: u64, from_tick: u32, to_tick: u32, rows: &[ChangeRow], layout: Rows, names: Names) {
    out.push(KIND_CHANGE_SET);
    put_u32(out, session);
    put_u64(out, sequence);
    put_name(out, names, "change_set");
    put_u32(out, from_tick);
    put_u32(out, to_tick);
    put_name(out, names, "sampled_motion");
    out.push(u8::from(layout == Rows::Tagged));
    if layout == Rows::Canonical {
        for field in WATCHED_FIELDS {
            put_name(out, names, field);
        }
    }
    put_u32(out, rows.len() as u32);
    out.reserve(rows.len() * change_row_size(layout, names));
    for row in rows {
        put_u32(out, row.index);
        put_u32(out, row.generation);
        for (field, value) in WATCHED_FIELDS.iter().zip(row.values) {
            if layout == Rows::Tagged {
                put_name(out, names, field);
            }
            put_f32(out, value);
        }
    }
}

fn change_row_size(layout: Rows, names: Names) -> usize {
    8 + WATCHED_FIELDS.iter().map(|field| 4 + if layout == Rows::Tagged { name_size(names, field) } else { 0 }).sum::<usize>()
}

/// The size of a change set message of `rows` rows, without encoding it.
pub fn change_set_size(rows: usize, layout: Rows, names: Names) -> usize {
    let mut header = Vec::new();
    encode_change_set(&mut header, 0, 0, 0, 0, &[], layout, names);
    header.len() + rows * change_row_size(layout, names)
}

pub fn decode_change_set(bytes: &[u8], names: Names) -> Decoded<ChangeSet> {
    let mut r = Reader::new(bytes);
    if r.u8()? != KIND_CHANGE_SET {
        return Err(DecodeError("not a change set".to_string()));
    }
    let session = r.u32()?;
    let sequence = r.u64()?;
    r.expect_name(names, "change_set")?;
    let from_tick = r.u32()?;
    let to_tick = r.u32()?;
    r.expect_name(names, "sampled_motion")?;
    let tagged = r.u8()? == 1;
    if !tagged {
        for field in WATCHED_FIELDS {
            r.expect_name(names, field)?;
        }
    }
    let count = r.u32()? as usize;
    let mut rows = Vec::with_capacity(count.min(1 << 20));
    for _ in 0..count {
        let index = r.u32()?;
        let generation = r.u32()?;
        let mut values = [0.0_f32; 4];
        for (field, value) in WATCHED_FIELDS.iter().zip(values.iter_mut()) {
            if tagged {
                r.expect_name(names, field)?;
            }
            *value = r.f32()?;
        }
        rows.push(ChangeRow { index, generation, values });
    }
    r.finish()?;
    Ok(ChangeSet { session, sequence, from_tick, to_tick, rows })
}

// ---------------------------------------------------------------------------
// Live values
// ---------------------------------------------------------------------------

/// A field a live-value subscription may name.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LiveField {
    PositionX,
    PositionY,
    VelocityX,
    VelocityY,
}

impl LiveField {
    pub const ALL: [LiveField; 4] = [LiveField::PositionX, LiveField::PositionY, LiveField::VelocityX, LiveField::VelocityY];

    pub fn name(self) -> &'static str {
        match self {
            LiveField::PositionX => "position.x",
            LiveField::PositionY => "position.y",
            LiveField::VelocityX => "velocity.x",
            LiveField::VelocityY => "velocity.y",
        }
    }

    fn named(name: &str) -> Decoded<LiveField> {
        LiveField::ALL.into_iter().find(|field| field.name() == name).ok_or_else(|| DecodeError(format!("`{name}` is not a live field")))
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LiveValue {
    pub index: u32,
    pub generation: u32,
    pub field: LiveField,
    pub value: f32,
}

#[derive(Clone, Debug, PartialEq)]
pub struct LiveValues {
    pub session: u32,
    pub sequence: u64,
    /// The tick boundary the values were read at.
    pub tick: u32,
    /// Every command of the session up to this request number was decided at
    /// a tick boundary at or before `tick`.
    pub applied_through: u64,
    pub values: Vec<LiveValue>,
}

pub fn encode_live_values(out: &mut Vec<u8>, session: u32, sequence: u64, tick: u32, applied_through: u64, values: &[LiveValue], names: Names) {
    out.push(KIND_LIVE_VALUES);
    put_u32(out, session);
    put_u64(out, sequence);
    put_name(out, names, "live_values");
    put_u32(out, tick);
    put_u64(out, applied_through);
    put_u32(out, values.len() as u32);
    for value in values {
        put_u32(out, value.index);
        put_u32(out, value.generation);
        put_name(out, names, value.field.name());
        put_f32(out, value.value);
    }
}

/// The size of a live-value message, without encoding it.
pub fn live_values_size(values: &[LiveValue], names: Names) -> usize {
    1 + 4 + 8 + name_size(names, "live_values") + 4 + 8 + 4 + values.iter().map(|value| 12 + name_size(names, value.field.name())).sum::<usize>()
}

pub fn decode_live_values(bytes: &[u8], names: Names) -> Decoded<LiveValues> {
    let mut r = Reader::new(bytes);
    if r.u8()? != KIND_LIVE_VALUES {
        return Err(DecodeError("not a live-value message".to_string()));
    }
    let session = r.u32()?;
    let sequence = r.u64()?;
    r.expect_name(names, "live_values")?;
    let tick = r.u32()?;
    let applied_through = r.u64()?;
    let count = r.u32()? as usize;
    let mut values = Vec::with_capacity(count.min(1 << 16));
    for _ in 0..count {
        let index = r.u32()?;
        let generation = r.u32()?;
        let field = LiveField::named(r.name(names)?)?;
        values.push(LiveValue { index, generation, field, value: r.f32()? });
    }
    r.finish()?;
    Ok(LiveValues { session, sequence, tick, applied_through, values })
}

// ---------------------------------------------------------------------------
// Commands, their result and the end of the stream
// ---------------------------------------------------------------------------

fn settable_name(field: Field) -> &'static str {
    match field {
        Field::VelocityX => "velocity.x",
        Field::VelocityY => "velocity.y",
    }
}

fn settable(name: &str) -> Decoded<Field> {
    match name {
        "velocity.x" => Ok(Field::VelocityX),
        "velocity.y" => Ok(Field::VelocityY),
        other => Err(DecodeError(format!("field `{other}` cannot be set"))),
    }
}

fn put_set(out: &mut Vec<u8>, set: &FieldSet, names: Names) {
    put_u32(out, set.index);
    put_u32(out, set.generation);
    put_name(out, names, settable_name(set.field));
    put_f32(out, set.value);
}

fn read_set(r: &mut Reader<'_>, names: Names) -> Decoded<FieldSet> {
    let index = r.u32()?;
    let generation = r.u32()?;
    let field = settable(r.name(names)?)?;
    Ok(FieldSet { index, generation, field, value: r.f32()? })
}

/// The `set` command: name-tagged parameters, as every control plane message.
pub fn encode_set(out: &mut Vec<u8>, session: u32, request: u64, set: &FieldSet, names: Names) {
    out.push(KIND_SET);
    put_u32(out, session);
    put_u64(out, request);
    put_name(out, names, "set");
    put_name(out, names, "entity");
    put_u32(out, set.index);
    put_u32(out, set.generation);
    put_name(out, names, "field");
    put_name(out, names, settable_name(set.field));
    put_name(out, names, "value");
    put_f32(out, set.value);
}

pub fn encode_transaction(out: &mut Vec<u8>, session: u32, request: u64, sets: &[FieldSet], want_inverse: bool, names: Names) {
    out.push(KIND_TRANSACTION);
    put_u32(out, session);
    put_u64(out, request);
    put_name(out, names, "transaction");
    put_name(out, names, "want_inverse");
    out.push(u8::from(want_inverse));
    put_name(out, names, "sets");
    put_u32(out, sets.len() as u32);
    for set in sets {
        put_set(out, set, names);
    }
}

pub fn encode_close(out: &mut Vec<u8>, session: u32) {
    out.push(KIND_CLOSE);
    put_u32(out, session);
}

#[derive(Clone, Debug, PartialEq)]
pub enum ClientMessage {
    Command { session: u32, request: u64, command: Command },
    Close { session: u32 },
}

pub fn decode_client_message(bytes: &[u8], names: Names) -> Decoded<ClientMessage> {
    let mut r = Reader::new(bytes);
    let kind = r.u8()?;
    let session = r.u32()?;
    let message = match kind {
        KIND_SET => {
            let request = r.u64()?;
            r.expect_name(names, "set")?;
            r.expect_name(names, "entity")?;
            let index = r.u32()?;
            let generation = r.u32()?;
            r.expect_name(names, "field")?;
            let field = settable(r.name(names)?)?;
            r.expect_name(names, "value")?;
            let value = r.f32()?;
            ClientMessage::Command { session, request, command: Command::Set(FieldSet { index, generation, field, value }) }
        }
        KIND_TRANSACTION => {
            let request = r.u64()?;
            r.expect_name(names, "transaction")?;
            r.expect_name(names, "want_inverse")?;
            let want_inverse = r.u8()? == 1;
            r.expect_name(names, "sets")?;
            let count = r.u32()? as usize;
            let mut sets = Vec::with_capacity(count.min(1 << 16));
            for _ in 0..count {
                sets.push(read_set(&mut r, names)?);
            }
            ClientMessage::Command { session, request, command: Command::Transaction { sets, want_inverse } }
        }
        KIND_CLOSE => ClientMessage::Close { session },
        other => return Err(DecodeError(format!("unknown request kind {other}"))),
    };
    r.finish()?;
    Ok(message)
}

/// The answer to a transaction: the tick boundary it was decided at and what
/// the fixed point decided.
#[derive(Clone, Debug, PartialEq)]
pub struct TransactionResult {
    pub session: u32,
    pub request: u64,
    pub tick: u32,
    pub outcome: Outcome,
}

pub fn encode_transaction_result(out: &mut Vec<u8>, result: &TransactionResult, names: Names) {
    out.push(KIND_TRANSACTION_RESULT);
    put_u32(out, result.session);
    put_u64(out, result.request);
    put_u32(out, result.tick);
    match &result.outcome {
        Outcome::TransactionApplied { inverse } => {
            put_name(out, names, "inverse");
            put_u32(out, inverse.len() as u32);
            for set in inverse {
                put_set(out, set, names);
            }
        }
        Outcome::TransactionRejected { element, reason } => {
            put_name(out, names, "rejected");
            put_u32(out, *element as u32);
            out.push(match reason {
                Rejection::StaleHandle => 0,
                Rejection::UnknownIndex => 1,
                Rejection::InvalidValue => 2,
            });
        }
        Outcome::Applied | Outcome::Rejected(_) => unreachable!("only a transaction is answered by a message"),
    }
}

pub fn decode_transaction_result(bytes: &[u8], names: Names) -> Decoded<TransactionResult> {
    let mut r = Reader::new(bytes);
    if r.u8()? != KIND_TRANSACTION_RESULT {
        return Err(DecodeError("not a transaction result".to_string()));
    }
    let session = r.u32()?;
    let request = r.u64()?;
    let tick = r.u32()?;
    let outcome = match r.name(names)? {
        "inverse" => {
            let count = r.u32()? as usize;
            let mut inverse = Vec::with_capacity(count.min(1 << 16));
            for _ in 0..count {
                inverse.push(read_set(&mut r, names)?);
            }
            Outcome::TransactionApplied { inverse }
        }
        "rejected" => {
            let element = r.u32()? as usize;
            let reason = match r.u8()? {
                0 => Rejection::StaleHandle,
                1 => Rejection::UnknownIndex,
                2 => Rejection::InvalidValue,
                other => return Err(DecodeError(format!("unknown rejection {other}"))),
            };
            Outcome::TransactionRejected { element, reason }
        }
        other => return Err(DecodeError(format!("unexpected `{other}` in a transaction result"))),
    };
    r.finish()?;
    Ok(TransactionResult { session, request, tick, outcome })
}

pub fn encode_end(out: &mut Vec<u8>, session: u32, tick: u32) {
    out.push(KIND_END);
    put_u32(out, session);
    put_u32(out, tick);
}

pub fn decode_end(bytes: &[u8]) -> Decoded<u32> {
    let mut r = Reader::new(bytes);
    if r.u8()? != KIND_END {
        return Err(DecodeError("not the end of the stream".to_string()));
    }
    r.u32()?;
    let tick = r.u32()?;
    r.finish()?;
    Ok(tick)
}

// ---------------------------------------------------------------------------
// The full-state read
// ---------------------------------------------------------------------------

/// Every component of every slot at one tick boundary, and the two globals:
/// what a client without change sets would read at every period.
pub struct FullState<'a> {
    pub tick: u32,
    pub generation: &'a [u32],
    pub alive: &'a [bool],
    /// The columns of [`STATE_FIELDS`], in that order.
    pub columns: [&'a [f32]; 9],
    pub energy: f32,
    pub random_stream: u64,
}

pub fn encode_full_state(out: &mut Vec<u8>, session: u32, state: &FullState<'_>, layout: Rows, names: Names) {
    out.push(KIND_FULL_STATE);
    put_u32(out, session);
    put_name(out, names, "full_state");
    put_u32(out, state.tick);
    out.push(u8::from(layout == Rows::Tagged));
    if layout == Rows::Canonical {
        put_name(out, names, "identity.generation");
        put_name(out, names, "identity.alive");
        for field in STATE_FIELDS {
            put_name(out, names, field);
        }
    }
    put_u32(out, state.generation.len() as u32);
    for slot in 0..state.generation.len() {
        if layout == Rows::Tagged {
            put_name(out, names, "identity.generation");
        }
        put_u32(out, state.generation[slot]);
        if layout == Rows::Tagged {
            put_name(out, names, "identity.alive");
        }
        out.push(u8::from(state.alive[slot]));
        for (field, column) in STATE_FIELDS.iter().zip(state.columns) {
            if layout == Rows::Tagged {
                put_name(out, names, field);
            }
            put_f32(out, column[slot]);
        }
    }
    put_name(out, names, "world.energy");
    put_f32(out, state.energy);
    put_name(out, names, "world.random_stream");
    put_u64(out, state.random_stream);
}

#[cfg(test)]
mod tests {
    use super::*;

    const BOTH: [Names; 2] = [Names::Strings, Names::Interned];

    fn rows(count: u32) -> Vec<ChangeRow> {
        (0..count).map(|i| ChangeRow { index: i * 3, generation: i % 4, values: [i as f32, 0.5, -1.0, 2.0 + i as f32] }).collect()
    }

    #[test]
    fn a_change_set_round_trips_in_every_layout_and_its_size_is_the_predicted_one() {
        for names in BOTH {
            for layout in [Rows::Canonical, Rows::Tagged] {
                for count in [0, 1, 257] {
                    let rows = rows(count);
                    let mut bytes = Vec::new();
                    encode_change_set(&mut bytes, 7, 42, 10, 12, &rows, layout, names);
                    assert_eq!(bytes.len(), change_set_size(rows.len(), layout, names));
                    let decoded = decode_change_set(&bytes, names).unwrap();
                    assert_eq!(decoded, ChangeSet { session: 7, sequence: 42, from_tick: 10, to_tick: 12, rows });
                }
            }
        }
        // A canonical row is its handle and four values whatever the names.
        assert_eq!(change_set_size(1_000, Rows::Canonical, Names::Strings) - change_set_size(0, Rows::Canonical, Names::Strings), 24_000);
        assert_eq!(change_set_size(1, Rows::Tagged, Names::Interned) - change_set_size(0, Rows::Tagged, Names::Interned), 32);
    }

    #[test]
    fn live_values_round_trip_and_their_size_is_the_predicted_one() {
        let values: Vec<LiveValue> =
            (0..25).flat_map(|i| LiveField::ALL.map(|field| LiveValue { index: i, generation: 1, field, value: i as f32 })).collect();
        for names in BOTH {
            let mut bytes = Vec::new();
            encode_live_values(&mut bytes, 1, 9, 600, 12_000, &values, names);
            assert_eq!(bytes.len(), live_values_size(&values, names));
            let decoded = decode_live_values(&bytes, names).unwrap();
            assert_eq!(decoded, LiveValues { session: 1, sequence: 9, tick: 600, applied_through: 12_000, values: values.clone() });
        }
    }

    #[test]
    fn commands_and_the_transaction_result_round_trip() {
        let set = FieldSet { index: 5, generation: 2, field: Field::VelocityY, value: -3.5 };
        let other = FieldSet { index: 6, generation: 0, field: Field::VelocityX, value: 8.0 };
        for names in BOTH {
            let mut bytes = Vec::new();
            encode_set(&mut bytes, 1, 77, &set, names);
            assert_eq!(decode_client_message(&bytes, names).unwrap(), ClientMessage::Command { session: 1, request: 77, command: Command::Set(set) });

            bytes.clear();
            encode_transaction(&mut bytes, 1, 78, &[set, other], true, names);
            let expected = Command::Transaction { sets: vec![set, other], want_inverse: true };
            assert_eq!(decode_client_message(&bytes, names).unwrap(), ClientMessage::Command { session: 1, request: 78, command: expected });

            bytes.clear();
            encode_close(&mut bytes, 1);
            assert_eq!(decode_client_message(&bytes, names).unwrap(), ClientMessage::Close { session: 1 });

            for outcome in [Outcome::TransactionApplied { inverse: vec![other, set] }, Outcome::TransactionRejected { element: 1, reason: Rejection::StaleHandle }] {
                let result = TransactionResult { session: 1, request: 78, tick: 601, outcome };
                bytes.clear();
                encode_transaction_result(&mut bytes, &result, names);
                assert_eq!(decode_transaction_result(&bytes, names).unwrap(), result);
            }
        }
        let mut end = Vec::new();
        encode_end(&mut end, 1, 640);
        assert_eq!(decode_end(&end).unwrap(), 640);
    }

    #[test]
    fn an_unknown_name_a_field_that_cannot_be_set_and_a_truncated_message_are_errors_that_say_why() {
        let mut bytes = vec![KIND_SET];
        put_u32(&mut bytes, 1);
        put_u64(&mut bytes, 1);
        put_u16(&mut bytes, 8);
        bytes.extend_from_slice(b"teleport");
        assert_eq!(decode_client_message(&bytes, Names::Strings).unwrap_err().0, "no stable name `teleport` in the contract");

        // A hand-made `set` of the position, which the contract does not let a client write.
        let mut bytes = vec![KIND_SET];
        put_u32(&mut bytes, 1);
        put_u64(&mut bytes, 1);
        put_name(&mut bytes, Names::Strings, "set");
        put_name(&mut bytes, Names::Strings, "entity");
        put_u32(&mut bytes, 0);
        put_u32(&mut bytes, 0);
        put_name(&mut bytes, Names::Strings, "field");
        put_name(&mut bytes, Names::Strings, "position.x");
        assert_eq!(decode_client_message(&bytes, Names::Strings).unwrap_err().0, "field `position.x` cannot be set");

        let set = FieldSet { index: 5, generation: 2, field: Field::VelocityY, value: -3.5 };
        let mut bytes = Vec::new();
        encode_set(&mut bytes, 1, 77, &set, Names::Interned);
        assert!(decode_client_message(&bytes[..bytes.len() - 1], Names::Interned).unwrap_err().0.starts_with("message truncated"));
        assert_eq!(decode_client_message(&[99, 0, 0, 0, 0], Names::Strings).unwrap_err().0, "unknown request kind 99");
    }

    #[test]
    fn a_full_state_read_has_one_row_per_slot_in_both_layouts() {
        let n = 10;
        let generation = vec![1_u32; n];
        let alive = vec![true; n];
        let column = vec![0.25_f32; n];
        let state = FullState { tick: 3, generation: &generation, alive: &alive, columns: [&column; 9], energy: 1.0, random_stream: 2 };
        let size = |layout, names| {
            let mut bytes = Vec::new();
            encode_full_state(&mut bytes, 1, &state, layout, names);
            bytes.len()
        };
        let canonical = size(Rows::Canonical, Names::Interned);
        assert_eq!(canonical, 1 + 4 + 2 + 4 + 1 + 11 * 2 + 4 + n * 41 + 2 + 4 + 2 + 8);
        assert_eq!(size(Rows::Tagged, Names::Interned), canonical - 11 * 2 + n * 11 * 2);
        assert!(size(Rows::Canonical, Names::Strings) > canonical);
        assert!(size(Rows::Tagged, Names::Strings) > size(Rows::Tagged, Names::Interned));
    }
}
