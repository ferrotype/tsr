use crate::api;
use serde_json::{json, Value};
use std::{collections::HashMap, io::Read};
use tsr_json::{Decode, Decoder, Encode, Encoder, Error, Kind, Options, RawValue, Token};
use tsr_jsstring::JsString;

fn fallback<'a>(action: &'a Value, name: &str, default: &'a str) -> &'a str {
    let text = api::action_str(action, name);
    if text.is_empty() {
        default
    } else {
        text
    }
}
fn hex(bytes: &[u8]) -> String {
    {
        use std::fmt::Write;
        let mut out = String::with_capacity(bytes.len() * 2);
        for b in bytes {
            write!(out, "{b:02x}").unwrap();
        }
        out
    }
}
fn unhex(text: &str) -> Result<Vec<u8>, String> {
    if !text.is_ascii() || !text.len().is_multiple_of(2) {
        return Err("invalid hex input".into());
    }
    (0..text.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&text[i..i + 2], 16).map_err(|e| e.to_string()))
        .collect()
}
fn bytes(row: &mut Value, name: &str, data: &[u8]) {
    row[format!("{name}_hex")] = json!(hex(data));
    row[format!("{name}_len")] = json!(data.len());
    if let Ok(text) = std::str::from_utf8(data) {
        row[format!("{name}_text")] = json!(text);
    }
}
fn error(row: &mut Value, result: Result<(), Error>) {
    row["ok"] = json!(result.is_ok());
    let Err(e) = result else {
        return;
    };
    row["error"] = json!(e.to_string());
    row["error_unexpected_eof"] = json!(e.is_unexpected_eof());
    match e {
        Error::Syntax(s) => {
            row["error_class"] = json!("SyntacticError");
            row["error_byte_offset"] = json!(s.offset);
            row["error_json_pointer"] = json!(s.pointer);
        }
        Error::Semantic(s) => {
            row["error_class"] = json!("SemanticError");
            row["error_byte_offset"] = json!(s.offset);
            row["error_json_pointer"] = json!(s.pointer);
            row["error_json_kind"] = json!(s.kind.name());
            row["error_go_type"] = json!(s.type_name);
            if let Some(value) = s.value {
                row["error_json_value_hex"] = json!(hex(&value));
            }
            if let Some(cause) = s.cause {
                row["error_cause"] = json!(cause.to_string());
                if let Error::Syntax(cause) = cause {
                    row["error_cause_byte_offset"] = json!(cause.offset);
                    row["error_cause_json_pointer"] = json!(cause.pointer);
                }
            }
        }
        Error::Eof => row["error_class"] = json!("EOF"),
        _ => row["error_class"] = json!("*errors.errorString"),
    }
}
#[derive(Default)]
struct Sample {
    name: String,
    count: Option<isize>,
    items: Option<Vec<String>>,
    extra: String,
}
impl Sample {
    fn new(shape: &str) -> Result<Self, String> {
        Ok(match shape {
            "" | "zero" => Self::default(),
            "empty_slice" => Self {
                items: Some(vec![]),
                ..Self::default()
            },
            "populated" => Self {
                name: "a".into(),
                count: Some(7),
                items: Some(vec!["x".into(), "y".into()]),
                extra: "e".into(),
            },
            _ => return Err(format!("unknown sample shape {shape:?}")),
        })
    }
}
impl Encode for Sample {
    fn type_name(&self) -> &'static str {
        "json.sampleFields"
    }
    fn encode(&self, out: &mut Encoder<'_>) -> Result<(), Error> {
        out.write_token(Token::BeginObject)?;
        out.string(b"name")?;
        out.value(&self.name)?;
        out.string(b"count")?;
        match self.count {
            Some(n) => out.int(n as i64)?,
            None => out.null()?,
        }
        out.string(b"items")?;
        out.array(self.items.iter().flatten())?;
        if !self.extra.is_empty() {
            out.string(b"extra")?;
            out.value(&self.extra)?;
        }
        out.write_token(Token::EndObject)
    }
}
impl Decode for Sample {
    fn decode(&mut self, d: &mut Decoder<'_>) -> Result<(), Error> {
        if d.peek_kind() == Kind::Null {
            d.read_token()?;
            *self = Self::default();
            return Ok(());
        }
        d.object(|name, d| match name {
            b"name" => d.value(&mut self.name),
            b"count" => d.value(&mut self.count),
            b"items" => d.value(&mut self.items),
            b"extra" => d.value(&mut self.extra),
            _ => d.skip_value(),
        })
    }
}
enum Target {
    Raw(RawValue),
    Float(f64),
    Int(i64),
    Uint(u64),
    Sample(Sample),
}
impl Target {
    fn new(kind: &str, preset: &str) -> Result<Self, String> {
        if !preset.is_empty() && kind != "sample" {
            return Err("only the sample takes a preset".into());
        }
        Ok(match kind {
            "raw_value" => Self::Raw(RawValue::default()),
            "float64" => Self::Float(0.0),
            "int64" => Self::Int(0),
            "uint64" => Self::Uint(0),
            "sample" => Self::Sample(Sample::new(preset)?),
            _ => return Err(format!("unknown target {kind}")),
        })
    }
    fn render(&self, row: &mut Value) {
        match self {
            Self::Raw(raw) => {
                bytes(row, "target", &raw.0);
                row["target_kind"] = json!(raw.kind().name());
            }
            Self::Float(value) => {
                row["target_float"] = json!(float_text(*value));
                row["target_float_bits"] = json!(value.to_bits());
            }
            Self::Int(value) => row["target_int"] = json!(value),
            Self::Uint(value) => row["target_uint"] = json!(value),
            Self::Sample(value) => {
                row["target_name"] = json!(value.name);
                row["target_extra"] = json!(value.extra);
                row["target_count_nil"] = json!(value.count.is_none());
                if let Some(n) = value.count {
                    row["target_count"] = json!(n);
                }
                row["target_items_nil"] = json!(value.items.is_none());
                row["target_items_len"] = json!(value.items.as_ref().map_or(0, Vec::len));
                match tsr_json::marshal(value, Options::default()) {
                    Ok(output) => bytes(row, "target_render", &output),
                    Err(e) => row["target_render_error"] = json!(e.to_string()),
                }
            }
        }
    }
}
impl Decode for Target {
    fn decode(&mut self, d: &mut Decoder<'_>) -> Result<(), Error> {
        match self {
            Self::Raw(x) => d.value(x),
            Self::Float(x) => d.value(x),
            Self::Int(x) => d.value(x),
            Self::Uint(x) => d.value(x),
            Self::Sample(x) => d.value(x),
        }
    }
}
// Probe-envelope formatting: Go's strconv 'g', not JavaScript Number.String.
fn float_text(value: f64) -> String {
    if value.is_nan() {
        return "NaN".into();
    }
    if value.is_infinite() {
        return if value.is_sign_negative() {
            "-Inf"
        } else {
            "+Inf"
        }
        .into();
    }
    let scientific = format!("{value:e}");
    let (mantissa, exponent) = scientific.split_once('e').expect("exponential display");
    let exponent: i32 = exponent.parse().expect("integer exponent");
    if (-4..6).contains(&exponent) {
        value.to_string()
    } else {
        format!("{mantissa}e{exponent:+03}")
    }
}
struct Chunks {
    data: Vec<u8>,
    pos: usize,
    chunk: usize,
}
impl Read for Chunks {
    fn read(&mut self, out: &mut [u8]) -> std::io::Result<usize> {
        let n = (self.data.len() - self.pos)
            .min(out.len())
            .min(if self.chunk == 0 {
                usize::MAX
            } else {
                self.chunk
            });
        out[..n].copy_from_slice(&self.data[self.pos..self.pos + n]);
        self.pos += n;
        Ok(n)
    }
}
struct IntegerRecord;
impl Encode for IntegerRecord {
    fn encode(&self, out: &mut Encoder<'_>) -> Result<(), Error> {
        out.write_token(Token::BeginObject)?;
        out.string(b"name")?;
        out.value("worker")?;
        out.string(b"counter")?;
        out.value(&u64::MAX)?;
        out.string(b"active")?;
        out.value(&true)?;
        out.write_token(Token::EndObject)
    }
}
fn build(kind: &str, text: &str, raw: &[u8], request: &Value) -> Result<Box<dyn Encode>, String> {
    Ok(match kind {
        "raw" => Box::new(RawValue(raw.to_vec())),
        "string" => Box::new(JsString::from_bytes(raw)),
        "float" => Box::new(text.parse::<f64>().map_err(|e| e.to_string())?),
        "int64" => Box::new(text.parse::<i64>().map_err(|e| e.to_string())?),
        "uint64" => Box::new(text.parse::<u64>().map_err(|e| e.to_string())?),
        "sample" => Box::new(Sample::new(text)?),
        "integer_record" => Box::new(IntegerRecord),
        "float_list" => Box::new(vec![1.0, f64::INFINITY]),
        "map" => {
            let mut map = HashMap::<String, String>::new();
            for pair in request["value"]["entries"]
                .as_array()
                .ok_or("missing map entries")?
            {
                map.insert(
                    pair[0].as_str().ok_or("string key")?.into(),
                    pair[1].as_str().ok_or("string value")?.into(),
                );
            }
            Box::new(map)
        }
        _ => return Err(format!("unknown value kind {kind}")),
    })
}
pub(super) fn replay(request: &Value) -> Result<Vec<Value>, String> {
    let mut options = Options::default();
    for option in request["options"].as_array().into_iter().flatten() {
        match api::action_str(option, "option") {
            "Deterministic" => {
                options.deterministic =
                    Some(option["enabled"].as_bool().ok_or("enabled must be bool")?);
            }
            "AllowDuplicateNames" => {
                options.allow_duplicate_names =
                    Some(option["enabled"].as_bool().ok_or("enabled must be bool")?);
            }
            "WithIndent" => options.indent = Some(api::action_str(option, "indent")),
            _ => return Err("unknown JSON option".into()),
        }
    }
    let mut rows = Vec::new();
    let mut written = Vec::new();
    let mut encoder = None;
    let mut decoder = None;
    let mut held: Option<(String, Target)> = None;
    for action in api::actions(request) {
        let op = api::action_op(action);
        let kind = fallback(
            action,
            "value_kind",
            api::action_str(&request["value"], "kind"),
        );
        let text = fallback(
            action,
            "value_text",
            api::action_str(&request["value"], "text"),
        );
        let target = fallback(action, "target", api::action_str(request, "target"));
        let raw = unhex(fallback(
            action,
            "input_hex",
            api::action_str(request, "input_hex"),
        ))?;
        let chunk = action["chunk"].as_u64().unwrap_or(0) as usize;
        let mut row = json!({"op":op});
        match op {
            "marshal"
            | "marshal_indent"
            | "marshal_write"
            | "marshal_indent_write"
            | "marshal_encode" => {
                let input = build(kind, text, &raw, request)?;
                row["value_kind"] = json!(kind);
                let prefix = api::action_str(action, "prefix");
                let indent = api::action_str(action, "indent");
                if matches!(op, "marshal_indent" | "marshal_indent_write") {
                    row["prefix"] = json!(prefix);
                    row["indent"] = json!(indent);
                }
                let (result, output) = match op {
                    "marshal" | "marshal_indent" => {
                        let opts = if op == "marshal" {
                            options
                        } else {
                            Options {
                                indent: (!(prefix.is_empty() && indent.is_empty()))
                                    .then_some(indent),
                                prefix: Some(prefix),
                                ..Options::default()
                            }
                        };
                        let (out, result) = tsr_json::marshal_partial(input.as_ref(), opts);
                        (result, out)
                    }
                    "marshal_encode" => {
                        let e: &mut Encoder<'_> = encoder
                            .as_mut()
                            .ok_or("marshal_encode before new_encoder")?;
                        let before = e.written_bytes().len();
                        let result = tsr_json::marshal_encode(e, input.as_ref(), options);
                        (result, e.written_bytes()[before..].to_vec())
                    }
                    _ => {
                        let before = written.len();
                        let result = if op == "marshal_write" {
                            tsr_json::marshal_write(&mut written, input.as_ref(), options)
                        } else {
                            tsr_json::marshal_indent_write(
                                &mut written,
                                input.as_ref(),
                                prefix,
                                indent,
                            )
                        };
                        (result, written[before..].to_vec())
                    }
                };
                error(&mut row, result);
                bytes(&mut row, "output", &output);
            }
            "new_encoder" => {
                written.clear();
                encoder = Some(Encoder::new(Options::default()).map_err(|e| e.to_string())?);
                row["ok"] = json!(true);
            }
            "encoder_write_token" => {
                let e = encoder.as_mut().ok_or("token before new_encoder")?;
                let name = api::action_str(action, "token");
                row["token"] = json!(name);
                let token = match name {
                    "begin_object" => Token::BeginObject,
                    "end_object" => Token::EndObject,
                    "begin_array" => Token::BeginArray,
                    "end_array" => Token::EndArray,
                    "null" => Token::Null,
                    _ => return Err("unknown token".into()),
                };
                error(&mut row, e.write_token(token));
            }
            "encoder_offset" => {
                let e = encoder.as_ref().ok_or("offset before new_encoder")?;
                row["output_offset"] = json!(e.output_offset());
                row["stack_depth"] = json!(e.stack_depth());
            }
            "written_bytes" => bytes(
                &mut row,
                "output",
                encoder
                    .as_ref()
                    .map_or(written.as_slice(), Encoder::written_bytes),
            ),
            "new_target" => {
                let preset = api::action_str(action, "preset");
                let t = Target::new(target, preset)?;
                row["target"] = json!(target);
                row["preset"] = json!(preset);
                row["ok"] = json!(true);
                t.render(&mut row);
                held = Some((target.into(), t));
            }
            "unmarshal" | "unmarshal_read" => {
                let reuse = action["reuse"].as_bool().unwrap_or(false);
                let mut fresh = Target::new(target, "")?;
                let t = if reuse {
                    let (name, t) = held.as_mut().ok_or("reuse before new_target")?;
                    if name != target {
                        return Err("target differs from held target".into());
                    }
                    t
                } else {
                    &mut fresh
                };
                row["target"] = json!(target);
                row["reuse"] = json!(reuse);
                bytes(&mut row, "input", &raw);
                if op == "unmarshal" {
                    error(&mut row, tsr_json::unmarshal(&raw, t, options));
                } else {
                    row["chunk"] = json!(chunk);
                    let mut reader = Chunks {
                        data: raw,
                        pos: 0,
                        chunk,
                    };
                    error(&mut row, tsr_json::unmarshal_read(&mut reader, t, options));
                    row["unread_len"] = json!(reader.data.len() - reader.pos);
                }
                t.render(&mut row);
            }
            "new_decoder" => {
                row["chunk"] = json!(chunk);
                bytes(&mut row, "input", &raw);
                row["ok"] = json!(true);
                decoder = Some(Decoder::new(Chunks {
                    data: raw,
                    pos: 0,
                    chunk,
                }));
            }
            _ => {
                let d = decoder
                    .as_mut()
                    .ok_or_else(|| format!("{op} before new_decoder"))?;
                match op {
                    "peek_kind" => row["kind"] = json!(d.peek_kind().name()),
                    "read_token" => match d.read_token() {
                        Ok(t) => {
                            row["ok"] = json!(true);
                            row["token_kind"] = json!(t.kind().name());
                            bytes(&mut row, "token", t.text());
                        }
                        Err(e) => error(&mut row, Err(e)),
                    },
                    "read_value" => match d.read_value() {
                        Ok(v) => {
                            row["ok"] = json!(true);
                            row["value_kind"] = json!(Kind::from_byte(v[0]).name());
                            bytes(&mut row, "value", &v);
                        }
                        Err(e) => error(&mut row, Err(e)),
                    },
                    "skip_value" => error(&mut row, d.skip_value()),
                    "input_offset" => row["input_offset"] = json!(d.input_offset()),
                    "stack_depth" => row["stack_depth"] = json!(d.stack_depth()),
                    "stack_pointer" => row["stack_pointer"] = json!(d.stack_pointer()),
                    "unread_buffer" => bytes(&mut row, "unread", d.unread_buffer()),
                    "unmarshal_decode" => {
                        let mut t = Target::new(target, "")?;
                        row["target"] = json!(target);
                        error(&mut row, tsr_json::unmarshal_decode(d, &mut t, options));
                        t.render(&mut row);
                    }
                    _ => return Err(format!("unknown JSON operation {op}")),
                }
            }
        }
        rows.push(row);
    }
    Ok(rows)
}
