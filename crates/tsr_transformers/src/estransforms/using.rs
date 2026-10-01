//! `transformers/estransforms/using.go`: lowers `using` and `await using`
//! declarations to a disposable-resource environment, a `try` block and the
//! `__addDisposableResource` and `__disposeResources` helpers. At the top
//! level of a file, imports, exports and declarations are hoisted out of the
//! `try` block.
use super::namedevaluation::{is_named_evaluation, transform_named_evaluation};
use super::utilities::{
    convert_class_declaration_to_class_expression, has_syntactic_modifier, identifier_text,
    list_nodes, new_node_list, restore_outer_expressions_all, skip_outer_expressions_all,
    subtree_facts, NIL,
};
use crate::transformer::{Error, Failure, TransformOptions, Transformer};
use crate::utilities::{
    convert_binding_pattern_to_assignment_pattern, is_generated_identifier, is_local_name,
};
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;
use tsr_ast::{
    clone_node, modifier_flags, node_flags, subtree_flags, AstBuilder, Factory, FactoryMethods,
    JsString, NodeId, NodeListId, NodeVisitor, RuntimeFactory, SyntaxKind as K,
};
use tsr_core::TextRange;
use tsr_printer::{emit_flags, generated_identifier_flags as g, AutoGenerateOptions, EmitContext};

/// `usingDeclarationTransformer`. The state upstream holds per source file;
/// `export_bindings` is `None` where upstream's map is nil.
struct UsingDeclarationTransformer {
    emit_context: EmitContext,
    failure: Failure,
    export_bindings: RefCell<Option<HashMap<JsString, NodeId>>>,
    export_binding_names: RefCell<Vec<JsString>>,
    export_vars: RefCell<Vec<NodeId>>,
    default_export_binding: Cell<Option<NodeId>>,
    export_equals_binding: Cell<Option<NodeId>>,
}

// port: tsc/internal/transformers/estransforms/using.go:newUsingDeclarationTransformer
pub fn new_using_declaration_transformer<'a>(
    opts: &TransformOptions<'a>,
) -> Option<Transformer<'a>> {
    let tx = Rc::new(UsingDeclarationTransformer {
        emit_context: opts.context.clone(),
        failure: opts.failure.clone(),
        export_bindings: RefCell::new(None),
        export_binding_names: RefCell::new(Vec::new()),
        export_vars: RefCell::new(Vec::new()),
        default_export_binding: Cell::new(None),
        export_equals_binding: Cell::new(None),
    });
    Some(Transformer::new(
        move |visitor: &mut NodeVisitor<'_>, node: Option<NodeId>| tx.visit(visitor, node),
        Some(opts.context.clone()),
        opts.failure.clone(),
    ))
}

/// `usingKind`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum UsingKind {
    None,
    Sync,
    Async,
}

/// The options of the `_default` bindings upstream names with `NewUniqueNameEx`.
fn default_binding_options() -> AutoGenerateOptions {
    AutoGenerateOptions {
        flags: g::RESERVED_IN_NESTED_SCOPES | g::FILE_LEVEL | g::OPTIMISTIC,
        ..AutoGenerateOptions::default()
    }
}

fn text(bytes: &[u8]) -> JsString {
    JsString::from_bytes(bytes)
}

/// `NodeFactory.NewNodeList(nodes)` followed by `list.Loc = loc`.
fn new_node_list_at(
    factory: &mut dyn RuntimeFactory,
    nodes: Vec<NodeId>,
    loc: TextRange,
) -> NodeListId {
    let nodes = factory.alloc_nodes(nodes.into_iter().map(Some).collect());
    factory.alloc_list(loc, nodes)
}

/// `Visitor().VisitSlice(nodes)`, its first result.
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

impl UsingDeclarationTransformer {
    /// The emit context handle, for its `&mut self` operations.
    fn ctx(&self) -> EmitContext {
        self.emit_context.clone()
    }

    // port: tsc/internal/transformers/estransforms/using.go:usingDeclarationTransformer.visit
    fn visit(&self, visitor: &mut NodeVisitor<'_>, node: Option<NodeId>) -> Option<NodeId> {
        // A block's statements are visited here directly, once per nested block.
        stacker::maybe_grow(128 * 1024, 2 * 1024 * 1024, || {
            self.visit_worker(visitor, node)
        })
    }

    fn visit_worker(&self, visitor: &mut NodeVisitor<'_>, node: Option<NodeId>) -> Option<NodeId> {
        if self.failure.is_set() {
            return node;
        }
        let id = node.expect(NIL);
        let Some(facts) = self.failure.ok(subtree_facts(visitor.factory(), id)) else {
            return node;
        };
        if facts & subtree_flags::USING == 0 {
            return node;
        }

        match visitor.factory().node(id).kind().known() {
            Some(K::SourceFile) => Some(self.visit_source_file(visitor, id)),
            Some(K::Block) => Some(self.visit_block(visitor, id)),
            Some(K::ForStatement) => self.visit_for_statement(visitor, id),
            Some(K::ForOfStatement) => self.visit_for_of_statement(visitor, id),
            _ => visitor.visit_each_child(node),
        }
    }

    // port: tsc/internal/transformers/estransforms/using.go:usingDeclarationTransformer.visitSourceFile
    #[allow(clippy::if_not_else)] // upstream's branch order
    fn visit_source_file(&self, visitor: &mut NodeVisitor<'_>, node: NodeId) -> NodeId {
        let is_declaration_file = match visitor.factory().read_source_file(node) {
            Ok(file) => file.is_declaration_file,
            Err(error) => {
                self.failure.record(error);
                return node;
            }
        };
        if is_declaration_file {
            return node;
        }

        let mut ctx = self.ctx();
        let (statement_list, end_of_file_token) = {
            let read = visitor.factory().node(node);
            let data = read.as_source_file().expect("SourceFile payload");
            (data.statements(), data.end_of_file_token())
        };
        let statements = list_nodes(visitor.factory(), Some(statement_list.expect(NIL)));
        let visited;
        let using_kind = get_using_kind_of_statements(visitor.factory(), &statements);
        if using_kind != UsingKind::None {
            // Imports and exports must stay at the top level. This means we must hoist all imports, exports, and
            // top-level function declarations and bindings out of the `try` statements we generate. For example:
            //
            // given:
            //
            //  import { w } from "mod";
            //  const x = expr1;
            //  using y = expr2;
            //  const z = expr3;
            //  export function f() {
            //    console.log(z);
            //  }
            //
            // produces:
            //
            //  import { x } from "mod";        // <-- preserved
            //  const x = expr1;                // <-- preserved
            //  var y, z;                       // <-- hoisted
            //  export function f() {           // <-- hoisted
            //    console.log(z);
            //  }
            //  const env_1 = { stack: [], error: void 0, hasError: false };
            //  try {
            //    y = __addDisposableResource(env_1, expr2, false);
            //    z = expr3;
            //  }
            //  catch (e_1) {
            //    env_1.error = e_1;
            //    env_1.hasError = true;
            //  }
            //  finally {
            //    __disposeResource(env_1);
            //  }
            //
            // In this transformation, we hoist `y`, `z`, and `f` to a new outer statement list while moving all other
            // statements in the source file into the `try` block, which is the same approach we use for System module
            // emit. Unlike System module emit, we attempt to preserve all statements prior to the first top-level
            // `using` to isolate the complexity of the transformed output to only where it is necessary.
            ctx.start_variable_environment();

            *self.export_bindings.borrow_mut() = Some(HashMap::new());
            self.export_vars.borrow_mut().clear();

            let (prologue, rest) = ctx.split_standard_prologue(visitor.factory(), &statements);
            let mut top_level_statements = Vec::new();
            top_level_statements.extend(visit_slice(visitor, prologue));

            // Collect and transform any leading statements up to the first `using` or `await using`. This preserves
            // the original statement order much as is possible.

            let mut pos = 0;
            while pos < rest.len() {
                let statement = rest[pos];
                if get_using_kind(visitor.factory(), statement) != UsingKind::None {
                    if pos > 0 {
                        top_level_statements.extend(visit_slice(visitor, &rest[..pos]));
                    }
                    break;
                }
                pos += 1;
            }

            assert!(
                pos < rest.len(),
                "Should have encountered at least one 'using' statement."
            );

            // transform the rest of the body
            let env_binding = self.create_env_binding(visitor);
            let body_statements = self.transform_using_declarations(
                visitor,
                &rest[pos..],
                env_binding,
                Some(&mut top_level_statements),
            );

            // add `export {}` declarations for any hoisted bindings.
            let export_specifiers = {
                let export_bindings = self.export_bindings.borrow();
                match &*export_bindings {
                    Some(bindings) if !bindings.is_empty() => {
                        let names = self.export_binding_names.borrow();
                        let mut export_specifiers = Vec::with_capacity(names.len());
                        for name in names.iter() {
                            let specifier = bindings.get(name);
                            assert!(
                                specifier.is_some(),
                                "Debug failure. False expression: Missing export binding for hoisted export name"
                            );
                            export_specifiers.push(*specifier.expect(NIL));
                        }
                        Some(export_specifiers)
                    }
                    _ => None,
                }
            };
            if let Some(export_specifiers) = export_specifiers {
                let factory = visitor.factory_mut();
                let list = new_node_list(factory, export_specifiers);
                let named_exports = factory.new_named_exports(Some(list));
                top_level_statements.push(factory.new_export_declaration(
                    None,  /*modifiers*/
                    false, /*isTypeOnly*/
                    Some(named_exports),
                    None, /*moduleSpecifier*/
                    None, /*attributes*/
                ));
            }

            top_level_statements.extend(ctx.end_variable_environment(visitor.factory_mut()));
            let export_vars = self.export_vars.borrow().clone();
            if !export_vars.is_empty() {
                let factory = visitor.factory_mut();
                let export_keyword = factory.new_modifier(K::ExportKeyword.into());
                let modifiers = factory.alloc_nodes(vec![Some(export_keyword)]);
                let modifiers = factory.new_modifier_list(modifiers);
                let declarations = new_node_list(factory, export_vars);
                let declaration_list =
                    factory.new_variable_declaration_list(Some(declarations), node_flags::LET);
                top_level_statements
                    .push(factory.new_variable_statement(Some(modifiers), Some(declaration_list)));
            }
            top_level_statements.extend(self.create_downlevel_using_statements(
                visitor,
                body_statements,
                env_binding,
                using_kind == UsingKind::Async,
            ));

            if let Some(export_equals_binding) = self.export_equals_binding.get() {
                top_level_statements.push(visitor.factory_mut().new_export_assignment(
                    None, /*modifiers*/
                    true, /*isExportEquals*/
                    None, /*typeNode*/
                    Some(export_equals_binding),
                ));
            }

            let factory = visitor.factory_mut();
            let list = new_node_list(factory, top_level_statements);
            visited = factory.update_source(node, Some(list), end_of_file_token);
        } else {
            visited = visitor.visit_each_child(Some(node)).expect(NIL);
        }
        let helpers = ctx.read_emit_helpers();
        ctx.add_emit_helper(visited, &helpers);
        self.export_vars.borrow_mut().clear();
        *self.export_bindings.borrow_mut() = None;
        self.export_binding_names.borrow_mut().clear();
        self.default_export_binding.set(None);
        self.export_equals_binding.set(None);
        visited
    }

    // port: tsc/internal/transformers/estransforms/using.go:usingDeclarationTransformer.visitBlock
    fn visit_block(&self, visitor: &mut NodeVisitor<'_>, node: NodeId) -> NodeId {
        let (statement_list, multi_line) = {
            let read = visitor.factory().node(node);
            let data = read.as_block().expect("Block payload");
            (data.statements().expect(NIL), data.multi_line())
        };
        let statements = list_nodes(visitor.factory(), Some(statement_list));
        let using_kind = get_using_kind_of_statements(visitor.factory(), &statements);
        if using_kind != UsingKind::None {
            let ctx = self.ctx();
            let (prologue, rest) = ctx.split_standard_prologue(visitor.factory(), &statements);
            let env_binding = self.create_env_binding(visitor);
            let mut statements = Vec::with_capacity(prologue.len() + 2);
            statements.extend(visit_slice(visitor, prologue));
            let body = self.transform_using_declarations(
                visitor,
                rest,
                env_binding,
                None, /*topLevelStatements*/
            );
            statements.extend(self.create_downlevel_using_statements(
                visitor,
                body,
                env_binding,
                using_kind == UsingKind::Async,
            ));
            let factory = visitor.factory_mut();
            let loc = factory.read_list(statement_list).loc();
            let statement_list = new_node_list_at(factory, statements, loc);
            return factory.update_block(node, Some(statement_list), multi_line);
        }
        visitor.visit_each_child(Some(node)).expect(NIL)
    }

    // port: tsc/internal/transformers/estransforms/using.go:usingDeclarationTransformer.visitForStatement
    #[allow(clippy::unused_self)] // upstream's method
    fn visit_for_statement(&self, visitor: &mut NodeVisitor<'_>, node: NodeId) -> Option<NodeId> {
        let (initializer, condition, incrementor, statement) = {
            let read = visitor.factory().node(node);
            let data = read.as_for_statement().expect("ForStatement payload");
            (
                data.initializer(),
                data.condition(),
                data.incrementor(),
                data.statement(),
            )
        };
        if let Some(initializer) = initializer {
            if is_using_variable_declaration_list(visitor.factory(), initializer) {
                // given:
                //
                //  for (using x = expr; cond; incr) { ... }
                //
                // produces a shallow transformation to:
                //
                //  {
                //    using x = expr;
                //    for (; cond; incr) { ... }
                //  }
                //
                // before handing the shallow transformation back to the visitor for an in-depth transformation.
                let factory = visitor.factory_mut();
                let variable_statement =
                    factory.new_variable_statement(None /*modifiers*/, Some(initializer));
                let for_statement = factory.update_for_statement(
                    node,
                    None, /*initializer*/
                    condition,
                    incrementor,
                    statement,
                );
                let list = new_node_list(factory, vec![variable_statement, for_statement]);
                let block = factory.new_block(Some(list), false /*multiLine*/);
                return visitor.visit_node(Some(block));
            }
        }
        visitor.visit_each_child(Some(node))
    }

    // port: tsc/internal/transformers/estransforms/using.go:usingDeclarationTransformer.visitForOfStatement
    fn visit_for_of_statement(
        &self,
        visitor: &mut NodeVisitor<'_>,
        node: NodeId,
    ) -> Option<NodeId> {
        let (await_modifier, initializer, expression, statement) = {
            let read = visitor.factory().node(node);
            let data = read
                .as_for_in_or_of_statement()
                .expect("ForInOrOfStatement payload");
            (
                data.await_modifier(),
                data.initializer(),
                data.expression(),
                data.statement(),
            )
        };
        let initializer = initializer.expect(NIL);
        if is_using_variable_declaration_list(visitor.factory(), initializer) {
            // given:
            //
            //  for (using x of y) { ... }
            //
            // produces a shallow transformation to:
            //
            //  for (const x_1 of y) {
            //    using x = x;
            //    ...
            //  }
            //
            // before handing the shallow transformation back to the visitor for an in-depth transformation.
            let mut ctx = self.ctx();
            let declarations = {
                let read = visitor.factory().node(initializer);
                read.as_variable_declaration_list()
                    .expect("VariableDeclarationList payload")
                    .declarations()
                    .expect(NIL)
            };
            let factory = visitor.factory_mut();
            let for_decl = if let Some(&first) = list_nodes(factory, Some(declarations)).first() {
                first
            } else {
                let temp = ctx.new_temp_variable(factory);
                factory.new_variable_declaration(Some(temp), None, None, None)
            };

            let is_await_using = get_using_kind_of_variable_declaration_list(factory, initializer)
                == UsingKind::Async;
            let for_decl_name = factory.node(for_decl).name();
            let temp = ctx.new_generated_name_for_node(factory, for_decl_name.expect(NIL));
            let using_var = factory.update_variable_declaration(
                for_decl,
                for_decl_name,
                None, /*exclamationToken*/
                None, /*type*/
                Some(temp),
            );
            let list = new_node_list(factory, vec![using_var]);
            let using_var_list = factory.new_variable_declaration_list(
                Some(list),
                if is_await_using {
                    node_flags::AWAIT_USING
                } else {
                    node_flags::USING
                },
            );
            let using_var_statement =
                factory.new_variable_statement(None /*modifiers*/, Some(using_var_list));
            let node_statement = statement.expect(NIL);
            let block = {
                let read = factory.node(node_statement);
                read.as_block()
                    .map(|data| (data.statements(), data.multi_line()))
            };
            let statement = if let Some((block_statements, multi_line)) = block {
                let block_statements = list_nodes(factory, block_statements);
                let mut statements = Vec::with_capacity(block_statements.len() + 1);
                statements.push(using_var_statement);
                statements.extend(block_statements);
                let list = new_node_list(factory, statements);
                factory.update_block(node_statement, Some(list), multi_line)
            } else {
                let list = new_node_list(factory, vec![using_var_statement, node_statement]);
                factory.new_block(Some(list), true /*multiLine*/)
            };
            let declaration = factory.new_variable_declaration(
                Some(temp),
                None, /*exclamationToken*/
                None, /*type*/
                None,
            );
            let list = new_node_list(factory, vec![declaration]);
            let declaration_list =
                factory.new_variable_declaration_list(Some(list), node_flags::CONST);
            let updated = factory.update_for_in_or_of_statement(
                node,
                await_modifier,
                Some(declaration_list),
                expression,
                Some(statement),
            );
            return visitor.visit_node(Some(updated));
        }
        visitor.visit_each_child(Some(node))
    }

    /// `transformUsingDeclarations`' `hoist` closure.
    fn hoist(
        &self,
        visitor: &mut NodeVisitor<'_>,
        node: NodeId,
        top_level_statements: Option<&mut Vec<NodeId>>,
    ) -> Option<NodeId> {
        let Some(top_level_statements) = top_level_statements else {
            return Some(node);
        };

        match visitor.factory().node(node).kind().known() {
            Some(
                K::ImportDeclaration
                | K::ImportEqualsDeclaration
                | K::ExportDeclaration
                | K::FunctionDeclaration,
            ) => {
                Self::hoist_import_or_export_or_hoisted_declaration(node, top_level_statements);
                None
            }
            Some(K::ExportAssignment) => Some(self.hoist_export_assignment(visitor, node)),
            Some(K::ClassDeclaration) => Some(self.hoist_class_declaration(visitor, node)),
            Some(K::VariableStatement) => self.hoist_variable_statement(visitor, node),
            _ => Some(node),
        }
    }

    /// `transformUsingDeclarations`' `hoistOrAppendNode` closure.
    fn hoist_or_append_node(
        &self,
        visitor: &mut NodeVisitor<'_>,
        node: NodeId,
        statements: &mut Vec<NodeId>,
        top_level_statements: Option<&mut Vec<NodeId>>,
    ) {
        if let Some(node) = self.hoist(visitor, node, top_level_statements) {
            statements.push(node);
        }
    }

    // port: tsc/internal/transformers/estransforms/using.go:usingDeclarationTransformer.transformUsingDeclarations
    fn transform_using_declarations(
        &self,
        visitor: &mut NodeVisitor<'_>,
        statements_in: &[NodeId],
        env_binding: NodeId,
        mut top_level_statements: Option<&mut Vec<NodeId>>,
    ) -> Vec<NodeId> {
        let mut statements = Vec::new();

        for &statement in statements_in {
            let using_kind = get_using_kind(visitor.factory(), statement);
            if using_kind != UsingKind::None {
                let declaration_list = visitor
                    .factory()
                    .node(statement)
                    .as_variable_statement()
                    .expect("VariableStatement payload")
                    .declaration_list()
                    .expect(NIL);
                let declaration_nodes = {
                    let read = visitor.factory().node(declaration_list);
                    read.as_variable_declaration_list()
                        .expect("VariableDeclarationList payload")
                        .declarations()
                        .expect(NIL)
                };
                let mut declarations = Vec::new();
                for mut declaration in list_nodes(visitor.factory(), Some(declaration_nodes)) {
                    let name = visitor.factory().node(declaration).name().expect(NIL);
                    if visitor.factory().node(name).kind() != K::Identifier {
                        // Since binding patterns are a grammar error, we reset `declarations` so we don't process this as a `using`.
                        declarations.clear();
                        break;
                    }

                    // perform a shallow transform for any named evaluation
                    if is_named_evaluation(&self.emit_context, visitor.factory(), declaration) {
                        declaration = transform_named_evaluation(
                            &self.emit_context,
                            visitor.factory_mut(),
                            declaration,
                            false, /*ignoreEmptyStringLiteral*/
                            b"",   /*assignedName*/
                        );
                    }

                    let initializer = visitor.factory().node(declaration).initializer();
                    let mut ctx = self.ctx();
                    let initializer = visitor.visit_node(initializer);
                    let factory = visitor.factory_mut();
                    let initializer = match initializer {
                        Some(initializer) => initializer,
                        None => ctx.new_void_zero_expression(factory),
                    };
                    let name = factory.node(declaration).name();
                    let helper = ctx.new_add_disposable_resource_helper(
                        factory,
                        env_binding,
                        initializer,
                        using_kind == UsingKind::Async,
                    );
                    declarations.push(factory.update_variable_declaration(
                        declaration,
                        name,
                        None, /*exclamationToken*/
                        None, /*type*/
                        Some(helper),
                    ));
                }

                // Only replace the statement if it was valid.
                if !declarations.is_empty() {
                    let factory = visitor.factory_mut();
                    let list = new_node_list(factory, declarations);
                    let var_list =
                        factory.new_variable_declaration_list(Some(list), node_flags::CONST);
                    self.ctx().set_original(var_list, declaration_list);
                    let loc = factory.node(declaration_list).range();
                    factory.node_mut(var_list).set_range(loc);
                    let updated = factory.update_variable_statement(
                        statement,
                        None, /*modifiers*/
                        Some(var_list),
                    );
                    self.hoist_or_append_node(
                        visitor,
                        updated,
                        &mut statements,
                        top_level_statements.as_deref_mut(),
                    );
                    continue;
                }
            }

            if let Some(result) = self.visit(visitor, Some(statement)) {
                let children = {
                    let read = visitor.factory().node(result);
                    read.data_source()
                        .as_syntax_list()
                        .map(|list| list.children())
                };
                if let Some(children) = children {
                    let children: Vec<_> = visitor.factory().read_nodes(children).iter().collect();
                    for node in children {
                        self.hoist_or_append_node(
                            visitor,
                            node.expect(NIL),
                            &mut statements,
                            top_level_statements.as_deref_mut(),
                        );
                    }
                } else {
                    self.hoist_or_append_node(
                        visitor,
                        result,
                        &mut statements,
                        top_level_statements.as_deref_mut(),
                    );
                }
            }
        }
        statements
    }

    // port: tsc/internal/transformers/estransforms/using.go:usingDeclarationTransformer.hoistImportOrExportOrHoistedDeclaration
    fn hoist_import_or_export_or_hoisted_declaration(
        node: NodeId,
        top_level_statements: &mut Vec<NodeId>,
    ) {
        // NOTE: `node` has already been visited
        top_level_statements.push(node);
    }

    // port: tsc/internal/transformers/estransforms/using.go:usingDeclarationTransformer.hoistExportAssignment
    fn hoist_export_assignment(&self, visitor: &mut NodeVisitor<'_>, node: NodeId) -> NodeId {
        let is_export_equals = visitor
            .factory()
            .node(node)
            .as_export_assignment()
            .expect("ExportAssignment payload")
            .is_export_equals();
        if is_export_equals {
            self.hoist_export_equals(visitor, node)
        } else {
            self.hoist_export_default(visitor, node)
        }
    }

    // port: tsc/internal/transformers/estransforms/using.go:usingDeclarationTransformer.hoistExportDefault
    fn hoist_export_default(&self, visitor: &mut NodeVisitor<'_>, node: NodeId) -> NodeId {
        // NOTE: `node` has already been visited
        if self.default_export_binding.get().is_some() {
            // invalid case of multiple `export default` declarations. Don't assert here, just pass it through
            return node;
        }

        // given:
        //
        //   export default expr;
        //
        // produces:
        //
        //   // top level
        //   var default_1;
        //   export { default_1 as default };
        //
        //   // body
        //   default_1 = expr;

        let mut ctx = self.ctx();
        let binding = ctx.new_unique_name_ex(
            visitor.factory_mut(),
            text(b"_default"),
            default_binding_options(),
        );
        self.default_export_binding.set(Some(binding));
        let default_identifier = visitor.factory_mut().new_identifier(text(b"default"));
        self.hoist_binding_identifier(
            visitor,
            binding,
            true, /*isExport*/
            Some(default_identifier),
            Some(node),
        );

        // give a class or function expression an assigned name, if needed.
        let mut expression = visitor.factory().node(node).expression().expect(NIL);
        let mut inner_expression = skip_outer_expressions_all(visitor.factory(), expression);
        if is_named_evaluation(&self.emit_context, visitor.factory(), inner_expression) {
            inner_expression = transform_named_evaluation(
                &self.emit_context,
                visitor.factory_mut(),
                inner_expression,
                false, /*ignoreEmptyStringLiteral*/
                b"default",
            );
            expression = restore_outer_expressions_all(
                &self.emit_context,
                visitor.factory_mut(),
                Some(expression),
                inner_expression,
            );
        }

        let factory = visitor.factory_mut();
        let assignment = ctx.new_assignment_expression(factory, binding, expression);
        factory.new_expression_statement(Some(assignment))
    }

    // port: tsc/internal/transformers/estransforms/using.go:usingDeclarationTransformer.hoistExportEquals
    fn hoist_export_equals(&self, visitor: &mut NodeVisitor<'_>, node: NodeId) -> NodeId {
        // NOTE: `node` has already been visited
        if self.export_equals_binding.get().is_some() {
            // invalid case of multiple `export default` declarations. Don't assert here, just pass it through
            return node;
        }

        // given:
        //
        //   export = expr;
        //
        // produces:
        //
        //   // top level
        //   var default_1;
        //
        //   try {
        //       // body
        //       default_1 = expr;
        //   } ...
        //
        //   // top level suffix
        //   export = default_1;

        let mut ctx = self.ctx();
        let factory = visitor.factory_mut();
        let binding = ctx.new_unique_name_ex(factory, text(b"_default"), default_binding_options());
        self.export_equals_binding.set(Some(binding));
        ctx.add_variable_declaration(factory, binding);

        // give a class or function expression an assigned name, if needed.
        let expression = factory.node(node).expression().expect(NIL);
        let assignment = ctx.new_assignment_expression(factory, binding, expression);
        factory.new_expression_statement(Some(assignment))
    }

    // port: tsc/internal/transformers/estransforms/using.go:usingDeclarationTransformer.hoistClassDeclaration
    fn hoist_class_declaration(&self, visitor: &mut NodeVisitor<'_>, node: NodeId) -> NodeId {
        // NOTE: `node` has already been visited
        let (name, loc) = {
            let read = visitor.factory().node(node);
            (read.name(), read.range())
        };
        if name.is_none() && self.default_export_binding.get().is_some() {
            // invalid case of multiple `export default` declarations. Don't assert here, just pass it through
            return node;
        }

        let is_exported = has_syntactic_modifier(visitor.factory(), node, modifier_flags::EXPORT);
        let is_default = has_syntactic_modifier(visitor.factory(), node, modifier_flags::DEFAULT);

        // When hoisting a class declaration at the top level of a file containing a top-level `using` statement, we
        // must first convert it to a class expression so that we can hoist the binding outside of the `try`.
        let mut ctx = self.ctx();
        let mut expression = convert_class_declaration_to_class_expression(
            &self.emit_context,
            visitor.factory_mut(),
            node,
        );
        if name.is_some() {
            // given:
            //
            //  using x = expr;
            //  class C {}
            //
            // produces:
            //
            //  var x, C;
            //  const env_1 = { ... };
            //  try {
            //    x = __addDisposableResource(env_1, expr, false);
            //    C = class {};
            //  }
            //  catch (e_1) {
            //    env_1.error = e_1;
            //    env_1.hasError = true;
            //  }
            //  finally {
            //    __disposeResources(env_1);
            //  }
            //
            // If the class is exported, we also produce an `export { C };`
            let local_name =
                builder(visitor).and_then(|builder| Ok(ctx.get_local_name(builder, Some(node))?));
            let Some(local_name) = self.failure.ok(local_name) else {
                return node;
            };
            self.hoist_binding_identifier(
                visitor,
                local_name,
                is_exported && !is_default,
                None, /*exportAlias*/
                Some(node),
            );
            let declaration_name = builder(visitor)
                .and_then(|builder| Ok(ctx.get_declaration_name(builder, Some(node))?));
            let Some(declaration_name) = self.failure.ok(declaration_name) else {
                return node;
            };
            expression =
                ctx.new_assignment_expression(visitor.factory_mut(), declaration_name, expression);
            ctx.set_original(expression, node);
            ctx.set_source_map_range(expression, loc);
            ctx.set_comment_range(expression, loc);
            if is_named_evaluation(&self.emit_context, visitor.factory(), expression) {
                expression = transform_named_evaluation(
                    &self.emit_context,
                    visitor.factory_mut(),
                    expression,
                    false, /*ignoreEmptyStringLiteral*/
                    b"",   /*assignedName*/
                );
            }
        }

        if is_default && self.default_export_binding.get().is_none() {
            // In the case of a default export, we create a temporary variable that we export as the default and then
            // assign to that variable.
            //
            // given:
            //
            //  using x = expr;
            //  export default class C {}
            //
            // produces:
            //
            //  export { default_1 as default };
            //  var x, C, default_1;
            //  const env_1 = { ... };
            //  try {
            //    x = __addDisposableResource(env_1, expr, false);
            //    default_1 = C = class {};
            //  }
            //  catch (e_1) {
            //    env_1.error = e_1;
            //    env_1.hasError = true;
            //  }
            //  finally {
            //    __disposeResources(env_1);
            //  }
            //
            // Though we will never reassign `default_1`, this most closely matches the specified runtime semantics.
            let binding = ctx.new_unique_name_ex(
                visitor.factory_mut(),
                text(b"_default"),
                default_binding_options(),
            );
            self.default_export_binding.set(Some(binding));
            let default_identifier = visitor.factory_mut().new_identifier(text(b"default"));
            self.hoist_binding_identifier(
                visitor,
                binding,
                true, /*isExport*/
                Some(default_identifier),
                Some(node),
            );
            expression = ctx.new_assignment_expression(visitor.factory_mut(), binding, expression);
            ctx.set_original(expression, node);
            if is_named_evaluation(&self.emit_context, visitor.factory(), expression) {
                expression = transform_named_evaluation(
                    &self.emit_context,
                    visitor.factory_mut(),
                    expression,
                    false, /*ignoreEmptyStringLiteral*/
                    b"default",
                );
            }
        }

        visitor
            .factory_mut()
            .new_expression_statement(Some(expression))
    }

    // port: tsc/internal/transformers/estransforms/using.go:usingDeclarationTransformer.hoistVariableStatement
    fn hoist_variable_statement(
        &self,
        visitor: &mut NodeVisitor<'_>,
        node: NodeId,
    ) -> Option<NodeId> {
        // NOTE: `node` has already been visited
        let mut expressions = Vec::new();
        let is_exported = has_syntactic_modifier(visitor.factory(), node, modifier_flags::EXPORT);
        let (declarations, loc) = {
            let read = visitor.factory().node(node);
            let declaration_list = read
                .as_variable_statement()
                .expect("VariableStatement payload")
                .declaration_list()
                .expect(NIL);
            let list = visitor.factory().node(declaration_list);
            let declarations = list
                .as_variable_declaration_list()
                .expect("VariableDeclarationList payload")
                .declarations()
                .expect(NIL);
            (declarations, read.range())
        };
        for variable in list_nodes(visitor.factory(), Some(declarations)) {
            self.hoist_binding_element(visitor, variable, is_exported, variable);
            if visitor.factory().node(variable).initializer().is_some() {
                expressions.push(self.hoist_initialized_variable(visitor, variable));
            }
        }
        if !expressions.is_empty() {
            let mut ctx = self.ctx();
            let factory = visitor.factory_mut();
            let expression = ctx.inline_expressions(factory, &expressions);
            let statement = factory.new_expression_statement(expression);
            ctx.set_original(statement, node);
            ctx.set_comment_range(statement, loc);
            ctx.set_source_map_range(statement, loc);
            return Some(statement);
        }
        None
    }

    // port: tsc/internal/transformers/estransforms/using.go:usingDeclarationTransformer.hoistInitializedVariable
    fn hoist_initialized_variable(&self, visitor: &mut NodeVisitor<'_>, node: NodeId) -> NodeId {
        // NOTE: `node` has already been visited
        let (name, initializer, loc) = {
            let read = visitor.factory().node(node);
            (read.name(), read.initializer(), read.range())
        };
        let Some(initializer) = initializer else {
            panic!("Expected initializer");
        };
        let mut ctx = self.ctx();
        let factory = visitor.factory_mut();
        let name = name.expect(NIL);
        let target = if factory.node(name).kind() == K::Identifier {
            let target = clone_node(factory, name);
            ctx.set_emit_flags(
                target,
                ctx.emit_flags(target) & !(emit_flags::LOCAL_NAME | emit_flags::EXPORT_NAME),
            );
            target
        } else {
            convert_binding_pattern_to_assignment_pattern(&self.emit_context, factory, name)
        };

        let assignment = ctx.new_assignment_expression(factory, target, initializer);
        ctx.set_original(assignment, node);
        ctx.set_comment_range(assignment, loc);
        ctx.set_source_map_range(assignment, loc);
        assignment
    }

    // port: tsc/internal/transformers/estransforms/using.go:usingDeclarationTransformer.hoistBindingElement
    fn hoist_binding_element(
        &self,
        visitor: &mut NodeVisitor<'_>,
        node: NodeId, /*VariableDeclaration|BindingElement*/
        is_exported_declaration: bool,
        original: NodeId,
    ) {
        // NOTE: `node` has already been visited
        let name = visitor.factory().node(node).name().expect(NIL);
        let elements = {
            let read = visitor.factory().node(name);
            read.as_binding_pattern().map(|pattern| pattern.elements())
        };
        if let Some(elements) = elements {
            for element in list_nodes(visitor.factory(), elements) {
                if visitor.factory().node(element).name().is_some() {
                    self.hoist_binding_element(visitor, element, is_exported_declaration, original);
                }
            }
        } else {
            self.hoist_binding_identifier(
                visitor,
                name,
                is_exported_declaration,
                None, /*exportAlias*/
                Some(original),
            );
        }
    }

    // port: tsc/internal/transformers/estransforms/using.go:usingDeclarationTransformer.hoistBindingIdentifier
    fn hoist_binding_identifier(
        &self,
        visitor: &mut NodeVisitor<'_>,
        node: NodeId,
        is_export: bool,
        export_alias: Option<NodeId>,
        original: Option<NodeId>,
    ) {
        // NOTE: `node` has already been visited
        let mut ctx = self.ctx();
        let factory = visitor.factory_mut();
        let mut name = node;
        if !is_generated_identifier(&self.emit_context, node) {
            name = clone_node(factory, name);
        }
        if is_export {
            if export_alias.is_none() && !is_local_name(&self.emit_context, name) {
                let var_decl = factory.new_variable_declaration(
                    Some(name),
                    None, /*exclamationToken*/
                    None, /*type*/
                    None, /*initializer*/
                );
                if let Some(original) = original {
                    ctx.set_original(var_decl, original);
                }
                self.export_vars.borrow_mut().push(var_decl);
                return;
            }

            let (local_name, export_name) = match export_alias {
                Some(export_alias) => (Some(name), export_alias),
                None => (None, name),
            };
            let specifier = factory.new_export_specifier(
                false, /*isTypeOnly*/
                local_name,
                Some(export_name),
            );
            if let Some(original) = original {
                ctx.set_original(specifier, original);
            }
            let name_text = identifier_text(factory, name);
            let mut export_bindings = self.export_bindings.borrow_mut();
            let export_bindings = export_bindings.get_or_insert_with(HashMap::new);
            if !export_bindings.contains_key(&name_text) {
                self.export_binding_names
                    .borrow_mut()
                    .push(name_text.clone());
            }
            export_bindings.insert(name_text, specifier);
        }
        ctx.add_variable_declaration(factory, name);
    }

    // port: tsc/internal/transformers/estransforms/using.go:usingDeclarationTransformer.createEnvBinding
    fn create_env_binding(&self, visitor: &mut NodeVisitor<'_>) -> NodeId {
        self.ctx()
            .new_unique_name(visitor.factory_mut(), text(b"env"))
    }

    // port: tsc/internal/transformers/estransforms/using.go:usingDeclarationTransformer.createDownlevelUsingStatements
    fn create_downlevel_using_statements(
        &self,
        visitor: &mut NodeVisitor<'_>,
        body_statements: Vec<NodeId>,
        env_binding: NodeId,
        is_async: bool,
    ) -> Vec<NodeId> {
        let mut ctx = self.ctx();
        let factory = visitor.factory_mut();
        let mut statements = Vec::with_capacity(2);

        // produces:
        //
        //  const env_1 = { stack: [], error: void 0, hasError: false };
        //
        let stack = factory.new_identifier(text(b"stack"));
        let array = factory.new_array_literal_expression(None, false /*multiLine*/);
        let stack = factory.new_property_assignment(
            None, /*modifiers*/
            Some(stack),
            None, /*postfixToken*/
            None, /*typeNode*/
            Some(array),
        );
        let error = factory.new_identifier(text(b"error"));
        let void_zero = ctx.new_void_zero_expression(factory);
        let error = factory.new_property_assignment(
            None, /*modifiers*/
            Some(error),
            None, /*postfixToken*/
            None, /*typeNode*/
            Some(void_zero),
        );
        let has_error = factory.new_identifier(text(b"hasError"));
        let false_expression = ctx.new_false_expression(factory);
        let has_error = factory.new_property_assignment(
            None, /*modifiers*/
            Some(has_error),
            None, /*postfixToken*/
            None, /*typeNode*/
            Some(false_expression),
        );
        let properties = new_node_list(factory, vec![stack, error, has_error]);
        let env_object =
            factory.new_object_literal_expression(Some(properties), false /*multiLine*/);
        let env_var = factory.new_variable_declaration(
            Some(env_binding),
            None, /*exclamationToken*/
            None, /*typeNode*/
            Some(env_object),
        );
        let list = new_node_list(factory, vec![env_var]);
        let env_var_list = factory.new_variable_declaration_list(Some(list), node_flags::CONST);
        let env_var_statement =
            factory.new_variable_statement(None /*modifiers*/, Some(env_var_list));
        statements.push(env_var_statement);

        // when `async` is `false`, produces:
        //
        //  try {
        //    <bodyStatements>
        //  }
        //  catch (e_1) {
        //      env_1.error = e_1;
        //      env_1.hasError = true;
        //  }
        //  finally {
        //    __disposeResources(env_1);
        //  }

        // when `async` is `true`, produces:
        //
        //  try {
        //    <bodyStatements>
        //  }
        //  catch (e_1) {
        //      env_1.error = e_1;
        //      env_1.hasError = true;
        //  }
        //  finally {
        //    const result_1 = __disposeResources(env_1);
        //    if (result_1) {
        //      await result_1;
        //    }
        //  }

        // Unfortunately, it is necessary to use two properties to indicate an error because `throw undefined` is legal
        // JavaScript.
        let list = new_node_list(factory, body_statements);
        let try_block = factory.new_block(Some(list), true /*multiLine*/);
        let body_catch_binding = ctx.new_unique_name(factory, text(b"e"));
        let catch_declaration = factory.new_variable_declaration(
            Some(body_catch_binding),
            None, /*exclamationToken*/
            None, /*type*/
            None, /*initializer*/
        );
        let error = factory.new_identifier(text(b"error"));
        let env_error = factory.new_property_access_expression(
            Some(env_binding),
            None,
            Some(error),
            node_flags::NONE,
        );
        let assignment = ctx.new_assignment_expression(factory, env_error, body_catch_binding);
        let set_error = factory.new_expression_statement(Some(assignment));
        let has_error = factory.new_identifier(text(b"hasError"));
        let env_has_error = factory.new_property_access_expression(
            Some(env_binding),
            None,
            Some(has_error),
            node_flags::NONE,
        );
        let true_expression = ctx.new_true_expression(factory);
        let assignment = ctx.new_assignment_expression(factory, env_has_error, true_expression);
        let set_has_error = factory.new_expression_statement(Some(assignment));
        let list = new_node_list(factory, vec![set_error, set_has_error]);
        let catch_block = factory.new_block(Some(list), true /*multiLine*/);
        let catch_clause = factory.new_catch_clause(Some(catch_declaration), Some(catch_block));

        let finally_block = if is_async {
            let result = ctx.new_unique_name(factory, text(b"result"));
            let dispose = ctx.new_dispose_resources_helper(factory, env_binding);
            let declaration = factory.new_variable_declaration(
                Some(result),
                None, /*exclamationToken*/
                None, /*type*/
                Some(dispose),
            );
            let list = new_node_list(factory, vec![declaration]);
            let declaration_list =
                factory.new_variable_declaration_list(Some(list), node_flags::CONST);
            let variable_statement =
                factory.new_variable_statement(None /*modifiers*/, Some(declaration_list));
            let await_expression = factory.new_await_expression(Some(result));
            let then_statement = factory.new_expression_statement(Some(await_expression));
            let if_statement = factory.new_if_statement(
                Some(result),
                Some(then_statement),
                None, /*elseStatement*/
            );
            let list = new_node_list(factory, vec![variable_statement, if_statement]);
            factory.new_block(Some(list), true /*multiLine*/)
        } else {
            let dispose = ctx.new_dispose_resources_helper(factory, env_binding);
            let statement = factory.new_expression_statement(Some(dispose));
            let list = new_node_list(factory, vec![statement]);
            factory.new_block(Some(list), true /*multiLine*/)
        };

        let try_statement =
            factory.new_try_statement(Some(try_block), Some(catch_clause), Some(finally_block));
        statements.push(try_statement);
        statements
    }
}

// port: tsc/internal/transformers/estransforms/using.go:isUsingVariableDeclarationList
fn is_using_variable_declaration_list(factory: &dyn Factory, node: NodeId) -> bool {
    factory.node(node).kind() == K::VariableDeclarationList
        && get_using_kind_of_variable_declaration_list(factory, node) != UsingKind::None
}

// port: tsc/internal/transformers/estransforms/using.go:getUsingKindOfVariableDeclarationList
fn get_using_kind_of_variable_declaration_list(factory: &dyn Factory, node: NodeId) -> UsingKind {
    match factory.node(node).flags() & node_flags::BLOCK_SCOPED {
        node_flags::AWAIT_USING => UsingKind::Async,
        node_flags::USING => UsingKind::Sync,
        _ => UsingKind::None,
    }
}

// port: tsc/internal/transformers/estransforms/using.go:getUsingKindOfVariableStatement
fn get_using_kind_of_variable_statement(factory: &dyn Factory, node: NodeId) -> UsingKind {
    let declaration_list = factory
        .node(node)
        .as_variable_statement()
        .expect("VariableStatement payload")
        .declaration_list()
        .expect(NIL);
    get_using_kind_of_variable_declaration_list(factory, declaration_list)
}

// port: tsc/internal/transformers/estransforms/using.go:getUsingKind
fn get_using_kind(factory: &dyn Factory, statement: NodeId) -> UsingKind {
    if factory.node(statement).kind() == K::VariableStatement {
        return get_using_kind_of_variable_statement(factory, statement);
    }
    UsingKind::None
}

// port: tsc/internal/transformers/estransforms/using.go:getUsingKindOfStatements
fn get_using_kind_of_statements(factory: &dyn Factory, statements: &[NodeId]) -> UsingKind {
    let mut result = UsingKind::None;
    for &statement in statements {
        let using_kind = get_using_kind(factory, statement);
        if using_kind == UsingKind::Async {
            return UsingKind::Async;
        }
        if using_kind > result {
            result = using_kind;
        }
    }
    result
}
