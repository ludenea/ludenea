use std::any::{TypeId, type_name};

use ludenea_ecs::World;
use ludenea_ecs::schema::{Schema, TypeSchema};
use serde::Serialize;
use serde::de::DeserializeOwned;

use crate::SimError;

/// Per-player input of one tick, recorded and replayed with postcard.
pub trait Input: Schema + Serialize + DeserializeOwned + Default + Clone + Send + 'static {}

impl<T: Schema + Serialize + DeserializeOwned + Default + Clone + Send + 'static> Input for T {}

/// The inputs of the current tick, one per player. A runtime resource: systems read it
/// with `Runtime<Inputs<I>>`.
pub struct Inputs<I> {
    pub(crate) players: Vec<I>,
}

impl<I> Inputs<I> {
    /// # Panics
    /// If `index` is not below `len()`.
    pub fn player(&self, index: usize) -> &I {
        &self.players[index]
    }

    pub fn len(&self) -> usize {
        self.players.len()
    }

    pub fn is_empty(&self) -> bool {
        self.players.is_empty()
    }
}

/// What a rollback session needs to know about one player's input, encoded: the input a
/// player holds before any is known, and the check every received input passes.
#[derive(Debug, Clone)]
pub struct PlayerInputSpec {
    /// The postcard encoding of one `I::default()`.
    pub default: Vec<u8>,
    /// Accepts only the canonical postcard encoding of exactly one input: the bytes are
    /// decoded, encoded again and compared, so two peers never record different bytes for one
    /// value.
    pub check: fn(&[u8]) -> Result<(), SimError>,
}

/// The input type of an app, erased so `Simulation` needs no type parameter.
pub(crate) struct InputType {
    pub(crate) id: TypeId,
    pub(crate) name: &'static str,
    /// Writes `players` default inputs.
    pub(crate) write_defaults: fn(&mut World, usize),
    /// The postcard bytes of `players` default inputs.
    pub(crate) encode_defaults: fn(usize) -> Vec<u8>,
    /// Decodes a postcard `Vec<I>`, rejecting leftover bytes, then writes it.
    /// Also rejects a `Vec` that does not hold exactly the given number of players.
    pub(crate) write_encoded: fn(&mut World, &[u8], usize) -> Result<(), SimError>,
    /// Like `write_encoded` without writing.
    pub(crate) check_encoded: fn(&[u8], usize) -> Result<(), SimError>,
    /// The postcard bytes of one default input.
    pub(crate) encode_default_one: fn() -> Vec<u8>,
    /// Accepts only the canonical postcard encoding of exactly one input.
    pub(crate) check_one: fn(&[u8]) -> Result<(), SimError>,
    pub(crate) schema: fn() -> TypeSchema,
}

impl InputType {
    pub(crate) fn of<I: Input>() -> Self {
        Self {
            id: TypeId::of::<I>(),
            name: type_name::<I>(),
            write_defaults: |world, players| write(world, vec![I::default(); players]),
            encode_defaults: |players| {
                postcard::to_allocvec(&vec![I::default(); players]).expect("inputs always encode")
            },
            write_encoded: |world, bytes, expected| {
                let players = decode_inputs::<I>(bytes)?;
                check_player_count(expected, players.len())?;
                write(world, players);
                Ok(())
            },
            check_encoded: |bytes, expected| {
                let players = decode_inputs::<I>(bytes)?;
                check_player_count(expected, players.len())
            },
            encode_default_one: || encode(&I::default()),
            check_one: |bytes| {
                let canonical =
                    encode(&ludenea_ecs::decode_exact::<I>(bytes).map_err(SimError::InputDecode)?);
                if canonical == bytes {
                    Ok(())
                } else {
                    Err(SimError::InputDecode(format!(
                        "{} bytes are not the canonical encoding of one input, the canonical encoding takes {}",
                        bytes.len(),
                        canonical.len()
                    )))
                }
            },
            schema: I::schema,
        }
    }
}

pub(crate) fn check_player_count(expected: usize, got: usize) -> Result<(), SimError> {
    if expected == got {
        Ok(())
    } else {
        Err(SimError::InputDecode(format!(
            "expected {expected} players, got {got}"
        )))
    }
}

pub(crate) fn write<I: Input>(world: &mut World, players: Vec<I>) {
    world
        .runtime_mut::<Inputs<I>>()
        .expect("App::input inserts the inputs resource")
        .players = players;
}

fn encode<I: Input>(input: &I) -> Vec<u8> {
    postcard::to_allocvec(input).expect("inputs always encode")
}

fn decode_inputs<I: Input>(bytes: &[u8]) -> Result<Vec<I>, SimError> {
    ludenea_ecs::decode_exact(bytes).map_err(SimError::InputDecode)
}
