//! Group `containers`: containers, function flags, precedence, reparse
//! identity, source-file tables, outer expressions.
//! Go: `tools/phase1/tables/go/containers_columns.go`; spec:
//! `data/phase1/tables/containers.json`.
use super::{parse_source, text, Column, Parsed};
use serde_json::{json, Value};
use tsr_ast::{NodeId, SyntaxKind as K};

pub const COLUMNS: &[&str] = &[
    "binder.FindUseStrictPrologue",
    "ast.GetDeclarationName",
    "ast.SourceFile.GetDeclarationMap",
    "ast.SourceFile.GetNameTable",
    "ast.SourceFile.HasIdentifier",
    "ast.GetOrComputeSourceFileData",
    "ast.GetFunctionFlags",
    "ast.GetExpressionPrecedence",
    "ast.GetLeftmostExpression",
    "ast.GetTypeNodePrecedence",
    "ast.ForEachReturnStatement",
    "ast.GetContainingFunction",
    "ast.GetEnclosingBlockScopeContainer",
    "ast.IsBlockScope",
    "ast.GetNewTargetContainer",
    "ast.GetReparsedNodeForNode",
    "ast.GetSuperContainer",
    "ast.HasContextSensitiveParameters",
    "ast.IsJSDocTypeAssertion",
    "ast.IsOuterExpression",
    "ast.SkipOuterExpressions",
    "ast.SkipPartiallyEmittedExpressions",
];

pub fn build(column: &str, input: &Value) -> Option<Result<Column, String>> {
    Some(match column {
        "binder.FindUseStrictPrologue" => find_use_strict_prologue(input),
        _ => return more(column, input),
    })
}

/// Statement containers: document-order index and statement list.
type Containers = Vec<(usize, Vec<Option<NodeId>>)>;

/// The statement containers (Go's `Node.CanHaveStatements`, ast.go:604) with
/// their statement lists, taken in setup.
fn statement_containers(parsed: &Parsed) -> Result<Containers, String> {
    let view = parsed.view();
    let mut found = Vec::new();
    for (at, id) in parsed.nodes.iter().enumerate() {
        let node = view.node(*id).map_err(text)?;
        if matches!(
            node.kind().known(),
            Some(K::SourceFile | K::Block | K::ModuleBlock | K::CaseClause | K::DefaultClause)
        ) {
            let statements = view
                .node_slice(node.statements(view).map_err(text)?)
                .map_err(text)?
                .iter()
                .collect();
            found.push((at, statements));
        }
    }
    Ok(found)
}

/// `[container, prologue]` of every container whose result is not nil.
fn find_use_strict_prologue(input: &Value) -> Result<Column, String> {
    let parsed = parse_source(input)?;
    let containers = statement_containers(&parsed)?;
    Ok(Box::new(move || {
        let view = parsed.view();
        let mut out = Vec::new();
        for (at, statements) in &containers {
            if let Some(found) =
                tsr_binder::find_use_strict_prologue(view, parsed.root(), statements)
            {
                out.push(json!([at, parsed.node_ref(Some(found))?]));
            }
        }
        Ok(Value::Array(out))
    }))
}

use super::helpers::{all, int, is_kind, node_map, node_predicate, ref_of, refs_of};
use crate::protocol::hex;
use tsr_arena::Error;
use tsr_ast::source_file_tables as tables;
use tsr_ast::utilities::outer_expression_kinds as oek;
use tsr_ast::utilities_containers as containers;
use tsr_ast::{AstView, FactoryMethods};

/// Go's `outerKinds`.
const OUTER_KINDS: &[u16] = &[
    oek::PARENTHESES,
    oek::TYPE_ASSERTIONS,
    oek::NON_NULL_ASSERTIONS,
    oek::PARTIALLY_EMITTED_EXPRESSIONS,
    oek::EXPRESSIONS_WITH_TYPE_ARGUMENTS,
    oek::SATISFIES,
    oek::PARENTHESES | oek::EXCLUDE_JSDOC_TYPE_ASSERTION,
    oek::ASSIGNMENTS,
    oek::COMMA,
    oek::ALL,
];

/// Go's `typeNodePrecedenceKinds`.
const TYPE_NODE_PRECEDENCE_KINDS: &[K] = &[
    K::ConditionalType,
    K::JSDocOptionalType,
    K::JSDocVariadicType,
    K::FunctionType,
    K::ConstructorType,
    K::UnionType,
    K::IntersectionType,
    K::TypeOperator,
    K::InferType,
    K::IndexedAccessType,
    K::ArrayType,
    K::OptionalType,
    K::TypeQuery,
    K::AnyKeyword,
    K::UnknownKeyword,
    K::StringKeyword,
    K::NumberKeyword,
    K::BigIntKeyword,
    K::SymbolKeyword,
    K::BooleanKeyword,
    K::UndefinedKeyword,
    K::NeverKeyword,
    K::ObjectKeyword,
    K::IntrinsicKeyword,
    K::VoidKeyword,
    K::JSDocAllType,
    K::JSDocNullableType,
    K::JSDocNonNullableType,
    K::LiteralType,
    K::TypePredicate,
    K::TypeReference,
    K::TypeLiteral,
    K::TupleType,
    K::RestType,
    K::ParenthesizedType,
    K::ThisType,
    K::MappedType,
    K::NamedTupleMember,
    K::TemplateLiteralType,
    K::ImportType,
    K::PropertyAccessExpression,
    K::ExpressionWithTypeArguments,
];

fn more(column: &str, input: &Value) -> Option<Result<Column, String>> {
    Some(match column {
        "ast.GetDeclarationName" => node_map("source", input, all, |_, view, node| {
            let name = tables::get_declaration_name(view, node).map_err(text)?;
            Ok(if name.is_empty() {
                Value::Null
            } else {
                json!(hex(&name))
            })
        }),
        "ast.SourceFile.GetDeclarationMap" => {
            node_map("bound", input, source_file_kind, |parsed, view, node| {
                let map = tables::get_declaration_map(view, parsed.bound(), node).map_err(text)?;
                let mut keys: Vec<_> = map.keys().cloned().collect();
                keys.sort();
                keys.into_iter()
                    .map(|key| Ok(json!([hex(&key), parsed.refs(map[&key].iter().copied())?])))
                    .collect::<Result<Vec<_>, String>>()
                    .map(Value::Array)
            })
        }
        "ast.SourceFile.GetNameTable" => {
            node_map("source_jsdoc", input, source_file_kind, |_, view, node| {
                let mut jsdoc = tsr_parser::ParserJsDocProvider::default();
                let table = tables::get_name_table(view, &mut jsdoc, node).map_err(text)?;
                let mut keys: Vec<_> = table.keys().cloned().collect();
                keys.sort();
                Ok(Value::Array(
                    keys.into_iter()
                        .map(|key| json!([hex(&key), table[&key]]))
                        .collect(),
                ))
            })
        }
        "ast.SourceFile.HasIdentifier" => {
            node_map("source", input, source_file_kind, |parsed, view, node| {
                let mut probes = std::collections::BTreeSet::new();
                for id in &parsed.nodes {
                    if is_kind(
                        view,
                        *id,
                        &[
                            K::Identifier,
                            K::PrivateIdentifier,
                            K::StringLiteral,
                            K::NumericLiteral,
                            K::BigIntLiteral,
                            K::NoSubstitutionTemplateLiteral,
                        ],
                    )
                    .map_err(text)?
                    {
                        probes.insert(view.node_text(*id).map_err(text)?.as_bytes().to_vec());
                    }
                }
                let mut probes: Vec<Vec<u8>> = probes.into_iter().collect();
                probes.push(Vec::new());
                probes.push(b"\x00absent".to_vec());
                probes
                    .iter()
                    .map(|name| {
                        Ok(json!(
                            tables::has_identifier(view, node, name).map_err(text)?
                        ))
                    })
                    .collect::<Result<Vec<_>, String>>()
                    .map(Value::Array)
            })
        }
        "ast.GetOrComputeSourceFileData" => {
            node_map("source", input, source_file_kind, |_, view, node| {
                let file = view.source_file(node).map_err(text)?;
                let statements = view
                    .node(node)
                    .map_err(text)?
                    .statements(view)
                    .map_err(text)?
                    .len() as i64;
                let (first, second) = (
                    tables::new_source_file_data_key(),
                    tables::new_source_file_data_key(),
                );
                let calls = std::cell::Cell::new(0i64);
                let compute = || {
                    calls.set(calls.get() + 1);
                    calls.get() * 1000 + statements
                };
                let a = tables::get_or_compute_source_file_data(&file.data, Some(first), compute);
                let b = tables::get_or_compute_source_file_data(&file.data, Some(first), compute);
                let c = tables::get_or_compute_source_file_data(&file.data, Some(second), compute);
                Ok(json!([a, b, c, calls.get()]))
            })
        }
        "ast.GetFunctionFlags" => node_map("source", input, all, |_, view, node| {
            Ok(json!(
                containers::get_function_flags(view, Some(node)).map_err(text)?
            ))
        }),
        "ast.GetExpressionPrecedence" => node_map("source", input, all, |_, view, node| {
            let read = view.node(node).map_err(text)?;
            Ok(json!(tsr_ast::precedence::get_expression_precedence(
                view, &read
            )
            .map_err(text)?))
        }),
        "ast.GetLeftmostExpression" => node_map("source", input, all, |parsed, view, node| {
            let through =
                tsr_ast::precedence::get_leftmost_expression(view, node, false).map_err(text)?;
            let stopping =
                tsr_ast::precedence::get_leftmost_expression(view, node, true).map_err(text)?;
            if through == node && stopping == node {
                return Ok(Value::Null);
            }
            Ok(json!([
                parsed.node_ref(Some(through))?,
                parsed.node_ref(Some(stopping))?
            ]))
        }),
        "ast.GetTypeNodePrecedence" => node_map(
            "source",
            input,
            type_node_precedence_kinds,
            |_, view, node| {
                Ok(json!(tsr_ast::precedence::get_type_node_precedence(
                    view, node
                )
                .map_err(text)?))
            },
        ),
        "ast.ForEachReturnStatement" => {
            node_map("source", input, block_kind, |parsed, view, node| {
                let mut all_returns = Vec::new();
                containers::for_each_return_statement(view, node, &mut |statement| {
                    all_returns.push(statement);
                    false
                })
                .map_err(text)?;
                let mut first = None;
                containers::for_each_return_statement(view, node, &mut |statement| {
                    first = Some(statement);
                    true
                })
                .map_err(text)?;
                if all_returns.is_empty() && first.is_none() {
                    return Ok(Value::Null);
                }
                Ok(json!([
                    refs_of(parsed, all_returns)?,
                    ref_of(parsed, first)?
                ]))
            })
        }
        "ast.GetContainingFunction" => {
            node_map("source", input, not_source_file, |parsed, view, node| {
                ref_of(
                    parsed,
                    containers::get_containing_function(view, node).map_err(text)?,
                )
            })
        }
        "ast.GetEnclosingBlockScopeContainer" => {
            node_map("source", input, all, |parsed, view, node| {
                ref_of(
                    parsed,
                    containers::get_enclosing_block_scope_container(view, node).map_err(text)?,
                )
            })
        }
        "ast.IsBlockScope" => node_predicate("source", input, all, |_, view, node| {
            containers::is_block_scope(view, node, view.node(node)?.parent())
        }),
        "ast.GetNewTargetContainer" => {
            node_map("source", input, not_source_file, |parsed, view, node| {
                ref_of(
                    parsed,
                    containers::get_new_target_container(view, node).map_err(text)?,
                )
            })
        }
        "ast.GetReparsedNodeForNode" => {
            node_map("source_jsdoc", input, all, |parsed, view, node| {
                let result =
                    containers::get_reparsed_node_for_node(view, Some(node)).map_err(text)?;
                if result == Some(node) {
                    return Ok(Value::Null);
                }
                parsed.node_ref_or_fields(result)
            })
        }
        "ast.GetSuperContainer" => node_map("source", input, all, |parsed, view, node| {
            let through = containers::get_super_container(view, node, false).map_err(text)?;
            let stopping = containers::get_super_container(view, node, true).map_err(text)?;
            if through.is_none() && stopping.is_none() {
                return Ok(Value::Null);
            }
            Ok(json!([ref_of(parsed, through)?, ref_of(parsed, stopping)?]))
        }),
        "ast.HasContextSensitiveParameters" => {
            node_predicate("source", input, function_like_data, |_, view, node| {
                containers::has_context_sensitive_parameters(view, node)
            })
        }
        "ast.IsJSDocTypeAssertion" => node_predicate("source", input, all, |_, view, node| {
            tsr_ast::utilities::is_jsdoc_type_assertion(view, Some(node))
        }),
        "ast.IsOuterExpression" => node_map("source", input, all, |_, view, node| {
            let mut bits = 0;
            for (bit, kinds) in OUTER_KINDS.iter().enumerate() {
                if tsr_ast::utilities::is_outer_expression(view, node, *kinds).map_err(text)? {
                    bits |= 1 << bit;
                }
            }
            Ok(int(bits))
        }),
        "ast.SkipOuterExpressions" => node_map("source", input, all, |parsed, view, node| {
            let mut out = Vec::new();
            let mut moved = false;
            for kinds in OUTER_KINDS {
                let result =
                    tsr_ast::utilities::skip_outer_expressions(view, node, *kinds).map_err(text)?;
                moved |= result != node;
                out.push(parsed.node_ref(Some(result))?);
            }
            Ok(if moved {
                Value::Array(out)
            } else {
                Value::Null
            })
        }),
        "ast.SkipPartiallyEmittedExpressions" => skip_partially_emitted_expressions(input),
        _ => return None,
    })
}

/// Go's `ast.SkipPartiallyEmittedExpressions` column: two partially emitted
/// expressions a factory (the input's builder) wraps around each expression.
fn skip_partially_emitted_expressions(input: &Value) -> Result<Column, String> {
    let mut parsed = parse_source(input)?;
    Ok(Box::new(move || {
        let mut out = Vec::new();
        for at in 0..parsed.nodes.len() {
            let node = parsed.nodes[at];
            if !tsr_ast::utilities_positions::is_expression_node(parsed.view(), node)
                .map_err(text)?
            {
                continue;
            }
            let builder = parsed.builder_mut()?;
            let inner = builder.new_partially_emitted_expression(Some(node));
            let wrapped = builder.new_partially_emitted_expression(Some(inner));
            let view = parsed.view();
            let skipped =
                tsr_ast::skip_partially_emitted_expressions(view, wrapped).map_err(text)?;
            let itself = tsr_ast::skip_partially_emitted_expressions(view, node).map_err(text)?;
            let itself = if itself == node {
                Value::Null
            } else {
                parsed.node_ref_or_fields(Some(itself))?
            };
            out.push(json!([
                at,
                [parsed.node_ref_or_fields(Some(skipped))?, itself]
            ]));
        }
        Ok(Value::Array(out))
    }))
}

fn source_file_kind(view: AstView<'_>, node: NodeId) -> Result<bool, Error> {
    is_kind(view, node, &[K::SourceFile])
}

fn block_kind(view: AstView<'_>, node: NodeId) -> Result<bool, Error> {
    is_kind(view, node, &[K::Block])
}

fn type_node_precedence_kinds(view: AstView<'_>, node: NodeId) -> Result<bool, Error> {
    is_kind(view, node, TYPE_NODE_PRECEDENCE_KINDS)
}

fn not_source_file(view: AstView<'_>, node: NodeId) -> Result<bool, Error> {
    Ok(view.node(node)?.kind() != K::SourceFile)
}

/// Go's `functionLikeData`.
fn function_like_data(view: AstView<'_>, node: NodeId) -> Result<bool, Error> {
    is_kind(
        view,
        node,
        &[
            K::GetAccessor,
            K::SetAccessor,
            K::ArrowFunction,
            K::CallSignature,
            K::ConstructSignature,
            K::Constructor,
            K::ConstructorType,
            K::FunctionDeclaration,
            K::FunctionExpression,
            K::FunctionType,
            K::IndexSignature,
            K::JSDocSignature,
            K::MethodDeclaration,
            K::MethodSignature,
        ],
    )
}
