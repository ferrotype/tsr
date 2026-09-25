use crate::Error;
use tsr_ast::utilities_middle::{
    get_pragma_argument, get_pragma_from_source_file as pragma_from_source_file,
};
use tsr_ast::{
    AstView, ExternalModuleIndicatorOptions, NodeDataRead, NodeId, SourceFileMetaData,
    SyntaxKind as K,
};
use tsr_core::{CompilerOptions, JsxEmit, ModuleDetectionKind, ModuleKind, ModuleResolutionKind};
use tsr_jsstring::JsString;
use tsr_module::Resolver;
use tsr_tspath as path;
/// The caller also uses this helper for the library metadata shortcut.
/// port: tsc/internal/compiler/fileloader.go:fileLoader.loadSourceFileMetaData
pub(crate) fn load(
    resolver: &mut Resolver,
    name: &[u8],
    options: &CompilerOptions,
    is_lib: bool,
    skip_resolution: bool,
) -> Result<SourceFileMetaData, Error> {
    if is_lib {
        return Ok(SourceFileMetaData {
            implied_node_format: ModuleKind::COMMON_JS,
            ..Default::default()
        });
    }
    let mut result = SourceFileMetaData::default();
    let scope = if skip_resolution {
        None
    } else {
        resolver.package_scope(&path::directory(name))?
    };
    if let Some(info) = scope {
        result.package_json_directory = info.directory.clone();
        let resolution = options.module_resolution_kind();
        if !has_suffix(name, &[b".mts", b".cts", b".mjs", b".cjs"])
            && (ModuleResolutionKind::NODE16..=ModuleResolutionKind::NODE_NEXT)
                .contains(&resolution)
            || name.windows(14).any(|w| w == b"/node_modules/")
        {
            result.package_json_type =
                JsString::from_bytes(info.string("type").unwrap_or_default());
        }
    }
    result.implied_node_format =
        implied_node_format_for_file(name, result.package_json_type.as_bytes());
    Ok(result)
}
/// port: tsc/internal/ast/utilities.go:GetImpliedNodeFormatForFile
fn implied_node_format_for_file(path: &[u8], package_json_type: &[u8]) -> ModuleKind {
    if has_suffix(path, &[b".d.mts", b".mts", b".mjs"]) {
        ModuleKind::ESNEXT
    } else if has_suffix(path, &[b".d.cts", b".cts", b".cjs"]) {
        ModuleKind::COMMON_JS
    } else if has_suffix(path, &[b".d.ts", b".ts", b".tsx", b".js", b".jsx"]) {
        if package_json_type == b"module" {
            ModuleKind::ESNEXT
        } else {
            ModuleKind::COMMON_JS
        }
    } else {
        ModuleKind::NONE
    }
}
pub(crate) use tsr_ast::{
    emit_module_format_of_file as emit_format, implied_node_format_for_emit as implied_for_emit,
};

/// This describes emitted syntax, independent of resolution-mode attributes and
/// the module resolver's mode selection.
/// port: tsc/internal/compiler/fileloader.go:getEmitSyntaxForUsageLocationWorker
pub(crate) fn emit_syntax(
    view: AstView<'_>,
    name: &[u8],
    meta: &SourceFileMetaData,
    usage: NodeId,
    options: &CompilerOptions,
) -> Result<ModuleKind, tsr_arena::Error> {
    let node = view.node(usage)?;
    if !matches!(
        node.kind().known(),
        Some(K::StringLiteral | K::NoSubstitutionTemplateLiteral)
    ) {
        return Err(tsr_arena::Error::InvalidGraph);
    }
    let parent = node.parent().ok_or(tsr_arena::Error::InvalidGraph)?;
    let parent_node = view.node(parent)?;
    let import_equals = if parent_node.kind() == K::ExternalModuleReference {
        parent_node
            .parent()
            .map(|id| {
                view.node(id)
                    .map(|node| node.kind() == K::ImportEqualsDeclaration)
            })
            .transpose()?
            .unwrap_or(false)
    } else {
        false
    };
    if tsr_ast::utilities_middle::is_require_call(view, &parent_node, false)? || import_equals {
        return Ok(ModuleKind::COMMON_JS);
    }
    let emit = emit_format(name, options, meta);
    let expression_parent =
        tsr_ast::utilities::walk_up_parenthesized_expressions(view, Some(parent))?
            .ok_or(tsr_arena::Error::InvalidGraph)?;
    let expression_parent = view.node(expression_parent)?;
    let import_call = if let NodeDataRead::CallExpression(call) = expression_parent.data() {
        let expression = call.expression().ok_or(tsr_arena::Error::InvalidGraph)?;
        let expression_node = view.node(expression)?;
        expression_node.kind() == K::ImportKeyword
            || if let NodeDataRead::MetaProperty(meta) = expression_node.data() {
                meta.keyword_token() == K::ImportKeyword
                    && view.node_text(expression)?.as_bytes() == b"defer"
            } else {
                false
            }
    } else {
        false
    };
    if import_call {
        return Ok(if should_transform_import_call(options, emit) {
            ModuleKind::COMMON_JS
        } else {
            ModuleKind::ESNEXT
        });
    }
    Ok(if emit == ModuleKind::COMMON_JS {
        ModuleKind::COMMON_JS
    } else if emit.is_non_node_esm() || emit == ModuleKind::PRESERVE {
        ModuleKind::ESNEXT
    } else {
        ModuleKind::NONE
    })
}
/// The pin's file name parameter is unused.
/// port: tsc/internal/ast/utilities.go:ShouldTransformImportCall
fn should_transform_import_call(
    options: &CompilerOptions,
    implied_node_format_for_emit: ModuleKind,
) -> bool {
    let module = options.emit_module_kind();
    if (ModuleKind::NODE16..=ModuleKind::NODE_NEXT).contains(&module)
        || module == ModuleKind::PRESERVE
    {
        return false;
    }
    implied_node_format_for_emit < ModuleKind::ES2015
}
/// port: tsc/internal/ast/parseoptions.go:GetExternalModuleIndicatorOptions
pub(crate) fn indicator(
    name: &[u8],
    options: &CompilerOptions,
    meta: &SourceFileMetaData,
) -> ExternalModuleIndicatorOptions {
    if path::is_declaration_file_name(name) {
        return ExternalModuleIndicatorOptions::default();
    }
    match options.emit_module_detection_kind() {
        ModuleDetectionKind::FORCE => ExternalModuleIndicatorOptions {
            force: true,
            jsx: false,
        },
        ModuleDetectionKind::AUTO => ExternalModuleIndicatorOptions {
            jsx: matches!(options.jsx, JsxEmit::REACT_JSX | JsxEmit::REACT_JSX_DEV),
            force: is_file_forced_to_be_module_by_format(name, options, meta),
        },
        _ => ExternalModuleIndicatorOptions::default(),
    }
}
/// Declaration files are excluded by the caller.
/// port: tsc/internal/ast/parseoptions.go:isFileForcedToBeModuleByFormat
fn is_file_forced_to_be_module_by_format(
    name: &[u8],
    options: &CompilerOptions,
    meta: &SourceFileMetaData,
) -> bool {
    implied_for_emit(name, options.emit_module_kind(), meta) == ModuleKind::ESNEXT
        || has_suffix(name, &[b".cjs", b".cts", b".mjs", b".mts"])
}
/// port: tsc/internal/ast/utilities.go:IsExclusivelyTypeOnlyImportOrExport
fn is_exclusively_type_only_import_or_export(
    view: AstView<'_>,
    node: &tsr_ast::NodeRead<'_>,
) -> Result<bool, Error> {
    let clause = match node.data() {
        NodeDataRead::ExportDeclaration(data) => return Ok(data.is_type_only()),
        // The data variant covers ImportDeclaration and JSImportDeclaration.
        NodeDataRead::ImportDeclaration(data) => data.import_clause(),
        NodeDataRead::JSDocImportTag(data) => data.import_clause(),
        _ => return Ok(false),
    };
    clause.map_or(Ok(false), |clause| Ok(view.node(clause)?.is_type_only()))
}
/// port: tsc/internal/compiler/fileloader.go:importSyntaxAffectsModuleResolution
pub(crate) fn import_syntax_affects_module_resolution(options: &CompilerOptions) -> bool {
    (ModuleResolutionKind::NODE16..=ModuleResolutionKind::NODE_NEXT)
        .contains(&options.module_resolution_kind())
        || options.resolve_package_json_exports()
        || options.resolve_package_json_imports()
}
/// port: tsc/internal/compiler/fileloader.go:getModeForUsageLocation
pub(crate) fn usage_mode(
    view: AstView<'_>,
    name: &[u8],
    meta: &SourceFileMetaData,
    usage: NodeId,
    options: &CompilerOptions,
) -> Result<ModuleKind, Error> {
    let node = view.node(usage)?;
    let parent = node
        .parent()
        .ok_or(Error::Unsupported("module specifier without parent"))?;
    let parent_node = view.node(parent)?;
    if matches!(
        parent_node.data(),
        NodeDataRead::ImportDeclaration(_)
            | NodeDataRead::ExportDeclaration(_)
            | NodeDataRead::JSDocImportTag(_)
    ) && is_exclusively_type_only_import_or_export(view, &parent_node)?
    {
        let attributes = match parent_node.data() {
            NodeDataRead::ImportDeclaration(data) => data.attributes(),
            NodeDataRead::ExportDeclaration(data) => data.attributes(),
            NodeDataRead::JSDocImportTag(data) => data.attributes(),
            _ => None,
        };
        if let Some(mode) = resolution_override(view, attributes)? {
            return Ok(mode);
        }
    }
    if let NodeDataRead::LiteralTypeNode(_) = parent_node.data() {
        if let Some(grandparent) = parent_node.parent() {
            if let NodeDataRead::ImportTypeNode(data) = view.node(grandparent)?.data() {
                if let Some(mode) = resolution_override(view, data.attributes())? {
                    return Ok(mode);
                }
            }
        }
    }
    if import_syntax_affects_module_resolution(options) {
        return Ok(emit_syntax(view, name, meta, usage, options)?);
    }
    Ok(ModuleKind::NONE)
}
fn has_suffix(name: &[u8], suffixes: &[&[u8]]) -> bool {
    suffixes.iter().any(|s| name.ends_with(s))
}

// ast.go:ImportAttributesNode.GetResolutionModeOverride with the source's absent
// grammar-error callback. Invalid values preserve the normal mode fallback.
fn resolution_override(
    view: AstView<'_>,
    attributes: Option<NodeId>,
) -> Result<Option<ModuleKind>, Error> {
    tsr_ast::utilities_middle::import_attributes_resolution_mode(view, attributes)
        .map_err(Error::from)
}

pub(crate) fn normal_mode(
    name: &[u8],
    meta: &SourceFileMetaData,
    options: &CompilerOptions,
) -> ModuleKind {
    if !import_syntax_affects_module_resolution(options) {
        return ModuleKind::NONE;
    }
    let emit = emit_format(name, options, meta);
    if emit == ModuleKind::COMMON_JS {
        ModuleKind::COMMON_JS
    } else if emit.is_non_node_esm() || emit == ModuleKind::PRESERVE {
        ModuleKind::ESNEXT
    } else {
        ModuleKind::NONE
    }
}
/// port: tsc/internal/ast/utilities.go:GetJSXImplicitImportBase
pub(crate) fn jsx_implicit_import_base(
    view: AstView<'_>,
    source: NodeId,
    options: &CompilerOptions,
) -> Result<JsString, Error> {
    let state = view.source_file(source)?;
    let pragmas = state.pragmas()?;
    let import_source = pragma_from_source_file(pragmas.iter(), b"jsximportsource");
    let runtime = pragma_from_source_file(pragmas.iter(), b"jsxruntime");
    let factory = JsString::from_bytes(b"factory".as_slice());
    if get_pragma_argument(runtime, &factory) == b"classic" {
        return Ok(JsString::default());
    }
    if matches!(options.jsx, JsxEmit::REACT_JSX | JsxEmit::REACT_JSX_DEV)
        || !options.jsx_import_source.is_empty()
        || import_source.is_some()
        || get_pragma_argument(runtime, &factory) == b"automatic"
    {
        let mut result = get_pragma_argument(import_source, &factory);
        if result.is_empty() {
            result = options.jsx_import_source.as_bytes();
        }
        if result.is_empty() {
            result = b"react";
        }
        return Ok(JsString::from_bytes(result));
    }
    Ok(JsString::default())
}

/// An empty base is no runtime import.
/// port: tsc/internal/ast/utilities.go:GetJSXRuntimeImport
pub(crate) fn jsx_runtime_import(base: &[u8], options: &CompilerOptions) -> JsString {
    if base.is_empty() {
        return JsString::default();
    }
    let mut name = base.to_vec();
    name.push(b'/');
    name.extend_from_slice(if options.jsx == JsxEmit::REACT_JSX_DEV {
        b"jsx-dev-runtime"
    } else {
        b"jsx-runtime"
    });
    JsString::from_bytes(name)
}

/// port: tsc/internal/compiler/fileloader.go:getModeForTypeReferenceDirectiveInFile
pub(crate) fn type_reference_mode(
    mode: i64,
    name: &[u8],
    meta: &SourceFileMetaData,
    options: &CompilerOptions,
) -> ModuleKind {
    if mode != 0 {
        // ResolutionMode is Go's machine-sized int; CompilerOptions ModuleKind
        // narrows at its actual enum representation boundary.
        #[allow(clippy::cast_possible_truncation)]
        return ModuleKind(mode as i32);
    }
    default_resolution_mode_for_file(name, meta, options)
}
/// port: tsc/internal/compiler/fileloader.go:getDefaultResolutionModeForFile
pub(crate) fn default_resolution_mode_for_file(
    name: &[u8],
    meta: &SourceFileMetaData,
    options: &CompilerOptions,
) -> ModuleKind {
    if import_syntax_affects_module_resolution(options) {
        implied_for_emit(name, options.emit_module_kind(), meta)
    } else {
        ModuleKind::NONE
    }
}
