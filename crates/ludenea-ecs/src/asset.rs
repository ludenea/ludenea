use std::cmp::Ordering;
use std::fmt;
use std::marker::PhantomData;

use ludenea_schema::{Schema, TypeSchema};
use serde::de::{self, DeserializeOwned, Visitor};
use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// Read-only simulation data loaded by identifier, shared by every handle to it. `NAME` is
/// its stable identifier; derive it with `#[derive(Asset)]`.
///
/// Snapshots encode assets with postcard, which cannot decode `#[serde(flatten)]` or
/// `#[serde(skip_serializing_if = "...")]`: asset types must not use them.
pub trait Asset: Schema + Serialize + DeserializeOwned + Send + Sync + 'static {
    const NAME: &'static str;
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
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(self)
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
