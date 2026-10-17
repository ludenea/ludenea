use std::any::{Any, type_name};

use serde::Serialize;
use serde::de::DeserializeOwned;

/// Per-entity simulation data. `NAME` is the stable identifier used for registration,
/// snapshots, and scene files; derive it with `#[derive(Component)]`.
///
/// Snapshots encode components with postcard, which cannot decode `#[serde(flatten)]` or
/// `#[serde(skip_serializing_if = "...")]`: component types must not use them.
pub trait Component: Serialize + DeserializeOwned + Send + 'static {
    const NAME: &'static str;
}

/// Global simulation data, one value per type. `NAME` is its stable identifier;
/// derive it with `#[derive(Resource)]`.
///
/// Snapshots encode resources with postcard, which cannot decode `#[serde(flatten)]` or
/// `#[serde(skip_serializing_if = "...")]`: resource types must not use them.
pub trait Resource: Serialize + DeserializeOwned + Send + 'static {
    const NAME: &'static str;
}

/// A decoded value boxed as `dyn Any`, or the reason decoding failed.
pub type DecodeResult = Result<Box<dyn Any + Send>, String>;

/// Type-erased serialization of one registered component or resource type. Every function
/// taking `&dyn Any` expects a value of that type and panics on any other.
#[derive(Clone, Copy)]
pub struct Codec {
    /// Encodes a value to postcard bytes.
    pub encode: fn(&dyn Any) -> Vec<u8>,
    /// Decodes postcard bytes into a boxed value.
    pub decode: fn(&[u8]) -> DecodeResult,
    /// Hashes a value with `hash_value`.
    pub hash: fn(&dyn Any) -> u64,
    /// Encodes a value to a single-line RON string.
    pub encode_ron: fn(&dyn Any) -> String,
    /// Decodes a RON string into a boxed value, through the concrete type so enum
    /// variants and `f32` values survive exactly.
    pub decode_ron: fn(&str) -> DecodeResult,
}

impl Codec {
    pub(crate) fn of<T: Serialize + DeserializeOwned + Send + 'static>() -> Self {
        Self {
            encode: |value| {
                postcard::to_allocvec(typed::<T>(value))
                    .unwrap_or_else(|error| panic!("cannot encode {}: {error}", type_name::<T>()))
            },
            decode: |bytes| Ok(Box::new(decode_exact::<T>(bytes)?)),
            hash: |value| crate::hash_value(typed::<T>(value)),
            encode_ron: |value| {
                ron::to_string(typed::<T>(value))
                    .unwrap_or_else(|error| panic!("cannot encode {}: {error}", type_name::<T>()))
            },
            decode_ron: |text| {
                let value: T = ron::from_str(text).map_err(|error| error.to_string())?;
                Ok(Box::new(value))
            },
        }
    }
}

/// Decodes postcard bytes, rejecting any bytes left unread after the value.
pub(crate) fn decode_exact<T: DeserializeOwned>(bytes: &[u8]) -> Result<T, String> {
    let (value, rest) = postcard::take_from_bytes(bytes).map_err(|error| error.to_string())?;
    if rest.is_empty() {
        Ok(value)
    } else {
        Err(format!("{} unexpected trailing bytes", rest.len()))
    }
}

fn typed<T: 'static>(value: &dyn Any) -> &T {
    value
        .downcast_ref::<T>()
        .unwrap_or_else(|| panic!("codec of {} received another type", type_name::<T>()))
}
