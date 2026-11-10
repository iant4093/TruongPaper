//! Minimal JSON value + parser used by the shader preprocessor (`// [COMBO]` / uniform comment
//! metadata) and the particle system (`particles/*.json`).
//!
//! NOTE: the spec (`docs/03` §4.4 / §5.3) uses `serde_json::Value`, but `serde`/`serde_json` are
//! not dependencies of `truongpaper-renderer`. This module mirrors the small subset of the
//! `serde_json::Value` API the spec code relies on (`get`, `as_str`, `as_f64`, `as_i64`, `as_u64`).
//! If `serde_json` is later added to this crate, this module can be replaced by `serde_json::Value`.

use std::fmt;

#[derive(Debug, Clone, PartialEq, Default)]
pub enum Value {
    #[default]
    Null,
    Bool(bool),
    Number(f64),
    String(String),
    Array(Vec<Value>),
    /// Insertion-ordered key/value pairs. Lookups return the last duplicate (like `serde_json`).
    Object(Vec<(String, Value)>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JsonError {
    pub message: String,
    pub offset: usize,
}

impl fmt::Display for JsonError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "JSON error at byte {}: {}", self.offset, self.message)
    }
}

impl std::error::Error for JsonError {}

impl Value {
    /// Parses a complete JSON document (trailing whitespace allowed).
    pub fn parse(src: &str) -> Result<Value, JsonError> {
        let mut p = Parser {
            bytes: src.as_bytes(),
            pos: 0,
            depth: 0,
        };
        p.skip_ws();
        let v = p.parse_value()?;
        p.skip_ws();
        if p.pos != p.bytes.len() {
            return Err(p.err("trailing characters"));
        }
        Ok(v)
    }

    pub fn get(&self, key: &str) -> Option<&Value> {
        match self {
            Value::Object(entries) => entries.iter().rev().find(|(k, _)| k == key).map(|e| &e.1),
            _ => None,
        }
    }

    pub fn as_str(&self) -> Option<&str> {
        match self {
            Value::String(s) => Some(s),
            _ => None,
        }
    }

    pub fn as_f64(&self) -> Option<f64> {
        match self {
            Value::Number(n) => Some(*n),
            _ => None,
        }
    }

    /// `Some` only for integral numbers within `i64` range (matches `serde_json` semantics).
    pub fn as_i64(&self) -> Option<i64> {
        match self {
            Value::Number(n)
                if n.fract() == 0.0 && *n >= i64::MIN as f64 && *n <= i64::MAX as f64 =>
            {
                Some(*n as i64)
            }
            _ => None,
        }
    }

    /// `Some` only for non-negative integral numbers.
    pub fn as_u64(&self) -> Option<u64> {
        match self {
            Value::Number(n) if n.fract() == 0.0 && *n >= 0.0 && *n <= u64::MAX as f64 => {
                Some(*n as u64)
            }
            _ => None,
        }
    }

    pub fn as_bool(&self) -> Option<bool> {
        match self {
            Value::Bool(b) => Some(*b),
            _ => None,
        }
    }

    pub fn as_array(&self) -> Option<&[Value]> {
        match self {
            Value::Array(a) => Some(a),
            _ => None,
        }
    }

    pub fn is_null(&self) -> bool {
        matches!(self, Value::Null)
    }

    pub fn is_object(&self) -> bool {
        matches!(self, Value::Object(_))
    }

    /// Serializes to compact JSON. Non-finite numbers become `null`. Besides the mandatory
    /// escapes, `<`, `>`, `&`, U+2028 and U+2029 are written as `\uXXXX` so the output is safe to
    /// embed in a `<script>` block or in JavaScript passed to `evaluate_script`.
    pub fn to_json_string(&self) -> String {
        let mut out = String::new();
        write_value(&mut out, self);
        out
    }
}

impl fmt::Display for Value {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.to_json_string())
    }
}

fn write_value(out: &mut String, v: &Value) {
    match v {
        Value::Null => out.push_str("null"),
        Value::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
        Value::Number(n) => write_number(out, *n),
        Value::String(s) => write_string(out, s),
        Value::Array(items) => {
            out.push('[');
            for (i, item) in items.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                write_value(out, item);
            }
            out.push(']');
        }
        Value::Object(entries) => {
            out.push('{');
            for (i, (k, item)) in entries.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                write_string(out, k);
                out.push(':');
                write_value(out, item);
            }
            out.push('}');
        }
    }
}

fn write_number(out: &mut String, n: f64) {
    use std::fmt::Write;
    /// 2^53: integers up to this magnitude are exact in f64.
    const MAX_EXACT_INT: f64 = 9_007_199_254_740_992.0;
    if !n.is_finite() {
        out.push_str("null");
    } else if n.fract() == 0.0 && n.abs() <= MAX_EXACT_INT {
        let _ = write!(out, "{}", n as i64);
    } else {
        // `Debug` gives the shortest round-trip form and switches to exponent notation for very
        // large/small magnitudes (`1e300`), both of which are valid JSON numbers.
        let _ = write!(out, "{n:?}");
    }
}

fn write_string(out: &mut String, s: &str) {
    use std::fmt::Write;
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{8}' => out.push_str("\\b"),
            '\u{c}' => out.push_str("\\f"),
            c if (c as u32) < 0x20 || matches!(c, '<' | '>' | '&' | '\u{2028}' | '\u{2029}') => {
                let _ = write!(out, "\\u{:04x}", c as u32);
            }
            c => out.push(c),
        }
    }
    out.push('"');
}

const MAX_DEPTH: usize = 128;

struct Parser<'a> {
    bytes: &'a [u8],
    pos: usize,
    depth: usize,
}

impl Parser<'_> {
    fn err(&self, msg: &str) -> JsonError {
        JsonError {
            message: msg.to_string(),
            offset: self.pos,
        }
    }

    fn peek(&self) -> Option<u8> {
        self.bytes.get(self.pos).copied()
    }

    fn skip_ws(&mut self) {
        while matches!(self.peek(), Some(b' ' | b'\t' | b'\n' | b'\r')) {
            self.pos += 1;
        }
    }

    fn expect_literal(&mut self, lit: &str, v: Value) -> Result<Value, JsonError> {
        if self.bytes[self.pos..].starts_with(lit.as_bytes()) {
            self.pos += lit.len();
            Ok(v)
        } else {
            Err(self.err("invalid literal"))
        }
    }

    fn parse_value(&mut self) -> Result<Value, JsonError> {
        match self.peek() {
            Some(b'{') => self.parse_object(),
            Some(b'[') => self.parse_array(),
            Some(b'"') => Ok(Value::String(self.parse_string()?)),
            Some(b't') => self.expect_literal("true", Value::Bool(true)),
            Some(b'f') => self.expect_literal("false", Value::Bool(false)),
            Some(b'n') => self.expect_literal("null", Value::Null),
            Some(b'-' | b'0'..=b'9') => self.parse_number(),
            Some(_) => Err(self.err("unexpected character")),
            None => Err(self.err("unexpected end of input")),
        }
    }

    fn enter(&mut self) -> Result<(), JsonError> {
        self.depth += 1;
        if self.depth > MAX_DEPTH {
            return Err(self.err("nesting too deep"));
        }
        Ok(())
    }

    fn parse_object(&mut self) -> Result<Value, JsonError> {
        self.enter()?;
        self.pos += 1; // '{'
        let mut entries = Vec::new();
        self.skip_ws();
        if self.peek() == Some(b'}') {
            self.pos += 1;
            self.depth -= 1;
            return Ok(Value::Object(entries));
        }
        loop {
            self.skip_ws();
            if self.peek() != Some(b'"') {
                return Err(self.err("expected object key"));
            }
            let key = self.parse_string()?;
            self.skip_ws();
            if self.peek() != Some(b':') {
                return Err(self.err("expected ':'"));
            }
            self.pos += 1;
            self.skip_ws();
            let v = self.parse_value()?;
            entries.push((key, v));
            self.skip_ws();
            match self.peek() {
                Some(b',') => self.pos += 1,
                Some(b'}') => {
                    self.pos += 1;
                    self.depth -= 1;
                    return Ok(Value::Object(entries));
                }
                _ => return Err(self.err("expected ',' or '}'")),
            }
        }
    }

    fn parse_array(&mut self) -> Result<Value, JsonError> {
        self.enter()?;
        self.pos += 1; // '['
        let mut items = Vec::new();
        self.skip_ws();
        if self.peek() == Some(b']') {
            self.pos += 1;
            self.depth -= 1;
            return Ok(Value::Array(items));
        }
        loop {
            self.skip_ws();
            items.push(self.parse_value()?);
            self.skip_ws();
            match self.peek() {
                Some(b',') => self.pos += 1,
                Some(b']') => {
                    self.pos += 1;
                    self.depth -= 1;
                    return Ok(Value::Array(items));
                }
                _ => return Err(self.err("expected ',' or ']'")),
            }
        }
    }

    fn parse_hex4(&mut self) -> Result<u32, JsonError> {
        let s = self
            .bytes
            .get(self.pos..self.pos + 4)
            .and_then(|b| std::str::from_utf8(b).ok())
            .ok_or_else(|| self.err("truncated \\u escape"))?;
        let v = u32::from_str_radix(s, 16).map_err(|_| self.err("invalid \\u escape"))?;
        self.pos += 4;
        Ok(v)
    }

    fn parse_string(&mut self) -> Result<String, JsonError> {
        self.pos += 1; // opening quote
        let mut out = String::new();
        loop {
            let start = self.pos;
            while let Some(b) = self.peek() {
                if b == b'"' || b == b'\\' || b < 0x20 {
                    break;
                }
                self.pos += 1;
            }
            // Input is a &str and we only split at ASCII bytes, so this slice is valid UTF-8.
            out.push_str(std::str::from_utf8(&self.bytes[start..self.pos]).unwrap_or(""));
            match self.peek() {
                Some(b'"') => {
                    self.pos += 1;
                    return Ok(out);
                }
                Some(b'\\') => {
                    self.pos += 1;
                    let esc = self.peek().ok_or_else(|| self.err("truncated escape"))?;
                    self.pos += 1;
                    match esc {
                        b'"' => out.push('"'),
                        b'\\' => out.push('\\'),
                        b'/' => out.push('/'),
                        b'b' => out.push('\u{8}'),
                        b'f' => out.push('\u{c}'),
                        b'n' => out.push('\n'),
                        b'r' => out.push('\r'),
                        b't' => out.push('\t'),
                        b'u' => {
                            let hi = self.parse_hex4()?;
                            let cp = if (0xD800..0xDC00).contains(&hi)
                                && self.bytes[self.pos..].starts_with(b"\\u")
                            {
                                self.pos += 2;
                                let lo = self.parse_hex4()?;
                                0x10000 + ((hi - 0xD800) << 10) + (lo.wrapping_sub(0xDC00) & 0x3FF)
                            } else {
                                hi
                            };
                            out.push(char::from_u32(cp).unwrap_or('\u{FFFD}'));
                        }
                        _ => return Err(self.err("invalid escape")),
                    }
                }
                Some(_) => return Err(self.err("control character in string")),
                None => return Err(self.err("unterminated string")),
            }
        }
    }

    fn parse_number(&mut self) -> Result<Value, JsonError> {
        let start = self.pos;
        while matches!(
            self.peek(),
            Some(b'-' | b'+' | b'.' | b'e' | b'E' | b'0'..=b'9')
        ) {
            self.pos += 1;
        }
        let s = std::str::from_utf8(&self.bytes[start..self.pos]).unwrap_or("");
        s.parse::<f64>().map(Value::Number).map_err(|_| JsonError {
            message: format!("invalid number '{s}'"),
            offset: start,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_nested_documents() {
        let v = Value::parse(
            r#"{"combo":"BLEND","default":2,"f":-1.5e1,"arr":[1,"a\n\u00e9",null,true],"o":{}}"#,
        )
        .unwrap();
        assert_eq!(v.get("combo").and_then(Value::as_str), Some("BLEND"));
        assert_eq!(v.get("default").and_then(Value::as_i64), Some(2));
        assert_eq!(v.get("f").and_then(Value::as_f64), Some(-15.0));
        assert_eq!(v.get("f").and_then(Value::as_i64), Some(-15));
        assert_eq!(v.get("f").and_then(Value::as_u64), None);
        let arr = v.get("arr").and_then(Value::as_array).unwrap();
        assert_eq!(arr[1].as_str(), Some("a\né"));
        assert!(arr[2].is_null());
        assert_eq!(arr[3].as_bool(), Some(true));
    }

    #[test]
    fn serializes_round_trip_and_escapes_script_text() {
        let src = r#"{"a":[1,-2.5,1e300,true,null],"s":"q\"\\\n</script>\u2028é","o":{}}"#;
        let v = Value::parse(src).unwrap();
        let out = v.to_json_string();
        assert_eq!(
            out,
            r#"{"a":[1,-2.5,1e300,true,null],"s":"q\"\\\n\u003c/script\u003e\u2028é","o":{}}"#
        );
        assert_eq!(Value::parse(&out).unwrap(), v);
        assert_eq!(Value::Number(f64::NAN).to_string(), "null");
        assert_eq!(Value::Number(0.1).to_string(), "0.1");
    }

    #[test]
    fn rejects_malformed_input() {
        assert!(Value::parse("{\"a\":}").is_err());
        assert!(Value::parse("[1,2").is_err());
        assert!(Value::parse("1 2").is_err());
        assert!(Value::parse(&"[".repeat(500)).is_err());
    }
}
