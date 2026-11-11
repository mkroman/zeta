//! An RFC 8259 parser for [`Value`](super::Value).
//!
//! The parser is strict about syntax (trailing commas, leading zeros and control characters
//! in strings are rejected) but lenient about the value types it produces, keeping integers
//! as `u64`/`i64` and only falling back to `f64` for fractional or exponent forms.

use super::{Error, Object, Value};

const MAX_DEPTH: usize = 128;

/// Parse one JSON document.
pub fn parse(text: &str) -> Result<Value, Error> {
    let mut parser = Parser {
        bytes: text.as_bytes(),
        index: 0,
    };
    let value = parser.value(0)?;
    parser.whitespace();
    if parser.index == parser.bytes.len() {
        Ok(value)
    } else {
        Err(parser.error("trailing characters after the JSON document"))
    }
}

struct Parser<'a> {
    bytes: &'a [u8],
    index: usize,
}

impl Parser<'_> {
    fn value(&mut self, depth: usize) -> Result<Value, Error> {
        if depth > MAX_DEPTH {
            return Err(self.error("JSON nesting is too deep"));
        }
        self.whitespace();
        match self.peek() {
            Some(b'{') => self.object(depth),
            Some(b'[') => self.array(depth),
            Some(b'"') => self.string().map(Value::String),
            Some(b't') => {
                self.literal(b"true")?;
                Ok(Value::Bool(true))
            }
            Some(b'f') => {
                self.literal(b"false")?;
                Ok(Value::Bool(false))
            }
            Some(b'n') => {
                self.literal(b"null")?;
                Ok(Value::Null)
            }
            Some(b'-' | b'0'..=b'9') => self.number(),
            Some(_) => Err(self.error("expected a JSON value")),
            None => Err(self.error("unexpected end of input")),
        }
    }

    fn object(&mut self, depth: usize) -> Result<Value, Error> {
        self.index += 1;
        let mut object = Object::new();
        self.whitespace();
        if self.eat(b'}') {
            return Ok(Value::Object(object));
        }
        loop {
            self.whitespace();
            if self.peek() != Some(b'"') {
                return Err(self.error("expected a quoted object key"));
            }
            let key = self.string()?;
            self.whitespace();
            if !self.eat(b':') {
                return Err(self.error("expected `:` after an object key"));
            }
            let value = self.value(depth + 1)?;
            object.insert(key, value);
            self.whitespace();
            match self.next() {
                Some(b',') => {}
                Some(b'}') => return Ok(Value::Object(object)),
                Some(_) => return Err(self.error("expected `,` or `}` in an object")),
                None => return Err(self.error("unexpected end of input in an object")),
            }
        }
    }

    fn array(&mut self, depth: usize) -> Result<Value, Error> {
        self.index += 1;
        let mut values = Vec::new();
        self.whitespace();
        if self.eat(b']') {
            return Ok(Value::Array(values));
        }
        loop {
            values.push(self.value(depth + 1)?);
            self.whitespace();
            match self.next() {
                Some(b',') => {}
                Some(b']') => return Ok(Value::Array(values)),
                Some(_) => return Err(self.error("expected `,` or `]` in an array")),
                None => return Err(self.error("unexpected end of input in an array")),
            }
        }
    }

    fn string(&mut self) -> Result<String, Error> {
        self.index += 1;
        let mut output = String::new();
        let mut segment = self.index;
        loop {
            let Some(byte) = self.peek() else {
                return Err(self.error("unterminated string"));
            };
            match byte {
                b'"' => {
                    output.push_str(self.slice(segment, self.index));
                    self.index += 1;
                    return Ok(output);
                }
                b'\\' => {
                    output.push_str(self.slice(segment, self.index));
                    self.index += 1;
                    self.escape(&mut output)?;
                    segment = self.index;
                }
                b if b < 0x20 => {
                    return Err(self.error("unescaped control character in a string"));
                }
                _ => self.index += 1,
            }
        }
    }

    fn escape(&mut self, output: &mut String) -> Result<(), Error> {
        let Some(byte) = self.next() else {
            return Err(self.error("unterminated escape sequence"));
        };
        match byte {
            b'"' => output.push('"'),
            b'\\' => output.push('\\'),
            b'/' => output.push('/'),
            b'b' => output.push('\u{0008}'),
            b'f' => output.push('\u{000c}'),
            b'n' => output.push('\n'),
            b'r' => output.push('\r'),
            b't' => output.push('\t'),
            b'u' => {
                let first = self.hex4()?;
                let code = if (0xd800..=0xdbff).contains(&first) {
                    if !self.eat(b'\\') || !self.eat(b'u') {
                        return Err(self.error("unpaired high surrogate in a string"));
                    }
                    let second = self.hex4()?;
                    if !(0xdc00..=0xdfff).contains(&second) {
                        return Err(self.error("invalid low surrogate in a string"));
                    }
                    0x10000 + ((first - 0xd800) << 10) + (second - 0xdc00)
                } else if (0xdc00..=0xdfff).contains(&first) {
                    return Err(self.error("unpaired low surrogate in a string"));
                } else {
                    first
                };
                let Some(character) = char::from_u32(code) else {
                    return Err(self.error("invalid unicode escape in a string"));
                };
                output.push(character);
            }
            _ => return Err(self.error("invalid escape sequence in a string")),
        }
        Ok(())
    }

    fn hex4(&mut self) -> Result<u32, Error> {
        let mut value = 0_u32;
        for _ in 0..4 {
            let Some(byte) = self.next() else {
                return Err(self.error("truncated unicode escape in a string"));
            };
            let Some(digit) = char::from(byte).to_digit(16) else {
                return Err(self.error("invalid unicode escape in a string"));
            };
            value = value * 16 + digit;
        }
        Ok(value)
    }

    fn number(&mut self) -> Result<Value, Error> {
        let start = self.index;
        self.eat(b'-');
        match self.next() {
            Some(b'0') => {
                if self.peek().is_some_and(|byte| byte.is_ascii_digit()) {
                    return Err(self.error("leading zero in a number"));
                }
            }
            Some(b'1'..=b'9') => {
                self.digits();
            }
            Some(_) => return Err(self.error("invalid number")),
            None => return Err(self.error("unexpected end of input in a number")),
        }

        let mut float = false;
        if self.eat(b'.') {
            float = true;
            if !self.digits() {
                return Err(self.error("expected digits after a decimal point"));
            }
        }
        if self.peek().is_some_and(|byte| byte == b'e' || byte == b'E') {
            float = true;
            self.index += 1;
            if self.peek().is_some_and(|byte| byte == b'+' || byte == b'-') {
                self.index += 1;
            }
            if !self.digits() {
                return Err(self.error("expected digits in an exponent"));
            }
        }

        let text = self.slice(start, self.index);
        if !float {
            if let Ok(value) = text.parse::<u64>() {
                return Ok(Value::U64(value));
            }
            if let Ok(value) = text.parse::<i64>() {
                return Ok(Value::I64(value));
            }
        }
        text.parse::<f64>()
            .map(Value::F64)
            .map_err(|_| self.error("invalid number"))
    }

    fn digits(&mut self) -> bool {
        let start = self.index;
        while self.peek().is_some_and(|byte| byte.is_ascii_digit()) {
            self.index += 1;
        }
        self.index > start
    }

    fn literal(&mut self, expected: &[u8]) -> Result<(), Error> {
        if self.bytes.get(self.index..self.index + expected.len()) == Some(expected) {
            self.index += expected.len();
            Ok(())
        } else {
            Err(self.error("invalid JSON literal"))
        }
    }

    fn whitespace(&mut self) {
        while self
            .peek()
            .is_some_and(|byte| matches!(byte, b' ' | b'\t' | b'\n' | b'\r'))
        {
            self.index += 1;
        }
    }

    fn peek(&self) -> Option<u8> {
        self.bytes.get(self.index).copied()
    }

    fn next(&mut self) -> Option<u8> {
        let byte = self.peek()?;
        self.index += 1;
        Some(byte)
    }

    fn eat(&mut self, byte: u8) -> bool {
        if self.peek() == Some(byte) {
            self.index += 1;
            true
        } else {
            false
        }
    }

    fn slice(&self, start: usize, end: usize) -> &str {
        // The parser only ever advances over UTF-8 boundaries: ASCII tokens and complete
        // UTF-8 sequences inside strings, which start from valid `&str` input.
        std::str::from_utf8(&self.bytes[start..end]).unwrap_or_default()
    }

    fn error(&self, message: &str) -> Error {
        Error::new(message, self.index)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_nested_documents() {
        let value = parse(
            r#"{
                "name": "bench \"quoted\"\n",
                "counts": [0, -1, 1.5, 2e3, true, false, null],
                "nested": {"empty": {}, "list": []}
            }"#,
        )
        .expect("valid JSON");
        assert_eq!(
            value.get("name").and_then(Value::as_str),
            Some("bench \"quoted\"\n")
        );
        let counts = value
            .get("counts")
            .and_then(Value::as_array)
            .expect("array");
        assert_eq!(counts[0].as_u64(), Some(0));
        assert_eq!(counts[1], Value::I64(-1));
        assert_eq!(counts[2], Value::F64(1.5));
        assert_eq!(counts[3], Value::F64(2000.0));
        assert_eq!(counts[4], Value::Bool(true));
        assert_eq!(counts[5], Value::Bool(false));
        assert_eq!(counts[6], Value::Null);
        assert_eq!(
            value
                .get("nested")
                .and_then(Value::as_object)
                .map(Object::len),
            Some(2)
        );
    }

    #[test]
    fn parses_unicode_escapes_and_surrogate_pairs() {
        let value = parse(r#""caf\u00e9 \ud83d\ude00""#).expect("valid JSON");
        assert_eq!(value, Value::String("café 😀".to_owned()));
        assert!(parse(r#""\ud83d""#).is_err());
        assert!(parse(r#""\ude00""#).is_err());
        assert!(parse(r#""\u12""#).is_err());
        assert!(parse(r#""\x""#).is_err());
    }

    #[test]
    fn rejects_malformed_documents() {
        for document in [
            "",
            "{",
            "[1,]",
            "{\"a\":1,}",
            "{\"a\" 1}",
            "01",
            "1.",
            "1e",
            "truex",
            "\"unterminated",
            "\"control\u{0001}\"",
            "[1] trailing",
        ] {
            assert!(parse(document).is_err(), "{document:?} should be rejected");
        }
        assert!(parse("[1, 2, 3]").is_ok());
    }

    #[test]
    fn rejects_excessive_nesting() {
        let document = format!("{}{}", "[".repeat(MAX_DEPTH + 2), "]".repeat(MAX_DEPTH + 2));
        assert!(parse(&document).is_err());
    }
}
