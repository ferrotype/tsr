//! Private S10 capture adapter over the shipped embedding API.
use wasm_bindgen::prelude::*;

// Match the instance wrapper's explicit Result::Err tag. Unknown JS exceptions
// and traps still retire the instance; this does not rewrite panic behavior.
fn api_error(value: impl std::fmt::Display) -> JsValue {
    JsValue::from_str(&format!("ts-wasm-api-error:{value}"))
}

/// Run the frozen S08 walker through a public embedding session.
#[wasm_bindgen]
pub fn observe_corpus(request: &str) -> Result<Vec<u8>, JsValue> {
    let request = serde_json::from_str(request).map_err(api_error)?;
    serde_json::to_vec(&s10_corpus::observe(&request, tsr_embed::Session::load)).map_err(api_error)
}
