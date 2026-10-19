use std::any::{Any, type_name};
use std::cell::RefCell;
use std::fmt;

use ludenea_schema::{Schema, TypeSchema};
use serde::de::{DeserializeOwned, Visitor};
use serde::{Deserializer, Serialize};

use crate::Entity;

/// Per-entity simulation data. `NAME` is the stable identifier used for registration,
/// snapshots, and scene files; derive it with `#[derive(Component)]`.
///
/// Snapshots encode components with postcard, which cannot decode `#[serde(flatten)]` or
/// `#[serde(skip_serializing_if = "...")]`: component types must not use them.
pub trait Component: Schema + Serialize + DeserializeOwned + Send + 'static {
    const NAME: &'static str;
}

/// Global simulation data, one value per type. `NAME` is its stable identifier;
/// derive it with `#[derive(Resource)]`.
///
/// Snapshots encode resources with postcard, which cannot decode `#[serde(flatten)]` or
/// `#[serde(skip_serializing_if = "...")]`: resource types must not use them.
pub trait Resource: Schema + Serialize + DeserializeOwned + Send + 'static {
    const NAME: &'static str;
}

/// A decoded value boxed as `dyn Any`, or the reason decoding failed.
pub type DecodeResult = Result<Box<dyn Any + Send>, String>;

type RemapEntities = fn(&[u8], &dyn Fn(Entity) -> Entity) -> Vec<u8>;

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
    /// Encodes a value to JSON. Fails unless decoding the JSON back gives the same postcard
    /// bytes, which rejects non-finite floats and non-string map keys.
    pub to_json: fn(&dyn Any) -> Result<serde_json::Value, String>,
    /// Decodes JSON into a boxed value, through the concrete type so validation such as
    /// `try_from` runs.
    pub from_json: fn(serde_json::Value) -> DecodeResult,
    /// Describes the serde shape of the type.
    pub schema: fn() -> TypeSchema,
    /// Rewrites every `Entity` value inside postcard bytes through the map, decoding and
    /// re-encoding the concrete type so every other value (non-finite floats included)
    /// survives exactly.
    ///
    /// # Panics
    /// If the bytes do not decode.
    pub remap_entities: RemapEntities,
}

impl Codec {
    pub(crate) fn of<T: Schema + Serialize + DeserializeOwned + Send + 'static>() -> Self {
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
            to_json: |value| {
                let json = serde_json::to_value(typed::<T>(value)).map_err(|e| e.to_string())?;
                let back: T = serde_json::from_value(json.clone()).map_err(|e| e.to_string())?;
                if postcard::to_allocvec(&back).ok()
                    == postcard::to_allocvec(typed::<T>(value)).ok()
                {
                    Ok(json)
                } else {
                    Err("the value has no exact JSON form".to_string())
                }
            },
            from_json: |json| {
                let value: T = serde_json::from_value(json).map_err(|error| error.to_string())?;
                Ok(Box::new(value))
            },
            schema: T::schema,
            remap_entities: |bytes, map| {
                let Remap::Collect(seen) =
                    decode_remapping::<T>(bytes, Remap::Collect(Vec::new())).1
                else {
                    unreachable!("collecting stays collecting")
                };
                let mapped = seen.into_iter().map(map).collect::<Vec<_>>();
                let (value, _) = decode_remapping::<T>(bytes, Remap::Replace(mapped.into_iter()));
                postcard::to_allocvec(&value)
                    .unwrap_or_else(|error| panic!("cannot encode {}: {error}", type_name::<T>()))
            },
        }
    }
}

/// What `Entity` decoding does on this thread while `remap_entities` runs: the first
/// decode collects every entity in decode order, the second replaces them in that order.
enum Remap {
    Collect(Vec<Entity>),
    Replace(std::vec::IntoIter<Entity>),
}

thread_local! {
    static REMAP: RefCell<Option<Remap>> = const { RefCell::new(None) };
}

/// Called by `Entity`'s `Deserialize` on every decoded entity.
pub(crate) fn remap_decoded(entity: Entity) -> Entity {
    REMAP.with_borrow_mut(|remap| match remap {
        None => entity,
        Some(Remap::Collect(seen)) => {
            seen.push(entity);
            entity
        }
        Some(Remap::Replace(mapped)) => mapped.next().expect("the same bytes decode the same"),
    })
}

/// Decodes `bytes` with `remap` active, returning the value and the remap state after.
fn decode_remapping<T: DeserializeOwned>(bytes: &[u8], remap: Remap) -> (T, Remap) {
    struct Reset;

    impl Drop for Reset {
        fn drop(&mut self) {
            REMAP.set(None);
        }
    }

    let _reset = Reset;
    REMAP.set(Some(remap));
    let value = decode_exact::<T>(bytes)
        .unwrap_or_else(|error| panic!("cannot decode {}: {error}", type_name::<T>()));
    (value, REMAP.take().expect("set above"))
}

/// Decodes postcard bytes, rejecting any bytes left unread after the value. When the type
/// refuses the value, as a failing `try_from` does, the error is the type's own message.
pub fn decode_exact<T: DeserializeOwned>(bytes: &[u8]) -> Result<T, String> {
    let mut deserializer = postcard::Deserializer::from_bytes(bytes);
    let value = T::deserialize(KeepMessage(&mut deserializer)).map_err(|error| error.0)?;
    let rest = deserializer.finalize().map_err(|error| error.to_string())?;
    if rest.is_empty() {
        Ok(value)
    } else {
        Err(format!("{} unexpected trailing bytes", rest.len()))
    }
}

/// The postcard deserializer with an error that keeps the message of a custom error, which
/// postcard replaces with "Serde Deserialization Error".
// ponytail: only the outermost type's own message is kept, since the values nested in it
// decode through postcard directly; wrap the visitors and accesses too if a nested type
// has to name why it refuses a value.
struct KeepMessage<'a, 'de>(&'a mut postcard::Deserializer<'de, postcard::de_flavors::Slice<'de>>);

#[derive(Debug)]
struct Message(String);

impl fmt::Display for Message {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl std::error::Error for Message {}

impl serde::de::Error for Message {
    fn custom<T: fmt::Display>(message: T) -> Self {
        Message(message.to_string())
    }
}

macro_rules! forward_to_postcard {
    ($($method:ident($($argument:ident: $type:ty),*);)*) => {$(
        fn $method<V: Visitor<'de>>(
            self,
            $($argument: $type,)*
            visitor: V,
        ) -> Result<V::Value, Message> {
            self.0
                .$method($($argument,)* visitor)
                .map_err(|error| Message(error.to_string()))
        }
    )*};
}

impl<'de> Deserializer<'de> for KeepMessage<'_, 'de> {
    type Error = Message;

    forward_to_postcard! {
        deserialize_any();
        deserialize_bool();
        deserialize_i8();
        deserialize_i16();
        deserialize_i32();
        deserialize_i64();
        deserialize_i128();
        deserialize_u8();
        deserialize_u16();
        deserialize_u32();
        deserialize_u64();
        deserialize_u128();
        deserialize_f32();
        deserialize_f64();
        deserialize_char();
        deserialize_str();
        deserialize_string();
        deserialize_bytes();
        deserialize_byte_buf();
        deserialize_option();
        deserialize_unit();
        deserialize_unit_struct(name: &'static str);
        deserialize_newtype_struct(name: &'static str);
        deserialize_seq();
        deserialize_tuple(len: usize);
        deserialize_tuple_struct(name: &'static str, len: usize);
        deserialize_map();
        deserialize_struct(name: &'static str, fields: &'static [&'static str]);
        deserialize_enum(name: &'static str, variants: &'static [&'static str]);
        deserialize_identifier();
        deserialize_ignored_any();
    }

    fn is_human_readable(&self) -> bool {
        self.0.is_human_readable()
    }
}

fn typed<T: 'static>(value: &dyn Any) -> &T {
    value
        .downcast_ref::<T>()
        .unwrap_or_else(|| panic!("codec of {} received another type", type_name::<T>()))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A decode that panics while replacing entities leaves no remap behind: a later plain
    /// decode on this thread keeps its entity.
    #[test]
    fn a_panic_while_replacing_clears_the_remap() {
        let other: Entity = postcard::from_bytes(&[9, 0]).unwrap();
        // (Entity 1v0, 7), then a trailing byte that fails the exact decode.
        let bytes = [1, 0, 7, 0];
        let panicked = std::panic::catch_unwind(|| {
            decode_remapping::<(Entity, u8)>(&bytes, Remap::Replace(vec![other].into_iter()))
        });
        assert!(panicked.is_err());
        assert!(REMAP.with_borrow(Option::is_none));
        let plain: (Entity, u8) = postcard::from_bytes(&bytes[..3]).unwrap();
        assert_eq!((plain.0.index(), plain.1), (1, 7));
    }
}
