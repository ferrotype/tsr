use crate::JsString;
use tsr_core::{CompilerOptions, ModuleKind, ResolutionMode};

/// Program-supplied file context, separate from immutable syntax storage.
/// Mirrors the SourceFileMetaData record in tsc/internal/ast/ast.go.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SourceFileMetaData {
    pub package_json_type: JsString,
    pub package_json_directory: JsString,
    pub implied_node_format: ResolutionMode,
}

fn has_suffix(name: &[u8], suffixes: &[&[u8]]) -> bool {
    suffixes.iter().any(|s| name.ends_with(s))
}

/// port: tsc/internal/ast/utilities.go:GetImpliedNodeFormatForEmitWorker
pub fn implied_node_format_for_emit(
    name: &[u8],
    emit: ModuleKind,
    meta: &SourceFileMetaData,
) -> ResolutionMode {
    if (ModuleKind::NODE16..=ModuleKind::NODE_NEXT).contains(&emit) {
        return meta.implied_node_format;
    }
    if meta.implied_node_format == ModuleKind::COMMON_JS
        && (meta.package_json_type.as_bytes() == b"commonjs"
            || has_suffix(name, &[b".cjs", b".cts"]))
    {
        return ModuleKind::COMMON_JS;
    }
    if meta.implied_node_format == ModuleKind::ESNEXT
        && (meta.package_json_type.as_bytes() == b"module" || has_suffix(name, &[b".mjs", b".mts"]))
    {
        return ModuleKind::ESNEXT;
    }
    ModuleKind::NONE
}

/// port: tsc/internal/ast/utilities.go:GetEmitModuleFormatOfFileWorker
pub fn emit_module_format_of_file(
    name: &[u8],
    options: &CompilerOptions,
    meta: &SourceFileMetaData,
) -> ModuleKind {
    let implied = implied_node_format_for_emit(name, options.emit_module_kind(), meta);
    if implied == ModuleKind::NONE {
        options.emit_module_kind()
    } else {
        implied
    }
}
