//! `transformers/estransforms/optionalchain.go`: `a?.b`, `a?.[b]`, `a?.()`
//! and `delete a?.b` to conditionals over a not-null test.
use super::utilities::{
    create_not_null_condition, is_simple_copiable_expression, skip_parentheses, subtree_facts, view,
};
use crate::transformer::{Error, Failure, TransformOptions, Transformer};
use std::cell::RefCell;
use std::rc::Rc;
use tsr_ast::{
    node_flags, subtree_flags, utilities::outer_expression_kinds, Factory, FactoryMethods, NodeId,
    NodeVisitor, RuntimeFactory, SyntaxKind as K,
};
use tsr_printer::{emit_flags, EmitContext};

struct OptionalChainTransformer {
    emit_context: RefCell<EmitContext>,
    failure: Failure,
}

/// `ast.SkipPartiallyEmittedExpressions`.
fn skip_partially_emitted_expressions(
    factory: &dyn RuntimeFactory,
    node: NodeId,
) -> Result<NodeId, Error> {
    Ok(tsr_ast::skip_partially_emitted_expressions(
        view(factory)?,
        node,
    )?)
}

fn printer_error(error: tsr_printer::Error) -> Error {
    match error {
        tsr_printer::Error::Arena(error) => Error::Arena(error),
        _ => Error::Unsupported("printer.NodeFactory.RestoreOuterExpressions"),
    }
}

fn nil_checked(node: Option<NodeId>) -> NodeId {
    node.expect("runtime error: invalid memory address or nil pointer dereference")
}

fn is_optional_chain(factory: &dyn Factory, node: NodeId) -> bool {
    factory.node(node).flags() & node_flags::OPTIONAL_CHAIN != 0
}

fn is_synthetic_reference_expression(factory: &dyn Factory, node: NodeId) -> bool {
    tsr_ast::is_synthetic_reference_expression(&factory.node(node))
}

/// A `SyntheticReferenceExpression`'s `Expression` and `ThisArg`.
fn synthetic_reference_parts(factory: &dyn Factory, node: NodeId) -> (NodeId, Option<NodeId>) {
    let read = factory.node(node);
    let data = read
        .as_synthetic_reference_expression()
        .expect("SyntheticReferenceExpression payload");
    (nil_checked(data.expression()), data.this_arg())
}

/// A visited list's nodes (`.Nodes` of a non-nil list).
fn list_nodes(factory: &dyn RuntimeFactory, list: Option<tsr_ast::NodeListId>) -> Vec<NodeId> {
    let list = list.expect("runtime error: invalid memory address or nil pointer dereference");
    let nodes = factory.read_list(list).nodes();
    factory.read_nodes(nodes).iter().map(nil_checked).collect()
}

impl OptionalChainTransformer {
    // port: tsc/internal/transformers/estransforms/optionalchain.go:optionalChainTransformer.visit
    fn visit(&self, visitor: &mut NodeVisitor<'_>, node: Option<NodeId>) -> Option<NodeId> {
        if self.failure.is_set() {
            return node;
        }
        let id = nil_checked(node);
        let Some(facts) = self.failure.ok(subtree_facts(visitor.factory(), id)) else {
            return node;
        };
        if facts & subtree_flags::OPTIONAL_CHAINING == 0 {
            return node;
        }
        match visitor.factory().node(id).kind().known() {
            Some(K::CallExpression) => Some(self.visit_call_expression(visitor, id, false)),
            Some(K::PropertyAccessExpression | K::ElementAccessExpression) => {
                if is_optional_chain(visitor.factory(), id) {
                    return Some(self.visit_optional_expression(visitor, id, false, false));
                }
                visitor.visit_each_child(node)
            }
            Some(K::DeleteExpression) => Some(self.visit_delete_expression(visitor, id)),
            _ => visitor.visit_each_child(node),
        }
    }

    // port: tsc/internal/transformers/estransforms/optionalchain.go:optionalChainTransformer.visitCallExpression
    fn visit_call_expression(
        &self,
        visitor: &mut NodeVisitor<'_>,
        node: NodeId,
        capture_this_arg: bool,
    ) -> NodeId {
        if is_optional_chain(visitor.factory(), node) {
            // If `node` is an optional chain, then it is the outermost chain of an optional expression.
            return self.visit_optional_expression(visitor, node, capture_this_arg, false);
        }
        let (node_expression, node_arguments, node_flags) = {
            let read = visitor.factory().node(node);
            let data = read.as_call_expression().expect("CallExpression payload");
            (
                nil_checked(data.expression()),
                data.arguments(),
                read.flags(),
            )
        };
        if tsr_ast::is_parenthesized_expression(&visitor.factory().node(node_expression)) {
            let Some(unwrapped) = self
                .failure
                .ok(skip_parentheses(visitor.factory(), node_expression))
            else {
                return node;
            };
            if is_optional_chain(visitor.factory(), unwrapped) {
                // capture thisArg for calls of parenthesized optional chains like `(foo?.bar)()`
                let expression =
                    self.visit_parenthesized_expression(visitor, node_expression, true, false);
                let args = visitor.visit_nodes(node_arguments);
                if is_synthetic_reference_expression(visitor.factory(), expression) {
                    let (synthetic_expression, this_arg) =
                        synthetic_reference_parts(visitor.factory(), expression);
                    let arguments = list_nodes(visitor.factory(), args);
                    let factory = visitor.factory_mut();
                    let mut emit_context = self.emit_context.borrow_mut();
                    let res = emit_context.new_function_call_call(
                        factory,
                        synthetic_expression,
                        this_arg,
                        arguments,
                    );
                    let loc = factory.node(node).range();
                    factory.set_node_range(res, loc);
                    emit_context.set_original(res, node);
                    return res;
                }
                return visitor.factory_mut().update_call_expression(
                    node,
                    Some(expression),
                    None, /*questionDotToken*/
                    None, /*typeArguments*/
                    args,
                    node_flags,
                );
            }
        }
        nil_checked(visitor.visit_each_child(Some(node)))
    }

    // port: tsc/internal/transformers/estransforms/optionalchain.go:optionalChainTransformer.visitParenthesizedExpression
    fn visit_parenthesized_expression(
        &self,
        visitor: &mut NodeVisitor<'_>,
        node: NodeId,
        capture_this_arg: bool,
        is_delete: bool,
    ) -> NodeId {
        let node_expression = nil_checked(visitor.factory().node(node).expression());
        let expr = self.visit_non_optional_expression(
            visitor,
            node_expression,
            capture_this_arg,
            is_delete,
        );
        if is_synthetic_reference_expression(visitor.factory(), expr) {
            // `(a.b)` -> { expression `((_a = a).b)`, thisArg: `_a` }
            // `(a[b])` -> { expression `((_a = a)[b])`, thisArg: `_a` }
            let (synthetic_expression, this_arg) =
                synthetic_reference_parts(visitor.factory(), expr);
            let factory = visitor.factory_mut();
            let updated = factory.update_parenthesized_expression(node, Some(synthetic_expression));
            let res = factory.new_synthetic_reference_expression(Some(updated), this_arg);
            self.emit_context.borrow_mut().set_original(res, node);
            return res;
        }
        visitor
            .factory_mut()
            .update_parenthesized_expression(node, Some(expr))
    }

    // port: tsc/internal/transformers/estransforms/optionalchain.go:optionalChainTransformer.visitPropertyOrElementAccessExpression
    fn visit_property_or_element_access_expression(
        &self,
        visitor: &mut NodeVisitor<'_>,
        node: NodeId,
        capture_this_arg: bool,
        is_delete: bool,
    ) -> NodeId {
        if is_optional_chain(visitor.factory(), node) {
            // If `node` is an optional chain, then it is the outermost chain of an optional expression.
            return self.visit_optional_expression(visitor, node, capture_this_arg, is_delete);
        }
        let node_expression = visitor.factory().node(node).expression();
        let mut expression = visitor.visit_node(node_expression);
        tsr_core::debug::assert(
            expression.is_none_or(|expression| {
                !is_synthetic_reference_expression(visitor.factory(), expression)
            }),
            &[],
        );

        let mut this_arg = None;
        if capture_this_arg {
            let visited = nil_checked(expression);
            let factory = visitor.factory_mut();
            if is_simple_copiable_expression(factory, visited) {
                this_arg = expression;
            } else {
                let mut emit_context = self.emit_context.borrow_mut();
                let temp = emit_context.new_temp_variable(factory);
                emit_context.add_variable_declaration(factory, temp);
                this_arg = Some(temp);
                expression = Some(emit_context.new_assignment_expression(factory, temp, visited));
            }
        }

        let (kind, flags) = {
            let read = visitor.factory().node(node);
            (read.kind(), read.flags())
        };
        let expression = if kind == K::PropertyAccessExpression {
            let name = visitor.factory().node(node).name();
            let name = visitor.visit_node(name);
            visitor.factory_mut().update_property_access_expression(
                node, expression, None, /*questionDotToken*/
                name, flags,
            )
        } else {
            let argument_expression = visitor
                .factory()
                .node(node)
                .as_element_access_expression()
                .expect("ElementAccessExpression payload")
                .argument_expression();
            let argument_expression = visitor.visit_node(argument_expression);
            visitor.factory_mut().update_element_access_expression(
                node,
                expression,
                None,
                argument_expression,
                flags,
            )
        };

        if this_arg.is_some() {
            let res = visitor
                .factory_mut()
                .new_synthetic_reference_expression(Some(expression), this_arg);
            self.emit_context.borrow_mut().set_original(res, node);
            return res;
        }
        expression
    }

    // port: tsc/internal/transformers/estransforms/optionalchain.go:optionalChainTransformer.visitDeleteExpression
    fn visit_delete_expression(&self, visitor: &mut NodeVisitor<'_>, node: NodeId) -> NodeId {
        let node_expression = nil_checked(visitor.factory().node(node).expression());
        let Some(unwrapped) = self
            .failure
            .ok(skip_parentheses(visitor.factory(), node_expression))
        else {
            return node;
        };
        if is_optional_chain(visitor.factory(), unwrapped) {
            return self.visit_non_optional_expression(visitor, node_expression, false, true);
        }
        nil_checked(visitor.visit_each_child(Some(node)))
    }

    // port: tsc/internal/transformers/estransforms/optionalchain.go:optionalChainTransformer.visitNonOptionalExpression
    fn visit_non_optional_expression(
        &self,
        visitor: &mut NodeVisitor<'_>,
        node: NodeId,
        capture_this_arg: bool,
        is_delete: bool,
    ) -> NodeId {
        // The chain's left side is visited here directly, once per link.
        stacker::maybe_grow(128 * 1024, 2 * 1024 * 1024, || {
            self.visit_non_optional_expression_worker(visitor, node, capture_this_arg, is_delete)
        })
    }

    fn visit_non_optional_expression_worker(
        &self,
        visitor: &mut NodeVisitor<'_>,
        node: NodeId,
        capture_this_arg: bool,
        is_delete: bool,
    ) -> NodeId {
        match visitor.factory().node(node).kind().known() {
            Some(K::ParenthesizedExpression) => {
                self.visit_parenthesized_expression(visitor, node, capture_this_arg, is_delete)
            }
            Some(K::ElementAccessExpression | K::PropertyAccessExpression) => self
                .visit_property_or_element_access_expression(
                    visitor,
                    node,
                    capture_this_arg,
                    is_delete,
                ),
            Some(K::CallExpression) => self.visit_call_expression(visitor, node, capture_this_arg),
            _ => nil_checked(visitor.visit_node(Some(node))),
        }
    }

    // port: tsc/internal/transformers/estransforms/optionalchain.go:optionalChainTransformer.visitOptionalExpression
    fn visit_optional_expression(
        &self,
        visitor: &mut NodeVisitor<'_>,
        node: NodeId,
        capture_this_arg: bool,
        is_delete: bool,
    ) -> NodeId {
        let Some(r) = self.failure.ok(flatten_chain(visitor.factory(), node)) else {
            return node;
        };
        let expression = nil_checked(r.expression);
        let chain = r.chain;
        let Some(skipped) = self.failure.ok(skip_partially_emitted_expressions(
            visitor.factory(),
            expression,
        )) else {
            return node;
        };
        let left = self.visit_non_optional_expression(
            visitor,
            skipped,
            is_call_chain(visitor.factory(), chain[0]),
            false,
        );
        let mut left_this_arg = None;
        let mut captured_left = left;
        if is_synthetic_reference_expression(visitor.factory(), left) {
            let (synthetic_expression, this_arg) =
                synthetic_reference_parts(visitor.factory(), left);
            left_this_arg = this_arg;
            captured_left = synthetic_expression;
        }
        let restored = {
            let emit_context = self.emit_context.borrow();
            match visitor.factory_mut().ast_builder_mut() {
                Some(builder) => emit_context
                    .restore_outer_expressions(
                        builder,
                        Some(expression),
                        captured_left,
                        outer_expression_kinds::PARTIALLY_EMITTED_EXPRESSIONS,
                    )
                    .map_err(printer_error),
                None => Err(Error::Unsupported(
                    "a transform over a factory without AST storage",
                )),
            }
        };
        let Some(mut left_expression) = self.failure.ok(restored) else {
            return node;
        };
        {
            let factory = visitor.factory_mut();
            if !is_simple_copiable_expression(factory, captured_left) {
                let mut emit_context = self.emit_context.borrow_mut();
                captured_left = emit_context.new_temp_variable(factory);
                emit_context.add_variable_declaration(factory, captured_left);
                left_expression =
                    emit_context.new_assignment_expression(factory, captured_left, left_expression);
            }
        }
        let mut right_expression = captured_left;
        let mut this_arg = None;

        for (i, &segment) in chain.iter().enumerate() {
            match visitor.factory().node(segment).kind().known() {
                Some(K::ElementAccessExpression | K::PropertyAccessExpression) => {
                    if i == chain.len() - 1 && capture_this_arg {
                        let factory = visitor.factory_mut();
                        if is_simple_copiable_expression(factory, right_expression) {
                            this_arg = Some(right_expression);
                        } else {
                            let mut emit_context = self.emit_context.borrow_mut();
                            let temp = emit_context.new_temp_variable(factory);
                            emit_context.add_variable_declaration(factory, temp);
                            this_arg = Some(temp);
                            right_expression = emit_context.new_assignment_expression(
                                factory,
                                temp,
                                right_expression,
                            );
                        }
                    }
                    if visitor.factory().node(segment).kind() == K::ElementAccessExpression {
                        let argument_expression = visitor
                            .factory()
                            .node(segment)
                            .as_element_access_expression()
                            .expect("ElementAccessExpression payload")
                            .argument_expression();
                        let argument_expression = visitor.visit_node(argument_expression);
                        right_expression = visitor.factory_mut().new_element_access_expression(
                            Some(right_expression),
                            None,
                            argument_expression,
                            node_flags::NONE,
                        );
                    } else {
                        let name = visitor.factory().node(segment).name();
                        let name = visitor.visit_node(name);
                        right_expression = visitor.factory_mut().new_property_access_expression(
                            Some(right_expression),
                            None,
                            name,
                            node_flags::NONE,
                        );
                    }
                }
                Some(K::CallExpression) => {
                    if let (0, Some(mut this)) = (i, left_this_arg) {
                        {
                            let mut emit_context = self.emit_context.borrow_mut();
                            if !emit_context.has_auto_generate_info(this) {
                                this = tsr_ast::clone_node(visitor.factory_mut(), this);
                                emit_context.add_emit_flags(this, emit_flags::NO_COMMENTS);
                            }
                        }
                        left_this_arg = Some(this);
                        let mut call_this_arg = this;
                        if visitor.factory().node(this).kind() == K::SuperKeyword {
                            call_this_arg = self
                                .emit_context
                                .borrow()
                                .new_this_expression(visitor.factory_mut());
                        }
                        let arguments = visitor.factory().node(segment).argument_list();
                        let arguments = visitor.visit_nodes(arguments);
                        let arguments = list_nodes(visitor.factory(), arguments);
                        right_expression = self.emit_context.borrow().new_function_call_call(
                            visitor.factory_mut(),
                            right_expression,
                            Some(call_this_arg),
                            arguments,
                        );
                    } else {
                        let arguments = visitor.factory().node(segment).argument_list();
                        let arguments = visitor.visit_nodes(arguments);
                        right_expression = visitor.factory_mut().new_call_expression(
                            Some(right_expression),
                            None,
                            None,
                            arguments,
                            node_flags::NONE,
                        );
                    }
                }
                _ => {}
            }
            self.emit_context
                .borrow_mut()
                .set_original(right_expression, segment);
        }

        let factory = visitor.factory_mut();
        let mut emit_context = self.emit_context.borrow_mut();
        let condition =
            create_not_null_condition(&emit_context, factory, left_expression, captured_left, true);
        let question = factory.new_token(K::QuestionToken.into());
        let mut target = if is_delete {
            let when_true = emit_context.new_true_expression(factory);
            let colon = factory.new_token(K::ColonToken.into());
            let when_false = factory.new_delete_expression(Some(right_expression));
            factory.new_conditional_expression(
                Some(condition),
                Some(question),
                Some(when_true),
                Some(colon),
                Some(when_false),
            )
        } else {
            let when_true = emit_context.new_void_zero_expression(factory);
            let colon = factory.new_token(K::ColonToken.into());
            factory.new_conditional_expression(
                Some(condition),
                Some(question),
                Some(when_true),
                Some(colon),
                Some(right_expression),
            )
        };
        let loc = factory.node(node).range();
        factory.set_node_range(target, loc);
        if this_arg.is_some() {
            target = factory.new_synthetic_reference_expression(Some(target), this_arg);
        }
        emit_context.set_original(target, node);
        target
    }
}

/// `flattenResult`.
struct FlattenResult {
    expression: Option<NodeId>,
    chain: Vec<NodeId>,
}

// port: tsc/internal/transformers/estransforms/optionalchain.go:isNonNullChain
fn is_non_null_chain(factory: &dyn Factory, node: NodeId) -> bool {
    let read = factory.node(node);
    tsr_ast::is_non_null_expression(&read) && read.flags() & node_flags::OPTIONAL_CHAIN != 0
}

// port: tsc/internal/transformers/estransforms/optionalchain.go:flattenChain
fn flatten_chain(factory: &dyn RuntimeFactory, mut chain: NodeId) -> Result<FlattenResult, Error> {
    tsr_core::debug::assert(!is_non_null_chain(factory, chain), &[]);
    let mut links = vec![chain];
    while !tsr_ast::is_tagged_template_expression(&factory.node(chain))
        && factory.node(chain).question_dot_token().is_none()
    {
        chain = skip_partially_emitted_expressions(
            factory,
            nil_checked(factory.node(chain).expression()),
        )?;
        tsr_core::debug::assert(!is_non_null_chain(factory, chain), &[]);
        links.insert(0, chain);
    }
    Ok(FlattenResult {
        expression: factory.node(chain).expression(),
        chain: links,
    })
}

// port: tsc/internal/transformers/estransforms/optionalchain.go:isCallChain
fn is_call_chain(factory: &dyn Factory, node: NodeId) -> bool {
    let read = factory.node(node);
    tsr_ast::is_call_expression(&read) && read.flags() & node_flags::OPTIONAL_CHAIN != 0
}

// port: tsc/internal/transformers/estransforms/optionalchain.go:newOptionalChainTransformer
pub fn new_optional_chain_transformer<'a>(opts: &TransformOptions<'a>) -> Option<Transformer<'a>> {
    let tx = Rc::new(OptionalChainTransformer {
        emit_context: RefCell::new(opts.context.clone()),
        failure: opts.failure.clone(),
    });
    Some(Transformer::new(
        move |visitor: &mut NodeVisitor<'_>, node: Option<NodeId>| tx.visit(visitor, node),
        Some(opts.context.clone()),
        opts.failure.clone(),
    ))
}
