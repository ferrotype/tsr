use crate::ResolvedModule;
use ts_core::{CompilerOptions, JsxEmit};
use ts_diagnostics::Message;
/// Only SourceFile.IsDeclarationFile is read by the pinned operation; callers
/// supply that projection without introducing an AST owner dependency.
/// port: tsc/internal/module/util.go:GetResolutionDiagnostic
pub fn resolution_diagnostic(
    options: &CompilerOptions,
    resolved: &ResolvedModule,
    is_declaration_file: bool,
) -> Option<&'static Message> {
    let need_jsx = || {
        if options.jsx == JsxEmit::NONE {
            Some(ts_diagnostics::Module_0_was_resolved_to_1_but_jsx_is_not_set)
        } else {
            None
        }
    };
    let need_js = || {
        if options.allow_js()
            || !options
                .no_implicit_any
                .default_if_unknown(options.strict)
                .is_true()
        {
            None
        } else {
            Some(ts_diagnostics::Could_not_find_a_declaration_file_for_module_0_1_implicitly_has_an_any_type)
        }
    };
    if resolved.resolved_using_extra_extensions {
        return None;
    }
    match resolved.extension.as_bytes() {
        b".ts" | b".d.ts" | b".mts" | b".d.mts" | b".cts" | b".d.cts" => None,
        b".tsx" => need_jsx(),
        b".jsx" => need_jsx().or_else(need_js),
        b".js" | b".mjs" | b".cjs" => need_js(),
        b".json" => {
            if options.resolve_json_module() {
                None
            } else {
                Some(ts_diagnostics::Module_0_was_resolved_to_1_but_resolveJsonModule_is_not_used)
            }
        }
        _ => {
            if is_declaration_file || options.allow_arbitrary_extensions.is_true() {
                None
            } else {
                Some(ts_diagnostics::Module_0_was_resolved_to_1_but_allowArbitraryExtensions_is_not_set)
            }
        }
    }
}
