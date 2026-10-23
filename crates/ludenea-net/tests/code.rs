use std::collections::BTreeSet;

use ludenea_net::{ROOM_CODE_ALPHABET, ROOM_CODE_LENGTH, RoomCode, RoomCodeError};

#[test]
fn room_code_from_entropy_uses_the_alphabet() {
    let codes: BTreeSet<RoomCode> = (0..1_000u64).map(RoomCode::from_entropy).collect();
    assert_eq!(codes.len(), 1_000);
    for code in &codes {
        assert_eq!(code.as_str().len(), ROOM_CODE_LENGTH);
        assert!(
            code.as_str()
                .chars()
                .all(|c| ROOM_CODE_ALPHABET.contains(c))
        );
        assert_eq!(code.to_string(), code.as_str());
    }
    // Most significant digit first, base 31, and the entropy wraps at 31^6.
    assert_eq!(RoomCode::from_entropy(0).as_str(), "222222");
    assert_eq!(RoomCode::from_entropy(30).as_str(), "22222Z");
    assert_eq!(RoomCode::from_entropy(31).as_str(), "222232");
    let modulus = 31u64.pow(6);
    assert_eq!(RoomCode::from_entropy(modulus), RoomCode::from_entropy(0));
    assert_eq!(RoomCode::from_entropy(u64::MAX).as_str().len(), 6);
}

#[test]
fn room_code_parse_accepts_lowercase_and_spaces() {
    let expected = RoomCode::from_entropy(123_456_789);
    let text = expected.as_str().to_ascii_lowercase();
    assert_eq!(RoomCode::parse(&text), Ok(expected.clone()));
    assert_eq!(RoomCode::parse(&format!("  {text}\t\n")), Ok(expected));
}

#[test]
fn room_code_parse_refuses() {
    let cases: [(&str, RoomCodeError, &str); 8] = [
        (
            "ABCDE",
            RoomCodeError::Length { found: 5 },
            "a room code has 6 characters, this one has 5",
        ),
        (
            "ABCDEFG",
            RoomCodeError::Length { found: 7 },
            "a room code has 6 characters, this one has 7",
        ),
        ("ABC0EF", character('0', 4), ""),
        ("ABCDEO", character('O', 6), ""),
        ("1BCDEF", character('1', 1), ""),
        ("ABIDEF", character('I', 3), ""),
        ("AbcdeL", character('L', 6), ""),
        ("ABC-EF", character('-', 4), ""),
    ];
    for (text, error, message) in cases {
        let found = RoomCode::parse(text).expect_err(text);
        assert_eq!(found, error, "{text}");
        let expected = match error {
            RoomCodeError::Length { .. } => message.to_string(),
            RoomCodeError::Character {
                character,
                position,
            } => format!(
                "'{character}' (character {position}) is not in a room code, which uses the \
                 digits 2 to 9 and the letters except I, L and O"
            ),
        };
        assert_eq!(found.to_string(), expected);
    }
}

fn character(character: char, position: usize) -> RoomCodeError {
    RoomCodeError::Character {
        character,
        position,
    }
}
