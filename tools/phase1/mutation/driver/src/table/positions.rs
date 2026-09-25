//! Group `positions`: names, type and expression positions, access kinds.
//! Go: `tools/phase1/tables/go/positions_columns.go`; spec:
//! `data/phase1/tables/positions.json`.
use super::helpers::{all, int, node_map, node_predicate, ref_of};
use super::{decode, text, Column, Parsed};
use crate::protocol::unhex;
use serde_json::{json, Value};
use tsr_arena::Error;
use tsr_ast::utilities_positions as positions;
use tsr_ast::{AstView, NodeId, SyntaxKind as K};
use tsr_jsstring::PositionMap;

pub const COLUMNS: &[&str] = &[
    "ast.IsDeclarationName",
    "ast.GetDeclarationFromName",
    "ast.IsArrayLiteralOrObjectLiteralDestructuringPattern",
    "ast.IsTypeOrJSTypeAliasDeclaration",
    "ast.IsWriteAccess",
    "ast.IsWriteOnlyAccess",
    "ast.IsWriteAccessForReference",
    "ast.GetMeaningFromDeclaration",
    "ast.IsArrayBindingOrAssignmentElement",
    "ast.IsDeclarationNameOrImportPropertyName",
    "ast.IsExpression",
    "ast.IsExpressionNode",
    "ast.IsInExpressionContext",
    "ast.IsLiteralComputedPropertyDeclarationName",
    "ast.IsParseTreeNode",
    "ast.IsPartOfTypeNode",
    "ast.IsThisInTypeQuery",
    "ast.IsTypeDeclaration",
    "ast.IsTypeDeclarationName",
    "ast.SkipTypeParentheses",
    "ast.TryGetPropertyNameOfBindingOrAssignmentElement",
    "ast.ComputePositionMap",
];

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct Texts {
    texts_hex: Vec<String>,
}

pub fn build(column: &str, input: &Value) -> Option<Result<Column, String>> {
    Some(match column {
        "ast.IsDeclarationName" => node_predicate("source", input, all, |_, view, node| {
            positions::is_declaration_name(view, node)
        }),
        "ast.GetDeclarationFromName" => {
            node_map("bound_jsdoc", input, all, |parsed, view, node| {
                let decl = positions::get_declaration_from_name(view, parsed.bound(), Some(node))
                    .map_err(text)?;
                ref_of(parsed, decl)
            })
        }
        "ast.IsArrayLiteralOrObjectLiteralDestructuringPattern" => {
            node_predicate("source", input, all, |_, view, node| {
                positions::is_array_literal_or_object_literal_destructuring_pattern(view, node)
            })
        }
        "ast.IsTypeOrJSTypeAliasDeclaration" => {
            node_predicate("source", input, all, |_, view, node| {
                Ok(positions::is_type_or_js_type_alias_declaration(
                    &view.node(node)?,
                ))
            })
        }
        "ast.IsWriteAccess" => node_predicate("source", input, all, |_, view, node| {
            tsr_ast::utilities::is_write_access(view, node)
        }),
        "ast.IsWriteOnlyAccess" => node_predicate("source", input, all, |_, view, node| {
            tsr_ast::utilities::is_write_only_access(view, node)
        }),
        "ast.IsWriteAccessForReference" => {
            node_map("bound_jsdoc", input, all, |parsed, view, node| {
                if !write_access_decided(parsed, view, node).map_err(text)? {
                    return Ok(Value::Null);
                }
                Ok(json!(positions::is_write_access_for_reference(
                    view,
                    parsed.bound(),
                    node
                )
                .map_err(text)?))
            })
        }
        "ast.GetMeaningFromDeclaration" => node_map("source", input, all, |_, view, node| {
            let meaning = positions::get_meaning_from_declaration(view, node).map_err(text)?;
            if meaning == positions::semantic_meaning::ALL {
                return Ok(Value::Null);
            }
            Ok(int(i64::from(meaning)))
        }),
        "ast.IsArrayBindingOrAssignmentElement" => {
            node_predicate("source", input, all, |_, view, node| {
                positions::is_array_binding_or_assignment_element(view, node)
            })
        }
        "ast.IsDeclarationNameOrImportPropertyName" => {
            node_predicate("source", input, not_source_file, |_, view, node| {
                positions::is_declaration_name_or_import_property_name(view, node)
            })
        }
        "ast.IsExpression" => node_predicate("source", input, all, |_, view, node| {
            positions::is_expression(view, node)
        }),
        "ast.IsExpressionNode" => node_predicate("source_jsdoc", input, all, |_, view, node| {
            positions::is_expression_node(view, node)
        }),
        "ast.IsInExpressionContext" => node_predicate(
            "source_jsdoc",
            input,
            expression_context_domain,
            |_, view, node| positions::is_in_expression_context(view, node),
        ),
        "ast.IsLiteralComputedPropertyDeclarationName" => {
            node_predicate("source", input, all, |_, view, node| {
                positions::is_literal_computed_property_declaration_name(view, node)
            })
        }
        "ast.IsParseTreeNode" => node_map("source", input, all, |_, view, node| {
            Ok(json!(positions::is_parse_tree_node(
                &view.node(node).map_err(text)?
            )))
        }),
        "ast.IsPartOfTypeNode" => node_predicate("source_jsdoc", input, all, |_, view, node| {
            positions::is_part_of_type_node(view, node)
        }),
        "ast.IsThisInTypeQuery" => node_predicate("source", input, all, |_, view, node| {
            positions::is_this_in_type_query(view, node)
        }),
        "ast.IsTypeDeclaration" => node_predicate("source", input, all, |_, view, node| {
            positions::is_type_declaration(view, node)
        }),
        "ast.IsTypeDeclarationName" => node_predicate("source", input, all, |_, view, node| {
            positions::is_type_declaration_name(view, node)
        }),
        "ast.SkipTypeParentheses" => node_map("source", input, all, |parsed, view, node| {
            let skipped = positions::skip_type_parentheses(view, node).map_err(text)?;
            if skipped == node {
                return Ok(Value::Null);
            }
            ref_of(parsed, Some(skipped))
        }),
        "ast.TryGetPropertyNameOfBindingOrAssignmentElement" => {
            node_map("source", input, all, |parsed, view, node| {
                let name =
                    positions::try_get_property_name_of_binding_or_assignment_element(view, node)
                        .map_err(text)?;
                ref_of(parsed, name)
            })
        }
        "ast.ComputePositionMap" => compute_position_map(input),
        _ => return None,
    })
}

/// Go's `notSourceFile`.
fn not_source_file(view: AstView<'_>, node: NodeId) -> Result<bool, Error> {
    Ok(view.node(node)?.kind() != K::SourceFile)
}

/// Go's `expressionContextDomain`.
fn expression_context_domain(view: AstView<'_>, node: NodeId) -> Result<bool, Error> {
    let read = view.node(node)?;
    let Some(parent) = read.parent() else {
        return Ok(false);
    };
    Ok(view.node(parent)?.kind() != K::DefaultClause)
}

/// Go's `writeAccessDecided`: whether `declarationIsWriteAccess` returns for
/// the declaration `GetDeclarationFromName` finds.
fn write_access_decided(parsed: &Parsed, view: AstView<'_>, node: NodeId) -> Result<bool, Error> {
    let Some(decl) = positions::get_declaration_from_name(view, parsed.bound(), Some(node))? else {
        return Ok(true);
    };
    let read = view.node(decl)?;
    if read.flags() & tsr_ast::node_flags::AMBIENT != 0 {
        return Ok(true);
    }
    Ok(matches!(
        read.kind().known(),
        Some(
            K::BinaryExpression
                | K::BindingElement
                | K::ClassDeclaration
                | K::ClassExpression
                | K::DefaultKeyword
                | K::EnumDeclaration
                | K::EnumMember
                | K::ExportSpecifier
                | K::ImportClause
                | K::ImportEqualsDeclaration
                | K::ImportSpecifier
                | K::InterfaceDeclaration
                | K::JSDocCallbackTag
                | K::JSDocTypedefTag
                | K::JsxAttribute
                | K::ModuleDeclaration
                | K::NamespaceExportDeclaration
                | K::NamespaceImport
                | K::NamespaceExport
                | K::Parameter
                | K::ShorthandPropertyAssignment
                | K::TypeAliasDeclaration
                | K::JSTypeAliasDeclaration
                | K::TypeParameter
                | K::PropertyAssignment
                | K::FunctionDeclaration
                | K::FunctionExpression
                | K::Constructor
                | K::MethodDeclaration
                | K::GetAccessor
                | K::SetAccessor
                | K::VariableDeclaration
                | K::PropertyDeclaration
                | K::MethodSignature
                | K::PropertySignature
                | K::JSDocPropertyTag
                | K::JSDocParameterTag
        )
    ))
}

/// Go's `ast.ComputePositionMap` column.
fn compute_position_map(input: &Value) -> Result<Column, String> {
    let texts: Texts = decode(input)?;
    Ok(Box::new(move || {
        let mut out = Vec::new();
        for raw in &texts.texts_hex {
            let text = unhex(&json!(raw))?;
            let map = PositionMap::new(&text);
            let last = isize::try_from(text.len()).map_err(|error| error.to_string())? + 1;
            let (mut to_utf16, mut to_utf8) = (Vec::new(), Vec::new());
            for offset in -1..=last {
                to_utf16.push(json!(map.utf8_to_utf16(offset)));
                to_utf8.push(json!(map.utf16_to_utf8(offset)));
            }
            out.push(json!([map.is_ascii_only(), to_utf16, to_utf8]));
        }
        Ok(Value::Array(out))
    }))
}
