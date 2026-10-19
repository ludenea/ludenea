//! The wire form of component and resource values: JSON in serde's externally tagged
//! form, where integers outside the range JavaScript numbers hold exactly travel as
//! decimal strings, located by the type schema.

use ludenea_ecs::schema::{FieldSchema, TypeSchema, VariantFields};
use serde_json::{Map, Number, Value as JsonValue};

/// `Number.MAX_SAFE_INTEGER`: larger integers lose precision as JavaScript numbers.
const MAX_SAFE_INTEGER: u64 = (1 << 53) - 1;

/// Turns the 64-bit integers of `value` that JavaScript cannot hold exactly into decimal
/// strings: those whose magnitude exceeds `2^53 - 1` (`Number.MAX_SAFE_INTEGER`), so
/// `2^53 - 1` itself stays a number. Clients follow the same rule when they send values
/// (see `ControlClient` in the TypeScript control client).
pub fn to_wire(mut value: JsonValue, schema: &TypeSchema) -> JsonValue {
    convert(&mut value, schema, &mut |integer, _| {
        if let JsonValue::Number(number) = integer
            && !is_safe(number)
        {
            *integer = JsonValue::String(number.to_string());
        }
        Ok(())
    })
    .expect("converting to the wire form never fails");
    value
}

/// Parses back the decimal strings `to_wire` makes; integers sent as numbers stay.
pub fn from_wire(mut value: JsonValue, schema: &TypeSchema) -> Result<JsonValue, String> {
    convert(&mut value, schema, &mut |integer, schema| {
        if let JsonValue::String(text) = integer {
            let number = match schema {
                TypeSchema::U64 => text.parse::<u64>().map(Number::from).ok(),
                _ => text.parse::<i64>().map(Number::from).ok(),
            };
            *integer = JsonValue::Number(number.ok_or_else(|| {
                format!("`{text}` is not a whole number; write the integer in decimal digits")
            })?);
        }
        Ok(())
    })?;
    Ok(value)
}

/// Like `to_wire` for a value without a schema (an event): every integer JavaScript cannot
/// hold exactly becomes a decimal string.
pub fn untyped_to_wire(mut value: JsonValue) -> JsonValue {
    fn convert(value: &mut JsonValue) {
        match value {
            JsonValue::Number(number) if !is_safe(number) => {
                *value = JsonValue::String(number.to_string());
            }
            JsonValue::Array(values) => values.iter_mut().for_each(convert),
            JsonValue::Object(map) => map.values_mut().for_each(convert),
            _ => {}
        }
    }
    convert(&mut value);
    value
}

fn is_safe(number: &Number) -> bool {
    match (number.as_u64(), number.as_i64()) {
        (Some(unsigned), _) => unsigned <= MAX_SAFE_INTEGER,
        (None, Some(signed)) => signed.unsigned_abs() <= MAX_SAFE_INTEGER,
        _ => true,
    }
}

type Integer<'a> = dyn FnMut(&mut JsonValue, &TypeSchema) -> Result<(), String> + 'a;

/// Calls `integer` on every value at a `u64` or `i64` position of `schema`. Parts of
/// `value` that do not match the schema are left as they are, for decoding to reject.
fn convert(
    value: &mut JsonValue,
    schema: &TypeSchema,
    integer: &mut Integer,
) -> Result<(), String> {
    match (schema, value) {
        (TypeSchema::U64 | TypeSchema::I64, value) => integer(value, schema),
        (TypeSchema::Option { item }, value) => convert(value, item, integer),
        (TypeSchema::Newtype { item, .. }, value) => convert(value, item, integer),
        (TypeSchema::List { item } | TypeSchema::Array { item, .. }, JsonValue::Array(values)) => {
            values
                .iter_mut()
                .try_for_each(|value| convert(value, item, integer))
        }
        (
            TypeSchema::Tuple { items } | TypeSchema::TupleStruct { items, .. },
            JsonValue::Array(values),
        ) => convert_items(values, items, integer),
        (TypeSchema::Struct { fields, .. }, JsonValue::Object(map)) => {
            convert_fields(map, fields, integer)
        }
        (TypeSchema::Map { value: item, .. }, JsonValue::Object(map)) => map
            .values_mut()
            .try_for_each(|value| convert(value, item, integer)),
        (TypeSchema::Enum { variants, .. }, JsonValue::Object(map)) => {
            for (name, content) in map.iter_mut() {
                let Some(variant) = variants.iter().find(|variant| variant.name == *name) else {
                    continue;
                };
                match (&variant.fields, content) {
                    (VariantFields::Newtype { item }, content) => convert(content, item, integer)?,
                    (VariantFields::Tuple { items }, JsonValue::Array(values)) => {
                        convert_items(values, items, integer)?;
                    }
                    (VariantFields::Struct { fields }, JsonValue::Object(map)) => {
                        convert_fields(map, fields, integer)?;
                    }
                    _ => {}
                }
            }
            Ok(())
        }
        _ => Ok(()),
    }
}

fn convert_items(
    values: &mut [JsonValue],
    items: &[TypeSchema],
    integer: &mut Integer,
) -> Result<(), String> {
    values
        .iter_mut()
        .zip(items)
        .try_for_each(|(value, item)| convert(value, item, integer))
}

fn convert_fields(
    map: &mut Map<String, JsonValue>,
    fields: &[FieldSchema],
    integer: &mut Integer,
) -> Result<(), String> {
    for field in fields {
        if let Some(value) = map.get_mut(&field.name) {
            convert(value, &field.schema, integer)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use ludenea_ecs::schema::{Schema, VariantSchema};
    use serde_json::json;

    fn schema() -> TypeSchema {
        let field = |name: &str, schema| FieldSchema {
            name: name.to_string(),
            schema,
        };
        TypeSchema::Struct {
            name: "test::Value".to_string(),
            fields: vec![
                field("big", TypeSchema::U64),
                field("small", TypeSchema::U64),
                field("negative", TypeSchema::I64),
                field("list", Vec::<Option<u64>>::schema()),
                field(
                    "choice",
                    TypeSchema::Enum {
                        name: "test::Choice".to_string(),
                        variants: vec![VariantSchema {
                            name: "Pair".to_string(),
                            fields: VariantFields::Tuple {
                                items: vec![TypeSchema::U32, TypeSchema::I64],
                            },
                        }],
                    },
                ),
            ],
        }
    }

    #[test]
    fn large_integers_become_strings_and_come_back() {
        let native = json!({
            "big": u64::MAX,
            "small": MAX_SAFE_INTEGER,
            "negative": -(1_i64 << 60),
            "list": [null, MAX_SAFE_INTEGER + 1],
            "choice": {"Pair": [7, i64::MIN]},
        });
        let wire = to_wire(native.clone(), &schema());
        assert_eq!(
            wire,
            json!({
                "big": u64::MAX.to_string(),
                "small": MAX_SAFE_INTEGER,
                "negative": (-(1_i64 << 60)).to_string(),
                "list": [null, (MAX_SAFE_INTEGER + 1).to_string()],
                "choice": {"Pair": [7, i64::MIN.to_string()]},
            })
        );
        assert_eq!(from_wire(wire, &schema()).unwrap(), native);
    }

    #[test]
    fn untyped_large_integers_become_strings() {
        let wire =
            untyped_to_wire(json!({"a": [MAX_SAFE_INTEGER, u64::MAX], "b": i64::MIN, "c": 1.5}));
        assert_eq!(
            wire,
            json!({"a": [MAX_SAFE_INTEGER, u64::MAX.to_string()], "b": i64::MIN.to_string(), "c": 1.5})
        );
    }

    #[test]
    fn rejects_a_string_that_is_not_an_integer() {
        let error = from_wire(json!({"big": "12x"}), &schema()).unwrap_err();
        assert!(error.contains("12x"), "{error}");
    }
}
