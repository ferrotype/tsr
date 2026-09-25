//! Group `accessors`: per-kind node accessors, list and flow helpers.
//! Go: `tools/phase1/tables/go/accessors_columns.go`; spec:
//! `data/phase1/tables/accessors.json`.
use super::helpers::{all, is_kind, node_map, node_predicate, ref_of, refs_of};
use super::{decode, text, Column, Parsed};
use serde::Deserialize;
use serde_json::{json, Value};
use tsr_arena::Error;
use tsr_ast::{AstView, NodeDataRead, NodeId, NodeListId, NodeSlice, SyntaxKind as K};

pub const COLUMNS: &[&str] = &[
    "ast.Node.Attributes",
    "ast.Node.Children",
    "ast.Node.ClassName",
    "ast.Node.CommentList",
    "ast.Node.Comments",
    "ast.Node.TypeExpression",
    "ast.Node.Statement",
    "ast.IsStatement",
    "ast.NodeList.HasTrailingComma",
    "ast.FlowSwitchClauseData.IsEmpty",
];

/// Go's `jsdocCommentKinds`.
const JSDOC_COMMENT_KINDS: &[K] = &[
    K::JSDoc,
    K::JSDocUnknownTag,
    K::JSDocAugmentsTag,
    K::JSDocImplementsTag,
    K::JSDocDeprecatedTag,
    K::JSDocPublicTag,
    K::JSDocPrivateTag,
    K::JSDocProtectedTag,
    K::JSDocReadonlyTag,
    K::JSDocOverrideTag,
    K::JSDocCallbackTag,
    K::JSDocOverloadTag,
    K::JSDocParameterTag,
    K::JSDocPropertyTag,
    K::JSDocReturnTag,
    K::JSDocThisTag,
    K::JSDocTypeTag,
    K::JSDocTemplateTag,
    K::JSDocTypedefTag,
    K::JSDocSeeTag,
    K::JSDocSatisfiesTag,
    K::JSDocThrowsTag,
    K::JSDocImportTag,
];

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Clauses {
    clauses: Vec<[i64; 2]>,
}

fn nodes(view: AstView<'_>, slice: NodeSlice) -> Result<Vec<NodeId>, String> {
    Ok(view
        .node_slice(slice)
        .map_err(text)?
        .iter()
        .flatten()
        .collect())
}

/// Go's `listRefs`.
fn list_refs(
    parsed: &Parsed,
    view: AstView<'_>,
    list: Option<NodeListId>,
) -> Result<Value, String> {
    match list {
        None => Ok(Value::Null),
        Some(list) => refs_of(parsed, nodes(view, view.list(list).map_err(text)?.nodes())?),
    }
}

pub fn build(column: &str, input: &Value) -> Option<Result<Column, String>> {
    Some(match column {
        "ast.Node.Attributes" => {
            node_map("source", input, attributes_kind, |parsed, view, node| {
                ref_of(parsed, view.node(node).map_err(text)?.attributes())
            })
        }
        "ast.Node.Children" => node_map("source", input, children_kind, |parsed, view, node| {
            list_refs(parsed, view, view.node(node).map_err(text)?.children_list())
        }),
        "ast.Node.ClassName" => node_map(
            "source_jsdoc",
            input,
            class_name_kind,
            |parsed, view, node| ref_of(parsed, view.node(node).map_err(text)?.class_name()),
        ),
        "ast.Node.CommentList" => {
            node_map("source_jsdoc", input, comment_kind, |parsed, view, node| {
                list_refs(parsed, view, view.node(node).map_err(text)?.comment_list())
            })
        }
        "ast.Node.Comments" => {
            node_map("source_jsdoc", input, comment_kind, |parsed, view, node| {
                let comments = view
                    .node(node)
                    .map_err(text)?
                    .comments(view)
                    .map_err(text)?;
                refs_of(parsed, nodes(view, comments)?)
            })
        }
        "ast.Node.TypeExpression" => node_map(
            "source_jsdoc",
            input,
            type_expression_kind,
            |parsed, view, node| ref_of(parsed, view.node(node).map_err(text)?.type_expression()),
        ),
        "ast.Node.Statement" => node_map("source", input, statement_kind, |parsed, view, node| {
            ref_of(parsed, view.node(node).map_err(text)?.statement())
        }),
        "ast.IsStatement" => node_predicate("source", input, all, |_, view, node| {
            tsr_ast::utilities::is_statement(view, node)
        }),
        "ast.NodeList.HasTrailingComma" => {
            node_map("source", input, trailing_comma_kind, |_, view, node| {
                let list = match view.node(node).map_err(text)?.data() {
                    NodeDataRead::CallExpression(data) => data.arguments(),
                    NodeDataRead::NewExpression(data) => data.arguments(),
                    NodeDataRead::ArrayLiteralExpression(data) => data.elements(),
                    NodeDataRead::ObjectLiteralExpression(data) => data.properties(),
                    _ => None,
                };
                match list {
                    None => Ok(Value::Null),
                    Some(list) => Ok(json!(view.list_has_trailing_comma(list).map_err(text)?)),
                }
            })
        }
        "ast.FlowSwitchClauseData.IsEmpty" => is_empty(input),
        _ => return None,
    })
}

/// Go's `ast.FlowSwitchClauseData.IsEmpty` column.
fn is_empty(input: &Value) -> Result<Column, String> {
    let clauses: Clauses = decode(input)?;
    Ok(Box::new(move || {
        Ok(Value::Array(
            clauses
                .clauses
                .iter()
                .map(|[start, end]| {
                    json!(tsr_ast::FlowSwitchClauseData::new(None, *start, *end).is_empty())
                })
                .collect(),
        ))
    }))
}

fn attributes_kind(view: AstView<'_>, node: NodeId) -> Result<bool, Error> {
    is_kind(
        view,
        node,
        &[
            K::JsxOpeningElement,
            K::JsxSelfClosingElement,
            K::ModuleDeclaration,
        ],
    )
}

fn children_kind(view: AstView<'_>, node: NodeId) -> Result<bool, Error> {
    is_kind(view, node, &[K::JsxElement, K::JsxFragment])
}

fn class_name_kind(view: AstView<'_>, node: NodeId) -> Result<bool, Error> {
    is_kind(view, node, &[K::JSDocAugmentsTag, K::JSDocImplementsTag])
}

fn comment_kind(view: AstView<'_>, node: NodeId) -> Result<bool, Error> {
    is_kind(view, node, JSDOC_COMMENT_KINDS)
}

fn type_expression_kind(view: AstView<'_>, node: NodeId) -> Result<bool, Error> {
    is_kind(
        view,
        node,
        &[
            K::JSDocParameterTag,
            K::JSDocPropertyTag,
            K::JSDocReturnTag,
            K::JSDocTypeTag,
            K::JSDocTypedefTag,
            K::JSDocCallbackTag,
            K::JSDocSatisfiesTag,
            K::JSDocThrowsTag,
        ],
    )
}

fn statement_kind(view: AstView<'_>, node: NodeId) -> Result<bool, Error> {
    is_kind(
        view,
        node,
        &[
            K::DoStatement,
            K::WhileStatement,
            K::ForStatement,
            K::ForInStatement,
            K::ForOfStatement,
            K::WithStatement,
            K::LabeledStatement,
        ],
    )
}

fn trailing_comma_kind(view: AstView<'_>, node: NodeId) -> Result<bool, Error> {
    is_kind(
        view,
        node,
        &[
            K::CallExpression,
            K::NewExpression,
            K::ArrayLiteralExpression,
            K::ObjectLiteralExpression,
        ],
    )
}
