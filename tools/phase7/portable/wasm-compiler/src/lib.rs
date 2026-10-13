//! Test-only wasm acceptance client. Compilation and queries enter tsr_embed;
//! baseline scheduling, DTOs and the pinned walker stay outside production.
#![forbid(unsafe_code)]
use std::sync::Arc;
use wasm_bindgen::prelude::*;
#[path = "../../assertions.rs"]
pub mod assertions;
#[path = "../../compiler_codec.rs"]
pub mod compiler_codec;
#[path = "../../compiler_consumer.rs"]
pub mod compiler_consumer;
#[path = "../../compiler_protocol.rs"]
pub mod compiler_protocol;
#[path = "../../mapper_codec.rs"]
pub mod mapper_codec;
#[path = "../../protocol.rs"]
pub mod protocol;
#[path = "../../walker/mod.rs"]
pub mod walker;
pub mod compiler_api {
    pub use tsr_embed::content_mapper as mapper;
    pub use tsr_embed::{Program, QueryError, QueryOperation as Operation, TypeRef};
}
pub mod walker_api {
    pub use crate::compiler_protocol::observation_rows;
    pub use tsr_embed::{
        node_builder_flags as builder_flags, node_builder_internal_flags as builder_internal_flags,
        symbol_format_flags, type_flags as tf, type_format_flags as ff, Program,
        QueryOperation as Operation, TypeRef,
    };
}

#[wasm_bindgen]
extern "C" {
    #[wasm_bindgen(catch, js_namespace = globalThis, js_name = tsrPortableCompilerCallback)]
    fn host_callback(request: &str) -> Result<String, JsValue>;
}

/// One consumer retains its compilation owners until Release. Its host must
/// discard the entire instance after a trap or any failed command; it cannot
/// retry a failed row with a replacement owner.
#[wasm_bindgen]
pub struct Compiler {
    consumer: compiler_consumer::CompilerConsumer,
    failed: bool,
}
#[wasm_bindgen]
impl Compiler {
    #[wasm_bindgen(constructor)]
    pub fn new() -> Self {
        Self {
            consumer: compiler_consumer::CompilerConsumer::new(Arc::new(|request| {
                let request = serde_json::to_string(&request).map_err(|error| error.to_string())?;
                let reply =
                    host_callback(&request).map_err(|error| format!("host callback: {error:?}"))?;
                serde_json::from_str(&reply).map_err(|error| error.to_string())
            })),
            failed: false,
        }
    }

    /// Returns the exact outer response/failure JSON frame. An error frame is
    /// terminal, including malformed input; callbacks are synchronous imports.
    pub fn execute(&mut self, request: &str) -> String {
        use compiler_protocol::CompilerOutputFrame;
        let result = if self.failed {
            Err("compiler consumer is terminal after an earlier failure".to_owned())
        } else {
            serde_json::from_str(request)
                .map_err(|error| error.to_string())
                .and_then(|request| self.consumer.execute(request))
        };
        let frame = match result {
            Ok(response) => CompilerOutputFrame::Response(response),
            Err(error) => {
                self.failed = true;
                CompilerOutputFrame::Failure(error)
            }
        };
        // All DTO fields have infallible JSON representations (integer, byte,
        // bool, string, sequence and struct); no arbitrary map keys or floats.
        serde_json::to_string(&frame).expect("compiler frame contains only JSON values")
    }
}
impl Default for Compiler {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn invalid_request_is_framed_and_terminal() {
        let mut compiler = Compiler::new();
        let first: compiler_protocol::CompilerOutputFrame = serde_json::from_str(
            &compiler.execute(r#"{"version":99,"command":{"command":"release","id":1}}"#),
        )
        .unwrap();
        let compiler_protocol::CompilerOutputFrame::Failure(error) = first else {
            panic!("invalid version must fail");
        };
        assert_eq!(error, "unsupported compiler protocol version");
        let second: compiler_protocol::CompilerOutputFrame =
            serde_json::from_str(&compiler.execute("{}")).unwrap();
        let compiler_protocol::CompilerOutputFrame::Failure(error) = second else {
            panic!("failed consumer must stay terminal");
        };
        assert_eq!(
            error,
            "compiler consumer is terminal after an earlier failure"
        );
    }

    #[test]
    fn malformed_duplicate_and_unknown_fields_fail_before_execution() {
        for request in [
            "",
            "{}",
            r#"{"version":1,"version":1,"command":{"command":"release","id":1}}"#,
            r#"{"version":1,"extra":true,"command":{"command":"release","id":1}}"#,
        ] {
            let mut compiler = Compiler::new();
            let frame: compiler_protocol::CompilerOutputFrame =
                serde_json::from_str(&compiler.execute(request)).unwrap();
            let compiler_protocol::CompilerOutputFrame::Failure(error) = frame else {
                panic!("invalid DTO must fail");
            };
            assert_ne!(error, "unknown compilation id", "{request}");
        }
    }
}
