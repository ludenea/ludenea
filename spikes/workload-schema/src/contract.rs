//! The contract description and its encodings. Throwaway, never product code.
//!
//! A description is data: the named structure and enumeration types, the
//! component types with the operations they admit, the globals, and the
//! commands, queries and events, each with a stable name and a version. Values
//! are encoded by their type in a canonical byte order (little-endian, fields
//! in description order, a count before a sequence, a presence byte before an
//! optional). The description itself is encoded in a fixed bootstrap format,
//! the only thing a client must know before it reads anything: this module is
//! that format, and it names nothing of the storage behind the values.

use std::fmt;

/// The format of the encoded description, bumped when this module's bootstrap
/// encoding changes (a format change, never a schema change).
pub const DESCRIPTION_FORMAT: u32 = 1;

/// A type of the contract's one type system.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Type {
    Bool,
    U8,
    U16,
    U32,
    U64,
    I32,
    I64,
    F32,
    F64,
    String,
    Handle,
    /// A structure type, by its stable name in the description.
    Struct(String),
    /// An enumeration type, by its stable name in the description.
    Enum(String),
    Sequence(Box<Type>),
    Optional(Box<Type>),
}

/// An entity reference: an index and a generation, never an address.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Handle {
    pub index: u32,
    pub generation: u32,
}

/// A value of the type system, as a tree. Small messages and defaults use it;
/// bulk state is read through a [`Reader`] without building a tree.
#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    Bool(bool),
    U8(u8),
    U16(u16),
    U32(u32),
    U64(u64),
    I32(i32),
    I64(i64),
    F32(f32),
    F64(f64),
    String(String),
    Handle(Handle),
    /// The fields in description order.
    Struct(Vec<Value>),
    /// The variant's index in the enumeration's description.
    Enum(u32),
    Sequence(Vec<Value>),
    Optional(Option<Box<Value>>),
}

#[derive(Clone, Debug, PartialEq)]
pub struct Field {
    pub name: String,
    pub ty: Type,
    /// A field with a default may be absent from a client's message and is
    /// filled with it; a field added with a default is a compatible change.
    pub default: Option<Value>,
    pub doc: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct StructType {
    pub name: String,
    pub version: u32,
    pub fields: Vec<Field>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct EnumType {
    pub name: String,
    pub version: u32,
    pub variants: Vec<String>,
}

/// A component type: a structure type entities carry, with the structural
/// operations it admits, described for authoring parity.
#[derive(Clone, Debug, PartialEq)]
pub struct Component {
    pub name: String,
    pub operations: Vec<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Global {
    pub name: String,
    pub ty: Type,
    pub doc: String,
}

/// A command or a query: a stable name, a version, a parameter structure and
/// a result structure.
#[derive(Clone, Debug, PartialEq)]
pub struct Operation {
    pub name: String,
    pub version: u32,
    pub parameters: String,
    pub result: String,
    pub doc: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Event {
    pub name: String,
    pub version: u32,
    pub payload: String,
    pub doc: String,
}

#[derive(Clone, Debug, PartialEq, Default)]
pub struct Description {
    pub engine: String,
    pub schema_version: u32,
    pub structs: Vec<StructType>,
    pub enums: Vec<EnumType>,
    pub components: Vec<Component>,
    pub globals: Vec<Global>,
    pub commands: Vec<Operation>,
    pub queries: Vec<Operation>,
    pub events: Vec<Event>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DecodeError(pub String);

impl fmt::Display for DecodeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "decode error: {}", self.0)
    }
}

impl std::error::Error for DecodeError {}

fn error<T>(message: impl Into<String>) -> Result<T, DecodeError> {
    Err(DecodeError(message.into()))
}

impl Description {
    pub fn structure(&self, name: &str) -> Result<&StructType, DecodeError> {
        match self.structs.iter().find(|s| s.name == name) {
            Some(s) => Ok(s),
            None => error(format!("unknown structure type `{name}`")),
        }
    }

    pub fn enumeration(&self, name: &str) -> Result<&EnumType, DecodeError> {
        match self.enums.iter().find(|e| e.name == name) {
            Some(e) => Ok(e),
            None => error(format!("unknown enumeration type `{name}`")),
        }
    }

    /// The description as bytes, in the bootstrap format.
    pub fn encode(&self) -> Vec<u8> {
        let mut w = Writer::new();
        w.bytes(b"LDCD");
        w.u32(DESCRIPTION_FORMAT);
        w.string(&self.engine);
        w.u32(self.schema_version);
        w.u32(self.structs.len() as u32);
        for s in &self.structs {
            w.string(&s.name);
            w.u32(s.version);
            w.u32(s.fields.len() as u32);
            for field in &s.fields {
                w.string(&field.name);
                encode_type(&mut w, &field.ty);
                match &field.default {
                    None => w.u8(0),
                    Some(value) => {
                        w.u8(1);
                        encode_value(&mut w, value, &field.ty, self).expect("a default matches its field's type");
                    }
                }
                w.string(&field.doc);
            }
        }
        w.u32(self.enums.len() as u32);
        for e in &self.enums {
            w.string(&e.name);
            w.u32(e.version);
            w.u32(e.variants.len() as u32);
            for variant in &e.variants {
                w.string(variant);
            }
        }
        w.u32(self.components.len() as u32);
        for c in &self.components {
            w.string(&c.name);
            w.u32(c.operations.len() as u32);
            for operation in &c.operations {
                w.string(operation);
            }
        }
        w.u32(self.globals.len() as u32);
        for g in &self.globals {
            w.string(&g.name);
            encode_type(&mut w, &g.ty);
            w.string(&g.doc);
        }
        for operations in [&self.commands, &self.queries] {
            w.u32(operations.len() as u32);
            for o in operations {
                w.string(&o.name);
                w.u32(o.version);
                w.string(&o.parameters);
                w.string(&o.result);
                w.string(&o.doc);
            }
        }
        w.u32(self.events.len() as u32);
        for e in &self.events {
            w.string(&e.name);
            w.u32(e.version);
            w.string(&e.payload);
            w.string(&e.doc);
        }
        w.finish()
    }

    /// A description from its bytes. Defaults are decoded after every type is
    /// known, since a default may be a structure value.
    pub fn decode(bytes: &[u8]) -> Result<Description, DecodeError> {
        let mut r = Reader::new(bytes);
        if r.take(4)? != b"LDCD" {
            return error("not a contract description");
        }
        let format = r.u32()?;
        if format != DESCRIPTION_FORMAT {
            return error(format!("description format {format}, this reader knows {DESCRIPTION_FORMAT}"));
        }
        let mut description = Description { engine: r.string()?, schema_version: r.u32()?, ..Description::default() };
        // The defaults' bytes are kept aside and decoded once the types are known.
        let mut pending_defaults: Vec<(usize, usize, Vec<u8>)> = Vec::new();
        let struct_count = r.u32()? as usize;
        for struct_index in 0..struct_count {
            let name = r.string()?;
            let version = r.u32()?;
            let field_count = r.u32()? as usize;
            let mut fields = Vec::with_capacity(field_count);
            for field_index in 0..field_count {
                let field_name = r.string()?;
                let ty = decode_type(&mut r)?;
                if r.u8()? == 1 {
                    // The default is decoded once every type is known; its
                    // bytes are kept aside until then.
                    let default_bytes = take_default_bytes(&mut r, &ty)?;
                    pending_defaults.push((struct_index, field_index, default_bytes.to_vec()));
                }
                let doc = r.string()?;
                fields.push(Field { name: field_name, ty, default: None, doc });
            }
            description.structs.push(StructType { name, version, fields });
        }
        let enum_count = r.u32()? as usize;
        for _ in 0..enum_count {
            let name = r.string()?;
            let version = r.u32()?;
            let variant_count = r.u32()? as usize;
            let mut variants = Vec::with_capacity(variant_count);
            for _ in 0..variant_count {
                variants.push(r.string()?);
            }
            description.enums.push(EnumType { name, version, variants });
        }
        let component_count = r.u32()? as usize;
        for _ in 0..component_count {
            let name = r.string()?;
            let operation_count = r.u32()? as usize;
            let mut operations = Vec::with_capacity(operation_count);
            for _ in 0..operation_count {
                operations.push(r.string()?);
            }
            description.components.push(Component { name, operations });
        }
        let global_count = r.u32()? as usize;
        for _ in 0..global_count {
            let name = r.string()?;
            let ty = decode_type(&mut r)?;
            let doc = r.string()?;
            description.globals.push(Global { name, ty, doc });
        }
        for kind in 0..2 {
            let count = r.u32()? as usize;
            let mut operations = Vec::with_capacity(count);
            for _ in 0..count {
                operations.push(Operation {
                    name: r.string()?,
                    version: r.u32()?,
                    parameters: r.string()?,
                    result: r.string()?,
                    doc: r.string()?,
                });
            }
            if kind == 0 {
                description.commands = operations;
            } else {
                description.queries = operations;
            }
        }
        let event_count = r.u32()? as usize;
        for _ in 0..event_count {
            description.events.push(Event { name: r.string()?, version: r.u32()?, payload: r.string()?, doc: r.string()? });
        }
        if r.at != bytes.len() {
            return error(format!("{} trailing bytes after the description", bytes.len() - r.at));
        }
        for (struct_index, field_index, default_bytes) in pending_defaults {
            let ty = description.structs[struct_index].fields[field_index].ty.clone();
            let mut dr = Reader::new(&default_bytes);
            let value = decode_value(&mut dr, &ty, &description)?;
            description.structs[struct_index].fields[field_index].default = Some(value);
        }
        Ok(description)
    }
}

/// The bytes of a default of type `ty`, taken without interpreting them: a
/// default is decoded once every type of the description is known. The
/// bootstrap format restricts defaults to the types whose length the bytes
/// give without a description (primitives, strings, handles, enumerations and
/// optionals of those).
fn take_default_bytes<'a>(r: &mut Reader<'a>, ty: &Type) -> Result<&'a [u8], DecodeError> {
    let start = r.at;
    skip_default(r, ty)?;
    Ok(&r.bytes[start..r.at])
}

fn skip_default(r: &mut Reader<'_>, ty: &Type) -> Result<(), DecodeError> {
    match ty {
        Type::Bool | Type::U8 => r.take(1).map(drop),
        Type::U16 => r.take(2).map(drop),
        Type::U32 | Type::I32 | Type::F32 | Type::Enum(_) => r.take(4).map(drop),
        Type::U64 | Type::I64 | Type::F64 | Type::Handle => r.take(8).map(drop),
        Type::String => {
            let len = r.u32()? as usize;
            r.take(len).map(drop)
        }
        Type::Optional(inner) => {
            if r.u8()? == 1 {
                skip_default(r, inner)?;
            }
            Ok(())
        }
        Type::Sequence(_) | Type::Struct(_) => error(format!("a default of type {ty:?} is not supported by the description format")),
    }
}

fn encode_type(w: &mut Writer, ty: &Type) {
    match ty {
        Type::Bool => w.u8(0),
        Type::U8 => w.u8(1),
        Type::U16 => w.u8(2),
        Type::U32 => w.u8(3),
        Type::U64 => w.u8(4),
        Type::I32 => w.u8(5),
        Type::I64 => w.u8(6),
        Type::F32 => w.u8(7),
        Type::F64 => w.u8(8),
        Type::String => w.u8(9),
        Type::Handle => w.u8(10),
        Type::Struct(name) => {
            w.u8(11);
            w.string(name);
        }
        Type::Enum(name) => {
            w.u8(12);
            w.string(name);
        }
        Type::Sequence(inner) => {
            w.u8(13);
            encode_type(w, inner);
        }
        Type::Optional(inner) => {
            w.u8(14);
            encode_type(w, inner);
        }
    }
}

fn decode_type(r: &mut Reader<'_>) -> Result<Type, DecodeError> {
    Ok(match r.u8()? {
        0 => Type::Bool,
        1 => Type::U8,
        2 => Type::U16,
        3 => Type::U32,
        4 => Type::U64,
        5 => Type::I32,
        6 => Type::I64,
        7 => Type::F32,
        8 => Type::F64,
        9 => Type::String,
        10 => Type::Handle,
        11 => Type::Struct(r.string()?),
        12 => Type::Enum(r.string()?),
        13 => Type::Sequence(Box::new(decode_type(r)?)),
        14 => Type::Optional(Box::new(decode_type(r)?)),
        tag => return error(format!("unknown type tag {tag}")),
    })
}

/// Encodes `value` as a `ty`, checking that the value has that type.
pub fn encode_value(w: &mut Writer, value: &Value, ty: &Type, d: &Description) -> Result<(), DecodeError> {
    match (ty, value) {
        (Type::Bool, Value::Bool(v)) => w.u8(u8::from(*v)),
        (Type::U8, Value::U8(v)) => w.u8(*v),
        (Type::U16, Value::U16(v)) => w.u16(*v),
        (Type::U32, Value::U32(v)) => w.u32(*v),
        (Type::U64, Value::U64(v)) => w.u64(*v),
        (Type::I32, Value::I32(v)) => w.i32(*v),
        (Type::I64, Value::I64(v)) => w.i64(*v),
        (Type::F32, Value::F32(v)) => w.f32(*v),
        (Type::F64, Value::F64(v)) => w.f64(*v),
        (Type::String, Value::String(v)) => w.string(v),
        (Type::Handle, Value::Handle(h)) => w.handle(*h),
        (Type::Struct(name), Value::Struct(fields)) => {
            let s = d.structure(name)?;
            if s.fields.len() != fields.len() {
                return error(format!("structure `{name}` has {} fields, the value {}", s.fields.len(), fields.len()));
            }
            for (field, value) in s.fields.iter().zip(fields) {
                encode_value(w, value, &field.ty, d)?;
            }
        }
        (Type::Enum(name), Value::Enum(index)) => {
            let e = d.enumeration(name)?;
            if *index as usize >= e.variants.len() {
                return error(format!("enumeration `{name}` has no variant {index}"));
            }
            w.u32(*index);
        }
        (Type::Sequence(inner), Value::Sequence(items)) => {
            w.u32(items.len() as u32);
            for item in items {
                encode_value(w, item, inner, d)?;
            }
        }
        (Type::Optional(inner), Value::Optional(item)) => match item {
            None => w.u8(0),
            Some(item) => {
                w.u8(1);
                encode_value(w, item, inner, d)?;
            }
        },
        (ty, value) => return error(format!("value {value:?} is not of type {ty:?}")),
    }
    Ok(())
}

/// Decodes a value of type `ty` as a tree.
pub fn decode_value(r: &mut Reader<'_>, ty: &Type, d: &Description) -> Result<Value, DecodeError> {
    Ok(match ty {
        Type::Bool => Value::Bool(r.bool()?),
        Type::U8 => Value::U8(r.u8()?),
        Type::U16 => Value::U16(r.u16()?),
        Type::U32 => Value::U32(r.u32()?),
        Type::U64 => Value::U64(r.u64()?),
        Type::I32 => Value::I32(r.i32()?),
        Type::I64 => Value::I64(r.i64()?),
        Type::F32 => Value::F32(r.f32()?),
        Type::F64 => Value::F64(r.f64()?),
        Type::String => Value::String(r.string()?),
        Type::Handle => Value::Handle(r.handle()?),
        Type::Struct(name) => {
            let s = d.structure(name)?;
            let mut fields = Vec::with_capacity(s.fields.len());
            for field in &s.fields {
                fields.push(decode_value(r, &field.ty, d)?);
            }
            Value::Struct(fields)
        }
        Type::Enum(name) => {
            let e = d.enumeration(name)?;
            let index = r.u32()?;
            if index as usize >= e.variants.len() {
                return error(format!("enumeration `{name}` has no variant {index}"));
            }
            Value::Enum(index)
        }
        Type::Sequence(inner) => {
            let count = r.u32()? as usize;
            let mut items = Vec::with_capacity(count.min(1 << 16));
            for _ in 0..count {
                items.push(decode_value(r, inner, d)?);
            }
            Value::Sequence(items)
        }
        Type::Optional(inner) => match r.u8()? {
            0 => Value::Optional(None),
            1 => Value::Optional(Some(Box::new(decode_value(r, inner, d)?))),
            other => return error(format!("presence byte {other}")),
        },
    })
}

/// Skips a value of type `ty` without building it.
pub fn skip_value(r: &mut Reader<'_>, ty: &Type, d: &Description) -> Result<(), DecodeError> {
    match ty {
        Type::Bool | Type::U8 => r.take(1).map(drop),
        Type::U16 => r.take(2).map(drop),
        Type::U32 | Type::I32 | Type::F32 | Type::Enum(_) => r.take(4).map(drop),
        Type::U64 | Type::I64 | Type::F64 | Type::Handle => r.take(8).map(drop),
        Type::String => {
            let len = r.u32()? as usize;
            r.take(len).map(drop)
        }
        Type::Struct(name) => {
            let s = d.structure(name)?;
            for field in &s.fields {
                skip_value(r, &field.ty, d)?;
            }
            Ok(())
        }
        Type::Sequence(inner) => {
            let count = r.u32()?;
            for _ in 0..count {
                skip_value(r, inner, d)?;
            }
            Ok(())
        }
        Type::Optional(inner) => {
            if r.u8()? == 1 {
                skip_value(r, inner, d)?;
            }
            Ok(())
        }
    }
}

/// The canonical byte writer: little-endian, a count before a sequence.
#[derive(Default)]
pub struct Writer {
    bytes: Vec<u8>,
}

impl Writer {
    pub fn new() -> Writer {
        Writer { bytes: Vec::new() }
    }

    pub fn with_capacity(capacity: usize) -> Writer {
        Writer { bytes: Vec::with_capacity(capacity) }
    }

    pub fn u8(&mut self, v: u8) {
        self.bytes.push(v);
    }

    pub fn bool(&mut self, v: bool) {
        self.bytes.push(u8::from(v));
    }

    pub fn u16(&mut self, v: u16) {
        self.bytes.extend_from_slice(&v.to_le_bytes());
    }

    pub fn u32(&mut self, v: u32) {
        self.bytes.extend_from_slice(&v.to_le_bytes());
    }

    pub fn u64(&mut self, v: u64) {
        self.bytes.extend_from_slice(&v.to_le_bytes());
    }

    pub fn i32(&mut self, v: i32) {
        self.bytes.extend_from_slice(&v.to_le_bytes());
    }

    pub fn i64(&mut self, v: i64) {
        self.bytes.extend_from_slice(&v.to_le_bytes());
    }

    pub fn f32(&mut self, v: f32) {
        self.bytes.extend_from_slice(&v.to_le_bytes());
    }

    pub fn f64(&mut self, v: f64) {
        self.bytes.extend_from_slice(&v.to_le_bytes());
    }

    pub fn handle(&mut self, h: Handle) {
        self.u32(h.index);
        self.u32(h.generation);
    }

    pub fn string(&mut self, s: &str) {
        self.u32(s.len() as u32);
        self.bytes.extend_from_slice(s.as_bytes());
    }

    pub fn bytes(&mut self, b: &[u8]) {
        self.bytes.extend_from_slice(b);
    }

    pub fn finish(self) -> Vec<u8> {
        self.bytes
    }
}

/// The canonical byte reader, with bounds checks and a position.
pub struct Reader<'a> {
    pub bytes: &'a [u8],
    pub at: usize,
}

impl<'a> Reader<'a> {
    pub fn new(bytes: &'a [u8]) -> Reader<'a> {
        Reader { bytes, at: 0 }
    }

    pub fn remaining(&self) -> usize {
        self.bytes.len() - self.at
    }

    pub fn take(&mut self, len: usize) -> Result<&'a [u8], DecodeError> {
        let end = self.at.checked_add(len).filter(|&end| end <= self.bytes.len());
        let Some(end) = end else {
            return error(format!("truncated at byte {} (wanted {len} more)", self.at));
        };
        let slice = &self.bytes[self.at..end];
        self.at = end;
        Ok(slice)
    }

    pub fn u8(&mut self) -> Result<u8, DecodeError> {
        Ok(self.take(1)?[0])
    }

    pub fn bool(&mut self) -> Result<bool, DecodeError> {
        match self.u8()? {
            0 => Ok(false),
            1 => Ok(true),
            other => error(format!("boolean byte {other} at byte {}", self.at - 1)),
        }
    }

    pub fn u16(&mut self) -> Result<u16, DecodeError> {
        Ok(u16::from_le_bytes(self.take(2)?.try_into().expect("two bytes")))
    }

    pub fn u32(&mut self) -> Result<u32, DecodeError> {
        Ok(u32::from_le_bytes(self.take(4)?.try_into().expect("four bytes")))
    }

    pub fn u64(&mut self) -> Result<u64, DecodeError> {
        Ok(u64::from_le_bytes(self.take(8)?.try_into().expect("eight bytes")))
    }

    pub fn i32(&mut self) -> Result<i32, DecodeError> {
        Ok(i32::from_le_bytes(self.take(4)?.try_into().expect("four bytes")))
    }

    pub fn i64(&mut self) -> Result<i64, DecodeError> {
        Ok(i64::from_le_bytes(self.take(8)?.try_into().expect("eight bytes")))
    }

    pub fn f32(&mut self) -> Result<f32, DecodeError> {
        Ok(f32::from_le_bytes(self.take(4)?.try_into().expect("four bytes")))
    }

    pub fn f64(&mut self) -> Result<f64, DecodeError> {
        Ok(f64::from_le_bytes(self.take(8)?.try_into().expect("eight bytes")))
    }

    pub fn handle(&mut self) -> Result<Handle, DecodeError> {
        Ok(Handle { index: self.u32()?, generation: self.u32()? })
    }

    pub fn string(&mut self) -> Result<String, DecodeError> {
        let len = self.u32()? as usize;
        match std::str::from_utf8(self.take(len)?) {
            Ok(s) => Ok(s.to_string()),
            Err(_) => error(format!("invalid UTF-8 in a string ending at byte {}", self.at)),
        }
    }

    pub fn finish(&self) -> Result<(), DecodeError> {
        if self.at == self.bytes.len() { Ok(()) } else { error(format!("{} trailing bytes", self.remaining())) }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Description {
        Description {
            engine: "spike".to_string(),
            schema_version: 2,
            structs: vec![
                StructType {
                    name: "point".to_string(),
                    version: 1,
                    fields: vec![
                        Field { name: "x".to_string(), ty: Type::F32, default: None, doc: "abscissa".to_string() },
                        Field { name: "label".to_string(), ty: Type::String, default: Some(Value::String("none".to_string())), doc: String::new() },
                        Field { name: "tag".to_string(), ty: Type::Optional(Box::new(Type::U32)), default: Some(Value::Optional(None)), doc: String::new() },
                    ],
                },
                StructType {
                    name: "path".to_string(),
                    version: 3,
                    fields: vec![
                        Field { name: "points".to_string(), ty: Type::Sequence(Box::new(Type::Struct("point".to_string()))), default: None, doc: String::new() },
                        Field { name: "kind".to_string(), ty: Type::Enum("kind".to_string()), default: Some(Value::Enum(1)), doc: String::new() },
                        Field { name: "owner".to_string(), ty: Type::Handle, default: Some(Value::Handle(Handle { index: 7, generation: 2 })), doc: String::new() },
                    ],
                },
            ],
            enums: vec![EnumType { name: "kind".to_string(), version: 1, variants: vec!["open".to_string(), "closed".to_string()] }],
            components: vec![Component { name: "point".to_string(), operations: vec!["set".to_string()] }],
            globals: vec![Global { name: "tick".to_string(), ty: Type::U32, doc: "the tick".to_string() }],
            commands: vec![Operation { name: "kick".to_string(), version: 1, parameters: "path".to_string(), result: "point".to_string(), doc: "d".to_string() }],
            queries: vec![],
            events: vec![Event { name: "done".to_string(), version: 1, payload: "point".to_string(), doc: String::new() }],
        }
    }

    #[test]
    fn the_description_survives_a_round_trip() {
        let description = sample();
        let bytes = description.encode();
        let decoded = Description::decode(&bytes).unwrap();
        assert_eq!(decoded, description);
    }

    #[test]
    fn a_damaged_description_is_rejected_with_a_reason() {
        let bytes = sample().encode();
        assert!(Description::decode(&bytes[..bytes.len() - 3]).unwrap_err().0.contains("truncated"));
        let mut trailing = bytes.clone();
        trailing.push(0);
        assert!(Description::decode(&trailing).unwrap_err().0.contains("trailing"));
        let mut magic = bytes;
        magic[0] = b'X';
        assert!(Description::decode(&magic).unwrap_err().0.contains("not a contract description"));
    }

    #[test]
    fn values_encode_and_decode_by_type_and_skip_matches() {
        let d = sample();
        let path = Type::Struct("path".to_string());
        let value = Value::Struct(vec![
            Value::Sequence(vec![
                Value::Struct(vec![Value::F32(1.5), Value::String("a".to_string()), Value::Optional(Some(Box::new(Value::U32(9))))]),
                Value::Struct(vec![Value::F32(-2.0), Value::String(String::new()), Value::Optional(None)]),
            ]),
            Value::Enum(1),
            Value::Handle(Handle { index: 3, generation: 4 }),
        ]);
        let mut w = Writer::new();
        encode_value(&mut w, &value, &path, &d).unwrap();
        w.u8(0xff);
        let bytes = w.finish();
        let mut r = Reader::new(&bytes);
        assert_eq!(decode_value(&mut r, &path, &d).unwrap(), value);
        assert_eq!(r.remaining(), 1);
        let mut r = Reader::new(&bytes);
        skip_value(&mut r, &path, &d).unwrap();
        assert_eq!(r.remaining(), 1);
        let mut w = Writer::new();
        assert!(encode_value(&mut w, &Value::U32(1), &path, &d).unwrap_err().0.contains("is not of type"));
        assert!(encode_value(&mut w, &Value::Enum(5), &Type::Enum("kind".to_string()), &d).unwrap_err().0.contains("no variant 5"));
    }
}
