//! Variable and lexical environments: hoisted `var` and `let` declarations,
//! hoisted functions and initialization statements, merged into a statement
//! list as custom prologues, and the visitor hooks that open and close them.
//!
//! No table lock is held while the factory runs: construction may reenter
//! this context through its hooks.

use super::{AutoGenerateOptions, EmitContext};
use crate::emit_flags;
use tsr_ast::{
    node_flags, Factory, FactoryMethods, JsString, ListVisit, NodeId, NodeListId, NodeVisit,
    NodeVisitor, NodeVisitorHooks, RuntimeFactory, SyntaxKind as K,
};
use tsr_core::TextRange;

/// Go's `environmentFlags`.
mod environment_flags {
    /// Currently visiting a parameter list.
    pub(super) const IN_PARAMETERS: u32 = 1 << 0;
    /// A temp variable was hoisted while visiting a parameter list.
    pub(super) const VARIABLES_HOISTED_IN_PARAMETERS: u32 = 1 << 1;
}

/// Go's `varScope`.
#[derive(Debug, Default)]
pub(super) struct VarScope {
    variables: Vec<NodeId>,
    functions: Vec<NodeId>,
    flags: u32,
    initialization_statements: Vec<NodeId>,
}

/// The visitor hooks `NewNodeVisitor` installs. Each hook holds a handle to
/// the context's shared tables, so the hooks outlive no borrow of the context.
pub struct EmitVisitorHooks {
    parameters: Box<ListVisit<'static>>,
    function_body: Box<NodeVisit<'static>>,
    iteration_body: Box<NodeVisit<'static>>,
    top_level_statements: Box<ListVisit<'static>>,
    embedded_statement: Box<NodeVisit<'static>>,
}

impl EmitVisitorHooks {
    /// Creates a new NodeVisitor attached to this EmitContext. `factory` must
    /// be the builder that carries this context's factory hooks.
    // port: tsc/internal/printer/emitcontext.go:EmitContext.NewNodeVisitor
    pub fn new_node_visitor<'a>(
        &'a self,
        visit: Option<&'a NodeVisit<'a>>,
        factory: &'a mut dyn RuntimeFactory,
    ) -> NodeVisitor<'a> {
        NodeVisitor::new(
            visit,
            Some(factory),
            NodeVisitorHooks {
                visit_parameters: Some(&*self.parameters),
                visit_function_body: Some(&*self.function_body),
                visit_iteration_body: Some(&*self.iteration_body),
                visit_top_level_statements: Some(&*self.top_level_statements),
                visit_embedded_statement: Some(&*self.embedded_statement),
                ..NodeVisitorHooks::default()
            },
        )
    }
}

/// `nodes` read from a node list, with Go's nil elements rejected.
fn list_nodes(factory: &dyn RuntimeFactory, list: NodeListId) -> Vec<NodeId> {
    let nodes = factory.read_list(list).nodes();
    factory
        .read_nodes(nodes)
        .iter()
        .map(|node| node.expect("nil node in statement list"))
        .collect()
}

/// `NodeFactory.NewNodeList`: an undefined location.
pub(super) fn new_node_list(factory: &mut dyn RuntimeFactory, nodes: Vec<NodeId>) -> NodeListId {
    let nodes = factory.alloc_nodes(nodes.into_iter().map(Some).collect());
    factory.alloc_list(TextRange::new(-1, -1), nodes)
}

/// A list with the same location as `original`.
fn new_node_list_at(
    factory: &mut dyn RuntimeFactory,
    nodes: Vec<NodeId>,
    original: NodeListId,
) -> NodeListId {
    let list = new_node_list(factory, nodes);
    let loc = factory.read_list(original).loc();
    factory.set_list_location(list, loc);
    list
}

// port: tsc/internal/printer/emitcontext.go:isHoistedVariable
fn is_hoisted_variable(factory: &dyn Factory, node: NodeId) -> bool {
    let read = factory.node(node);
    let name = read
        .name()
        .expect("runtime error: invalid memory address or nil pointer dereference");
    factory.node(name).kind() == K::Identifier && read.initializer().is_none()
}

/// `ast.IsPrologueDirective` over the factory's reads.
pub(super) fn is_prologue_directive(factory: &dyn Factory, node: NodeId) -> bool {
    let read = factory.node(node);
    read.kind() == K::ExpressionStatement
        && factory
            .node(
                read.expression()
                    .expect("runtime error: invalid memory address or nil pointer dereference"),
            )
            .kind()
            == K::StringLiteral
}

/// The text of a prologue directive's string literal.
fn prologue_text(factory: &dyn Factory, node: NodeId) -> JsString {
    let expression = factory
        .node(node)
        .expression()
        .expect("runtime error: invalid memory address or nil pointer dereference");
    factory
        .node(expression)
        .as_string_literal()
        .expect("prologue directive expression is a StringLiteral")
        .text_owned()
}

impl EmitContext {
    /// Returns the visitor hooks `NewNodeVisitor` installs, bound to this
    /// context's tables.
    pub fn visitor_hooks(&self) -> EmitVisitorHooks {
        let context = self.clone();
        let visit_parameters = move |visitor: &mut NodeVisitor<'_>, nodes: Option<NodeListId>| {
            context.clone().visit_parameters(nodes, visitor)
        };
        let context = self.clone();
        let visit_function_body = move |visitor: &mut NodeVisitor<'_>, node: Option<NodeId>| {
            context.clone().visit_function_body(node, visitor)
        };
        let context = self.clone();
        let visit_iteration_body = move |visitor: &mut NodeVisitor<'_>, node: Option<NodeId>| {
            context.clone().visit_iteration_body(node, visitor)
        };
        let context = self.clone();
        let visit_top_level_statements =
            move |visitor: &mut NodeVisitor<'_>, nodes: Option<NodeListId>| {
                context.clone().visit_variable_environment(nodes, visitor)
            };
        let context = self.clone();
        let visit_embedded_statement =
            move |visitor: &mut NodeVisitor<'_>, node: Option<NodeId>| {
                context.clone().visit_embedded_statement(node, visitor)
            };
        EmitVisitorHooks {
            parameters: Box::new(visit_parameters),
            function_body: Box::new(visit_function_body),
            iteration_body: Box::new(visit_iteration_body),
            top_level_statements: Box::new(visit_top_level_statements),
            embedded_statement: Box::new(visit_embedded_statement),
        }
    }

    /// Starts a new VariableEnvironment used to track hoisted `var` statements
    /// and function declarations (Strada's `startLexicalEnvironment`).
    // port: tsc/internal/printer/emitcontext.go:EmitContext.StartVariableEnvironment
    pub fn start_variable_environment(&mut self) {
        self.tables().var_scope_stack.push(VarScope::default());
        self.start_lexical_environment();
    }

    /// Ends the current VariableEnvironment, returning a list of statements
    /// that should be emitted at the start of the current scope.
    // port: tsc/internal/printer/emitcontext.go:EmitContext.EndVariableEnvironment
    pub fn end_variable_environment(&mut self, factory: &mut dyn RuntimeFactory) -> Vec<NodeId> {
        let scope = self.tables().var_scope_stack.pop().expect("stack is empty");
        let mut statements = Vec::new();
        if !scope.functions.is_empty() {
            statements.clone_from(&scope.functions);
        }
        if !scope.variables.is_empty() {
            let declarations = new_node_list(factory, scope.variables);
            let var_decl_list =
                factory.new_variable_declaration_list(Some(declarations), node_flags::NONE);
            let var_statement = factory.new_variable_statement(None, Some(var_decl_list));
            self.set_emit_flags(var_statement, emit_flags::CUSTOM_PROLOGUE);
            statements.push(var_statement);
        }
        if !scope.initialization_statements.is_empty() {
            statements.extend(scope.initialization_statements);
        }
        statements.extend(self.end_lexical_environment(factory));
        statements
    }

    /// Invokes `end_variable_environment` and merges the results into
    /// `statements`. Upstream dereferences a nil list when declarations were
    /// merged into it; that is a panic here too.
    // port: tsc/internal/printer/emitcontext.go:EmitContext.EndAndMergeVariableEnvironmentList
    pub fn end_and_merge_variable_environment_list(
        &mut self,
        factory: &mut dyn RuntimeFactory,
        statements: Option<NodeListId>,
    ) -> Option<NodeListId> {
        let nodes = statements.map_or_else(Vec::new, |list| list_nodes(factory, list));
        let (result, changed) = self.end_and_merge_variable_environment_changed(factory, nodes);
        if changed {
            let statements = statements
                .expect("runtime error: invalid memory address or nil pointer dereference");
            return Some(new_node_list_at(factory, result, statements));
        }
        statements
    }

    /// Invokes `end_variable_environment` and merges the results into `statements`.
    // port: tsc/internal/printer/emitcontext.go:EmitContext.EndAndMergeVariableEnvironment
    pub fn end_and_merge_variable_environment(
        &mut self,
        factory: &mut dyn RuntimeFactory,
        statements: Vec<NodeId>,
    ) -> Vec<NodeId> {
        self.end_and_merge_variable_environment_changed(factory, statements)
            .0
    }

    // port: tsc/internal/printer/emitcontext.go:EmitContext.endAndMergeVariableEnvironment
    fn end_and_merge_variable_environment_changed(
        &mut self,
        factory: &mut dyn RuntimeFactory,
        statements: Vec<NodeId>,
    ) -> (Vec<NodeId>, bool) {
        let declarations = self.end_variable_environment(factory);
        self.merge_environment_changed(factory, statements, &declarations)
    }

    /// Adds a `var` declaration to the current VariableEnvironment (Strada's
    /// `hoistVariableDeclaration`).
    // port: tsc/internal/printer/emitcontext.go:EmitContext.AddVariableDeclaration
    pub fn add_variable_declaration(&mut self, factory: &mut dyn Factory, name: NodeId) {
        let var_decl = factory.new_variable_declaration(Some(name), None, None, None);
        self.set_emit_flags(var_decl, emit_flags::NO_NESTED_SOURCE_MAPS);
        let mut tables = self.tables();
        let scope = tables.var_scope_stack.last_mut().expect("stack is empty");
        scope.variables.push(var_decl);
        if scope.flags & environment_flags::IN_PARAMETERS != 0 {
            scope.flags |= environment_flags::VARIABLES_HOISTED_IN_PARAMETERS;
        }
    }

    /// Adds a hoisted function declaration to the current VariableEnvironment
    /// (Strada's `hoistFunctionDeclaration`).
    // port: tsc/internal/printer/emitcontext.go:EmitContext.AddHoistedFunctionDeclaration
    pub fn add_hoisted_function_declaration(&mut self, node: NodeId) {
        self.set_emit_flags(node, emit_flags::CUSTOM_PROLOGUE);
        let mut tables = self.tables();
        let scope = tables.var_scope_stack.last_mut().expect("stack is empty");
        scope.functions.push(node);
    }

    /// Starts a new LexicalEnvironment used to track block-scoped `let`,
    /// `const` and `using` declarations (Strada's `startBlockScope`).
    // port: tsc/internal/printer/emitcontext.go:EmitContext.StartLexicalEnvironment
    pub fn start_lexical_environment(&mut self) {
        self.tables().let_scope_stack.push(VarScope::default());
    }

    /// Ends the current LexicalEnvironment, returning a list of statements
    /// that should be emitted at the start of the current scope.
    // port: tsc/internal/printer/emitcontext.go:EmitContext.EndLexicalEnvironment
    pub fn end_lexical_environment(&mut self, factory: &mut dyn RuntimeFactory) -> Vec<NodeId> {
        let scope = self.tables().let_scope_stack.pop().expect("stack is empty");
        let mut statements = Vec::new();
        if !scope.variables.is_empty() {
            let declarations = new_node_list(factory, scope.variables);
            let var_decl_list =
                factory.new_variable_declaration_list(Some(declarations), node_flags::LET);
            let var_statement = factory.new_variable_statement(None, Some(var_decl_list));
            self.set_emit_flags(var_statement, emit_flags::CUSTOM_PROLOGUE);
            statements.push(var_statement);
        }
        statements
    }

    /// Invokes `end_lexical_environment` and merges the results into
    /// `statements`; a nil list that receives declarations panics as upstream.
    // port: tsc/internal/printer/emitcontext.go:EmitContext.EndAndMergeLexicalEnvironmentList
    pub fn end_and_merge_lexical_environment_list(
        &mut self,
        factory: &mut dyn RuntimeFactory,
        statements: Option<NodeListId>,
    ) -> Option<NodeListId> {
        let nodes = statements.map_or_else(Vec::new, |list| list_nodes(factory, list));
        let (result, changed) = self.end_and_merge_lexical_environment_changed(factory, nodes);
        if changed {
            let statements = statements
                .expect("runtime error: invalid memory address or nil pointer dereference");
            return Some(new_node_list_at(factory, result, statements));
        }
        statements
    }

    /// Invokes `end_lexical_environment` and merges the results into `statements`.
    // port: tsc/internal/printer/emitcontext.go:EmitContext.EndAndMergeLexicalEnvironment
    pub fn end_and_merge_lexical_environment(
        &mut self,
        factory: &mut dyn RuntimeFactory,
        statements: Vec<NodeId>,
    ) -> Vec<NodeId> {
        self.end_and_merge_lexical_environment_changed(factory, statements)
            .0
    }

    // port: tsc/internal/printer/emitcontext.go:EmitContext.endAndMergeLexicalEnvironment
    fn end_and_merge_lexical_environment_changed(
        &mut self,
        factory: &mut dyn RuntimeFactory,
        statements: Vec<NodeId>,
    ) -> (Vec<NodeId>, bool) {
        let declarations = self.end_lexical_environment(factory);
        self.merge_environment_changed(factory, statements, &declarations)
    }

    /// Adds a `let` declaration to the current LexicalEnvironment.
    // port: tsc/internal/printer/emitcontext.go:EmitContext.AddLexicalDeclaration
    pub fn add_lexical_declaration(&mut self, factory: &mut dyn Factory, name: NodeId) {
        let var_decl = factory.new_variable_declaration(Some(name), None, None, None);
        self.set_emit_flags(var_decl, emit_flags::NO_NESTED_SOURCE_MAPS);
        let mut tables = self.tables();
        let scope = tables.let_scope_stack.last_mut().expect("stack is empty");
        scope.variables.push(var_decl);
    }

    /// Merges declarations produced by `end_variable_environment` or
    /// `end_lexical_environment` into a statement list.
    // port: tsc/internal/printer/emitcontext.go:EmitContext.MergeEnvironmentList
    pub fn merge_environment_list(
        &mut self,
        factory: &mut dyn RuntimeFactory,
        statements: NodeListId,
        declarations: &[NodeId],
    ) -> NodeListId {
        let nodes = list_nodes(factory, statements);
        let (result, changed) = self.merge_environment_changed(factory, nodes, declarations);
        if changed {
            return new_node_list_at(factory, result, statements);
        }
        statements
    }

    /// Merges declarations produced by `end_variable_environment` or
    /// `end_lexical_environment` into a slice of statements.
    // port: tsc/internal/printer/emitcontext.go:EmitContext.MergeEnvironment
    pub fn merge_environment(
        &mut self,
        factory: &dyn RuntimeFactory,
        statements: Vec<NodeId>,
        declarations: &[NodeId],
    ) -> Vec<NodeId> {
        self.merge_environment_changed(factory, statements, declarations)
            .0
    }

    /// The result is laid out as: standard prologues (right, then left),
    /// hoisted functions (right, left), hoisted variables (right, left),
    /// lexical init statements (right, left), other statements (left). New
    /// lexical init statements must evaluate before existing ones.
    // port: tsc/internal/printer/emitcontext.go:EmitContext.mergeEnvironment
    fn merge_environment_changed(
        &self,
        factory: &dyn RuntimeFactory,
        statements: Vec<NodeId>,
        declarations: &[NodeId],
    ) -> (Vec<NodeId>, bool) {
        if declarations.is_empty() {
            return (statements, false);
        }
        // `findSpanEnd` and `findSpanEndWithEmitContext`.
        let find_span_end = |array: &[NodeId], test: &dyn Fn(NodeId) -> bool, start: usize| {
            let mut i = start;
            while i < array.len() && test(array[i]) {
                i += 1;
            }
            i
        };
        let is_prologue = |node| is_prologue_directive(factory, node);
        let is_hoisted_function = |node| self.is_hoisted_function(factory, node);
        let is_hoisted_variable_statement =
            |node| self.is_hoisted_variable_statement(factory, node);
        let is_custom_prologue = |node| self.is_custom_prologue(node);

        let mut changed = false;

        // find standard prologues on left in the following order: standard
        // directives, hoisted functions, hoisted variables, other custom
        let left_standard_prologue_end = find_span_end(&statements, &is_prologue, 0);
        let left_hoisted_functions_end = find_span_end(
            &statements,
            &is_hoisted_function,
            left_standard_prologue_end,
        );
        let left_hoisted_variables_end = find_span_end(
            &statements,
            &is_hoisted_variable_statement,
            left_hoisted_functions_end,
        );

        // find standard prologues on right in the following order: standard
        // directives, hoisted functions, hoisted variables, other custom
        let right_standard_prologue_end = find_span_end(declarations, &is_prologue, 0);
        let right_hoisted_functions_end = find_span_end(
            declarations,
            &is_hoisted_function,
            right_standard_prologue_end,
        );
        let right_hoisted_variables_end = find_span_end(
            declarations,
            &is_hoisted_variable_statement,
            right_hoisted_functions_end,
        );
        let right_custom_prologue_end = find_span_end(
            declarations,
            &is_custom_prologue,
            right_hoisted_variables_end,
        );
        assert!(
            right_custom_prologue_end == declarations.len(),
            "Expected declarations to be valid standard or custom prologues"
        );

        let mut left = statements.clone();

        // splice other custom prologues from right into left
        if right_custom_prologue_end > right_hoisted_variables_end {
            left.splice(
                left_hoisted_variables_end..left_hoisted_variables_end,
                declarations[right_hoisted_variables_end..right_custom_prologue_end]
                    .iter()
                    .copied(),
            );
            changed = true;
        }

        // splice hoisted variables from right into left
        if right_hoisted_variables_end > right_hoisted_functions_end {
            left.splice(
                left_hoisted_functions_end..left_hoisted_functions_end,
                declarations[right_hoisted_functions_end..right_hoisted_variables_end]
                    .iter()
                    .copied(),
            );
            changed = true;
        }

        // splice hoisted functions from right into left
        if right_hoisted_functions_end > right_standard_prologue_end {
            left.splice(
                left_standard_prologue_end..left_standard_prologue_end,
                declarations[right_standard_prologue_end..right_hoisted_functions_end]
                    .iter()
                    .copied(),
            );
            changed = true;
        }

        // splice standard prologues from right into left (that are not already in left)
        if right_standard_prologue_end > 0 {
            if left_standard_prologue_end == 0 {
                left.splice(
                    0..0,
                    declarations[..right_standard_prologue_end].iter().copied(),
                );
                changed = true;
            } else {
                let left_prologues: Vec<JsString> = statements[..left_standard_prologue_end]
                    .iter()
                    .map(|&left_prologue| prologue_text(factory, left_prologue))
                    .collect();
                for &right_prologue in declarations[..right_standard_prologue_end].iter().rev() {
                    let text = prologue_text(factory, right_prologue);
                    if !left_prologues.contains(&text) {
                        left.insert(0, right_prologue);
                        changed = true;
                    }
                }
            }
        }

        (left, changed)
    }

    // port: tsc/internal/printer/emitcontext.go:EmitContext.isCustomPrologue
    fn is_custom_prologue(&self, node: NodeId) -> bool {
        self.emit_flags(node) & emit_flags::CUSTOM_PROLOGUE != 0
    }

    // port: tsc/internal/printer/emitcontext.go:EmitContext.isHoistedFunction
    fn is_hoisted_function(&self, factory: &dyn Factory, node: NodeId) -> bool {
        self.is_custom_prologue(node) && factory.node(node).kind() == K::FunctionDeclaration
    }

    // port: tsc/internal/printer/emitcontext.go:EmitContext.isHoistedVariableStatement
    fn is_hoisted_variable_statement(&self, factory: &dyn RuntimeFactory, node: NodeId) -> bool {
        if !self.is_custom_prologue(node) {
            return false;
        }
        let read = factory.node(node);
        let Some(statement) = read.as_variable_statement() else {
            return false;
        };
        let list = statement
            .declaration_list()
            .expect("runtime error: invalid memory address or nil pointer dereference");
        let list_read = factory.node(list);
        let declarations = list_read
            .as_variable_declaration_list()
            .expect("interface conversion: ast.nodeData is not *ast.VariableDeclarationList")
            .declarations();
        let Some(declarations) = declarations else {
            return true;
        };
        let nodes = factory.read_list(declarations).nodes();
        factory.read_nodes(nodes).iter().all(|declaration| {
            is_hoisted_variable(
                factory,
                declaration
                    .expect("runtime error: invalid memory address or nil pointer dereference"),
            )
        })
    }

    // port: tsc/internal/printer/emitcontext.go:EmitContext.VisitVariableEnvironment
    pub fn visit_variable_environment(
        &mut self,
        nodes: Option<NodeListId>,
        visitor: &mut NodeVisitor<'_>,
    ) -> Option<NodeListId> {
        self.start_variable_environment();
        let visited = visitor.visit_nodes(nodes);
        self.end_and_merge_variable_environment_list(visitor.factory_mut(), visited)
    }

    // port: tsc/internal/printer/emitcontext.go:EmitContext.VisitParameters
    pub fn visit_parameters(
        &mut self,
        nodes: Option<NodeListId>,
        visitor: &mut NodeVisitor<'_>,
    ) -> Option<NodeListId> {
        self.start_variable_environment();
        // Upstream holds the scope pointer; nested environments are balanced,
        // so the scope keeps its stack slot until this visit returns.
        let scope_index = self.tables().var_scope_stack.len() - 1;
        let old_flags = {
            let mut tables = self.tables();
            let scope = &mut tables.var_scope_stack[scope_index];
            let old_flags = scope.flags;
            scope.flags |= environment_flags::IN_PARAMETERS;
            old_flags
        };
        let mut nodes = visitor.visit_nodes(nodes);

        // As of ES2015, any runtime execution of that occurs in for a parameter
        // (such as evaluating an initializer or a binding pattern), occurs in
        // its own lexical scope. As a result, any expression that we might
        // transform that introduces a temporary variable would fail as the
        // temporary variable exists in a different lexical scope. To address
        // this, we move any binding patterns and initializers in a parameter
        // list to the body if we detect a variable being hoisted while
        // visiting a parameter list when the emit target is greater than
        // ES2015. (Which is now all targets.)
        let hoisted = self.tables().var_scope_stack[scope_index].flags
            & environment_flags::VARIABLES_HOISTED_IN_PARAMETERS
            != 0;
        if hoisted {
            nodes = self.add_default_value_assignments_if_needed(visitor.factory_mut(), nodes);
        }
        self.tables().var_scope_stack[scope_index].flags = old_flags;
        // !!! c.suspendVariableEnvironment()
        nodes
    }

    // port: tsc/internal/printer/emitcontext.go:EmitContext.addDefaultValueAssignmentsIfNeeded
    fn add_default_value_assignments_if_needed(
        &mut self,
        factory: &mut dyn RuntimeFactory,
        node_list: Option<NodeListId>,
    ) -> Option<NodeListId> {
        let node_list = node_list?;
        let mut result: Option<Vec<Option<NodeId>>> = None;
        let nodes: Vec<Option<NodeId>> = {
            let slice = factory.read_list(node_list).nodes();
            factory.read_nodes(slice).iter().collect()
        };
        for (i, parameter) in nodes.iter().enumerate() {
            let parameter = parameter
                .expect("runtime error: invalid memory address or nil pointer dereference");
            let updated = self.add_default_value_assignment_if_needed(factory, parameter);
            if updated != parameter {
                result.get_or_insert_with(|| nodes.clone())[i] = Some(updated);
            }
        }
        if let Some(result) = result {
            let nodes = factory.alloc_nodes(result);
            let res = factory.alloc_list(TextRange::new(-1, -1), nodes);
            let loc = factory.read_list(node_list).loc();
            factory.set_list_location(res, loc);
            return Some(res);
        }
        Some(node_list)
    }

    // port: tsc/internal/printer/emitcontext.go:EmitContext.addDefaultValueAssignmentIfNeeded
    fn add_default_value_assignment_if_needed(
        &mut self,
        factory: &mut dyn RuntimeFactory,
        parameter: NodeId,
    ) -> NodeId {
        let (dot_dot_dot_token, name, initializer) = {
            let read = factory.node(parameter);
            let data = read
                .as_parameter_declaration()
                .expect("interface conversion: ast.nodeData is not *ast.ParameterDeclaration");
            (data.dot_dot_dot_token(), data.name(), data.initializer())
        };
        // A rest parameter cannot have a binding pattern or an initializer,
        // so let's just ignore it.
        if dot_dot_dot_token.is_some() {
            return parameter;
        }
        let name = name.expect("runtime error: invalid memory address or nil pointer dereference");
        if tsr_ast::utilities::is_binding_pattern(&factory.node(name)) {
            return self.add_default_value_assignment_for_binding_pattern(factory, parameter);
        } else if let Some(initializer) = initializer {
            return self.add_default_value_assignment_for_initializer(
                factory,
                parameter,
                name,
                initializer,
            );
        }
        parameter
    }

    // port: tsc/internal/printer/emitcontext.go:EmitContext.addDefaultValueAssignmentForBindingPattern
    fn add_default_value_assignment_for_binding_pattern(
        &mut self,
        factory: &mut dyn RuntimeFactory,
        parameter: NodeId,
    ) -> NodeId {
        let (modifiers, dot_dot_dot_token, name, question_token, type_node, initializer) = {
            let read = factory.node(parameter);
            let data = read
                .as_parameter_declaration()
                .expect("interface conversion: ast.nodeData is not *ast.ParameterDeclaration");
            (
                data.modifiers(),
                data.dot_dot_dot_token(),
                data.name(),
                data.question_token(),
                data.r#type(),
                data.initializer(),
            )
        };
        let init_node = if let Some(initializer) = initializer {
            let generated = self.new_generated_name_for_node_ex(
                factory,
                parameter,
                AutoGenerateOptions::default(),
            );
            let void_zero = self.new_void_zero_expression(factory);
            let condition = self.new_strict_equality_expression(factory, generated, void_zero);
            let question_token = factory.new_token(K::QuestionToken.into());
            let colon_token = factory.new_token(K::ColonToken.into());
            let when_false = self.new_generated_name_for_node_ex(
                factory,
                parameter,
                AutoGenerateOptions::default(),
            );
            factory.new_conditional_expression(
                Some(condition),
                Some(question_token),
                Some(initializer),
                Some(colon_token),
                Some(when_false),
            )
        } else {
            self.new_generated_name_for_node_ex(factory, parameter, AutoGenerateOptions::default())
        };
        let declaration = factory.new_variable_declaration(name, None, type_node, Some(init_node));
        let declarations = new_node_list(factory, vec![declaration]);
        let declaration_list =
            factory.new_variable_declaration_list(Some(declarations), node_flags::NONE);
        let statement = factory.new_variable_statement(None, Some(declaration_list));
        self.add_initialization_statement(statement);
        let generated =
            self.new_generated_name_for_node_ex(factory, parameter, AutoGenerateOptions::default());
        factory.update_parameter_declaration(
            parameter,
            modifiers,
            dot_dot_dot_token,
            Some(generated),
            question_token,
            type_node,
            None,
        )
    }

    // port: tsc/internal/printer/emitcontext.go:EmitContext.addDefaultValueAssignmentForInitializer
    fn add_default_value_assignment_for_initializer(
        &mut self,
        factory: &mut dyn RuntimeFactory,
        parameter: NodeId,
        name: NodeId,
        initializer: NodeId,
    ) -> NodeId {
        self.add_emit_flags(
            initializer,
            emit_flags::NO_SOURCE_MAP | emit_flags::NO_COMMENTS,
        );
        let name_clone = tsr_ast::clone_node(factory, name);
        self.add_emit_flags(name_clone, emit_flags::NO_SOURCE_MAP);
        let init_assignment = self.new_assignment_expression(factory, name_clone, initializer);
        let parameter_loc = factory.node(parameter).range();
        factory.set_node_range(init_assignment, parameter_loc);
        self.add_emit_flags(init_assignment, emit_flags::NO_COMMENTS);
        let statement = factory.new_expression_statement(Some(init_assignment));
        let statements = new_node_list(factory, vec![statement]);
        let init_block = factory.new_block(Some(statements), false);
        factory.set_node_range(init_block, parameter_loc);
        self.add_emit_flags(
            init_block,
            emit_flags::SINGLE_LINE
                | emit_flags::NO_TRAILING_SOURCE_MAP
                | emit_flags::NO_TOKEN_SOURCE_MAPS
                | emit_flags::NO_COMMENTS,
        );
        let name_clone = tsr_ast::clone_node(factory, name);
        let type_check = self.new_type_check(factory, name_clone, b"undefined");
        let if_statement = factory.new_if_statement(Some(type_check), Some(init_block), None);
        self.add_initialization_statement(if_statement);
        let (modifiers, dot_dot_dot_token, name, question_token, type_node) = {
            let read = factory.node(parameter);
            let data = read
                .as_parameter_declaration()
                .expect("interface conversion: ast.nodeData is not *ast.ParameterDeclaration");
            (
                data.modifiers(),
                data.dot_dot_dot_token(),
                data.name(),
                data.question_token(),
                data.r#type(),
            )
        };
        factory.update_parameter_declaration(
            parameter,
            modifiers,
            dot_dot_dot_token,
            name,
            question_token,
            type_node,
            None,
        )
    }

    // port: tsc/internal/printer/emitcontext.go:EmitContext.AddInitializationStatement
    pub fn add_initialization_statement(&mut self, node: NodeId) {
        assert!(!self.tables().var_scope_stack.is_empty(), "stack is empty");
        self.add_emit_flags(node, emit_flags::CUSTOM_PROLOGUE);
        self.tables()
            .var_scope_stack
            .last_mut()
            .expect("stack is empty")
            .initialization_statements
            .push(node);
    }

    // port: tsc/internal/printer/emitcontext.go:EmitContext.ConvertToFunctionBlock
    pub fn convert_to_function_block(
        &mut self,
        factory: &mut dyn RuntimeFactory,
        node: NodeId,
        multi_line: bool,
    ) -> NodeId {
        if factory.node(node).kind() == K::Block {
            return node;
        }
        let loc = factory.node(node).range();
        let return_statement = factory.new_return_statement(Some(node));
        factory.set_node_range(return_statement, loc);
        let statements = new_node_list(factory, vec![return_statement]);
        factory.set_list_location(statements, loc);
        let block = factory.new_block(Some(statements), multi_line);
        factory.set_node_range(block, loc);
        block
    }

    // port: tsc/internal/printer/emitcontext.go:EmitContext.VisitFunctionBody
    pub fn visit_function_body(
        &mut self,
        node: Option<NodeId>,
        visitor: &mut NodeVisitor<'_>,
    ) -> Option<NodeId> {
        // !!! c.resumeVariableEnvironment()
        let updated = visitor.visit_node(node);
        let factory = visitor.factory_mut();
        let declarations = self.end_variable_environment(factory);
        if declarations.is_empty() {
            return updated;
        }

        let Some(updated) = updated else {
            let statements = new_node_list(factory, declarations);
            return Some(factory.new_block(Some(statements), true));
        };

        if factory.node(updated).kind() != K::Block {
            self.add_emit_flags(updated, emit_flags::NO_COMMENTS);
            let block = self.convert_to_function_block(factory, updated, false);
            let (statements, multi_line) = block_parts(factory, block);
            let merged = self.merge_environment_list(factory, statements, &declarations);
            return Some(factory.update_block(block, Some(merged), multi_line));
        }

        let (statements, multi_line) = block_parts(factory, updated);
        let merged = self.merge_environment_list(factory, statements, &declarations);
        Some(factory.update_block(updated, Some(merged), multi_line))
    }

    // port: tsc/internal/printer/emitcontext.go:EmitContext.VisitIterationBody
    pub fn visit_iteration_body(
        &mut self,
        body: Option<NodeId>,
        visitor: &mut NodeVisitor<'_>,
    ) -> Option<NodeId> {
        body?;

        self.start_lexical_environment();
        let updated = self
            .visit_embedded_statement(body, visitor)
            .expect("Expected visitor to return a statement.");

        let factory = visitor.factory_mut();
        let mut statements = self.end_lexical_environment(factory);
        if !statements.is_empty() {
            if factory.node(updated).kind() == K::Block {
                let (list, multi_line) = block_parts(factory, updated);
                statements.extend(list_nodes(factory, list));
                let statements_list = new_node_list_at(factory, statements, list);
                return Some(factory.update_block(updated, Some(statements_list), multi_line));
            }
            statements.push(updated);
            let statements = new_node_list(factory, statements);
            return Some(factory.new_block(Some(statements), true));
        }

        Some(updated)
    }

    // port: tsc/internal/printer/emitcontext.go:EmitContext.VisitEmbeddedStatement
    pub fn visit_embedded_statement(
        &mut self,
        node: Option<NodeId>,
        visitor: &mut NodeVisitor<'_>,
    ) -> Option<NodeId> {
        let node = node?;
        let embedded_statement = visitor.visit_embedded_statement(Some(node));
        let factory = visitor.factory_mut();
        if embedded_statement
            .is_none_or(|statement| factory.node(statement).kind() == K::NotEmittedStatement)
        {
            let empty_statement = factory.new_empty_statement();
            let loc = factory.node(node).range();
            factory.set_node_range(empty_statement, loc);
            self.set_original(empty_statement, node);
            self.assign_comment_range(factory, empty_statement, node);
            return Some(empty_statement);
        }
        embedded_statement
    }

    // port: tsc/internal/printer/emitcontext.go:EmitContext.NewNotEmittedStatement
    pub fn new_not_emitted_statement(&mut self, factory: &mut dyn Factory, node: NodeId) -> NodeId {
        let statement = factory.new_not_emitted_statement();
        let loc = factory.node(node).range();
        factory.set_node_range(statement, loc);
        self.set_original(statement, node);
        self.assign_comment_range(factory, statement, node);
        statement
    }
}

/// A block's statement list (`StatementList()`, which a block always has) and
/// its `MultiLine` flag.
fn block_parts(factory: &dyn RuntimeFactory, block: NodeId) -> (NodeListId, bool) {
    let read = factory.node(block);
    let data = read
        .as_block()
        .expect("interface conversion: ast.nodeData is not *ast.Block");
    (
        data.statements()
            .expect("runtime error: invalid memory address or nil pointer dereference"),
        data.multi_line(),
    )
}
