//! Group `targets`: callee targets, JSX tags, JSDoc deprecation, member helpers.
//! Go: `tools/phase1/tables/go/targets_columns.go`; spec:
//! `data/phase1/tables/targets.json`.
use super::helpers::{all, int, is_kind, node_map, node_predicate, ref_of};
use super::{text, Column};
use crate::protocol::hex;
use serde_json::{json, Value};
use tsr_arena::Error;
use tsr_ast::utilities_targets as targets;
use tsr_ast::{AstView, NodeId, SyntaxKind as K};

pub const COLUMNS: &[&str] = &[
    "ast.EntityNameToString",
    "ast.GetAssignedName",
    "ast.GetHostSignatureFromJSDoc",
    "ast.GetJSDocDeprecatedTag",
    "ast.GetNamespaceDeclarationNode",
    "ast.GetPropertyNameForPropertyNameNode",
    "ast.GetTypeAnnotationNode",
    "ast.HasInitializer",
    "ast.HasQuestionToken",
    "ast.IsCallExpressionTarget",
    "ast.IsCallOrNewExpressionTarget",
    "ast.IsDecoratorTarget",
    "ast.IsNewExpressionTarget",
    "ast.IsTaggedTemplateTag",
    "ast.IsJsxOpeningLikeElementTagName",
    "ast.IsDeprecatedDeclaration",
    "ast.IsDeprecatedDeclarationWithCachedFlags",
    "ast.IsIterationStatement",
    "ast.IsJsxTagName",
    "ast.IsLet",
    "ast.IsPrototypeAccess",
];

type Target = fn(AstView<'_>, NodeId, bool, bool) -> Result<bool, Error>;

pub fn build(column: &str, input: &Value) -> Option<Result<Column, String>> {
    Some(match column {
        "ast.EntityNameToString" => {
            node_map("source", input, entity_name_domain, |_, view, node| {
                let bracketed = |name: NodeId| -> Result<Vec<u8>, Error> {
                    let mut out = b"<".to_vec();
                    out.extend_from_slice(view.node_text(name)?.as_bytes());
                    out.push(b'>');
                    Ok(out)
                };
                let plain = targets::entity_name_to_string(view, node, None).map_err(text)?;
                let with =
                    targets::entity_name_to_string(view, node, Some(&bracketed)).map_err(text)?;
                Ok(json!([hex(&plain), hex(&with)]))
            })
        }
        "ast.GetAssignedName" => node_map("source", input, all, |parsed, view, node| {
            ref_of(
                parsed,
                tsr_ast::get_assigned_name(view, node).map_err(text)?,
            )
        }),
        "ast.GetHostSignatureFromJSDoc" => {
            node_map("source_jsdoc", input, all, |parsed, view, node| {
                ref_of(
                    parsed,
                    targets::get_host_signature_from_js_doc(view, node).map_err(text)?,
                )
            })
        }
        "ast.GetJSDocDeprecatedTag" => {
            node_map("source_jsdoc", input, all, |parsed, view, node| {
                let mut jsdoc = tsr_parser::ParserJsDocProvider::default();
                let tag = targets::get_js_doc_deprecated_tag(view, &mut jsdoc, parsed.root(), node)
                    .map_err(text)?;
                ref_of(parsed, tag)
            })
        }
        "ast.GetNamespaceDeclarationNode" => node_map(
            "source",
            input,
            namespace_declaration_kinds,
            |parsed, view, node| {
                ref_of(
                    parsed,
                    tsr_ast::utilities_middle::get_namespace_declaration_node(view, node)
                        .map_err(text)?,
                )
            },
        ),
        "ast.GetPropertyNameForPropertyNameNode" => {
            node_map("source", input, property_name_kinds, |_, view, node| {
                Ok(json!(hex(
                    &targets::get_property_name_for_property_name_node(view, node).map_err(text)?
                )))
            })
        }
        "ast.GetTypeAnnotationNode" => {
            node_map("source_jsdoc", input, all, |parsed, view, node| {
                ref_of(
                    parsed,
                    targets::get_type_annotation_node(view, node).map_err(text)?,
                )
            })
        }
        "ast.HasInitializer" => node_predicate("source", input, all, |_, view, node| {
            Ok(tsr_ast::utilities_middle::has_initializer(
                &view.node(node)?,
            ))
        }),
        "ast.HasQuestionToken" => node_predicate("source", input, all, |_, view, node| {
            tsr_ast::utilities_middle::has_question_token(view, &view.node(node)?)
        }),
        "ast.IsCallExpressionTarget" => {
            node_map("source", input, not_source_file, |_, view, node| {
                callee(view, node, targets::is_call_expression_target)
            })
        }
        "ast.IsCallOrNewExpressionTarget" => {
            node_map("source", input, not_source_file, |_, view, node| {
                callee(view, node, targets::is_call_or_new_expression_target)
            })
        }
        "ast.IsDecoratorTarget" => node_map("source", input, not_source_file, |_, view, node| {
            callee(view, node, targets::is_decorator_target)
        }),
        "ast.IsNewExpressionTarget" => {
            node_map("source", input, not_source_file, |_, view, node| {
                callee(view, node, targets::is_new_expression_target)
            })
        }
        "ast.IsTaggedTemplateTag" => node_map("source", input, not_source_file, |_, view, node| {
            callee(view, node, targets::is_tagged_template_tag)
        }),
        "ast.IsJsxOpeningLikeElementTagName" => {
            node_map("source", input, not_source_file, |_, view, node| {
                callee(view, node, targets::is_jsx_opening_like_element_tag_name)
            })
        }
        "ast.IsDeprecatedDeclaration" => {
            node_predicate("source_jsdoc", input, all, |parsed, view, node| {
                let mut jsdoc = tsr_parser::ParserJsDocProvider::default();
                targets::is_deprecated_declaration(view, &mut jsdoc, parsed.root(), node)
            })
        }
        "ast.IsDeprecatedDeclarationWithCachedFlags" => {
            node_predicate("source_jsdoc", input, all, |parsed, view, node| {
                let mut jsdoc = tsr_parser::ParserJsDocProvider::default();
                let flags = view.node(node)?.flags();
                targets::is_deprecated_declaration_with_cached_flags(
                    view,
                    &mut jsdoc,
                    parsed.root(),
                    node,
                    flags,
                )
            })
        }
        "ast.IsIterationStatement" => node_map("source", input, all, |_, view, node| {
            let mut bits = 0;
            if targets::is_iteration_statement(view, node, false).map_err(text)? {
                bits |= 1;
            }
            if targets::is_iteration_statement(view, node, true).map_err(text)? {
                bits |= 2;
            }
            Ok(int(bits))
        }),
        "ast.IsJsxTagName" => node_predicate("source", input, not_source_file, |_, view, node| {
            targets::is_jsx_tag_name(view, node)
        }),
        "ast.IsLet" => node_predicate("source", input, all, |_, view, node| {
            targets::is_let(view, node)
        }),
        "ast.IsPrototypeAccess" => node_predicate("source", input, all, |_, view, node| {
            targets::is_prototype_access(view, node)
        }),
        _ => return None,
    })
}

/// Go's `callee`: bit `2*skip + include` is set where the predicate is true.
fn callee(view: AstView<'_>, node: NodeId, target: Target) -> Result<Value, String> {
    let mut bits = 0;
    for (bit, (include, skip)) in [(false, false), (true, false), (false, true), (true, true)]
        .into_iter()
        .enumerate()
    {
        if target(view, node, include, skip).map_err(text)? {
            bits |= 1 << bit;
        }
    }
    Ok(int(bits))
}

/// Go's `notSourceFile`.
fn not_source_file(view: AstView<'_>, node: NodeId) -> Result<bool, Error> {
    Ok(view.node(node)?.kind() != K::SourceFile)
}

/// Go's `entityNameDomain`.
fn entity_name_domain(view: AstView<'_>, node: NodeId) -> Result<bool, Error> {
    let read = view.node(node)?;
    let both = |left: Option<NodeId>, right: Option<NodeId>| -> Result<bool, Error> {
        Ok(entity_name_domain(view, left.ok_or(Error::InvalidGraph)?)?
            && entity_name_domain(view, right.ok_or(Error::InvalidGraph)?)?)
    };
    match read.kind().known() {
        Some(K::ThisKeyword | K::Identifier | K::PrivateIdentifier) => Ok(true),
        Some(K::QualifiedName) => {
            let qualified = read
                .data_source()
                .as_qualified_name()
                .ok_or(Error::InvalidGraph)?;
            both(qualified.left(), qualified.right())
        }
        Some(K::PropertyAccessExpression) => both(read.expression(), read.name()),
        Some(K::JsxNamespacedName) => {
            let namespaced = read
                .data_source()
                .as_jsx_namespaced_name()
                .ok_or(Error::InvalidGraph)?;
            both(namespaced.namespace(), read.name())
        }
        _ => Ok(false),
    }
}

fn namespace_declaration_kinds(view: AstView<'_>, node: NodeId) -> Result<bool, Error> {
    is_kind(
        view,
        node,
        &[
            K::ImportDeclaration,
            K::JSImportDeclaration,
            K::ImportEqualsDeclaration,
            K::ExportDeclaration,
        ],
    )
}

fn property_name_kinds(view: AstView<'_>, node: NodeId) -> Result<bool, Error> {
    is_kind(
        view,
        node,
        &[
            K::Identifier,
            K::PrivateIdentifier,
            K::StringLiteral,
            K::NoSubstitutionTemplateLiteral,
            K::NumericLiteral,
            K::BigIntLiteral,
            K::JsxNamespacedName,
            K::ComputedPropertyName,
        ],
    )
}
