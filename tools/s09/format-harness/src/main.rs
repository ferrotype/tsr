//! The Rust side of the formatter differential (docs/S09-3-formatter-plan.md).
//!
//! It answers the same requests as `tools/s09/format_oracle/main.go` with the
//! same row grammar, so `scripts/s09_format.py compare` can hold the port to the
//! pinned implementation request by request. Only the operations that have been
//! ported are accepted; asking for another one is an error, never an empty
//! answer.
#![forbid(unsafe_code)]

use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};
use std::io::{BufRead, Write};
use ts_arena::NodeId;
use ts_ast::{
    AstView, ExternalModuleIndicatorOptions, JsString, SourceFileParseOptions, SyntaxKind as K,
};
use ts_astnav::Navigator;
use ts_jsstring::SourceText;

const PROTOCOL_VERSION: i64 = 1;
/// At most this many evenly spaced probe positions per file, plus the end.
const MAX_POSITIONS: usize = 512;

/// Rows of one stream; every row ends in a newline.
struct Stream {
    rows: usize,
    failures: usize,
    hash: Sha256,
    detail: Option<String>,
}

impl Stream {
    fn new(detail: bool) -> Self {
        Self {
            rows: 0,
            failures: 0,
            hash: Sha256::new(),
            detail: detail.then(String::new),
        }
    }
    fn row(&mut self, text: &str) {
        self.rows += 1;
        // '!' marks a failure where upstream panics, '?' a returned error.
        if text.contains("|!") || text.contains("|?") {
            self.failures += 1;
        }
        self.hash.update(text.as_bytes());
        self.hash.update(b"\n");
        if let Some(detail) = &mut self.detail {
            detail.push_str(text);
            detail.push('\n');
        }
    }
    fn result(self) -> Value {
        let mut out = Map::new();
        out.insert("rows".into(), json!(self.rows));
        out.insert("failures".into(), json!(self.failures));
        out.insert("sha256".into(), json!(hex(&self.hash.finalize())));
        if let Some(detail) = self.detail {
            out.insert("detail".into(), json!(detail));
        }
        Value::Object(out)
    }
}

fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write;
    bytes.iter().fold(String::new(), |mut out, byte| {
        let _ = write!(out, "{byte:02x}");
        out
    })
}

fn unhex(text: &str) -> Result<Vec<u8>, String> {
    if !text.len().is_multiple_of(2) {
        return Err("source_hex: odd length".into());
    }
    (0..text.len())
        .step_by(2)
        .map(|at| u8::from_str_radix(&text[at..at + 2], 16).map_err(|e| format!("source_hex: {e}")))
        .collect()
}

/// Evenly spaced byte offsets ending with the end of the text. They depend on
/// the length alone, so both sides derive the same set.
fn positions(length: usize) -> Vec<i64> {
    let stride = ((length + MAX_POSITIONS) / MAX_POSITIONS).max(1);
    let mut out: Vec<i64> = (0..length).step_by(stride).map(|p| p as i64).collect();
    out.push(length as i64);
    out
}

fn node_text(view: AstView<'_>, id: NodeId) -> Result<String, ts_astnav::Error> {
    let node = view.node(id)?;
    Ok(format!(
        "{},{},{}",
        node.kind().raw(),
        node.pos(),
        node.end()
    ))
}

fn optional(view: AstView<'_>, id: Option<NodeId>) -> Result<String, ts_astnav::Error> {
    id.map_or_else(|| Ok("-".into()), |id| node_text(view, id))
}

/// One entry-point call. A failure where upstream panics becomes the row's
/// value, so one failing position does not hide the rest of the file.
fn call(action: impl FnOnce() -> Result<String, ts_astnav::Error>) -> String {
    match action() {
        Ok(text) => text,
        Err(error) => format!("!{error}"),
    }
}

const CHILD_KINDS: [K; 6] = [
    K::OpenBraceToken,
    K::CloseBraceToken,
    K::OpenParenToken,
    K::CloseParenToken,
    K::OpenBracketToken,
    K::CloseBracketToken,
];

fn navigation(view: AstView<'_>, source: NodeId, length: usize, detail: bool) -> Value {
    let mut stream = Stream::new(detail);
    let mut provider = ts_parser::ParserJsDocProvider::default();
    let mut navigator = Navigator::new(view, source, &mut provider);
    for p in positions(length) {
        let mut token = None;
        let mut preceding = None;
        let row = call(|| {
            let found = navigator.get_token_at_position(p)?;
            token = Some(found);
            node_text(view, found)
        });
        stream.row(&format!("T|{p}|{row}"));
        let row = call(|| optional(view, navigator.find_preceding_token(p)?));
        stream.row(&format!("P|{p}|{row}"));
        let row = call(|| {
            preceding = navigator.find_preceding_token_ex(p, None, true)?;
            optional(view, preceding)
        });
        stream.row(&format!("X|{p}|{row}"));
        if let Some(token) = token {
            let row = call(|| {
                Ok(format!(
                    "{}|{}",
                    navigator.get_start_of_node(token, false)?,
                    navigator.get_start_of_node(token, true)?
                ))
            });
            stream.row(&format!("S|{p}|{row}"));
            if let Some(parent) = view.node(token).ok().and_then(|node| node.parent()) {
                for kind in CHILD_KINDS {
                    let row = call(|| optional(view, navigator.find_child_of_kind(parent, kind)?));
                    stream.row(&format!("C|{p}|{}|{row}", kind as u16));
                }
            }
        }
        if let Some(preceding) = preceding {
            if let Some(parent) = view.node(preceding).ok().and_then(|node| node.parent()) {
                let row = call(|| optional(view, navigator.find_next_token(preceding, parent)?));
                stream.row(&format!("N|{p}|{row}"));
                let row = call(|| optional(view, navigator.find_next_token(preceding, source)?));
                stream.row(&format!("F|{p}|{row}"));
            }
        }
    }
    stream.result()
}

fn observe(request: &Value) -> Result<Value, String> {
    let field = |name: &str| request.get(name).ok_or_else(|| format!("missing {name}"));
    let text = |name: &str| {
        field(name)?
            .as_str()
            .ok_or_else(|| format!("{name}: string required"))
    };
    let flag = |name: &str| {
        field(name)?
            .as_bool()
            .ok_or_else(|| format!("{name}: bool required"))
    };
    let id = text("id")?.to_owned();
    if field("version")?.as_i64() != Some(PROTOCOL_VERSION) || id.is_empty() {
        return Err(format!("request {id:?}: unsupported version or empty id"));
    }
    let source = unhex(text("source_hex")?)?;
    let length = source.len();
    let script_kind = field("script_kind")?
        .as_i64()
        .ok_or("script_kind: integer required")?;
    let detail = flag("detail")?;
    let ops: Vec<&str> = field("ops")?
        .as_array()
        .ok_or("ops: array required")?
        .iter()
        .map(|op| op.as_str().ok_or("ops: strings required"))
        .collect::<Result<_, _>>()?;
    if ops.is_empty() {
        return Err(format!("request {id:?}: no operations"));
    }
    let options = SourceFileParseOptions {
        file_name: JsString::from_bytes(text("filename")?.as_bytes()),
        path: JsString::from_bytes(text("path")?.as_bytes()),
        external_module_indicator_options: ExternalModuleIndicatorOptions {
            jsx: flag("jsx")?,
            force: flag("force")?,
        },
    };
    let parsed = ts_parser::parse_source_file(
        SourceText::from_loaded_bytes(source),
        ts_core::ScriptKind(script_kind as i32),
        options,
    );
    let file = parsed.publish_unbound();
    let view = file.view();
    let root = file.root().ok_or("parsed file has no root")?;
    let mut out = Map::new();
    out.insert("id".into(), json!(id));
    {
        let node = view.node(root).map_err(|e| format!("{e:?}"))?;
        let state = view.source_file(root).map_err(|e| format!("{e:?}"))?;
        out.insert(
            "parse".into(),
            json!({"end": node.end(), "node_count": state.node_count,
                   "language_variant": state.language_variant.0, "lines": state.ecma_line_map().len()}),
        );
    }
    for op in ops {
        match op {
            "nav" => {
                out.insert("nav".into(), navigation(view, root, length, detail));
            }
            "rules" => {
                let mut result = Map::new();
                for name in ["default", "tabs", "two", "dense", "terse"] {
                    let mut stream = Stream::new(detail);
                    let mut provider = ts_parser::ParserJsDocProvider::default();
                    let mut format_file = ts_format::FormatFile {
                        view,
                        source: root,
                        jsdoc: &mut provider,
                    };
                    let settings = ts_format::probe::variant(name).expect("a known variant");
                    ts_format::probe::rules(&mut format_file, settings, &mut |row| stream.row(row));
                    result.insert(name.into(), stream.result());
                }
                out.insert("rules".into(), Value::Object(result));
            }
            "rulesmap" => {
                let mut stream = Stream::new(detail);
                ts_format::probe::rules_map(&mut |row| stream.row(row));
                out.insert("rulesmap".into(), stream.result());
            }
            "scan" => {
                let mut stream = Stream::new(detail);
                let mut provider = ts_parser::ParserJsDocProvider::default();
                let mut format_file = ts_format::FormatFile {
                    view,
                    source: root,
                    jsdoc: &mut provider,
                };
                ts_format::probe::scan(&mut format_file, &mut |row| stream.row(row));
                out.insert("scan".into(), stream.result());
            }
            other => return Err(format!("operation {other:?} is not ported yet")),
        }
    }
    Ok(Value::Object(out))
}

fn run() -> Result<(), String> {
    let stdin = std::io::stdin();
    let stdout = std::io::stdout();
    let mut output = stdout.lock();
    let mut seen = std::collections::HashSet::new();
    for line in stdin.lock().lines() {
        let line = line.map_err(|e| e.to_string())?;
        let request: Value =
            serde_json::from_str(&line).map_err(|e| format!("invalid request: {e}"))?;
        let id = request["id"].as_str().unwrap_or_default().to_owned();
        if !seen.insert(id.clone()) {
            return Err(format!("duplicate request identity {id:?}"));
        }
        let answer = match observe(&request) {
            Ok(answer) => answer,
            Err(error) => json!({"id": id, "error": error}),
        };
        writeln!(output, "{answer}").map_err(|e| e.to_string())?;
        output.flush().map_err(|e| e.to_string())?;
    }
    Ok(())
}

fn main() {
    if let Err(error) = run() {
        eprintln!("S09 format harness protocol: {error}");
        std::process::exit(2);
    }
}
