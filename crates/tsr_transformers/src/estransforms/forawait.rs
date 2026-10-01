//! `transformers/estransforms/forawait.go`: lowers `for await` loops and async
//! generators for targets before ES2018.
use super::utilities::{ast_view, list_nodes, new_node_list, SuperAccessState};
use crate::transformer::{Failure, TransformOptions, Transformer};
use std::cell::Cell;
use std::rc::Rc;
use tsr_ast::{
    node_flags, subtree_flags,
    utilities_containers::{function_flags, get_function_flags},
    Factory, FactoryMethods, JsString, NodeId, NodeListId, NodeVisit, NodeVisitor, RuntimeFactory,
    SyntaxKind as K,
};
use tsr_core::{collections::OrderedSet, TextRange};
use tsr_printer::{
    emit_flags, emit_helpers, generated_identifier_flags as g, AutoGenerateOptions, EmitContext,
    EmitVisitorHooks,
};

const NIL: &str = "runtime error: invalid memory address or nil pointer dereference";

/// Facts we track as we traverse the tree (`forAwaitHierarchyFacts`).
mod facts {
    pub(super) const NONE: u32 = 0;

    //
    // Ancestor facts
    //

    pub(super) const HAS_LEXICAL_THIS: u32 = 1 << 0;
    pub(super) const ITERATION_CONTAINER: u32 = 1 << 1;

    //
    // Ancestor masks
    //

    pub(super) const ANCESTOR_FACTS_MASK: u32 = (1 << 2) - 1;

    pub(super) const SOURCE_FILE_EXCLUDES: u32 = ITERATION_CONTAINER;
    pub(super) const STRICT_MODE_SOURCE_FILE_INCLUDES: u32 = NONE;

    pub(super) const CLASS_OR_FUNCTION_INCLUDES: u32 = HAS_LEXICAL_THIS;
    pub(super) const CLASS_OR_FUNCTION_EXCLUDES: u32 = ITERATION_CONTAINER;

    pub(super) const ARROW_FUNCTION_INCLUDES: u32 = NONE;
    pub(super) const ARROW_FUNCTION_EXCLUDES: u32 = CLASS_OR_FUNCTION_EXCLUDES;

    pub(super) const ITERATION_STATEMENT_INCLUDES: u32 = ITERATION_CONTAINER;
    pub(super) const ITERATION_STATEMENT_EXCLUDES: u32 = NONE;
}

/// `forawaitTransformer`. Upstream's `compilerOptions` and
/// `exportedVariableStatement` are written and never read; the latter is kept
/// for its reset in `visitSourceFile`.
struct ForawaitTransformer {
    emit_context: EmitContext,
    /// The hooks of `EmitContext.NewNodeVisitor`, for the fallback and
    /// no-async-modifier visitors.
    hooks: EmitVisitorHooks,
    failure: Failure,
    super_access: SuperAccessState,
    enclosing_function_flags: Cell<u32>,
    for_await_hierarchy_facts: Cell<u32>,
    exported_variable_statement: Cell<bool>,
}

type Visit = fn(&ForawaitTransformer, &mut NodeVisitor<'_>, NodeId) -> NodeId;

// port: tsc/internal/transformers/estransforms/forawait.go:newforawaitTransformer
pub fn new_for_await_transformer<'a>(opts: &TransformOptions<'a>) -> Option<Transformer<'a>> {
    let tx = Rc::new(ForawaitTransformer {
        emit_context: opts.context.clone(),
        hooks: opts.context.visitor_hooks(),
        failure: opts.failure.clone(),
        super_access: SuperAccessState::new(&opts.context, opts.failure.clone()),
        enclosing_function_flags: Cell::new(function_flags::NORMAL),
        for_await_hierarchy_facts: Cell::new(facts::NONE),
        exported_variable_statement: Cell::new(false),
    });
    Some(Transformer::new(
        move |visitor: &mut NodeVisitor<'_>, node: Option<NodeId>| tx.visit(visitor, node),
        Some(opts.context.clone()),
        opts.failure.clone(),
    ))
}

impl ForawaitTransformer {
    /// The emit context handle, for its `&mut self` operations.
    fn ctx(&self) -> EmitContext {
        self.emit_context.clone()
    }

    fn function_flags(&self) -> u32 {
        self.enclosing_function_flags.get()
    }

    fn in_async_generator(&self) -> bool {
        let flags = self.function_flags();
        flags & function_flags::ASYNC != 0 && flags & function_flags::GENERATOR != 0
    }

    // port: tsc/internal/transformers/estransforms/forawait.go:forawaitTransformer.affectsSubtree
    fn affects_subtree(&self, exclude_facts: u32, include_facts: u32) -> bool {
        let current = self.for_await_hierarchy_facts.get();
        current != (current & !exclude_facts | include_facts)
    }

    /// Sets the HierarchyFacts for this node prior to visiting this node's
    /// subtree, returning the facts set prior to modification.
    // port: tsc/internal/transformers/estransforms/forawait.go:forawaitTransformer.enterSubtree
    fn enter_subtree(&self, exclude_facts: u32, include_facts: u32) -> u32 {
        let ancestor_facts = self.for_await_hierarchy_facts.get();
        self.for_await_hierarchy_facts
            .set((ancestor_facts & !exclude_facts | include_facts) & facts::ANCESTOR_FACTS_MASK);
        ancestor_facts
    }

    /// Restores the HierarchyFacts for this node's ancestor after visiting
    /// this node's subtree.
    // port: tsc/internal/transformers/estransforms/forawait.go:forawaitTransformer.exitSubtree
    fn exit_subtree(&self, ancestor_facts: u32) {
        self.for_await_hierarchy_facts.set(ancestor_facts);
    }

    // port: tsc/internal/transformers/estransforms/forawait.go:forawaitTransformer.visitModifiersNoAsync
    fn visit_modifiers_no_async(
        &self,
        visitor: &mut NodeVisitor<'_>,
        modifiers: Option<NodeListId>,
    ) -> Option<NodeListId> {
        let visit: &NodeVisit<'_> = &|visitor: &mut NodeVisitor<'_>, node: Option<NodeId>| {
            let id = node.expect(NIL);
            if visitor.node(id).kind() == K::AsyncKeyword {
                return None;
            }
            node
        };
        self.hooks
            .new_node_visitor(Some(visit), visitor.factory_mut())
            .visit_modifiers(modifiers)
    }

    // port: tsc/internal/transformers/estransforms/forawait.go:forawaitTransformer.doWithHierarchyFacts
    fn do_with_hierarchy_facts(
        &self,
        cb: Visit,
        visitor: &mut NodeVisitor<'_>,
        node: NodeId,
        exclude_facts: u32,
        include_facts: u32,
    ) -> NodeId {
        if self.affects_subtree(exclude_facts, include_facts) {
            let ancestor_facts = self.enter_subtree(exclude_facts, include_facts);
            let result = cb(self, visitor, node);
            self.exit_subtree(ancestor_facts);
            return result;
        }
        cb(self, visitor, node)
    }

    // port: tsc/internal/transformers/estransforms/forawait.go:forawaitTransformer.visitDefault
    fn visit_default(_tx: &Self, visitor: &mut NodeVisitor<'_>, node: NodeId) -> NodeId {
        visitor.visit_each_child(Some(node)).expect(NIL)
    }

    /// `trackSuperAccess(node)`; false after recording a failed read.
    fn track(&self, visitor: &NodeVisitor<'_>, node: NodeId) -> bool {
        self.failure
            .ok(self
                .super_access
                .track_super_access(visitor.factory(), node))
            .is_some()
    }

    // port: tsc/internal/transformers/estransforms/forawait.go:forawaitTransformer.fallbackVisitor
    fn fallback_visitor(&self, visitor: &mut NodeVisitor<'_>, node: NodeId) -> Option<NodeId> {
        if self
            .super_access
            .captured_super_properties
            .borrow()
            .is_none()
        {
            return Some(node);
        }
        if matches!(
            visitor.node(node).kind().known(),
            Some(
                K::FunctionExpression
                    | K::FunctionDeclaration
                    | K::MethodDeclaration
                    | K::GetAccessor
                    | K::SetAccessor
                    | K::Constructor
            )
        ) {
            return Some(node);
        }
        if !self.track(visitor, node) {
            return Some(node);
        }
        let visit: &NodeVisit<'_> = &|visitor: &mut NodeVisitor<'_>, node: Option<NodeId>| {
            self.visit_fallback(visitor, node)
        };
        self.hooks
            .new_node_visitor(Some(visit), visitor.factory_mut())
            .visit_each_child(Some(node))
    }

    // port: tsc/internal/transformers/estransforms/forawait.go:forawaitTransformer.visitFallback
    fn visit_fallback(
        &self,
        visitor: &mut NodeVisitor<'_>,
        node: Option<NodeId>,
    ) -> Option<NodeId> {
        if self.failure.is_set() {
            return node;
        }
        self.fallback_visitor(visitor, node.expect(NIL))
    }

    #[allow(clippy::match_same_arms)] // Keep each pinned case independently auditable.
                                      // port: tsc/internal/transformers/estransforms/forawait.go:forawaitTransformer.visit
    fn visit(&self, visitor: &mut NodeVisitor<'_>, node: Option<NodeId>) -> Option<NodeId> {
        if self.failure.is_set() {
            return node;
        }
        let id = node.expect(NIL);
        if ast_view(visitor.factory()).subtree_facts(id)
            & subtree_flags::FOR_AWAIT_OR_ASYNC_GENERATOR
            == 0
        {
            return self.fallback_visitor(visitor, id);
        }
        if !self.track(visitor, id) {
            return node;
        }
        match visitor.node(id).kind().known() {
            Some(K::SourceFile) => Some(self.visit_source_file(visitor, id)),
            Some(K::AwaitExpression) => self.visit_await_expression(visitor, id),
            Some(K::YieldExpression) => self.visit_yield_expression(visitor, id),
            Some(K::ReturnStatement) => self.visit_return_statement(visitor, id),
            Some(K::LabeledStatement) => self.visit_labeled_statement(visitor, id),
            Some(K::DoStatement | K::WhileStatement | K::ForInStatement) => {
                Some(self.do_with_hierarchy_facts(
                    Self::visit_default,
                    visitor,
                    id,
                    facts::ITERATION_STATEMENT_EXCLUDES,
                    facts::ITERATION_STATEMENT_INCLUDES,
                ))
            }
            Some(K::ForOfStatement) => Some(self.visit_for_of_statement(visitor, id, None)),
            Some(K::ForStatement) => Some(self.do_with_hierarchy_facts(
                Self::visit_default,
                visitor,
                id,
                facts::ITERATION_STATEMENT_EXCLUDES,
                facts::ITERATION_STATEMENT_INCLUDES,
            )),
            Some(K::Constructor) => Some(self.do_with_hierarchy_facts(
                Self::visit_constructor_declaration,
                visitor,
                id,
                facts::CLASS_OR_FUNCTION_EXCLUDES,
                facts::CLASS_OR_FUNCTION_INCLUDES,
            )),
            Some(K::MethodDeclaration) => Some(self.do_with_hierarchy_facts(
                Self::visit_method_declaration,
                visitor,
                id,
                facts::CLASS_OR_FUNCTION_EXCLUDES,
                facts::CLASS_OR_FUNCTION_INCLUDES,
            )),
            Some(K::GetAccessor) => Some(self.do_with_hierarchy_facts(
                Self::visit_get_accessor_declaration,
                visitor,
                id,
                facts::CLASS_OR_FUNCTION_EXCLUDES,
                facts::CLASS_OR_FUNCTION_INCLUDES,
            )),
            Some(K::SetAccessor) => Some(self.do_with_hierarchy_facts(
                Self::visit_set_accessor_declaration,
                visitor,
                id,
                facts::CLASS_OR_FUNCTION_EXCLUDES,
                facts::CLASS_OR_FUNCTION_INCLUDES,
            )),
            Some(K::FunctionDeclaration) => Some(self.do_with_hierarchy_facts(
                Self::visit_function_declaration,
                visitor,
                id,
                facts::CLASS_OR_FUNCTION_EXCLUDES,
                facts::CLASS_OR_FUNCTION_INCLUDES,
            )),
            Some(K::FunctionExpression) => Some(self.do_with_hierarchy_facts(
                Self::visit_function_expression,
                visitor,
                id,
                facts::CLASS_OR_FUNCTION_EXCLUDES,
                facts::CLASS_OR_FUNCTION_INCLUDES,
            )),
            Some(K::ArrowFunction) => Some(self.do_with_hierarchy_facts(
                Self::visit_arrow_function,
                visitor,
                id,
                facts::ARROW_FUNCTION_EXCLUDES,
                facts::ARROW_FUNCTION_INCLUDES,
            )),
            Some(K::ClassDeclaration | K::ClassExpression) => Some(self.do_with_hierarchy_facts(
                Self::visit_default,
                visitor,
                id,
                facts::CLASS_OR_FUNCTION_EXCLUDES,
                facts::CLASS_OR_FUNCTION_INCLUDES,
            )),
            _ => visitor.visit_each_child(node),
        }
    }

    // port: tsc/internal/transformers/estransforms/forawait.go:forawaitTransformer.visitAwaitExpression
    fn visit_await_expression(
        &self,
        visitor: &mut NodeVisitor<'_>,
        node: NodeId,
    ) -> Option<NodeId> {
        if self.in_async_generator() {
            let (expression, loc) = {
                let read = visitor.node(node);
                (read.expression(), read.range())
            };
            let expression = visitor.visit_node(expression).expect(NIL);
            let factory = visitor.factory_mut();
            let awaited = self.ctx().new_await_helper(factory, expression);
            let result = factory.new_yield_expression(None /*asteriskToken*/, Some(awaited));
            factory.set_node_range(result, loc);
            self.ctx().set_original(result, node);
            return Some(result);
        }
        visitor.visit_each_child(Some(node))
    }

    // port: tsc/internal/transformers/estransforms/forawait.go:forawaitTransformer.visitYieldExpression
    fn visit_yield_expression(
        &self,
        visitor: &mut NodeVisitor<'_>,
        node: NodeId,
    ) -> Option<NodeId> {
        if self.in_async_generator() {
            let (asterisk_token, expression, loc) = {
                let read = visitor.node(node);
                let data = read
                    .as_yield_expression()
                    .expect("interface conversion: ast.nodeData is not *ast.YieldExpression");
                (data.asterisk_token(), data.expression(), read.range())
            };
            if asterisk_token.is_some() {
                let expression = visitor.visit_node(expression).expect(NIL);
                let factory = visitor.factory_mut();
                let expression_loc = factory.node(expression).range();

                let async_values_result = self.ctx().new_async_values_helper(factory, expression);
                factory.set_node_range(async_values_result, expression_loc);

                let async_delegator_result = self
                    .ctx()
                    .new_async_delegator_helper(factory, async_values_result);
                factory.set_node_range(async_delegator_result, expression_loc);

                let inner_yield = factory.update_yield_expression(
                    node,
                    asterisk_token,
                    Some(async_delegator_result),
                );

                let awaited_yield = self.ctx().new_await_helper(factory, inner_yield);

                let result =
                    factory.new_yield_expression(None /*asteriskToken*/, Some(awaited_yield));
                factory.set_node_range(result, loc);
                self.ctx().set_original(result, node);
                return Some(result);
            }

            let inner_expression = if expression.is_some() {
                visitor.visit_node(expression).expect(NIL)
            } else {
                self.ctx().new_void_zero_expression(visitor.factory_mut())
            };

            let factory = visitor.factory_mut();
            let awaited = self.create_downlevel_await(factory, inner_expression);
            let result = factory.new_yield_expression(None /*asteriskToken*/, Some(awaited));
            factory.set_node_range(result, loc);
            self.ctx().set_original(result, node);
            return Some(result);
        }

        visitor.visit_each_child(Some(node))
    }

    // port: tsc/internal/transformers/estransforms/forawait.go:forawaitTransformer.visitReturnStatement
    fn visit_return_statement(
        &self,
        visitor: &mut NodeVisitor<'_>,
        node: NodeId,
    ) -> Option<NodeId> {
        if self.in_async_generator() {
            let expression = visitor.node(node).expression();
            let expression = if expression.is_some() {
                visitor.visit_node(expression).expect(NIL)
            } else {
                self.ctx().new_void_zero_expression(visitor.factory_mut())
            };
            let factory = visitor.factory_mut();
            let awaited = self.create_downlevel_await(factory, expression);
            return Some(factory.update_return_statement(node, Some(awaited)));
        }

        visitor.visit_each_child(Some(node))
    }

    // port: tsc/internal/transformers/estransforms/forawait.go:forawaitTransformer.visitLabeledStatement
    fn visit_labeled_statement(
        &self,
        visitor: &mut NodeVisitor<'_>,
        node: NodeId,
    ) -> Option<NodeId> {
        if self.function_flags() & function_flags::ASYNC != 0 {
            let statement = unwrap_innermost_statement_of_label(visitor.factory(), node);
            let is_for_await = {
                let read = visitor.node(statement);
                read.kind() == K::ForOfStatement
                    && read
                        .as_for_in_or_of_statement()
                        .expect("interface conversion: ast.nodeData is not *ast.ForInOrOfStatement")
                        .await_modifier()
                        .is_some()
            };
            if is_for_await {
                return Some(self.visit_for_of_statement(visitor, statement, Some(node)));
            }
            let visited = visitor.visit_node(Some(statement)).expect(NIL);
            return Some(self.ctx().restore_enclosing_label(
                visitor.factory_mut(),
                visited,
                Some(node),
            ));
        }
        visitor.visit_each_child(Some(node))
    }

    // port: tsc/internal/transformers/estransforms/forawait.go:forawaitTransformer.visitSourceFile
    fn visit_source_file(&self, visitor: &mut NodeVisitor<'_>, node: NodeId) -> NodeId {
        let ancestor_facts = self.enter_subtree(
            facts::SOURCE_FILE_EXCLUDES,
            facts::STRICT_MODE_SOURCE_FILE_INCLUDES,
        );
        self.exported_variable_statement.set(false);
        let visited = visitor.visit_each_child(Some(node)).expect(NIL);
        let mut ctx = self.ctx();
        let helpers = ctx.read_emit_helpers();
        ctx.add_emit_helper(visited, &helpers);
        self.exit_subtree(ancestor_facts);
        visited
    }

    /// Visits a ForOfStatement and converts it into a ES2015-compatible ForOfStatement.
    // port: tsc/internal/transformers/estransforms/forawait.go:forawaitTransformer.visitForOfStatement
    fn visit_for_of_statement(
        &self,
        visitor: &mut NodeVisitor<'_>,
        node: NodeId,
        outermost_labeled_statement: Option<NodeId>,
    ) -> NodeId {
        let ancestor_facts = self.enter_subtree(
            facts::ITERATION_STATEMENT_EXCLUDES,
            facts::ITERATION_STATEMENT_INCLUDES,
        );
        let await_modifier = visitor
            .node(node)
            .as_for_in_or_of_statement()
            .expect("interface conversion: ast.nodeData is not *ast.ForInOrOfStatement")
            .await_modifier();
        let result = if await_modifier.is_some() {
            self.transform_for_await_of_statement(
                visitor,
                node,
                outermost_labeled_statement,
                ancestor_facts,
            )
        } else {
            let visited = visitor.visit_each_child(Some(node)).expect(NIL);
            self.ctx().restore_enclosing_label(
                visitor.factory_mut(),
                visited,
                outermost_labeled_statement,
            )
        };
        self.exit_subtree(ancestor_facts);
        result
    }

    // port: tsc/internal/transformers/estransforms/forawait.go:forawaitTransformer.convertForOfStatementHead
    fn convert_for_of_statement_head(
        &self,
        visitor: &mut NodeVisitor<'_>,
        node: NodeId,
        bound_value: NodeId,
        non_user_code: NodeId,
    ) -> NodeId {
        let (node_expression, node_initializer, node_statement) = {
            let read = visitor.node(node);
            let data = read
                .as_for_in_or_of_statement()
                .expect("interface conversion: ast.nodeData is not *ast.ForInOrOfStatement");
            (
                data.expression().expect(NIL),
                data.initializer(),
                data.statement(),
            )
        };
        let mut ctx = self.ctx();
        let factory = visitor.factory_mut();
        let expression_loc = factory.node(node_expression).range();
        let value = ctx.new_temp_variable(factory);
        ctx.add_variable_declaration(factory, value);
        let iterator_value_expression = ctx.new_assignment_expression(factory, value, bound_value);
        let iterator_value_statement =
            factory.new_expression_statement(Some(iterator_value_expression));
        ctx.set_source_map_range(iterator_value_statement, expression_loc);

        let false_keyword = factory.new_keyword_expression(K::FalseKeyword.into());
        let exit_non_user_code_expression =
            ctx.new_assignment_expression(factory, non_user_code, false_keyword);
        let exit_non_user_code_statement =
            factory.new_expression_statement(Some(exit_non_user_code_expression));
        ctx.set_source_map_range(exit_non_user_code_statement, expression_loc);

        let mut statements = vec![
            Some(iterator_value_statement),
            Some(exit_non_user_code_statement),
        ];
        let binding =
            ctx.create_for_of_binding_statement(factory, node_initializer.expect(NIL), value);
        statements.push(visitor.visit_node(Some(binding)));

        let mut body_location = TextRange::new(0, 0);
        let mut statements_location = TextRange::new(0, 0);
        let statement = visitor.visit_embedded_statement(node_statement).expect(NIL);
        let factory = visitor.factory_mut();
        if factory.node(statement).kind() == K::Block {
            let statement_list = factory.node(statement).statement_list();
            statements.extend(list_nodes(factory, statement_list));
            body_location = factory.node(statement).range();
            statements_location = factory.read_list(statement_list.expect(NIL)).loc();
        } else {
            statements.push(Some(statement));
        }

        let stmt_list = new_node_list(factory, statements);
        factory.set_list_location(stmt_list, statements_location);
        let block = factory.new_block(Some(stmt_list), true);
        factory.set_node_range(block, body_location);
        block
    }

    // port: tsc/internal/transformers/estransforms/forawait.go:forawaitTransformer.createDownlevelAwait
    fn create_downlevel_await(
        &self,
        factory: &mut dyn RuntimeFactory,
        expression: NodeId,
    ) -> NodeId {
        if self.function_flags() & function_flags::GENERATOR != 0 {
            let awaited = self.ctx().new_await_helper(factory, expression);
            return factory.new_yield_expression(None /*asteriskToken*/, Some(awaited));
        }
        factory.new_await_expression(Some(expression))
    }

    #[allow(clippy::too_many_lines)] // One pinned function, kept in its node creation order.
                                     // port: tsc/internal/transformers/estransforms/forawait.go:forawaitTransformer.transformForAwaitOfStatement
    fn transform_for_await_of_statement(
        &self,
        visitor: &mut NodeVisitor<'_>,
        node: NodeId,
        outermost_labeled_statement: Option<NodeId>,
        ancestor_facts: u32,
    ) -> NodeId {
        let (node_expression, node_loc) = {
            let read = visitor.node(node);
            (read.expression(), read.range())
        };
        let expression = visitor.visit_node(node_expression).expect(NIL);
        let mut ctx = self.ctx();
        let f = visitor.factory_mut();
        let is_identifier = f.node(expression).kind() == K::Identifier;

        let iterator = if is_identifier {
            ctx.new_generated_name_for_node(f, expression)
        } else {
            ctx.new_temp_variable(f)
        };

        let result = if is_identifier {
            ctx.new_generated_name_for_node(f, iterator)
        } else {
            ctx.new_temp_variable(f)
        };

        let non_user_code = ctx.new_temp_variable(f);
        let done = ctx.new_temp_variable(f);
        ctx.add_variable_declaration(f, done);
        let error_record = ctx.new_unique_name(f, JsString::from_bytes(&b"e"[..]));
        let catch_variable = ctx.new_generated_name_for_node(f, error_record);
        let return_method = ctx.new_temp_variable(f);
        let call_values = ctx.new_async_values_helper(f, expression);
        let expression_loc = f.node(node_expression.expect(NIL)).range();
        f.set_node_range(call_values, expression_loc);
        let next = f.new_identifier(JsString::from_bytes(&b"next"[..]));
        let next_access =
            f.new_property_access_expression(Some(iterator), None, Some(next), node_flags::NONE);
        let no_arguments = new_node_list(f, Vec::new());
        let call_next = f.new_call_expression(
            Some(next_access),
            None,
            None,
            Some(no_arguments),
            node_flags::NONE,
        );
        let done_name = f.new_identifier(JsString::from_bytes(&b"done"[..]));
        let get_done =
            f.new_property_access_expression(Some(result), None, Some(done_name), node_flags::NONE);
        let value_name = f.new_identifier(JsString::from_bytes(&b"value"[..]));
        let get_value = f.new_property_access_expression(
            Some(result),
            None,
            Some(value_name),
            node_flags::NONE,
        );
        let call_return = ctx.new_function_call_call(f, return_method, Some(iterator), Vec::new());

        ctx.add_variable_declaration(f, error_record);
        ctx.add_variable_declaration(f, return_method);

        // if we are enclosed in an outer loop ensure we reset 'errorRecord' per each iteration
        let initializer = if ancestor_facts & facts::ITERATION_CONTAINER != 0 {
            let void_zero = ctx.new_void_zero_expression(f);
            let reset = ctx.new_assignment_expression(f, error_record, void_zero);
            ctx.inline_expressions(f, &[reset, call_values])
                .expect("two expressions inline to one")
        } else {
            call_values
        };

        // Build the for statement
        let iterator_decl =
            f.new_variable_declaration(Some(iterator), None, None, Some(initializer));
        f.set_node_range(iterator_decl, expression_loc);
        let true_keyword = f.new_keyword_expression(K::TrueKeyword.into());
        let non_user_code_decl =
            f.new_variable_declaration(Some(non_user_code), None, None, Some(true_keyword));
        let result_decl = f.new_variable_declaration(Some(result), None, None, None);
        let declarations = new_node_list(
            f,
            vec![
                Some(non_user_code_decl),
                Some(iterator_decl),
                Some(result_decl),
            ],
        );
        let var_decl_list = f.new_variable_declaration_list(Some(declarations), node_flags::NONE);
        f.set_node_range(var_decl_list, expression_loc);

        let awaited_next = self.create_downlevel_await(f, call_next);
        let assign_result = ctx.new_assignment_expression(f, result, awaited_next);
        let assign_done = ctx.new_assignment_expression(f, done, get_done);
        let not_done = f.new_prefix_unary_expression(K::ExclamationToken.into(), Some(done));
        let condition = ctx
            .inline_expressions(f, &[assign_result, assign_done, not_done])
            .expect("three expressions inline to one");

        let true_keyword = f.new_keyword_expression(K::TrueKeyword.into());
        let incrementor = ctx.new_assignment_expression(f, non_user_code, true_keyword);

        let body = self.convert_for_of_statement_head(visitor, node, get_value, non_user_code);
        let f = visitor.factory_mut();
        let for_statement = f.new_for_statement(
            Some(var_decl_list),
            Some(condition),
            Some(incrementor),
            Some(body),
        );
        f.set_node_range(for_statement, node_loc);
        ctx.add_emit_flags(for_statement, emit_flags::NO_TOKEN_TRAILING_SOURCE_MAPS);
        ctx.set_original(for_statement, node);

        // Build the try/catch/finally
        let labeled = ctx.restore_enclosing_label(f, for_statement, outermost_labeled_statement);
        let try_statements = new_node_list(f, vec![Some(labeled)]);
        let try_block = f.new_block(Some(try_statements), true);

        // catch clause: { e_1 = { error: e_2 }; }
        let error_name = f.new_identifier(JsString::from_bytes(&b"error"[..]));
        let error_property =
            f.new_property_assignment(None, Some(error_name), None, None, Some(catch_variable));
        let properties = new_node_list(f, vec![Some(error_property)]);
        let error_object = f.new_object_literal_expression(Some(properties), false);
        let record_error = ctx.new_assignment_expression(f, error_record, error_object);
        let record_statement = f.new_expression_statement(Some(record_error));
        let catch_statements = new_node_list(f, vec![Some(record_statement)]);
        let catch_body = f.new_block(Some(catch_statements), false);
        ctx.add_emit_flags(catch_body, emit_flags::SINGLE_LINE);
        let catch_declaration = f.new_variable_declaration(Some(catch_variable), None, None, None);
        let catch_clause = f.new_catch_clause(Some(catch_declaration), Some(catch_body));

        // finally block
        // inner try: if (!nonUserCode && !done && (returnMethod = iterator.return)) await returnMethod.call(iterator);
        let not_non_user_code =
            f.new_prefix_unary_expression(K::ExclamationToken.into(), Some(non_user_code));
        let and_token = f.new_token(K::AmpersandAmpersandToken.into());
        let not_done = f.new_prefix_unary_expression(K::ExclamationToken.into(), Some(done));
        let left = f.new_binary_expression(
            None,
            Some(not_non_user_code),
            None,
            Some(and_token),
            Some(not_done),
        );
        let and_token = f.new_token(K::AmpersandAmpersandToken.into());
        let return_name = f.new_identifier(JsString::from_bytes(&b"return"[..]));
        let get_return = f.new_property_access_expression(
            Some(iterator),
            None,
            Some(return_name),
            node_flags::NONE,
        );
        let assign_return = ctx.new_assignment_expression(f, return_method, get_return);
        let inner_if_condition =
            f.new_binary_expression(None, Some(left), None, Some(and_token), Some(assign_return));
        let awaited_return = self.create_downlevel_await(f, call_return);
        let await_return_statement = f.new_expression_statement(Some(awaited_return));
        let inner_if_statement =
            f.new_if_statement(Some(inner_if_condition), Some(await_return_statement), None);
        ctx.add_emit_flags(inner_if_statement, emit_flags::SINGLE_LINE);

        let inner_try_statements = new_node_list(f, vec![Some(inner_if_statement)]);
        let inner_try_block = f.new_block(Some(inner_try_statements), false);

        // inner finally: if (errorRecord) throw errorRecord.error;
        let error_name = f.new_identifier(JsString::from_bytes(&b"error"[..]));
        let get_error = f.new_property_access_expression(
            Some(error_record),
            None,
            Some(error_name),
            node_flags::NONE,
        );
        let throw_error = f.new_throw_statement(Some(get_error));
        let inner_finally_if = f.new_if_statement(Some(error_record), Some(throw_error), None);
        ctx.add_emit_flags(inner_finally_if, emit_flags::SINGLE_LINE);
        let inner_finally_statements = new_node_list(f, vec![Some(inner_finally_if)]);
        let inner_finally_block = f.new_block(Some(inner_finally_statements), false);
        ctx.add_emit_flags(inner_finally_block, emit_flags::SINGLE_LINE);

        let inner_try_statement =
            f.new_try_statement(Some(inner_try_block), None, Some(inner_finally_block));
        let finally_statements = new_node_list(f, vec![Some(inner_try_statement)]);
        let finally_block = f.new_block(Some(finally_statements), true);

        f.new_try_statement(Some(try_block), Some(catch_clause), Some(finally_block))
    }

    /// `ast.GetFunctionFlags(node)`, or `None` after recording a failed read.
    fn get_function_flags(&self, visitor: &NodeVisitor<'_>, node: NodeId) -> Option<u32> {
        self.failure
            .ok(get_function_flags(ast_view(visitor.factory()), Some(node)))
    }

    // port: tsc/internal/transformers/estransforms/forawait.go:forawaitTransformer.visitConstructorDeclaration
    fn visit_constructor_declaration(&self, visitor: &mut NodeVisitor<'_>, node: NodeId) -> NodeId {
        let Some(flags) = self.get_function_flags(visitor, node) else {
            return node;
        };
        let (modifiers, parameters, body) = {
            let read = visitor.node(node);
            (read.modifiers(), read.parameter_list(), read.body())
        };
        let saved_enclosing_function_flags = self.enclosing_function_flags.replace(flags);
        let parameters = self.ctx().visit_parameters(parameters, visitor);
        let body = self.ctx().visit_function_body(body, visitor);
        let updated = visitor.factory_mut().update_constructor_declaration(
            node, modifiers, None, /*typeParameters*/
            parameters, None, /*returnType*/
            None, /*fullSignature*/
            body,
        );
        self.enclosing_function_flags
            .set(saved_enclosing_function_flags);
        updated
    }

    // port: tsc/internal/transformers/estransforms/forawait.go:forawaitTransformer.visitGetAccessorDeclaration
    fn visit_get_accessor_declaration(
        &self,
        visitor: &mut NodeVisitor<'_>,
        node: NodeId,
    ) -> NodeId {
        let Some(flags) = self.get_function_flags(visitor, node) else {
            return node;
        };
        let (modifiers, name, parameters, body) = {
            let read = visitor.node(node);
            (
                read.modifiers(),
                read.name(),
                read.parameter_list(),
                read.body(),
            )
        };
        let saved_enclosing_function_flags = self.enclosing_function_flags.replace(flags);
        let name = visitor.visit_node(name);
        let parameters = self.ctx().visit_parameters(parameters, visitor);
        let body = self.ctx().visit_function_body(body, visitor);
        let updated = visitor.factory_mut().update_get_accessor_declaration(
            node, modifiers, name, None, /*typeParameters*/
            parameters, None, /*returnType*/
            None, /*fullSignature*/
            body,
        );
        self.enclosing_function_flags
            .set(saved_enclosing_function_flags);
        updated
    }

    // port: tsc/internal/transformers/estransforms/forawait.go:forawaitTransformer.visitSetAccessorDeclaration
    fn visit_set_accessor_declaration(
        &self,
        visitor: &mut NodeVisitor<'_>,
        node: NodeId,
    ) -> NodeId {
        let Some(flags) = self.get_function_flags(visitor, node) else {
            return node;
        };
        let (modifiers, name, parameters, body) = {
            let read = visitor.node(node);
            (
                read.modifiers(),
                read.name(),
                read.parameter_list(),
                read.body(),
            )
        };
        let saved_enclosing_function_flags = self.enclosing_function_flags.replace(flags);
        let name = visitor.visit_node(name);
        let parameters = self.ctx().visit_parameters(parameters, visitor);
        let body = self.ctx().visit_function_body(body, visitor);
        let updated = visitor.factory_mut().update_set_accessor_declaration(
            node, modifiers, name, None, /*typeParameters*/
            parameters, None, /*returnType*/
            None, /*fullSignature*/
            body,
        );
        self.enclosing_function_flags
            .set(saved_enclosing_function_flags);
        updated
    }

    /// The modifiers, asterisk, parameters and body of a function-like
    /// declaration that may be an async generator, in upstream's order: the
    /// shared part of `visitMethodDeclaration`, `visitFunctionDeclaration`
    /// and `visitFunctionExpression`.
    fn visit_generator_parts(
        &self,
        visitor: &mut NodeVisitor<'_>,
        node: NodeId,
        asterisk_token: Option<NodeId>,
    ) -> (
        Option<NodeListId>,
        Option<NodeId>,
        Option<NodeListId>,
        Option<NodeId>,
    ) {
        let flags = self.function_flags();
        let (modifiers, parameters, body) = {
            let read = visitor.node(node);
            (read.modifiers(), read.parameter_list(), read.body())
        };

        let modifiers = if flags & function_flags::GENERATOR != 0 {
            self.visit_modifiers_no_async(visitor, modifiers)
        } else {
            modifiers
        };

        let asterisk_token = if flags & function_flags::ASYNC != 0 {
            None
        } else {
            asterisk_token
        };

        let (parameters, body) = if self.in_async_generator() {
            let parameters = self.transform_async_generator_function_parameter_list(visitor, node);
            let body = self.transform_async_generator_function_body(visitor, node);
            (parameters, Some(body))
        } else {
            let parameters = self.ctx().visit_parameters(parameters, visitor);
            let body = self.ctx().visit_function_body(body, visitor);
            (parameters, body)
        };
        (modifiers, asterisk_token, parameters, body)
    }

    // port: tsc/internal/transformers/estransforms/forawait.go:forawaitTransformer.visitMethodDeclaration
    fn visit_method_declaration(&self, visitor: &mut NodeVisitor<'_>, node: NodeId) -> NodeId {
        let Some(flags) = self.get_function_flags(visitor, node) else {
            return node;
        };
        let (asterisk_token, name) = {
            let read = visitor.node(node);
            let data = read
                .as_method_declaration()
                .expect("interface conversion: ast.nodeData is not *ast.MethodDeclaration");
            (data.asterisk_token(), data.name())
        };
        let saved_enclosing_function_flags = self.enclosing_function_flags.replace(flags);
        let (modifiers, asterisk_token, parameters, body) =
            self.visit_generator_parts(visitor, node, asterisk_token);
        let name = visitor.visit_node(name);
        let updated = visitor.factory_mut().update_method_declaration(
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
        self.enclosing_function_flags
            .set(saved_enclosing_function_flags);
        updated
    }

    // port: tsc/internal/transformers/estransforms/forawait.go:forawaitTransformer.visitFunctionDeclaration
    fn visit_function_declaration(&self, visitor: &mut NodeVisitor<'_>, node: NodeId) -> NodeId {
        let Some(flags) = self.get_function_flags(visitor, node) else {
            return node;
        };
        let (asterisk_token, name) = {
            let read = visitor.node(node);
            let data = read
                .as_function_declaration()
                .expect("interface conversion: ast.nodeData is not *ast.FunctionDeclaration");
            (data.asterisk_token(), data.name())
        };
        let saved_enclosing_function_flags = self.enclosing_function_flags.replace(flags);
        let (modifiers, asterisk_token, parameters, body) =
            self.visit_generator_parts(visitor, node, asterisk_token);
        let updated = visitor.factory_mut().update_function_declaration(
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
        self.enclosing_function_flags
            .set(saved_enclosing_function_flags);
        updated
    }

    // port: tsc/internal/transformers/estransforms/forawait.go:forawaitTransformer.visitArrowFunction
    fn visit_arrow_function(&self, visitor: &mut NodeVisitor<'_>, node: NodeId) -> NodeId {
        let Some(flags) = self.get_function_flags(visitor, node) else {
            return node;
        };
        let (modifiers, parameters, equals_greater_than_token, body) = {
            let read = visitor.node(node);
            let data = read
                .as_arrow_function()
                .expect("interface conversion: ast.nodeData is not *ast.ArrowFunction");
            (
                data.modifiers(),
                data.parameters(),
                data.equals_greater_than_token(),
                data.body(),
            )
        };
        let saved_enclosing_function_flags = self.enclosing_function_flags.replace(flags);
        let parameters = self.ctx().visit_parameters(parameters, visitor);
        let body = self.ctx().visit_function_body(body, visitor);
        let updated = visitor.factory_mut().update_arrow_function(
            node,
            modifiers,
            None, /*typeParameters*/
            parameters,
            None, /*returnType*/
            None, /*fullSignature*/
            equals_greater_than_token,
            body,
        );
        self.enclosing_function_flags
            .set(saved_enclosing_function_flags);
        updated
    }

    // port: tsc/internal/transformers/estransforms/forawait.go:forawaitTransformer.visitFunctionExpression
    fn visit_function_expression(&self, visitor: &mut NodeVisitor<'_>, node: NodeId) -> NodeId {
        let Some(flags) = self.get_function_flags(visitor, node) else {
            return node;
        };
        let (asterisk_token, name) = {
            let read = visitor.node(node);
            let data = read
                .as_function_expression()
                .expect("interface conversion: ast.nodeData is not *ast.FunctionExpression");
            (data.asterisk_token(), data.name())
        };
        let saved_enclosing_function_flags = self.enclosing_function_flags.replace(flags);
        let (modifiers, asterisk_token, parameters, body) =
            self.visit_generator_parts(visitor, node, asterisk_token);
        let updated = visitor.factory_mut().update_function_expression(
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
        self.enclosing_function_flags
            .set(saved_enclosing_function_flags);
        updated
    }

    // port: tsc/internal/transformers/estransforms/forawait.go:forawaitTransformer.transformAsyncGeneratorFunctionParameterList
    fn transform_async_generator_function_parameter_list(
        &self,
        visitor: &mut NodeVisitor<'_>,
        node: NodeId,
    ) -> Option<NodeListId> {
        let parameter_list = visitor.node(node).parameter_list();
        let parameters = list_nodes(visitor.factory(), parameter_list);
        if is_simple_parameter_list(visitor.factory(), &parameters) {
            return self.ctx().visit_parameters(parameter_list, visitor);
        }
        // Add fixed parameters to preserve the function's `length` property.
        let mut new_parameters = Vec::new();
        let mut ctx = self.ctx();
        let factory = visitor.factory_mut();
        for parameter in parameters {
            let (initializer, dot_dot_dot_token, name) = {
                let read = factory.node(parameter.expect(NIL));
                let data = read
                    .as_parameter_declaration()
                    .expect("interface conversion: ast.nodeData is not *ast.ParameterDeclaration");
                (data.initializer(), data.dot_dot_dot_token(), data.name())
            };
            if initializer.is_some() || dot_dot_dot_token.is_some() {
                break;
            }
            let generated = ctx.new_generated_name_for_node_ex(
                factory,
                name.expect(NIL),
                AutoGenerateOptions {
                    flags: g::RESERVED_IN_NESTED_SCOPES,
                    ..AutoGenerateOptions::default()
                },
            );
            let new_parameter =
                factory.new_parameter_declaration(None, None, Some(generated), None, None, None);
            new_parameters.push(Some(new_parameter));
        }
        let new_parameters_array = new_node_list(factory, new_parameters);
        let loc = factory.read_list(parameter_list.expect(NIL)).loc();
        factory.set_list_location(new_parameters_array, loc);
        Some(new_parameters_array)
    }

    #[allow(clippy::too_many_lines)] // One pinned function, kept in its node creation order.
                                     // port: tsc/internal/transformers/estransforms/forawait.go:forawaitTransformer.transformAsyncGeneratorFunctionBody
    fn transform_async_generator_function_body(
        &self,
        visitor: &mut NodeVisitor<'_>,
        node: NodeId,
    ) -> NodeId {
        let (parameter_list, body, node_name) = {
            let read = visitor.node(node);
            (read.parameter_list(), read.body().expect(NIL), read.name())
        };
        let (body_statements, body_multi_line) = block_parts(visitor.factory(), body);
        let parameters = list_nodes(visitor.factory(), parameter_list);
        let inner_parameters = if is_simple_parameter_list(visitor.factory(), &parameters) {
            None
        } else {
            self.ctx().visit_parameters(parameter_list, visitor)
        };

        let state = &self.super_access;
        let saved_captured_super_properties = state
            .captured_super_properties
            .replace(Some(OrderedSet::default()));
        let saved_has_super_element_access = state.has_super_element_access.replace(false);
        let saved_has_super_property_assignment =
            state.has_super_property_assignment.replace(false);
        let mut ctx = self.ctx();
        let options = AutoGenerateOptions {
            flags: g::OPTIMISTIC | g::FILE_LEVEL,
            ..AutoGenerateOptions::default()
        };
        let super_binding = ctx.new_unique_name_ex(
            visitor.factory_mut(),
            JsString::from_bytes(&b"_super"[..]),
            options.clone(),
        );
        let saved_super_binding = state.super_binding.replace(Some(super_binding));
        let super_index_binding = ctx.new_unique_name_ex(
            visitor.factory_mut(),
            JsString::from_bytes(&b"_superIndex"[..]),
            options,
        );
        let saved_super_index_binding =
            state.super_index_binding.replace(Some(super_index_binding));

        let visited_statements = visitor.visit_nodes(body_statements);
        let factory = visitor.factory_mut();
        let mut async_body = factory.update_block(body, visited_statements, body_multi_line);
        let (async_statements, async_multi_line) = block_parts(factory, async_body);
        let merged = ctx.end_and_merge_variable_environment_list(factory, async_statements);
        async_body = factory.update_block(async_body, merged, async_multi_line);

        // Substitute super property accesses with _super/_superIndex helpers
        let emit_super_helpers = captured_size(state) > 0 || state.has_super_element_access.get();
        if emit_super_helpers {
            async_body = state
                .substitute_super_accesses_in_body(factory, async_body)
                .expect(NIL);
        }

        let inner_params = match inner_parameters {
            Some(inner_parameters) => inner_parameters,
            None => new_node_list(factory, Vec::new()),
        };

        let name = node_name.map(|name| ctx.new_generated_name_for_node(factory, name));

        let asterisk = factory.new_token(K::AsteriskToken.into());
        let generator_func = factory.new_function_expression(
            None, /*modifiers*/
            Some(asterisk),
            name,
            None, /*typeParameters*/
            Some(inner_params),
            None, /*returnType*/
            None, /*fullSignature*/
            Some(async_body),
        );

        let async_generator = ctx.new_async_generator_helper(
            factory,
            generator_func,
            self.for_await_hierarchy_facts.get() & facts::HAS_LEXICAL_THIS != 0,
        );
        let return_statement = factory.new_return_statement(Some(async_generator));

        ctx.start_variable_environment();
        if emit_super_helpers && captured_size(state) > 0 {
            let statement = state.create_super_access_variable_statement(factory);
            ctx.add_initialization_statement(statement);
        }

        let outer_statements = new_node_list(factory, vec![Some(return_statement)]);

        let merged = ctx.end_and_merge_variable_environment_list(factory, Some(outer_statements));
        let block = factory.update_block(body, merged, body_multi_line);

        if emit_super_helpers && state.has_super_element_access.get() {
            if state.has_super_property_assignment.get() {
                ctx.add_emit_helper(block, &[&emit_helpers::ADVANCED_ASYNC_SUPER_HELPER]);
            } else {
                ctx.add_emit_helper(block, &[&emit_helpers::ASYNC_SUPER_HELPER]);
            }
        }

        state
            .captured_super_properties
            .replace(saved_captured_super_properties);
        state
            .has_super_element_access
            .set(saved_has_super_element_access);
        state
            .has_super_property_assignment
            .set(saved_has_super_property_assignment);
        state.super_binding.set(saved_super_binding);
        state.super_index_binding.set(saved_super_index_binding);

        block
    }
}

/// Follows LabeledStatement chains to find the innermost statement.
// port: tsc/internal/transformers/estransforms/forawait.go:unwrapInnermostStatementOfLabel
fn unwrap_innermost_statement_of_label(factory: &dyn RuntimeFactory, mut node: NodeId) -> NodeId {
    loop {
        let statement = factory.node(node).statement().expect(NIL);
        if factory.node(statement).kind() != K::LabeledStatement {
            return statement;
        }
        node = statement;
    }
}

/// A block's statement list and its `MultiLine` flag.
fn block_parts(factory: &dyn RuntimeFactory, block: NodeId) -> (Option<NodeListId>, bool) {
    let read = factory.node(block);
    let data = read
        .as_block()
        .expect("interface conversion: ast.nodeData is not *ast.Block");
    (data.statements(), data.multi_line())
}

/// `capturedSuperProperties.Size()`.
fn captured_size(state: &SuperAccessState) -> usize {
    state
        .captured_super_properties
        .borrow()
        .as_ref()
        .expect(NIL)
        .len()
}

/// Whether every parameter has no initializer and an Identifier name.
// TODO(a8): estransforms.isSimpleParameterList
fn is_simple_parameter_list(factory: &dyn RuntimeFactory, parameters: &[Option<NodeId>]) -> bool {
    parameters.iter().all(|parameter| {
        let read = factory.node(parameter.expect(NIL));
        let data = read
            .as_parameter_declaration()
            .expect("interface conversion: ast.nodeData is not *ast.ParameterDeclaration");
        data.initializer().is_none()
            && factory.node(data.name().expect(NIL)).kind() == K::Identifier
    })
}
