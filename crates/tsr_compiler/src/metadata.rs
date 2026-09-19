use crate::Error;
use ts_ast::{
    AstView, ExternalModuleIndicatorOptions, NodeDataRead, NodeId, SourceFileMetaData,
    SyntaxKind as K,
};
use ts_core::{CompilerOptions, JsxEmit, ModuleDetectionKind, ModuleKind, ModuleResolutionKind};
use ts_jsstring::JsString;
use ts_module::Resolver;
use ts_tspath as path;
pub(crate) fn load(
    resolver: &mut Resolver,
    name: &[u8],
    options: &CompilerOptions,
    is_lib: bool,
) -> Result<SourceFileMetaData, Error> {
    if is_lib {
        return Ok(SourceFileMetaData {
            implied_node_format: ModuleKind::COMMON_JS,
            ..Default::default()
        });
    }
    let mut result = SourceFileMetaData::default();
    if let Some(info) = resolver.package_scope(&path::directory(name))? {
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
    result.implied_node_format = if has_suffix(name, &[b".mts", b".mjs"]) {
        ModuleKind::ESNEXT
    } else if has_suffix(name, &[b".cts", b".cjs"]) {
        ModuleKind::COMMON_JS
    } else if has_suffix(name, &[b".ts", b".tsx", b".js", b".jsx"]) {
        if result.package_json_type.as_bytes() == b"module" {
            ModuleKind::ESNEXT
        } else {
            ModuleKind::COMMON_JS
        }
    } else {
        ModuleKind::NONE
    };
    Ok(result)
}
/// port: tsc/internal/ast/utilities.go:GetImpliedNodeFormatForEmitWorker
pub(crate) fn implied_for_emit(
    name: &[u8],
    emit: ModuleKind,
    meta: &SourceFileMetaData,
) -> ModuleKind {
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
pub(crate) fn emit_format(
    name: &[u8],
    options: &CompilerOptions,
    meta: &SourceFileMetaData,
) -> ModuleKind {
    let implied = implied_for_emit(name, options.emit_module_kind(), meta);
    if implied == ModuleKind::NONE {
        options.emit_module_kind()
    } else {
        implied
    }
}

/// This describes emitted syntax, independent of resolution-mode attributes and
/// the module resolver's mode selection.
/// port: tsc/internal/compiler/fileloader.go:getEmitSyntaxForUsageLocationWorker
pub(crate) fn emit_syntax(
    view: AstView<'_>,
    name: &[u8],
    meta: &SourceFileMetaData,
    usage: NodeId,
    options: &CompilerOptions,
) -> Result<ModuleKind, ts_arena::Error> {
    let node = view.node(usage)?;
    if !matches!(
        node.kind().known(),
        Some(K::StringLiteral | K::NoSubstitutionTemplateLiteral)
    ) {
        return Err(ts_arena::Error::InvalidGraph);
    }
    let parent = node.parent().ok_or(ts_arena::Error::InvalidGraph)?;
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
    if ts_ast::utilities_middle::is_require_call(view, &parent_node, false)? || import_equals {
        return Ok(ModuleKind::COMMON_JS);
    }
    let emit = emit_format(name, options, meta);
    let expression_parent =
        ts_ast::utilities::walk_up_parenthesized_expressions(view, Some(parent))?
            .ok_or(ts_arena::Error::InvalidGraph)?;
    let expression_parent = view.node(expression_parent)?;
    let import_call = if let NodeDataRead::CallExpression(call) = expression_parent.data() {
        let expression = call.expression().ok_or(ts_arena::Error::InvalidGraph)?;
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
        // port: tsc/internal/ast/utilities.go:ShouldTransformImportCall
        let module = options.emit_module_kind();
        return Ok(
            if !(ModuleKind::NODE16..=ModuleKind::NODE_NEXT).contains(&module)
                && module != ModuleKind::PRESERVE
                && emit < ModuleKind::ES2015
            {
                ModuleKind::COMMON_JS
            } else {
                ModuleKind::ESNEXT
            },
        );
    }
    Ok(if emit == ModuleKind::COMMON_JS {
        ModuleKind::COMMON_JS
    } else if emit.is_non_node_esm() || emit == ModuleKind::PRESERVE {
        ModuleKind::ESNEXT
    } else {
        ModuleKind::NONE
    })
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
            force: implied_for_emit(name, options.emit_module_kind(), meta) == ModuleKind::ESNEXT
                || has_suffix(name, &[b".cjs", b".cts", b".mjs", b".mts"]),
        },
        _ => ExternalModuleIndicatorOptions::default(),
    }
}
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
    let type_only = |id: Option<NodeId>| -> Result<bool, Error> {
        id.map_or(Ok(false), |id| Ok(view.node(id)?.is_type_only()))
    };
    let attributes = match parent_node.data() {
        NodeDataRead::ImportDeclaration(data) => {
            if type_only(data.import_clause())? {
                data.attributes()
            } else {
                None
            }
        }
        NodeDataRead::ExportDeclaration(data) if data.is_type_only() => data.attributes(),
        NodeDataRead::JSDocImportTag(data) => {
            if type_only(data.import_clause())? {
                data.attributes()
            } else {
                None
            }
        }
        NodeDataRead::LiteralTypeNode(_) => {
            if let Some(grandparent) = parent_node.parent() {
                if let NodeDataRead::ImportTypeNode(data) = view.node(grandparent)?.data() {
                    data.attributes()
                } else {
                    None
                }
            } else {
                None
            }
        }
        _ => None,
    };
    if let Some(mode) = resolution_override(view, attributes)? {
        return Ok(mode);
    }
    let resolution = options.module_resolution_kind();
    if !(ModuleResolutionKind::NODE16..=ModuleResolutionKind::NODE_NEXT).contains(&resolution)
        && !options.resolve_package_json_exports()
        && !options.resolve_package_json_imports()
    {
        return Ok(ModuleKind::NONE);
    }
    if ts_ast::utilities_middle::is_require_call(view, &parent_node, false)?
        || parent_node.kind() == K::ExternalModuleReference
    {
        return Ok(ModuleKind::COMMON_JS);
    }
    let implied = implied_for_emit(name, options.emit_module_kind(), meta);
    let emit = if implied == ModuleKind::NONE {
        options.emit_module_kind()
    } else {
        implied
    };
    if parent_node.kind() == K::CallExpression {
        let module = options.emit_module_kind();
        return Ok(
            if !(ModuleKind::NODE16..=ModuleKind::NODE_NEXT).contains(&module)
                && module != ModuleKind::PRESERVE
                && emit < ModuleKind::ES2015
            {
                ModuleKind::COMMON_JS
            } else {
                ModuleKind::ESNEXT
            },
        );
    }
    Ok(if emit == ModuleKind::COMMON_JS {
        ModuleKind::COMMON_JS
    } else if emit.is_non_node_esm() || emit == ModuleKind::PRESERVE {
        ModuleKind::ESNEXT
    } else {
        ModuleKind::NONE
    })
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
    ts_ast::utilities_middle::import_attributes_resolution_mode(view, attributes)
        .map_err(Error::from)
}

pub(crate) fn normal_mode(
    name: &[u8],
    meta: &SourceFileMetaData,
    options: &CompilerOptions,
) -> ModuleKind {
    let resolution = options.module_resolution_kind();
    if !(ModuleResolutionKind::NODE16..=ModuleResolutionKind::NODE_NEXT).contains(&resolution)
        && !options.resolve_package_json_exports()
        && !options.resolve_package_json_imports()
    {
        return ModuleKind::NONE;
    }
    let implied = implied_for_emit(name, options.emit_module_kind(), meta);
    let emit = if implied == ModuleKind::NONE {
        options.emit_module_kind()
    } else {
        implied
    };
    if emit == ModuleKind::COMMON_JS {
        ModuleKind::COMMON_JS
    } else if emit.is_non_node_esm() || emit == ModuleKind::PRESERVE {
        ModuleKind::ESNEXT
    } else {
        ModuleKind::NONE
    }
}
pub(crate) fn jsx_runtime_import(
    view: AstView<'_>,
    source: NodeId,
    options: &CompilerOptions,
) -> Result<Option<JsString>, Error> {
    let state = view.source_file(source)?;
    let pragmas = state.pragmas()?;
    let source = pragmas
        .iter()
        .rev()
        .find(|pragma| pragma.name.as_bytes() == b"jsximportsource");
    let runtime = pragmas
        .iter()
        .rev()
        .find(|pragma| pragma.name.as_bytes() == b"jsxruntime");
    if pragma_argument(runtime) == b"classic" {
        return Ok(None);
    }
    if matches!(options.jsx, JsxEmit::REACT_JSX | JsxEmit::REACT_JSX_DEV)
        || !options.jsx_import_source.is_empty()
        || source.is_some()
        || pragma_argument(runtime) == b"automatic"
    {
        let mut base = pragma_argument(source);
        if base.is_empty() {
            base = options.jsx_import_source.as_bytes();
        }
        if base.is_empty() {
            base = b"react";
        }
        let mut name = base.to_vec();
        name.extend_from_slice(if options.jsx == JsxEmit::REACT_JSX_DEV {
            b"/jsx-dev-runtime"
        } else {
            b"/jsx-runtime"
        });
        return Ok(Some(JsString::from_bytes(name)));
    }
    Ok(None)
}

fn pragma_argument(pragma: Option<&ts_ast::Pragma>) -> &[u8] {
    pragma
        .and_then(|p| p.args.get(b"factory".as_slice()))
        .map_or(b"".as_slice(), |arg| arg.value.as_bytes())
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
    if (ModuleResolutionKind::NODE16..=ModuleResolutionKind::NODE_NEXT)
        .contains(&options.module_resolution_kind())
        || options.resolve_package_json_exports()
        || options.resolve_package_json_imports()
    {
        implied_for_emit(name, options.emit_module_kind(), meta)
    } else {
        ModuleKind::NONE
    }
}
