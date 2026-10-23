//! Room codes: the six characters a host reads out so others can join its room.

use std::fmt;

use thiserror::Error;

/// The characters of a room code: the digits 2 to 9 and the letters except I, L and O, which
/// are easily mistaken for 1 and 0.
pub const ROOM_CODE_ALPHABET: &str = "23456789ABCDEFGHJKMNPQRSTUVWXYZ";

/// The number of characters of a room code.
pub const ROOM_CODE_LENGTH: usize = 6;

/// A room code, always [`ROOM_CODE_LENGTH`] characters of [`ROOM_CODE_ALPHABET`].
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct RoomCode(String);

/// Why text is not a room code.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum RoomCodeError {
    /// The text, once trimmed, does not have [`ROOM_CODE_LENGTH`] characters.
    #[error("a room code has 6 characters, this one has {found}")]
    Length { found: usize },
    /// A character outside [`ROOM_CODE_ALPHABET`]; `position` counts from 1.
    #[error(
        "'{character}' (character {position}) is not in a room code, which uses the digits 2 \
         to 9 and the letters except I, L and O"
    )]
    Character { character: char, position: usize },
}

impl RoomCode {
    /// The code of `entropy % 31^6`, written in base 31 with the most significant digit first.
    pub fn from_entropy(entropy: u64) -> RoomCode {
        let base = ROOM_CODE_ALPHABET.len() as u64;
        let mut value = entropy % base.pow(ROOM_CODE_LENGTH as u32);
        let mut digits = [b'0'; ROOM_CODE_LENGTH];
        for digit in digits.iter_mut().rev() {
            // `value % base` is below 31, the alphabet's length.
            *digit = ROOM_CODE_ALPHABET.as_bytes()[(value % base) as usize];
            value /= base;
        }
        RoomCode(digits.iter().map(|&digit| char::from(digit)).collect())
    }

    /// Reads a code typed by a person: surrounding ASCII whitespace is dropped and letters
    /// are uppercased.
    ///
    /// # Errors
    ///
    /// [`RoomCodeError::Length`] when the text is not 6 characters, and
    /// [`RoomCodeError::Character`] for the first character outside the alphabet.
    pub fn parse(text: &str) -> Result<RoomCode, RoomCodeError> {
        let code = text.trim_ascii().to_ascii_uppercase();
        let found = code.chars().count();
        if found != ROOM_CODE_LENGTH {
            return Err(RoomCodeError::Length { found });
        }
        match code
            .chars()
            .enumerate()
            .find(|(_, character)| !ROOM_CODE_ALPHABET.contains(*character))
        {
            Some((index, character)) => Err(RoomCodeError::Character {
                character,
                position: index + 1,
            }),
            None => Ok(RoomCode(code)),
        }
    }

    /// The code as text.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for RoomCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}
