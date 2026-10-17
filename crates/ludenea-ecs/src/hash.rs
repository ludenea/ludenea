//! Canonical, target-independent hashing of serde values and of the world state.

use std::collections::BTreeMap;
use std::fmt;

use serde::ser;
use serde::{Deserialize, Serialize, Serializer};
use xxhash_rust::xxh3::Xxh3;

use crate::{Entity, World};

/// Name of the hash leaf that covers the entity allocator state.
pub const ENTITIES_LEAF: &str = "ludenea::Entities";

const CANONICAL_NAN_F32: u32 = 0x7fc0_0000;
const CANONICAL_NAN_F64: u64 = 0x7ff8_0000_0000_0000;

/// Hashes `value` through its serde representation with xxh3: fixed-width little-endian
/// integers (`usize` and `isize` as 64-bit), `u64` length prefixes, `u32` variant indices,
/// and floats as bits with every NaN made canonical. The result is the same on every target.
///
/// # Panics
/// If `value` serializes a sequence or map of unknown length, or reports a custom error.
pub fn hash_value<T: Serialize + ?Sized>(value: &T) -> u64 {
    let mut hasher = HashSerializer(Xxh3::new());
    value
        .serialize(&mut hasher)
        .unwrap_or_else(|error| panic!("hash_value cannot hash this value: {error}"));
    hasher.0.digest()
}

/// Serde helper: a `u64` as exactly 16 lowercase hexadecimal digits, because JavaScript
/// numbers lose `u64` precision above 2^53.
pub mod hex_u64 {
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(value: &u64, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(&format_args!("{value:016x}"))
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<u64, D::Error> {
        let text = String::deserialize(deserializer)?;
        if text.len() != 16 || !text.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f')) {
            return Err(serde::de::Error::custom(
                "expected exactly 16 lowercase hexadecimal digits",
            ));
        }
        u64::from_str_radix(&text, 16).map_err(serde::de::Error::custom)
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct NamedHash {
    pub name: String,
    #[serde(with = "hex_u64")]
    pub hash: u64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct EntityHash {
    pub entity: Entity,
    #[serde(with = "hex_u64")]
    pub hash: u64,
    /// Sorted by name.
    pub components: Vec<NamedHash>,
}

/// Hashes of the world state, from the root down to each component, so two diverging
/// worlds can be compared leaf by leaf.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct HashTree {
    #[serde(with = "hex_u64")]
    pub root: u64,
    /// Registered resources sorted by name, then the `ludenea::Entities` leaf.
    pub resources: Vec<NamedHash>,
    /// Alive entities in ascending index order.
    pub entities: Vec<EntityHash>,
}

impl World {
    pub fn hash(&self) -> u64 {
        self.hash_tree().root
    }

    pub fn hash_tree(&self) -> HashTree {
        let mut resources: Vec<NamedHash> = self
            .resources
            .iter()
            .map(|(name, entry)| NamedHash {
                name: name.to_string(),
                hash: (entry.codec.hash)(&*entry.value.value()),
            })
            .collect();
        resources.push(NamedHash {
            name: ENTITIES_LEAF.to_string(),
            hash: hash_value(&self.entities),
        });

        // Components are visited in name order, so each entity's list comes out sorted.
        let mut components: BTreeMap<u32, Vec<NamedHash>> = BTreeMap::new();
        for (name, entry) in &self.components {
            entry.storage.for_each(&mut |index, value| {
                components.entry(index).or_default().push(NamedHash {
                    name: name.to_string(),
                    hash: (entry.codec.hash)(value),
                });
            });
        }

        let entities: Vec<EntityHash> = self
            .entities()
            .map(|entity| {
                let components = components.remove(&entity.index()).unwrap_or_default();
                let mut hasher = Xxh3::new();
                for leaf in &components {
                    write_named(&mut hasher, leaf);
                }
                EntityHash {
                    entity,
                    hash: hasher.digest(),
                    components,
                }
            })
            .collect();

        let mut root = Xxh3::new();
        for leaf in &resources {
            write_named(&mut root, leaf);
        }
        for leaf in &entities {
            root.update(&leaf.entity.index().to_le_bytes());
            root.update(&leaf.entity.generation().to_le_bytes());
            root.update(&leaf.hash.to_le_bytes());
        }
        HashTree {
            root: root.digest(),
            resources,
            entities,
        }
    }
}

fn write_named(hasher: &mut Xxh3, leaf: &NamedHash) {
    hasher.update(&(leaf.name.len() as u64).to_le_bytes());
    hasher.update(leaf.name.as_bytes());
    hasher.update(&leaf.hash.to_le_bytes());
}

#[derive(Debug)]
struct HashError(String);

impl fmt::Display for HashError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl std::error::Error for HashError {}

impl ser::Error for HashError {
    fn custom<T: fmt::Display>(message: T) -> Self {
        Self(message.to_string())
    }
}

struct HashSerializer(Xxh3);

impl HashSerializer {
    fn length(&mut self, length: Option<usize>) -> Result<(), HashError> {
        let length = length.ok_or_else(|| {
            HashError("sequences and maps must report their length to be hashed".to_string())
        })?;
        self.0.update(&(length as u64).to_le_bytes());
        Ok(())
    }

    fn variant(&mut self, index: u32) {
        self.0.update(&index.to_le_bytes());
    }
}

impl Serializer for &mut HashSerializer {
    type Ok = ();
    type Error = HashError;
    type SerializeSeq = Self;
    type SerializeTuple = Self;
    type SerializeTupleStruct = Self;
    type SerializeTupleVariant = Self;
    type SerializeMap = Self;
    type SerializeStruct = Self;
    type SerializeStructVariant = Self;

    fn serialize_bool(self, value: bool) -> Result<(), HashError> {
        self.0.update(&[u8::from(value)]);
        Ok(())
    }

    fn serialize_i8(self, value: i8) -> Result<(), HashError> {
        self.0.update(&value.to_le_bytes());
        Ok(())
    }

    fn serialize_i16(self, value: i16) -> Result<(), HashError> {
        self.0.update(&value.to_le_bytes());
        Ok(())
    }

    fn serialize_i32(self, value: i32) -> Result<(), HashError> {
        self.0.update(&value.to_le_bytes());
        Ok(())
    }

    fn serialize_i64(self, value: i64) -> Result<(), HashError> {
        self.0.update(&value.to_le_bytes());
        Ok(())
    }

    fn serialize_i128(self, value: i128) -> Result<(), HashError> {
        self.0.update(&value.to_le_bytes());
        Ok(())
    }

    fn serialize_u8(self, value: u8) -> Result<(), HashError> {
        self.0.update(&value.to_le_bytes());
        Ok(())
    }

    fn serialize_u16(self, value: u16) -> Result<(), HashError> {
        self.0.update(&value.to_le_bytes());
        Ok(())
    }

    fn serialize_u32(self, value: u32) -> Result<(), HashError> {
        self.0.update(&value.to_le_bytes());
        Ok(())
    }

    fn serialize_u64(self, value: u64) -> Result<(), HashError> {
        self.0.update(&value.to_le_bytes());
        Ok(())
    }

    fn serialize_u128(self, value: u128) -> Result<(), HashError> {
        self.0.update(&value.to_le_bytes());
        Ok(())
    }

    fn serialize_f32(self, value: f32) -> Result<(), HashError> {
        let bits = if value.is_nan() {
            CANONICAL_NAN_F32
        } else {
            value.to_bits()
        };
        self.0.update(&bits.to_le_bytes());
        Ok(())
    }

    fn serialize_f64(self, value: f64) -> Result<(), HashError> {
        let bits = if value.is_nan() {
            CANONICAL_NAN_F64
        } else {
            value.to_bits()
        };
        self.0.update(&bits.to_le_bytes());
        Ok(())
    }

    fn serialize_char(self, value: char) -> Result<(), HashError> {
        self.0.update(&u32::from(value).to_le_bytes());
        Ok(())
    }

    fn serialize_str(self, value: &str) -> Result<(), HashError> {
        self.serialize_bytes(value.as_bytes())
    }

    fn serialize_bytes(self, value: &[u8]) -> Result<(), HashError> {
        self.length(Some(value.len()))?;
        self.0.update(value);
        Ok(())
    }

    fn serialize_none(self) -> Result<(), HashError> {
        self.0.update(&[0]);
        Ok(())
    }

    fn serialize_some<T: Serialize + ?Sized>(self, value: &T) -> Result<(), HashError> {
        self.0.update(&[1]);
        value.serialize(self)
    }

    fn serialize_unit(self) -> Result<(), HashError> {
        Ok(())
    }

    fn serialize_unit_struct(self, _name: &'static str) -> Result<(), HashError> {
        Ok(())
    }

    fn serialize_unit_variant(
        self,
        _name: &'static str,
        index: u32,
        _variant: &'static str,
    ) -> Result<(), HashError> {
        self.variant(index);
        Ok(())
    }

    fn serialize_newtype_struct<T: Serialize + ?Sized>(
        self,
        _name: &'static str,
        value: &T,
    ) -> Result<(), HashError> {
        value.serialize(self)
    }

    fn serialize_newtype_variant<T: Serialize + ?Sized>(
        self,
        _name: &'static str,
        index: u32,
        _variant: &'static str,
        value: &T,
    ) -> Result<(), HashError> {
        self.variant(index);
        value.serialize(self)
    }

    fn serialize_seq(self, length: Option<usize>) -> Result<Self, HashError> {
        self.length(length)?;
        Ok(self)
    }

    fn serialize_tuple(self, _length: usize) -> Result<Self, HashError> {
        Ok(self)
    }

    fn serialize_tuple_struct(
        self,
        _name: &'static str,
        _length: usize,
    ) -> Result<Self, HashError> {
        Ok(self)
    }

    fn serialize_tuple_variant(
        self,
        _name: &'static str,
        index: u32,
        _variant: &'static str,
        _length: usize,
    ) -> Result<Self, HashError> {
        self.variant(index);
        Ok(self)
    }

    fn serialize_map(self, length: Option<usize>) -> Result<Self, HashError> {
        self.length(length)?;
        Ok(self)
    }

    fn serialize_struct(self, _name: &'static str, _length: usize) -> Result<Self, HashError> {
        Ok(self)
    }

    fn serialize_struct_variant(
        self,
        _name: &'static str,
        index: u32,
        _variant: &'static str,
        _length: usize,
    ) -> Result<Self, HashError> {
        self.variant(index);
        Ok(self)
    }
}

impl ser::SerializeSeq for &mut HashSerializer {
    type Ok = ();
    type Error = HashError;

    fn serialize_element<T: Serialize + ?Sized>(&mut self, value: &T) -> Result<(), HashError> {
        value.serialize(&mut **self)
    }

    fn end(self) -> Result<(), HashError> {
        Ok(())
    }
}

impl ser::SerializeTuple for &mut HashSerializer {
    type Ok = ();
    type Error = HashError;

    fn serialize_element<T: Serialize + ?Sized>(&mut self, value: &T) -> Result<(), HashError> {
        value.serialize(&mut **self)
    }

    fn end(self) -> Result<(), HashError> {
        Ok(())
    }
}

impl ser::SerializeTupleStruct for &mut HashSerializer {
    type Ok = ();
    type Error = HashError;

    fn serialize_field<T: Serialize + ?Sized>(&mut self, value: &T) -> Result<(), HashError> {
        value.serialize(&mut **self)
    }

    fn end(self) -> Result<(), HashError> {
        Ok(())
    }
}

impl ser::SerializeTupleVariant for &mut HashSerializer {
    type Ok = ();
    type Error = HashError;

    fn serialize_field<T: Serialize + ?Sized>(&mut self, value: &T) -> Result<(), HashError> {
        value.serialize(&mut **self)
    }

    fn end(self) -> Result<(), HashError> {
        Ok(())
    }
}

impl ser::SerializeMap for &mut HashSerializer {
    type Ok = ();
    type Error = HashError;

    fn serialize_key<T: Serialize + ?Sized>(&mut self, key: &T) -> Result<(), HashError> {
        key.serialize(&mut **self)
    }

    fn serialize_value<T: Serialize + ?Sized>(&mut self, value: &T) -> Result<(), HashError> {
        value.serialize(&mut **self)
    }

    fn end(self) -> Result<(), HashError> {
        Ok(())
    }
}

impl ser::SerializeStruct for &mut HashSerializer {
    type Ok = ();
    type Error = HashError;

    fn serialize_field<T: Serialize + ?Sized>(
        &mut self,
        _key: &'static str,
        value: &T,
    ) -> Result<(), HashError> {
        value.serialize(&mut **self)
    }

    fn end(self) -> Result<(), HashError> {
        Ok(())
    }
}

impl ser::SerializeStructVariant for &mut HashSerializer {
    type Ok = ();
    type Error = HashError;

    fn serialize_field<T: Serialize + ?Sized>(
        &mut self,
        _key: &'static str,
        value: &T,
    ) -> Result<(), HashError> {
        value.serialize(&mut **self)
    }

    fn end(self) -> Result<(), HashError> {
        Ok(())
    }
}
