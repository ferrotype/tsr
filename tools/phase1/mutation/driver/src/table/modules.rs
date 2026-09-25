//! Group `modules`: modules, imports, type-only forms, augmentations, symbol names.
//! Go: `tools/phase1/tables/go/modules_columns.go`; spec:
//! `data/phase1/tables/modules.json`.
use super::helpers::{
    all, int, is_kind, node_map, node_predicate, ref_of, source_column, typed, values_map,
};
use super::{text, Column, Parsed};
use crate::protocol::hex;
use serde::Deserialize;
use serde_json::{json, Value};
use tsr_arena::Error;
use tsr_ast::utilities_middle::{get_pragma_argument, get_pragma_from_source_file};
use tsr_ast::utilities_modules as modules;
use tsr_ast::{AstView, NodeId, Pragma, SymbolAccess, SymbolId, SyntaxKind as K};
use tsr_core::{CompilerOptions, ModuleKind, Tristate};
use tsr_jsstring::JsString;

pub const COLUMNS: &[&str] = &[
    "ast.IsAnyExportAssignment",
    "ast.IsImportDeclarationOrJSImportDeclaration",
    "ast.EscapeAllInternalSymbolNames",
    "ast.EscapeInternalSymbolName",
    "ast.EscapeSymbolName",
    "ast.TryGetAmbientModuleNameFromSymbolName",
    "ast.IsAmbientModuleSymbolName",
    "ast.SymbolName",
    "ast.Symbol.CombinedLocalAndExportSymbolFlags",
    "ast.Symbol.IsExternalModule",
    "ast.GetNonAugmentationDeclaration",
    "ast.GetSourceFileOfModule",
    "ast.GetExternalModuleImportEqualsDeclarationExpression",
    "ast.GetExternalModuleName",
    "ast.GetImportAttributes",
    "ast.GetModuleSpecifierOfBareOrAccessedRequire",
    "ast.HasImportAttributes",
    "ast.HasResolutionModeOverride",
    "ast.ImportFromModuleSpecifier",
    "ast.TryGetImportFromModuleSpecifier",
    "ast.IsDefaultImport",
    "ast.IsEffectiveExternalModule",
    "ast.IsEmittableImport",
    "ast.IsExportNamespaceAsDefaultDeclaration",
    "ast.IsExternalModuleAugmentation",
    "ast.IsExternalModuleImportEqualsDeclaration",
    "ast.IsExternalModuleIndicator",
    "ast.IsInternalModuleImportEqualsDeclaration",
    "ast.IsModuleWithStringLiteralName",
    "ast.IsPartOfTypeOnlyImportOrExportDeclaration",
    "ast.IsTypeOnlyImportOrExportDeclaration",
    "ast.IsTypeOnlyImportDeclaration",
    "ast.IsPlainJSFile",
    "ast.IsRequireVariableStatement",
    "ast.IsValidTypeOnlyAliasUseSite",
    "ast.IsVariableDeclarationInitializedToBareOrAccessedRequire",
    "ast.GetPragmaFromSourceFile",
    "ast.GetPragmaArgument",
    "ast.GetEmitModuleFormatOfFileWorker",
];

/// Go's `pragmaNames`: the pragma names the pinned parser records.
const PRAGMA_NAMES: &[&[u8]] = &[
    b"reference",
    b"amd-dependency",
    b"amd-module",
    b"ts-check",
    b"ts-nocheck",
    b"jsx",
    b"jsxfrag",
    b"jsximportsource",
    b"jsxruntime",
];

/// Go's `pragmaArguments`: the argument names those pragmas carry.
const PRAGMA_ARGUMENTS: &[&[u8]] = &[
    b"path",
    b"types",
    b"lib",
    b"no-default-lib",
    b"resolution-mode",
    b"preserve",
    b"name",
    b"factory",
];

/// Go's `pragmaArgs`: a pragma's arguments sorted by name, or null.
fn pragma_args(pragma: Option<&Pragma>) -> Value {
    pragma.map_or(Value::Null, |pragma| {
        Value::Array(
            pragma
                .args
                .iter()
                .map(|(name, argument)| {
                    json!([hex(name.as_bytes()), hex(argument.value.as_bytes())])
                })
                .collect(),
        )
    })
}

/// Go's `pragmaValues`: `GetPragmaArgument` over every argument name, in order.
fn pragma_values(pragma: Option<&Pragma>) -> Value {
    Value::Array(
        PRAGMA_ARGUMENTS
            .iter()
            .map(|name| {
                json!(hex(get_pragma_argument(
                    pragma,
                    &JsString::from_bytes(*name)
                )))
            })
            .collect(),
    )
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct EmitFormatCase {
    file_name: String,
    module: i32,
    implied_node_format: i32,
    package_json_type: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct EmitFormatCases {
    cases: Vec<EmitFormatCase>,
}

/// Go's `moduleKinds`.
const MODULE_KINDS: &[ModuleKind] = &[
    ModuleKind::NONE,
    ModuleKind::COMMON_JS,
    ModuleKind::AMD,
    ModuleKind::UMD,
    ModuleKind::SYSTEM,
    ModuleKind::ES2015,
    ModuleKind::ES2020,
    ModuleKind::ES2022,
    ModuleKind::ESNEXT,
    ModuleKind::NODE16,
    ModuleKind::NODE18,
    ModuleKind::NODE20,
    ModuleKind::NODE_NEXT,
    ModuleKind::PRESERVE,
];

pub fn build(column: &str, input: &Value) -> Option<Result<Column, String>> {
    Some(match column {
        "ast.IsAnyExportAssignment" => node_predicate("source", input, all, |_, view, node| {
            Ok(modules::is_any_export_assignment(&view.node(node)?))
        }),
        "ast.IsImportDeclarationOrJSImportDeclaration" => {
            node_predicate("source", input, all, |_, view, node| {
                Ok(modules::is_import_declaration_or_js_import_declaration(
                    &view.node(node)?,
                ))
            })
        }
        "ast.EscapeAllInternalSymbolNames" => values_map(input, |name| {
            Ok(json!(hex(&tsr_ast::escape_all_internal_symbol_names(name))))
        }),
        "ast.EscapeInternalSymbolName" => values_map(input, |name| {
            Ok(json!(hex(&tsr_ast::escape_internal_symbol_name(name))))
        }),
        "ast.EscapeSymbolName" => values_map(input, |name| {
            Ok(json!(hex(&tsr_ast::escape_symbol_name(name))))
        }),
        "ast.TryGetAmbientModuleNameFromSymbolName" => values_map(input, |name| {
            Ok(tsr_ast::try_get_ambient_module_name_from_symbol_name(name)
                .map_or(Value::Null, |module| json!(hex(module))))
        }),
        "ast.IsAmbientModuleSymbolName" => values_map(input, |name| {
            Ok(json!(tsr_ast::is_ambient_module_symbol_name(name)))
        }),
        "ast.SymbolName" => node_map("bound", input, all, |parsed, view, node| {
            let Some((bound, symbol)) = symbol_of(parsed, node)? else {
                return Ok(Value::Null);
            };
            let symbol = bound.symbol(symbol).map_err(text)?;
            let name = tsr_ast::symbol_name(&symbol, view).map_err(text)?;
            Ok(json!(hex(&stable_symbol_name(name.as_bytes()))))
        }),
        "ast.Symbol.CombinedLocalAndExportSymbolFlags" => {
            node_map("bound", input, all, |parsed, _, node| {
                let Some((bound, symbol)) = symbol_of(parsed, node)? else {
                    return Ok(Value::Null);
                };
                let symbol = bound.symbol(symbol).map_err(text)?;
                let flags = symbol
                    .combined_local_and_export_symbol_flags(|id| Ok(bound.symbol(id)?.flags()))
                    .map_err(text)?;
                Ok(json!([flags, symbol.export_symbol().is_some()]))
            })
        }
        "ast.Symbol.IsExternalModule" => node_map("bound", input, all, |parsed, _, node| {
            let Some((bound, symbol)) = symbol_of(parsed, node)? else {
                return Ok(Value::Null);
            };
            Ok(json!(bound
                .symbol(symbol)
                .map_err(text)?
                .is_external_module()))
        }),
        "ast.GetNonAugmentationDeclaration" => {
            node_map("bound", input, all, |parsed, view, node| {
                let Some((bound, symbol)) = symbol_of(parsed, node)? else {
                    return Ok(Value::Null);
                };
                ref_of(
                    parsed,
                    modules::get_non_augmentation_declaration(view, bound, symbol).map_err(text)?,
                )
            })
        }
        "ast.GetSourceFileOfModule" => node_map("bound", input, all, |parsed, view, node| {
            let Some((bound, symbol)) = symbol_of(parsed, node)? else {
                return Ok(Value::Null);
            };
            if bound.symbol(symbol).map_err(text)?.flags() & tsr_ast::symbol_flags::MODULE == 0 {
                return Ok(Value::Null);
            }
            ref_of(
                parsed,
                modules::get_source_file_of_module(view, bound, symbol).map_err(text)?,
            )
        }),
        "ast.GetExternalModuleImportEqualsDeclarationExpression" => node_map(
            "source",
            input,
            modules::is_external_module_import_equals_declaration,
            |parsed, view, node| {
                ref_of(
                    parsed,
                    modules::get_external_module_import_equals_declaration_expression(view, node)
                        .map_err(text)?,
                )
            },
        ),
        "ast.GetExternalModuleName" => node_map(
            "source",
            input,
            external_module_name_kinds,
            |parsed, view, node| {
                ref_of(
                    parsed,
                    modules::get_external_module_name(view, node).map_err(text)?,
                )
            },
        ),
        "ast.GetImportAttributes" => node_map(
            "source",
            input,
            import_attributes_kinds,
            |parsed, view, node| {
                ref_of(
                    parsed,
                    modules::get_import_attributes(view, node).map_err(text)?,
                )
            },
        ),
        "ast.GetModuleSpecifierOfBareOrAccessedRequire" => {
            node_map("source", input, all, |parsed, view, node| {
                ref_of(
                    parsed,
                    modules::get_module_specifier_of_bare_or_accessed_require(view, node)
                        .map_err(text)?,
                )
            })
        }
        "ast.HasImportAttributes" => node_predicate("source", input, all, |_, view, node| {
            Ok(modules::has_import_attributes(&view.node(node)?))
        }),
        "ast.HasResolutionModeOverride" => node_predicate("source", input, all, |_, view, node| {
            modules::has_resolution_mode_override(view, Some(node))
        }),
        "ast.ImportFromModuleSpecifier" => node_map(
            "source",
            input,
            import_specifier_domain,
            |parsed, view, node| {
                ref_of(
                    parsed,
                    modules::import_from_module_specifier(view, node).map_err(text)?,
                )
            },
        ),
        "ast.TryGetImportFromModuleSpecifier" => node_map(
            "source",
            input,
            string_literal_kinds,
            |parsed, view, node| {
                ref_of(
                    parsed,
                    modules::try_get_import_from_module_specifier(view, node).map_err(text)?,
                )
            },
        ),
        "ast.IsDefaultImport" => node_predicate("source", input, all, |_, view, node| {
            tsr_ast::utilities_middle::is_default_import(view, &view.node(node)?)
        }),
        "ast.IsEffectiveExternalModule" => {
            node_map("bound", input, source_file_kind, |_, view, node| {
                let file = view.source_file(node).map_err(text)?;
                let mut bits = 0;
                for (bit, kind) in MODULE_KINDS.iter().enumerate() {
                    let options = CompilerOptions {
                        module: *kind,
                        ..CompilerOptions::default()
                    };
                    if modules::is_effective_external_module(&file, &options) {
                        bits |= 1 << bit;
                    }
                }
                Ok(int(bits))
            })
        }
        "ast.IsEmittableImport" => node_predicate("source", input, all, |_, view, node| {
            modules::is_emittable_import(view, node)
        }),
        "ast.IsExportNamespaceAsDefaultDeclaration" => {
            node_predicate("source", input, export_clause_domain, |_, view, node| {
                modules::is_export_namespace_as_default_declaration(view, node)
            })
        }
        "ast.IsExternalModuleAugmentation" => {
            node_predicate("source", input, all, |_, view, node| {
                modules::is_external_module_augmentation(view, node)
            })
        }
        "ast.IsExternalModuleImportEqualsDeclaration" => {
            node_predicate("source", input, all, |_, view, node| {
                modules::is_external_module_import_equals_declaration(view, node)
            })
        }
        "ast.IsExternalModuleIndicator" => node_predicate("source", input, all, |_, view, node| {
            modules::is_external_module_indicator(view, node)
        }),
        "ast.IsInternalModuleImportEqualsDeclaration" => {
            node_predicate("source", input, all, |_, view, node| {
                tsr_ast::utilities_middle::is_internal_module_import_equals_declaration(
                    view,
                    &view.node(node)?,
                )
            })
        }
        "ast.IsModuleWithStringLiteralName" => {
            node_predicate("source", input, all, |_, view, node| {
                modules::is_module_with_string_literal_name(view, node)
            })
        }
        "ast.IsPartOfTypeOnlyImportOrExportDeclaration" => {
            node_predicate("source", input, all, |_, view, node| {
                modules::is_part_of_type_only_import_or_export_declaration(view, node)
            })
        }
        "ast.IsTypeOnlyImportOrExportDeclaration" => {
            node_predicate("source", input, all, |_, view, node| {
                modules::is_type_only_import_or_export_declaration(view, node)
            })
        }
        "ast.IsTypeOnlyImportDeclaration" => {
            node_predicate("source", input, all, |_, view, node| {
                modules::is_type_only_import_declaration(view, node)
            })
        }
        "ast.IsPlainJSFile" => node_map("source", input, source_file_kind, |_, view, node| {
            let file = view.source_file(node).map_err(text)?;
            let mut bits = 0;
            for (bit, check_js) in [Tristate::UNKNOWN, Tristate::FALSE, Tristate::TRUE]
                .into_iter()
                .enumerate()
            {
                if tsr_ast::utilities_middle::is_plain_js_file(Some(&file), check_js) {
                    bits |= 1 << bit;
                }
            }
            Ok(int(bits))
        }),
        "ast.IsRequireVariableStatement" => {
            node_predicate("source", input, all, |_, view, node| {
                modules::is_require_variable_statement(view, node)
            })
        }
        "ast.IsValidTypeOnlyAliasUseSite" => {
            node_predicate("source_jsdoc", input, all, |_, view, node| {
                modules::is_valid_type_only_alias_use_site(view, node)
            })
        }
        "ast.IsVariableDeclarationInitializedToBareOrAccessedRequire" => {
            node_predicate("source", input, all, |_, view, node| {
                tsr_ast::is_variable_declaration_initialized_to_bare_or_accessed_require(view, node)
            })
        }
        "ast.GetPragmaFromSourceFile" => source_column(input, |parsed| {
            let view = parsed.view();
            let state = view.source_file(parsed.root()).map_err(text)?;
            let pragmas = state.pragmas().map_err(text)?;
            let mut out = Vec::new();
            for name in PRAGMA_NAMES {
                out.push(pragma_args(get_pragma_from_source_file(
                    pragmas.iter(),
                    name,
                )));
            }
            Ok(Value::Array(out))
        }),
        "ast.GetPragmaArgument" => source_column(input, |parsed| {
            let view = parsed.view();
            let state = view.source_file(parsed.root()).map_err(text)?;
            let pragmas = state.pragmas().map_err(text)?;
            let mut out = Vec::new();
            for pragma in pragmas.iter() {
                out.push(json!([
                    hex(pragma.name.as_bytes()),
                    pragma_values(Some(pragma))
                ]));
            }
            out.push(json!([Value::Null, pragma_values(None)]));
            Ok(Value::Array(out))
        }),
        "ast.GetEmitModuleFormatOfFileWorker" => typed::<EmitFormatCases>(input, |input| {
            let mut out = Vec::new();
            for case in &input.cases {
                let options = CompilerOptions {
                    module: ModuleKind(case.module),
                    ..CompilerOptions::default()
                };
                let meta = tsr_ast::SourceFileMetaData {
                    package_json_type: JsString::from_bytes(case.package_json_type.as_bytes()),
                    implied_node_format: ModuleKind(case.implied_node_format),
                    ..tsr_ast::SourceFileMetaData::default()
                };
                out.push(json!(
                    tsr_ast::emit_module_format_of_file(case.file_name.as_bytes(), &options, &meta)
                        .0
                ));
            }
            Ok(Value::Array(out))
        }),
        _ => return None,
    })
}

/// Go's `stableSymbolName`: a pattern ambient module's node id blanked.
fn stable_symbol_name(name: &[u8]) -> Vec<u8> {
    const MARKER: &[u8] = b"\"pattern@";
    match name.windows(MARKER.len()).rposition(|part| part == MARKER) {
        Some(at) => [&name[..at], MARKER, b"#"].concat(),
        None => name.to_vec(),
    }
}

/// Go's `symbolOf`: the bound node's symbol, if any.
fn symbol_of(
    parsed: &Parsed,
    node: NodeId,
) -> Result<Option<(tsr_ast::BoundView<'_>, SymbolId)>, String> {
    let bound = parsed.bound().ok_or("the input is not bound")?;
    Ok(bound
        .node_binding(node)
        .map_err(text)?
        .and_then(|binding| binding.symbol)
        .map(|symbol| (bound, symbol)))
}

fn source_file_kind(view: AstView<'_>, node: NodeId) -> Result<bool, Error> {
    is_kind(view, node, &[K::SourceFile])
}

fn string_literal_kinds(view: AstView<'_>, node: NodeId) -> Result<bool, Error> {
    is_kind(
        view,
        node,
        &[K::StringLiteral, K::NoSubstitutionTemplateLiteral],
    )
}

fn external_module_name_kinds(view: AstView<'_>, node: NodeId) -> Result<bool, Error> {
    is_kind(
        view,
        node,
        &[
            K::ImportDeclaration,
            K::JSImportDeclaration,
            K::ExportDeclaration,
            K::ImportEqualsDeclaration,
            K::ImportType,
            K::CallExpression,
            K::ModuleDeclaration,
        ],
    )
}

fn import_attributes_kinds(view: AstView<'_>, node: NodeId) -> Result<bool, Error> {
    is_kind(
        view,
        node,
        &[
            K::ImportDeclaration,
            K::JSImportDeclaration,
            K::ExportDeclaration,
            K::ImportType,
        ],
    )
}

/// Go's filter of `ast.ImportFromModuleSpecifier`.
fn import_specifier_domain(view: AstView<'_>, node: NodeId) -> Result<bool, Error> {
    Ok(
        tsr_ast::utilities::is_string_literal_like(&view.node(node)?)
            && modules::try_get_import_from_module_specifier(view, node)?.is_some(),
    )
}

/// Go's `exportClauseDomain`.
fn export_clause_domain(view: AstView<'_>, node: NodeId) -> Result<bool, Error> {
    let read = view.node(node)?;
    if read.kind() != K::ExportDeclaration {
        return Ok(true);
    }
    Ok(read
        .data_source()
        .as_export_declaration()
        .ok_or(Error::InvalidGraph)?
        .export_clause()
        .is_some())
}
