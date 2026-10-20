//! The canonical hash covers every serde shape: the hash of a value that holds one of each
//! is pinned, so the format cannot change unnoticed on any target, and changing any single
//! field, a variant's index under the same payload, or where content ends between length
//! prefixes changes it, so no shape can silently drop out of the state hash. A sequence of
//! unknown length is refused with the reason, and hashes travel as sixteen hexadecimal
//! digits.

use std::collections::BTreeMap;

use ludenea_ecs::{NamedHash, hash_value};
use serde::ser::SerializeSeq;
use serde::{Serialize, Serializer};

/// The hash of `shapes()`. It is the same on every target; a change to it changes every
/// state hash, golden trace and replay, so it is a change of the hash format.
const SHAPES_HASH: u64 = 14_838_969_156_561_360_344;

#[derive(Serialize)]
struct Marker;

#[derive(Serialize)]
struct Meters(i32);

#[derive(Serialize)]
struct Pair(u8, i16);

#[derive(Serialize)]
enum Shape {
    Empty,
    Full,
    Newtype(u32),
    Tuple(i8, u16),
    Struct {
        inner: i32,
    },
    /// After the others, so their indices, and the pinned hash, stay as they were.
    Other(u32),
}

/// A byte string: serde has no type of its own for one (a `Vec<u8>` is a sequence).
struct Bytes(Vec<u8>);

impl Serialize for Bytes {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_bytes(&self.0)
    }
}

/// One field of every serde shape.
#[derive(Serialize)]
struct Shapes {
    u8: u8,
    u16: u16,
    u32: u32,
    u64: u64,
    u128: u128,
    i8: i8,
    i16: i16,
    i32: i32,
    i64: i64,
    i128: i128,
    char: char,
    bool: bool,
    f32: f32,
    f64: f64,
    string: String,
    bytes: Bytes,
    none: Option<u8>,
    some: Option<u8>,
    unit: (),
    unit_struct: Marker,
    newtype_struct: Meters,
    tuple_struct: Pair,
    tuple: (u8, i32),
    unit_variant: Shape,
    newtype_variant: Shape,
    tuple_variant: Shape,
    struct_variant: Shape,
    sequence: Vec<i32>,
    string_keys: BTreeMap<String, i32>,
    integer_keys: BTreeMap<u16, String>,
}

fn shapes() -> Shapes {
    Shapes {
        u8: 8,
        u16: 16,
        u32: 32,
        u64: 64,
        u128: 128,
        i8: -8,
        i16: -16,
        i32: -32,
        i64: -64,
        i128: -128,
        char: 'l',
        bool: true,
        f32: 0.5,
        f64: -1.25,
        string: "ludenea".to_string(),
        bytes: Bytes(vec![1, 2, 3]),
        none: None,
        some: Some(7),
        unit: (),
        unit_struct: Marker,
        newtype_struct: Meters(100),
        tuple_struct: Pair(1, -2),
        tuple: (3, -4),
        unit_variant: Shape::Empty,
        newtype_variant: Shape::Newtype(5),
        tuple_variant: Shape::Tuple(-6, 7),
        struct_variant: Shape::Struct { inner: -9 },
        sequence: vec![10, 20, 30],
        string_keys: BTreeMap::from([("x".to_string(), 1), ("y".to_string(), 2)]),
        integer_keys: BTreeMap::from([(1, "one".to_string()), (2, "two".to_string())]),
    }
}

#[test]
fn every_shape_hashes_to_the_pinned_value() {
    assert_eq!(hash_value(&shapes()), SHAPES_HASH);
}

/// A change to a single field.
type Change = fn(&mut Shapes);

#[test]
fn every_field_of_every_shape_changes_the_hash() {
    // The unit and the unit struct have a single value, so only the pinned hash covers them.
    let changes: &[(&str, Change)] = &[
        ("u8", |shapes| shapes.u8 += 1),
        ("u16", |shapes| shapes.u16 += 1),
        ("u32", |shapes| shapes.u32 += 1),
        ("u64", |shapes| shapes.u64 += 1),
        ("u128", |shapes| shapes.u128 += 1),
        ("i8", |shapes| shapes.i8 += 1),
        ("i16", |shapes| shapes.i16 += 1),
        ("i32", |shapes| shapes.i32 += 1),
        ("i64", |shapes| shapes.i64 += 1),
        ("i128", |shapes| shapes.i128 += 1),
        ("char", |shapes| shapes.char = 'm'),
        ("bool", |shapes| shapes.bool = false),
        ("f32", |shapes| shapes.f32 = 0.75),
        ("f64", |shapes| shapes.f64 = -1.5),
        ("string", |shapes| shapes.string.push('!')),
        ("bytes", |shapes| shapes.bytes.0[0] += 1),
        ("none", |shapes| shapes.none = Some(0)),
        ("some", |shapes| shapes.some = Some(8)),
        ("newtype_struct", |shapes| shapes.newtype_struct.0 += 1),
        ("tuple_struct.0", |shapes| shapes.tuple_struct.0 += 1),
        ("tuple_struct.1", |shapes| shapes.tuple_struct.1 += 1),
        ("tuple.0", |shapes| shapes.tuple.0 += 1),
        ("tuple.1", |shapes| shapes.tuple.1 += 1),
        ("unit_variant", |shapes| shapes.unit_variant = Shape::Full),
        ("newtype_variant", |shapes| {
            shapes.newtype_variant = Shape::Newtype(6);
        }),
        ("newtype_variant index", |shapes| {
            shapes.newtype_variant = Shape::Other(5);
        }),
        ("tuple_variant.0", |shapes| {
            shapes.tuple_variant = Shape::Tuple(-5, 7);
        }),
        ("tuple_variant.1", |shapes| {
            shapes.tuple_variant = Shape::Tuple(-6, 8);
        }),
        ("struct_variant.inner", |shapes| {
            shapes.struct_variant = Shape::Struct { inner: -8 };
        }),
        ("sequence[1]", |shapes| shapes.sequence[1] += 1),
        ("string_keys key", |shapes| {
            let value = shapes.string_keys.remove("y").unwrap();
            shapes.string_keys.insert("z".to_string(), value);
        }),
        ("string_keys value", |shapes| {
            *shapes.string_keys.get_mut("x").unwrap() += 1;
        }),
        ("integer_keys key", |shapes| {
            let value = shapes.integer_keys.remove(&2).unwrap();
            shapes.integer_keys.insert(3, value);
        }),
        ("integer_keys value", |shapes| {
            shapes.integer_keys.get_mut(&1).unwrap().push('!');
        }),
        // The same bytes in the same order, the string's last one moved to the front of the
        // byte string that follows it: only the length prefixes tell them apart.
        ("string end moved into bytes", |shapes| {
            let moved = shapes.string.pop().unwrap();
            shapes.bytes.0.insert(0, moved as u8);
        }),
    ];
    let original = hash_value(&shapes());
    for (field, change) in changes {
        let mut changed = shapes();
        change(&mut changed);
        assert_ne!(
            hash_value(&changed),
            original,
            "changing {field} left the hash unchanged"
        );
    }
}

/// A sequence that does not report its length.
struct UnknownLength;

impl Serialize for UnknownLength {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_seq(None)?.end()
    }
}

#[test]
#[should_panic(
    expected = "hash_value cannot hash this value: sequences and maps must report their length to be hashed"
)]
fn a_sequence_of_unknown_length_panics_with_the_reason() {
    hash_value(&UnknownLength);
}

#[test]
fn hashes_travel_as_exactly_sixteen_lowercase_hexadecimal_digits() {
    let named = NamedHash {
        name: "x".to_string(),
        hash: 0x0123_4567_89ab_cdef,
    };
    let text = r#"{"name":"x","hash":"0123456789abcdef"}"#;
    assert_eq!(serde_json::to_string(&named).unwrap(), text);
    assert_eq!(serde_json::from_str::<NamedHash>(text).unwrap(), named);
    for hash in ["ff", "00000000000000FF", "+00000000000000f"] {
        let text = format!(r#"{{"name":"x","hash":"{hash}"}}"#);
        let error = serde_json::from_str::<NamedHash>(&text).unwrap_err();
        assert!(
            error
                .to_string()
                .starts_with("expected exactly 16 lowercase hexadecimal digits"),
            "{hash}: {error}"
        );
    }
}
