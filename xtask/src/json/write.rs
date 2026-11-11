//! Compact and pretty serialization for [`Value`](super::Value).
//!
//! Object keys are emitted in the map's sorted order, keeping report payloads deterministic
//! across runs.

use std::fmt::Write as _;

use super::Value;

/// Serialize a value without insignificant whitespace.
#[must_use]
pub fn to_string(value: &Value) -> String {
    let mut output = String::new();
    write_value(value, &mut output, None);
    output
}

/// Serialize a value with two-space indentation.
#[must_use]
pub fn to_string_pretty(value: &Value) -> String {
    let mut output = String::new();
    write_value(value, &mut output, Some(0));
    output
}

fn write_value(value: &Value, output: &mut String, indent: Option<usize>) {
    match value {
        Value::Null => output.push_str("null"),
        Value::Bool(true) => output.push_str("true"),
        Value::Bool(false) => output.push_str("false"),
        Value::U64(value) => {
            let _ = write!(output, "{value}");
        }
        Value::I64(value) => {
            let _ = write!(output, "{value}");
        }
        Value::F64(value) => {
            if value.is_finite() && value.fract() == 0.0 {
                let _ = write!(output, "{value:.1}");
            } else {
                let _ = write!(output, "{value}");
            }
        }
        Value::String(value) => write_string(value, output),
        Value::Array(values) => {
            output.push('[');
            if !values.is_empty() {
                for (index, value) in values.iter().enumerate() {
                    if index > 0 {
                        output.push(',');
                    }
                    newline(output, indent, 1);
                    write_value(value, output, indent.map(|depth| depth + 1));
                }
                newline(output, indent, 0);
            }
            output.push(']');
        }
        Value::Object(object) => {
            output.push('{');
            if !object.is_empty() {
                for (index, (key, value)) in object.iter().enumerate() {
                    if index > 0 {
                        output.push(',');
                    }
                    newline(output, indent, 1);
                    write_string(key, output);
                    output.push(':');
                    if indent.is_some() {
                        output.push(' ');
                    }
                    write_value(value, output, indent.map(|depth| depth + 1));
                }
                newline(output, indent, 0);
            }
            output.push('}');
        }
    }
}

fn newline(output: &mut String, indent: Option<usize>, extra: usize) {
    if let Some(depth) = indent {
        output.push('\n');
        for _ in 0..(depth + extra) {
            output.push_str("  ");
        }
    }
}

fn write_string(value: &str, output: &mut String) {
    output.push('"');
    for character in value.chars() {
        match character {
            '"' => output.push_str("\\\""),
            '\\' => output.push_str("\\\\"),
            '\u{0008}' => output.push_str("\\b"),
            '\u{000c}' => output.push_str("\\f"),
            '\n' => output.push_str("\\n"),
            '\r' => output.push_str("\\r"),
            '\t' => output.push_str("\\t"),
            character if character < '\u{0020}' => {
                let _ = write!(output, "\\u{:04x}", u32::from(character));
            }
            character => output.push(character),
        }
    }
    output.push('"');
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::json::parse;

    #[test]
    fn writes_compact_and_pretty_json() {
        let value = Value::object([
            ("b", Value::from(2_u64)),
            ("a", Value::from("x\"y\\z\n")),
            ("empty", Value::array(Vec::new())),
            ("list", Value::array([Value::from(true), Value::Null])),
        ]);
        assert_eq!(
            to_string(&value),
            r#"{"a":"x\"y\\z\n","b":2,"empty":[],"list":[true,null]}"#
        );
        assert_eq!(
            to_string_pretty(&value),
            "{\n  \"a\": \"x\\\"y\\\\z\\n\",\n  \"b\": 2,\n  \"empty\": [],\n  \"list\": [\n    true,\n    null\n  ]\n}"
        );
    }

    #[test]
    fn roundtrips_serialized_documents() {
        let document = r#"{"a":[1,-2,3.5,{"b":"c"}],"d":null}"#;
        let value = parse(document).expect("valid JSON");
        assert_eq!(parse(&to_string(&value)).ok(), Some(value));
    }
}
