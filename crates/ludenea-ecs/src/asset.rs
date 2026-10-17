use std::fmt;

use ludenea_schema::{Schema, TypeSchema};
use serde::de::{self, Visitor};
use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// The identifier of an asset, serialized as 32 lowercase hexadecimal digits.
#[derive(Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
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
