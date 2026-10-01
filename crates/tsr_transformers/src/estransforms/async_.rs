//! `transformers/estransforms/async.go`: lowers ES2017 async functions,
//! methods, accessors and arrows to `__awaiter` calls over generator bodies,
//! capturing `this`, `arguments` and `super` property accesses.
//!
//! Upstream keeps four node visitors: the transformer's own (`tx.Visitor()`),
//! the async-body visitor, the fallback visitor and the super-access visitor.
//! Here a visitor borrows the factory, so an auxiliary visitor is created over
//! the current visitor's factory where upstream uses it. The methods that take
//! `m` receive the transformer's own visitor; the callbacks of the auxiliary
//! visitors create one where upstream calls back into `tx.visit`.
use super::utilities::{identifier_text, new_node_list, SuperAccessState};
use crate::transformer::{Failure, TransformOptions, Transformer};
use std::cell::{Cell, RefCell};
use std::collections::HashSet;
use std::rc::Rc;
use tsr_ast::subtree_flags::{ANY_AWAIT, AWAIT};
use tsr_ast::utilities_containers::function_flags;
use tsr_ast::{
    node_flags, AstView, Factory, FactoryMethods, JsString, NodeId, NodeKind, NodeListId,
    NodeVisitor, RuntimeFactory, SyntaxKind as K,
};
use tsr_core::TextRange;
use tsr_printer::generated_identifier_flags as g;
use tsr_printer::{emit_flags, emit_helpers, AutoGenerateOptions, EmitContext, EmitVisitorHooks};

const NIL: &str = "runtime error: invalid memory address or nil pointer dereference";

/// Go's `asyncContextFlags`.
mod async_context {
    pub(super) const NON_TOP_LEVEL: u32 = 1 << 0;
    pub(super) const HAS_LEXICAL_THIS: u32 = 1 << 1;
}

/// Go's `lexicalArgumentsInfo`.
#[derive(Clone, Copy, Default)]
struct LexicalArgumentsInfo {
    binding: Option<NodeId>,
    used: bool,
}

/// Which of upstream's node visitors a visitor created here is.
#[derive(Clone, Copy)]
enum Visitor {
    /// `tx.Visitor()`, whose callback is `visit`.
    Transformer,
    /// `asyncBodyVisitor`, whose callback is `visitAsyncBodyNode`.
    AsyncBody,
    /// `fallbackNodeVisitor`, whose callback is `visitFallback`.
    Fallback,
}

/// The parameter names an async body's `var` declarations may collide with:
/// upstream's set pointer, shared until a catch clause copies it.
type ParameterNames = Option<Rc<HashSet<JsString>>>;

struct AsyncTransformer {
    context: EmitContext,
    hooks: EmitVisitorHooks,
    failure: Failure,
    super_access: SuperAccessState,

    context_flags: Cell<u32>,

    enclosing_function_parameter_names: RefCell<ParameterNames>,
    lexical_arguments: Cell<LexicalArgumentsInfo>,
}

// port: tsc/internal/transformers/estransforms/async.go:newAsyncTransformer
pub fn new_async_transformer<'a>(opts: &TransformOptions<'a>) -> Option<Transformer<'a>> {
    let tx = AsyncTransformer {
        context: opts.context.clone(),
        hooks: opts.context.visitor_hooks(),
        failure: opts.failure.clone(),
        super_access: SuperAccessState::init_super_access_visitor(&opts.context, &opts.failure),
        context_flags: Cell::new(0),
        enclosing_function_parameter_names: RefCell::new(None),
        lexical_arguments: Cell::new(LexicalArgumentsInfo::default()),
    };
    Some(Transformer::new(
        move |visitor: &mut NodeVisitor<'_>, node: Option<NodeId>| {
            let node = node?;
            if tx.failure.is_set() {
                return Some(node);
            }
            tx.visit(visitor, node)
        },
        Some(opts.context.clone()),
        opts.failure.clone(),
    ))
}

impl AsyncTransformer {
    /// Runs `f` with one of upstream's visitors over `visitor`'s factory.
    fn with_visitor<R>(
        &self,
        visitor: &mut NodeVisitor<'_>,
        which: Visitor,
        f: impl FnOnce(&mut NodeVisitor<'_>) -> R,
    ) -> R {
        let visit = |visitor: &mut NodeVisitor<'_>, node: Option<NodeId>| -> Option<NodeId> {
            let node = node?;
            if self.failure.is_set() {
                return Some(node);
            }
            match which {
                Visitor::Transformer => self.visit(visitor, node),
                Visitor::AsyncBody => self.visit_async_body_node(visitor, node),
                Visitor::Fallback => self.visit_fallback(visitor, node),
            }
        };
        let mut created = self
            .hooks
            .new_node_visitor(Some(&visit), visitor.factory_mut());
        f(&mut created)
    }

    /// An AST query over the factory's parsed view. A failed query is
    /// recorded, which fails the file, and answers `T::default()`.
    fn query<T: Default>(
        &self,
        factory: &dyn RuntimeFactory,
        query: impl FnOnce(AstView<'_>) -> Result<T, tsr_arena::Error>,
    ) -> T {
        let Some(view) = factory.ast_view() else {
            self.failure.record(tsr_arena::Error::InvalidGraph);
            return T::default();
        };
        self.failure.ok(query(view)).unwrap_or_default()
    }

    /// `ast.GetFunctionFlags`.
    fn function_flags(&self, factory: &dyn RuntimeFactory, node: NodeId) -> u32 {
        self.query(factory, |view| {
            tsr_ast::utilities_containers::get_function_flags(view, Some(node))
        })
    }

    // port: tsc/internal/transformers/estransforms/async.go:asyncTransformer.visitSourceFile
    fn visit_source_file(&self, m: &mut NodeVisitor<'_>, node: NodeId) -> NodeId {
        let is_declaration_file = match m.factory().read_source_file(node) {
            Ok(file) => file.is_declaration_file,
            Err(error) => {
                self.failure.record(error);
                return node;
            }
        };
        if is_declaration_file {
            return node;
        }

        self.set_context_flag(async_context::NON_TOP_LEVEL, false);
        self.set_context_flag(async_context::HAS_LEXICAL_THIS, false);
        let visited = m.visit_each_child(Some(node)).expect(NIL);
        let mut context = self.context.clone();
        let helpers = context.read_emit_helpers();
        context.add_emit_helper(visited, &helpers);
        visited
    }

    // port: tsc/internal/transformers/estransforms/async.go:asyncTransformer.setContextFlag
    fn set_context_flag(&self, flag: u32, val: bool) {
        if val {
            self.context_flags.set(self.context_flags.get() | flag);
        } else {
            self.context_flags.set(self.context_flags.get() & !flag);
        }
    }

    // port: tsc/internal/transformers/estransforms/async.go:asyncTransformer.inContext
    fn in_context(&self, flags: u32) -> bool {
        self.context_flags.get() & flags != 0
    }

    // port: tsc/internal/transformers/estransforms/async.go:asyncTransformer.inTopLevelContext
    fn in_top_level_context(&self) -> bool {
        !self.in_context(async_context::NON_TOP_LEVEL)
    }

    // port: tsc/internal/transformers/estransforms/async.go:asyncTransformer.inHasLexicalThisContext
    fn in_has_lexical_this_context(&self) -> bool {
        self.in_context(async_context::HAS_LEXICAL_THIS)
    }

    // port: tsc/internal/transformers/estransforms/async.go:asyncTransformer.doWithContext
    fn do_with_context(
        &self,
        flags: u32,
        cb: fn(&Self, &mut NodeVisitor<'_>, NodeId) -> NodeId,
        m: &mut NodeVisitor<'_>,
        node: NodeId,
    ) -> NodeId {
        let flags_to_set = flags & !self.context_flags.get();
        if flags_to_set != 0 {
            self.set_context_flag(flags_to_set, true);
            let result = cb(self, m, node);
            self.set_context_flag(flags_to_set, false);
            return result;
        }
        cb(self, m, node)
    }

    // port: tsc/internal/transformers/estransforms/async.go:asyncTransformer.visitDefault
    #[allow(clippy::unused_self)] // upstream's method, a `doWithContext` callback
    fn visit_default(&self, m: &mut NodeVisitor<'_>, node: NodeId) -> NodeId {
        m.visit_each_child(Some(node)).expect(NIL)
    }

    /// `visitor` is any visitor; the fallback visitor is created over its
    /// factory.
    // port: tsc/internal/transformers/estransforms/async.go:asyncTransformer.fallbackVisitor
    fn fallback_visitor(&self, visitor: &mut NodeVisitor<'_>, node: NodeId) -> Option<NodeId> {
        if self
            .super_access
            .captured_super_properties
            .borrow()
            .is_none()
            && self.lexical_arguments.get().binding.is_none()
        {
            return Some(node);
        }
        self.super_access
            .track_super_access(visitor.factory(), node);
        match visitor.node(node).kind().known() {
            Some(
                K::FunctionExpression
                | K::FunctionDeclaration
                | K::MethodDeclaration
                | K::GetAccessor
                | K::SetAccessor
                | K::Constructor,
            ) => return Some(node),
            // Parameters, binding elements and variable declarations fall
            // through to visitEachChild, as every other kind does.
            Some(K::Identifier) => {
                let lexical_arguments = self.lexical_arguments.get();
                if let Some(binding) = lexical_arguments.binding {
                    if identifier_text(visitor.factory(), node).as_bytes() == b"arguments"
                        && !self.query(visitor.factory(), |view| {
                            tsr_ast::is_identifier_name(view, node)
                        })
                        && !self.query(visitor.factory(), |view| {
                            tsr_ast::utilities_middle::is_label_name(view, node)
                        })
                    {
                        self.lexical_arguments.set(LexicalArgumentsInfo {
                            used: true,
                            ..lexical_arguments
                        });
                        return Some(binding);
                    }
                }
            }
            _ => {}
        }
        self.with_visitor(visitor, Visitor::Fallback, |fallback| {
            fallback.visit_each_child(Some(node))
        })
    }

    // port: tsc/internal/transformers/estransforms/async.go:asyncTransformer.visitFallback
    fn visit_fallback(&self, visitor: &mut NodeVisitor<'_>, node: NodeId) -> Option<NodeId> {
        self.fallback_visitor(visitor, node)
    }

    // port: tsc/internal/transformers/estransforms/async.go:asyncTransformer.visit
    fn visit(&self, m: &mut NodeVisitor<'_>, node: NodeId) -> Option<NodeId> {
        let clear_lexical_this = self.context.emit_flags(node) & emit_flags::NO_LEXICAL_THIS != 0
            && self.in_has_lexical_this_context();
        if clear_lexical_this {
            self.set_context_flag(async_context::HAS_LEXICAL_THIS, false);
        }
        // Upstream restores the flag in a deferred call.
        let result = (|| {
            use async_context::{HAS_LEXICAL_THIS, NON_TOP_LEVEL};
            let Some(view) = m.factory().ast_view() else {
                self.failure.record(tsr_arena::Error::InvalidGraph);
                return Some(node);
            };
            if view.subtree_facts(node) & (ANY_AWAIT | AWAIT) == 0 {
                return self.fallback_visitor(m, node);
            }
            self.super_access.track_super_access(m.factory(), node);
            match m.node(node).kind().known() {
                // ES2017 async modifier should be elided for targets < ES2017
                Some(K::AsyncKeyword) => None,
                Some(K::SourceFile) => Some(self.visit_source_file(m, node)),
                Some(K::AwaitExpression) => self.visit_await_expression(m, node),
                Some(K::MethodDeclaration) => Some(self.do_with_context(
                    NON_TOP_LEVEL | HAS_LEXICAL_THIS,
                    Self::visit_method_declaration,
                    m,
                    node,
                )),
                Some(K::FunctionDeclaration) => Some(self.do_with_context(
                    NON_TOP_LEVEL | HAS_LEXICAL_THIS,
                    Self::visit_function_declaration,
                    m,
                    node,
                )),
                Some(K::FunctionExpression) => Some(self.do_with_context(
                    NON_TOP_LEVEL | HAS_LEXICAL_THIS,
                    Self::visit_function_expression,
                    m,
                    node,
                )),
                Some(K::ArrowFunction) => {
                    Some(self.do_with_context(NON_TOP_LEVEL, Self::visit_arrow_function, m, node))
                }
                Some(K::GetAccessor) => Some(self.do_with_context(
                    NON_TOP_LEVEL | HAS_LEXICAL_THIS,
                    Self::visit_get_accessor_declaration,
                    m,
                    node,
                )),
                Some(K::SetAccessor) => Some(self.do_with_context(
                    NON_TOP_LEVEL | HAS_LEXICAL_THIS,
                    Self::visit_set_accessor_declaration,
                    m,
                    node,
                )),
                Some(K::Constructor) => Some(self.do_with_context(
                    NON_TOP_LEVEL | HAS_LEXICAL_THIS,
                    Self::visit_constructor_declaration,
                    m,
                    node,
                )),
                Some(K::ClassDeclaration | K::ClassExpression) => Some(self.do_with_context(
                    NON_TOP_LEVEL | HAS_LEXICAL_THIS,
                    Self::visit_default,
                    m,
                    node,
                )),
                _ => m.visit_each_child(Some(node)),
            }
        })();
        if clear_lexical_this {
            self.set_context_flag(async_context::HAS_LEXICAL_THIS, true);
        }
        result
    }

    /// `a` is the async-body visitor.
    // port: tsc/internal/transformers/estransforms/async.go:asyncTransformer.visitAsyncBodyNode
    fn visit_async_body_node(&self, a: &mut NodeVisitor<'_>, node: NodeId) -> Option<NodeId> {
        let kind = a.node(node).kind();
        if is_node_with_possible_hoisted_declaration(kind) {
            match kind.known() {
                Some(K::VariableStatement) => {
                    return self.with_visitor(a, Visitor::Transformer, |m| {
                        self.visit_variable_statement_in_async_body(m, node)
                    });
                }
                Some(K::ForStatement) => {
                    return Some(self.with_visitor(a, Visitor::Transformer, |m| {
                        self.visit_for_statement_in_async_body(m, node)
                    }));
                }
                Some(K::ForInStatement) => {
                    return Some(self.with_visitor(a, Visitor::Transformer, |m| {
                        self.visit_for_in_statement_in_async_body(m, node)
                    }));
                }
                Some(K::ForOfStatement) => {
                    return Some(self.with_visitor(a, Visitor::Transformer, |m| {
                        self.visit_for_of_statement_in_async_body(m, node)
                    }));
                }
                Some(K::CatchClause) => return self.visit_catch_clause_in_async_body(a, node),
                Some(
                    K::Block
                    | K::SwitchStatement
                    | K::CaseBlock
                    | K::CaseClause
                    | K::DefaultClause
                    | K::TryStatement
                    | K::DoStatement
                    | K::WhileStatement
                    | K::IfStatement
                    | K::WithStatement
                    | K::LabeledStatement,
                ) => return a.visit_each_child(Some(node)),
                _ => {}
            }
        }
        self.with_visitor(a, Visitor::Transformer, |m| self.visit(m, node))
    }

    /// `a` is the async-body visitor.
    // port: tsc/internal/transformers/estransforms/async.go:asyncTransformer.visitCatchClauseInAsyncBody
    fn visit_catch_clause_in_async_body(
        &self,
        a: &mut NodeVisitor<'_>,
        node: NodeId,
    ) -> Option<NodeId> {
        let mut catch_clause_names = HashSet::new();
        let variable_declaration = a
            .node(node)
            .as_catch_clause()
            .expect("CatchClause payload")
            .variable_declaration();
        if let Some(variable_declaration) = variable_declaration {
            self.record_declaration_name(
                a.factory(),
                variable_declaration,
                &mut catch_clause_names,
            );
        }

        // names declared in a catch variable are block scoped
        let enclosing = self.enclosing_function_parameter_names.borrow().clone();
        let mut catch_clause_unshadowed_names: Option<HashSet<JsString>> = None;
        for escaped_name in &catch_clause_names {
            if let Some(enclosing) = enclosing
                .as_ref()
                .filter(|names| names.contains(escaped_name))
            {
                catch_clause_unshadowed_names
                    .get_or_insert_with(|| (**enclosing).clone())
                    .remove(escaped_name);
            }
        }

        if let Some(names) = catch_clause_unshadowed_names {
            let saved_enclosing_function_parameter_names = self
                .enclosing_function_parameter_names
                .replace(Some(Rc::new(names)));
            let result = a.visit_each_child(Some(node));
            *self.enclosing_function_parameter_names.borrow_mut() =
                saved_enclosing_function_parameter_names;
            return result;
        }
        a.visit_each_child(Some(node))
    }

    // port: tsc/internal/transformers/estransforms/async.go:asyncTransformer.visitVariableStatementInAsyncBody
    fn visit_variable_statement_in_async_body(
        &self,
        m: &mut NodeVisitor<'_>,
        node: NodeId,
    ) -> Option<NodeId> {
        let decl_list = m
            .node(node)
            .as_variable_statement()
            .expect("VariableStatement payload")
            .declaration_list();
        if self.is_variable_declaration_list_with_colliding_name(m.factory(), decl_list) {
            let expression = self.visit_variable_declaration_list_with_colliding_names(
                m,
                decl_list.expect(NIL),
                false,
            );
            if let Some(expression) = expression {
                return Some(m.new_expression_statement(Some(expression)));
            }
            return None;
        }
        m.visit_each_child(Some(node))
    }

    // port: tsc/internal/transformers/estransforms/async.go:asyncTransformer.visitForInStatementInAsyncBody
    fn visit_for_in_statement_in_async_body(
        &self,
        m: &mut NodeVisitor<'_>,
        node: NodeId,
    ) -> NodeId {
        let (initializer, expression, statement) = for_in_or_of_parts(m, node);
        let visited_initializer =
            if self.is_variable_declaration_list_with_colliding_name(m.factory(), initializer) {
                self.visit_variable_declaration_list_with_colliding_names(
                    m,
                    initializer.expect(NIL),
                    true,
                )
            } else {
                m.visit_node(initializer)
            };

        let expression = m.visit_node(expression);
        let statement = self.with_visitor(m, Visitor::AsyncBody, |a| {
            a.visit_embedded_statement(statement)
        });
        m.update_for_in_or_of_statement(
            node,
            None, /*awaitModifier*/
            visited_initializer,
            expression,
            statement,
        )
    }

    // port: tsc/internal/transformers/estransforms/async.go:asyncTransformer.visitForOfStatementInAsyncBody
    fn visit_for_of_statement_in_async_body(
        &self,
        m: &mut NodeVisitor<'_>,
        node: NodeId,
    ) -> NodeId {
        let (initializer, expression, statement) = for_in_or_of_parts(m, node);
        let await_modifier = m
            .node(node)
            .as_for_in_or_of_statement()
            .expect("ForInOrOfStatement payload")
            .await_modifier();
        let visited_initializer =
            if self.is_variable_declaration_list_with_colliding_name(m.factory(), initializer) {
                self.visit_variable_declaration_list_with_colliding_names(
                    m,
                    initializer.expect(NIL),
                    true,
                )
            } else {
                m.visit_node(initializer)
            };

        let await_modifier = m.visit_node(await_modifier);
        let expression = m.visit_node(expression);
        let statement = self.with_visitor(m, Visitor::AsyncBody, |a| {
            a.visit_embedded_statement(statement)
        });
        m.update_for_in_or_of_statement(
            node,
            await_modifier,
            visited_initializer,
            expression,
            statement,
        )
    }

    // port: tsc/internal/transformers/estransforms/async.go:asyncTransformer.visitForStatementInAsyncBody
    fn visit_for_statement_in_async_body(&self, m: &mut NodeVisitor<'_>, node: NodeId) -> NodeId {
        let (initializer, condition, incrementor, statement) = {
            let read = m.node(node);
            let data = read.as_for_statement().expect("ForStatement payload");
            (
                data.initializer(),
                data.condition(),
                data.incrementor(),
                data.statement(),
            )
        };
        let visited_initializer = match initializer {
            Some(list)
                if self
                    .is_variable_declaration_list_with_colliding_name(m.factory(), Some(list)) =>
            {
                self.visit_variable_declaration_list_with_colliding_names(m, list, false)
            }
            _ => m.visit_node(initializer),
        };

        let condition = m.visit_node(condition);
        let incrementor = m.visit_node(incrementor);
        let statement = self.with_visitor(m, Visitor::AsyncBody, |a| {
            a.visit_embedded_statement(statement)
        });
        m.update_for_statement(node, visited_initializer, condition, incrementor, statement)
    }

    /// Visits an AwaitExpression node.
    ///
    /// This function will be called any time a ES2017 await expression is encountered.
    // port: tsc/internal/transformers/estransforms/async.go:asyncTransformer.visitAwaitExpression
    fn visit_await_expression(&self, m: &mut NodeVisitor<'_>, node: NodeId) -> Option<NodeId> {
        // do not downlevel a top-level await as it is module syntax...
        if self.in_top_level_context() {
            return m.visit_each_child(Some(node));
        }
        let expression = m.node(node).expression();
        let expression = m.visit_node(expression);
        let yield_expr = m.new_yield_expression(None /*asteriskToken*/, expression);
        let loc = m.node(node).range();
        m.set_node_range(yield_expr, loc);
        self.context.clone().set_original(yield_expr, node);
        Some(yield_expr)
    }

    // port: tsc/internal/transformers/estransforms/async.go:asyncTransformer.visitConstructorDeclaration
    fn visit_constructor_declaration(&self, m: &mut NodeVisitor<'_>, node: NodeId) -> NodeId {
        let (modifiers, parameters) = {
            let read = m.node(node);
            (read.modifiers(), read.parameter_list())
        };
        let saved_lexical_arguments = self.lexical_arguments.take();
        let modifiers = m.visit_modifiers(modifiers);
        let parameters = self.context.clone().visit_parameters(parameters, m);
        let body = Some(self.transform_method_body(m, node));
        let updated = m.update_constructor_declaration(
            node, modifiers, None, /*typeParameters*/
            parameters, None, /*returnType*/
            None, /*fullSignature*/
            body,
        );
        self.lexical_arguments.set(saved_lexical_arguments);
        updated
    }

    /// Visits a MethodDeclaration node.
    ///
    /// This function will be called when one of the following conditions are met:
    /// - The node is marked as async
    // port: tsc/internal/transformers/estransforms/async.go:asyncTransformer.visitMethodDeclaration
    fn visit_method_declaration(&self, m: &mut NodeVisitor<'_>, node: NodeId) -> NodeId {
        let function_flags = self.function_flags(m.factory(), node);
        let saved_lexical_arguments = self.lexical_arguments.take();

        let (parameters, body) = if function_flags & function_flags::ASYNC != 0 {
            let parameters = self.transform_async_function_parameter_list(m, node);
            let body = Some(self.transform_async_function_body(m, node, parameters));
            (parameters, body)
        } else {
            let parameter_list = m.node(node).parameter_list();
            let parameters = self.context.clone().visit_parameters(parameter_list, m);
            let body = Some(self.transform_method_body(m, node));
            (parameters, body)
        };

        let (modifiers, asterisk_token, name) = {
            let read = m.node(node);
            let data = read
                .as_method_declaration()
                .expect("MethodDeclaration payload");
            (data.modifiers(), data.asterisk_token(), data.name())
        };
        let modifiers = m.visit_modifiers(modifiers);
        let updated = m.update_method_declaration(
            node,
            modifiers,
            asterisk_token,
            name,
            None, /*postfixToken*/
            None, /*typeParameters*/
            parameters,
            None, /*returnType*/
            None, /*fullSignature*/
            body,
        );
        self.lexical_arguments.set(saved_lexical_arguments);
        updated
    }

    // port: tsc/internal/transformers/estransforms/async.go:asyncTransformer.visitGetAccessorDeclaration
    fn visit_get_accessor_declaration(&self, m: &mut NodeVisitor<'_>, node: NodeId) -> NodeId {
        let (modifiers, name, parameters) = {
            let read = m.node(node);
            (read.modifiers(), read.name(), read.parameter_list())
        };
        let saved_lexical_arguments = self.lexical_arguments.take();
        let modifiers = m.visit_modifiers(modifiers);
        let parameters = self.context.clone().visit_parameters(parameters, m);
        let body = Some(self.transform_method_body(m, node));
        let updated = m.update_get_accessor_declaration(
            node, modifiers, name, None, /*typeParameters*/
            parameters, None, /*returnType*/
            None, /*fullSignature*/
            body,
        );
        self.lexical_arguments.set(saved_lexical_arguments);
        updated
    }

    // port: tsc/internal/transformers/estransforms/async.go:asyncTransformer.visitSetAccessorDeclaration
    fn visit_set_accessor_declaration(&self, m: &mut NodeVisitor<'_>, node: NodeId) -> NodeId {
        let (modifiers, name, parameters) = {
            let read = m.node(node);
            (read.modifiers(), read.name(), read.parameter_list())
        };
        let saved_lexical_arguments = self.lexical_arguments.take();
        let modifiers = m.visit_modifiers(modifiers);
        let parameters = self.context.clone().visit_parameters(parameters, m);
        let body = Some(self.transform_method_body(m, node));
        let updated = m.update_set_accessor_declaration(
            node, modifiers, name, None, /*typeParameters*/
            parameters, None, /*returnType*/
            None, /*fullSignature*/
            body,
        );
        self.lexical_arguments.set(saved_lexical_arguments);
        updated
    }

    /// Visits a FunctionDeclaration node.
    ///
    /// This function will be called when one of the following conditions are met:
    /// - The node is marked async
    // port: tsc/internal/transformers/estransforms/async.go:asyncTransformer.visitFunctionDeclaration
    fn visit_function_declaration(&self, m: &mut NodeVisitor<'_>, node: NodeId) -> NodeId {
        let function_flags = self.function_flags(m.factory(), node);
        let saved_lexical_arguments = self.lexical_arguments.take();

        let (parameters, body) = if function_flags & function_flags::ASYNC != 0 {
            let parameters = self.transform_async_function_parameter_list(m, node);
            let body = Some(self.transform_async_function_body(m, node, parameters));
            (parameters, body)
        } else {
            let (parameter_list, body) = {
                let read = m.node(node);
                (read.parameter_list(), read.body())
            };
            let parameters = self.context.clone().visit_parameters(parameter_list, m);
            let body = self.context.clone().visit_function_body(body, m);
            (parameters, body)
        };

        let (modifiers, asterisk_token, name) = {
            let read = m.node(node);
            let data = read
                .as_function_declaration()
                .expect("FunctionDeclaration payload");
            (data.modifiers(), data.asterisk_token(), data.name())
        };
        let modifiers = m.visit_modifiers(modifiers);
        let name = m.visit_node(name);
        let updated = m.update_function_declaration(
            node,
            modifiers,
            asterisk_token,
            name,
            None, /*typeParameters*/
            parameters,
            None, /*returnType*/
            None, /*fullSignature*/
            body,
        );
        self.lexical_arguments.set(saved_lexical_arguments);
        updated
    }

    /// Visits a FunctionExpression node.
    ///
    /// This function will be called when one of the following conditions are met:
    /// - The node is marked async
    // port: tsc/internal/transformers/estransforms/async.go:asyncTransformer.visitFunctionExpression
    fn visit_function_expression(&self, m: &mut NodeVisitor<'_>, node: NodeId) -> NodeId {
        let function_flags = self.function_flags(m.factory(), node);
        let saved_lexical_arguments = self.lexical_arguments.take();

        let (parameters, body) = if function_flags & function_flags::ASYNC != 0 {
            let parameters = self.transform_async_function_parameter_list(m, node);
            let body = Some(self.transform_async_function_body(m, node, parameters));
            (parameters, body)
        } else {
            let (parameter_list, body) = {
                let read = m.node(node);
                (read.parameter_list(), read.body())
            };
            let parameters = self.context.clone().visit_parameters(parameter_list, m);
            let body = self.context.clone().visit_function_body(body, m);
            (parameters, body)
        };

        let (modifiers, asterisk_token, name) = {
            let read = m.node(node);
            let data = read
                .as_function_expression()
                .expect("FunctionExpression payload");
            (data.modifiers(), data.asterisk_token(), data.name())
        };
        let modifiers = m.visit_modifiers(modifiers);
        let name = m.visit_node(name);
        let updated = m.update_function_expression(
            node,
            modifiers,
            asterisk_token,
            name,
            None, /*typeParameters*/
            parameters,
            None, /*returnType*/
            None, /*fullSignature*/
            body,
        );
        self.lexical_arguments.set(saved_lexical_arguments);
        updated
    }

    /// Visits an ArrowFunction.
    ///
    /// This function will be called when one of the following conditions are met:
    /// - The node is marked async
    // port: tsc/internal/transformers/estransforms/async.go:asyncTransformer.visitArrowFunction
    fn visit_arrow_function(&self, m: &mut NodeVisitor<'_>, node: NodeId) -> NodeId {
        // `arguments` in class static blocks is always an error, but we preserve Strada's emit
        // behavior for baseline compatibility. In Strada, checker-based `isArgumentsLocalBinding`
        // returns false for `arguments` in static blocks (since the binding doesn't exist due to
        // the error), so the async transform leaves them untouched.
        let saved_lexical_arguments =
            if self.context.emit_flags(node) & emit_flags::NO_LEXICAL_ARGUMENTS != 0 {
                Some(self.lexical_arguments.take())
            } else {
                None
            };

        let function_flags = self.function_flags(m.factory(), node);

        let (parameters, body) = if function_flags & function_flags::ASYNC != 0 {
            let parameters = self.transform_async_function_parameter_list(m, node);
            let body = Some(self.transform_async_function_body(m, node, parameters));
            (parameters, body)
        } else {
            let (parameter_list, body) = {
                let read = m.node(node);
                (read.parameter_list(), read.body())
            };
            let parameters = self.context.clone().visit_parameters(parameter_list, m);
            let body = self.context.clone().visit_function_body(body, m);
            (parameters, body)
        };

        let (modifiers, equals_greater_than_token) = {
            let read = m.node(node);
            let data = read.as_arrow_function().expect("ArrowFunction payload");
            (data.modifiers(), data.equals_greater_than_token())
        };
        let modifiers = m.visit_modifiers(modifiers);
        let updated = m.update_arrow_function(
            node,
            modifiers,
            None, /*typeParameters*/
            parameters,
            None, /*returnType*/
            None, /*fullSignature*/
            equals_greater_than_token,
            body,
        );
        // Upstream restores the saved arguments in a deferred call.
        if let Some(saved_lexical_arguments) = saved_lexical_arguments {
            self.lexical_arguments.set(saved_lexical_arguments);
        }
        updated
    }

    // port: tsc/internal/transformers/estransforms/async.go:asyncTransformer.recordDeclarationName
    #[allow(clippy::self_only_used_in_recursion)] // upstream's method
    fn record_declaration_name(
        &self,
        factory: &dyn RuntimeFactory,
        node: NodeId,
        names: &mut HashSet<JsString>,
    ) {
        let Some(name) = factory.node(node).name() else {
            return;
        };
        let read = factory.node(name);
        if read.kind() == K::Identifier {
            names.insert(identifier_text(factory, name));
        } else if tsr_ast::utilities::is_binding_pattern(&read) {
            for element in pattern_elements(factory, name) {
                if factory.node(element).kind() != K::OmittedExpression {
                    self.record_declaration_name(factory, element, names);
                }
            }
        }
    }

    // port: tsc/internal/transformers/estransforms/async.go:asyncTransformer.isVariableDeclarationListWithCollidingName
    fn is_variable_declaration_list_with_colliding_name(
        &self,
        factory: &dyn RuntimeFactory,
        node: Option<NodeId>,
    ) -> bool {
        let Some(node) = node else {
            return false;
        };
        let read = factory.node(node);
        read.kind() == K::VariableDeclarationList
            && read.flags() & node_flags::BLOCK_SCOPED == 0
            && declarations(factory, node)
                .into_iter()
                .any(|declaration| self.collides_with_parameter_name(factory, declaration))
    }

    // port: tsc/internal/transformers/estransforms/async.go:asyncTransformer.visitVariableDeclarationListWithCollidingNames
    fn visit_variable_declaration_list_with_colliding_names(
        &self,
        m: &mut NodeVisitor<'_>,
        node: NodeId,
        has_receiver: bool,
    ) -> Option<NodeId> {
        self.hoist_variable_declaration_list(m, node);

        let all = declarations(m.factory(), node);
        let variables: Vec<NodeId> = all
            .iter()
            .copied()
            .filter(|&decl| {
                m.node(decl)
                    .as_variable_declaration()
                    .expect("VariableDeclaration payload")
                    .initializer()
                    .is_some()
            })
            .collect();

        if variables.is_empty() {
            if has_receiver {
                let name = m.node(*all.first().expect(NIL)).name().expect(NIL);
                let target = if tsr_ast::utilities::is_binding_pattern(&m.node(name)) {
                    convert_binding_pattern_to_assignment_pattern(
                        &mut self.context.clone(),
                        m.factory_mut(),
                        name,
                    )
                } else {
                    name
                };
                return m.visit_node(Some(target));
            }
            return None;
        }

        let mut expressions = Vec::with_capacity(variables.len());
        for variable in variables {
            expressions.push(self.transform_initialized_variable(m, variable).expect(NIL));
        }
        self.context.inline_expressions(m, &expressions)
    }

    // port: tsc/internal/transformers/estransforms/async.go:asyncTransformer.hoistVariableDeclarationList
    fn hoist_variable_declaration_list(&self, m: &mut NodeVisitor<'_>, node: NodeId) {
        for decl in declarations(m.factory(), node) {
            self.hoist_variable(m, decl);
        }
    }

    // port: tsc/internal/transformers/estransforms/async.go:asyncTransformer.hoistVariable
    fn hoist_variable(&self, m: &mut NodeVisitor<'_>, node: NodeId) {
        let Some(name) = m.node(node).name() else {
            return;
        };
        let kind = m.node(name).kind();
        if kind == K::Identifier {
            self.context.clone().add_variable_declaration(m, name);
        } else if tsr_ast::utilities::is_binding_pattern_kind(kind) {
            for element in pattern_elements(m.factory(), name) {
                if m.node(element).kind() != K::OmittedExpression {
                    self.hoist_variable(m, element);
                }
            }
        }
    }

    // port: tsc/internal/transformers/estransforms/async.go:asyncTransformer.transformInitializedVariable
    fn transform_initialized_variable(
        &self,
        m: &mut NodeVisitor<'_>,
        node: NodeId,
    ) -> Option<NodeId> {
        let (name, initializer, loc) = {
            let read = m.node(node);
            let data = read
                .as_variable_declaration()
                .expect("VariableDeclaration payload");
            (data.name().expect(NIL), data.initializer(), read.range())
        };
        let mut context = self.context.clone();
        let target = if tsr_ast::utilities::is_binding_pattern(&m.node(name)) {
            convert_binding_pattern_to_assignment_pattern(&mut context, m.factory_mut(), name)
        } else {
            name
        };
        let converted = context.new_assignment_expression(m, target, initializer.expect(NIL));
        context.set_source_map_range(converted, loc);
        m.visit_node(Some(converted))
    }

    // port: tsc/internal/transformers/estransforms/async.go:asyncTransformer.collidesWithParameterName
    fn collides_with_parameter_name(&self, factory: &dyn RuntimeFactory, node: NodeId) -> bool {
        let Some(name) = factory.node(node).name() else {
            return false;
        };
        let kind = factory.node(name).kind();
        if kind == K::Identifier {
            return self
                .enclosing_function_parameter_names
                .borrow()
                .as_ref()
                .is_some_and(|names| names.contains(&identifier_text(factory, name)));
        }
        if tsr_ast::utilities::is_binding_pattern_kind(kind) {
            for element in pattern_elements(factory, name) {
                if factory.node(element).kind() != K::OmittedExpression
                    && self.collides_with_parameter_name(factory, element)
                {
                    return true;
                }
            }
        }
        false
    }

    // port: tsc/internal/transformers/estransforms/async.go:asyncTransformer.transformMethodBody
    fn transform_method_body(&self, m: &mut NodeVisitor<'_>, node: NodeId) -> NodeId {
        let saved_super_access = self.super_access.save();
        self.super_access.reset(m);

        let mut context = self.context.clone();
        context.start_variable_environment();
        let body = m.node(node).body();
        let mut updated = context.visit_function_body(body, m).expect(NIL);

        // Minor optimization, emit `_super` helper to capture `super` access in an arrow.
        let emit_super_helpers = (self.super_access.captured_super_property_count() > 0
            || self.super_access.has_super_element_access.get())
            && (self.function_flags(m.factory(), self.get_original_if_function_like(m, node))
                & function_flags::ASYNC_GENERATOR)
                != function_flags::ASYNC_GENERATOR;

        if emit_super_helpers && self.super_access.captured_super_property_count() > 0 {
            let statement = self
                .super_access
                .create_super_access_variable_statement(m.factory_mut());
            context.add_initialization_statement(statement);
        }

        let (statements, multi_line, loc) = block_parts(m.factory(), updated);
        let merged_statements =
            context.end_and_merge_variable_environment_list(m.factory_mut(), statements);
        if emit_super_helpers && self.super_access.has_super_element_access.get() && !multi_line {
            let new_block = m.new_block(merged_statements, true);
            m.set_node_range(new_block, loc);
            updated = new_block;
        } else {
            updated = m.update_block(updated, merged_statements, multi_line);
        }

        if emit_super_helpers && self.super_access.has_super_element_access.get() {
            if self.super_access.has_super_property_assignment.get() {
                context.add_emit_helper(updated, &[&emit_helpers::ADVANCED_ASYNC_SUPER_HELPER]);
            } else {
                context.add_emit_helper(updated, &[&emit_helpers::ASYNC_SUPER_HELPER]);
            }
        }

        self.super_access.restore(saved_super_access);
        updated
    }

    // port: tsc/internal/transformers/estransforms/async.go:asyncTransformer.createCaptureArgumentsStatement
    fn create_capture_arguments_statement(&self, m: &mut NodeVisitor<'_>) -> NodeId {
        let arguments = m.new_identifier(JsString::from_bytes(&b"arguments"[..]));
        let variable = m.new_variable_declaration(
            self.lexical_arguments.get().binding,
            None,
            None,
            Some(arguments),
        );
        let declarations = new_node_list(m.factory_mut(), vec![variable]);
        let decl_list = m.new_variable_declaration_list(Some(declarations), node_flags::NONE);
        let statement = m.new_variable_statement(None, Some(decl_list));
        self.context.clone().add_emit_flags(
            statement,
            emit_flags::START_ON_NEW_LINE | emit_flags::CUSTOM_PROLOGUE,
        );
        statement
    }

    // port: tsc/internal/transformers/estransforms/async.go:asyncTransformer.transformAsyncFunctionParameterList
    fn transform_async_function_parameter_list(
        &self,
        m: &mut NodeVisitor<'_>,
        node: NodeId,
    ) -> Option<NodeListId> {
        let parameter_list = m.node(node).parameter_list();
        let parameters = list_nodes(m.factory(), parameter_list.expect(NIL));
        if is_simple_parameter_list(m.factory(), &parameters) {
            return self.context.clone().visit_parameters(parameter_list, m);
        }

        let mut context = self.context.clone();
        let mut new_parameters = Vec::new();
        for parameter in parameters {
            let (initializer, dot_dot_dot_token, name) = parameter_parts(m.factory(), parameter);
            if initializer.is_some() || dot_dot_dot_token.is_some() {
                // for an arrow function, capture the remaining arguments in a rest parameter.
                // for any other function/method this isn't necessary as we can just use `arguments`.
                if m.node(node).kind() == K::ArrowFunction {
                    let dot_dot_dot = m.new_token(K::DotDotDotToken.into());
                    let args = context.new_unique_name_ex(
                        m,
                        JsString::from_bytes(&b"args"[..]),
                        AutoGenerateOptions {
                            flags: g::RESERVED_IN_NESTED_SCOPES,
                            ..AutoGenerateOptions::default()
                        },
                    );
                    let rest_parameter = m.new_parameter_declaration(
                        None,
                        Some(dot_dot_dot),
                        Some(args),
                        None,
                        None,
                        None,
                    );
                    new_parameters.push(rest_parameter);
                }
                break;
            }
            // for arrow functions we capture fixed parameters to forward to `__awaiter`. For all other functions
            // we add fixed parameters to preserve the function's `length` property.
            let generated = context.new_generated_name_for_node_ex(
                m,
                name.expect(NIL),
                AutoGenerateOptions {
                    flags: g::RESERVED_IN_NESTED_SCOPES,
                    ..AutoGenerateOptions::default()
                },
            );
            let new_parameter =
                m.new_parameter_declaration(None, None, Some(generated), None, None, None);
            new_parameters.push(new_parameter);
        }
        let new_parameters_array = new_node_list(m.factory_mut(), new_parameters);
        let loc = m.factory().read_list(parameter_list.expect(NIL)).loc();
        m.factory_mut().set_list_location(new_parameters_array, loc);
        Some(new_parameters_array)
    }

    // port: tsc/internal/transformers/estransforms/async.go:asyncTransformer.transformAsyncFunctionBody
    #[allow(clippy::too_many_lines)] // upstream's function
    fn transform_async_function_body(
        &self,
        m: &mut NodeVisitor<'_>,
        node: NodeId,
        outer_parameters: Option<NodeListId>,
    ) -> NodeId {
        let is_arrow = m.node(node).kind() == K::ArrowFunction;
        let saved_super_access = if is_arrow {
            None
        } else {
            let saved = self.super_access.save();
            self.super_access.reset(m);
            Some(saved)
        };
        let mut context = self.context.clone();

        let (parameter_list, body) = {
            let read = m.node(node);
            (read.parameter_list(), read.body())
        };
        let parameters = list_nodes(m.factory(), parameter_list.expect(NIL));
        let mut inner_parameters = None;
        if !is_simple_parameter_list(m.factory(), &parameters) {
            inner_parameters = context.visit_parameters(parameter_list, m);
        }

        let saved_lexical_arguments = self.lexical_arguments.get();
        let capture_lexical_arguments = saved_lexical_arguments.binding.is_none();
        if capture_lexical_arguments {
            let binding = context.new_unique_name(m, JsString::from_bytes(&b"arguments"[..]));
            self.lexical_arguments.set(LexicalArgumentsInfo {
                binding: Some(binding),
                used: false,
            });
        }

        let mut arguments_expression = None;
        if inner_parameters.is_some() {
            if is_arrow {
                // `node` does not have a simple parameter list, so `outerParameters` refers to placeholders that are
                // forwarded to `innerParameters`, matching how they are introduced in `transformAsyncFunctionParameterList`.
                let mut parameter_bindings = Vec::new();
                let outer = list_nodes(m.factory(), outer_parameters.expect(NIL));
                for (i, &param) in parameters.iter().enumerate() {
                    if i >= outer.len() {
                        break;
                    }
                    let (initializer, dot_dot_dot_token, _) = parameter_parts(m.factory(), param);
                    let (_, _, outer_name) = parameter_parts(m.factory(), outer[i]);
                    if initializer.is_some() || dot_dot_dot_token.is_some() {
                        parameter_bindings.push(m.new_spread_element(outer_name));
                        break;
                    }
                    parameter_bindings.push(outer_name.expect(NIL));
                }
                let bindings = new_node_list(m.factory_mut(), parameter_bindings);
                arguments_expression = Some(m.new_array_literal_expression(Some(bindings), false));
            } else {
                arguments_expression =
                    Some(m.new_identifier(JsString::from_bytes(&b"arguments"[..])));
            }
        }

        // An async function is emit as an outer function that calls an inner
        // generator function. To preserve lexical bindings, we pass the current
        // `this` and `arguments` objects to `__awaiter`. The generator function
        // passed to `__awaiter` is executed inside of the callback to the
        // promise constructor.

        let mut names = HashSet::new();
        for &parameter in &parameters {
            self.record_declaration_name(m.factory(), parameter, &mut names);
        }
        let saved_enclosing_function_parameter_names = self
            .enclosing_function_parameter_names
            .replace(Some(Rc::new(names)));

        let has_lexical_this = self.in_has_lexical_this_context();

        let mut async_body = self.transform_async_function_body_worker(m, body.expect(NIL));
        let (statements, multi_line, _) = block_parts(m.factory(), async_body);
        let merged = context.end_and_merge_variable_environment_list(m.factory_mut(), statements);
        async_body = m.update_block(async_body, merged, multi_line);

        // Substitute super property accesses with _super/_superIndex helpers
        let emit_super_helpers = self
            .super_access
            .captured_super_properties
            .borrow()
            .as_ref()
            .is_some_and(|captured| {
                !captured.is_empty() || self.super_access.has_super_element_access.get()
            });
        if emit_super_helpers {
            inner_parameters = self
                .super_access
                .with_super_access_visitor(m, |visitor| visitor.visit_nodes(inner_parameters));
            async_body = self
                .super_access
                .substitute_super_accesses_in_body(m, async_body)
                .expect(NIL);
        }

        let result = if is_arrow {
            let mut result = context.new_awaiter_helper(
                m.factory_mut(),
                has_lexical_this,
                arguments_expression,
                inner_parameters,
                async_body,
            );

            if capture_lexical_arguments && self.lexical_arguments.get().used {
                let block = context.convert_to_function_block(
                    m.factory_mut(),
                    result,
                    true, /*multiLine*/
                );
                if m.node(result).kind() != K::Block {
                    let (statements, _, _) = block_parts(m.factory(), block);
                    let first = list_nodes(m.factory(), statements.expect(NIL))[0];
                    context.set_original(first, result);
                }
                let (statements, multi_line, _) = block_parts(m.factory(), block);
                let capture = self.create_capture_arguments_statement(m);
                let merged = context.merge_environment_list(
                    m.factory_mut(),
                    statements.expect(NIL),
                    &[capture],
                );
                result = m.update_block(block, Some(merged), multi_line);
            }
            result
        } else {
            context.start_variable_environment();

            // Minor optimization, emit `_super` helper to capture `super` access in an arrow.
            if emit_super_helpers && self.super_access.captured_super_property_count() > 0 {
                let statement = self
                    .super_access
                    .create_super_access_variable_statement(m.factory_mut());
                context.add_initialization_statement(statement);
            }

            if capture_lexical_arguments && self.lexical_arguments.get().used {
                let statement = self.create_capture_arguments_statement(m);
                context.add_initialization_statement(statement);
            }

            let awaiter = context.new_awaiter_helper(
                m.factory_mut(),
                has_lexical_this,
                arguments_expression,
                inner_parameters,
                async_body,
            );
            let statements = vec![m.new_return_statement(Some(awaiter))];

            let statements = new_node_list(m.factory_mut(), statements);
            let statements =
                context.end_and_merge_variable_environment_list(m.factory_mut(), Some(statements));
            let block = m.new_block(statements, true);
            let loc = m.node(body.expect(NIL)).range();
            m.set_node_range(block, loc);

            if emit_super_helpers && self.super_access.has_super_element_access.get() {
                if self.super_access.has_super_property_assignment.get() {
                    context.add_emit_helper(block, &[&emit_helpers::ADVANCED_ASYNC_SUPER_HELPER]);
                } else {
                    context.add_emit_helper(block, &[&emit_helpers::ASYNC_SUPER_HELPER]);
                }
            }

            block
        };

        *self.enclosing_function_parameter_names.borrow_mut() =
            saved_enclosing_function_parameter_names;
        if let Some(saved_super_access) = saved_super_access {
            self.super_access.restore(saved_super_access);
            self.lexical_arguments.set(saved_lexical_arguments);
        } else if capture_lexical_arguments && !self.lexical_arguments.get().used {
            // If we created a new binding but it wasn't used, restore the previous state.
            // If it was used, keep the binding alive so sibling arrows can reuse it
            // (the `var` declaration hoists to the enclosing function scope).
            self.lexical_arguments.set(saved_lexical_arguments);
        } else if capture_lexical_arguments {
            // Keep the binding but clear the used flag so siblings don't re-emit the capture statement.
            self.lexical_arguments.set(LexicalArgumentsInfo {
                used: false,
                ..self.lexical_arguments.get()
            });
        }
        result
    }

    // port: tsc/internal/transformers/estransforms/async.go:asyncTransformer.transformAsyncFunctionBodyWorker
    fn transform_async_function_body_worker(
        &self,
        m: &mut NodeVisitor<'_>,
        body: NodeId,
    ) -> NodeId {
        if m.node(body).kind() == K::Block {
            let (statements, multi_line, _) = block_parts(m.factory(), body);
            let statements =
                self.with_visitor(m, Visitor::AsyncBody, |a| a.visit_nodes(statements));
            return m.update_block(body, statements, multi_line);
        }
        // Convert expression body to block body with return statement
        let visited = self.with_visitor(m, Visitor::AsyncBody, |a| a.visit_node(Some(body)));
        let ret = m.new_return_statement(visited);
        let loc = m.node(body).range();
        m.set_node_range(ret, loc);
        let list = new_node_list(m.factory_mut(), vec![ret]);
        m.factory_mut().set_list_location(list, loc);
        let block = m.new_block(Some(list), false /*multiLine*/);
        m.set_node_range(block, loc);
        block
    }

    // port: tsc/internal/transformers/estransforms/async.go:asyncTransformer.getOriginalIfFunctionLike
    fn get_original_if_function_like(&self, m: &NodeVisitor<'_>, node: NodeId) -> NodeId {
        let original = self.context.most_original(node);
        if tsr_ast::utilities::is_function_like_declaration(Some(&m.node(original))) {
            return original;
        }
        node
    }
}

/// Checks top-down whether an assignment target expression contains a super
/// property or element access (`super.x` or `super[x]`). This avoids relying
/// on parent pointers (`IsAssignmentTarget`) which may not be set on
/// synthesized AST nodes from prior transforms.
// port: tsc/internal/transformers/estransforms/async.go:assignmentTargetContainsSuperProperty
#[allow(clippy::match_same_arms)] // upstream's case order
pub(super) fn assignment_target_contains_super_property(
    factory: &dyn RuntimeFactory,
    node: NodeId,
) -> bool {
    let read = factory.node(node);
    match read.kind().known() {
        Some(K::PropertyAccessExpression | K::ElementAccessExpression) => {
            factory.node(read.expression().expect(NIL)).kind() == K::SuperKeyword
        }
        Some(K::ParenthesizedExpression) => {
            assignment_target_contains_super_property(factory, read.expression().expect(NIL))
        }
        Some(K::ArrayLiteralExpression) => {
            let elements = read
                .as_array_literal_expression()
                .expect("ArrayLiteralExpression payload")
                .elements();
            elements.is_some_and(|elements| {
                list_nodes(factory, elements)
                    .into_iter()
                    .any(|element| assignment_target_contains_super_property(factory, element))
            })
        }
        Some(K::ObjectLiteralExpression) => {
            let properties = read
                .as_object_literal_expression()
                .expect("ObjectLiteralExpression payload")
                .properties();
            for prop in properties.map_or_else(Vec::new, |list| list_nodes(factory, list)) {
                let prop_read = factory.node(prop);
                let target = match prop_read.kind().known() {
                    Some(K::PropertyAssignment) => prop_read
                        .as_property_assignment()
                        .expect("PropertyAssignment payload")
                        .initializer(),
                    Some(K::ShorthandPropertyAssignment) => prop_read.name(),
                    Some(K::SpreadAssignment) => prop_read.expression(),
                    _ => continue,
                };
                if assignment_target_contains_super_property(factory, target.expect(NIL)) {
                    return true;
                }
            }
            false
        }
        Some(K::SpreadElement) => {
            assignment_target_contains_super_property(factory, read.expression().expect(NIL))
        }
        _ => false,
    }
}

/// Checks if a prefix/postfix unary expression is `++` or `--`.
// port: tsc/internal/transformers/estransforms/async.go:isUpdateExpression
pub(super) fn is_update_expression(factory: &dyn RuntimeFactory, node: NodeId) -> bool {
    let read = factory.node(node);
    if let Some(data) = read.as_prefix_unary_expression() {
        let op = data.operator();
        return op == K::PlusPlusToken || op == K::MinusMinusToken;
    }
    if let Some(data) = read.as_postfix_unary_expression() {
        let op = data.operator();
        return op == K::PlusPlusToken || op == K::MinusMinusToken;
    }
    false
}

/// Checks if every parameter has no initializer and an Identifier name.
// port: tsc/internal/transformers/estransforms/async.go:isSimpleParameterList
fn is_simple_parameter_list(factory: &dyn RuntimeFactory, params: &[NodeId]) -> bool {
    for &param in params {
        let (initializer, _, name) = parameter_parts(factory, param);
        if initializer.is_some() || factory.node(name.expect(NIL)).kind() != K::Identifier {
            return false;
        }
    }
    true
}

/// Checks if a node could contain hoisted declarations.
// port: tsc/internal/transformers/estransforms/async.go:isNodeWithPossibleHoistedDeclaration
fn is_node_with_possible_hoisted_declaration(kind: NodeKind) -> bool {
    matches!(
        kind.known(),
        Some(
            K::Block
                | K::VariableStatement
                | K::WithStatement
                | K::IfStatement
                | K::SwitchStatement
                | K::CaseBlock
                | K::CaseClause
                | K::DefaultClause
                | K::LabeledStatement
                | K::ForStatement
                | K::ForInStatement
                | K::ForOfStatement
                | K::DoStatement
                | K::WhileStatement
                | K::TryStatement
                | K::CatchClause
        )
    )
}

/// The nodes of a list, with Go's nil elements rejected.
fn list_nodes(factory: &dyn RuntimeFactory, list: NodeListId) -> Vec<NodeId> {
    let nodes = factory.read_list(list).nodes();
    factory
        .read_nodes(nodes)
        .iter()
        .map(|node| node.expect(NIL))
        .collect()
}

/// A binding pattern's `Elements.Nodes`.
fn pattern_elements(factory: &dyn RuntimeFactory, pattern: NodeId) -> Vec<NodeId> {
    let elements = factory
        .node(pattern)
        .as_binding_pattern()
        .expect("BindingPattern payload")
        .elements();
    list_nodes(factory, elements.expect(NIL))
}

/// A variable declaration list's `Declarations.Nodes`.
fn declarations(factory: &dyn RuntimeFactory, list: NodeId) -> Vec<NodeId> {
    let declarations = factory
        .node(list)
        .as_variable_declaration_list()
        .expect("VariableDeclarationList payload")
        .declarations();
    list_nodes(factory, declarations.expect(NIL))
}

/// A parameter's initializer, rest token and name.
fn parameter_parts(
    factory: &dyn RuntimeFactory,
    parameter: NodeId,
) -> (Option<NodeId>, Option<NodeId>, Option<NodeId>) {
    let read = factory.node(parameter);
    let data = read
        .as_parameter_declaration()
        .expect("ParameterDeclaration payload");
    (data.initializer(), data.dot_dot_dot_token(), data.name())
}

/// A block's statement list, `MultiLine` flag and location.
fn block_parts(
    factory: &dyn RuntimeFactory,
    block: NodeId,
) -> (Option<NodeListId>, bool, TextRange) {
    let read = factory.node(block);
    let data = read.as_block().expect("Block payload");
    (data.statements(), data.multi_line(), read.range())
}

/// A for-in or for-of statement's initializer, expression and statement.
fn for_in_or_of_parts(
    factory: &dyn Factory,
    node: NodeId,
) -> (Option<NodeId>, Option<NodeId>, Option<NodeId>) {
    let read = factory.node(node);
    let data = read
        .as_for_in_or_of_statement()
        .expect("ForInOrOfStatement payload");
    (data.initializer(), data.expression(), data.statement())
}

/// A binding element's rest token, name and initializer.
fn binding_element_parts(
    factory: &dyn RuntimeFactory,
    element: NodeId,
) -> (Option<NodeId>, Option<NodeId>, Option<NodeId>) {
    let read = factory.node(element);
    let data = read.as_binding_element().expect("BindingElement payload");
    (data.dot_dot_dot_token(), data.name(), data.initializer())
}

// TODO(h1): transformers.convertBindingElementToArrayAssignmentElement
fn convert_binding_element_to_array_assignment_element(
    context: &mut EmitContext,
    factory: &mut dyn RuntimeFactory,
    element: NodeId,
) -> NodeId {
    let (dot_dot_dot_token, name, initializer) = binding_element_parts(factory, element);
    let Some(name) = name else {
        let elision = factory.new_omitted_expression();
        context.set_original(elision, element);
        context.assign_comment_and_source_map_ranges(factory, elision, element);
        return elision;
    };
    if dot_dot_dot_token.is_some() {
        let spread = factory.new_spread_element(Some(name));
        context.set_original(spread, element);
        context.assign_comment_and_source_map_ranges(factory, spread, element);
        return spread;
    }
    let expression = convert_binding_name_to_assignment_element_target(context, factory, name);
    if let Some(initializer) = initializer {
        let assignment = context.new_assignment_expression(factory, expression, initializer);
        context.set_original(assignment, element);
        context.assign_comment_and_source_map_ranges(factory, assignment, element);
        return assignment;
    }
    expression
}

// TODO(h1): transformers.convertBindingElementToObjectAssignmentElement
fn convert_binding_element_to_object_assignment_element(
    context: &mut EmitContext,
    factory: &mut dyn RuntimeFactory,
    element: NodeId,
) -> NodeId {
    let (dot_dot_dot_token, name, initializer) = binding_element_parts(factory, element);
    if dot_dot_dot_token.is_some() {
        let spread = factory.new_spread_assignment(name);
        context.set_original(spread, element);
        context.assign_comment_and_source_map_ranges(factory, spread, element);
        return spread;
    }
    let property_name = factory
        .node(element)
        .as_binding_element()
        .expect("BindingElement payload")
        .property_name();
    if let Some(property_name) = property_name {
        let mut expression =
            convert_binding_name_to_assignment_element_target(context, factory, name.expect(NIL));
        if let Some(initializer) = initializer {
            expression = context.new_assignment_expression(factory, expression, initializer);
        }
        let assignment = factory.new_property_assignment(
            None, /*modifiers*/
            Some(property_name),
            None, /*postfixToken*/
            None, /*typeNode*/
            Some(expression),
        );
        context.set_original(assignment, element);
        context.assign_comment_and_source_map_ranges(factory, assignment, element);
        return assignment;
    }
    let equals_token = initializer.map(|_| factory.new_token(K::EqualsToken.into()));
    let assignment = factory.new_shorthand_property_assignment(
        None, /*modifiers*/
        name,
        None, /*postfixToken*/
        None, /*typeNode*/
        equals_token,
        initializer,
    );
    context.set_original(assignment, element);
    context.assign_comment_and_source_map_ranges(factory, assignment, element);
    assignment
}

// TODO(h1): transformers.ConvertBindingPatternToAssignmentPattern
fn convert_binding_pattern_to_assignment_pattern(
    context: &mut EmitContext,
    factory: &mut dyn RuntimeFactory,
    element: NodeId,
) -> NodeId {
    match factory.node(element).kind().known() {
        Some(K::ArrayBindingPattern) => {
            convert_binding_element_to_array_assignment_pattern(context, factory, element)
        }
        Some(K::ObjectBindingPattern) => {
            convert_binding_element_to_object_assignment_pattern(context, factory, element)
        }
        _ => panic!("Unknown binding pattern"),
    }
}

// TODO(h1): transformers.convertBindingElementToObjectAssignmentPattern
fn convert_binding_element_to_object_assignment_pattern(
    context: &mut EmitContext,
    factory: &mut dyn RuntimeFactory,
    element: NodeId,
) -> NodeId {
    let elements = factory
        .node(element)
        .as_binding_pattern()
        .expect("BindingPattern payload")
        .elements()
        .expect(NIL);
    let mut properties = Vec::new();
    for binding_element in list_nodes(factory, elements) {
        properties.push(convert_binding_element_to_object_assignment_element(
            context,
            factory,
            binding_element,
        ));
    }
    let property_list = new_node_list(factory, properties);
    let loc = factory.read_list(elements).loc();
    factory.set_list_location(property_list, loc);
    let object =
        factory.new_object_literal_expression(Some(property_list), false /*multiLine*/);
    context.set_original(object, element);
    context.assign_comment_and_source_map_ranges(factory, object, element);
    object
}

// TODO(h1): transformers.convertBindingElementToArrayAssignmentPattern
fn convert_binding_element_to_array_assignment_pattern(
    context: &mut EmitContext,
    factory: &mut dyn RuntimeFactory,
    element: NodeId,
) -> NodeId {
    let elements = factory
        .node(element)
        .as_binding_pattern()
        .expect("BindingPattern payload")
        .elements()
        .expect(NIL);
    let mut converted = Vec::new();
    for binding_element in list_nodes(factory, elements) {
        converted.push(convert_binding_element_to_array_assignment_element(
            context,
            factory,
            binding_element,
        ));
    }
    let element_list = new_node_list(factory, converted);
    let loc = factory.read_list(elements).loc();
    factory.set_list_location(element_list, loc);
    let object = factory.new_array_literal_expression(Some(element_list), false /*multiLine*/);
    context.set_original(object, element);
    context.assign_comment_and_source_map_ranges(factory, object, element);
    object
}

// TODO(h1): transformers.convertBindingNameToAssignmentElementTarget
fn convert_binding_name_to_assignment_element_target(
    context: &mut EmitContext,
    factory: &mut dyn RuntimeFactory,
    element: NodeId,
) -> NodeId {
    if tsr_ast::utilities::is_binding_pattern(&factory.node(element)) {
        return convert_binding_pattern_to_assignment_pattern(context, factory, element);
    }
    element
}
