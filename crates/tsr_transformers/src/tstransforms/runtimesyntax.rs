//! `transformers/tstransforms/runtimesyntax.go`: transforms the TypeScript
//! syntax with runtime semantics into JavaScript: enums and namespaces become
//! immediately invoked functions over a `var`, parameter properties become
//! property declarations and constructor assignments, internal
//! `import x = A.B` aliases become variables, and references to namespace
//! exports and enum members are qualified with their container.
//!
//! Upstream's map of the first declaration of each name in the current scope
//! is a Go map, a reference that `pushScope` saves and `popScope` restores
//! only when the scope changed; it is an `Rc` here so a restore hands back the
//! same map, with the entries added to it in place.
use super::utilities::constant_expression;
use crate::destructuring::{flatten_destructuring_assignment, FlattenLevel};
use crate::estransforms::utilities::{identifier_text, list_nodes, new_node_list, view};
use crate::extract_modifiers;
use crate::transformer::{
    Error, Failure, SharedEmitResolver, SharedReferenceResolver, TransformOptions, Transformer,
};
use crate::utilities::{
    convert_variable_declaration_to_assignment_expression, find_super_statement_index_path,
    interface_conversion, is_generated_identifier, is_identifier_reference, is_local_name,
};
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;
use tsr_ast::utilities::{is_binding_pattern, is_enum_const, is_parameter_property_declaration};
use tsr_ast::utilities_class::child_is_decorated;
use tsr_ast::{
    is_constructor_declaration, is_enum_declaration, is_identifier, is_instantiated_module,
    is_module_declaration, is_try_statement, modifier_flags, node_flags, subtree_flags,
    token_flags, AstBuilder, FactoryMethods, JsString, NodeId, NodeListId, NodeVisitor,
    RuntimeFactory, SyntaxKind as K,
};
use tsr_core::{CompilerOptions, ModuleKind, TextRange};
use tsr_printer::emit_resolver::ConstantValue;
use tsr_printer::{emit_flags, AssignedNameOptions, EmitContext, EmitFlags, NameOptions};

const NIL: &str = "runtime error: invalid memory address or nil pointer dereference";

/// Upstream's `map[string]*ast.Node` of the first declaration of each name in
/// the current scope; `None` is the nil map.
type FirstDeclarations = Option<Rc<RefCell<HashMap<JsString, NodeId>>>>;

/// `RuntimeSyntaxTransformer`. No field is borrowed across a visit.
struct RuntimeSyntaxTransformer<'a> {
    compiler_options: Arc<CompilerOptions>,
    emit_context: EmitContext,
    failure: Failure,
    parent_node: Cell<Option<NodeId>>,
    current_node: Cell<Option<NodeId>>,
    current_source_file: Cell<Option<NodeId>>,
    /// SourceFile | Block | ModuleBlock | CaseBlock
    current_scope: Cell<Option<NodeId>>,
    current_scope_first_declarations_of_name: RefCell<FirstDeclarations>,
    current_enum: Cell<Option<NodeId>>,
    current_namespace: Cell<Option<NodeId>>,
    resolver: SharedReferenceResolver<'a>,
    emit_resolver: SharedEmitResolver<'a>,
}

// port: tsc/internal/transformers/tstransforms/runtimesyntax.go:NewRuntimeSyntaxTransformer
pub fn new_runtime_syntax_transformer<'a>(opts: &TransformOptions<'a>) -> Option<Transformer<'a>> {
    let compiler_options = Arc::clone(&opts.compiler_options);
    let emit_context = opts.context.clone();
    let tx = Rc::new(RuntimeSyntaxTransformer {
        compiler_options,
        emit_context: emit_context.clone(),
        failure: opts.failure.clone(),
        parent_node: Cell::new(None),
        current_node: Cell::new(None),
        current_source_file: Cell::new(None),
        current_scope: Cell::new(None),
        current_scope_first_declarations_of_name: RefCell::new(None),
        current_enum: Cell::new(None),
        current_namespace: Cell::new(None),
        resolver: Rc::clone(&opts.resolver),
        emit_resolver: Rc::clone(&opts.emit_resolver),
    });
    Some(Transformer::new(
        move |visitor: &mut NodeVisitor<'_>, node: Option<NodeId>| {
            if tx.failure.is_set() {
                return node;
            }
            match tx.visit(visitor, node) {
                Ok(visited) => visited,
                Err(error) => {
                    tx.failure.record(error);
                    node
                }
            }
        },
        Some(emit_context),
        opts.failure.clone(),
    ))
}

/// The builder behind the visitor's factory, which the printer's declaration
/// name helpers take.
fn builder<'v>(visitor: &'v mut NodeVisitor<'_>) -> Result<&'v mut AstBuilder, Error> {
    visitor
        .factory_mut()
        .ast_builder_mut()
        .ok_or(Error::Unsupported(
            "a transform over a factory without AST storage",
        ))
}

/// The nodes of `nodes` as a slice of the factory, visited (`VisitSlice`).
fn visit_slice(visitor: &mut NodeVisitor<'_>, nodes: &[NodeId]) -> Vec<NodeId> {
    let slice = visitor
        .factory_mut()
        .alloc_nodes(nodes.iter().copied().map(Some).collect());
    let (visited, _) = visitor.visit_slice(slice);
    visitor
        .factory()
        .read_nodes(visited)
        .iter()
        .map(|node| node.expect(NIL))
        .collect()
}

/// `node.ModifierFlags()`: the flags of the node's modifier list.
fn syntactic_modifier_flags(factory: &dyn RuntimeFactory, node: NodeId) -> u32 {
    let modifiers = factory.node(node).modifiers();
    modifiers.map_or(modifier_flags::NONE, |list| {
        factory.read_list(list).modifier_flags()
    })
}

/// `NodeFactory.NewNodeList(nodes)` with the location `loc`.
fn new_node_list_at(
    factory: &mut dyn RuntimeFactory,
    nodes: Vec<NodeId>,
    loc: TextRange,
) -> NodeListId {
    let list = new_node_list(factory, nodes);
    factory.set_list_location(list, loc);
    list
}

/// `NodeFactory.NewSyntaxList(nodes)`.
fn new_syntax_list(factory: &mut dyn RuntimeFactory, nodes: Vec<NodeId>) -> NodeId {
    let children = factory.alloc_nodes(nodes.into_iter().map(Some).collect());
    factory.new_syntax_list(children)
}

/// The location of a node list (`list.Loc`).
fn list_loc(factory: &dyn RuntimeFactory, list: Option<NodeListId>) -> TextRange {
    factory.read_list(list.expect(NIL)).loc()
}

/// Restores the ancestor tracking on every exit of `visit`, as upstream's
/// deferred `popNode`.
struct PopNode<'t, 'a> {
    tx: &'t RuntimeSyntaxTransformer<'a>,
    grandparent_node: Option<NodeId>,
}

impl Drop for PopNode<'_, '_> {
    fn drop(&mut self) {
        self.tx.pop_node(self.grandparent_node);
    }
}

/// Restores the scope on every exit of `visit`, as upstream's deferred
/// `popScope`.
struct PopScope<'t, 'a> {
    tx: &'t RuntimeSyntaxTransformer<'a>,
    saved_current_scope: Option<NodeId>,
    saved_current_scope_first_declarations_of_name: FirstDeclarations,
}

impl Drop for PopScope<'_, '_> {
    fn drop(&mut self) {
        self.tx.pop_scope(
            self.saved_current_scope,
            self.saved_current_scope_first_declarations_of_name.take(),
        );
    }
}

/// The innermost declaration of a dotted namespace (`namespace A.B.C`).
// port: tsc/internal/transformers/tstransforms/runtimesyntax.go:getInnermostModuleDeclarationFromDottedModule
pub(crate) fn get_innermost_module_declaration_from_dotted_module(
    factory: &dyn RuntimeFactory,
    mut module_declaration: NodeId,
) -> NodeId {
    loop {
        match factory.node(module_declaration).body() {
            Some(body) if factory.node(body).kind() == K::ModuleDeclaration => {
                module_declaration = body;
            }
            _ => return module_declaration,
        }
    }
}

impl RuntimeSyntaxTransformer<'_> {
    fn context(&self) -> EmitContext {
        self.emit_context.clone()
    }

    /// Pushes a new child node onto the ancestor tracking stack, returning the
    /// grandparent node to be restored later via `pop_node`.
    // port: tsc/internal/transformers/tstransforms/runtimesyntax.go:RuntimeSyntaxTransformer.pushNode
    fn push_node(&self, node: NodeId) -> Option<NodeId> {
        let grandparent_node = self.parent_node.get();
        self.parent_node.set(self.current_node.get());
        self.current_node.set(Some(node));
        grandparent_node
    }

    /// Pops the last child node off the ancestor tracking stack, restoring the
    /// grandparent node.
    // port: tsc/internal/transformers/tstransforms/runtimesyntax.go:RuntimeSyntaxTransformer.popNode
    fn pop_node(&self, grandparent_node: Option<NodeId>) {
        self.current_node.set(self.parent_node.get());
        self.parent_node.set(grandparent_node);
    }

    // port: tsc/internal/transformers/tstransforms/runtimesyntax.go:RuntimeSyntaxTransformer.pushScope
    fn push_scope(
        &self,
        factory: &dyn RuntimeFactory,
        node: NodeId,
    ) -> (Option<NodeId>, FirstDeclarations) {
        let saved_current_scope = self.current_scope.get();
        let saved_current_scope_first_declarations_of_name = self
            .current_scope_first_declarations_of_name
            .borrow()
            .clone();
        match factory.node(node).kind().known() {
            Some(K::SourceFile) => {
                self.current_scope.set(Some(node));
                self.current_source_file.set(Some(node));
                *self.current_scope_first_declarations_of_name.borrow_mut() = None;
            }
            Some(K::CaseBlock | K::ModuleBlock | K::Block) => {
                self.current_scope.set(Some(node));
                *self.current_scope_first_declarations_of_name.borrow_mut() = None;
            }
            Some(K::FunctionDeclaration | K::ClassDeclaration | K::VariableStatement) => {
                self.record_declaration_in_scope(factory, node);
            }
            _ => {}
        }
        (
            saved_current_scope,
            saved_current_scope_first_declarations_of_name,
        )
    }

    // port: tsc/internal/transformers/tstransforms/runtimesyntax.go:RuntimeSyntaxTransformer.popScope
    fn pop_scope(
        &self,
        saved_current_scope: Option<NodeId>,
        saved_current_scope_first_declarations_of_name: FirstDeclarations,
    ) {
        if self.current_scope.get() != saved_current_scope {
            // only reset the first declaration for a name if we are exiting the scope in which it was declared
            *self.current_scope_first_declarations_of_name.borrow_mut() =
                saved_current_scope_first_declarations_of_name;
        }

        self.current_scope.set(saved_current_scope);
    }

    /// Visits each node in the AST
    // port: tsc/internal/transformers/tstransforms/runtimesyntax.go:RuntimeSyntaxTransformer.visit
    fn visit(
        &self,
        visitor: &mut NodeVisitor<'_>,
        node: Option<NodeId>,
    ) -> Result<Option<NodeId>, Error> {
        // A constructor's body is visited here directly, once per nested class.
        stacker::maybe_grow(128 * 1024, 2 * 1024 * 1024, || {
            self.visit_worker(visitor, node)
        })
    }

    fn visit_worker(
        &self,
        visitor: &mut NodeVisitor<'_>,
        node: Option<NodeId>,
    ) -> Result<Option<NodeId>, Error> {
        let id = node.expect(NIL);
        let grandparent_node = self.push_node(id);
        let _pop_node = PopNode {
            tx: self,
            grandparent_node,
        };

        let (saved_current_scope, saved_current_scope_first_declarations_of_name) =
            self.push_scope(visitor.factory(), id);
        let _pop_scope = PopScope {
            tx: self,
            saved_current_scope,
            saved_current_scope_first_declarations_of_name,
        };

        let facts = view(visitor.factory())?.subtree_facts(id);
        if facts & subtree_flags::TYPE_SCRIPT == 0
            && (self.current_namespace.get().is_none() && self.current_enum.get().is_none()
                || facts & subtree_flags::IDENTIFIER == 0)
        {
            return Ok(node);
        }

        let kind = visitor.factory().node(id).kind();
        match kind.known() {
            // TypeScript parameter property modifiers are elided
            Some(
                K::PublicKeyword
                | K::PrivateKeyword
                | K::ProtectedKeyword
                | K::ReadonlyKeyword
                | K::OverrideKeyword,
            ) => Ok(None),
            Some(K::EnumDeclaration) => Ok(Some(self.visit_enum_declaration(visitor, id)?)),
            Some(K::ModuleDeclaration) => Ok(Some(self.visit_module_declaration(visitor, id)?)),
            Some(K::ClassDeclaration) => Ok(Some(self.visit_class_declaration(visitor, id)?)),
            Some(K::ClassExpression) => Ok(Some(self.visit_class_expression(visitor, id)?)),
            Some(K::Constructor) => Ok(Some(self.visit_constructor_declaration(visitor, id)?)),
            Some(K::FunctionDeclaration) => Ok(Some(self.visit_function_declaration(visitor, id)?)),
            Some(K::VariableStatement) => self.visit_variable_statement(visitor, id),
            Some(K::ExportDeclaration | K::ImportDeclaration | K::ImportClause) => {
                if self.current_namespace.get().is_some()
                    && self
                        .current_scope
                        .get()
                        .is_some_and(|scope| visitor.factory().node(scope).kind() != K::Block)
                {
                    // do not emit ES6 imports and exports since they are illegal inside a namespace
                    Ok(None)
                } else {
                    Ok(visitor.visit_each_child(node))
                }
            }
            Some(K::ImportEqualsDeclaration) => {
                let module_reference_kind = || {
                    let module_reference = visitor
                        .factory()
                        .node(id)
                        .data_source()
                        .as_import_equals_declaration()
                        .expect("interface conversion: ast.nodeData is not *ast.ImportEqualsDeclaration")
                        .to_owned()
                        .module_reference
                        .expect(NIL);
                    visitor.factory().node(module_reference).kind()
                };
                let scope_kind = self
                    .current_scope
                    .get()
                    .map(|scope| visitor.factory().node(scope).kind());
                if self.current_namespace.get().is_some()
                    && scope_kind.is_some_and(|kind| kind != K::Block)
                    && module_reference_kind() == K::ExternalModuleReference
                {
                    // do not emit ES6 imports and exports since they are illegal inside a namespace
                    Ok(None)
                } else if self.current_namespace.get().is_some()
                    && scope_kind.is_some_and(|kind| kind == K::Block)
                    && module_reference_kind() != K::ExternalModuleReference
                {
                    // inside a block within a namespace, elide internal import aliases
                    Ok(None)
                } else {
                    Ok(self.visit_import_equals_declaration(visitor, id))
                }
            }
            Some(K::Identifier) => Ok(Some(self.visit_identifier(visitor, id)?)),
            Some(K::ShorthandPropertyAssignment) => {
                Ok(Some(self.visit_shorthand_property_assignment(visitor, id)?))
            }
            _ => Ok(visitor.visit_each_child(node)),
        }
    }

    /// Records that a declaration was emitted in the current scope, if it was
    /// the first declaration for the provided symbol.
    // port: tsc/internal/transformers/tstransforms/runtimesyntax.go:RuntimeSyntaxTransformer.recordDeclarationInScope
    fn record_declaration_in_scope(&self, factory: &dyn RuntimeFactory, node: NodeId) {
        let read = factory.node(node);
        match read.kind().known() {
            Some(K::VariableStatement) => {
                let declaration_list = read
                    .data_source()
                    .as_variable_statement()
                    .expect("interface conversion: ast.nodeData is not *ast.VariableStatement")
                    .to_owned()
                    .declaration_list
                    .expect(NIL);
                drop(read);
                self.record_declaration_in_scope(factory, declaration_list);
                return;
            }
            Some(K::VariableDeclarationList) => {
                let declarations = read
                    .data_source()
                    .as_variable_declaration_list()
                    .expect(
                        "interface conversion: ast.nodeData is not *ast.VariableDeclarationList",
                    )
                    .to_owned()
                    .declarations
                    .expect(NIL);
                drop(read);
                for decl in list_nodes(factory, Some(declarations)) {
                    self.record_declaration_in_scope(factory, decl);
                }
                return;
            }
            Some(K::ArrayBindingPattern | K::ObjectBindingPattern) => {
                let elements = read.element_list();
                drop(read);
                for element in list_nodes(factory, elements) {
                    self.record_declaration_in_scope(factory, element);
                }
                return;
            }
            _ => {}
        }
        let name = read.name();
        drop(read);
        if let Some(name) = name {
            let name_read = factory.node(name);
            if is_identifier(&name_read) {
                drop(name_read);
                let mut slot = self.current_scope_first_declarations_of_name.borrow_mut();
                let map = slot.get_or_insert_with(Rc::default);
                let text = identifier_text(factory, name);
                map.borrow_mut().entry(text).or_insert(node);
            } else if is_binding_pattern(&name_read) {
                drop(name_read);
                self.record_declaration_in_scope(factory, name);
            }
        }
    }

    /// Determines whether a declaration is the first declaration with the same
    /// name emitted in the current scope.
    // port: tsc/internal/transformers/tstransforms/runtimesyntax.go:RuntimeSyntaxTransformer.isFirstDeclarationInScope
    fn is_first_declaration_in_scope(&self, factory: &dyn RuntimeFactory, node: NodeId) -> bool {
        let name = factory.node(node).name();
        if let Some(name) = name {
            if is_identifier(&factory.node(name)) {
                let text = identifier_text(factory, name);
                let slot = self.current_scope_first_declarations_of_name.borrow();
                if let Some(first_declaration) = slot
                    .as_ref()
                    .and_then(|map| map.borrow().get(&text).copied())
                {
                    return first_declaration == node;
                }
            }
        }
        false
    }

    // port: tsc/internal/transformers/tstransforms/runtimesyntax.go:RuntimeSyntaxTransformer.isExportOfNamespace
    fn is_export_of_namespace(&self, factory: &dyn RuntimeFactory, node: NodeId) -> bool {
        self.current_namespace.get().is_some()
            && self
                .current_scope
                .get()
                .is_none_or(|scope| factory.node(scope).kind() != K::Block)
            && syntactic_modifier_flags(factory, node) & modifier_flags::EXPORT != 0
    }

    /// Gets an expression that represents a property name, such as `"foo"` for
    /// the identifier `foo`.
    // port: tsc/internal/transformers/tstransforms/runtimesyntax.go:RuntimeSyntaxTransformer.getExpressionForPropertyName
    #[allow(clippy::unused_self)] // upstream's method
    fn get_expression_for_property_name(
        &self,
        visitor: &mut NodeVisitor<'_>,
        member: NodeId,
    ) -> Option<NodeId> {
        let name = visitor.factory().node(member).name().expect(NIL);
        let read = visitor.factory().node(name);
        match read.kind().known() {
            Some(K::PrivateIdentifier) => {
                drop(read);
                Some(visitor.factory_mut().new_identifier(JsString::default()))
            }
            Some(K::ComputedPropertyName) => {
                // enums don't support computed properties so we always generate the 'expression' part of the name as-is.
                let expression = read.expression();
                drop(read);
                visitor.visit_node(expression)
            }
            Some(K::Identifier) => {
                drop(read);
                let text = identifier_text(visitor.factory(), name);
                Some(
                    visitor
                        .factory_mut()
                        .new_string_literal(text, token_flags::NONE),
                )
            }
            // !!! propagate token flags (will produce new diffs)
            Some(K::StringLiteral) => {
                let text = read
                    .as_string_literal()
                    .expect("interface conversion: ast.nodeData is not *ast.StringLiteral")
                    .text_owned();
                drop(read);
                Some(
                    visitor
                        .factory_mut()
                        .new_string_literal(text, token_flags::NONE),
                )
            }
            Some(K::NumericLiteral) => {
                let text = read
                    .as_numeric_literal()
                    .expect("interface conversion: ast.nodeData is not *ast.NumericLiteral")
                    .text_owned();
                drop(read);
                Some(
                    visitor
                        .factory_mut()
                        .new_numeric_literal(text, token_flags::NONE),
                )
            }
            _ => Some(name),
        }
    }

    /// Gets an expression like `E["A"]` that references an enum member.
    // port: tsc/internal/transformers/tstransforms/runtimesyntax.go:RuntimeSyntaxTransformer.getEnumQualifiedElement
    fn get_enum_qualified_element(
        &self,
        visitor: &mut NodeVisitor<'_>,
        enum_declaration: NodeId,
        member: NodeId,
    ) -> NodeId {
        let ns = self.get_namespace_container_name(visitor, enum_declaration);
        let expression = self.get_expression_for_property_name(visitor, member);
        let prop = self.get_namespace_qualified_element(visitor, ns, expression);
        self.context().add_emit_flags(
            prop,
            emit_flags::NO_COMMENTS
                | emit_flags::NO_NESTED_COMMENTS
                | emit_flags::NO_SOURCE_MAP
                | emit_flags::NO_NESTED_SOURCE_MAPS,
        );
        prop
    }

    /// Gets an expression used to refer to a namespace or enum from within the
    /// body of its declaration.
    // port: tsc/internal/transformers/tstransforms/runtimesyntax.go:RuntimeSyntaxTransformer.getNamespaceContainerName
    fn get_namespace_container_name(&self, visitor: &mut NodeVisitor<'_>, node: NodeId) -> NodeId {
        self.context()
            .new_generated_name_for_node(visitor.factory_mut(), node)
    }

    /// Gets an expression used to refer to an export of a namespace or a
    /// member of an enum by property name.
    // port: tsc/internal/transformers/tstransforms/runtimesyntax.go:RuntimeSyntaxTransformer.getNamespaceQualifiedProperty
    fn get_namespace_qualified_property(
        &self,
        visitor: &mut NodeVisitor<'_>,
        ns: NodeId,
        name: NodeId,
    ) -> NodeId {
        self.context().get_namespace_member_name(
            visitor.factory_mut(),
            ns,
            name,
            NameOptions {
                allow_source_maps: true,
                ..NameOptions::default()
            },
        )
    }

    /// Gets an expression used to refer to an export of a namespace or a
    /// member of an enum by indexed access.
    // port: tsc/internal/transformers/tstransforms/runtimesyntax.go:RuntimeSyntaxTransformer.getNamespaceQualifiedElement
    fn get_namespace_qualified_element(
        &self,
        visitor: &mut NodeVisitor<'_>,
        ns: NodeId,
        expression: Option<NodeId>,
    ) -> NodeId {
        let qualified_name = visitor.factory_mut().new_element_access_expression(
            Some(ns),
            None, /*questionDotToken*/
            expression,
            node_flags::NONE,
        );
        self.context().assign_comment_and_source_map_ranges(
            visitor.factory(),
            qualified_name,
            expression.expect(NIL),
        );
        qualified_name
    }

    /// Gets an expression used within the provided node's container for any
    /// exported references.
    // port: tsc/internal/transformers/tstransforms/runtimesyntax.go:RuntimeSyntaxTransformer.getExportQualifiedReferenceToDeclaration
    fn get_export_qualified_reference_to_declaration(
        &self,
        visitor: &mut NodeVisitor<'_>,
        node: NodeId,
    ) -> Result<NodeId, Error> {
        if self.is_export_of_namespace(visitor.factory(), node) {
            let namespace = self.current_namespace.get().expect(NIL);
            let ns = self.get_namespace_container_name(visitor, namespace);
            return Ok(self
                .context()
                .get_external_module_or_namespace_export_name(
                    builder(visitor)?,
                    Some(ns),
                    node,
                    false, /*allowComments*/
                    true,  /*allowSourceMaps*/
                )?);
        }
        Ok(self.context().get_declaration_name_ex(
            builder(visitor)?,
            Some(node),
            NameOptions {
                allow_source_maps: true,
                ..NameOptions::default()
            },
        )?)
    }

    // port: tsc/internal/transformers/tstransforms/runtimesyntax.go:RuntimeSyntaxTransformer.addVarForDeclaration
    fn add_var_for_declaration(
        &self,
        visitor: &mut NodeVisitor<'_>,
        statements: &mut Vec<NodeId>,
        node: NodeId,
    ) -> Result<bool, Error> {
        self.record_declaration_in_scope(visitor.factory(), node);
        if !self.is_first_declaration_in_scope(visitor.factory(), node) {
            return Ok(false);
        }

        let mut context = self.context();
        // var name;
        let name = context.get_local_name_ex(
            builder(visitor)?,
            Some(node),
            AssignedNameOptions {
                allow_source_maps: true,
                ..AssignedNameOptions::default()
            },
        )?;
        let factory = visitor.factory_mut();
        let var_decl = factory.new_variable_declaration(Some(name), None, None, None);
        let var_flags = if self.current_scope.get() == self.current_source_file.get() {
            node_flags::NONE
        } else {
            node_flags::LET
        };
        let declarations = new_node_list(factory, vec![var_decl]);
        let var_decls = factory.new_variable_declaration_list(Some(declarations), var_flags);
        // Replicate modifierVisitor: strip decorators, TypeScript modifiers, and export when in namespace.
        let mut modifier_mask = !(modifier_flags::TYPE_SCRIPT_MODIFIER | modifier_flags::DECORATOR);
        if self.current_namespace.get().is_some() {
            modifier_mask &= !modifier_flags::EXPORT;
        }
        let node_modifiers = factory.node(node).modifiers();
        let modifiers = extract_modifiers(&context, factory, node_modifiers, modifier_mask);
        let var_statement = factory.new_variable_statement(modifiers, Some(var_decls));

        context.set_original(var_decl, node);
        // !!! synthetic comments
        context.set_original(var_statement, node);

        // Adjust the source map emit to match the old emitter.
        let (is_enum, loc) = {
            let read = factory.node(node);
            (is_enum_declaration(&read), read.range())
        };
        if is_enum {
            context.set_source_map_range(var_decls, loc);
        } else {
            context.set_source_map_range(var_statement, loc);
        }

        // Trailing comments for enum declaration should be emitted after the function closure
        // instead of the variable statement:
        //
        //     /** Leading comment*/
        //     enum E {
        //         A
        //     } // trailing comment
        //
        // Should emit:
        //
        //     /** Leading comment*/
        //     var E;
        //     (function (E) {
        //         E[E["A"] = 0] = "A";
        //     })(E || (E = {})); // trailing comment
        //
        context.set_comment_range(var_statement, loc);
        context.add_emit_flags(var_statement, emit_flags::NO_TRAILING_COMMENTS);
        statements.push(var_statement);

        Ok(true)
    }

    /// The emit flags of an enum or namespace statement: no leading comments
    /// when a leading `var` was emitted, unless the declaration is at the top
    /// of a System module.
    fn declaration_statement_emit_flags(&self, var_added: bool) -> EmitFlags {
        let mut flags = emit_flags::NONE;
        if var_added
            && (self.compiler_options.emit_module_kind() != ModuleKind::SYSTEM
                || self.current_scope.get() != self.current_source_file.get())
        {
            flags |= emit_flags::NO_LEADING_COMMENTS;
        }
        flags
    }

    /// `x || (x = {})`, `exports.x || (exports.x = {})`, and
    /// `x = (exports.x || (exports.x = {}))` for an export of a namespace.
    fn container_argument(
        &self,
        visitor: &mut NodeVisitor<'_>,
        node: NodeId,
    ) -> Result<NodeId, Error> {
        let context = self.context();
        let left = self.get_export_qualified_reference_to_declaration(visitor, node)?;
        let target = self.get_export_qualified_reference_to_declaration(visitor, node)?;
        let properties = new_node_list(visitor.factory_mut(), Vec::new());
        let factory = visitor.factory_mut();
        let object = factory.new_object_literal_expression(Some(properties), false);
        let assignment = context.new_assignment_expression(factory, target, object);
        let mut argument = context.new_logical_or_expression(factory, left, assignment);

        if self.is_export_of_namespace(visitor.factory(), node) {
            // `localName` is the expression used within this node's containing scope for any local references.
            let local_name = self.context().get_local_name_ex(
                builder(visitor)?,
                Some(node),
                AssignedNameOptions {
                    allow_source_maps: true,
                    ..AssignedNameOptions::default()
                },
            )?;

            //  x = (exports.x || (exports.x = {}))
            argument =
                context.new_assignment_expression(visitor.factory_mut(), local_name, argument);
        }
        Ok(argument)
    }

    /// `(function (name) { ... })(argument)` as a statement, with the
    /// original, ranges and emit flags of `node`. `body` builds the function
    /// body after the parameter.
    fn new_immediately_invoked_function_statement(
        &self,
        visitor: &mut NodeVisitor<'_>,
        node: NodeId,
        argument: NodeId,
        emit_flags: EmitFlags,
        body: impl FnOnce(&Self, &mut NodeVisitor<'_>) -> Result<NodeId, Error>,
    ) -> Result<NodeId, Error> {
        let mut context = self.context();
        // (function (name) { ... })(name || (name = {}))
        let param_name = context.new_generated_name_for_node(visitor.factory_mut(), node);
        let name_loc = {
            let name = visitor.factory().node(node).name().expect(NIL);
            visitor.factory().node(name).range()
        };
        context.set_source_map_range(param_name, name_loc);

        let param = visitor.factory_mut().new_parameter_declaration(
            None,
            None,
            Some(param_name),
            None,
            None,
            None,
        );
        let body = body(self, visitor)?;
        let factory = visitor.factory_mut();
        let parameters = new_node_list(factory, vec![param]);
        let function = factory.new_function_expression(
            None,
            None,
            None,
            None,
            Some(parameters),
            None,
            None,
            Some(body),
        );
        let callee = factory.new_parenthesized_expression(Some(function));
        let arguments = new_node_list(factory, vec![argument]);
        let call = factory.new_call_expression(
            Some(callee),
            None,
            None,
            Some(arguments),
            node_flags::NONE,
        );
        let statement = factory.new_expression_statement(Some(call));
        context.set_original(statement, node);
        context.assign_comment_and_source_map_ranges(visitor.factory(), statement, node);
        context.add_emit_flags(statement, emit_flags);
        Ok(statement)
    }

    // port: tsc/internal/transformers/tstransforms/runtimesyntax.go:RuntimeSyntaxTransformer.visitEnumDeclaration
    fn visit_enum_declaration(
        &self,
        visitor: &mut NodeVisitor<'_>,
        node: NodeId,
    ) -> Result<NodeId, Error> {
        if !self.should_emit_enum_declaration(visitor, node)? {
            return Ok(self
                .context()
                .new_not_emitted_statement(visitor.factory_mut(), node));
        }

        let mut statements = Vec::new();

        // If needed, we should emit a variable declaration for the enum:
        //  var name;
        let var_added = self.add_var_for_declaration(visitor, &mut statements, node)?;

        // If we emit a leading variable declaration, we should not emit leading comments for the enum body, but we should
        // still emit the comments if we are emitting to a System module.
        let emit_flags = self.declaration_statement_emit_flags(var_added);

        //  x || (x = {})
        //  exports.x || (exports.x = {})
        let enum_arg = self.container_argument(visitor, node)?;

        let enum_statement = self.new_immediately_invoked_function_statement(
            visitor,
            node,
            enum_arg,
            emit_flags,
            |tx, visitor| tx.transform_enum_body(visitor, node),
        )?;
        statements.push(enum_statement);
        Ok(new_syntax_list(visitor.factory_mut(), statements))
    }

    /// Transforms the body of an enum declaration.
    // port: tsc/internal/transformers/tstransforms/runtimesyntax.go:RuntimeSyntaxTransformer.transformEnumBody
    fn transform_enum_body(
        &self,
        visitor: &mut NodeVisitor<'_>,
        node: NodeId,
    ) -> Result<NodeId, Error> {
        let saved_current_enum = self.current_enum.get();
        self.current_enum.set(Some(node));

        // visit the children of `node` in advance to capture any references to enum members
        let node = visitor.visit_each_child(Some(node)).expect(NIL);
        let members = visitor
            .factory()
            .node(node)
            .data_source()
            .as_enum_declaration()
            .expect("interface conversion: ast.nodeData is not *ast.EnumDeclaration")
            .to_owned()
            .members;
        let member_count = list_nodes(visitor.factory(), Some(members.expect(NIL))).len();

        let mut statements = Vec::new();
        for i in 0..member_count {
            //  E[E["A"] = 0] = "A";
            self.transform_enum_member(visitor, &mut statements, node, i)?;
        }

        let loc = list_loc(visitor.factory(), members);
        let statement_list = new_node_list_at(visitor.factory_mut(), statements, loc);

        self.current_enum.set(saved_current_enum);
        Ok(visitor
            .factory_mut()
            .new_block(Some(statement_list), true /*multiline*/))
    }

    /// Transforms an enum member into a statement. It is expected that `enum`
    /// has already been visited.
    // port: tsc/internal/transformers/tstransforms/runtimesyntax.go:RuntimeSyntaxTransformer.transformEnumMember
    fn transform_enum_member(
        &self,
        visitor: &mut NodeVisitor<'_>,
        statements: &mut Vec<NodeId>,
        enum_declaration: NodeId,
        index: usize,
    ) -> Result<(), Error> {
        let member_node = {
            let members = visitor
                .factory()
                .node(enum_declaration)
                .data_source()
                .as_enum_declaration()
                .expect("interface conversion: ast.nodeData is not *ast.EnumDeclaration")
                .to_owned()
                .members
                .expect(NIL);
            let factory = visitor.factory();
            factory
                .read_nodes(factory.read_list(members).nodes())
                .at(index)
                .expect(NIL)
        };
        let member = visitor
            .factory()
            .node(member_node)
            .data_source()
            .as_enum_member()
            .expect("interface conversion: ast.nodeData is not *ast.EnumMember")
            .to_owned();

        let saved_parent = self.parent_node.get();
        self.parent_node.set(self.current_node.get());
        self.current_node.set(Some(member_node));

        //  E[E["A"] = x] = "A";
        //             ^
        let mut expression = member.initializer; // NOTE: already visited

        let use_explicit_reverse_mapping;

        let mut context = self.context();
        // `EmitResolver.GetEnumMemberValue` dereferences a nil parse node.
        let parse_node = context
            .parse_node(visitor.factory(), member_node)
            .expect(NIL);
        let result = self
            .emit_resolver
            .borrow_mut()
            .get_enum_member_value(parse_node)?;
        match &result.value {
            Some(value @ ConstantValue::Number(_)) => {
                expression = Some(constant_expression(value, visitor.factory_mut()));
                use_explicit_reverse_mapping = true;
            }
            Some(value @ ConstantValue::String(_)) => {
                expression = Some(constant_expression(value, visitor.factory_mut()));
                use_explicit_reverse_mapping = false;
            }
            None => {
                if expression.is_none() {
                    expression = Some(context.new_void_zero_expression(visitor.factory_mut()));
                }
                use_explicit_reverse_mapping = !result.is_syntactically_string;
            }
        }
        let mut expression = expression.expect(NIL);

        // Define the enum member property:
        //  E[E["A"] = 0] = "A";
        //    ^^^^^^^^--_____
        let target = self.get_enum_qualified_element(visitor, enum_declaration, member_node);
        expression = context.new_assignment_expression(visitor.factory_mut(), target, expression);

        if use_explicit_reverse_mapping {
            //  E[E["A"] = 0] = "A";
            //  ^^--------------^^^^^
            let ns = self.get_namespace_container_name(visitor, enum_declaration);
            let element = visitor.factory_mut().new_element_access_expression(
                Some(ns),
                None, /*questionDotToken*/
                Some(expression),
                node_flags::NONE,
            );
            let name = self
                .get_expression_for_property_name(visitor, member_node)
                .expect(NIL);
            expression = context.new_assignment_expression(visitor.factory_mut(), element, name);
        }

        let member_statement = visitor
            .factory_mut()
            .new_expression_statement(Some(expression));
        context.assign_comment_and_source_map_ranges(visitor.factory(), expression, member_node);
        context.assign_comment_and_source_map_ranges(
            visitor.factory(),
            member_statement,
            member_node,
        );
        statements.push(member_statement);

        self.current_node.set(self.parent_node.get());
        self.parent_node.set(saved_parent);
        Ok(())
    }

    // port: tsc/internal/transformers/tstransforms/runtimesyntax.go:RuntimeSyntaxTransformer.visitModuleDeclaration
    fn visit_module_declaration(
        &self,
        visitor: &mut NodeVisitor<'_>,
        node: NodeId,
    ) -> Result<NodeId, Error> {
        if !self.should_emit_module_declaration(visitor, node)? {
            return Ok(self
                .context()
                .new_not_emitted_statement(visitor.factory_mut(), node));
        }

        let mut statements = Vec::new();

        // If needed, we should emit a variable declaration for the module:
        //  var name;
        let var_added = self.add_var_for_declaration(visitor, &mut statements, node)?;

        // If we emit a leading variable declaration, we should not emit leading comments for the module body, but we should
        // still emit the comments if we are emitting to a System module.
        let emit_flags = self.declaration_statement_emit_flags(var_added);

        //  x || (x = {})
        //  exports.x || (exports.x = {})
        let module_arg = self.container_argument(visitor, node)?;

        let module_statement = self.new_immediately_invoked_function_statement(
            visitor,
            node,
            module_arg,
            emit_flags,
            |tx, visitor| {
                let namespace_local_name = tx.get_namespace_container_name(visitor, node);
                Ok(tx.transform_module_body(visitor, node, namespace_local_name))
            },
        )?;
        statements.push(module_statement);
        Ok(new_syntax_list(visitor.factory_mut(), statements))
    }

    /// Upstream takes the namespace's local name and does not read it.
    // port: tsc/internal/transformers/tstransforms/runtimesyntax.go:RuntimeSyntaxTransformer.transformModuleBody
    fn transform_module_body(
        &self,
        visitor: &mut NodeVisitor<'_>,
        mut node: NodeId,
        _namespace_local_name: NodeId,
    ) -> NodeId {
        let saved_current_namespace = self.current_namespace.get();
        let saved_current_scope = self.current_scope.get();
        let saved_current_scope_first_declarations_of_name = self
            .current_scope_first_declarations_of_name
            .borrow()
            .clone();

        self.current_namespace.set(Some(node));
        *self.current_scope_first_declarations_of_name.borrow_mut() = None;

        let mut statements = Vec::new();
        let mut context = self.context();
        context.start_variable_environment();

        // Go's zero `core.TextRange`.
        let mut statements_location = TextRange::new(0, 0);
        let mut block_location = TextRange::new(0, 0);
        let body = visitor.factory().node(node).body();
        if let Some(body) = body {
            if visitor.factory().node(body).kind() == K::ModuleBlock {
                // visit the children of `node` in advance to capture any references to namespace members
                node = visitor.visit_each_child(Some(node)).expect(NIL);
                let body = visitor.factory().node(node).body().expect(NIL);
                let body_statements = visitor
                    .factory()
                    .node(body)
                    .data_source()
                    .as_module_block()
                    .expect("interface conversion: ast.nodeData is not *ast.ModuleBlock")
                    .to_owned()
                    .statements;
                statements = list_nodes(visitor.factory(), Some(body_statements.expect(NIL)));
                statements_location = list_loc(visitor.factory(), body_statements);
                block_location = visitor.factory().node(body).range();
            } else {
                // node.Body.Kind == ast.KindModuleDeclaration
                // !!! Strada didn't do this; why?
                // tx.currentScope = node.AsNode()
                statements = visit_slice(visitor, &[body]);
                let innermost =
                    get_innermost_module_declaration_from_dotted_module(visitor.factory(), node);
                let module_block = visitor.factory().node(innermost).body().expect(NIL);
                let block_statements = visitor
                    .factory()
                    .node(module_block)
                    .data_source()
                    .as_module_block()
                    .expect("interface conversion: ast.nodeData is not *ast.ModuleBlock")
                    .to_owned()
                    .statements;
                statements_location = list_loc(visitor.factory(), block_statements).with_pos(-1);
            }
        }

        self.current_namespace.set(saved_current_namespace);
        self.current_scope.set(saved_current_scope);
        *self.current_scope_first_declarations_of_name.borrow_mut() =
            saved_current_scope_first_declarations_of_name;

        let statements =
            context.end_and_merge_variable_environment(visitor.factory_mut(), statements);
        let factory = visitor.factory_mut();
        let statement_list = new_node_list_at(factory, statements, statements_location);
        let block = factory.new_block(Some(statement_list), true /*multiline*/);
        factory.set_node_range(block, block_location);

        //  namespace hello.hi.world {
        //       function foo() {}
        //
        //       // TODO, blah
        //  }
        //
        // should be emitted as
        //
        //  var hello;
        //  (function (hello) {
        //      var hi;
        //      (function (hi) {
        //          var world;
        //          (function (world) {
        //              function foo() { }
        //              // TODO, blah
        //          })(world = hi.world || (hi.world = {}));
        //      })(hi = hello.hi || (hello.hi = {}));
        //  })(hello || (hello = {}));
        //
        // We only want to emit comment on the namespace which contains block body itself, not the containing namespaces.
        let body = factory.node(node).body();
        if body.is_none_or(|body| factory.node(body).kind() != K::ModuleBlock) {
            context.add_emit_flags(block, emit_flags::NO_COMMENTS);
        }
        block
    }

    // port: tsc/internal/transformers/tstransforms/runtimesyntax.go:RuntimeSyntaxTransformer.visitImportEqualsDeclaration
    // Upstream's branch order: the non-exported case first.
    #[allow(clippy::if_not_else)]
    fn visit_import_equals_declaration(
        &self,
        visitor: &mut NodeVisitor<'_>,
        node: NodeId,
    ) -> Option<NodeId> {
        let n = visitor
            .factory()
            .node(node)
            .data_source()
            .as_import_equals_declaration()
            .expect("interface conversion: ast.nodeData is not *ast.ImportEqualsDeclaration")
            .to_owned();
        let module_reference = n.module_reference.expect(NIL);
        if visitor.factory().node(module_reference).kind() == K::ExternalModuleReference {
            return visitor.visit_each_child(Some(node));
        }

        let mut context = self.context();
        let module_reference =
            context.create_expression_from_entity_name(visitor.factory_mut(), module_reference);
        context.set_emit_flags(
            module_reference,
            emit_flags::NO_COMMENTS | emit_flags::NO_NESTED_COMMENTS,
        );
        if !self.is_export_of_namespace(visitor.factory(), node) {
            //  export var ${name} = ${moduleReference};
            //  var ${name} = ${moduleReference};
            let factory = visitor.factory_mut();
            let var_decl = factory.new_variable_declaration(
                n.name,
                None, /*exclamationToken*/
                None, /*type*/
                Some(module_reference),
            );
            context.set_original(var_decl, node);
            let declarations = new_node_list(factory, vec![var_decl]);
            let var_list =
                factory.new_variable_declaration_list(Some(declarations), node_flags::NONE);
            let var_modifiers =
                extract_modifiers(&context, factory, n.modifiers, modifier_flags::EXPORT);
            let var_statement = factory.new_variable_statement(var_modifiers, Some(var_list));
            context.set_original(var_statement, node);
            context.assign_comment_and_source_map_ranges(visitor.factory(), var_statement, node);
            Some(var_statement)
        } else {
            // exports.${name} = ${moduleReference};
            let loc = visitor.factory().node(node).range();
            let statement = self.create_export_statement(
                visitor,
                n.name.expect(NIL),
                module_reference,
                loc,
                loc,
                node,
            );
            visitor.factory_mut().set_node_range(statement, loc);
            Some(statement)
        }
    }

    // port: tsc/internal/transformers/tstransforms/runtimesyntax.go:RuntimeSyntaxTransformer.visitVariableStatement
    fn visit_variable_statement(
        &self,
        visitor: &mut NodeVisitor<'_>,
        node: NodeId,
    ) -> Result<Option<NodeId>, Error> {
        if self.is_export_of_namespace(visitor.factory(), node) {
            let mut expressions = Vec::new();
            let declaration_list = visitor
                .factory()
                .node(node)
                .data_source()
                .as_variable_statement()
                .expect("interface conversion: ast.nodeData is not *ast.VariableStatement")
                .to_owned()
                .declaration_list
                .expect(NIL);
            let declarations = visitor
                .factory()
                .node(declaration_list)
                .data_source()
                .as_variable_declaration_list()
                .expect("interface conversion: ast.nodeData is not *ast.VariableDeclarationList")
                .to_owned()
                .declarations;
            for declaration in list_nodes(visitor.factory(), Some(declarations.expect(NIL))) {
                let v = visitor
                    .factory()
                    .node(declaration)
                    .data_source()
                    .as_variable_declaration()
                    .expect("interface conversion: ast.nodeData is not *ast.VariableDeclaration")
                    .to_owned();
                if v.initializer.is_none() {
                    continue;
                }
                let name = v.name.expect(NIL);
                if is_binding_pattern(&visitor.factory().node(name)) {
                    let visited = visitor.visit_node(Some(declaration)).expect(NIL);
                    let callback =
                        |visitor: &mut NodeVisitor<'_>,
                         export_name: NodeId,
                         export_value: NodeId,
                         location: Option<TextRange>| {
                            self.create_namespace_export_expression(
                                visitor,
                                export_name,
                                export_value,
                                location,
                            )
                        };
                    let expression = flatten_destructuring_assignment(
                        visitor,
                        &self.emit_context,
                        visited,
                        false, /*needsValue*/
                        FlattenLevel::All,
                        Some(&callback),
                    )?;
                    if let Some(expression) = expression {
                        expressions.push(expression);
                    }
                } else {
                    let expression = convert_variable_declaration_to_assignment_expression(
                        &self.emit_context,
                        visitor.factory_mut(),
                        declaration,
                    );
                    if let Some(expression) = expression {
                        expressions.push(expression);
                    }
                }
            }
            if expressions.is_empty() {
                return Ok(None);
            }
            let mut context = self.context();
            let expression = context
                .inline_expressions(visitor.factory_mut(), &expressions)
                .expect(NIL);
            let statement = visitor
                .factory_mut()
                .new_expression_statement(Some(expression));
            context.set_original(statement, node);
            context.assign_comment_and_source_map_ranges(visitor.factory(), statement, node);

            // re-visit as the new node
            let saved_current = self.current_node.get();
            self.current_node.set(Some(statement));
            let statement = visitor.visit_each_child(Some(statement));
            self.current_node.set(saved_current);
            return Ok(statement);
        }
        Ok(visitor.visit_each_child(Some(node)))
    }

    /// Creates an assignment to a namespace member for use as a callback
    /// during destructuring flattening.
    // port: tsc/internal/transformers/tstransforms/runtimesyntax.go:RuntimeSyntaxTransformer.createNamespaceExportExpression
    fn create_namespace_export_expression(
        &self,
        visitor: &mut NodeVisitor<'_>,
        export_name: NodeId,
        export_value: NodeId,
        location: Option<TextRange>,
    ) -> NodeId {
        let namespace = self.current_namespace.get().expect(NIL);
        let ns = self.get_namespace_container_name(visitor, namespace);
        let member_name = self.get_namespace_qualified_property(visitor, ns, export_name);
        let expression = self.context().new_assignment_expression(
            visitor.factory_mut(),
            member_name,
            export_value,
        );
        if let Some(location) = location {
            visitor.factory_mut().set_node_range(expression, location);
        }
        expression
    }

    // port: tsc/internal/transformers/tstransforms/runtimesyntax.go:RuntimeSyntaxTransformer.visitFunctionDeclaration
    fn visit_function_declaration(
        &self,
        visitor: &mut NodeVisitor<'_>,
        node: NodeId,
    ) -> Result<NodeId, Error> {
        if self.is_export_of_namespace(visitor.factory(), node) {
            let n = visitor
                .factory()
                .node(node)
                .data_source()
                .as_function_declaration()
                .expect("interface conversion: ast.nodeData is not *ast.FunctionDeclaration")
                .to_owned();
            let extracted = extract_modifiers(
                &self.emit_context,
                visitor.factory_mut(),
                n.modifiers,
                !modifier_flags::EXPORT,
            );
            let modifiers = visitor.visit_modifiers(extracted);
            let name = visitor.visit_node(n.name);
            let parameters = visitor.visit_nodes(n.parameters);
            let body = visitor.visit_node(n.body);
            let updated = visitor.factory_mut().update_function_declaration(
                node,
                modifiers,
                n.asterisk_token,
                name,
                None, /*typeParameters*/
                parameters,
                None, /*returnType*/
                None, /*fullSignature*/
                body,
            );
            let export = self.create_export_statement_for_declaration(visitor, node)?;
            return Ok(new_syntax_list(
                visitor.factory_mut(),
                vec![updated, export],
            ));
        }
        Ok(visitor.visit_each_child(Some(node)).expect(NIL))
    }

    // port: tsc/internal/transformers/tstransforms/runtimesyntax.go:RuntimeSyntaxTransformer.getParameterProperties
    #[allow(clippy::unused_self)] // upstream's method
    fn get_parameter_properties(
        &self,
        visitor: &NodeVisitor<'_>,
        constructor: Option<NodeId>,
    ) -> Result<Vec<NodeId>, Error> {
        let mut parameter_properties = Vec::new();
        if let Some(constructor) = constructor {
            let parameters = visitor.factory().node(constructor).parameter_list();
            for parameter in list_nodes(visitor.factory(), parameters) {
                if is_parameter_property_declaration(
                    view(visitor.factory())?,
                    parameter,
                    constructor,
                )? {
                    parameter_properties.push(parameter);
                }
            }
        }
        Ok(parameter_properties)
    }

    /// The members of a class with a property declaration prepended for each
    /// identifier-named parameter property of its constructor: the shared
    /// tail of `visitClassDeclaration` and `visitClassExpression`.
    fn add_parameter_property_declarations(
        &self,
        visitor: &mut NodeVisitor<'_>,
        original_members: Option<NodeListId>,
        mut members: Option<NodeListId>,
    ) -> Result<Option<NodeListId>, Error> {
        let constructor = list_nodes(visitor.factory(), original_members)
            .into_iter()
            .find(|&member| is_constructor_declaration(&visitor.factory().node(member)));
        let parameter_properties = self.get_parameter_properties(visitor, constructor)?;

        if !parameter_properties.is_empty() {
            let mut new_members = Vec::new();
            for parameter in parameter_properties {
                let name = visitor.factory().node(parameter).name().expect(NIL);
                if is_identifier(&visitor.factory().node(name)) {
                    let factory = visitor.factory_mut();
                    let name = tsr_ast::clone_node(factory, name);
                    let parameter_property = factory.new_property_declaration(
                        None, /*modifiers*/
                        Some(name),
                        None, /*questionOrExclamationToken*/
                        None, /*type*/
                        None, /*initializer*/
                    );
                    self.context().set_original(parameter_property, parameter);
                    new_members.push(parameter_property);
                }
            }
            if !new_members.is_empty() {
                new_members.extend(list_nodes(visitor.factory(), Some(members.expect(NIL))));
                let loc = list_loc(visitor.factory(), original_members);
                members = Some(new_node_list_at(visitor.factory_mut(), new_members, loc));
            }
        }
        Ok(members)
    }

    // port: tsc/internal/transformers/tstransforms/runtimesyntax.go:RuntimeSyntaxTransformer.visitClassDeclaration
    fn visit_class_declaration(
        &self,
        visitor: &mut NodeVisitor<'_>,
        node: NodeId,
    ) -> Result<NodeId, Error> {
        let n = visitor
            .factory()
            .node(node)
            .data_source()
            .as_class_declaration()
            .expect("interface conversion: ast.nodeData is not *ast.ClassDeclaration")
            .to_owned();
        let exported = self.is_export_of_namespace(visitor.factory(), node);
        let modifiers = if exported {
            let extracted = extract_modifiers(
                &self.emit_context,
                visitor.factory_mut(),
                n.modifiers,
                !modifier_flags::EXPORT_DEFAULT,
            );
            visitor.visit_modifiers(extracted)
        } else {
            visitor.visit_modifiers(n.modifiers)
        };

        let mut name = visitor.visit_node(n.name);
        if name.is_none()
            && (exported
                || child_is_decorated(
                    view(visitor.factory())?,
                    self.compiler_options.experimental_decorators.is_true(),
                    node,
                    None,
                )?)
        {
            name = Some(self.get_namespace_container_name(visitor, node));
        }
        let heritage_clauses = visitor.visit_nodes(n.heritage_clauses);
        let members = visitor.visit_nodes(n.members);
        let members = self.add_parameter_property_declarations(visitor, n.members, members)?;

        let updated = visitor.factory_mut().update_class_declaration(
            node,
            modifiers,
            name,
            None, /*typeParameters*/
            heritage_clauses,
            members,
        );
        if exported {
            let export = self.create_export_statement_for_declaration(visitor, node)?;
            return Ok(new_syntax_list(
                visitor.factory_mut(),
                vec![updated, export],
            ));
        }
        Ok(updated)
    }

    // port: tsc/internal/transformers/tstransforms/runtimesyntax.go:RuntimeSyntaxTransformer.visitClassExpression
    fn visit_class_expression(
        &self,
        visitor: &mut NodeVisitor<'_>,
        node: NodeId,
    ) -> Result<NodeId, Error> {
        let n = visitor
            .factory()
            .node(node)
            .data_source()
            .as_class_expression()
            .expect("interface conversion: ast.nodeData is not *ast.ClassExpression")
            .to_owned();
        let extracted = extract_modifiers(
            &self.emit_context,
            visitor.factory_mut(),
            n.modifiers,
            !modifier_flags::EXPORT_DEFAULT,
        );
        let modifiers = visitor.visit_modifiers(extracted);
        let name = visitor.visit_node(n.name);
        let heritage_clauses = visitor.visit_nodes(n.heritage_clauses);
        let members = visitor.visit_nodes(n.members);
        let members = self.add_parameter_property_declarations(visitor, n.members, members)?;

        Ok(visitor.factory_mut().update_class_expression(
            node,
            modifiers,
            name,
            None, /*typeParameters*/
            heritage_clauses,
            members,
        ))
    }

    // port: tsc/internal/transformers/tstransforms/runtimesyntax.go:RuntimeSyntaxTransformer.visitConstructorDeclaration
    fn visit_constructor_declaration(
        &self,
        visitor: &mut NodeVisitor<'_>,
        node: NodeId,
    ) -> Result<NodeId, Error> {
        let n = visitor
            .factory()
            .node(node)
            .data_source()
            .as_constructor_declaration()
            .expect("interface conversion: ast.nodeData is not *ast.ConstructorDeclaration")
            .to_owned();
        let modifiers = visitor.visit_modifiers(n.modifiers);
        let parameters = self.context().visit_parameters(n.parameters, visitor);
        // `node.Body.AsBlock()`
        let body = n.body.expect(NIL);
        {
            let read = visitor.factory().node(body);
            if read.kind() != K::Block {
                interface_conversion(&read, "Block");
            }
        }
        let body = self.visit_constructor_body(visitor, body, node)?;
        Ok(visitor.factory_mut().update_constructor_declaration(
            node, modifiers, None, /*typeParameters*/
            parameters, None, /*returnType*/
            None, /*fullSignature*/
            body,
        ))
    }

    // port: tsc/internal/transformers/tstransforms/runtimesyntax.go:RuntimeSyntaxTransformer.visitConstructorBody
    fn visit_constructor_body(
        &self,
        visitor: &mut NodeVisitor<'_>,
        body: NodeId,
        constructor: NodeId,
    ) -> Result<Option<NodeId>, Error> {
        let parameter_properties = self.get_parameter_properties(visitor, Some(constructor))?;
        let mut context = self.context();
        if parameter_properties.is_empty() {
            return Ok(context.visit_function_body(Some(body), visitor));
        }

        let grandparent_of_body = self.push_node(body);
        let (saved_current_scope, saved_current_scope_first_declarations_of_name) =
            self.push_scope(visitor.factory(), body);

        context.start_variable_environment();
        let body_statements = visitor.factory().node(body).statement_list().expect(NIL);
        let body_nodes = list_nodes(visitor.factory(), Some(body_statements));
        let (prologue, rest) = context.split_standard_prologue(visitor.factory(), &body_nodes);
        let mut statements = prologue.to_vec();

        // Transform parameters into property assignments. Transforms this:
        //
        //  constructor (public x, public y) {
        //  }
        //
        // Into this:
        //
        //  constructor (x, y) {
        //      this.x = x;
        //      this.y = y;
        //  }
        //

        let mut parameter_property_assignments = Vec::new();
        for parameter in parameter_properties {
            let name = visitor.factory().node(parameter).name().expect(NIL);
            if !is_identifier(&visitor.factory().node(name)) {
                continue;
            }
            let factory = visitor.factory_mut();
            let name_parent = factory.node(name).parent();
            let property_name = tsr_ast::clone_node(factory, name);
            // .Parent set to get node to printback using text from original file instead of processed text; TODO: this should be achievable via EmitFlags instead
            factory.set_node_parent(property_name, name_parent);
            context.add_emit_flags(
                property_name,
                emit_flags::NO_COMMENTS | emit_flags::NO_SOURCE_MAP,
            );

            let local_name = tsr_ast::clone_node(factory, name);
            // .Parent set to get node to printback using text from original file instead of processed text; TODO: this should be achievable via EmitFlags instead
            factory.set_node_parent(local_name, name_parent);
            context.add_emit_flags(local_name, emit_flags::NO_COMMENTS);

            let this = context.new_this_expression(factory);
            let target = factory.new_property_access_expression(
                Some(this),
                None, /*questionDotToken*/
                Some(property_name),
                node_flags::NONE,
            );
            let assignment = context.new_assignment_expression(factory, target, local_name);
            let parameter_property = factory.new_expression_statement(Some(assignment));
            context.set_original(parameter_property, parameter);
            context.add_emit_flags(parameter_property, emit_flags::START_ON_NEW_LINE);
            parameter_property_assignments.push(parameter_property);
        }

        let super_path = find_super_statement_index_path(visitor.factory(), rest, 0);

        if super_path.is_empty() {
            statements.extend(parameter_property_assignments);
            statements.extend(visit_slice(visitor, rest));
        } else {
            statements.extend(self.transform_constructor_body_worker(
                visitor,
                rest,
                &super_path,
                &parameter_property_assignments,
            )?);
        }

        let statements =
            context.end_and_merge_variable_environment(visitor.factory_mut(), statements);
        let loc = list_loc(visitor.factory(), Some(body_statements));
        let statement_list = new_node_list_at(visitor.factory_mut(), statements, loc);

        self.pop_scope(
            saved_current_scope,
            saved_current_scope_first_declarations_of_name,
        );
        self.pop_node(grandparent_of_body);
        let factory = visitor.factory_mut();
        let updated = factory.new_block(Some(statement_list), true /*multiline*/);
        context.set_original(updated, body);
        let body_loc = factory.node(body).range();
        factory.set_node_range(updated, body_loc);
        Ok(Some(updated))
    }

    // port: tsc/internal/transformers/tstransforms/runtimesyntax.go:RuntimeSyntaxTransformer.transformConstructorBodyWorker
    fn transform_constructor_body_worker(
        &self,
        visitor: &mut NodeVisitor<'_>,
        statements_in: &[NodeId],
        super_path: &[usize],
        initializer_statements: &[NodeId],
    ) -> Result<Vec<NodeId>, Error> {
        let mut statements_out = Vec::new();
        let super_statement_index = super_path[0];
        let super_statement = statements_in[super_statement_index];

        // visit up to the statement containing `super`
        statements_out.extend(visit_slice(
            visitor,
            &statements_in[..super_statement_index],
        ));

        // if the statement containing `super` is a `try` statement, transform the body of the `try` block
        if is_try_statement(&visitor.factory().node(super_statement)) {
            let try_statement = super_statement;
            let try_data = visitor
                .factory()
                .node(try_statement)
                .data_source()
                .as_try_statement()
                .expect("interface conversion: ast.nodeData is not *ast.TryStatement")
                .to_owned();
            let try_block = try_data.try_block.expect(NIL);
            let try_block_data = visitor
                .factory()
                .node(try_block)
                .data_source()
                .as_block()
                .expect("interface conversion: ast.nodeData is not *ast.Block")
                .to_owned();

            // keep track of hierarchy as we descend
            let grandparent_of_try_statement = self.push_node(try_statement);
            let grandparent_of_try_block = self.push_node(try_block);
            let (saved_current_scope, saved_current_scope_first_declarations_of_name) =
                self.push_scope(visitor.factory(), try_block);

            // visit the `try` block
            let block_statements = list_nodes(
                visitor.factory(),
                Some(try_block_data.statements.expect(NIL)),
            );
            let try_block_statements = self.transform_constructor_body_worker(
                visitor,
                &block_statements,
                &super_path[1..],
                initializer_statements,
            )?;

            // restore hierarchy as we ascend to the `try` statement
            self.pop_scope(
                saved_current_scope,
                saved_current_scope_first_declarations_of_name,
            );
            self.pop_node(grandparent_of_try_block);

            let loc = list_loc(visitor.factory(), try_block_data.statements);
            let try_block_statement_list =
                new_node_list_at(visitor.factory_mut(), try_block_statements, loc);
            let updated_block = visitor.factory_mut().update_block(
                try_block,
                Some(try_block_statement_list),
                try_block_data.multi_line,
            );
            let catch_clause = visitor.visit_node(try_data.catch_clause);
            let finally_block = visitor.visit_node(try_data.finally_block);
            statements_out.push(visitor.factory_mut().update_try_statement(
                try_statement,
                Some(updated_block),
                catch_clause,
                finally_block,
            ));

            // restore hierarchy as we ascend to the parent of the `try` statement
            self.pop_node(grandparent_of_try_statement);
        } else {
            // visit the statement containing `super`
            statements_out.extend(visit_slice(
                visitor,
                &statements_in[super_statement_index..=super_statement_index],
            ));

            // insert the initializer statements
            statements_out.extend_from_slice(initializer_statements);
        }

        // visit the statements after `super`
        statements_out.extend(visit_slice(
            visitor,
            &statements_in[super_statement_index + 1..],
        ));
        Ok(statements_out)
    }

    // port: tsc/internal/transformers/tstransforms/runtimesyntax.go:RuntimeSyntaxTransformer.visitShorthandPropertyAssignment
    fn visit_shorthand_property_assignment(
        &self,
        visitor: &mut NodeVisitor<'_>,
        node: NodeId,
    ) -> Result<NodeId, Error> {
        let n = visitor
            .factory()
            .node(node)
            .data_source()
            .as_shorthand_property_assignment()
            .expect("interface conversion: ast.nodeData is not *ast.ShorthandPropertyAssignment")
            .to_owned();
        let name = n.name.expect(NIL);
        let exported_or_imported_name = self.visit_expression_identifier(visitor, name)?;
        if exported_or_imported_name != name {
            let mut expression = exported_or_imported_name;
            if n.object_assignment_initializer.is_some() {
                let equals_token = match n.equals_token {
                    Some(token) => token,
                    None => visitor.factory_mut().new_token(K::EqualsToken.into()),
                };
                let initializer = visitor.visit_node(n.object_assignment_initializer);
                expression = visitor.factory_mut().new_binary_expression(
                    None, /*modifiers*/
                    Some(expression),
                    None, /*typeNode*/
                    Some(equals_token),
                    initializer,
                );
            }

            let factory = visitor.factory_mut();
            let updated = factory.new_property_assignment(
                None, /*modifiers*/
                Some(name),
                None, /*postfixToken*/
                None, /*typeNode*/
                Some(expression),
            );
            let loc = factory.node(node).range();
            factory.set_node_range(updated, loc);
            let mut context = self.context();
            context.set_original(updated, node);
            context.assign_comment_and_source_map_ranges(visitor.factory(), updated, node);
            return Ok(updated);
        }
        let initializer = visitor.visit_node(n.object_assignment_initializer);
        Ok(visitor.factory_mut().update_shorthand_property_assignment(
            node,
            None, /*modifiers*/
            Some(exported_or_imported_name),
            None, /*postfixToken*/
            None, /*typeNode*/
            n.equals_token,
            initializer,
        ))
    }

    // port: tsc/internal/transformers/tstransforms/runtimesyntax.go:RuntimeSyntaxTransformer.visitIdentifier
    fn visit_identifier(
        &self,
        visitor: &mut NodeVisitor<'_>,
        node: NodeId,
    ) -> Result<NodeId, Error> {
        let parent = self.parent_node.get().expect(NIL);
        if is_identifier_reference(visitor.factory(), node, parent) {
            return self.visit_expression_identifier(visitor, node);
        }
        Ok(node)
    }

    // port: tsc/internal/transformers/tstransforms/runtimesyntax.go:RuntimeSyntaxTransformer.visitExpressionIdentifier
    fn visit_expression_identifier(
        &self,
        visitor: &mut NodeVisitor<'_>,
        node: NodeId,
    ) -> Result<NodeId, Error> {
        let mut context = self.context();
        if (self.current_enum.get().is_some() || self.current_namespace.get().is_some())
            && !is_generated_identifier(&context, node)
            && !is_local_name(&context, node)
        {
            let location = context.most_original(node);
            let container = self
                .resolver
                .borrow_mut()
                .get_referenced_export_container(location, false /*prefixLocals*/)?;
            if let Some(container) = container {
                let is_container = {
                    let read = visitor.factory().node(container);
                    is_enum_declaration(&read) || is_module_declaration(&read)
                };
                if is_container {
                    let container_name = self.get_namespace_container_name(visitor, container);

                    let member_name = tsr_ast::clone_node(visitor.factory_mut(), node);
                    context.set_emit_flags(
                        member_name,
                        emit_flags::NO_COMMENTS | emit_flags::NO_SOURCE_MAP,
                    );

                    let expression = context.get_namespace_member_name(
                        visitor.factory_mut(),
                        container_name,
                        member_name,
                        NameOptions {
                            allow_source_maps: true,
                            ..NameOptions::default()
                        },
                    );
                    context.assign_comment_and_source_map_ranges(
                        visitor.factory(),
                        expression,
                        node,
                    );
                    return Ok(expression);
                }
            }
        }
        Ok(node)
    }

    // port: tsc/internal/transformers/tstransforms/runtimesyntax.go:RuntimeSyntaxTransformer.createExportStatementForDeclaration
    fn create_export_statement_for_declaration(
        &self,
        visitor: &mut NodeVisitor<'_>,
        node: NodeId,
    ) -> Result<NodeId, Error> {
        let mut context = self.context();
        let namespace = self.current_namespace.get().expect(NIL);
        let ns = self.get_namespace_container_name(visitor, namespace);
        let export_name = context.get_external_module_or_namespace_export_name(
            builder(visitor)?,
            Some(ns),
            node,
            false, /*allowComments*/
            true,  /*allowSourceMaps*/
        )?;
        let local_name = context.get_local_name(builder(visitor)?, Some(node))?;
        let expression =
            context.new_assignment_expression(visitor.factory_mut(), export_name, local_name);
        let (loc, name) = {
            let read = visitor.factory().node(node);
            (read.range(), read.name())
        };
        let mut export_assignment_source_map_range = loc;
        if let Some(name) = name {
            export_assignment_source_map_range = export_assignment_source_map_range
                .with_pos(i64::from(visitor.factory().node(name).pos()));
        }
        context.set_source_map_range(expression, export_assignment_source_map_range);

        let statement = visitor
            .factory_mut()
            .new_expression_statement(Some(expression));
        let export_statement_source_map_range = loc.with_pos(-1);
        context.set_source_map_range(statement, export_statement_source_map_range);
        Ok(statement)
    }

    // port: tsc/internal/transformers/tstransforms/runtimesyntax.go:RuntimeSyntaxTransformer.createExportAssignment
    fn create_export_assignment(
        &self,
        visitor: &mut NodeVisitor<'_>,
        name: NodeId,
        expression: NodeId,
        export_assignment_source_map_range: TextRange,
        original: NodeId,
    ) -> NodeId {
        let namespace = self.current_namespace.get().expect(NIL);
        let ns = self.get_namespace_container_name(visitor, namespace);
        let export_name = self.get_namespace_qualified_property(visitor, ns, name);
        let mut context = self.context();
        let export_assignment =
            context.new_assignment_expression(visitor.factory_mut(), export_name, expression);
        context.set_original(export_assignment, original);
        context.set_source_map_range(export_assignment, export_assignment_source_map_range);
        export_assignment
    }

    // port: tsc/internal/transformers/tstransforms/runtimesyntax.go:RuntimeSyntaxTransformer.createExportStatement
    fn create_export_statement(
        &self,
        visitor: &mut NodeVisitor<'_>,
        name: NodeId,
        expression: NodeId,
        export_assignment_source_map_range: TextRange,
        export_statement_source_map_range: TextRange,
        original: NodeId,
    ) -> NodeId {
        let assignment = self.create_export_assignment(
            visitor,
            name,
            expression,
            export_assignment_source_map_range,
            original,
        );
        let export_statement = visitor
            .factory_mut()
            .new_expression_statement(Some(assignment));
        let mut context = self.context();
        context.set_original(export_statement, original);
        context.set_source_map_range(export_statement, export_statement_source_map_range);
        export_statement
    }

    // port: tsc/internal/transformers/tstransforms/runtimesyntax.go:RuntimeSyntaxTransformer.shouldEmitEnumDeclaration
    fn should_emit_enum_declaration(
        &self,
        visitor: &NodeVisitor<'_>,
        node: NodeId,
    ) -> Result<bool, Error> {
        Ok(!is_enum_const(view(visitor.factory())?, node)?
            || self.compiler_options.should_preserve_const_enums())
    }

    // port: tsc/internal/transformers/tstransforms/runtimesyntax.go:RuntimeSyntaxTransformer.shouldEmitModuleDeclaration
    fn should_emit_module_declaration(
        &self,
        visitor: &NodeVisitor<'_>,
        node: NodeId,
    ) -> Result<bool, Error> {
        let Some(pn) = self.emit_context.parse_node(visitor.factory(), node) else {
            // If we can't find a parse tree node, assume the node is instantiated.
            return Ok(true);
        };
        Ok(is_instantiated_module(
            view(visitor.factory())?,
            pn,
            self.compiler_options.should_preserve_const_enums(),
        )?)
    }
}
