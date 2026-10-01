//! The printer's source maps against native output
//! (`data/phase3/printer/source-maps.json`, produced by
//! `tools/phase3/printer_witness/a1_sourcemap_test.go` at the pin): each file
//! is printed through `Printer::write` with a source-map generator, as the
//! emitter's `printSourceFile` does, and the printed bytes and the map's JSON
//! must match byte for byte.

use crate::{EmitTextWriter, Printer, PrinterOptions, SourceMapSource, TextWriter};
use serde_json::Value;
use std::rc::Rc;
use tsr_ast::{JsString, SourceFileParseOptions};
use tsr_core::{NewLineKind, ScriptKind};
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

fn run(case: &Value) -> (Vec<u8>, Vec<u8>) {
    let file_name = case["file_name"].as_str().expect("file name").to_owned();
    let text = bytes(&case["source"]);
    let script_kind = if tsr_tspath::file_extension_is(file_name.as_bytes(), b".json") {
        ScriptKind::JSON
    } else if tsr_tspath::file_extension_is(file_name.as_bytes(), b".tsx") {
        ScriptKind::TSX
    } else {
        ScriptKind::TS
    };
    let parsed = tsr_parser::parse_source_file(
        SourceText::from_loaded_bytes(text.clone()),
        script_kind,
        SourceFileParseOptions {
            file_name: JsString::from_bytes(file_name.as_bytes()),
            path: JsString::from_bytes(file_name.as_bytes()),
            ..Default::default()
        },
    );
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
    let mut generator = tsr_sourcemap::new_generator(
        JsString::from_bytes(&b"main.js"[..]),
        JsString::default(),
        JsString::from_bytes(&b"/"[..]),
        JsString::from_bytes(&b"/"[..]),
        true,
    );
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

#[test]
fn printed_files_and_source_maps_match_the_pin() {
    let document: Value = serde_json::from_str(FIXTURE).expect("fixture");
    let cases = document["cases"].as_array().expect("cases");
    assert!(cases.len() >= 40);
    let mut failures = Vec::new();
    for case in cases {
        let id = case["id"].as_str().expect("id");
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
