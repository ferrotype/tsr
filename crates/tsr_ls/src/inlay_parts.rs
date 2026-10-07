//! The native hint-label grammar. These parts are deliberately not printer
//! tokens: the protocol preserves their boundaries and optional name targets.
use crate::{documentation::list, Result};
use std::collections::HashMap;
use tsr_ast::{utilities as ast, AstView, NodeId, NodeListId, NodeRead, SymbolId, SyntaxKind as K};
use tsr_jsstring::escape::{escape_string, QuoteChar};

pub(crate) struct LabelPart {
    pub value: String,
    pub symbol: Option<SymbolId>,
}
enum Task {
    Node(NodeId),
    Text(&'static str),
}
fn push(out: &mut Vec<Task>, node: Option<NodeId>) {
    if let Some(node) = node {
        out.push(Task::Node(node));
    }
}
fn parts(
    out: &mut Vec<Task>,
    view: AstView<'_>,
    ids: Option<NodeListId>,
    separator: &'static str,
) -> Result<()> {
    for (i, node) in list(view, ids)?.into_iter().enumerate() {
        if i > 0 && !separator.is_empty() {
            out.push(Task::Text(separator));
        }
        push(out, Some(node));
    }
    Ok(())
}
fn type_arguments(
    out: &mut Vec<Task>,
    view: AstView<'_>,
    n: &NodeRead<'_>,
    separator: &'static str,
) -> Result<()> {
    if !list(view, n.type_argument_list())?.is_empty() {
        out.push(Task::Text("<"));
        parts(out, view, n.type_argument_list(), separator)?;
        out.push(Task::Text(">"));
    }
    Ok(())
}
fn signature(out: &mut Vec<Task>, view: AstView<'_>, n: &NodeRead<'_>) -> Result<()> {
    if !list(view, n.type_parameter_list())?.is_empty() {
        out.push(Task::Text("<"));
        parts(out, view, n.type_parameter_list(), ", ")?;
        out.push(Task::Text(">"));
    }
    out.push(Task::Text("("));
    parts(out, view, n.parameter_list(), ", ")?;
    out.push(Task::Text(")"));
    Ok(())
}
fn annotation(out: &mut Vec<Task>, n: &NodeRead<'_>, separator: &'static str) {
    if n.type_node().is_some() {
        out.push(Task::Text(separator));
        push(out, n.type_node());
    }
}
// port: tsc/internal/ls/inlay_hints.go:inlayHintState.getLiteralText
fn literal(view: AstView<'_>, node: NodeId, single: bool) -> Result<String> {
    let n = view.node(node)?;
    let text = view.node_text(node)?;
    let bytes = match n.kind().known() {
        Some(K::StringLiteral) => {
            let quote = if single {
                QuoteChar::Single
            } else {
                QuoteChar::Double
            };
            let mut b = vec![quote as u8];
            b.extend(escape_string(text.as_bytes(), quote));
            b.push(quote as u8);
            b
        }
        Some(K::TemplateHead | K::TemplateMiddle | K::TemplateTail) => {
            let raw = n.raw_text();
            let raw = if raw.is_empty() {
                escape_string(text.as_bytes(), QuoteChar::Backtick)
            } else {
                raw.to_vec()
            };
            let mut b = vec![if n.kind() == K::TemplateHead {
                b'`'
            } else {
                b'}'
            }];
            b.extend(raw);
            b.extend_from_slice(if n.kind() == K::TemplateTail {
                b"`"
            } else {
                b"${"
            });
            b
        }
        _ => text.as_bytes().to_vec(),
    };
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}
// port: tsc/internal/ls/inlay_hints.go:inlayHintState.getInlayHintLabelParts
pub(crate) fn render(
    view: AstView<'_>,
    root: NodeId,
    symbols: &HashMap<NodeId, SymbolId>,
    single: bool,
) -> Result<Vec<LabelPart>> {
    let mut result = Vec::new();
    let mut stack = vec![Task::Node(root)];
    while let Some(task) = stack.pop() {
        let Task::Node(node) = task else {
            if let Task::Text(value) = task {
                result.push(LabelPart {
                    value: value.into(),
                    symbol: None,
                });
            }
            continue;
        };
        let n = view.node(node)?;
        let kind = n.kind().known().ok_or(tsr_arena::Error::InvalidGraph)?;
        let token = tsr_scanner::token_to_string(kind);
        if !token.is_empty() {
            result.push(LabelPart {
                value: token.into(),
                symbol: None,
            });
            continue;
        }
        if ast::is_literal_expression(&n)
            || matches!(kind, K::TemplateHead | K::TemplateMiddle | K::TemplateTail)
        {
            result.push(LabelPart {
                value: literal(view, node, single)?,
                symbol: None,
            });
            continue;
        }
        let mut next = Vec::new();
        match kind {
            K::Identifier => result.push(LabelPart {
                value: String::from_utf8_lossy(view.node_text(node)?.as_bytes()).into_owned(),
                symbol: symbols.get(&node).copied(),
            }),
            K::QualifiedName => {
                let d = n.data_source().as_qualified_name().unwrap();
                push(&mut next, d.left());
                next.push(Task::Text("."));
                push(&mut next, d.right());
            }
            K::TypePredicate => {
                let d = n.data_source().as_type_predicate_node().unwrap();
                if d.asserts_modifier().is_some() {
                    next.push(Task::Text("asserts "));
                }
                push(&mut next, d.parameter_name());
                annotation(&mut next, &n, " is ");
            }
            K::TypeReference => {
                push(
                    &mut next,
                    n.data_source()
                        .as_type_reference_node()
                        .unwrap()
                        .type_name(),
                );
                type_arguments(&mut next, view, &n, ",")?;
            }
            K::TypeParameter => {
                parts(&mut next, view, n.modifiers(), "")?;
                push(&mut next, n.name());
                let d = n.data_source().as_type_parameter_declaration().unwrap();
                if d.constraint().is_some() {
                    next.push(Task::Text(" extends "));
                    push(&mut next, d.constraint());
                }
                if d.default_type().is_some() {
                    next.push(Task::Text(" = "));
                    push(&mut next, d.default_type());
                }
            }
            K::Parameter | K::NamedTupleMember => {
                if kind == K::Parameter {
                    parts(&mut next, view, n.modifiers(), " ")?;
                }
                let rest = if kind == K::Parameter {
                    n.data_source()
                        .as_parameter_declaration()
                        .unwrap()
                        .dot_dot_dot_token()
                } else {
                    n.data_source()
                        .as_named_tuple_member()
                        .unwrap()
                        .dot_dot_dot_token()
                };
                if rest.is_some() {
                    next.push(Task::Text("..."));
                }
                push(&mut next, n.name());
                if n.question_token(view)?.is_some() {
                    next.push(Task::Text("?"));
                }
                if kind == K::NamedTupleMember {
                    next.push(Task::Text(": "));
                    push(&mut next, n.type_node());
                } else {
                    annotation(&mut next, &n, ": ");
                }
            }
            K::ConstructorType | K::FunctionType => {
                if kind == K::ConstructorType {
                    next.push(Task::Text("new "));
                }
                signature(&mut next, view, &n)?;
                next.push(Task::Text(" => "));
                push(&mut next, n.type_node());
            }
            K::TypeQuery => {
                next.push(Task::Text("typeof "));
                push(
                    &mut next,
                    n.data_source().as_type_query_node().unwrap().expr_name(),
                );
                type_arguments(&mut next, view, &n, ", ")?;
            }
            K::TypeLiteral | K::ObjectBindingPattern => {
                let ids = if kind == K::TypeLiteral {
                    n.member_list()
                } else {
                    n.element_list()
                };
                next.push(Task::Text("{"));
                if !list(view, ids)?.is_empty() {
                    next.push(Task::Text(" "));
                    parts(
                        &mut next,
                        view,
                        ids,
                        if kind == K::TypeLiteral { "; " } else { ", " },
                    )?;
                    next.push(Task::Text(" "));
                }
                next.push(Task::Text("}"));
            }
            K::ArrayType => {
                push(
                    &mut next,
                    n.data_source().as_array_type_node().unwrap().element_type(),
                );
                next.push(Task::Text("[]"));
            }
            K::TupleType | K::ArrayBindingPattern => {
                next.push(Task::Text("["));
                parts(&mut next, view, n.element_list(), ", ")?;
                next.push(Task::Text("]"));
            }
            K::OptionalType => {
                push(&mut next, n.type_node());
                next.push(Task::Text("?"));
            }
            K::RestType => {
                next.push(Task::Text("..."));
                push(&mut next, n.type_node());
            }
            K::UnionType => parts(
                &mut next,
                view,
                n.data_source().as_union_type_node().unwrap().types(),
                " | ",
            )?,
            K::IntersectionType => parts(
                &mut next,
                view,
                n.data_source().as_intersection_type_node().unwrap().types(),
                " & ",
            )?,
            K::ConditionalType => {
                let d = n.data_source().as_conditional_type_node().unwrap();
                push(&mut next, d.check_type());
                next.push(Task::Text(" extends "));
                push(&mut next, d.extends_type());
                next.push(Task::Text(" ? "));
                push(&mut next, d.true_type());
                next.push(Task::Text(" : "));
                push(&mut next, d.false_type());
            }
            K::InferType => {
                next.push(Task::Text("infer "));
                push(
                    &mut next,
                    n.data_source()
                        .as_infer_type_node()
                        .unwrap()
                        .type_parameter(),
                );
            }
            K::ParenthesizedType => {
                next.push(Task::Text("("));
                push(&mut next, n.type_node());
                next.push(Task::Text(")"));
            }
            K::TypeOperator => {
                next.push(Task::Text(tsr_scanner::token_to_string(
                    n.data_source()
                        .as_type_operator_node()
                        .unwrap()
                        .operator()
                        .known()
                        .ok_or(tsr_arena::Error::InvalidGraph)?,
                )));
                push(&mut next, n.type_node());
            }
            K::IndexedAccessType => {
                let d = n.data_source().as_indexed_access_type_node().unwrap();
                push(&mut next, d.object_type());
                next.push(Task::Text("["));
                push(&mut next, d.index_type());
                next.push(Task::Text("]"));
            }
            K::MappedType => {
                let d = n.data_source().as_mapped_type_node().unwrap();
                next.push(Task::Text("{ "));
                if let Some(token) = d.readonly_token() {
                    match view.node(token)?.kind().known() {
                        Some(K::PlusToken) => next.push(Task::Text("+")),
                        Some(K::MinusToken) => next.push(Task::Text("-")),
                        _ => {}
                    }
                    next.push(Task::Text("readonly "));
                }
                next.push(Task::Text("["));
                push(&mut next, d.type_parameter());
                if d.name_type().is_some() {
                    next.push(Task::Text(" as "));
                    push(&mut next, d.name_type());
                }
                next.push(Task::Text("]"));
                if let Some(token) = n.question_token(view)? {
                    match view.node(token)?.kind().known() {
                        Some(K::PlusToken) => next.push(Task::Text("+")),
                        Some(K::MinusToken) => next.push(Task::Text("-")),
                        _ => {}
                    }
                    next.push(Task::Text("?"));
                }
                next.push(Task::Text(": "));
                push(&mut next, n.type_node());
                next.push(Task::Text("; }"));
            }
            K::LiteralType => push(
                &mut next,
                n.data_source().as_literal_type_node().unwrap().literal(),
            ),
            K::ImportType => {
                let d = n.data_source().as_import_type_node().unwrap();
                if d.is_type_of() {
                    next.push(Task::Text("typeof "));
                }
                next.push(Task::Text("import("));
                push(&mut next, d.argument());
                next.push(Task::Text(")"));
                if d.qualifier().is_some() {
                    next.push(Task::Text("."));
                    push(&mut next, d.qualifier());
                }
                type_arguments(&mut next, view, &n, ", ")?;
            }
            K::PropertySignature | K::MethodSignature => {
                if !list(view, n.modifiers())?.is_empty() {
                    parts(&mut next, view, n.modifiers(), " ")?;
                    next.push(Task::Text(" "));
                }
                push(&mut next, n.name());
                push(&mut next, n.postfix_token());
                if kind == K::MethodSignature {
                    signature(&mut next, view, &n)?;
                }
                annotation(&mut next, &n, ": ");
            }
            K::IndexSignature => {
                next.push(Task::Text("["));
                parts(&mut next, view, n.parameter_list(), ", ")?;
                next.push(Task::Text("]"));
                annotation(&mut next, &n, ": ");
            }
            K::CallSignature | K::ConstructSignature => {
                if kind == K::ConstructSignature {
                    next.push(Task::Text("new "));
                }
                signature(&mut next, view, &n)?;
                annotation(&mut next, &n, ": ");
            }
            K::BindingElement => push(&mut next, n.name()),
            K::PrefixUnaryExpression => {
                let d = n.data_source().as_prefix_unary_expression().unwrap();
                next.push(Task::Text(tsr_scanner::token_to_string(
                    d.operator().known().ok_or(tsr_arena::Error::InvalidGraph)?,
                )));
                push(&mut next, d.operand());
            }
            K::TemplateLiteralType => {
                let d = n.data_source().as_template_literal_type_node().unwrap();
                push(&mut next, d.head());
                parts(&mut next, view, d.template_spans(), "")?;
            }
            K::TemplateLiteralTypeSpan => {
                push(&mut next, n.type_node());
                push(
                    &mut next,
                    n.data_source()
                        .as_template_literal_type_span()
                        .unwrap()
                        .literal(),
                );
            }
            K::ThisType => next.push(Task::Text("this")),
            K::ComputedPropertyName => {
                next.push(Task::Text("["));
                push(&mut next, n.expression());
                next.push(Task::Text("]"));
            }
            K::PropertyAccessExpression => {
                push(&mut next, n.expression());
                next.push(Task::Text("."));
                push(&mut next, n.name());
            }
            K::ElementAccessExpression => {
                push(&mut next, n.expression());
                next.push(Task::Text("["));
                push(
                    &mut next,
                    n.data_source()
                        .as_element_access_expression()
                        .unwrap()
                        .argument_expression(),
                );
                next.push(Task::Text("]"));
            }
            _ => {
                return Err(tsr_astnav::Error::Assertion(format!(
                    "Bad syntax kind in inlay label: {kind:?}"
                ))
                .into())
            }
        }
        stack.extend(next.into_iter().rev());
    }
    Ok(result)
}
