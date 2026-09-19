//! Frozen S05 protocol adapter. Production algorithms live in the library.

use std::collections::BTreeSet;
use std::fmt;
use std::io::{self, BufRead, Write};
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::{Arc, Mutex};

use serde::de::{self, MapAccess, SeqAccess, Visitor};
use serde::{Deserialize, Deserializer};
use serde_json::{json, Map, Value};
use ts_ast::SyntaxKind;
use ts_core::{LanguageVariant, ScriptTarget};
use ts_jsnum::Number;
use ts_scanner::{Checkpoint, DiagnosticArgument, ErrorCallback, Scanner, ScannerDiagnostic};

const MAX_RECORD: usize = 32 * 1024 * 1024;
const MAX_SOURCE: usize = 4 * 1024 * 1024;

/// Deserialize without map-key replacement or number coercion, recursively.
struct Strict(Value);
impl<'de> Deserialize<'de> for Strict {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct StrictVisitor;
        impl<'de> Visitor<'de> for StrictVisitor {
            type Value = Strict;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("strict integer-only JSON")
            }
            fn visit_bool<E: de::Error>(self, value: bool) -> Result<Strict, E> {
                Ok(Strict(value.into()))
            }
            fn visit_i64<E: de::Error>(self, value: i64) -> Result<Strict, E> {
                Ok(Strict(value.into()))
            }
            fn visit_u64<E: de::Error>(self, value: u64) -> Result<Strict, E> {
                i64::try_from(value)
                    .map(|value| Strict(value.into()))
                    .map_err(E::custom)
            }
            fn visit_str<E: de::Error>(self, value: &str) -> Result<Strict, E> {
                Ok(Strict(value.into()))
            }
            fn visit_string<E: de::Error>(self, value: String) -> Result<Strict, E> {
                Ok(Strict(value.into()))
            }
            fn visit_unit<E: de::Error>(self) -> Result<Strict, E> {
                Ok(Strict(Value::Null))
            }
            fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Strict, A::Error> {
                let mut values = Vec::new();
                while let Some(Strict(value)) = seq.next_element()? {
                    values.push(value);
                }
                Ok(Strict(Value::Array(values)))
            }
            fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Strict, A::Error> {
                let mut values = Map::new();
                while let Some((key, Strict(value))) = map.next_entry::<String, Strict>()? {
                    if values.insert(key.clone(), value).is_some() {
                        return Err(de::Error::custom(format!("duplicate key {key}")));
                    }
                }
                Ok(Strict(Value::Object(values)))
            }
        }
        deserializer.deserialize_any(StrictVisitor)
    }
}

fn hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut value = String::with_capacity(bytes.len() * 2);
    for &byte in bytes {
        value.push(char::from(DIGITS[usize::from(byte >> 4)]));
        value.push(char::from(DIGITS[usize::from(byte & 15)]));
    }
    value
}
fn unhex(value: &Value) -> Result<Vec<u8>, String> {
    let text = value.as_str().ok_or("expected hex string")?;
    if text.len() % 2 != 0 || text.len() > MAX_SOURCE * 2 {
        return Err("invalid hex length".into());
    }
    let nibble = |byte: u8| match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        _ => None,
    };
    text.as_bytes()
        .chunks_exact(2)
        .map(|pair| {
            Ok(nibble(pair[0]).ok_or("invalid lowercase hex")? * 16
                + nibble(pair[1]).ok_or("invalid lowercase hex")?)
        })
        .collect()
}
fn fields(value: &Value, expected: &str) -> Result<(), String> {
    let object = value.as_object().ok_or("expected object")?;
    let expected: BTreeSet<_> = expected.split_whitespace().collect();
    if object.keys().map(String::as_str).collect::<BTreeSet<_>>() != expected {
        return Err("unknown/missing object field".into());
    }
    Ok(())
}
fn target(value: i64) -> bool {
    (0..=12).contains(&value) || matches!(value, 99 | 100)
}
fn op_fields(op: &str) -> Option<&'static str> {
    Some(match op {
        "scan"
        | "scan_all"
        | "snapshot"
        | "rescan_less_than"
        | "rescan_greater_than"
        | "rescan_asterisk_equals"
        | "rescan_hash"
        | "rescan_question"
        | "scan_jsx"
        | "scan_jsx_identifier"
        | "scan_jsx_attribute"
        | "rescan_jsx_attribute"
        | "scan_jsdoc"
        | "reset"
        | "mark"
        | "rewind"
        | "commit"
        | "can_follow_jsdoc_at"
        | "identifier_token"
        | "valid_identifier"
        | "intrinsic_jsx_name"
        | "string_to_token"
        | "keyword_suggestions"
        | "shebang"
        | "normalize_jsdoc"
        | "number_from_string"
        | "pseudo_bigint" => "op",
        "rescan_template"
        | "rescan_jsx"
        | "scan_jsx_ex"
        | "scan_jsdoc_text"
        | "set_skip_trivia"
        | "set_skip_jsdoc_asterisks"
        | "set_on_error" => "op flag",
        "rescan_slash" => "op report_errors",
        "set_text" => "op text_hex",
        "reset_pos" | "reset_token_state" => "op pos",
        "set_variant" | "set_target" => "op value",
        "observe" => "op getter",
        "identifier_block" => "op first count",
        "identifier_point" => "op point",
        "identifier_text" => "op variant",
        "equal_fold" => "op other_hex",
        "token_to_string" => "op kind",
        "number_format" => "op bits",
        "skip_trivia" => "op pos options stop_after_line_break stop_at_comments in_jsdoc",
        "comment_ranges" => "op pos trailing",
        _ => return None,
    })
}
struct Request {
    json: Value,
    source: Vec<u8>,
    replacement: Vec<Option<Vec<u8>>>,
}
impl Request {
    fn parse(line: &[u8]) -> Result<Self, String> {
        let Strict(value) = serde_json::from_slice(line).map_err(|error| error.to_string())?;
        fields(
            &value,
            "version id source_hex decode_source target variant skip_trivia actions",
        )?;
        if value["version"].as_i64() != Some(1) {
            return Err("unknown version".into());
        }
        let id = value["id"].as_str().ok_or("invalid identity")?;
        if id.is_empty() || id.chars().count() > 1024 {
            return Err("invalid identity length".into());
        }
        let mut source = unhex(&value["source_hex"])?;
        let decode = value["decode_source"]
            .as_bool()
            .ok_or("invalid decode flag")?;
        value["skip_trivia"]
            .as_bool()
            .ok_or("invalid trivia flag")?;
        if !target(value["target"].as_i64().ok_or("invalid target")?)
            || !matches!(value["variant"].as_i64(), Some(0 | 1))
        {
            return Err("invalid target/variant".into());
        }
        let actions = value["actions"].as_array().ok_or("invalid actions")?;
        if actions.is_empty() || actions.len() > 4096 {
            return Err("invalid action count".into());
        }
        let mut replacement = Vec::with_capacity(actions.len());
        let mut marks = 0_i32;
        for action in actions {
            let op = action["op"].as_str().ok_or("invalid operation")?;
            fields(action, op_fields(op).ok_or("unknown operation")?)?;
            let mut bytes = None;
            for (key, arg) in action.as_object().expect("validated object") {
                match key.as_str() {
                    "op" | "getter" | "report_errors" => {
                        arg.as_str().ok_or("invalid string argument")?;
                    }
                    "flag"
                    | "options"
                    | "stop_after_line_break"
                    | "stop_at_comments"
                    | "in_jsdoc"
                    | "trailing" => {
                        arg.as_bool().ok_or("invalid bool argument")?;
                    }
                    "text_hex" | "other_hex" => {
                        bytes = Some(unhex(arg)?);
                    }
                    "bits" => {
                        if unhex(arg)?.len() != 8 {
                            return Err("invalid float bits".into());
                        }
                    }
                    _ => {
                        arg.as_i64().ok_or("invalid integer argument")?;
                    }
                }
            }
            match op {
                "rescan_slash"
                    if !matches!(
                        action["report_errors"].as_str(),
                        Some("omitted" | "false" | "true")
                    ) =>
                {
                    return Err("invalid reporting mode".into());
                }
                "set_variant" if !matches!(action["value"].as_i64(), Some(0 | 1)) => {
                    return Err("invalid variant".into());
                }
                "identifier_text" if !matches!(action["variant"].as_i64(), Some(0 | 1)) => {
                    return Err("invalid identifier variant".into());
                }
                "set_target" if !target(action["value"].as_i64().expect("validated integer")) => {
                    return Err("invalid target".into());
                }
                "observe"
                    if !matches!(
                        action["getter"].as_str(),
                        Some(
                            "text"
                                | "token"
                                | "flags"
                                | "full_start"
                                | "start"
                                | "end"
                                | "token_text"
                                | "value"
                                | "range"
                                | "directives"
                                | "predicates"
                        )
                    ) =>
                {
                    return Err("invalid getter".into());
                }
                "identifier_point"
                    if i32::try_from(action["point"].as_i64().expect("validated integer"))
                        .is_err() =>
                {
                    return Err("invalid rune".into());
                }
                "identifier_block" => {
                    let first = action["first"].as_i64().expect("validated integer");
                    let count = action["count"].as_i64().expect("validated integer");
                    if !(0..=0x10_ffff).contains(&first)
                        || !(1..=4096).contains(&count)
                        || first + count > 0x11_0000
                    {
                        return Err("invalid identifier block".into());
                    }
                }
                "token_to_string" if !matches!(action["kind"].as_i64(), Some(0..=350)) => {
                    return Err("invalid kind".into());
                }
                "mark" => marks += 1,
                "rewind" | "commit" => {
                    marks -= 1;
                    if marks < 0 {
                        return Err("checkpoint underflow".into());
                    }
                }
                _ => {}
            }
            replacement.push(bytes);
        }
        if marks != 0 {
            return Err("unclosed checkpoints".into());
        }
        if decode {
            source = ts_jsstring::SourceText::from_bytes(source)
                .as_bytes()
                .to_vec();
        }
        Ok(Self {
            json: value,
            source,
            replacement,
        })
    }
    fn actions(&self) -> &[Value] {
        self.json["actions"].as_array().expect("validated actions")
    }
}

type Sink = Arc<Mutex<Vec<ScannerDiagnostic>>>;
fn callback(sink: &Sink) -> ErrorCallback<'static> {
    let sink = Arc::clone(sink);
    Box::new(move |diagnostic| {
        sink.lock()
            .expect("diagnostic sink is not held while scanning")
            .push(diagnostic);
    })
}
fn diagnostic(value: ScannerDiagnostic) -> Value {
    let args: Vec<_> = value
        .args
        .into_iter()
        .map(|arg| match arg {
            DiagnosticArgument::String(bytes) => json!({"kind":"string","hex":hex(&bytes)}),
            DiagnosticArgument::Integer(value) => json!({"kind":"integer","value":value}),
        })
        .collect();
    json!({"code":value.message.code,"category":value.message.category as i32,"key":value.message.key,"start":value.start,"length":value.length,"args":args})
}
fn directives(s: &Scanner<'_>) -> Value {
    Value::Array(
        s.comment_directives()
            .iter()
            .map(|d| json!({"start":d.loc.pos(),"end":d.loc.end(),"kind":d.kind as i32}))
            .collect(),
    )
}
fn predicates(s: &Scanner<'_>) -> Value {
    json!([
        s.has_unicode_escape(),
        s.has_extended_unicode_escape(),
        s.has_preceding_line_break(),
        s.has_preceding_jsdoc_comment(),
        s.has_preceding_jsdoc_leading_asterisks(),
        s.has_preceding_jsdoc_with_deprecated_tag(),
        s.has_preceding_jsdoc_with_see_or_link()
    ])
}
fn snapshot(s: &Scanner<'_>, kind: SyntaxKind) -> Value {
    json!({"kind":kind as u16,"token":s.token() as u16,"full_start":s.token_full_start(),"start":s.token_start(),"end":s.token_end(),"text_hex":hex(s.token_text()),"value_hex":hex(s.token_value()),"flags":s.token_flags(),"range":[s.token_range().pos(),s.token_range().end()],"predicates":predicates(s),"directives":directives(s)})
}
fn number(value: Number) -> Value {
    let n = value.value();
    let class = if n.is_nan() {
        "nan"
    } else if n == f64::INFINITY {
        "positive_infinity"
    } else if n == f64::NEG_INFINITY {
        "negative_infinity"
    } else {
        "finite"
    };
    let bits = (!n.is_nan()).then(|| format!("{:016x}", n.to_bits()));
    json!({"number_class":class,"bits":bits,"text_hex":hex(value.to_string().as_bytes())})
}
fn execute<'a>(
    s: &mut Scanner<'a>,
    marks: &mut Vec<Checkpoint<'a>>,
    request: &'a Request,
    index: usize,
    sink: &Sink,
) -> Value {
    let action = &request.actions()[index];
    let op = action["op"].as_str().expect("validated op");
    let integer = |key: &str| action[key].as_i64().expect("validated integer");
    let flag = |key: &str| action[key].as_bool().expect("validated boolean");
    let raw = || {
        request.replacement[index]
            .as_deref()
            .expect("validated byte argument")
    };
    let kind = match op {
        "scan" | "scan_all" => s.scan(),
        "snapshot" => s.token(),
        "rescan_less_than" => s.rescan_less_than_token(),
        "rescan_greater_than" => s.rescan_greater_than_token(),
        "rescan_asterisk_equals" => s.rescan_asterisk_equals_token(),
        "rescan_hash" => s.rescan_hash_token(),
        "rescan_question" => s.rescan_question_token(),
        "rescan_template" => s.rescan_template_token(flag("flag")),
        "rescan_slash" => s.rescan_slash_token(action["report_errors"] == "true"),
        "rescan_jsx" => s.rescan_jsx_token(flag("flag")),
        "scan_jsx" => s.scan_jsx_token(),
        "scan_jsx_ex" => s.scan_jsx_token_ex(flag("flag")),
        "scan_jsx_identifier" => s.scan_jsx_identifier(),
        "scan_jsx_attribute" => s.scan_jsx_attribute_value(),
        "rescan_jsx_attribute" => s.rescan_jsx_attribute_value(),
        "scan_jsdoc" => s.scan_jsdoc_token(),
        "scan_jsdoc_text" => s.scan_jsdoc_comment_text_token(flag("flag")),
        "can_follow_jsdoc_at" => return json!(s.can_follow_jsdoc_at()),
        "reset" => {
            s.reset();
            return Value::Null;
        }
        "set_text" => {
            s.set_text(raw());
            return Value::Null;
        }
        "reset_pos" => {
            s.reset_pos(integer("pos"));
            return Value::Null;
        }
        "reset_token_state" => {
            s.reset_token_state(integer("pos"));
            return Value::Null;
        }
        "set_skip_trivia" => {
            s.set_skip_trivia(flag("flag"));
            return Value::Null;
        }
        "set_skip_jsdoc_asterisks" => {
            s.set_skip_jsdoc_leading_asterisks(flag("flag"));
            return Value::Null;
        }
        "set_variant" => {
            s.set_language_variant(LanguageVariant(integer("value") as i32));
            return Value::Null;
        }
        "set_target" => {
            s.set_script_target(ScriptTarget(integer("value") as i32));
            return Value::Null;
        }
        "set_on_error" => {
            s.set_on_error(flag("flag").then(|| callback(sink)));
            return Value::Null;
        }
        "mark" => {
            marks.push(s.mark());
            return Value::Null;
        }
        "rewind" => {
            s.rewind(marks.pop().expect("validated LIFO checkpoint"));
            return Value::Null;
        }
        "commit" => {
            s.commit(marks.pop().expect("validated LIFO checkpoint"));
            return Value::Null;
        }
        "observe" => {
            return match action["getter"].as_str().expect("validated getter") {
                "text" => json!(hex(s.text())),
                "token" => json!(s.token() as u16),
                "flags" => json!(s.token_flags()),
                "full_start" => json!(s.token_full_start()),
                "start" => json!(s.token_start()),
                "end" => json!(s.token_end()),
                "token_text" => json!(hex(s.token_text())),
                "value" => json!(hex(s.token_value())),
                "range" => json!([s.token_range().pos(), s.token_range().end()]),
                "directives" => directives(s),
                "predicates" => predicates(s),
                _ => unreachable!("validated getter"),
            };
        }
        "identifier_point" => {
            let ch = integer("point") as i32;
            return json!([
                ts_scanner::is_identifier_start(ch),
                ts_scanner::is_identifier_part(ch),
                ts_scanner::is_identifier_part_ex(ch, LanguageVariant::JSX)
            ]);
        }
        "identifier_block" => {
            let first = integer("first") as i32;
            let count = integer("count") as usize;
            let mut start = vec![0_u8; count.div_ceil(8)];
            let mut part = start.clone();
            let mut jsx = start.clone();
            for i in 0..count {
                let ch = first + i as i32;
                let mask = 1 << (i % 8);
                if ts_scanner::is_identifier_start(ch) {
                    start[i / 8] |= mask;
                }
                if ts_scanner::is_identifier_part(ch) {
                    part[i / 8] |= mask;
                }
                if ts_scanner::is_identifier_part_ex(ch, LanguageVariant::JSX) {
                    jsx[i / 8] |= mask;
                }
            }
            return json!({"start_hex":hex(&start),"part_hex":hex(&part),"jsx_hex":hex(&jsx)});
        }
        "equal_fold" => return json!(ts_scanner::equal_fold(s.text(), raw())),
        "identifier_token" => return json!(ts_scanner::get_identifier_token(s.text()) as u16),
        "valid_identifier" => return json!(ts_scanner::is_valid_identifier(s.text())),
        "identifier_text" => {
            return json!(ts_scanner::is_identifier_text(
                s.text(),
                LanguageVariant(integer("variant") as i32)
            ));
        }
        "intrinsic_jsx_name" => return json!(ts_scanner::is_intrinsic_jsx_name(s.text())),
        "string_to_token" => return json!(ts_scanner::string_to_token(s.text()) as u16),
        "token_to_string" => {
            return json!(hex(ts_scanner::token_to_string(
                SyntaxKind::from_u16(integer("kind") as u16).expect("validated kind")
            )
            .as_bytes()));
        }
        "keyword_suggestions" => {
            let mut values = ts_scanner::get_viable_keyword_suggestions();
            values.sort_unstable();
            return json!(values
                .into_iter()
                .map(|value| hex(value.as_bytes()))
                .collect::<Vec<_>>());
        }
        "shebang" => return json!(hex(ts_scanner::get_shebang(s.text()))),
        "normalize_jsdoc" => {
            return json!(hex(&ts_scanner::normalize_jsdoc_type_source_text(s.text())));
        }
        "skip_trivia" => {
            let options = ts_scanner::SkipTriviaOptions {
                stop_after_line_break: flag("stop_after_line_break"),
                stop_at_comments: flag("stop_at_comments"),
                in_jsdoc: flag("in_jsdoc"),
            };
            return json!(ts_scanner::skip_trivia_ex(
                s.text(),
                integer("pos"),
                flag("options").then_some(&options)
            ));
        }
        "comment_ranges" => {
            let values: Vec<_> = if flag("trailing") {
                ts_scanner::get_trailing_comment_ranges(s.text(), integer("pos")).collect()
            } else {
                ts_scanner::get_leading_comment_ranges(s.text(), integer("pos")).collect()
            };
            return Value::Array(values.into_iter().map(|value|json!({"start":value.loc.pos(),"end":value.loc.end(),"kind":value.kind as u16,"trailing_newline":value.has_trailing_new_line})).collect());
        }
        "number_from_string" => return number(ts_jsnum::from_string(s.text())),
        "number_format" => {
            return number(Number::new(f64::from_bits(
                u64::from_str_radix(action["bits"].as_str().expect("validated bits"), 16)
                    .expect("validated bits"),
            )));
        }
        "pseudo_bigint" => return json!(hex(&ts_jsnum::parse_pseudo_big_int(s.text()))),
        _ => unreachable!("validated operation"),
    };
    snapshot(s, kind)
}
fn panic_value(action: &Value, message: &str, source: &[u8]) -> Value {
    let op = action["op"].as_str().expect("validated operation");
    let mut input = None;
    let class = match message {
        "Cannot reset token state to negative position"
        | "'ReScanAsteriskEqualsToken' should only be called on a '*='"
        | "'reScanQuestionToken' should only be called on a '??'" => "contract",
        "Debug failure. False expression." => "upstream_assertion",
        _ => {
            let expected = hex(source.strip_suffix(b"n").unwrap_or(source));
            if op == "pseudo_bigint"
                && message == format!("Failed to parse big int (hex): {expected}")
            {
                input = Some(expected);
                "invalid_bigint"
            } else if message.starts_with("index out of bounds: the len is ")
                || message.starts_with("range start index ")
                || message.starts_with("range end index ")
                || message.starts_with("slice index starts at ")
            {
                "bounds"
            } else {
                "unexpected"
            }
        }
    };
    json!({"message":message,"class":class,"input_hex":input})
}
fn write_record(out: &mut impl Write, value: &Value) -> Result<(), String> {
    let bytes = serde_json::to_vec(value).map_err(|error| error.to_string())?;
    if bytes.len() + 1 > MAX_RECORD {
        return Err("oversized response record".into());
    }
    out.write_all(&bytes)
        .and_then(|()| out.write_all(b"\n"))
        .map_err(|error| error.to_string())
}
fn run(request: &Request, out: &mut impl Write) -> Result<(), String> {
    let id = &request.json["id"];
    write_record(out, &json!({"event":"begin","id":id,"version":1}))?;
    let sink = Arc::new(Mutex::new(Vec::new()));
    let mut s = Scanner::new();
    let mut marks = Vec::new();
    s.set_text(&request.source);
    s.set_on_error(Some(callback(&sink)));
    s.set_script_target(ScriptTarget(
        request.json["target"].as_i64().expect("validated target") as i32,
    ));
    s.set_language_variant(LanguageVariant(
        request.json["variant"].as_i64().expect("validated variant") as i32,
    ));
    s.set_skip_trivia(
        request.json["skip_trivia"]
            .as_bool()
            .expect("validated bool"),
    );
    let mut ordinal = 0;
    let mut completed = 0;
    let mut stopped = false;
    for (index, action) in request.actions().iter().enumerate() {
        let mut count = 0;
        loop {
            let outcome = catch_unwind(AssertUnwindSafe(|| {
                execute(&mut s, &mut marks, request, index, &sink)
            }));
            let (status, value) = match outcome {
                Ok(value) => ("ok", value),
                Err(payload) => {
                    stopped = true;
                    let message = payload
                        .downcast_ref::<String>()
                        .cloned()
                        .or_else(|| {
                            payload
                                .downcast_ref::<&str>()
                                .map(|value| (*value).to_owned())
                        })
                        .unwrap_or_else(|| "non-string panic payload".into());
                    ("panic", panic_value(action, &message, s.text()))
                }
            };
            let diagnostics: Vec<_> =
                std::mem::take(&mut *sink.lock().expect("callback lock released"))
                    .into_iter()
                    .map(diagnostic)
                    .collect();
            write_record(
                out,
                &json!({"event":"observation","id":id,"action":index,"ordinal":ordinal,"status":status,"value":value,"diagnostics":diagnostics}),
            )?;
            ordinal += 1;
            count += 1;
            if stopped || action["op"] != "scan_all" || s.token() == SyntaxKind::EndOfFile {
                break;
            }
            if count >= s.text().len() + 2 {
                return Err(format!(
                    "case {id} action {index} exceeded scan token bound"
                ));
            }
        }
        completed = index + 1;
        if stopped {
            break;
        }
    }
    write_record(
        out,
        &json!({"event":"end","id":id,"observations":ordinal,"completed_actions":completed}),
    )?;
    out.flush().map_err(|error| error.to_string())
}
fn serve(input: &mut impl BufRead, out: &mut impl Write) -> Result<(), String> {
    loop {
        let mut line = Vec::new();
        loop {
            let buffer = input.fill_buf().map_err(|error| error.to_string())?;
            if buffer.is_empty() {
                if line.is_empty() {
                    return Ok(());
                }
                return Err("incomplete request framing".into());
            }
            let end = buffer.iter().position(|&byte| byte == b'\n');
            let count = end.map_or(buffer.len(), |index| index + 1);
            if line.len() + count > MAX_RECORD {
                return Err("oversized request record".into());
            }
            line.extend_from_slice(&buffer[..count]);
            input.consume(count);
            if end.is_some() {
                break;
            }
        }
        let request = Request::parse(&line)?;
        run(&request, out)?;
    }
}
fn main() {
    let result = serve(
        &mut io::stdin().lock(),
        &mut io::BufWriter::new(io::stdout().lock()),
    );
    if let Err(error) = result {
        eprintln!("S05 Rust adapter: {error}");
        std::process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wire(source: &[u8], actions: Value) -> Vec<u8> {
        serde_json::to_vec(&json!({"version":1,"id":"test","source_hex":hex(source),"decode_source":false,"target":0,"variant":0,"skip_trivia":true,"actions":actions})).unwrap()
    }

    #[test]
    fn rejects_duplicate_keys_types_missing_fields_and_illegal_checkpoints() {
        for actions in [
            json!([]),
            json!([{"op":"scan","flag":true}]),
            json!([{"op":"missing"}]),
            json!([{"op":"reset_pos","pos":true}]),
            json!([{"op":"reset_pos","pos":9223372036854775808_u64}]),
            json!([{"op":"identifier_point","point":2147483648_i64}]),
            json!([{"op":"identifier_block","first":1114111,"count":2}]),
            json!([{"op":"rescan_slash","report_errors":true}]),
            json!([{"op":"rescan_slash","report_errors":"maybe"}]),
            json!([{"op":"observe","getter":"secret"}]),
            json!([{"op":"mark"}]),
            json!([{"op":"rewind"}]),
        ] {
            assert!(Request::parse(&wire(b"", actions)).is_err());
        }
        let valid = String::from_utf8(wire(b"", json!([{"op":"scan"}]))).unwrap();
        for raw in [
            valid.replace("\"version\":1", "\"version\":true"),
            valid.replace("\"version\":1", "\"version\":1.0"),
            valid.replace("\"id\":\"test\"", "\"id\":\"test\",\"id\":\"other\""),
            valid.replace("\"op\":\"scan\"", "\"op\":\"scan\",\"op\":\"reset\""),
            valid.replace("\"source_hex\":\"\"", "\"source_hex\":\"FF\""),
            valid.replace("\"source_hex\":\"\"", "\"source_hex\":\"f\""),
            valid.replace("\"variant\":0", "\"variant\":2"),
            valid.replace("\"target\":0", "\"target\":13"),
            valid.replace("\"id\":\"test\"", "\"id\":null"),
            format!("{valid}{{}}"),
        ] {
            assert!(Request::parse(raw.as_bytes()).is_err(), "{raw}");
        }
        assert!(Request::parse(&wire(
            b"",
            json!([{"op":"mark"},{"op":"mark"},{"op":"commit"},{"op":"rewind"}])
        ))
        .is_ok());
    }

    #[test]
    fn fresh_requests_restore_diagnostics_and_truncated_input_is_not_executed() {
        let mut first = wire(
            b"/a/zz",
            json!([{"op":"set_on_error","flag":false},{"op":"scan"},{"op":"rescan_slash","report_errors":"true"}]),
        );
        let second = wire(
            b"/a/zz",
            json!([{"op":"scan"},{"op":"rescan_slash","report_errors":"true"}]),
        );
        first.push(b'\n');
        first.extend_from_slice(&second);
        first.push(b'\n');
        let mut output = Vec::new();
        serve(&mut first.as_slice(), &mut output).unwrap();
        let counts: Vec<_> = output
            .split(|&byte| byte == b'\n')
            .filter(|line| !line.is_empty())
            .map(|line| serde_json::from_slice::<Value>(line).unwrap())
            .filter(|record| record["event"] == "observation")
            .map(|record| record["diagnostics"].as_array().unwrap().len())
            .collect();
        assert_eq!(counts, [0, 0, 0, 0, 2]);
        output.clear();
        assert!(serve(&mut second.as_slice(), &mut output).is_err());
        assert!(output.is_empty());
    }

    #[test]
    fn bigint_panic_retains_actual_input_bytes() {
        let request = Request::parse(&wire(b"0x\xffn", json!([{"op":"pseudo_bigint"}]))).unwrap();
        let mut output = Vec::new();
        run(&request, &mut output).unwrap();
        let observation: Value =
            serde_json::from_slice(output.split(|&byte| byte == b'\n').nth(1).unwrap()).unwrap();
        assert_eq!(observation["status"], "panic");
        assert_eq!(observation["value"]["class"], "invalid_bigint");
        assert_eq!(observation["value"]["input_hex"], "3078ff");
        assert_eq!(
            observation["value"]["message"],
            "Failed to parse big int (hex): 3078ff"
        );
    }
    #[test]
    fn decoded_source_controls_panic_payload() {
        for source in [
            b"\xff\xfe0\x00b\x002\x00n\x00".as_slice(),
            b"\xfe\xff\x000\x00b\x002\x00n",
            b"\xef\xbb\xbf0b2n",
        ] {
            let raw = String::from_utf8(wire(source, json!([{"op":"pseudo_bigint"}])))
                .unwrap()
                .replace("\"decode_source\":false", "\"decode_source\":true");
            let request = Request::parse(raw.as_bytes()).unwrap();
            let mut output = Vec::new();
            run(&request, &mut output).unwrap();
            let observation: Value =
                serde_json::from_slice(output.split(|&byte| byte == b'\n').nth(1).unwrap())
                    .unwrap();
            assert_eq!(observation["status"], "panic");
            assert_eq!(observation["value"]["class"], "invalid_bigint");
            assert_eq!(observation["value"]["input_hex"], "306232");
            assert_eq!(
                observation["value"]["message"],
                "Failed to parse big int (hex): 306232"
            );
        }
    }
}
