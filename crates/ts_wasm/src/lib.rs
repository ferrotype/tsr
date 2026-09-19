//! Byte-preserving parser entry points for a bare wasm32 instance.
//!
//! Source is already loaded (BOM/encoding conversion belongs to the host).
//! Returned vectors are copied into owned JS typed arrays by wasm-bindgen.
//! A wasm trap is terminal for the instance; use the supplied JS wrapper.

use ts_ast::SourceFileParseOptions;
use ts_core::ScriptKind;
use ts_jsstring::{JsString, SourceText};
use wasm_bindgen::prelude::*;

// Private ABI tag: only an explicit Rust Result::Err uses this prefix. Unknown
// JS exceptions (including engine RangeError stack exhaustion) remain terminal.
fn api_error(value: impl std::fmt::Display) -> JsValue {
    JsValue::from_str(&format!("ts-wasm-api-error:{value}"))
}

#[cfg(feature = "checker")]
mod checker;
#[cfg(feature = "checker")]
pub use checker::{MemoryHost, WasmSession};

/// Capture-only entry point: the frozen S08 walker over the public embedding API.
#[cfg(feature = "corpus")]
#[wasm_bindgen]
pub fn observe_corpus(request: &str) -> Result<Vec<u8>, JsValue> {
    let request = serde_json::from_str(request).map_err(api_error)?;
    serde_json::to_vec(&s10_corpus::observe(&request, ts_embed::Session::load)).map_err(api_error)
}

fn options(file_name: &[u8], jsx: bool, force: bool) -> SourceFileParseOptions {
    SourceFileParseOptions {
        file_name: JsString::from_bytes(file_name),
        path: JsString::from_bytes(file_name),
        external_module_indicator_options: ts_ast::ExternalModuleIndicatorOptions { jsx, force },
    }
}

/// Parse a fresh file and return observable node/identifier/diagnostic counts.
/// No AST survives the call. The throughput runner also verifies encoded bytes
/// outside its parse-only interval.
#[wasm_bindgen]
pub fn parse(
    source: &[u8],
    file_name: &[u8],
    script_kind: i32,
    jsx: bool,
    force: bool,
) -> Vec<u32> {
    let file = ts_embed::parse(
        SourceText::from_loaded_bytes(source),
        ScriptKind(script_kind),
        options(file_name, jsx, force),
    );
    let state = file
        .view()
        .source_file(file.root().expect("parsed source"))
        .expect("parser source metadata");
    vec![
        state.node_count as u32,
        state.identifier_count as u32,
        state.diagnostics.len() as u32,
    ]
}

/// Parse and encode protocol-8 bytes, including lazily materialized JSDoc.
#[wasm_bindgen]
pub fn parse_and_encode(
    source: &[u8],
    file_name: &[u8],
    script_kind: i32,
    jsx: bool,
    force: bool,
) -> Result<Vec<u8>, JsValue> {
    ts_embed::parse_and_encode(
        SourceText::from_loaded_bytes(source),
        ScriptKind(script_kind),
        options(file_name, jsx, force),
    )
    .map_err(api_error)
}
