//! The production wasm JSON schema must decode through the compiler-independent
//! portable DTO and equal the separate public Rust consumer's observations.
//! These are native serialization controls; executing the wasm artifact and
//! testing its trap/error boundary belongs to the external JS bridge.
#[path = "../../../tools/phase7/portable/consumer.rs"]
mod consumer;
#[path = "../../../tools/phase7/portable/protocol.rs"]
mod protocol;

use protocol::{TranspileRequest, TranspileResponse, VERSION};

fn compare(source: &[u8], filename: &[u8], options: &[u8], declaration: bool, report: bool) {
    let request = TranspileRequest {
        version: VERSION,
        declaration,
        source: source.to_vec(),
        filename: filename.to_vec(),
        compiler_options: options.to_vec(),
        report_diagnostics: report,
    };
    let expected = consumer::execute(&request).unwrap();
    let options = std::str::from_utf8(options).unwrap();
    let encoded = if declaration {
        tsr_wasm::transpile_declaration(source, filename, options, report)
    } else {
        tsr_wasm::transpile_module(source, filename, options, report)
    }
    .unwrap_or_else(|_| panic!("the valid wasm transpile request succeeds"));
    let actual: TranspileResponse = serde_json::from_slice(&encoded).unwrap();
    actual.validate().unwrap();
    assert_eq!(actual, expected);
    assert!(actual
        .sources
        .iter()
        .any(|source| source.text == request.source));
}

#[test]
fn module_wire_matches_the_public_consumer_with_byte_strings_and_maps() {
    compare(
        b"// \xff\xed\xa0\x80\nexport const n: number = 1;\n",
        b"/\xfe/a.ts",
        br#"{"sourceMap":true,"declaration":true,"noEmit":true,"paths":{"10":["a"],"2":["b"]}}"#,
        false,
        true,
    );
    compare(b"export const n: number = ;", b"a.ts", b"{}", false, true);
}

#[test]
fn declaration_wire_matches_the_public_consumer_with_emit_diagnostics() {
    compare(
        b"export const sym = Symbol();\nexport function f() { return 1; }\n",
        b"a.mts",
        br#"{"target":2,"declarationMap":true,"noEmit":true,"declaration":false}"#,
        true,
        false,
    );
}
