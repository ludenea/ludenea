use std::cmp::Ordering;
use std::fmt;
use std::marker::PhantomData;

use ludenea_schema::{Schema, TypeSchema};
use serde::de::{self, DeserializeOwned, Visitor};
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::World;

/// Read-only simulation data loaded by identifier, shared by every handle to it. `NAME` is
/// its stable identifier; derive it with `#[derive(Asset)]`, or, to give `validate`,
/// derive `Schema` and implement it by hand.
///
/// Snapshots encode assets with postcard, which cannot decode `#[serde(flatten)]` or
/// `#[serde(skip_serializing_if = "...")]`: asset types must not use them.
pub trait Asset: Schema + Serialize + DeserializeOwned + Send + Sync + 'static {
    const NAME: &'static str;

    /// Checks a decoded value before it enters `world` (`World::insert_asset`,
    /// `World::insert_asset_ron`, `World::restore`), so systems never meet a value the
    /// world cannot use; the error says why. Every value is accepted by default.
    fn validate(&self, world: &World) -> Result<(), String> {
        let _ = world;
        Ok(())
    }
}

/// A typed reference to an asset of type `T`, serialized exactly as its `AssetId`.
pub struct Handle<T: Asset> {
    pub id: AssetId,
    asset: PhantomData<fn() -> T>,
}

impl<T: Asset> Handle<T> {
    pub fn new(id: AssetId) -> Self {
        Self {
            id,
            asset: PhantomData,
        }
    }
}

// Implemented by hand: derives would require `T` itself to implement each trait.
impl<T: Asset> Clone for Handle<T> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<T: Asset> Copy for Handle<T> {}

impl<T: Asset> PartialEq for Handle<T> {
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id
    }
}

impl<T: Asset> Eq for Handle<T> {}

impl<T: Asset> PartialOrd for Handle<T> {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl<T: Asset> Ord for Handle<T> {
    fn cmp(&self, other: &Self) -> Ordering {
        self.id.cmp(&other.id)
    }
}

impl<T: Asset> fmt::Debug for Handle<T> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "Handle<{}>({})", T::NAME, self.id)
    }
}

impl<T: Asset> Serialize for Handle<T> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.id.serialize(serializer)
    }
}

impl<'de, T: Asset> Deserialize<'de> for Handle<T> {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        AssetId::deserialize(deserializer).map(Self::new)
    }
}

impl<T: Asset> Schema for Handle<T> {
    fn schema() -> TypeSchema {
        TypeSchema::Handle {
            asset: T::NAME.to_string(),
        }
    }
}

/// The identifier of an asset, serialized as 32 lowercase hexadecimal digits.
#[derive(Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(type = "string"))]
pub struct AssetId(pub u128);

impl fmt::Display for AssetId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{:032x}", self.0)
    }
}

impl Serialize for AssetId {
    /// The text `Display` writes, encoded by hand: formatting it digit by digit cost a fifth
    /// of a tick's world hash in a scene of 10,000 sprites.
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        const DIGITS: &[u8; 16] = b"0123456789abcdef";
        let mut text = [0u8; 32];
        for (index, digit) in text.iter_mut().enumerate() {
            *digit = DIGITS[((self.0 >> (124 - 4 * index)) & 0xf) as usize];
        }
        serializer.serialize_str(std::str::from_utf8(&text).expect("ASCII digits"))
    }
}

impl<'de> Deserialize<'de> for AssetId {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct HexVisitor;

        impl Visitor<'_> for HexVisitor {
            type Value = AssetId;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("32 lowercase hexadecimal digits")
            }

            fn visit_str<E: de::Error>(self, text: &str) -> Result<AssetId, E> {
                let lowercase_hex = |c: char| c.is_ascii_digit() || ('a'..='f').contains(&c);
                if text.len() != 32 || !text.chars().all(lowercase_hex) {
                    return Err(E::invalid_value(de::Unexpected::Str(text), &self));
                }
                u128::from_str_radix(text, 16)
                    .map(AssetId)
                    .map_err(|_| E::invalid_value(de::Unexpected::Str(text), &self))
            }
        }

        deserializer.deserialize_str(HexVisitor)
    }
}

impl Schema for AssetId {
    fn schema() -> TypeSchema {
        TypeSchema::AssetId
    }
}
