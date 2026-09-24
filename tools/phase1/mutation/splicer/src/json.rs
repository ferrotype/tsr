//! Canonical JSON: sorted keys, no whitespace, ASCII-only strings. Byte-equal
//! to Python's `json.dumps(value, sort_keys=True, separators=(",", ":"),
//! ensure_ascii=True)` (`s08_oracle.canonical`) for the integers, strings,
//! lists and objects a plan holds, whatever serde_json's map features are.

use std::fmt::Write as _;

use serde_json::Value;

pub fn canonical(value: &Value) -> String {
    let mut out = String::new();
    write_value(value, &mut out);
    out
}

fn write_value(value: &Value, out: &mut String) {
    match value {
        Value::Null => out.push_str("null"),
        Value::Bool(flag) => out.push_str(if *flag { "true" } else { "false" }),
        Value::Number(number) => out.push_str(&number.to_string()),
        Value::String(text) => write_string(text, out),
        Value::Array(items) => {
            out.push('[');
            for (index, item) in items.iter().enumerate() {
                if index > 0 {
                    out.push(',');
                }
                write_value(item, out);
            }
            out.push(']');
        }
        Value::Object(map) => {
            let mut entries: Vec<_> = map.iter().collect();
            entries.sort_by(|left, right| left.0.cmp(right.0));
            out.push('{');
            for (index, (key, item)) in entries.into_iter().enumerate() {
                if index > 0 {
                    out.push(',');
                }
                write_string(key, out);
                out.push(':');
                write_value(item, out);
            }
            out.push('}');
        }
    }
}

fn write_string(text: &str, out: &mut String) {
    out.push('"');
    for c in text.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{08}' => out.push_str("\\b"),
            '\u{0c}' => out.push_str("\\f"),
            c if c.is_ascii() && !c.is_ascii_control() => out.push(c),
            c => {
                let mut units = [0u16; 2];
                for unit in c.encode_utf16(&mut units) {
                    let _ = write!(out, "\\u{unit:04x}");
                }
            }
        }
    }
    out.push('"');
}

#[cfg(test)]
mod tests {
    use super::canonical;
    use serde_json::json;

    #[test]
    fn matches_python_canonical_json() {
        // python3 -c 'import json; print(json.dumps({"b":[1,"é\n\u0001"],"a":None,"c":{"z":True,"y":"\U0001F600"}},
        //     sort_keys=True, separators=(",",":"), ensure_ascii=True))'
        let value = json!({"b": [1, "é\n\u{1}"], "a": null, "c": {"z": true, "y": "\u{1F600}"}});
        let expected = concat!(
            r#"{"a":null,"b":[1,""#,
            r"\u00e9\n\u0001",
            r#""],"c":{"y":""#,
            r"\ud83d\ude00",
            r#"","z":true}}"#
        );
        assert_eq!(canonical(&value), expected);
    }
}
