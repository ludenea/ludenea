//! A reader of JSON documents, enough for a compiler's reflection file.
//! Throwaway, never product code.

#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    Null,
    Bool(bool),
    Number(f64),
    Text(String),
    List(Vec<Value>),
    Object(Vec<(String, Value)>),
}

impl Value {
    pub fn get(&self, key: &str) -> Option<&Value> {
        match self {
            Value::Object(members) => members.iter().find(|(name, _)| name == key).map(|(_, value)| value),
            _ => None,
        }
    }

    pub fn text(&self) -> Option<&str> {
        match self {
            Value::Text(text) => Some(text),
            _ => None,
        }
    }

    pub fn list(&self) -> &[Value] {
        match self {
            Value::List(items) => items,
            _ => &[],
        }
    }
}

struct Reader<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl Reader<'_> {
    fn skip_space(&mut self) {
        while self.bytes.get(self.at).is_some_and(|byte| byte.is_ascii_whitespace()) {
            self.at += 1;
        }
    }

    fn expect(&mut self, byte: u8) -> Result<(), String> {
        self.skip_space();
        if self.bytes.get(self.at) == Some(&byte) {
            self.at += 1;
            Ok(())
        } else {
            Err(format!("expected '{}' at byte {}", byte as char, self.at))
        }
    }

    fn literal(&mut self, word: &str, value: Value) -> Result<Value, String> {
        if self.bytes[self.at..].starts_with(word.as_bytes()) {
            self.at += word.len();
            Ok(value)
        } else {
            Err(format!("unexpected text at byte {}", self.at))
        }
    }

    fn string(&mut self) -> Result<String, String> {
        self.expect(b'"')?;
        let mut text = Vec::new();
        loop {
            match self.bytes.get(self.at).copied() {
                None => return Err("a string does not end".to_string()),
                Some(b'"') => {
                    self.at += 1;
                    return Ok(String::from_utf8_lossy(&text).into_owned());
                }
                Some(b'\\') => {
                    let escaped = self.bytes.get(self.at + 1).copied().ok_or("an escape does not end")?;
                    self.at += 2;
                    match escaped {
                        b'n' => text.push(b'\n'),
                        b't' => text.push(b'\t'),
                        b'r' => text.push(b'\r'),
                        b'u' => {
                            // A code point outside ASCII is kept as a mark:
                            // names and kinds, all the spike reads, have none.
                            self.at = (self.at + 4).min(self.bytes.len());
                            text.push(b'?');
                        }
                        other => text.push(other),
                    }
                }
                Some(byte) => {
                    text.push(byte);
                    self.at += 1;
                }
            }
        }
    }

    fn value(&mut self, depth: u32) -> Result<Value, String> {
        if depth > 64 {
            return Err("the document nests too deep".to_string());
        }
        self.skip_space();
        match self.bytes.get(self.at).copied() {
            None => Err("the document ends early".to_string()),
            Some(b'{') => {
                self.at += 1;
                let mut members = Vec::new();
                self.skip_space();
                if self.bytes.get(self.at) == Some(&b'}') {
                    self.at += 1;
                    return Ok(Value::Object(members));
                }
                loop {
                    self.skip_space();
                    let key = self.string()?;
                    self.expect(b':')?;
                    members.push((key, self.value(depth + 1)?));
                    self.skip_space();
                    match self.bytes.get(self.at).copied() {
                        Some(b',') => self.at += 1,
                        Some(b'}') => {
                            self.at += 1;
                            return Ok(Value::Object(members));
                        }
                        _ => return Err(format!("expected ',' or '}}' at byte {}", self.at)),
                    }
                }
            }
            Some(b'[') => {
                self.at += 1;
                let mut items = Vec::new();
                self.skip_space();
                if self.bytes.get(self.at) == Some(&b']') {
                    self.at += 1;
                    return Ok(Value::List(items));
                }
                loop {
                    items.push(self.value(depth + 1)?);
                    self.skip_space();
                    match self.bytes.get(self.at).copied() {
                        Some(b',') => self.at += 1,
                        Some(b']') => {
                            self.at += 1;
                            return Ok(Value::List(items));
                        }
                        _ => return Err(format!("expected ',' or ']' at byte {}", self.at)),
                    }
                }
            }
            Some(b'"') => self.string().map(Value::Text),
            Some(b't') => self.literal("true", Value::Bool(true)),
            Some(b'f') => self.literal("false", Value::Bool(false)),
            Some(b'n') => self.literal("null", Value::Null),
            Some(_) => {
                let start = self.at;
                while self.bytes.get(self.at).is_some_and(|byte| byte.is_ascii_digit() || b"+-.eE".contains(byte)) {
                    self.at += 1;
                }
                std::str::from_utf8(&self.bytes[start..self.at]).ok().and_then(|text| text.parse::<f64>().ok()).map(Value::Number).ok_or_else(|| format!("unexpected text at byte {start}"))
            }
        }
    }
}

pub fn parse(text: &str) -> Result<Value, String> {
    let mut reader = Reader { bytes: text.as_bytes(), at: 0 };
    let value = reader.value(0)?;
    reader.skip_space();
    if reader.at == reader.bytes.len() { Ok(value) } else { Err(format!("text follows the document at byte {}", reader.at)) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_reflection_like_document_is_read() {
        let value = parse(r#"{"parameters": [{"name": "view", "used": 1, "type": {"kind": "constantBuffer"}}, {"name": "a\"b", "x": [true, false, null, -1.5e2]}], "entryPoints": []}"#).unwrap();
        let parameters = value.get("parameters").unwrap().list();
        assert_eq!(parameters.len(), 2);
        assert_eq!(parameters[0].get("name").and_then(Value::text), Some("view"));
        assert_eq!(parameters[0].get("used"), Some(&Value::Number(1.0)));
        assert_eq!(parameters[0].get("type").and_then(|kind| kind.get("kind")).and_then(Value::text), Some("constantBuffer"));
        assert_eq!(parameters[1].get("name").and_then(Value::text), Some("a\"b"));
        assert_eq!(parameters[1].get("x").unwrap().list(), [Value::Bool(true), Value::Bool(false), Value::Null, Value::Number(-150.0)]);
        assert!(value.get("entryPoints").unwrap().list().is_empty());
    }

    #[test]
    fn a_malformed_document_is_refused() {
        assert!(parse("{\"a\": }").is_err());
        assert!(parse("[1, 2").is_err());
        assert!(parse("{} x").is_err());
        assert!(parse("\"open").is_err());
    }
}
