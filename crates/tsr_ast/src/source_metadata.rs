use crate::JsString;
use ts_core::ResolutionMode;

/// Program-supplied file context, separate from immutable syntax storage.
/// Mirrors the SourceFileMetaData record in tsc/internal/ast/ast.go.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SourceFileMetaData {
    pub package_json_type: JsString,
    pub package_json_directory: JsString,
    pub implied_node_format: ResolutionMode,
}
