//! `transformers/inliners/constenum.go`: replaces a property or element access
//! that the checker resolves to a constant enum member with the constant.
use crate::transformer::{Error, TransformOptions, Transformer};
use std::rc::Rc;
use tsr_ast::{token_flags, FactoryMethods, JsString, NodeId, NodeVisitor, SyntaxKind as K};
use tsr_printer::emit_resolver::ConstantValue;

const NIL: &str = "runtime error: invalid memory address or nil pointer dereference";

/// `ConstEnumInliningTransformer`: the state the visit reads.
struct ConstEnumInliningTransformer<'a> {
    opts: TransformOptions<'a>,
}

/// Under isolated modules the pin fails the construction (`debug.Fail`); the
/// port records the failure, so the chain refuses the file instead.
// port: tsc/internal/transformers/inliners/constenum.go:NewConstEnumInliningTransformer
pub fn new_const_enum_inlining_transformer<'a>(
    opt: &TransformOptions<'a>,
) -> Option<Transformer<'a>> {
    let compiler_options = &opt.compiler_options;
    let emit_context = opt.context.clone();
    if compiler_options.isolated_modules() {
        opt.failure.record(Error::Unsupported(
            "Debug failure. const enums are not inlined under isolated modules",
        ));
        return Some(Transformer::new(
            |_: &mut NodeVisitor<'_>, node: Option<NodeId>| node,
            Some(emit_context),
            opt.failure.clone(),
        ));
    }
    let tx = Rc::new(ConstEnumInliningTransformer { opts: opt.clone() });
    Some(Transformer::new(
        move |visitor: &mut NodeVisitor<'_>, node: Option<NodeId>| tx.visit(visitor, node),
        Some(emit_context),
        opt.failure.clone(),
    ))
}

impl ConstEnumInliningTransformer<'_> {
    // port: tsc/internal/transformers/inliners/constenum.go:ConstEnumInliningTransformer.visit
    fn visit(&self, visitor: &mut NodeVisitor<'_>, node: Option<NodeId>) -> Option<NodeId> {
        let failure = &self.opts.failure;
        if failure.is_set() {
            return node;
        }
        let id = node.expect(NIL);
        match visitor.factory().node(id).kind().known() {
            Some(K::PropertyAccessExpression | K::ElementAccessExpression) => {
                let parse = self.opts.context.parse_node(visitor.factory(), id);
                let Some(parse) = parse else {
                    return visitor.visit_each_child(node);
                };
                let value = self
                    .opts
                    .emit_resolver
                    .borrow_mut()
                    .get_constant_value(parse);
                let Some(value) = failure.ok(value) else {
                    return node;
                };
                if let Some(value) = value {
                    let replacement = new_constant(visitor, value);
                    if self
                        .opts
                        .compiler_options
                        .remove_comments
                        .is_false_or_unknown()
                    {
                        let original = self.opts.context.most_original(id);
                        if !tsr_ast::utilities::node_is_synthesized(
                            &visitor.factory().node(original),
                        ) {
                            let view = visitor.factory().ast_view().expect(NIL);
                            let original_text = tsr_scanner::get_text_of_node(view, original);
                            let Some(original_text) = failure.ok(original_text) else {
                                return node;
                            };
                            let escaped_text = safe_multi_line_comment(original_text.as_bytes());
                            self.opts.context.clone().add_synthetic_trailing_comment(
                                replacement,
                                K::MultiLineCommentTrivia,
                                escaped_text,
                                false,
                            );
                        }
                    }
                    return Some(replacement);
                }
                visitor.visit_each_child(node)
            }
            _ => visitor.visit_each_child(node),
        }
    }
}

/// The replacement switch of `visit`. The resolver's constant has no
/// `PseudoBigInt` case, so upstream's bigint branch has no counterpart.
fn new_constant(visitor: &mut NodeVisitor<'_>, value: ConstantValue) -> NodeId {
    let factory = visitor.factory_mut();
    match value {
        ConstantValue::Number(v) => {
            if v.is_inf() {
                if v.abs() == v {
                    factory.new_identifier(JsString::from_bytes(&b"Infinity"[..]))
                } else {
                    let infinity = factory.new_identifier(JsString::from_bytes(&b"Infinity"[..]));
                    factory.new_prefix_unary_expression(K::MinusToken.into(), Some(infinity))
                }
            } else if v.is_nan() {
                factory.new_identifier(JsString::from_bytes(&b"NaN"[..]))
            } else if v.abs() == v {
                factory.new_numeric_literal(
                    JsString::from_bytes(v.to_string().into_bytes()),
                    token_flags::NONE,
                )
            } else {
                let literal = factory.new_numeric_literal(
                    JsString::from_bytes(v.abs().to_string().into_bytes()),
                    token_flags::NONE,
                );
                factory.new_prefix_unary_expression(K::MinusToken.into(), Some(literal))
            }
        }
        ConstantValue::String(v) => factory.new_string_literal(v, token_flags::NONE),
    }
}

// port: tsc/internal/transformers/inliners/constenum.go:safeMultiLineComment
fn safe_multi_line_comment(mut text: &[u8]) -> JsString {
    let mut b = Vec::with_capacity(text.len() + 2);
    b.push(b' ');
    while let Some(i) = text.windows(2).position(|pair| pair == b"*/") {
        b.extend_from_slice(&text[..i]);
        b.extend_from_slice(b"*_/");
        text = &text[i + 2..];
    }
    b.extend_from_slice(text);
    b.push(b' ');
    JsString::from_bytes(b)
}
