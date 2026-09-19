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

/// The number of leading statements the insertion probe takes. The position
/// probe takes them all.
const MAX_STATEMENTS: usize = 4;

/// Every child `ForEachChild` reaches, with lists flattened.
struct Flat<'v> {
    view: AstView<'v>,
    out: Vec<NodeId>,
}

impl ts_ast::ChildVisitor for Flat<'_> {
    fn visit_node(&mut self, node: NodeId) -> std::ops::ControlFlow<()> {
        self.out.push(node);
        std::ops::ControlFlow::Continue(())
    }
    fn visit_list(&mut self, nodes: ts_ast::NodeListId) -> std::ops::ControlFlow<()> {
        match self.view.list(nodes) {
            Ok(list) => self.visit_node_slice(list.nodes()),
            Err(_) => std::ops::ControlFlow::Break(()),
        }
    }
    fn visit_node_slice(&mut self, nodes: ts_ast::NodeSlice) -> std::ops::ControlFlow<()> {
        match self.view.node_slice(nodes) {
            Ok(read) => {
                self.out.extend(read.iter().flatten());
                std::ops::ControlFlow::Continue(())
            }
            Err(_) => std::ops::ControlFlow::Break(()),
        }
    }
}

/// `kind,pos,end(children...)` in child order, so that a structural difference
/// shows as one.
fn nested(view: AstView<'_>, id: NodeId, out: &mut String) -> Result<(), String> {
    let node = view.node(id).map_err(|e| format!("{e:?}"))?;
    out.push_str(&format!(
        "{},{},{}(",
        node.kind().raw(),
        node.pos(),
        node.end()
    ));
    let mut children = Flat {
        view,
        out: Vec::new(),
    };
    let _ = node.for_each_child(&mut children);
    for child in children.out {
        nested(view, child, out)?;
    }
    out.push(')');
    Ok(())
}

/// What an API request carries: the statement encoded to protocol bytes and
/// decoded into a fresh tree. The wire digest is its own row.
fn decoded(
    view: AstView<'_>,
    root: NodeId,
    statement: NodeId,
    index: usize,
    stream: &mut Stream,
) -> Option<ts_encoder::DecodedTree> {
    // The codec keeps upstream's panics, so they are caught as the oracle
    // catches them and reported in upstream's words.
    let answer = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let mut provider = ts_parser::ParserJsDocProvider::default();
        ts_encoder::encode_node(view, statement, Some(root), &mut provider)
            .map_err(|error| error.to_string())
            .and_then(|wire| {
                let digest = hex(&Sha256::digest(&wire.bytes));
                ts_encoder::decode_nodes(&wire.bytes, &ts_arena::Counters::default())
                    .map(|tree| (digest, tree))
                    .map_err(|error| error.to_string())
            })
    }));
    match answer {
        Ok(Ok((digest, tree))) => {
            stream.row(&format!("W|{index}|{digest}"));
            Some(tree)
        }
        Ok(Err(error)) => {
            stream.row(&format!("W|{index}|?{error}"));
            None
        }
        Err(payload) => {
            stream.row(&format!(
                "W|{index}|!{}",
                go_panic_text(&panic_text(&*payload))
            ));
            None
        }
    }
}

fn panic_text(payload: &(dyn std::any::Any + Send)) -> String {
    payload
        .downcast_ref::<String>()
        .cloned()
        .or_else(|| {
            payload
                .downcast_ref::<&str>()
                .map(|text| (*text).to_owned())
        })
        .unwrap_or_else(|| "panic".to_owned())
}

/// Rust words an out-of-range index differently from the Go runtime.
fn go_panic_text(text: &str) -> String {
    let parse = || -> Option<String> {
        let rest = text.strip_prefix("index out of bounds: the len is ")?;
        let (length, index) = rest.split_once(" but the index is ")?;
        Some(format!(
            "runtime error: index out of range [{index}] with length {length}"
        ))
    };
    parse().unwrap_or_else(|| text.to_owned())
}

fn leading_statements(
    view: AstView<'_>,
    root: NodeId,
    limit: usize,
) -> Result<Vec<NodeId>, String> {
    let node = view.node(root).map_err(|e| format!("{e:?}"))?;
    let Some(list) = node.statement_list() else {
        return Ok(Vec::new());
    };
    let nodes = view.list(list).map_err(|e| format!("{e:?}"))?.nodes();
    Ok(view
        .node_slice(nodes)
        .map_err(|e| format!("{e:?}"))?
        .iter()
        .flatten()
        .take(limit)
        .collect())
}

/// Where insertion is probed: three line starts spread over the file and one
/// offset that is usually inside a line.
fn targets(view: AstView<'_>, root: NodeId, length: usize) -> Result<Vec<i64>, String> {
    let state = view.source_file(root).map_err(|e| format!("{e:?}"))?;
    let lines = state.ecma_line_map();
    let mut out: Vec<i64> = Vec::new();
    let mut add = |position: i64| {
        if !out.contains(&position) {
            out.push(position);
        }
    };
    for index in [0, lines.len() / 3, 2 * lines.len() / 3] {
        add(i64::from(lines[index]));
    }
    let all = positions(length);
    add(all[all.len() / 2]);
    Ok(out)
}

/// The body of the insertion handler after request decoding, for each leading
/// statement at each target. Upstream prints one decoded tree several times;
/// here a tree is consumed by the request, so each row decodes its own.
fn insertion(
    view: AstView<'_>,
    root: NodeId,
    length: usize,
    variant: &str,
    detail: bool,
) -> Result<Value, String> {
    let settings = ts_format::probe::variant(variant).expect("a known variant");
    let mut stream = Stream::new(detail);
    let places = targets(view, root, length)?;
    for (index, statement) in leading_statements(view, root, MAX_STATEMENTS)?
        .into_iter()
        .enumerate()
    {
        if decoded(view, root, statement, index, &mut stream).is_none() {
            continue;
        }
        for &position in &places {
            let answer = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                let mut quiet = Stream::new(false);
                let tree = decoded(view, root, statement, index, &mut quiet)
                    .expect("the statement decoded a moment ago");
                let mut provider = ts_parser::ParserJsDocProvider::default();
                let mut target = ts_format::FormatFile {
                    view,
                    source: root,
                    jsdoc: &mut provider,
                };
                ts_api::format_decoded_for_insertion(tree, &mut target, position, &settings)
            }));
            let text = match answer {
                Ok(Ok(text)) => hex(&text),
                Ok(Err(error)) => format!("!{error}"),
                Err(payload) => format!("!{}", go_panic_text(&panic_text(&*payload))),
            };
            stream.row(&format!("R|{index}|{position}|{text}"));
        }
    }
    Ok(stream.result())
}

/// `print_and_position_node` as the insertion handler calls it: the printed
/// text, then every node of the positioned tree in child order.
fn positioned(view: AstView<'_>, root: NodeId, detail: bool) -> Result<Value, String> {
    let mut stream = Stream::new(detail);
    for (index, statement) in leading_statements(view, root, usize::MAX)?
        .into_iter()
        .enumerate()
    {
        let Some(mut tree) = decoded(view, root, statement, index, &mut stream) else {
            continue;
        };
        let Some(node) = tree.root else {
            stream.row(&format!(
                "P|{index}|!runtime error: invalid memory address or nil pointer dereference"
            ));
            continue;
        };
        let settings = ts_format::FormatCodeSettings::default();
        let context = ts_printer::EmitContext::new();
        match ts_printer::print_and_position_node(
            &mut tree.builder,
            node,
            &settings.editor.new_line_character,
            settings.editor.indent_size as isize,
            &context,
        ) {
            Ok(text) => stream.row(&format!("P|{index}|{}", hex(&text))),
            Err(error) => {
                stream.row(&format!("P|{index}|!{error}"));
                continue;
            }
        }
        let mut shape = String::new();
        match nested(tree.builder.view(), node, &mut shape) {
            Ok(()) => stream.row(&format!("N|{index}|{shape}")),
            Err(error) => stream.row(&format!("N|{index}|!{error}")),
        }
    }
    Ok(stream.result())
}

/// The edit list is the observation. Whether it applies is a second fact: some
/// settings make the pinned formatter emit overlapping edits.
fn document(
    view: AstView<'_>,
    root: NodeId,
    edits: &[ts_core::TextChange],
    detail: bool,
) -> Result<Value, String> {
    let mut stream = Stream::new(detail);
    for edit in edits {
        stream.row(&format!(
            "E|{}|{}|{}",
            edit.range.pos(),
            edit.range.end(),
            hex(&edit.new_text)
        ));
    }
    let Value::Object(mut out) = stream.result() else {
        unreachable!("a stream result is an object");
    };
    let state = view.source_file(root).map_err(|e| format!("{e:?}"))?;
    match ts_core::apply_bulk_edits(state.text().as_bytes(), edits) {
        Ok(text) => out.insert("text_sha256".into(), json!(hex(&Sha256::digest(&text)))),
        Err(error) => out.insert("text_panic".into(), json!(error.to_string())),
    };
    Ok(Value::Object(out))
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
            "insert" => {
                let mut result = Map::new();
                for name in ["default", "tabs"] {
                    result.insert(name.into(), insertion(view, root, length, name, detail)?);
                }
                out.insert("insert".into(), Value::Object(result));
            }
            "position" => {
                out.insert("position".into(), positioned(view, root, detail)?);
            }
            "entry" => {
                let mut result = Map::new();
                let positions = positions(length);
                for name in ["default", "dense", "terse"] {
                    let mut stream = Stream::new(detail);
                    let mut provider = ts_parser::ParserJsDocProvider::default();
                    let mut format_file = ts_format::FormatFile {
                        view,
                        source: root,
                        jsdoc: &mut provider,
                    };
                    let settings = ts_format::probe::variant(name).expect("a known variant");
                    let new_line = settings.editor.new_line_character.clone();
                    let context = ts_format::FormatContext::new(settings, &new_line);
                    ts_format::probe::entry(&mut format_file, &context, &positions, &mut |row| {
                        stream.row(row);
                    });
                    result.insert(name.into(), stream.result());
                }
                out.insert("entry".into(), Value::Object(result));
            }
            "format" => {
                let mut result = Map::new();
                for name in ["default", "tabs", "two", "dense", "terse"] {
                    let mut provider = ts_parser::ParserJsDocProvider::default();
                    let mut format_file = ts_format::FormatFile {
                        view,
                        source: root,
                        jsdoc: &mut provider,
                    };
                    let settings = ts_format::probe::variant(name).expect("a known variant");
                    let new_line = settings.editor.new_line_character.clone();
                    let context = ts_format::FormatContext::new(settings, &new_line);
                    let answer = match ts_format::format_document(&mut format_file, &context) {
                        Ok(edits) => document(view, root, &edits, detail)?,
                        Err(error) => json!({"panic": error.to_string()}),
                    };
                    result.insert(name.into(), answer);
                }
                out.insert("format".into(), Value::Object(result));
            }
            "indent" => {
                let mut result = Map::new();
                let positions = positions(length);
                for name in ["default", "tabs", "two"] {
                    let mut stream = Stream::new(detail);
                    let mut provider = ts_parser::ParserJsDocProvider::default();
                    let mut format_file = ts_format::FormatFile {
                        view,
                        source: root,
                        jsdoc: &mut provider,
                    };
                    let settings = ts_format::probe::variant(name).expect("a known variant");
                    ts_format::probe::indent(&mut format_file, &settings, &positions, &mut |row| {
                        stream.row(row);
                    });
                    result.insert(name.into(), stream.result());
                }
                out.insert("indent".into(), Value::Object(result));
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
    // A caught panic is reported in its row, not on stderr.
    std::panic::set_hook(Box::new(|_| {}));
    if let Err(error) = run() {
        eprintln!("S09 format harness protocol: {error}");
        std::process::exit(2);
    }
}
