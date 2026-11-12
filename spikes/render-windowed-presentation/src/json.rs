//! A JSON value the spike builds and prints, with no dependency. Throwaway,
//! never product code.

/// A JSON value. `Raw` holds text that is already JSON: the document a child
/// process printed.
#[derive(Clone, Debug)]
pub enum Json {
    Null,
    Bool(bool),
    Int(i64),
    Float(f64),
    Text(String),
    List(Vec<Json>),
    Object(Vec<(String, Json)>),
    Raw(String),
}

impl Json {
    pub fn object() -> Json {
        Json::Object(Vec::new())
    }

    /// Adds a member to an object and returns it, for chained construction.
    pub fn with(mut self, key: &str, value: impl Into<Json>) -> Json {
        self.set(key, value);
        self
    }

    pub fn set(&mut self, key: &str, value: impl Into<Json>) {
        if let Json::Object(members) = self {
            let value = value.into();
            match members.iter_mut().find(|(name, _)| name == key) {
                Some(member) => member.1 = value,
                None => members.push((key.to_string(), value)),
            }
        }
    }

    pub fn print(&self) -> String {
        let mut out = String::new();
        self.write(&mut out);
        out
    }

    fn write(&self, out: &mut String) {
        match self {
            Json::Null => out.push_str("null"),
            Json::Bool(value) => out.push_str(if *value { "true" } else { "false" }),
            Json::Int(value) => out.push_str(&value.to_string()),
            Json::Float(value) => {
                if value.is_finite() {
                    out.push_str(&format!("{value:.3}"));
                } else {
                    out.push_str("null");
                }
            }
            Json::Text(text) => write_text(text, out),
            Json::List(items) => {
                out.push('[');
                for (index, item) in items.iter().enumerate() {
                    if index > 0 {
                        out.push(',');
                    }
                    item.write(out);
                }
                out.push(']');
            }
            Json::Object(members) => {
                out.push('{');
                for (index, (key, value)) in members.iter().enumerate() {
                    if index > 0 {
                        out.push(',');
                    }
                    write_text(key, out);
                    out.push(':');
                    value.write(out);
                }
                out.push('}');
            }
            Json::Raw(text) => out.push_str(text),
        }
    }
}

fn write_text(text: &str, out: &mut String) {
    out.push('"');
    for character in text.chars() {
        match character {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            control if (control as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", control as u32)),
            other => out.push(other),
        }
    }
    out.push('"');
}

impl From<bool> for Json {
    fn from(value: bool) -> Json {
        Json::Bool(value)
    }
}

impl From<i64> for Json {
    fn from(value: i64) -> Json {
        Json::Int(value)
    }
}

impl From<i32> for Json {
    fn from(value: i32) -> Json {
        Json::Int(i64::from(value))
    }
}

impl From<u32> for Json {
    fn from(value: u32) -> Json {
        Json::Int(i64::from(value))
    }
}

impl From<u64> for Json {
    fn from(value: u64) -> Json {
        Json::Int(value.min(i64::MAX as u64) as i64)
    }
}

impl From<usize> for Json {
    fn from(value: usize) -> Json {
        Json::Int(value.min(i64::MAX as usize) as i64)
    }
}

impl From<f64> for Json {
    fn from(value: f64) -> Json {
        Json::Float(value)
    }
}

impl From<&str> for Json {
    fn from(value: &str) -> Json {
        Json::Text(value.to_string())
    }
}

impl From<String> for Json {
    fn from(value: String) -> Json {
        Json::Text(value)
    }
}

impl From<Vec<Json>> for Json {
    fn from(value: Vec<Json>) -> Json {
        Json::List(value)
    }
}

impl<T: Into<Json>> From<Option<T>> for Json {
    fn from(value: Option<T>) -> Json {
        value.map_or(Json::Null, Into::into)
    }
}

#[cfg(test)]
mod tests {
    use super::Json;

    #[test]
    fn an_object_prints_its_members_in_order_and_escapes_text() {
        let value = Json::object().with("name", "a \"quoted\"\nline").with("count", 3_u64).with("ok", true).with("none", Option::<u64>::None);
        assert_eq!(value.print(), r#"{"name":"a \"quoted\"\nline","count":3,"ok":true,"none":null}"#);
    }

    #[test]
    fn setting_a_member_twice_keeps_the_last_value() {
        let value = Json::object().with("a", 1_u64).with("a", 2_u64);
        assert_eq!(value.print(), r#"{"a":2}"#);
    }
}
