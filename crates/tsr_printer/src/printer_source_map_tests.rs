//! The printer's source maps against native output
//! (`data/phase3/printer/source-maps.json`, produced by
//! `tools/phase3/printer_witness/a1_sourcemap_test.go` at the pin): each file
//! is printed through `Printer::write` with a source-map generator, as the
//! emitter's `printSourceFile` does, and the printed bytes and the map's JSON
//! must match byte for byte.
//!
//! A case with `writes` reuses one printer over several writes, as the
//! producer's `a1RunWrites` does: files, marker rewrites, emit-flag and range
//! edits, and each write's node, source file and generator.

use crate::{EmitContext, EmitTextWriter, Printer, PrinterOptions, SourceMapSource, TextWriter};
use serde_json::Value;
use std::ops::ControlFlow;
use std::rc::Rc;
use tsr_ast::{
    AstBuilder, AstView, ChildVisitor, FactoryMethods, JsString, NodeId, NodeListId, NodeSlice,
    NodeVisitor, SourceFileParseOptions, SyntaxKind as K,
};
use tsr_core::{NewLineKind, ScriptKind, TextRange};
use tsr_jsstring::SourceText;
use tsr_sourcemap::Source;

const FIXTURE: &str = include_str!("../../../data/phase3/printer/source-maps.json");

/// A fixture value: `{"text": ...}` or, for bytes that are not UTF-8,
/// `{"hex": ...}`.
fn bytes(value: &Value) -> Vec<u8> {
    if let Some(text) = value["text"].as_str() {
        return text.as_bytes().to_vec();
    }
    let hex = value["hex"].as_str().expect("text or hex");
    (0..hex.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&hex[i..i + 2], 16).expect("hex"))
        .collect()
}

/// The source the producer's `MapSourcePosition` handler maps to.
struct Original {
    text: Vec<u8>,
    line_map: Vec<i32>,
}

impl Source for Original {
    fn text(&self) -> &[u8] {
        &self.text
    }
    fn file_name(&self) -> &[u8] {
        b"/original.ts"
    }
    fn ecma_line_map(&self) -> &[i32] {
        &self.line_map
    }
}

/// `parser.ParseSourceFile` with the script kind of the file's extension.
fn parse(file_name: &str, text: Vec<u8>) -> tsr_ast::ParsedFile {
    let script_kind = if tsr_tspath::file_extension_is(file_name.as_bytes(), b".json") {
        ScriptKind::JSON
    } else if tsr_tspath::file_extension_is(file_name.as_bytes(), b".tsx") {
        ScriptKind::TSX
    } else {
        ScriptKind::TS
    };
    tsr_parser::parse_source_file(
        SourceText::from_loaded_bytes(text),
        script_kind,
        SourceFileParseOptions {
            file_name: JsString::from_bytes(file_name.as_bytes()),
            path: JsString::from_bytes(file_name.as_bytes()),
            ..Default::default()
        },
    )
}

fn new_generator() -> tsr_sourcemap::Generator {
    tsr_sourcemap::new_generator(
        JsString::from_bytes(&b"main.js"[..]),
        JsString::default(),
        JsString::from_bytes(&b"/"[..]),
        JsString::from_bytes(&b"/"[..]),
        true,
    )
}

fn run(case: &Value) -> (Vec<u8>, Vec<u8>) {
    let file_name = case["file_name"].as_str().expect("file name").to_owned();
    let text = bytes(&case["source"]);
    let parsed = parse(&file_name, text.clone());
    let flag = |name: &str| case[name].as_bool() == Some(true);
    let new_line = if flag("crlf") {
        NewLineKind::CRLF
    } else {
        NewLineKind::LF
    };
    let emit_context = crate::EmitContext::new();
    let mut printer = Printer::new(
        PrinterOptions {
            new_line,
            remove_comments: flag("remove_comments"),
            source_map: true,
            inline_sources: flag("inline_sources"),
            omit_brace_source_map_positions: flag("omit_braces"),
            ..PrinterOptions::default()
        },
        &emit_context,
    );
    if flag("mapped") {
        let original: Rc<dyn Source> = Rc::new(Original {
            line_map: tsr_jsstring::line_map::compute_ecma_line_starts(&text),
            text,
        });
        printer.map_source_position = Some(Box::new(move |source, data, pos| {
            if data.file_name() != file_name.as_bytes() {
                return Some((source.clone(), pos));
            }
            if pos % 3 == 0 {
                return None;
            }
            Some((SourceMapSource::Mapped(Rc::clone(&original)), pos))
        }));
    }
    let mut generator = new_generator();
    let mut writer = TextWriter::new(
        if new_line == NewLineKind::CRLF {
            b"\r\n"
        } else {
            b"\n"
        },
        0,
    );
    printer
        .write(
            parsed.view(),
            parsed.root(),
            Some(parsed.root()),
            &mut writer,
            Some(&mut generator),
        )
        .expect("printed");
    (
        writer.text().to_vec(),
        generator.string().as_bytes().to_vec(),
    )
}

/// The children of a node in `ForEachChild` order.
struct Children<'a>(AstView<'a>, Vec<NodeId>);

impl ChildVisitor for Children<'_> {
    fn visit_node(&mut self, node: NodeId) -> ControlFlow<()> {
        self.1.push(node);
        ControlFlow::Continue(())
    }
    fn visit_list(&mut self, nodes: NodeListId) -> ControlFlow<()> {
        let slice = self.0.list(nodes).expect("list").nodes();
        self.visit_node_slice(slice)
    }
    fn visit_node_slice(&mut self, nodes: NodeSlice) -> ControlFlow<()> {
        self.1
            .extend(self.0.node_slice(nodes).expect("nodes").iter().flatten());
        ControlFlow::Continue(())
    }
}

/// The producer's `a1PickNode`: the `index`-th node of `kind` in a pre-order
/// walk of the file, or the file for an empty kind.
fn pick(view: AstView<'_>, files: &[NodeId], spec: &Value) -> NodeId {
    let root = files[usize::try_from(spec["file"].as_u64().unwrap_or(0)).expect("file")];
    let kind = spec["kind"].as_str().unwrap_or("");
    if kind.is_empty() {
        return root;
    }
    let mut remaining = spec["index"].as_u64().unwrap_or(0);
    let mut pending = vec![root];
    while let Some(node) = pending.pop() {
        let read = view.node(node).expect("node");
        if format!("{:?}", read.kind().known().expect("known kind")) == kind {
            if remaining == 0 {
                return node;
            }
            remaining -= 1;
        }
        let mut children = Children(view, Vec::new());
        let _ = read.for_each_child(&mut children);
        pending.extend(children.1.into_iter().rev());
    }
    panic!("no node picked");
}

fn range(value: &Value) -> TextRange {
    let pos = value[0].as_i64().expect("pos");
    let end = value[1].as_i64().expect("end");
    TextRange::new(pos, end)
}

fn token_kind(name: &str) -> K {
    match name {
        "OpenBraceToken" => K::OpenBraceToken,
        "CloseBraceToken" => K::CloseBraceToken,
        _ => panic!("unknown token {name}"),
    }
}

/// The producer's `a1Rewrite` of one node.
fn rewrite(
    ec: &EmitContext,
    visitor: &mut NodeVisitor<'_>,
    node: Option<NodeId>,
) -> Option<NodeId> {
    let id = node?;
    let mut ec = ec.clone();
    let read = visitor.factory().node(id);
    match read.kind().known() {
        Some(K::ExpressionStatement) => {
            let expression = read.expression()?;
            let is_marker = visitor
                .factory()
                .node(expression)
                .as_identifier()
                .is_some_and(|identifier| identifier.text() == b"$notemitted");
            if is_marker {
                return Some(ec.new_not_emitted_statement(visitor, id));
            }
        }
        Some(K::Identifier) => {
            let text = read.as_identifier()?.text().to_vec();
            let range = read.range();
            if let Some(name) = text.strip_prefix(b"$created_") {
                return Some(visitor.new_identifier(JsString::from_bytes(name)));
            }
            if let Some(name) = text.strip_prefix(b"$ranged_") {
                let identifier = visitor.new_identifier(JsString::from_bytes(name));
                ec.set_source_map_range(identifier, range);
                return Some(identifier);
            }
            return node;
        }
        _ => {}
    }
    visitor.visit_each_child(node)
}

/// The message of a caught panic.
fn panic_text(payload: &(dyn std::any::Any + Send)) -> String {
    if let Some(text) = payload.downcast_ref::<String>() {
        return text.clone();
    }
    if let Some(text) = payload.downcast_ref::<&str>() {
        return (*text).to_owned();
    }
    "<non-string panic>".to_owned()
}

/// The producer's `a1RunWrites`: each write's text, then each generator's
/// map.
fn run_writes(case: &Value) -> (Vec<Vec<u8>>, Vec<Vec<u8>>) {
    let flag = |name: &str| case[name].as_bool() == Some(true);
    let mut ec = EmitContext::new();
    let mut factory = AstBuilder::with_hooks(
        SourceText::default(),
        &tsr_arena::Counters::new(),
        ec.factory_hooks(),
    );
    let mut sources = vec![(
        case["file_name"].as_str().expect("file name").to_owned(),
        bytes(&case["source"]),
    )];
    for extra in case["extra_files"].as_array().into_iter().flatten() {
        sources.push((
            extra["file_name"].as_str().expect("file name").to_owned(),
            bytes(&extra["source"]),
        ));
    }
    let mut files = Vec::new();
    for (file_name, text) in sources {
        let parsed = parse(&file_name, text);
        files.push(parsed.root());
        factory.retain_file(parsed.publish_unbound());
    }
    if flag("rewrite") {
        let hooks = ec.visitor_hooks();
        let visit_context = ec.clone();
        let visit = move |visitor: &mut NodeVisitor<'_>, node: Option<NodeId>| {
            rewrite(&visit_context, visitor, node)
        };
        for file in &mut files {
            *file = hooks
                .new_node_visitor(Some(&visit), &mut factory)
                .visit_source_file(*file);
        }
    }
    for edit in case["edits"].as_array().into_iter().flatten() {
        let node = pick(factory.view(), &files, edit);
        if let Some(flags) = edit["flags"].as_u64() {
            ec.add_emit_flags(node, u32::try_from(flags).expect("flags"));
        }
        if !edit["source_map_range"].is_null() {
            ec.set_source_map_range(node, range(&edit["source_map_range"]));
        }
        if !edit["comment_range"].is_null() {
            ec.set_comment_range(node, range(&edit["comment_range"]));
        }
        if let Some(token) = edit["token"].as_str() {
            ec.set_token_source_map_range(
                node,
                token_kind(token).into(),
                range(&edit["token_range"]),
            );
        }
    }
    let printer = Printer::new(
        PrinterOptions {
            new_line: NewLineKind::LF,
            remove_comments: flag("remove_comments"),
            source_map: true,
            inline_sources: flag("inline_sources"),
            omit_brace_source_map_positions: flag("omit_braces"),
            preserve_source_newlines: flag("preserve_source_newlines"),
            ..PrinterOptions::default()
        },
        &ec,
    );
    let count = usize::try_from(case["generators"].as_u64().unwrap_or(0)).expect("count");
    let mut generators: Vec<_> = (0..count).map(|_| new_generator()).collect();
    let mut outputs = Vec::new();
    for write in case["writes"].as_array().expect("writes") {
        let node = pick(factory.view(), &files, write);
        let file = files[usize::try_from(write["file"].as_u64().unwrap_or(0)).expect("file")];
        let source_file = (write["no_source_file"].as_bool() != Some(true)).then_some(file);
        let generator = match write["generator"].as_u64().unwrap_or(0) {
            0 => None,
            index => Some(&mut generators[usize::try_from(index - 1).expect("index")]),
        };
        let mut writer = TextWriter::new(b"\n", 0);
        let (printer, view, sink) = (&printer, factory.view(), &mut writer);
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(move || {
            printer.write(view, node, source_file, sink, generator)
        }));
        outputs.push(match result {
            Ok(Ok(())) => writer.text().to_vec(),
            Ok(Err(error)) => format!("printer error: {error}").into_bytes(),
            Err(payload) => format!("panic: {}", panic_text(&*payload)).into_bytes(),
        });
    }
    let maps = generators
        .iter_mut()
        .map(|generator| generator.string().as_bytes().to_vec())
        .collect();
    (outputs, maps)
}

fn expected_list(case: &Value, key: &str) -> Vec<Vec<u8>> {
    case[key].as_array().expect(key).iter().map(bytes).collect()
}

#[test]
fn printed_files_and_source_maps_match_the_pin() {
    let document: Value = serde_json::from_str(FIXTURE).expect("fixture");
    let cases = document["cases"].as_array().expect("cases");
    assert!(cases.len() >= 117);
    let mut failures = Vec::new();
    for case in cases {
        let id = case["id"].as_str().expect("id");
        if !case["writes"].is_null() {
            let (outputs, maps) = run_writes(case);
            for (what, actual, expected) in [
                ("outputs", outputs, expected_list(case, "expected_outputs")),
                ("maps", maps, expected_list(case, "expected_maps")),
            ] {
                if actual != expected {
                    let show = |list: &[Vec<u8>]| -> Vec<String> {
                        list.iter()
                            .map(|text| String::from_utf8_lossy(text).into_owned())
                            .collect()
                    };
                    failures.push(format!(
                        "{id}: {what} differ\n--- pinned\n{:#?}\n--- ported\n{:#?}",
                        show(&expected),
                        show(&actual)
                    ));
                }
            }
            continue;
        }
        let (output, map) = run(case);
        let (expected_output, expected_map) = (
            bytes(&case["expected_output"]),
            bytes(&case["expected_map"]),
        );
        if output != expected_output {
            failures.push(format!(
                "{id}: output differs\n--- pinned\n{}\n--- ported\n{}",
                String::from_utf8_lossy(&expected_output),
                String::from_utf8_lossy(&output)
            ));
        }
        if map != expected_map {
            failures.push(format!(
                "{id}: map differs\n--- pinned\n{}\n--- ported\n{}",
                String::from_utf8_lossy(&expected_map),
                String::from_utf8_lossy(&map)
            ));
        }
    }
    assert!(
        failures.is_empty(),
        "{} differences over {} cases:\n{}",
        failures.len(),
        cases.len(),
        failures.join("\n")
    );
}
