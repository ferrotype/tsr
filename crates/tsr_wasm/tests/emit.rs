//! Native control for `WasmSession::emit`'s request/response encoding: the
//! decoded response equals `tsr_embed::Session::emit` over a program loaded
//! as `MemoryHost::compile` loads it. Only successful calls run natively;
//! wasm-bindgen's error values need a wasm instance, so the error encoding
//! is `tools/s10/wasm/test.mjs`'s.
use serde_json::{json, Value};
use std::sync::Arc;
use tsr_arena::Counters;
use tsr_embed::{EmitOnly, EmitOptions, EmitOutput, FileCache, ProgramOptions, Session};
use tsr_jsstring::JsString;
use tsr_wasm::{MemoryHost, WasmSession};

const UTIL: &[u8] = b"export function helper(n: number) {\n    return n + 1;\n}\n";
const MAIN: &[u8] = b"import { helper } from \"./util\";\nexport const value = helper(41);\n";

/// The wasm session and its native control over the same files and options.
fn sessions(files: &[(&[u8], &[u8])], options: &Value) -> (WasmSession, Session) {
    let mut host = MemoryHost::new(b"/", true);
    let mut native = tsr_vfs::MemoryBuilder::new(b"/", true);
    let mut roots = Vec::new();
    for &(name, text) in files {
        host.add_file(name, text, true);
        native.insert_physical(name, text);
        roots.push(JsString::from_bytes(name));
    }
    let wasm = host
        .compile(&options.to_string())
        .unwrap_or_else(|_| panic!("the wasm entry compiles"));
    let control = Session::load(
        ProgramOptions {
            config: tsr_tsoptions::ParsedCommandLine::new(
                tsr_tsoptions::raw::compiler_options(options).unwrap(),
                roots,
            ),
            host: Arc::new(tsr_bundled::BundledFs::new(Arc::new(native.finish()))),
            current_directory: JsString::from_bytes(b"/".as_slice()),
            default_library_path: JsString::from_bytes(tsr_bundled::LIB_PATH),
            skip_module_resolution: false,
            single_threaded: tsr_core::Tristate::UNKNOWN,
        },
        &mut FileCache::new(),
        &Counters::new(),
    )
    .unwrap();
    (wasm, control)
}

fn emit(session: &WasmSession, request: &Value) -> Value {
    let response = session
        .emit(&request.to_string())
        .unwrap_or_else(|_| panic!("the request {request} emits"));
    serde_json::from_slice(&response).expect("a JSON response")
}

fn bytes(value: &Value) -> Vec<u8> {
    serde_json::from_value(value.clone()).expect("a byte array")
}

/// The response's skip flag and files against the control's.
fn assert_files(response: &Value, control: &EmitOutput) {
    let object = response.as_object().expect("an object");
    assert_eq!(
        object.keys().collect::<Vec<_>>(),
        ["emit_skipped", "diagnostics", "files"]
    );
    assert_eq!(response["emit_skipped"], control.emit_skipped);
    let files: Vec<(Vec<u8>, Vec<u8>)> = response["files"]
        .as_array()
        .expect("files")
        .iter()
        .map(|file| {
            assert_eq!(file.as_object().expect("a file").len(), 2);
            (bytes(&file["name"]), bytes(&file["text"]))
        })
        .collect();
    let expected: Vec<(Vec<u8>, Vec<u8>)> = control
        .files
        .iter()
        .map(|file| (file.name.as_bytes().to_vec(), file.text.clone()))
        .collect();
    assert_eq!(files, expected);
}

fn names(response: &Value) -> Vec<String> {
    response["files"]
        .as_array()
        .expect("files")
        .iter()
        .map(|file| String::from_utf8(bytes(&file["name"])).expect("a UTF-8 name"))
        .collect()
}

#[test]
fn emit_encodes_the_session_emit() {
    let options = json!({"declaration": true, "sourceMap": true, "declarationMap": true, "module": 99, "target": 9});
    let (wasm, control) = sessions(
        &[(b"/src/main.ts", MAIN), (b"/src/util.ts", UTIL)],
        &options,
    );
    let util = control.program().file(b"/src/util.ts").unwrap();
    let cases = [
        (json!({}), EmitOptions::default()),
        (
            json!({"files": null, "emitOnly": null, "forceEmit": null}),
            EmitOptions::default(),
        ),
        (
            json!({"files": [b"/src/util.ts"], "emitOnly": 1}),
            EmitOptions {
                target_source_files: Some(&[util]),
                emit_only: EmitOnly::Js,
                ..EmitOptions::default()
            },
        ),
        (
            json!({"emitOnly": 2, "forceEmit": false}),
            EmitOptions {
                emit_only: EmitOnly::Dts,
                ..EmitOptions::default()
            },
        ),
    ];
    for (request, control_options) in cases {
        let response = emit(&wasm, &request);
        let expected = control.emit(&control_options).unwrap();
        assert_files(&response, &expected);
        assert_eq!(response["diagnostics"], json!([]));
    }
    assert_eq!(
        names(&emit(&wasm, &json!({}))),
        [
            "/src/util.js.map",
            "/src/util.js",
            "/src/util.d.ts.map",
            "/src/util.d.ts",
            "/src/main.js.map",
            "/src/main.js",
            "/src/main.d.ts.map",
            "/src/main.d.ts",
        ]
    );
}

/// `noEmit`: the pin's empty, skipped and forced results.
#[test]
fn emit_encodes_no_emit_results() {
    let (wasm, control) = sessions(&[(b"/src/util.ts", UTIL)], &json!({"noEmit": true}));
    let util = control.program().file(b"/src/util.ts").unwrap();
    for (request, control_options, skipped, files) in [
        (json!({}), EmitOptions::default(), false, &[][..]),
        (
            json!({"files": [b"/src/util.ts"]}),
            EmitOptions {
                target_source_files: Some(&[util]),
                ..EmitOptions::default()
            },
            true,
            &[][..],
        ),
        (
            json!({"files": [b"/src/util.ts"], "forceEmit": true}),
            EmitOptions {
                target_source_files: Some(&[util]),
                force_emit: true,
                ..EmitOptions::default()
            },
            false,
            &["/src/util.js"][..],
        ),
    ] {
        let response = emit(&wasm, &request);
        assert_files(&response, &control.emit(&control_options).unwrap());
        assert_eq!(response["emit_skipped"], skipped);
        assert_eq!(names(&response), files);
    }
}

/// An emit diagnostic is a `diagnostics` row: its file name, UTF-8 range,
/// code, arguments and related information.
#[test]
fn emit_encodes_emit_diagnostics() {
    let source = b"export const Hidden = class {\n    private secret = 1;\n};\n";
    let (wasm, control) = sessions(
        &[(b"/src/hidden.ts", source)],
        &json!({"declaration": true}),
    );
    let response = emit(&wasm, &json!({}));
    let expected = control.emit(&EmitOptions::default()).unwrap();
    assert_files(&response, &expected);
    assert_eq!(names(&response), ["/src/hidden.js"]);
    let rows = response["diagnostics"].as_array().expect("diagnostics");
    assert_eq!(rows.len(), expected.diagnostics.len());
    assert_eq!(rows.len(), 1);
    for (row, diagnostic) in rows.iter().zip(&expected.diagnostics) {
        assert_eq!(bytes(&row["file"]), b"/src/hidden.ts");
        assert_eq!(row["start"], diagnostic.loc.pos());
        assert_eq!(row["end"], diagnostic.loc.end());
        assert_eq!(row["code"], 4094);
        assert_eq!(row["code"], diagnostic.code);
        assert_eq!(
            row["arguments"]
                .as_array()
                .expect("arguments")
                .iter()
                .map(bytes)
                .collect::<Vec<_>>(),
            [b"secret".to_vec()]
        );
        assert_eq!(
            row["related"].as_array().expect("related").len(),
            diagnostic.related_information.len()
        );
    }
}
