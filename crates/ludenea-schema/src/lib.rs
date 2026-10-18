//! Field schemas: a serializable description of the serde shape of a type, derived with
//! `#[derive(Schema)]`. The editor reads them to build forms and TypeScript types.

extern crate self as ludenea_schema;

pub mod entities;
mod typescript;

use std::collections::{BTreeMap, BTreeSet};

pub use ludenea_macros::Schema;
use serde::{Deserialize, Serialize};
pub use typescript::typescript_declarations;

/// The serde shape of a type.
///
/// `schema()` recurses through field types, so a recursive type (a struct holding a `Vec`
/// of itself) overflows the stack: do not derive `Schema` for one.
pub trait Schema {
    fn schema() -> TypeSchema;
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum TypeSchema {
    Bool,
    U8,
    U16,
    U32,
    U64,
    I8,
    I16,
    I32,
    I64,
    F32,
    F64,
    String,
    Unit,
    Option {
        item: Box<TypeSchema>,
    },
    List {
        item: Box<TypeSchema>,
    },
    Array {
        item: Box<TypeSchema>,
        len: u32,
    },
    Map {
        key: Box<TypeSchema>,
        value: Box<TypeSchema>,
    },
    Tuple {
        items: Vec<TypeSchema>,
    },
    Struct {
        name: String,
        fields: Vec<FieldSchema>,
    },
    /// A one-field tuple struct, which serde serializes as its bare inner value.
    Newtype {
        name: String,
        item: Box<TypeSchema>,
    },
    TupleStruct {
        name: String,
        items: Vec<TypeSchema>,
    },
    UnitStruct {
        name: String,
    },
    Enum {
        name: String,
        variants: Vec<VariantSchema>,
    },
    Entity,
    AssetId,
    /// A typed asset handle, serialized as its `AssetId`; `asset` is the stable name of
    /// the asset type.
    Handle {
        asset: String,
    },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct FieldSchema {
    pub name: String,
    pub schema: TypeSchema,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct VariantSchema {
    pub name: String,
    pub fields: VariantFields,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum VariantFields {
    Unit,
    Newtype { item: Box<TypeSchema> },
    Tuple { items: Vec<TypeSchema> },
    Struct { fields: Vec<FieldSchema> },
}

macro_rules! primitive {
    ($($ty:ty => $variant:ident),* $(,)?) => {
        $(impl Schema for $ty {
            fn schema() -> TypeSchema {
                TypeSchema::$variant
            }
        })*
    };
}

primitive! {
    bool => Bool, u8 => U8, u16 => U16, u32 => U32, u64 => U64, usize => U64,
    i8 => I8, i16 => I16, i32 => I32, i64 => I64, f32 => F32, f64 => F64,
    String => String, () => Unit,
}

impl<T: Schema> Schema for Option<T> {
    fn schema() -> TypeSchema {
        TypeSchema::Option {
            item: Box::new(T::schema()),
        }
    }
}

impl<T: Schema> Schema for Vec<T> {
    fn schema() -> TypeSchema {
        TypeSchema::List {
            item: Box::new(T::schema()),
        }
    }
}

impl<T: Schema> Schema for BTreeSet<T> {
    fn schema() -> TypeSchema {
        TypeSchema::List {
            item: Box::new(T::schema()),
        }
    }
}

impl<T: Schema, const N: usize> Schema for [T; N] {
    fn schema() -> TypeSchema {
        TypeSchema::Array {
            item: Box::new(T::schema()),
            len: u32::try_from(N).expect("array length fits in u32"),
        }
    }
}

impl<K: Schema, V: Schema> Schema for BTreeMap<K, V> {
    fn schema() -> TypeSchema {
        TypeSchema::Map {
            key: Box::new(K::schema()),
            value: Box::new(V::schema()),
        }
    }
}

macro_rules! tuple {
    ($($name:ident),+) => {
        impl<$($name: Schema),+> Schema for ($($name,)+) {
            fn schema() -> TypeSchema {
                TypeSchema::Tuple { items: vec![$($name::schema()),+] }
            }
        }
    };
}

tuple!(A);
tuple!(A, B);
tuple!(A, B, C);
tuple!(A, B, C, D);

/// A derive on a container with `#[serde(rename_all = "camelCase")]` is rejected, so a
/// schema never disagrees with the serde shape.
///
/// ```compile_fail
/// use ludenea_schema::Schema;
/// use serde::{Deserialize, Serialize};
///
/// #[derive(Schema, Serialize, Deserialize)]
/// #[serde(rename_all = "camelCase")]
/// struct Camel {
///     field_name: u8,
/// }
/// ```
///
/// Likewise a skipped field.
///
/// ```compile_fail
/// use ludenea_schema::Schema;
/// use serde::{Deserialize, Serialize};
///
/// #[derive(Schema, Serialize, Deserialize)]
/// struct Skipped {
///     #[serde(skip)]
///     hidden: u8,
/// }
/// ```
///
/// Likewise every other shape-changing attribute, such as `transparent` on a container or
/// `skip_serializing_if` on a field.
///
/// ```compile_fail
/// use ludenea_schema::Schema;
/// use serde::{Deserialize, Serialize};
///
/// #[derive(Schema, Serialize, Deserialize)]
/// #[serde(transparent)]
/// struct Transparent {
///     inner: u8,
/// }
/// ```
///
/// ```compile_fail
/// use ludenea_schema::Schema;
/// use serde::{Deserialize, Serialize};
///
/// #[derive(Schema, Serialize, Deserialize)]
/// struct Optional {
///     #[serde(skip_serializing_if = "Option::is_none")]
///     value: Option<u8>,
/// }
/// ```
///
/// ```compile_fail
/// use ludenea_schema::Schema;
/// use serde::{Deserialize, Serialize};
///
/// #[derive(Schema, Serialize, Deserialize)]
/// #[serde(rename_all_fields = "camelCase")]
/// enum Fields {
///     Variant { field_name: u8 },
/// }
/// ```
///
/// Likewise custom serialization of one field, either way.
///
/// ```compile_fail
/// use ludenea_schema::Schema;
/// use serde::{Deserialize, Serialize, Serializer};
///
/// fn as_zero<S: Serializer>(_: &u8, serializer: S) -> Result<S::Ok, S::Error> {
///     serializer.serialize_u8(0)
/// }
///
/// #[derive(Schema, Serialize, Deserialize)]
/// struct Custom {
///     #[serde(serialize_with = "as_zero")]
///     value: u8,
/// }
/// ```
///
/// ```compile_fail
/// use ludenea_schema::Schema;
/// use serde::{Deserialize, Deserializer, Serialize};
///
/// fn from_any<'de, D: Deserializer<'de>>(deserializer: D) -> Result<u8, D::Error> {
///     u8::deserialize(deserializer)
/// }
///
/// #[derive(Schema, Serialize, Deserialize)]
/// struct Custom {
///     #[serde(deserialize_with = "from_any")]
///     value: u8,
/// }
/// ```
///
/// A generic type is rejected too: all its instantiations would share one schema name.
///
/// ```compile_fail
/// use ludenea_schema::Schema;
///
/// #[derive(Schema)]
/// struct Wrapper<T> {
///     inner: T,
/// }
/// ```
#[cfg(doctest)]
struct _CompileFail;

#[cfg(test)]
#[allow(dead_code)]
mod tests {
    use super::*;

    #[derive(Schema)]
    struct Vec2 {
        x: f32,
        y: f32,
    }

    #[derive(Schema)]
    enum Shape {
        Dot,
        Circle(Vec2),
        Segment(Vec2, Vec2),
        Box { width: f32, height: f32 },
    }

    #[derive(Schema)]
    struct Meters(f32);

    #[derive(Schema)]
    struct Pair(u8, u8);

    #[derive(Schema)]
    struct Marker;

    fn name_of(schema: &TypeSchema) -> &str {
        match schema {
            TypeSchema::Struct { name, .. }
            | TypeSchema::Newtype { name, .. }
            | TypeSchema::TupleStruct { name, .. }
            | TypeSchema::UnitStruct { name }
            | TypeSchema::Enum { name, .. } => name,
            other => panic!("not a named type: {other:?}"),
        }
    }

    #[test]
    fn struct_schema_lists_fields_in_declaration_order() {
        let TypeSchema::Struct { name, fields } = Vec2::schema() else {
            panic!("expected a struct");
        };
        assert!(name.ends_with("::Vec2"), "{name}");
        let names: Vec<_> = fields.iter().map(|field| field.name.as_str()).collect();
        assert_eq!(names, ["x", "y"]);
        assert_eq!(fields[0].schema, TypeSchema::F32);
    }

    #[test]
    fn enum_schema_covers_unit_newtype_tuple_and_struct_variants() {
        let TypeSchema::Enum { variants, .. } = Shape::schema() else {
            panic!("expected an enum");
        };
        let fields: Vec<_> = variants.iter().map(|variant| &variant.fields).collect();
        assert_eq!(fields[0], &VariantFields::Unit);
        assert!(
            matches!(fields[1], VariantFields::Newtype { item } if name_of(item).ends_with("::Vec2"))
        );
        assert!(matches!(fields[2], VariantFields::Tuple { items } if items.len() == 2));
        assert!(matches!(fields[3], VariantFields::Struct { fields } if fields.len() == 2));
    }

    #[test]
    fn one_field_tuple_struct_is_newtype() {
        assert!(matches!(
            Meters::schema(),
            TypeSchema::Newtype { item, .. } if *item == TypeSchema::F32
        ));
        assert!(matches!(
            Pair::schema(),
            TypeSchema::TupleStruct { items, .. } if items.len() == 2
        ));
        assert!(matches!(Marker::schema(), TypeSchema::UnitStruct { .. }));
    }

    #[test]
    fn containers_map_to_option_list_array_map_tuple() {
        let boxed = |schema: TypeSchema| Box::new(schema);
        assert_eq!(
            Option::<u8>::schema(),
            TypeSchema::Option {
                item: boxed(TypeSchema::U8)
            }
        );
        assert_eq!(
            Vec::<String>::schema(),
            TypeSchema::List {
                item: boxed(TypeSchema::String)
            }
        );
        assert_eq!(
            <[Vec2; 8]>::schema(),
            TypeSchema::Array {
                item: boxed(Vec2::schema()),
                len: 8
            }
        );
        assert_eq!(
            BTreeMap::<u8, bool>::schema(),
            TypeSchema::Map {
                key: boxed(TypeSchema::U8),
                value: boxed(TypeSchema::Bool)
            }
        );
        assert_eq!(
            <(u8, ())>::schema(),
            TypeSchema::Tuple {
                items: vec![TypeSchema::U8, TypeSchema::Unit]
            }
        );
        assert_eq!(usize::schema(), TypeSchema::U64);
    }

    #[test]
    fn schema_serializes_with_a_kind_tag() {
        let json = serde_json::to_value(Option::<u8>::schema()).unwrap();
        assert_eq!(
            json,
            serde_json::json!({ "kind": "option", "item": { "kind": "u8" } })
        );
    }
}
