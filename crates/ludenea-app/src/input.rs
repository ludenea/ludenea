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

/// The input type of an app, erased so `Simulation` needs no type parameter.
pub(crate) struct InputType {
    pub(crate) id: TypeId,
    pub(crate) name: &'static str,
    /// Writes `players` default inputs.
    pub(crate) write_defaults: fn(&mut World, usize),
    /// Decodes a postcard `Vec<I>`, rejecting leftover bytes, then writes it.
    /// Also rejects a `Vec` that does not hold exactly the given number of players.
    pub(crate) write_encoded: fn(&mut World, &[u8], usize) -> Result<(), SimError>,
    pub(crate) schema: fn() -> TypeSchema,
}

impl InputType {
    pub(crate) fn of<I: Input>() -> Self {
        Self {
            id: TypeId::of::<I>(),
            name: type_name::<I>(),
            write_defaults: |world, players| write(world, vec![I::default(); players]),
            write_encoded: |world, bytes, expected| {
                let players = decode_exact::<Vec<I>>(bytes)?;
                check_player_count(expected, players.len())?;
                write(world, players);
                Ok(())
            },
            schema: I::schema,
        }
    }
}

/// Decodes postcard bytes, rejecting any bytes left unread after the value.
pub(crate) fn decode_exact<T: DeserializeOwned>(bytes: &[u8]) -> Result<T, SimError> {
    let (value, rest) = postcard::take_from_bytes(bytes)
        .map_err(|error| SimError::InputDecode(error.to_string()))?;
    if rest.is_empty() {
        Ok(value)
    } else {
        Err(SimError::InputDecode(format!(
            "{} unexpected trailing bytes",
            rest.len()
        )))
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
