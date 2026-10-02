//! `transformers/estransforms/objectrestspread.go`: lowers object spread in
//! literals to `Object.assign` and object rest in bindings, parameters,
//! assignments, `for-of` heads and catch clauses to `__rest` for targets
//! before ES2018.
//!
//! A failed storage read records the failure and is read as "no facts": the
//! visit stops at the next node and the chain reports the failure, so no
//! output built from that answer escapes.
use super::utilities::{
    has_syntactic_modifier, list_nodes, new_node_list, skip_parentheses, subtree_facts, view, NIL,
};
use crate::destructuring::{
    flatten_destructuring_assignment, flatten_destructuring_binding, FlattenLevel,
};
use crate::transformer::{Error, Failure, TransformOptions, Transformer};
use std::cell::{Cell, RefCell};
use std::collections::HashSet;
use std::rc::Rc;
use std::sync::Arc;
use tsr_ast::subtree_flags::{self, ES_OBJECT_REST_OR_SPREAD, OBJECT_REST_OR_SPREAD};
use tsr_ast::utilities::is_binding_pattern;
use tsr_ast::{
    modifier_flags, node_flags, Factory, FactoryMethods, NodeId, NodeListId, NodeVisitor,
    RuntimeFactory, SyntaxKind as K,
};
use tsr_core::{CompilerOptions, TextRange};
use tsr_printer::{emit_flags, EmitContext};

/// `objectRestSpreadTransformer`.
struct ObjectRestSpreadTransformer {
    emit_context: EmitContext,
    failure: Failure,
    compiler_options: Arc<CompilerOptions>,

    in_exported_variable_statement: Cell<bool>,
    expression_result_is_unused: Cell<bool>,

    /// Upstream's nilable set: `None` is its nil map, which turns the
    /// subtree-facts shortcut of `visit` back on.
    parameters_with_preceding_object_rest_or_spread: RefCell<Option<HashSet<NodeId>>>,
}

/// `oldParamScope`: the set of the enclosing parameter list.
type OldParamScope = Option<HashSet<NodeId>>;

// port: tsc/internal/transformers/estransforms/objectrestspread.go:newObjectRestSpreadTransformer
pub fn new_object_rest_spread_transformer<'a>(
    opts: &TransformOptions<'a>,
) -> Option<Transformer<'a>> {
    let tx = Rc::new(ObjectRestSpreadTransformer {
        emit_context: opts.context.clone(),
        failure: opts.failure.clone(),
        compiler_options: opts.compiler_options.clone(),
        in_exported_variable_statement: Cell::new(false),
        expression_result_is_unused: Cell::new(false),
        parameters_with_preceding_object_rest_or_spread: RefCell::new(None),
    });
    Some(Transformer::new(
        move |visitor: &mut NodeVisitor<'_>, node: Option<NodeId>| tx.visit(visitor, node),
        Some(opts.context.clone()),
        opts.failure.clone(),
    ))
}

/// The children of a `SyntaxList` result of the flattener, or the result
/// itself.
fn declarations_of(factory: &dyn RuntimeFactory, declarations: NodeId) -> Vec<NodeId> {
    let read = factory.node(declarations);
    if read.kind() == K::SyntaxList {
        let children = read
            .as_syntax_list()
            .expect("interface conversion: ast.nodeData is not *ast.SyntaxList")
            .children();
        drop(read);
        return factory
            .read_nodes(children)
            .iter()
            .map(|node| node.expect(NIL))
            .collect();
    }
    vec![declarations]
}

/// A block's statement list and its `MultiLine` flag.
fn block_parts(factory: &dyn RuntimeFactory, block: NodeId) -> (Option<NodeListId>, bool) {
    let read = factory.node(block);
    let data = read
        .as_block()
        .expect("interface conversion: ast.nodeData is not *ast.Block");
    (data.statements(), data.multi_line())
}

/// `ast.IsAssignmentPattern`.
fn is_assignment_pattern(factory: &dyn RuntimeFactory, node: NodeId) -> bool {
    matches!(
        factory.node(node).kind().known(),
        Some(K::ArrayLiteralExpression | K::ObjectLiteralExpression)
    )
}

impl ObjectRestSpreadTransformer {
    /// The emit context handle, for its `&mut self` operations.
    fn ctx(&self) -> EmitContext {
        self.emit_context.clone()
    }

    /// `node.SubtreeFacts()`; a failed read is recorded and has no facts.
    fn facts(&self, factory: &dyn RuntimeFactory, node: NodeId) -> u32 {
        self.failure
            .ok(subtree_facts(factory, node))
            .unwrap_or(subtree_flags::NONE)
    }

    /// `ast.ContainsObjectRestOrSpread`; a failed read is recorded.
    fn contains_object_rest_or_spread(&self, factory: &dyn RuntimeFactory, node: NodeId) -> bool {
        self.failure
            .ok(view(factory))
            .is_some_and(|view| view.contains_object_rest_or_spread(node))
    }

    // port: tsc/internal/transformers/estransforms/objectrestspread.go:objectRestSpreadTransformer.visit
    fn visit(&self, visitor: &mut NodeVisitor<'_>, node: Option<NodeId>) -> Option<NodeId> {
        if self.failure.is_set() {
            return node;
        }
        let id = node.expect(NIL);
        if self.facts(visitor.factory(), id) & ES_OBJECT_REST_OR_SPREAD == 0
            && self
                .parameters_with_preceding_object_rest_or_spread
                .borrow()
                .is_none()
        {
            return node;
        }
        // Save the expressionResultIsUnused flag set by the parent for this node,
        // then reset to false for children (the default). Specific cases below override as needed.
        let expression_result_is_unused = self.expression_result_is_unused.replace(false);
        let result = match visitor.node(id).kind().known() {
            Some(K::SourceFile) => Some(self.visit_source_file(visitor, id)),
            Some(K::ObjectLiteralExpression) => self.visit_object_literal_expression(visitor, id),
            Some(K::BinaryExpression) => {
                self.visit_binary_expression(visitor, id, expression_result_is_unused)
            }
            Some(K::ExpressionStatement) => {
                self.expression_result_is_unused.set(true);
                visitor.visit_each_child(node)
            }
            Some(K::ParenthesizedExpression) => {
                self.expression_result_is_unused
                    .set(expression_result_is_unused);
                visitor.visit_each_child(node)
            }
            Some(K::ForOfStatement) => self.visit_for_of_statement(visitor, id),
            Some(K::VariableStatement) => self.visit_variable_statement(visitor, id),
            Some(K::VariableDeclaration) => self.visit_variable_declaration(visitor, id),
            Some(K::CatchClause) => self.visit_catch_clause(visitor, id),
            Some(K::Parameter) => self.visit_parameter(visitor, id),
            Some(K::Constructor) => Some(self.visit_constructor_declaration(visitor, id)),
            Some(K::GetAccessor) => Some(self.visit_get_accessor_declaration(visitor, id)),
            Some(K::SetAccessor) => Some(self.visit_set_accessor_declaration(visitor, id)),
            Some(K::MethodDeclaration) => Some(self.visit_method_declaration(visitor, id)),
            Some(K::FunctionDeclaration) => Some(self.visit_function_declaration(visitor, id)),
            Some(K::ArrowFunction) => Some(self.visit_arrow_function(visitor, id)),
            Some(K::FunctionExpression) => Some(self.visit_function_expression(visitor, id)),
            _ => visitor.visit_each_child(node),
        };
        self.expression_result_is_unused
            .set(expression_result_is_unused);
        result
    }

    // port: tsc/internal/transformers/estransforms/objectrestspread.go:objectRestSpreadTransformer.visitSourceFile
    fn visit_source_file(&self, visitor: &mut NodeVisitor<'_>, node: NodeId) -> NodeId {
        let visited = visitor.visit_each_child(Some(node)).expect(NIL);
        let mut ctx = self.ctx();
        let helpers = ctx.read_emit_helpers();
        ctx.add_emit_helper(visited, &helpers);
        visited
    }

    // port: tsc/internal/transformers/estransforms/objectrestspread.go:objectRestSpreadTransformer.visitParameter
    fn visit_parameter(&self, visitor: &mut NodeVisitor<'_>, node: NodeId) -> Option<NodeId> {
        let (dot_dot_dot_token, name, initializer) = {
            let read = visitor.node(node);
            let data = read
                .as_parameter_declaration()
                .expect("interface conversion: ast.nodeData is not *ast.ParameterDeclaration");
            (data.dot_dot_dot_token(), data.name(), data.initializer())
        };
        let preceded = self
            .parameters_with_preceding_object_rest_or_spread
            .borrow()
            .as_ref()
            .is_some_and(|parameters| parameters.contains(&node));
        if preceded {
            let mut name = name.expect(NIL);
            if is_binding_pattern(&visitor.node(name)) {
                name = self
                    .ctx()
                    .new_generated_name_for_node(visitor.factory_mut(), node);
            }
            return Some(visitor.factory_mut().update_parameter_declaration(
                node,
                None,
                dot_dot_dot_token,
                Some(name),
                None,
                None,
                None,
            ));
        }
        if self.facts(visitor.factory(), node) & OBJECT_REST_OR_SPREAD != 0 {
            // Binding patterns are converted into a generated name and are
            // evaluated inside the function body.
            let generated = self
                .ctx()
                .new_generated_name_for_node(visitor.factory_mut(), node);
            let initializer = visitor.visit_node(initializer);
            return Some(visitor.factory_mut().update_parameter_declaration(
                node,
                None,
                dot_dot_dot_token,
                Some(generated),
                None,
                None,
                initializer,
            ));
        }
        visitor.visit_each_child(Some(node))
    }

    // port: tsc/internal/transformers/estransforms/objectrestspread.go:objectRestSpreadTransformer.collectParametersWithPrecedingObjectRestOrSpread
    fn collect_parameters_with_preceding_object_rest_or_spread(
        &self,
        factory: &dyn RuntimeFactory,
        node: NodeId,
    ) -> Option<HashSet<NodeId>> {
        let mut result: Option<HashSet<NodeId>> = None;
        for parameter in list_nodes(factory, factory.node(node).parameter_list()) {
            if let Some(result) = result.as_mut() {
                result.insert(parameter);
            } else if self.facts(factory, parameter) & OBJECT_REST_OR_SPREAD != 0 {
                result = Some(HashSet::new());
            }
        }
        result
    }

    // port: tsc/internal/transformers/estransforms/objectrestspread.go:objectRestSpreadTransformer.enterParameterListContext
    fn enter_parameter_list_context(
        &self,
        factory: &dyn RuntimeFactory,
        node: NodeId,
    ) -> OldParamScope {
        let collected = self.collect_parameters_with_preceding_object_rest_or_spread(factory, node);
        self.parameters_with_preceding_object_rest_or_spread
            .replace(collected)
    }

    // port: tsc/internal/transformers/estransforms/objectrestspread.go:objectRestSpreadTransformer.exitParameterListContext
    fn exit_parameter_list_context(&self, scope: OldParamScope) {
        self.parameters_with_preceding_object_rest_or_spread
            .replace(scope);
    }

    // port: tsc/internal/transformers/estransforms/objectrestspread.go:objectRestSpreadTransformer.visitContructorDeclaration
    fn visit_constructor_declaration(&self, visitor: &mut NodeVisitor<'_>, node: NodeId) -> NodeId {
        let old = self.enter_parameter_list_context(visitor.factory(), node);
        let (modifiers, parameters) = {
            let read = visitor.node(node);
            (read.modifiers(), read.parameter_list())
        };
        let parameters = visitor.visit_nodes(parameters);
        let body = self.transform_function_body(visitor, node);
        let updated = visitor.factory_mut().update_constructor_declaration(
            node, modifiers, None, /*typeParameters*/
            parameters, None, /*returnType*/
            None, /*fullSignature*/
            body,
        );
        self.exit_parameter_list_context(old);
        updated
    }

    // port: tsc/internal/transformers/estransforms/objectrestspread.go:objectRestSpreadTransformer.visitGetAccessorDeclaration
    fn visit_get_accessor_declaration(
        &self,
        visitor: &mut NodeVisitor<'_>,
        node: NodeId,
    ) -> NodeId {
        let old = self.enter_parameter_list_context(visitor.factory(), node);
        let (modifiers, name, parameters) = {
            let read = visitor.node(node);
            (read.modifiers(), read.name(), read.parameter_list())
        };
        let name = visitor.visit_node(name);
        let parameters = visitor.visit_nodes(parameters);
        let body = self.transform_function_body(visitor, node);
        let updated = visitor.factory_mut().update_get_accessor_declaration(
            node, modifiers, name, None, /*typeParameters*/
            parameters, None, /*returnType*/
            None, /*fullSignature*/
            body,
        );
        self.exit_parameter_list_context(old);
        updated
    }

    // port: tsc/internal/transformers/estransforms/objectrestspread.go:objectRestSpreadTransformer.visitSetAccessorDeclaration
    fn visit_set_accessor_declaration(
        &self,
        visitor: &mut NodeVisitor<'_>,
        node: NodeId,
    ) -> NodeId {
        let old = self.enter_parameter_list_context(visitor.factory(), node);
        let (modifiers, name, parameters) = {
            let read = visitor.node(node);
            (read.modifiers(), read.name(), read.parameter_list())
        };
        let name = visitor.visit_node(name);
        let parameters = visitor.visit_nodes(parameters);
        let body = self.transform_function_body(visitor, node);
        let updated = visitor.factory_mut().update_set_accessor_declaration(
            node, modifiers, name, None, /*typeParameters*/
            parameters, None, /*returnType*/
            None, /*fullSignature*/
            body,
        );
        self.exit_parameter_list_context(old);
        updated
    }

    // port: tsc/internal/transformers/estransforms/objectrestspread.go:objectRestSpreadTransformer.visitMethodDeclaration
    fn visit_method_declaration(&self, visitor: &mut NodeVisitor<'_>, node: NodeId) -> NodeId {
        let old = self.enter_parameter_list_context(visitor.factory(), node);
        let (modifiers, asterisk_token, name, postfix_token, parameters) = {
            let read = visitor.node(node);
            let data = read
                .as_method_declaration()
                .expect("interface conversion: ast.nodeData is not *ast.MethodDeclaration");
            (
                read.modifiers(),
                data.asterisk_token(),
                read.name(),
                read.postfix_token(),
                read.parameter_list(),
            )
        };
        let name = visitor.visit_node(name);
        let parameters = visitor.visit_nodes(parameters);
        let body = self.transform_function_body(visitor, node);
        let updated = visitor.factory_mut().update_method_declaration(
            node,
            modifiers,
            asterisk_token,
            name,
            postfix_token,
            None, /*typeParameters*/
            parameters,
            None, /*returnType*/
            None, /*fullSignature*/
            body,
        );
        self.exit_parameter_list_context(old);
        updated
    }

    // port: tsc/internal/transformers/estransforms/objectrestspread.go:objectRestSpreadTransformer.visitFunctionDeclaration
    fn visit_function_declaration(&self, visitor: &mut NodeVisitor<'_>, node: NodeId) -> NodeId {
        let old = self.enter_parameter_list_context(visitor.factory(), node);
        let (modifiers, asterisk_token, name, parameters) = {
            let read = visitor.node(node);
            let data = read
                .as_function_declaration()
                .expect("interface conversion: ast.nodeData is not *ast.FunctionDeclaration");
            (
                read.modifiers(),
                data.asterisk_token(),
                read.name(),
                read.parameter_list(),
            )
        };
        let name = visitor.visit_node(name);
        let parameters = visitor.visit_nodes(parameters);
        let body = self.transform_function_body(visitor, node);
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
        self.exit_parameter_list_context(old);
        updated
    }

    // port: tsc/internal/transformers/estransforms/objectrestspread.go:objectRestSpreadTransformer.visitArrowFunction
    fn visit_arrow_function(&self, visitor: &mut NodeVisitor<'_>, node: NodeId) -> NodeId {
        let old = self.enter_parameter_list_context(visitor.factory(), node);
        let (modifiers, parameters, equals_greater_than_token) = {
            let read = visitor.node(node);
            let data = read
                .as_arrow_function()
                .expect("interface conversion: ast.nodeData is not *ast.ArrowFunction");
            (
                read.modifiers(),
                read.parameter_list(),
                data.equals_greater_than_token(),
            )
        };
        let parameters = visitor.visit_nodes(parameters);
        let body = self.transform_function_body(visitor, node);
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
        self.exit_parameter_list_context(old);
        updated
    }

    // port: tsc/internal/transformers/estransforms/objectrestspread.go:objectRestSpreadTransformer.visitFunctionExpression
    fn visit_function_expression(&self, visitor: &mut NodeVisitor<'_>, node: NodeId) -> NodeId {
        let old = self.enter_parameter_list_context(visitor.factory(), node);
        let (modifiers, asterisk_token, name, parameters) = {
            let read = visitor.node(node);
            let data = read
                .as_function_expression()
                .expect("interface conversion: ast.nodeData is not *ast.FunctionExpression");
            (
                read.modifiers(),
                data.asterisk_token(),
                read.name(),
                read.parameter_list(),
            )
        };
        let name = visitor.visit_node(name);
        let parameters = visitor.visit_nodes(parameters);
        let body = self.transform_function_body(visitor, node);
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
        self.exit_parameter_list_context(old);
        updated
    }

    // port: tsc/internal/transformers/estransforms/objectrestspread.go:objectRestSpreadTransformer.transformFunctionBody
    fn transform_function_body(
        &self,
        visitor: &mut NodeVisitor<'_>,
        node: NodeId,
    ) -> Option<NodeId> {
        // EmitContext().VisitFunctionBody is not used here because this transformer needs to inject
        // object rest assignments between visiting the body and merging the variable environment.
        let mut ctx = self.ctx();
        ctx.start_variable_environment();
        let node_body = visitor.node(node).body();
        let body = visitor.visit_node(node_body);
        let extras = ctx.end_variable_environment(visitor.factory_mut());
        ctx.start_variable_environment();
        let new_statements = self.collect_object_rest_assignments(visitor, node);
        let extras = ctx.end_and_merge_variable_environment(visitor.factory_mut(), extras);
        if new_statements.is_empty() && extras.is_empty() {
            return body;
        }

        let f = visitor.factory_mut();
        let mut body = body.unwrap_or_else(|| {
            let statements = new_node_list(f, Vec::new());
            f.new_block(Some(statements), true)
        });
        let mut prefix = Vec::new();
        let mut suffix = Vec::new();
        if f.node(body).kind() == K::Block {
            let mut custom = false;
            let statements = list_nodes(f, f.node(body).statement_list());
            for (i, &statement) in statements.iter().enumerate() {
                if !custom && self.is_prologue_directive(f, statement) {
                    prefix.push(statement);
                } else if ctx.emit_flags(statement) & emit_flags::CUSTOM_PROLOGUE != 0 {
                    custom = true;
                    prefix.push(statement);
                } else {
                    suffix = statements[i..].to_vec();
                    break;
                }
            }
        } else {
            let loc = f.node(body).range();
            let ret = f.new_return_statement(Some(body));
            f.set_node_range(ret, loc);
            let list = new_node_list(f, Vec::new());
            f.set_list_location(list, loc);
            body = f.new_block(Some(list), true);
            suffix.push(ret);
        }

        let mut statements = prefix;
        statements.extend(extras);
        statements.extend(new_statements);
        statements.extend(suffix);
        let new_statement_list = new_node_list(f, statements);
        let (body_statements, multi_line) = block_parts(f, body);
        let loc = f.read_list(body_statements.expect(NIL)).loc();
        f.set_list_location(new_statement_list, loc);
        Some(f.update_block(body, Some(new_statement_list), multi_line))
    }

    /// `ast.IsPrologueDirective`; a failed read is recorded.
    fn is_prologue_directive(&self, factory: &dyn RuntimeFactory, statement: NodeId) -> bool {
        self.failure
            .ok(view(factory).and_then(|view| {
                tsr_ast::utilities::is_prologue_directive(view, statement).map_err(Error::from)
            }))
            .unwrap_or(false)
    }

    /// The variable statement of a flattened parameter, marked as a custom
    /// prologue.
    fn new_custom_prologue_variable_statement(
        &self,
        factory: &mut dyn RuntimeFactory,
        declarations: NodeId,
    ) -> NodeId {
        let decls = declarations_of(factory, declarations);
        let list = new_node_list(factory, decls);
        let declaration_list = factory.new_variable_declaration_list(Some(list), node_flags::NONE);
        let statement = factory.new_variable_statement(None, Some(declaration_list));
        self.ctx()
            .add_emit_flags(statement, emit_flags::CUSTOM_PROLOGUE);
        statement
    }

    // port: tsc/internal/transformers/estransforms/objectrestspread.go:objectRestSpreadTransformer.collectObjectRestAssignments
    fn collect_object_rest_assignments(
        &self,
        visitor: &mut NodeVisitor<'_>,
        node: NodeId,
    ) -> Vec<NodeId> {
        let mut ctx = self.ctx();
        let mut contains_preceding_object_rest_or_spread = false;
        let mut results = Vec::new();
        let parameters = list_nodes(visitor.factory(), visitor.node(node).parameter_list());
        for parameter in parameters {
            if self.failure.is_set() {
                break;
            }
            if contains_preceding_object_rest_or_spread {
                let (name, initializer, parameter_loc) = {
                    let read = visitor.node(parameter);
                    (read.name().expect(NIL), read.initializer(), read.range())
                };
                if is_binding_pattern(&visitor.node(name)) {
                    // In cases where a binding pattern is simply '[]' or '{}',
                    // we usually don't want to emit a var declaration; however, in the presence
                    // of an initializer, we must emit that expression to preserve side effects.
                    let element_list = visitor.node(name).element_list();
                    if !list_nodes(visitor.factory(), element_list).is_empty() {
                        let generated =
                            ctx.new_generated_name_for_node(visitor.factory_mut(), parameter);
                        let declarations = flatten_destructuring_binding(
                            visitor,
                            &ctx,
                            parameter,
                            Some(generated),
                            FlattenLevel::All,
                            false,
                            false,
                        );
                        let Some(declarations) = self.failure.ok(declarations) else {
                            break;
                        };
                        if let Some(declarations) = declarations {
                            let statement = self.new_custom_prologue_variable_statement(
                                visitor.factory_mut(),
                                declarations,
                            );
                            results.push(statement);
                        }
                    } else if initializer.is_some() {
                        let name =
                            ctx.new_generated_name_for_node(visitor.factory_mut(), parameter);
                        let initializer = visitor.visit_node(initializer).expect(NIL);
                        let f = visitor.factory_mut();
                        let assignment = ctx.new_assignment_expression(f, name, initializer);
                        let statement = f.new_expression_statement(Some(assignment));
                        ctx.add_emit_flags(statement, emit_flags::CUSTOM_PROLOGUE);
                        results.push(statement);
                    }
                } else if initializer.is_some() {
                    // Converts a parameter initializer into a function body statement, i.e.:
                    //
                    //  function f(x = 1) { }
                    //
                    // becomes
                    //
                    //  function f(x) {
                    //    if (typeof x === "undefined") { x = 1; }
                    //  }
                    let f = visitor.factory_mut();
                    let name_loc = f.node(name).range();
                    let name = tsr_ast::clone_node(f, name);
                    f.set_node_range(name, name_loc);
                    ctx.add_emit_flags(name, emit_flags::NO_SOURCE_MAP);

                    let initializer = visitor.visit_node(initializer).expect(NIL);
                    ctx.add_emit_flags(
                        initializer,
                        emit_flags::NO_SOURCE_MAP | emit_flags::NO_COMMENTS,
                    );

                    let f = visitor.factory_mut();
                    let assignment = ctx.new_assignment_expression(f, name, initializer);
                    f.set_node_range(assignment, parameter_loc);
                    ctx.add_emit_flags(assignment, emit_flags::NO_COMMENTS);

                    let expression_statement = f.new_expression_statement(Some(assignment));
                    let statements = new_node_list(f, vec![expression_statement]);
                    let block = f.new_block(Some(statements), false);
                    f.set_node_range(block, parameter_loc);
                    ctx.add_emit_flags(
                        block,
                        emit_flags::SINGLE_LINE
                            | emit_flags::NO_TRAILING_SOURCE_MAP
                            | emit_flags::NO_TOKEN_SOURCE_MAPS
                            | emit_flags::NO_COMMENTS,
                    );

                    let name_clone = tsr_ast::clone_node(f, name);
                    let type_check = ctx.new_type_check(f, name_clone, b"undefined");
                    let statement = f.new_if_statement(Some(type_check), Some(block), None);
                    f.set_node_range(statement, parameter_loc);
                    ctx.add_emit_flags(
                        statement,
                        emit_flags::NO_TOKEN_SOURCE_MAPS
                            | emit_flags::NO_TRAILING_SOURCE_MAP
                            | emit_flags::CUSTOM_PROLOGUE
                            | emit_flags::NO_COMMENTS
                            | emit_flags::START_ON_NEW_LINE,
                    );
                    results.push(statement);
                }
            } else if self.facts(visitor.factory(), parameter) & OBJECT_REST_OR_SPREAD != 0 {
                contains_preceding_object_rest_or_spread = true;
                let generated = ctx.new_generated_name_for_node(visitor.factory_mut(), parameter);
                let declarations = flatten_destructuring_binding(
                    visitor,
                    &ctx,
                    parameter,
                    Some(generated),
                    FlattenLevel::ObjectRest,
                    false,
                    true,
                );
                let Some(declarations) = self.failure.ok(declarations) else {
                    break;
                };
                if let Some(declarations) = declarations {
                    let statement = self.new_custom_prologue_variable_statement(
                        visitor.factory_mut(),
                        declarations,
                    );
                    results.push(statement);
                }
            }
        }

        results
    }

    // port: tsc/internal/transformers/estransforms/objectrestspread.go:objectRestSpreadTransformer.visitCatchClause
    fn visit_catch_clause(&self, visitor: &mut NodeVisitor<'_>, node: NodeId) -> Option<NodeId> {
        let (variable_declaration, node_block) = {
            let read = visitor.node(node);
            let data = read
                .as_catch_clause()
                .expect("interface conversion: ast.nodeData is not *ast.CatchClause");
            (data.variable_declaration(), data.block())
        };
        if let Some(variable_declaration) = variable_declaration {
            let declaration_name = visitor.node(variable_declaration).name().expect(NIL);
            if is_binding_pattern(&visitor.node(declaration_name))
                && self.facts(visitor.factory(), declaration_name) & OBJECT_REST_OR_SPREAD != 0
            {
                let mut ctx = self.ctx();
                let name = ctx.new_generated_name_for_node(visitor.factory_mut(), declaration_name);
                let updated_decl = visitor.factory_mut().update_variable_declaration(
                    variable_declaration,
                    Some(declaration_name),
                    None,
                    None,
                    Some(name),
                );
                let visited_bindings = flatten_destructuring_binding(
                    visitor,
                    &ctx,
                    updated_decl,
                    None,
                    FlattenLevel::ObjectRest,
                    false,
                    false,
                );
                let Some(visited_bindings) = self.failure.ok(visited_bindings) else {
                    return Some(node);
                };
                let mut block = visitor.visit_node(node_block).expect(NIL);
                if let Some(visited_bindings) = visited_bindings {
                    let f = visitor.factory_mut();
                    let decls = declarations_of(f, visited_bindings);
                    let declarations = new_node_list(f, decls);
                    let declaration_list =
                        f.new_variable_declaration_list(Some(declarations), node_flags::NONE);
                    let new_statement = f.new_variable_statement(None, Some(declaration_list));
                    let (block_statements, multi_line) = block_parts(f, block);
                    let mut statements = vec![new_statement];
                    statements.extend(list_nodes(f, block_statements));
                    let statement_list = new_node_list(f, statements);
                    let loc = f.read_list(block_statements.expect(NIL)).loc();
                    f.set_list_location(statement_list, loc);

                    block = f.update_block(block, Some(statement_list), multi_line);
                }
                let f = visitor.factory_mut();
                let declaration = f.update_variable_declaration(
                    variable_declaration,
                    Some(name),
                    None,
                    None,
                    None,
                );
                return Some(f.update_catch_clause(node, Some(declaration), Some(block)));
            }
        }
        visitor.visit_each_child(Some(node))
    }

    // port: tsc/internal/transformers/estransforms/objectrestspread.go:objectRestSpreadTransformer.visitVariableStatement
    fn visit_variable_statement(
        &self,
        visitor: &mut NodeVisitor<'_>,
        node: NodeId,
    ) -> Option<NodeId> {
        if has_syntactic_modifier(visitor.factory(), node, modifier_flags::EXPORT) {
            let old_in_exported_variable_statement =
                self.in_exported_variable_statement.replace(true);
            let result = visitor.visit_each_child(Some(node));
            self.in_exported_variable_statement
                .set(old_in_exported_variable_statement);
            return result;
        }
        visitor.visit_each_child(Some(node))
    }

    // port: tsc/internal/transformers/estransforms/objectrestspread.go:objectRestSpreadTransformer.visitVariableDeclaration
    fn visit_variable_declaration(
        &self,
        visitor: &mut NodeVisitor<'_>,
        node: NodeId,
    ) -> Option<NodeId> {
        if self.in_exported_variable_statement.get() {
            self.in_exported_variable_statement.set(false);
            let result = self.visit_variable_declaration_worker(visitor, node, true);
            self.in_exported_variable_statement.set(true);
            return result;
        }
        self.visit_variable_declaration_worker(visitor, node, false)
    }

    // port: tsc/internal/transformers/estransforms/objectrestspread.go:objectRestSpreadTransformer.visitVariableDeclarationWorker
    fn visit_variable_declaration_worker(
        &self,
        visitor: &mut NodeVisitor<'_>,
        node: NodeId,
        exported: bool,
    ) -> Option<NodeId> {
        // If we are here it is because the name contains a binding pattern with a rest somewhere in it.
        let name = visitor.node(node).name().expect(NIL);
        if is_binding_pattern(&visitor.node(name))
            && self.facts(visitor.factory(), node) & OBJECT_REST_OR_SPREAD != 0
        {
            let ctx = self.ctx();
            let result = flatten_destructuring_binding(
                visitor,
                &ctx,
                node,
                None,
                FlattenLevel::ObjectRest,
                exported,
                false,
            );
            return match self.failure.ok(result) {
                Some(result) => result,
                None => Some(node),
            };
        }
        visitor.visit_each_child(Some(node))
    }

    // port: tsc/internal/transformers/estransforms/objectrestspread.go:objectRestSpreadTransformer.visitForOftatement
    fn visit_for_of_statement(
        &self,
        visitor: &mut NodeVisitor<'_>,
        node: NodeId,
    ) -> Option<NodeId> {
        let (await_modifier, initializer, expression, node_statement) = {
            let read = visitor.node(node);
            let data = read
                .as_for_in_or_of_statement()
                .expect("interface conversion: ast.nodeData is not *ast.ForInOrOfStatement");
            (
                data.await_modifier(),
                data.initializer().expect(NIL),
                data.expression(),
                data.statement(),
            )
        };
        if self.facts(visitor.factory(), initializer) & OBJECT_REST_OR_SPREAD != 0
            || (is_assignment_pattern(visitor.factory(), initializer)
                && self.contains_object_rest_or_spread(visitor.factory(), initializer))
        {
            let Some(initializer_without_parens) = self
                .failure
                .ok(skip_parentheses(visitor.factory(), initializer))
            else {
                return Some(node);
            };
            if visitor.node(initializer_without_parens).kind() == K::VariableDeclarationList
                || is_assignment_pattern(visitor.factory(), initializer_without_parens)
            {
                let mut body_location = TextRange::new(0, 0);
                let mut statements_location = TextRange::new(0, 0);
                let mut ctx = self.ctx();
                let temp = ctx.new_temp_variable(visitor.factory_mut());
                let binding = ctx.create_for_of_binding_statement(
                    visitor.factory_mut(),
                    initializer_without_parens,
                    temp,
                );
                let res = visitor.visit_node(Some(binding));
                let mut statements = Vec::with_capacity(1);
                if let Some(res) = res {
                    statements.push(res);
                }
                let statement_is_block = node_statement
                    .is_some_and(|statement| visitor.node(statement).kind() == K::Block);
                if statement_is_block {
                    let statement = node_statement.expect(NIL);
                    let statement_list = visitor.node(statement).statement_list();
                    for statement in list_nodes(visitor.factory(), statement_list) {
                        if let Some(visited) = visitor.visit_each_child(Some(statement)) {
                            statements.push(visited);
                        }
                    }
                    body_location = visitor.node(statement).range();
                    statements_location = visitor
                        .factory()
                        .read_list(statement_list.expect(NIL))
                        .loc();
                } else if let Some(statement) = node_statement {
                    statements.push(visitor.visit_each_child(Some(statement)).expect(NIL));
                    body_location = visitor.node(statement).range();
                    statements_location = visitor.node(statement).range();
                }

                let f = visitor.factory_mut();
                let declaration = f.new_variable_declaration(Some(temp), None, None, None);
                let declarations = new_node_list(f, vec![declaration]);
                let list = f.new_variable_declaration_list(Some(declarations), node_flags::LET);
                let initializer_loc = f.node(initializer).range();
                f.set_node_range(list, initializer_loc);

                let expr = visitor.visit_each_child(expression);

                let f = visitor.factory_mut();
                let statements_list = new_node_list(f, statements);
                f.set_list_location(statements_list, statements_location);

                let block = f.new_block(Some(statements_list), true);
                f.set_node_range(block, body_location);

                return Some(f.update_for_in_or_of_statement(
                    node,
                    await_modifier,
                    Some(list),
                    expr,
                    Some(block),
                ));
            }
        }
        visitor.visit_each_child(Some(node))
    }

    // port: tsc/internal/transformers/estransforms/objectrestspread.go:objectRestSpreadTransformer.visitBinaryExpression
    fn visit_binary_expression(
        &self,
        visitor: &mut NodeVisitor<'_>,
        node: NodeId,
        expression_result_is_unused: bool,
    ) -> Option<NodeId> {
        let (left, operator_token, right) = {
            let read = visitor.node(node);
            let data = read
                .as_binary_expression()
                .expect("interface conversion: ast.nodeData is not *ast.BinaryExpression");
            (data.left(), data.operator_token(), data.right())
        };
        let is_destructuring_assignment =
            self.failure.ok(view(visitor.factory()).and_then(|view| {
                tsr_ast::is_destructuring_assignment(view, node).map_err(Error::from)
            }));
        let Some(is_destructuring_assignment) = is_destructuring_assignment else {
            return Some(node);
        };
        if is_destructuring_assignment
            && self.contains_object_rest_or_spread(visitor.factory(), left.expect(NIL))
        {
            let ctx = self.ctx();
            let result = flatten_destructuring_assignment(
                visitor,
                &ctx,
                node,
                !expression_result_is_unused,
                FlattenLevel::ObjectRest,
                None,
            );
            return match self.failure.ok(result) {
                Some(result) => result,
                None => Some(node),
            };
        }
        if visitor.node(operator_token.expect(NIL)).kind() == K::CommaToken {
            self.expression_result_is_unused.set(true);
            let left = visitor.visit_node(left);
            self.expression_result_is_unused
                .set(expression_result_is_unused);
            let right = visitor.visit_node(right);
            return Some(visitor.factory_mut().update_binary_expression(
                node,
                None,
                left,
                None,
                operator_token,
                right,
            ));
        }
        visitor.visit_each_child(Some(node))
    }

    // port: tsc/internal/transformers/estransforms/objectrestspread.go:objectRestSpreadTransformer.visitObjectLiteralExpression
    fn visit_object_literal_expression(
        &self,
        visitor: &mut NodeVisitor<'_>,
        node: NodeId,
    ) -> Option<NodeId> {
        if (self.facts(visitor.factory(), node) & OBJECT_REST_OR_SPREAD) == 0 {
            return visitor.visit_each_child(Some(node));
        }
        // spread elements emit like so:
        // non-spread elements are chunked together into object literals, and then all are passed to __assign:
        //     { a, ...o, b } => __assign(__assign({a}, o), {b});
        // If the first element is a spread element, then the first argument to __assign is {}:
        //     { ...o, a, b, ...o2 } => __assign(__assign(__assign({}, o), {a, b}), o2)
        //
        // We cannot call __assign with more than two elements, since any element could cause side effects. For
        // example:
        //      var k = { a: 1, b: 2 };
        //      var o = { a: 3, ...k, b: k.a++ };
        //      // expected: { a: 1, b: 1 }
        // If we translate the above to `__assign({ a: 3 }, k, { b: k.a++ })`, the `k.a++` will evaluate before
        // `k` is spread and we end up with `{ a: 2, b: 1 }`.
        //
        // This also occurs for spread elements, not just property assignments:
        //      var k = { a: 1, get b() { l = { z: 9 }; return 2; } };
        //      var l = { c: 3 };
        //      var o = { ...k, ...l };
        //      // expected: { a: 1, b: 2, z: 9 }
        // If we translate the above to `__assign({}, k, l)`, the `l` will evaluate before `k` is spread and we
        // end up with `{ a: 1, b: 2, c: 3 }`

        let properties = visitor.node(node).property_list();
        let mut objects = Self::chunk_object_literal_elements(visitor, properties);
        let f = visitor.factory_mut();
        if !objects.is_empty() && f.node(objects[0]).kind() != K::ObjectLiteralExpression {
            let empty = new_node_list(f, Vec::new());
            let object = f.new_object_literal_expression(Some(empty), false);
            objects.insert(0, object);
        }
        assert!(
            !objects.is_empty(),
            "runtime error: index out of range [0] with length 0"
        );
        let mut expression = objects[0];
        let script_target = self.compiler_options.emit_script_target();
        if objects.len() > 1 {
            for &obj in &objects[1..] {
                expression = self
                    .ctx()
                    .new_assign_helper(f, vec![expression, obj], script_target);
            }
            return Some(expression);
        }
        Some(self.ctx().new_assign_helper(f, objects, script_target))
    }

    // port: tsc/internal/transformers/estransforms/objectrestspread.go:objectRestSpreadTransformer.chunkObjectLiteralElements
    fn chunk_object_literal_elements(
        visitor: &mut NodeVisitor<'_>,
        list: Option<NodeListId>,
    ) -> Vec<NodeId> {
        let elements = list_nodes(visitor.factory(), list);
        if elements.is_empty() {
            return Vec::new();
        }
        let mut chunk_object: Vec<NodeId> = Vec::new();
        let mut objects = Vec::with_capacity(1);
        for e in elements {
            let kind = visitor.node(e).kind();
            if kind == K::SpreadAssignment {
                if !chunk_object.is_empty() {
                    let f = visitor.factory_mut();
                    let properties = new_node_list(f, std::mem::take(&mut chunk_object));
                    objects.push(f.new_object_literal_expression(Some(properties), false));
                }
                let target = visitor.node(e).expression();
                objects.push(visitor.visit_node(target).expect(NIL));
            } else {
                let elem = if kind == K::PropertyAssignment {
                    let (name, initializer) = {
                        let read = visitor.node(e);
                        (read.name(), read.initializer())
                    };
                    let initializer = visitor.visit_node(initializer);
                    visitor.factory_mut().new_property_assignment(
                        None,
                        name,
                        None,
                        None,
                        initializer,
                    )
                } else {
                    visitor.visit_node(Some(e)).expect(NIL)
                };
                chunk_object.push(elem);
            }
        }
        if !chunk_object.is_empty() {
            let f = visitor.factory_mut();
            let properties = new_node_list(f, chunk_object);
            objects.push(f.new_object_literal_expression(Some(properties), false));
        }
        objects
    }
}
