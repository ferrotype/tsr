//! `transformers/moduletransforms/commonjsmodule.go`: lowers an ES module to
//! CommonJS: `import` and `export` declarations become `require` calls and
//! `exports` assignments, references to imported bindings read through the
//! module object, assignments to exported bindings update `exports`, and
//! dynamic `import()` becomes a `Promise` over `require`.
//!
//! Upstream keeps five node visitors: the transformer's own (`tx.Visitor()`,
//! whose callback is `visit`), the top-level, top-level-nested,
//! discarded-value and assignment-pattern visitors. A visitor borrows the
//! factory here, so each is created over the current visitor's factory where
//! upstream uses it ([`Visitor`]); every method receives whichever visitor is
//! current and reaches the factory through it.
//!
//! The ancestor stack (`parentNode`, `currentNode`) and the per-file state
//! live in `Cell`s and `RefCell`s that no visit borrows. A failed storage read
//! or resolver query is recorded in the options' `Failure`: the visit that met
//! it returns its node unchanged and every later visit stops.
use super::externalmoduleinfo::{
    collect_external_module_info, create_external_helpers_import_declaration_if_needed,
    get_export_needs_import_star_helper, get_import_needs_import_default_helper,
    get_import_needs_import_star_helper, ExternalModuleInfo,
};
use super::utilities::{
    get_external_module_name_literal, is_declaration_name_of_enum_or_namespace,
    is_file_level_reserved_generated_identifier, is_simple_inlineable_expression,
    rewrite_module_specifier,
};
use crate::destructuring::{flatten_destructuring_assignment, FlattenLevel};
use crate::estransforms::utilities::{list_nodes, new_node_list, subtree_facts, view};
use crate::extract_modifiers;
use crate::transformer::{
    EmitModuleFormatOfFile, Error, Failure, SharedReferenceResolver, TransformOptions, Transformer,
};
use crate::utilities::{
    convert_variable_declaration_to_assignment_expression, is_export_name, is_generated_identifier,
    is_helper_name, is_identifier_reference, is_local_name, single_or_many,
};
use std::cell::{Cell, RefCell};
use std::collections::HashSet;
use std::rc::Rc;
use std::sync::Arc;
use tsr_ast::subtree_flags::{DYNAMIC_IMPORT, IDENTIFIER};
use tsr_ast::utilities::{
    has_syntactic_modifier, is_binding_pattern, is_external_module, is_in_js_file,
    is_string_literal_like,
};
use tsr_ast::utilities_middle::{
    get_namespace_declaration_node, is_default_import, is_require_call,
    module_export_name_is_default,
};
use tsr_ast::utilities_modules::{
    is_effective_external_module, is_external_module_import_equals_declaration,
};
use tsr_ast::{
    modifier_flags, node_flags, token_flags, AstBuilder, FactoryMethods, JsString, NodeId,
    NodeListId, NodeVisitor, RuntimeFactory, SyntaxKind as K,
};
use tsr_core::{CompilerOptions, JsxEmit, ModuleKind, ScriptTarget, TextRange};
use tsr_printer::generated_identifier_flags as g;
use tsr_printer::{
    emit_flags, AutoGenerateOptions, EmitContext, EmitVisitorHooks, GeneratedIdentifierFlagsExt,
};
use tsr_tspath::{file_extension_is_one_of, SUPPORTED_JS_EXTENSIONS_FLAT};

const NIL: &str = "runtime error: invalid memory address or nil pointer dereference";

/// A visit's result: upstream's node (nil is `None`) or the failure that
/// stopped it.
type Visited = Result<Option<NodeId>, Error>;

/// Which of upstream's node visitors a visitor created here is.
#[derive(Clone, Copy)]
enum Visitor {
    /// `tx.Visitor()`, whose callback is `visit`.
    Main,
    /// `topLevelVisitor`: statements at the top level of a module.
    TopLevel,
    /// `topLevelNestedVisitor`: nested statements at the top level.
    TopLevelNested,
    /// `discardedValueVisitor`: expressions whose values are discarded.
    DiscardedValue,
    /// `assignmentPatternVisitor`: the patterns of a destructuring assignment.
    AssignmentPattern,
}

/// `CommonJSModuleTransformer`.
struct CommonJsModuleTransformer<'a> {
    context: EmitContext,
    hooks: EmitVisitorHooks,
    failure: Failure,
    compiler_options: Arc<CompilerOptions>,
    resolver: SharedReferenceResolver<'a>,
    get_emit_module_format_of_file: EmitModuleFormatOfFile<'a>,
    module_kind: ModuleKind,
    language_version: ScriptTarget,
    current_source_file: Cell<Option<NodeId>>,
    current_module_info: RefCell<Option<Rc<ExternalModuleInfo>>>,
    /// used for ancestor tracking via pushNode/popNode to detect expression identifiers
    parent_node: Cell<Option<NodeId>>,
    /// used for ancestor tracking via pushNode/popNode to detect expression identifiers
    current_node: Cell<Option<NodeId>>,
}

/// Upstream creates its four auxiliary visitors here; they are created where
/// they are used (see the module documentation).
// port: tsc/internal/transformers/moduletransforms/commonjsmodule.go:NewCommonJSModuleTransformer
pub fn new_common_js_module_transformer<'a>(
    opts: &TransformOptions<'a>,
) -> Option<Transformer<'a>> {
    let compiler_options = opts.compiler_options.clone();
    let emit_context = opts.context.clone();
    let tx = CommonJsModuleTransformer {
        hooks: emit_context.visitor_hooks(),
        context: emit_context.clone(),
        failure: opts.failure.clone(),
        resolver: opts.resolver.clone(),
        get_emit_module_format_of_file: opts.get_emit_module_format_of_file.clone(),
        language_version: compiler_options.emit_script_target(),
        module_kind: compiler_options.emit_module_kind(),
        compiler_options,
        current_source_file: Cell::new(None),
        current_module_info: RefCell::new(None),
        parent_node: Cell::new(None),
        current_node: Cell::new(None),
    };
    Some(Transformer::new(
        move |visitor: &mut NodeVisitor<'_>, node: Option<NodeId>| {
            tx.dispatch(Visitor::Main, visitor, node)
        },
        Some(emit_context),
        opts.failure.clone(),
    ))
}

/// `NodeFactory.NewIdentifier(text)`.
fn new_identifier(f: &mut dyn RuntimeFactory, text: &[u8]) -> NodeId {
    f.new_identifier(JsString::from_bytes(text))
}

/// `node.Text()` of an identifier, string literal or other named node.
fn node_text(f: &dyn RuntimeFactory, node: NodeId) -> Result<JsString, Error> {
    Ok(view(f)?.node_text(node)?.into_js_string())
}

/// The builder behind the visitor's factory, which the printer's name
/// helpers (`GetDeclarationName`, `GetLocalName`, `GetExportName`) take.
fn builder(f: &mut dyn RuntimeFactory) -> Result<&mut AstBuilder, Error> {
    f.ast_builder_mut().ok_or(Error::Unsupported(
        "a transform over a factory without AST storage",
    ))
}

/// `transformers.SingleOrMany(statements, factory)` of a statement list built
/// by appending: an empty list is Go's nil slice.
fn single_or_many_of(f: &mut dyn RuntimeFactory, statements: &[NodeId]) -> Option<NodeId> {
    single_or_many(f, (!statements.is_empty()).then_some(statements))
}

/// `ast.IsImportCall`.
fn is_import_call(f: &dyn RuntimeFactory, node: NodeId) -> Result<bool, Error> {
    Ok(tsr_ast::utilities_positions::is_import_call(
        view(f)?,
        node,
    )?)
}

/// The visitor `core.IfElse(resultIsDiscarded, tx.discardedValueVisitor,
/// tx.Visitor())` picks.
fn discarded_or_main(result_is_discarded: bool) -> Visitor {
    if result_is_discarded {
        Visitor::DiscardedValue
    } else {
        Visitor::Main
    }
}

/// A binary expression's `Left`, `OperatorToken` and `Right`.
fn binary_parts(
    f: &dyn RuntimeFactory,
    node: NodeId,
) -> (Option<NodeId>, Option<NodeId>, Option<NodeId>) {
    let read = f.node(node);
    let data = read
        .as_binary_expression()
        .expect("BinaryExpression payload");
    (data.left(), data.operator_token(), data.right())
}

/// A shorthand property assignment's `Name()`, `EqualsToken` and
/// `ObjectAssignmentInitializer`.
fn shorthand_parts(
    f: &dyn RuntimeFactory,
    node: NodeId,
) -> (Option<NodeId>, Option<NodeId>, Option<NodeId>) {
    let read = f.node(node);
    let data = read
        .as_shorthand_property_assignment()
        .expect("ShorthandPropertyAssignment payload");
    (
        data.name(),
        data.equals_token(),
        data.object_assignment_initializer(),
    )
}

/// A `for` statement's initializer, condition, incrementor and body.
fn for_statement_parts(
    f: &dyn RuntimeFactory,
    node: NodeId,
) -> (
    Option<NodeId>,
    Option<NodeId>,
    Option<NodeId>,
    Option<NodeId>,
) {
    let read = f.node(node);
    let data = read.as_for_statement().expect("ForStatement payload");
    (
        data.initializer(),
        data.condition(),
        data.incrementor(),
        data.statement(),
    )
}

/// A `for..in` or `for..of` statement's await modifier, initializer,
/// expression and body.
fn for_in_or_of_statement_parts(
    f: &dyn RuntimeFactory,
    node: NodeId,
) -> (
    Option<NodeId>,
    Option<NodeId>,
    Option<NodeId>,
    Option<NodeId>,
) {
    let read = f.node(node);
    let data = read
        .as_for_in_or_of_statement()
        .expect("ForInOrOfStatement payload");
    (
        data.await_modifier(),
        data.initializer(),
        data.expression(),
        data.statement(),
    )
}

impl CommonJsModuleTransformer<'_> {
    /// A visitor callback: the visit of `which`, with a failure recorded and
    /// the node returned unchanged.
    fn dispatch(
        &self,
        which: Visitor,
        v: &mut NodeVisitor<'_>,
        node: Option<NodeId>,
    ) -> Option<NodeId> {
        if self.failure.is_set() {
            return node;
        }
        let node = node.expect(NIL);
        let result = match which {
            Visitor::Main => self.visit(v, node),
            Visitor::TopLevel => self.visit_top_level(v, node),
            Visitor::TopLevelNested => self.visit_top_level_nested(v, node),
            Visitor::DiscardedValue => self.visit_discarded_value(v, node),
            Visitor::AssignmentPattern => self.visit_assignment_pattern(v, node),
        };
        match result {
            Ok(visited) => visited,
            Err(error) => {
                self.failure.record(error);
                Some(node)
            }
        }
    }

    /// Runs `f` with one of upstream's visitors over `v`'s factory.
    fn with_visitor<R>(
        &self,
        v: &mut NodeVisitor<'_>,
        which: Visitor,
        f: impl FnOnce(&mut NodeVisitor<'_>) -> R,
    ) -> R {
        let visit = |visitor: &mut NodeVisitor<'_>, node: Option<NodeId>| -> Option<NodeId> {
            self.dispatch(which, visitor, node)
        };
        let mut created = self.hooks.new_node_visitor(Some(&visit), v.factory_mut());
        f(&mut created)
    }

    /// `<visitor>.VisitNode(node)`.
    fn visit_node_with(
        &self,
        v: &mut NodeVisitor<'_>,
        which: Visitor,
        node: Option<NodeId>,
    ) -> Option<NodeId> {
        self.with_visitor(v, which, |visitor| visitor.visit_node(node))
    }

    /// `<visitor>.VisitNodes(list)`.
    fn visit_nodes_with(
        &self,
        v: &mut NodeVisitor<'_>,
        which: Visitor,
        list: Option<NodeListId>,
    ) -> Option<NodeListId> {
        self.with_visitor(v, which, |visitor| visitor.visit_nodes(list))
    }

    /// `<visitor>.VisitEachChild(node)`.
    fn visit_each_child_with(
        &self,
        v: &mut NodeVisitor<'_>,
        which: Visitor,
        node: NodeId,
    ) -> Option<NodeId> {
        self.with_visitor(v, which, |visitor| visitor.visit_each_child(Some(node)))
    }

    /// `<visitor>.VisitEmbeddedStatement(node)`.
    fn visit_embedded_statement_with(
        &self,
        v: &mut NodeVisitor<'_>,
        which: Visitor,
        node: Option<NodeId>,
    ) -> Option<NodeId> {
        self.with_visitor(v, which, |visitor| visitor.visit_embedded_statement(node))
    }

    /// `tx.EmitContext().VisitIterationBody(body, <visitor>)`.
    fn visit_iteration_body_with(
        &self,
        v: &mut NodeVisitor<'_>,
        which: Visitor,
        body: Option<NodeId>,
    ) -> Option<NodeId> {
        let mut ec = self.ec();
        self.with_visitor(v, which, |visitor| ec.visit_iteration_body(body, visitor))
    }

    /// `<visitor>.VisitSlice(nodes)`: the visited nodes and whether any
    /// changed. The slice is allocated in the factory for the visitor.
    fn visit_slice_with(
        &self,
        v: &mut NodeVisitor<'_>,
        which: Visitor,
        nodes: &[NodeId],
    ) -> (Vec<NodeId>, bool) {
        let slice = v
            .factory_mut()
            .alloc_nodes(nodes.iter().copied().map(Some).collect());
        self.with_visitor(v, which, |visitor| {
            let (visited, changed) = visitor.visit_slice(slice);
            let visited = visitor
                .factory()
                .read_nodes(visited)
                .iter()
                .map(|node| node.expect(NIL))
                .collect();
            (visited, changed)
        })
    }

    /// The emit context handle, whose mutators take `&mut self`.
    fn ec(&self) -> EmitContext {
        self.context.clone()
    }

    /// `tx.currentModuleInfo`, set for the file being transformed.
    fn info(&self) -> Rc<ExternalModuleInfo> {
        self.current_module_info.borrow().clone().expect(NIL)
    }

    /// Pushes a new child node onto the ancestor tracking stack, returning the
    /// grandparent node to be restored later via `popNode`.
    // port: tsc/internal/transformers/moduletransforms/commonjsmodule.go:CommonJSModuleTransformer.pushNode
    fn push_node(&self, node: NodeId) -> Option<NodeId> {
        let grandparent_node = self.parent_node.get();
        self.parent_node.set(self.current_node.get());
        self.current_node.set(Some(node));
        grandparent_node
    }

    /// Pops the last child node off the ancestor tracking stack, restoring the
    /// grandparent node.
    // port: tsc/internal/transformers/moduletransforms/commonjsmodule.go:CommonJSModuleTransformer.popNode
    fn pop_node(&self, grandparent_node: Option<NodeId>) {
        self.current_node.set(self.parent_node.get());
        self.parent_node.set(grandparent_node);
    }

    /// Visits a node at the top level of the source file.
    // port: tsc/internal/transformers/moduletransforms/commonjsmodule.go:CommonJSModuleTransformer.visitTopLevel
    fn visit_top_level(&self, v: &mut NodeVisitor<'_>, node: NodeId) -> Visited {
        let grandparent_node = self.push_node(node);
        let result = match v.factory().node(node).kind().known() {
            Some(K::ImportDeclaration) => self.visit_top_level_import_declaration(v, node),
            Some(K::ImportEqualsDeclaration) => {
                self.visit_top_level_import_equals_declaration(v, node)
            }
            Some(K::ExportDeclaration) => self.visit_top_level_export_declaration(v, node),
            Some(K::ExportAssignment) => Ok(self.visit_top_level_export_assignment(v, node)),
            Some(K::FunctionDeclaration) => self.visit_top_level_function_declaration(v, node),
            Some(K::ClassDeclaration) => self.visit_top_level_class_declaration(v, node),
            Some(K::VariableStatement) => self.visit_top_level_variable_statement(v, node),
            _ => self.visit_top_level_nested_no_stack(v, node),
        };
        self.pop_node(grandparent_node);
        result
    }

    /// Visits nested elements at the top-level of a module.
    // port: tsc/internal/transformers/moduletransforms/commonjsmodule.go:CommonJSModuleTransformer.visitTopLevelNested
    fn visit_top_level_nested(&self, v: &mut NodeVisitor<'_>, node: NodeId) -> Visited {
        let grandparent_node = self.push_node(node);
        let result = self.visit_top_level_nested_no_stack(v, node);
        self.pop_node(grandparent_node);
        result
    }

    /// Visits nested elements at the top-level of a module without ancestor
    /// tracking.
    // port: tsc/internal/transformers/moduletransforms/commonjsmodule.go:CommonJSModuleTransformer.visitTopLevelNestedNoStack
    fn visit_top_level_nested_no_stack(&self, v: &mut NodeVisitor<'_>, node: NodeId) -> Visited {
        match v.factory().node(node).kind().known() {
            Some(K::VariableStatement) => self.visit_top_level_variable_statement(v, node),
            Some(K::ForStatement) => self.visit_top_level_nested_for_statement(v, node),
            Some(K::ForInStatement | K::ForOfStatement) => {
                self.visit_top_level_nested_for_in_or_of_statement(v, node)
            }
            Some(K::DoStatement) => Ok(Some(self.visit_top_level_nested_do_statement(v, node))),
            Some(K::WhileStatement) => {
                Ok(Some(self.visit_top_level_nested_while_statement(v, node)))
            }
            Some(K::LabeledStatement) => {
                Ok(Some(self.visit_top_level_nested_labeled_statement(v, node)))
            }
            Some(K::WithStatement) => Ok(Some(self.visit_top_level_nested_with_statement(v, node))),
            Some(K::IfStatement) => Ok(Some(self.visit_top_level_nested_if_statement(v, node))),
            Some(K::SwitchStatement) => {
                Ok(Some(self.visit_top_level_nested_switch_statement(v, node)))
            }
            Some(K::CaseBlock) => Ok(self.visit_top_level_nested_case_block(v, node)),
            Some(K::CaseClause | K::DefaultClause) => Ok(Some(
                self.visit_top_level_nested_case_or_default_clause(v, node),
            )),
            Some(K::TryStatement) => Ok(self.visit_top_level_nested_try_statement(v, node)),
            Some(K::CatchClause) => Ok(Some(self.visit_top_level_nested_catch_clause(v, node))),
            Some(K::Block) => Ok(self.visit_top_level_nested_block(v, node)),
            _ => self.visit_no_stack(v, node, false /*resultIsDiscarded*/),
        }
    }

    /// Visits source elements that are not top-level or top-level nested
    /// statements.
    // port: tsc/internal/transformers/moduletransforms/commonjsmodule.go:CommonJSModuleTransformer.visit
    fn visit(&self, v: &mut NodeVisitor<'_>, node: NodeId) -> Visited {
        let grandparent_node = self.push_node(node);
        let result = self.visit_no_stack(v, node, false /*resultIsDiscarded*/);
        self.pop_node(grandparent_node);
        result
    }

    /// Visits source elements that are not top-level or top-level nested
    /// statements without ancestor tracking.
    // port: tsc/internal/transformers/moduletransforms/commonjsmodule.go:CommonJSModuleTransformer.visitNoStack
    fn visit_no_stack(
        &self,
        v: &mut NodeVisitor<'_>,
        node: NodeId,
        result_is_discarded: bool,
    ) -> Visited {
        let kind = v.factory().node(node).kind();
        // This visitor does not need to descend into the tree if there are no dynamic imports or identifiers in the subtree
        if kind != K::SourceFile
            && subtree_facts(v.factory(), node)? & (DYNAMIC_IMPORT | IDENTIFIER) == 0
        {
            return Ok(Some(node));
        }

        match kind.known() {
            Some(K::SourceFile) => self.visit_source_file(v, node).map(Some),
            Some(K::ForStatement) => Ok(Some(self.visit_for_statement(v, node))),
            Some(K::ForInStatement | K::ForOfStatement) => {
                Ok(Some(self.visit_for_in_or_of_statement(v, node)))
            }
            Some(K::ExpressionStatement) => Ok(self.visit_expression_statement(v, node)),
            Some(K::VoidExpression) => Ok(self.visit_void_expression(v, node)),
            Some(K::ParenthesizedExpression) => Ok(Some(self.visit_parenthesized_expression(
                v,
                node,
                result_is_discarded,
            ))),
            Some(K::PartiallyEmittedExpression) => Ok(Some(
                self.visit_partially_emitted_expression(v, node, result_is_discarded),
            )),
            Some(K::CallExpression) => self.visit_call_expression(v, node),
            Some(K::TaggedTemplateExpression) => self.visit_tagged_template_expression(v, node),
            Some(K::BinaryExpression) => self.visit_binary_expression(v, node, result_is_discarded),
            Some(K::PrefixUnaryExpression) => {
                self.visit_prefix_unary_expression(v, node, result_is_discarded)
            }
            Some(K::PostfixUnaryExpression) => {
                self.visit_postfix_unary_expression(v, node, result_is_discarded)
            }
            Some(K::ShorthandPropertyAssignment) => {
                self.visit_shorthand_property_assignment(v, node)
            }
            Some(K::Identifier) => self.visit_identifier(v, node),
            _ => Ok(self.visit_each_child_with(v, Visitor::Main, node)),
        }
    }

    /// Visits source elements whose value is discarded if they are
    /// expressions.
    // port: tsc/internal/transformers/moduletransforms/commonjsmodule.go:CommonJSModuleTransformer.visitDiscardedValue
    fn visit_discarded_value(&self, v: &mut NodeVisitor<'_>, node: NodeId) -> Visited {
        let grandparent_node = self.push_node(node);
        let result = self.visit_no_stack(v, node, true /*resultIsDiscarded*/);
        self.pop_node(grandparent_node);
        result
    }

    // port: tsc/internal/transformers/moduletransforms/commonjsmodule.go:CommonJSModuleTransformer.visitAssignmentPattern
    fn visit_assignment_pattern(&self, v: &mut NodeVisitor<'_>, node: NodeId) -> Visited {
        let grandparent_node = self.push_node(node);
        let result = self.visit_assignment_pattern_no_stack(v, node);
        self.pop_node(grandparent_node);
        result
    }

    // port: tsc/internal/transformers/moduletransforms/commonjsmodule.go:CommonJSModuleTransformer.visitAssignmentPatternNoStack
    fn visit_assignment_pattern_no_stack(&self, v: &mut NodeVisitor<'_>, node: NodeId) -> Visited {
        match v.factory().node(node).kind().known() {
            // AssignmentPattern
            Some(K::ObjectLiteralExpression | K::ArrayLiteralExpression) => {
                Ok(self.visit_each_child_with(v, Visitor::AssignmentPattern, node))
            }

            // AssignmentProperty
            Some(K::PropertyAssignment) => Ok(Some(self.visit_assignment_property(v, node))),
            Some(K::ShorthandPropertyAssignment) => {
                self.visit_shorthand_assignment_property(v, node).map(Some)
            }

            // AssignmentRestProperty
            Some(K::SpreadAssignment) => self.visit_assignment_rest_property(v, node).map(Some),

            // AssignmentRestElement
            Some(K::SpreadElement) => self.visit_assignment_rest_element(v, node).map(Some),

            // AssignmentElement
            _ => {
                if tsr_ast::utilities_positions::is_expression(view(v.factory())?, node)? {
                    return self.visit_assignment_element(v, node);
                }

                self.visit_no_stack(v, node, false /*resultIsDiscarded*/)
            }
        }
    }

    // port: tsc/internal/transformers/moduletransforms/commonjsmodule.go:CommonJSModuleTransformer.visitSourceFile
    fn visit_source_file(&self, v: &mut NodeVisitor<'_>, node: NodeId) -> Result<NodeId, Error> {
        let (is_declaration_file, is_effective_external) = {
            let file = v.factory().read_source_file(node)?;
            (
                file.is_declaration_file,
                is_effective_external_module(&file, &self.compiler_options),
            )
        };
        if is_declaration_file
            || !(is_effective_external || subtree_facts(v.factory(), node)? & DYNAMIC_IMPORT != 0)
        {
            return Ok(node);
        }

        self.current_source_file.set(Some(node));
        let info = collect_external_module_info(
            v.factory_mut(),
            node,
            &self.compiler_options,
            &self.context,
            &self.resolver,
        )?;
        *self.current_module_info.borrow_mut() = Some(Rc::new(info));
        let updated = self.transform_common_js_module(v, node);
        self.current_source_file.set(None);
        *self.current_module_info.borrow_mut() = None;
        updated
    }

    // port: tsc/internal/transformers/moduletransforms/commonjsmodule.go:CommonJSModuleTransformer.shouldEmitUnderscoreUnderscoreESModule
    fn should_emit_underscore_underscore_es_module(
        &self,
        f: &dyn RuntimeFactory,
    ) -> Result<bool, Error> {
        let current_source_file = self.current_source_file.get().expect(NIL);
        let file = f.read_source_file(current_source_file)?;
        if file_extension_is_one_of(file.file_name(), SUPPORTED_JS_EXTENSIONS_FLAT)
            && file.common_js_module_indicator().is_some()
            && file
                .external_module_indicator
                .is_none_or(|indicator| f.node(indicator).kind() == K::SourceFile)
        {
            return Ok(false);
        }
        if self.info().export_equals.is_none() && is_external_module(&file) {
            return Ok(true);
        }
        Ok(false)
    }

    // port: tsc/internal/transformers/moduletransforms/commonjsmodule.go:CommonJSModuleTransformer.createUnderscoreUnderscoreESModule
    fn create_underscore_underscore_es_module(&self, f: &mut dyn RuntimeFactory) -> NodeId {
        let mut ec = self.ec();
        let object = new_identifier(f, b"Object");
        let define_property = new_identifier(f, b"defineProperty");
        let callee = f.new_property_access_expression(
            Some(object),
            None, /*questionDotToken*/
            Some(define_property),
            node_flags::NONE,
        );
        let exports = new_identifier(f, b"exports");
        let es_module =
            f.new_string_literal(JsString::from_bytes(&b"__esModule"[..]), token_flags::NONE);
        let value = new_identifier(f, b"value");
        let true_expression = ec.new_true_expression(f);
        let property = f.new_property_assignment(
            None, /*modifiers*/
            Some(value),
            None, /*postfixToken*/
            None, /*typeNode*/
            Some(true_expression),
        );
        let properties = new_node_list(f, vec![property]);
        let descriptor =
            f.new_object_literal_expression(Some(properties), false /*multiLine*/);
        let arguments = new_node_list(f, vec![exports, es_module, descriptor]);
        let call = f.new_call_expression(
            Some(callee),
            None, /*questionDotToken*/
            None, /*typeArguments*/
            Some(arguments),
            node_flags::NONE,
        );
        let statement = f.new_expression_statement(Some(call));
        ec.set_emit_flags(statement, emit_flags::CUSTOM_PROLOGUE);
        statement
    }

    // port: tsc/internal/transformers/moduletransforms/commonjsmodule.go:CommonJSModuleTransformer.transformCommonJSModule
    fn transform_common_js_module(
        &self,
        v: &mut NodeVisitor<'_>,
        node: NodeId,
    ) -> Result<NodeId, Error> {
        let mut ec = self.ec();
        ec.start_variable_environment();

        let (source_statements, end_of_file_token) = {
            let read = v.factory().node(node);
            let file = read
                .data_source()
                .as_source_file()
                .expect("SourceFile payload");
            (file.statements().expect(NIL), file.end_of_file_token())
        };
        let source_nodes = list_nodes(v.factory(), Some(source_statements));

        // emit standard prologue directives (e.g. "use strict")
        let (prologue, rest) = ec.split_standard_prologue(v.factory(), &source_nodes);
        let mut statements = prologue.to_vec();

        // emit custom prologues from other transformations
        let (custom, rest) = ec.split_custom_prologue(v.factory(), rest);
        let (custom, rest) = (custom.to_vec(), rest.to_vec());
        statements.extend(self.visit_slice_with(v, Visitor::TopLevel, &custom).0);

        // emits `Object.defineProperty(exports, "__esModule", { value: true });` at the top of the file
        if self.should_emit_underscore_underscore_es_module(v.factory())? {
            statements.push(self.create_underscore_underscore_es_module(v.factory_mut()));
        }

        // initialize all exports to `undefined`, e.g.:
        //  exports.a = exports.b = void 0;
        let info = self.info();
        if !info.exported_names.is_empty() {
            const CHUNK_SIZE: usize = 50;
            let f = v.factory_mut();
            for chunk in info.exported_names.chunks(CHUNK_SIZE) {
                let mut right = ec.new_void_zero_expression(f);
                for &next_id in chunk {
                    let left = if f.node(next_id).kind() == K::StringLiteral {
                        let exports = new_identifier(f, b"exports");
                        let literal = ec.new_string_literal_from_node(f, next_id);
                        f.new_element_access_expression(
                            Some(exports),
                            None, /*questionDotToken*/
                            Some(literal),
                            node_flags::NONE,
                        )
                    } else {
                        let name = tsr_ast::clone_node(f, next_id);
                        ec.set_emit_flags(
                            name,
                            emit_flags::NO_SOURCE_MAP | emit_flags::NO_COMMENTS,
                        );
                        let exports = new_identifier(f, b"exports");
                        f.new_property_access_expression(
                            Some(exports),
                            None, /*questionDotToken*/
                            Some(name),
                            node_flags::NONE,
                        )
                    };
                    right = ec.new_assignment_expression(f, left, right);
                }
                let statement = f.new_expression_statement(Some(right));
                ec.add_emit_flags(statement, emit_flags::CUSTOM_PROLOGUE);
                statements.push(statement);
            }
        }

        // initialize exports for function declarations, e.g.:
        //  exports.f = f;
        //  function f() {}
        // These are marked as custom prologue so they are ordered before the external helpers
        // import declaration (e.g., `const tslib_1 = require("tslib")`), matching TypeScript's emit order.
        let exported_functions_start = statements.len();
        for &function in info.exported_functions.values() {
            self.append_exports_of_class_or_function_declaration(v, &mut statements, function)?;
        }
        for &statement in &statements[exported_functions_start..] {
            ec.add_emit_flags(statement, emit_flags::CUSTOM_PROLOGUE);
        }

        // visit the remaining statements in the source file
        let (rest, _) = self.visit_slice_with(v, Visitor::TopLevel, &rest);
        statements.extend(rest);

        // emit `module.exports = ...` if needd
        self.append_export_equals_if_needed(v, &mut statements);

        // merge temp variables into the statement list
        let statements = ec.end_and_merge_variable_environment(v.factory_mut(), statements);

        let f = v.factory_mut();
        let statement_list = new_node_list(f, statements);
        let loc = f.read_list(source_statements).loc();
        f.set_list_location(statement_list, loc);
        let mut result = f.update_source(node, Some(statement_list), end_of_file_token);
        let helpers = ec.read_emit_helpers();
        ec.add_emit_helper(result, &helpers);

        let file_name = f.read_source_file(node)?.file_name().to_vec();
        let file_module_kind = (self.get_emit_module_format_of_file)(&file_name)?;
        let external_helpers_import_declaration =
            create_external_helpers_import_declaration_if_needed(
                &mut ec,
                f,
                result,
                &self.compiler_options,
                file_module_kind,
                false, /*hasExportStarsToExportValues*/
                false, /*hasImportStar*/
                false, /*hasImportDefault*/
            )?;
        if let Some(external_helpers_import_declaration) = external_helpers_import_declaration {
            let result_statements = f.node(result).statement_list().expect(NIL);
            let result_nodes = list_nodes(f, Some(result_statements));
            let (prologue, rest) = ec.split_standard_prologue(f, &result_nodes);
            let (custom, rest) = ec.split_custom_prologue(f, rest);
            let mut statements = prologue.to_vec();
            statements.extend_from_slice(custom);
            let rest = rest.to_vec();
            statements.push(
                self.visit_node_with(
                    v,
                    Visitor::TopLevel,
                    Some(external_helpers_import_declaration),
                )
                .expect(NIL),
            );
            statements.extend(rest);
            let f = v.factory_mut();
            let statement_list = new_node_list(f, statements);
            let loc = f.read_list(result_statements).loc();
            f.set_list_location(statement_list, loc);
            result = f.update_source(result, Some(statement_list), end_of_file_token);
        }

        Ok(result)
    }

    /// Adds the down-level representation of `export=` to the statement list
    /// if one exists in the source file.
    // port: tsc/internal/transformers/moduletransforms/commonjsmodule.go:CommonJSModuleTransformer.appendExportEqualsIfNeeded
    fn append_export_equals_if_needed(
        &self,
        v: &mut NodeVisitor<'_>,
        statements: &mut Vec<NodeId>,
    ) {
        if let Some(export_equals) = self.info().export_equals {
            let expression_result = self.visit_export_equals(v, export_equals);
            if let Some(expression_result) = expression_result {
                let mut ec = self.ec();
                let f = v.factory_mut();
                let module = new_identifier(f, b"module");
                let exports = new_identifier(f, b"exports");
                let target = f.new_property_access_expression(
                    Some(module),
                    None, /*questionDotToken*/
                    Some(exports),
                    node_flags::NONE,
                );
                let assignment = ec.new_assignment_expression(f, target, expression_result);
                let statement = f.new_expression_statement(Some(assignment));

                ec.assign_comment_and_source_map_ranges(f, statement, export_equals);
                ec.add_emit_flags(statement, emit_flags::NO_COMMENTS);
                statements.push(statement);
            }
        }
    }

    // port: tsc/internal/transformers/moduletransforms/commonjsmodule.go:CommonJSModuleTransformer.visitExportEquals
    fn visit_export_equals(&self, v: &mut NodeVisitor<'_>, node: NodeId) -> Option<NodeId> {
        let grandparent_node = self.push_node(node);
        let expression = v.factory().node(node).expression();
        let result = self.visit_node_with(v, Visitor::Main, expression);
        self.pop_node(grandparent_node);
        result
    }

    /// Appends the exports of an ImportDeclaration to a statement list.
    // port: tsc/internal/transformers/moduletransforms/commonjsmodule.go:CommonJSModuleTransformer.appendExportsOfImportDeclaration
    fn append_exports_of_import_declaration(
        &self,
        v: &mut NodeVisitor<'_>,
        statements: &mut Vec<NodeId>,
        decl: NodeId,
    ) -> Result<(), Error> {
        if self.info().export_equals.is_some() {
            return Ok(());
        }

        let Some(import_clause) = v.factory().node(decl).import_clause() else {
            return Ok(());
        };

        let mut seen = HashSet::new();
        let (name, named_bindings) = {
            let read = v.factory().node(import_clause);
            let clause = read.as_import_clause().expect("ImportClause payload");
            (clause.name(), clause.named_bindings())
        };
        if name.is_some() {
            self.append_exports_of_declaration(
                v,
                statements,
                import_clause,
                Some(&mut seen),
                false, /*liveBinding*/
            )?;
        }

        if let Some(named_bindings) = named_bindings {
            match v.factory().node(named_bindings).kind().known() {
                Some(K::NamespaceImport) => {
                    self.append_exports_of_declaration(
                        v,
                        statements,
                        named_bindings,
                        Some(&mut seen),
                        false, /*liveBinding*/
                    )?;
                }

                Some(K::NamedImports) => {
                    let elements = v.factory().node(named_bindings).element_list();
                    for import_binding in list_nodes(v.factory(), elements) {
                        self.append_exports_of_declaration(
                            v,
                            statements,
                            import_binding,
                            Some(&mut seen),
                            true, /*liveBinding*/
                        )?;
                    }
                }
                _ => {}
            }
        }

        Ok(())
    }

    /// Appends the exports of a VariableStatement to a statement list.
    // port: tsc/internal/transformers/moduletransforms/commonjsmodule.go:CommonJSModuleTransformer.appendExportsOfVariableStatement
    fn append_exports_of_variable_statement(
        &self,
        v: &mut NodeVisitor<'_>,
        statements: &mut Vec<NodeId>,
        node: NodeId,
    ) -> Result<(), Error> {
        let declaration_list = v
            .factory()
            .node(node)
            .as_variable_statement()
            .expect("VariableStatement payload")
            .declaration_list()
            .expect(NIL);
        self.append_exports_of_variable_declaration_list(
            v,
            statements,
            declaration_list,
            false, /*isForInOrOfInitializer*/
        )
    }

    /// Appends the exports of a VariableDeclarationList to a statement list.
    // port: tsc/internal/transformers/moduletransforms/commonjsmodule.go:CommonJSModuleTransformer.appendExportsOfVariableDeclarationList
    fn append_exports_of_variable_declaration_list(
        &self,
        v: &mut NodeVisitor<'_>,
        statements: &mut Vec<NodeId>,
        node: NodeId,
        is_for_in_or_of_initializer: bool,
    ) -> Result<(), Error> {
        if self.info().export_equals.is_some() {
            return Ok(());
        }

        let declarations = v
            .factory()
            .node(node)
            .as_variable_declaration_list()
            .expect("VariableDeclarationList payload")
            .declarations();
        for decl in list_nodes(v.factory(), declarations) {
            self.append_exports_of_binding_element(
                v,
                statements,
                decl,
                is_for_in_or_of_initializer,
            )?;
        }

        Ok(())
    }

    /// Appends the exports of a VariableDeclaration or BindingElement to a
    /// statement list.
    // port: tsc/internal/transformers/moduletransforms/commonjsmodule.go:CommonJSModuleTransformer.appendExportsOfBindingElement
    fn append_exports_of_binding_element(
        &self,
        v: &mut NodeVisitor<'_>,
        statements: &mut Vec<NodeId>,
        decl: NodeId, /*VariableDeclaration | BindingElement*/
        is_for_in_or_of_initializer: bool,
    ) -> Result<(), Error> {
        if self.info().export_equals.is_some() {
            return Ok(());
        }
        let Some(name) = v.factory().node(decl).name() else {
            return Ok(());
        };

        if is_binding_pattern(&v.factory().node(name)) {
            let elements = v.factory().node(name).element_list();
            for element in list_nodes(v.factory(), elements) {
                if !tsr_ast::is_omitted_expression(&v.factory().node(element)) {
                    self.append_exports_of_binding_element(
                        v,
                        statements,
                        element,
                        is_for_in_or_of_initializer,
                    )?;
                }
            }
        } else if !is_generated_identifier(&self.context, name) && {
            let read = v.factory().node(decl);
            !tsr_ast::is_variable_declaration(&read)
                || read.initializer().is_some()
                || is_for_in_or_of_initializer
        } {
            self.append_exports_of_declaration(
                v, statements, decl, None,  /*seen*/
                false, /*liveBinding*/
            )?;
        }

        Ok(())
    }

    /// Appends the exports of a ClassDeclaration or FunctionDeclaration to a
    /// statement list.
    // port: tsc/internal/transformers/moduletransforms/commonjsmodule.go:CommonJSModuleTransformer.appendExportsOfClassOrFunctionDeclaration
    fn append_exports_of_class_or_function_declaration(
        &self,
        v: &mut NodeVisitor<'_>,
        statements: &mut Vec<NodeId>,
        decl: NodeId,
    ) -> Result<(), Error> {
        if self.info().export_equals.is_some() {
            return Ok(());
        }

        let mut ec = self.ec();
        let mut seen = HashSet::new();
        if has_syntactic_modifier(view(v.factory())?, decl, modifier_flags::EXPORT)? {
            let export_name =
                if has_syntactic_modifier(view(v.factory())?, decl, modifier_flags::DEFAULT)? {
                    new_identifier(v.factory_mut(), b"default")
                } else {
                    ec.get_declaration_name(builder(v.factory_mut())?, Some(decl))?
                };

            let export_value = ec.get_local_name(builder(v.factory_mut())?, Some(decl))?;
            let location = v.factory().node(decl).range();
            self.append_export_statement(
                v.factory_mut(),
                statements,
                &mut seen,
                export_name,
                export_value,
                Some(location),
                false, /*allowComments*/
                false, /*liveBinding*/
            )?;
        }

        if v.factory().node(decl).name().is_some() {
            return self.append_exports_of_declaration(
                v,
                statements,
                decl,
                Some(&mut seen),
                false, /*liveBinding*/
            );
        }

        Ok(())
    }

    /// Appends the exports of a declaration to a statement list.
    // port: tsc/internal/transformers/moduletransforms/commonjsmodule.go:CommonJSModuleTransformer.appendExportsOfDeclaration
    fn append_exports_of_declaration(
        &self,
        v: &mut NodeVisitor<'_>,
        statements: &mut Vec<NodeId>,
        decl: NodeId,
        seen: Option<&mut HashSet<JsString>>,
        live_binding: bool,
    ) -> Result<(), Error> {
        let info = self.info();
        if info.export_equals.is_some() {
            return Ok(());
        }

        let mut own_seen = HashSet::new();
        let seen = seen.unwrap_or(&mut own_seen);

        let name = v.factory().node(decl).name();
        if !info.export_specifiers.is_empty()
            && name.is_some_and(|name| tsr_ast::is_identifier(&v.factory().node(name)))
        {
            let mut ec = self.ec();
            let name = ec.get_declaration_name(builder(v.factory_mut())?, Some(decl))?;
            let export_specifiers = info.export_specifiers.get(&node_text(v.factory(), name)?);
            if !export_specifiers.is_empty() {
                let export_value = self.visit_expression_identifier(v.factory_mut(), name)?;
                for &export_specifier in export_specifiers {
                    let specifier_name = v.factory().node(export_specifier).name().expect(NIL);
                    let location = v.factory().node(specifier_name).range();
                    self.append_export_statement(
                        v.factory_mut(),
                        statements,
                        seen,
                        specifier_name,
                        export_value,
                        Some(location), /*location*/
                        false,          /*allowComments*/
                        live_binding,
                    )?;
                }
            }
        }

        Ok(())
    }

    /// Appends the down-level representation of an export to a statement list.
    #[allow(clippy::too_many_arguments)]
    // port: tsc/internal/transformers/moduletransforms/commonjsmodule.go:CommonJSModuleTransformer.appendExportStatement
    fn append_export_statement(
        &self,
        f: &mut dyn RuntimeFactory,
        statements: &mut Vec<NodeId>,
        seen: &mut HashSet<JsString>,
        export_name: NodeId, /*ModuleExportName*/
        expression: NodeId,
        location: Option<TextRange>,
        allow_comments: bool,
        live_binding: bool,
    ) -> Result<(), Error> {
        if f.node(export_name).kind() != K::StringLiteral {
            let text = node_text(f, export_name)?;
            if seen.contains(&text) {
                return Ok(());
            }
            seen.insert(text);
        }
        statements.push(self.create_export_statement(
            f,
            export_name,
            expression,
            location,
            allow_comments,
            live_binding,
        ));
        Ok(())
    }

    /// Creates a call to the current file's export function to export a value.
    // port: tsc/internal/transformers/moduletransforms/commonjsmodule.go:CommonJSModuleTransformer.createExportStatement
    fn create_export_statement(
        &self,
        f: &mut dyn RuntimeFactory,
        name: NodeId, /*ModuleExportName*/
        value: NodeId,
        location: Option<TextRange>,
        allow_comments: bool,
        live_binding: bool,
    ) -> NodeId {
        let mut ec = self.ec();
        let expression =
            self.create_export_expression(f, name, value, None /*location*/, live_binding);
        let statement = f.new_expression_statement(Some(expression));
        if let Some(location) = location {
            ec.set_comment_range(statement, location);
        }
        ec.add_emit_flags(statement, emit_flags::START_ON_NEW_LINE);
        if !allow_comments {
            ec.add_emit_flags(statement, emit_flags::NO_COMMENTS);
        }
        statement
    }

    /// Creates a call to the current file's export function to export a value.
    // port: tsc/internal/transformers/moduletransforms/commonjsmodule.go:CommonJSModuleTransformer.createExportExpression
    fn create_export_expression(
        &self,
        f: &mut dyn RuntimeFactory,
        name: NodeId, /*ModuleExportName*/
        value: NodeId,
        location: Option<TextRange>,
        live_binding: bool,
    ) -> NodeId {
        let mut ec = self.ec();
        let expression = if live_binding {
            // For a live binding we emit a getter on `exports` that returns the value:
            //  Object.defineProperty(exports, "<name>", { enumerable: true, get: function () { return <value>; } });
            let object = new_identifier(f, b"Object");
            let define_property = new_identifier(f, b"defineProperty");
            let callee = f.new_property_access_expression(
                Some(object),
                None, /*questionDotToken*/
                Some(define_property),
                node_flags::NONE,
            );
            let exports = new_identifier(f, b"exports");
            let name_literal = ec.new_string_literal_from_node(f, name);
            let enumerable = new_identifier(f, b"enumerable");
            let true_expression = ec.new_true_expression(f);
            let enumerable = f.new_property_assignment(
                None, /*modifiers*/
                Some(enumerable),
                None, /*postfixToken*/
                None, /*typeNode*/
                Some(true_expression),
            );
            let get = new_identifier(f, b"get");
            let parameters = new_node_list(f, Vec::new());
            let return_statement = f.new_return_statement(Some(value));
            let body_statements = new_node_list(f, vec![return_statement]);
            let body = f.new_block(Some(body_statements), false /*multiLine*/);
            let getter = f.new_function_expression(
                None, /*modifiers*/
                None, /*asteriskToken*/
                None, /*name*/
                None, /*typeParameters*/
                Some(parameters),
                None, /*type*/
                None, /*fullSignature*/
                Some(body),
            );
            let get = f.new_property_assignment(
                None, /*modifiers*/
                Some(get),
                None, /*postfixToken*/
                None, /*typeNode*/
                Some(getter),
            );
            let properties = new_node_list(f, vec![enumerable, get]);
            let descriptor =
                f.new_object_literal_expression(Some(properties), false /*multiLine*/);
            let arguments = new_node_list(f, vec![exports, name_literal, descriptor]);
            f.new_call_expression(
                Some(callee),
                None, /*questionDotToken*/
                None, /*typeArguments*/
                Some(arguments),
                node_flags::NONE,
            )
        } else {
            // Otherwise, we emit a simple property assignment.
            let left = if f.node(name).kind() == K::StringLiteral {
                // emits:
                //  exports["<name>"] = <value>;
                let exports = new_identifier(f, b"exports");
                let literal = ec.new_string_literal_from_node(f, name);
                f.new_element_access_expression(
                    Some(exports),
                    None, /*questionDotToken*/
                    Some(literal),
                    node_flags::NONE,
                )
            } else {
                // emits:
                //  exports.<name> = <value>;
                let exports = new_identifier(f, b"exports");
                let name = tsr_ast::clone_node(f, name);
                f.new_property_access_expression(
                    Some(exports),
                    None, /*questionDotToken*/
                    Some(name),
                    node_flags::NONE,
                )
            };
            ec.new_assignment_expression(f, left, value)
        };
        if let Some(location) = location {
            ec.set_comment_range(expression, location);
        }
        expression
    }

    /// Creates a `require()` call to import an external module.
    // port: tsc/internal/transformers/moduletransforms/commonjsmodule.go:CommonJSModuleTransformer.createRequireCall
    fn create_require_call(
        &self,
        f: &mut dyn RuntimeFactory,
        node: NodeId, /*ImportDeclaration | ImportEqualsDeclaration | ExportDeclaration*/
    ) -> Result<NodeId, Error> {
        let mut args = Vec::new();
        let module_name = get_external_module_name_literal(
            f,
            node,
            self.current_source_file.get().expect(NIL),
            None, /*resolver*/
            &self.compiler_options,
        )?;
        if let Some(module_name) = module_name {
            let mut ec = self.ec();
            args.push(
                rewrite_module_specifier(&mut ec, f, Some(module_name), &self.compiler_options)
                    .expect(NIL),
            );
        }
        let require = new_identifier(f, b"require");
        let arguments = new_node_list(f, args);
        Ok(f.new_call_expression(
            Some(require),
            None, /*questionDotToken*/
            None, /*typeArguments*/
            Some(arguments),
            node_flags::NONE,
        ))
    }

    // port: tsc/internal/transformers/moduletransforms/commonjsmodule.go:CommonJSModuleTransformer.getHelperExpressionForExport
    fn get_helper_expression_for_export(
        &self,
        v: &mut NodeVisitor<'_>,
        node: NodeId, /*ExportDeclaration*/
        inner_expr: NodeId,
    ) -> Result<NodeId, Error> {
        if get_export_needs_import_star_helper(v.factory(), node)? {
            let helper = self
                .ec()
                .new_import_star_helper(v.factory_mut(), inner_expr);
            return Ok(self
                .visit_node_with(v, Visitor::Main, Some(helper))
                .expect(NIL));
        }
        Ok(inner_expr)
    }

    // port: tsc/internal/transformers/moduletransforms/commonjsmodule.go:CommonJSModuleTransformer.getHelperExpressionForImport
    fn get_helper_expression_for_import(
        &self,
        v: &mut NodeVisitor<'_>,
        node: NodeId, /*ImportDeclaration*/
        inner_expr: NodeId,
    ) -> Result<NodeId, Error> {
        if get_import_needs_import_star_helper(v.factory(), node)? {
            let helper = self
                .ec()
                .new_import_star_helper(v.factory_mut(), inner_expr);
            return Ok(self
                .visit_node_with(v, Visitor::Main, Some(helper))
                .expect(NIL));
        }
        if get_import_needs_import_default_helper(v.factory(), node)? {
            let helper = self
                .ec()
                .new_import_default_helper(v.factory_mut(), inner_expr);
            return Ok(self
                .visit_node_with(v, Visitor::Main, Some(helper))
                .expect(NIL));
        }
        Ok(inner_expr)
    }

    // port: tsc/internal/transformers/moduletransforms/commonjsmodule.go:CommonJSModuleTransformer.visitTopLevelImportDeclaration
    fn visit_top_level_import_declaration(&self, v: &mut NodeVisitor<'_>, node: NodeId) -> Visited {
        let mut ec = self.ec();
        if v.factory().node(node).import_clause().is_none() {
            // import "mod";
            let f = v.factory_mut();
            let require = self.create_require_call(f, node)?;
            let statement = f.new_expression_statement(Some(require));
            ec.set_original(statement, node);
            ec.assign_comment_and_source_map_ranges(f, statement, node);
            return Ok(Some(statement));
        }

        let mut statements = Vec::new();
        let mut variables = Vec::new();
        let namespace_declaration = get_namespace_declaration_node(view(v.factory())?, node)?;
        // `ast.IsDefaultImport(node)`, which upstream reads only beside a
        // namespace declaration.
        let is_default = match namespace_declaration {
            Some(_) => is_default_import(view(v.factory())?, &v.factory().node(node))?,
            None => false,
        };
        if let Some(namespace_declaration) = namespace_declaration.filter(|_| !is_default) {
            // import * as n from "mod";
            let name = v.factory().node(namespace_declaration).name().expect(NIL);
            let name = tsr_ast::clone_node(v.factory_mut(), name);
            let require = self.create_require_call(v.factory_mut(), node)?;
            let initializer = self.get_helper_expression_for_import(v, node, require)?;
            variables.push(v.factory_mut().new_variable_declaration(
                Some(name),
                None, /*exclamationToken*/
                None, /*type*/
                Some(initializer),
            ));
        } else {
            // import d from "mod";
            // import { x, y } from "mod";
            // import d, { x, y } from "mod";
            // import d, * as n from "mod";
            let generated_name = ec.new_generated_name_for_node(v.factory_mut(), node);
            let require = self.create_require_call(v.factory_mut(), node)?;
            let initializer = self.get_helper_expression_for_import(v, node, require)?;
            variables.push(v.factory_mut().new_variable_declaration(
                Some(generated_name),
                None, /*exclamationToken*/
                None, /*type*/
                Some(initializer),
            ));

            if let Some(namespace_declaration) = namespace_declaration.filter(|_| is_default) {
                let f = v.factory_mut();
                let name = f.node(namespace_declaration).name().expect(NIL);
                let name = tsr_ast::clone_node(f, name);
                let generated_name = ec.new_generated_name_for_node(f, node);
                variables.push(f.new_variable_declaration(
                    Some(name),
                    None, /*exclamationToken*/
                    None, /*type*/
                    Some(generated_name),
                ));
            }
        }

        let f = v.factory_mut();
        let variables = new_node_list(f, variables);
        let declaration_list = f.new_variable_declaration_list(Some(variables), node_flags::CONST);
        let var_statement =
            f.new_variable_statement(None /*modifiers*/, Some(declaration_list));

        ec.set_original(var_statement, node);
        ec.assign_comment_and_source_map_ranges(f, var_statement, node);
        statements.push(var_statement);
        self.append_exports_of_import_declaration(v, &mut statements, node)?;
        Ok(single_or_many_of(v.factory_mut(), &statements))
    }

    /// # Panics
    ///
    /// For an `import =` of an internal module reference, as upstream.
    // port: tsc/internal/transformers/moduletransforms/commonjsmodule.go:CommonJSModuleTransformer.visitTopLevelImportEqualsDeclaration
    fn visit_top_level_import_equals_declaration(
        &self,
        v: &mut NodeVisitor<'_>,
        node: NodeId,
    ) -> Visited {
        // import m = n;
        assert!(
            is_external_module_import_equals_declaration(view(v.factory())?, node)?,
            "import= for internal module references should be handled in an earlier transformer."
        );

        let mut ec = self.ec();
        let mut statements = Vec::new();
        let name = v.factory().node(node).name();
        if has_syntactic_modifier(view(v.factory())?, node, modifier_flags::EXPORT)? {
            // export import m = require("mod");
            let f = v.factory_mut();
            let require = self.create_require_call(f, node)?;
            let location = f.node(node).range();
            let expression = self.create_export_expression(
                f,
                name.expect(NIL),
                require,
                Some(location),
                false, /*liveBinding*/
            );
            let statement = f.new_expression_statement(Some(expression));

            ec.set_original(statement, node);
            ec.assign_comment_and_source_map_ranges(f, statement, node);
            statements.push(statement);
        } else {
            // import m = require("mod");
            let f = v.factory_mut();
            let name = tsr_ast::clone_node(f, name.expect(NIL));
            let require = self.create_require_call(f, node)?;
            let declaration = f.new_variable_declaration(
                Some(name),
                None, /*exclamationToken*/
                None, /*typeNode*/
                Some(require),
            );
            let declarations = new_node_list(f, vec![declaration]);
            let declaration_list =
                f.new_variable_declaration_list(Some(declarations), node_flags::CONST);
            let statement =
                f.new_variable_statement(None /*modifiers*/, Some(declaration_list));
            ec.set_original(statement, node);
            ec.assign_comment_and_source_map_ranges(f, statement, node);
            statements.push(statement);
        }

        self.append_exports_of_declaration(
            v,
            &mut statements,
            node,
            None,  /*seen*/
            false, /*liveBinding*/
        )?;
        Ok(single_or_many_of(v.factory_mut(), &statements))
    }

    // port: tsc/internal/transformers/moduletransforms/commonjsmodule.go:CommonJSModuleTransformer.visitTopLevelExportDeclaration
    fn visit_top_level_export_declaration(&self, v: &mut NodeVisitor<'_>, node: NodeId) -> Visited {
        let (module_specifier, export_clause) = {
            let read = v.factory().node(node);
            let data = read
                .as_export_declaration()
                .expect("ExportDeclaration payload");
            (data.module_specifier(), data.export_clause())
        };
        if module_specifier.is_none() {
            // Elide export declarations with no module specifier as they are handled
            // elsewhere.
            return Ok(None);
        }

        let mut ec = self.ec();
        let generated_name = ec.new_generated_name_for_node(v.factory_mut(), node);
        if let Some(export_clause) = export_clause
            .filter(|&export_clause| tsr_ast::is_named_exports(&v.factory().node(export_clause)))
        {
            // export { x, y } from "mod";
            let mut statements = Vec::new();
            let f = v.factory_mut();
            let require = self.create_require_call(f, node)?;
            let declaration = f.new_variable_declaration(
                Some(generated_name),
                None, /*exclamationToken*/
                None, /*type*/
                Some(require),
            );
            let declarations = new_node_list(f, vec![declaration]);
            let declaration_list =
                f.new_variable_declaration_list(Some(declarations), node_flags::NONE);
            let var_statement =
                f.new_variable_statement(None /*modifiers*/, Some(declaration_list));
            ec.set_original(var_statement, node);
            ec.assign_comment_and_source_map_ranges(f, var_statement, node);
            statements.push(var_statement);

            let elements = f.node(export_clause).element_list();
            for specifier in list_nodes(f, elements) {
                let specifier_name = f.node(specifier).property_name_or_name().expect(NIL);
                let export_needs_import_default =
                    module_export_name_is_default(view(f)?, specifier_name)?;

                let target = if export_needs_import_default {
                    ec.new_import_default_helper(f, generated_name)
                } else {
                    generated_name
                };

                let name = f.node(specifier).name().expect(NIL);
                let export_name = if tsr_ast::is_string_literal(&f.node(name)) {
                    ec.new_string_literal_from_node(f, name)
                } else {
                    ec.get_export_name(builder(f)?, Some(specifier))?
                };

                let exported_value = if tsr_ast::is_string_literal(&f.node(specifier_name)) {
                    f.new_element_access_expression(
                        Some(target),
                        None, /*questionDotToken*/
                        Some(specifier_name),
                        node_flags::NONE,
                    )
                } else {
                    f.new_property_access_expression(
                        Some(target),
                        None, /*questionDotToken*/
                        Some(specifier_name),
                        node_flags::NONE,
                    )
                };
                let expression = self.create_export_expression(
                    f,
                    export_name,
                    exported_value,
                    None, /*location*/
                    true, /*liveBinding*/
                );
                let statement = f.new_expression_statement(Some(expression));
                ec.set_original(statement, specifier);
                ec.assign_comment_and_source_map_ranges(f, statement, specifier);
                statements.push(statement);
            }

            return Ok(single_or_many_of(f, &statements));
        }

        if let Some(export_clause) = export_clause {
            // export * as ns from "mod";
            // export * as default from "mod";
            let f = v.factory_mut();
            let clause_name = f.node(export_clause).name().expect(NIL);
            let export_name = if tsr_ast::is_string_literal(&f.node(clause_name)) {
                ec.new_string_literal_from_node(f, clause_name)
            } else {
                tsr_ast::clone_node(f, clause_name)
            };
            let require = self.create_require_call(f, node)?;
            let value = self.get_helper_expression_for_export(v, node, require)?;
            let f = v.factory_mut();
            let expression = self.create_export_expression(
                f,
                export_name,
                value,
                None,  /*location*/
                false, /*liveBinding*/
            );
            let statement = f.new_expression_statement(Some(expression));
            ec.set_original(statement, node);
            ec.assign_comment_and_source_map_ranges(f, statement, node);
            return Ok(Some(statement));
        }

        // export * from "mod";
        let f = v.factory_mut();
        let require = self.create_require_call(f, node)?;
        let exports = new_identifier(f, b"exports");
        let helper = ec.new_export_star_helper(f, require, exports);
        let expression = self.visit_node_with(v, Visitor::Main, Some(helper));
        let f = v.factory_mut();
        let statement = f.new_expression_statement(expression);
        ec.set_original(statement, node);
        ec.assign_comment_and_source_map_ranges(f, statement, node);
        Ok(Some(statement))
    }

    // port: tsc/internal/transformers/moduletransforms/commonjsmodule.go:CommonJSModuleTransformer.visitTopLevelExportAssignment
    fn visit_top_level_export_assignment(
        &self,
        v: &mut NodeVisitor<'_>,
        node: NodeId,
    ) -> Option<NodeId> {
        let (is_export_equals, expression, location) = {
            let read = v.factory().node(node);
            let data = read
                .as_export_assignment()
                .expect("ExportAssignment payload");
            (data.is_export_equals(), data.expression(), read.range())
        };
        if is_export_equals {
            return None;
        }

        let default = new_identifier(v.factory_mut(), b"default");
        let value = self
            .visit_node_with(v, Visitor::Main, expression)
            .expect(NIL);
        Some(self.create_export_statement(
            v.factory_mut(),
            default,
            value,
            Some(location), /*location*/
            true,           /*allowComments*/
            false,          /*liveBinding*/
        ))
    }

    // port: tsc/internal/transformers/moduletransforms/commonjsmodule.go:CommonJSModuleTransformer.visitTopLevelFunctionDeclaration
    fn visit_top_level_function_declaration(
        &self,
        v: &mut NodeVisitor<'_>,
        node: NodeId,
    ) -> Visited {
        if has_syntactic_modifier(view(v.factory())?, node, modifier_flags::EXPORT)? {
            let (modifiers, asterisk_token, parameters, body) = {
                let read = v.factory().node(node);
                let data = read
                    .as_function_declaration()
                    .expect("FunctionDeclaration payload");
                (
                    data.modifiers(),
                    data.asterisk_token(),
                    data.parameters(),
                    data.body(),
                )
            };
            let mut ec = self.ec();
            let modifiers = extract_modifiers(
                &ec,
                v.factory_mut(),
                modifiers,
                !modifier_flags::EXPORT_DEFAULT,
            );
            let name = ec.get_declaration_name(builder(v.factory_mut())?, Some(node))?;
            let parameters = self.visit_nodes_with(v, Visitor::Main, parameters);
            let body = self.visit_node_with(v, Visitor::Main, body);
            Ok(Some(v.factory_mut().update_function_declaration(
                node,
                modifiers,
                asterisk_token,
                Some(name),
                None, /*typeParameters*/
                parameters,
                None, /*type*/
                None, /*fullSignature*/
                body,
            )))
        } else {
            Ok(self.visit_each_child_with(v, Visitor::Main, node))
        }
    }

    // port: tsc/internal/transformers/moduletransforms/commonjsmodule.go:CommonJSModuleTransformer.visitTopLevelClassDeclaration
    fn visit_top_level_class_declaration(&self, v: &mut NodeVisitor<'_>, node: NodeId) -> Visited {
        let mut statements = Vec::new();
        if has_syntactic_modifier(view(v.factory())?, node, modifier_flags::EXPORT)? {
            let (modifiers, heritage_clauses, members) = {
                let read = v.factory().node(node);
                let data = read
                    .as_class_declaration()
                    .expect("ClassDeclaration payload");
                (data.modifiers(), data.heritage_clauses(), data.members())
            };
            let mut ec = self.ec();
            let extracted = extract_modifiers(
                &ec,
                v.factory_mut(),
                modifiers,
                !modifier_flags::EXPORT_DEFAULT,
            );
            let modifiers = self.with_visitor(v, Visitor::Main, |m| m.visit_modifiers(extracted));
            let name = ec.get_declaration_name(builder(v.factory_mut())?, Some(node))?;
            let heritage_clauses = self.visit_nodes_with(v, Visitor::Main, heritage_clauses);
            let members = self.visit_nodes_with(v, Visitor::Main, members);
            statements.push(v.factory_mut().update_class_declaration(
                node,
                modifiers,
                Some(name),
                None, /*typeParameters*/
                heritage_clauses,
                members,
            ));
        } else {
            statements.push(
                self.visit_each_child_with(v, Visitor::Main, node)
                    .expect(NIL),
            );
        }
        self.append_exports_of_class_or_function_declaration(v, &mut statements, node)?;
        Ok(single_or_many_of(v.factory_mut(), &statements))
    }

    // port: tsc/internal/transformers/moduletransforms/commonjsmodule.go:CommonJSModuleTransformer.visitTopLevelVariableStatement
    fn visit_top_level_variable_statement(&self, v: &mut NodeVisitor<'_>, node: NodeId) -> Visited {
        if !has_syntactic_modifier(view(v.factory())?, node, modifier_flags::EXPORT)? {
            return self.visit_top_level_nested_variable_statement(v, node);
        }

        // export var a = b;
        let mut ec = self.ec();
        let mut pending = PendingVariableStatement::default();

        let (node_modifiers, declaration_list) = {
            let read = v.factory().node(node);
            let data = read
                .as_variable_statement()
                .expect("VariableStatement payload");
            (data.modifiers(), data.declaration_list().expect(NIL))
        };
        let declarations = v
            .factory()
            .node(declaration_list)
            .as_variable_declaration_list()
            .expect("VariableDeclarationList payload")
            .declarations();

        // If we're exporting these variables, then these just become assignments to 'exports.x'.
        for mut variable in list_nodes(v.factory(), declarations) {
            let (name, exclamation_token, type_node, initializer) = {
                let read = v.factory().node(variable);
                let data = read
                    .as_variable_declaration()
                    .expect("VariableDeclaration payload");
                (
                    data.name().expect(NIL),
                    data.exclamation_token(),
                    data.r#type(),
                    data.initializer(),
                )
            };
            let name_is_identifier = tsr_ast::is_identifier(&v.factory().node(name));
            let name_is_binding_pattern = is_binding_pattern(&v.factory().node(name));

            if name_is_identifier && is_local_name(&ec, name) {
                // A "local name" generally means a variable declaration that *shouldn't* be
                // converted to `exports.x = ...`, even if the declaration is exported. This
                // usually indicates a class or function declaration that was converted into
                // a variable declaration, as most references to the declaration will remain
                // untransformed (i.e., `new C` rather than `new exports.C`). In these cases,
                // an `export { x }` declaration will follow.

                if pending.modifiers.is_none() {
                    pending.modifiers = extract_modifiers(
                        &ec,
                        v.factory_mut(),
                        node_modifiers,
                        !modifier_flags::EXPORT_DEFAULT,
                    );
                }

                if initializer.is_some() {
                    let value = self
                        .visit_node_with(v, Visitor::Main, initializer)
                        .expect(NIL);
                    let f = v.factory_mut();
                    let export_expression = self
                        .create_export_expression(f, name, value, None, false /*liveBinding*/);
                    variable = f.update_variable_declaration(
                        variable,
                        Some(name),
                        None, /*exclamationToken*/
                        None, /*type*/
                        Some(export_expression),
                    );
                }

                pending.push_variable(&mut ec, v.factory_mut(), node, variable);
            } else if initializer.is_some_and(|initializer| {
                let read = v.factory().node(initializer);
                !name_is_binding_pattern
                    && (tsr_ast::is_arrow_function(&read)
                        || tsr_ast::is_function_expression(&read)
                        || tsr_ast::is_class_expression(&read))
            }) {
                // preserve variable declarations for functions and classes to assign names

                let initializer = self.visit_node_with(v, Visitor::Main, initializer);
                let f = v.factory_mut();
                let declaration = f.new_variable_declaration(
                    Some(name),
                    exclamation_token,
                    type_node,
                    initializer,
                );
                pending.push_variable(&mut ec, f, node, declaration);

                let exports = new_identifier(f, b"exports");
                let property_access = f.new_property_access_expression(
                    Some(exports),
                    None, /*questionDotToken*/
                    Some(name),
                    node_flags::NONE,
                );
                ec.assign_comment_and_source_map_ranges(f, property_access, name);

                let value = tsr_ast::clone_node(f, name);
                let assignment = ec.new_assignment_expression(f, property_access, value);
                pending.push_expression(&mut ec, f, node, assignment);
            } else if name_is_identifier {
                let expression = self.transform_initialized_variable(v, variable)?;
                if let Some(expression) = expression {
                    let visited = self
                        .visit_node_with(v, Visitor::Main, Some(expression))
                        .expect(NIL);
                    pending.push_expression(&mut ec, v.factory_mut(), node, visited);
                }
            } else if name_is_binding_pattern {
                // For binding patterns with export modifier, use flattenDestructuringAssignment
                // to decompose into individual export assignments
                let expression = self.transform_initialized_variable(v, variable)?;
                if let Some(expression) = expression {
                    pending.push_expression(&mut ec, v.factory_mut(), node, expression);
                }
            } else {
                // For binding patterns, we can't do exports.{pattern} = value
                // Just emit the assignment and let appendExportsOfVariableStatement handle the exports
                let expression = convert_variable_declaration_to_assignment_expression(
                    &ec,
                    v.factory_mut(),
                    variable,
                );
                if expression.is_some() {
                    let visited = self
                        .visit_node_with(v, Visitor::Main, expression)
                        .expect(NIL);
                    pending.push_expression(&mut ec, v.factory_mut(), node, visited);
                }
            }
        }

        pending.commit_pending_variables(&mut ec, v.factory_mut(), node);
        pending.commit_pending_expressions(&mut ec, v.factory_mut(), node);
        let mut statements = pending.statements;
        self.append_exports_of_variable_statement(v, &mut statements, node)?;
        Ok(single_or_many_of(v.factory_mut(), &statements))
    }

    // port: tsc/internal/transformers/moduletransforms/commonjsmodule.go:CommonJSModuleTransformer.transformInitializedVariable
    fn transform_initialized_variable(&self, v: &mut NodeVisitor<'_>, node: NodeId) -> Visited {
        let (name, initializer) = {
            let read = v.factory().node(node);
            let data = read
                .as_variable_declaration()
                .expect("VariableDeclaration payload");
            (data.name().expect(NIL), data.initializer())
        };
        let Some(initializer) = initializer else {
            return Ok(None);
        };
        if is_binding_pattern(&v.factory().node(name)) {
            // Convert the binding pattern into an equivalent assignment expression and visit it
            // as a destructuring assignment. This preserves native destructuring (and therefore
            // iterator semantics for array patterns) whenever each leaf identifier can be
            // substituted to an export reference. Only when the destructuring would assign to
            // re-aliased or multi-exported names (where native destructuring cannot update all
            // targets) does `visitDestructuringAssignment` fall back to flattening.
            let assignment = convert_variable_declaration_to_assignment_expression(
                &self.context,
                v.factory_mut(),
                node,
            )
            .expect(NIL);
            let grandparent_node = self.push_node(assignment);
            let result =
                self.visit_destructuring_assignment(v, assignment, true /*valueIsDiscarded*/);
            self.pop_node(grandparent_node);
            return result;
        }
        let mut ec = self.ec();
        let f = v.factory_mut();
        let exports = new_identifier(f, b"exports");
        let property_access = f.new_property_access_expression(
            Some(exports),
            None, /*questionDotToken*/
            Some(name),
            node_flags::NONE,
        );
        ec.assign_comment_and_source_map_ranges(f, property_access, name);
        Ok(Some(ec.new_assignment_expression(
            f,
            property_access,
            initializer,
        )))
    }

    /// Visits a top-level nested variable statement as it may contain `var`
    /// declarations that are hoisted and may still be exported with
    /// `export {}`.
    // port: tsc/internal/transformers/moduletransforms/commonjsmodule.go:CommonJSModuleTransformer.visitTopLevelNestedVariableStatement
    fn visit_top_level_nested_variable_statement(
        &self,
        v: &mut NodeVisitor<'_>,
        node: NodeId,
    ) -> Visited {
        let mut statements = vec![self
            .visit_each_child_with(v, Visitor::Main, node)
            .expect(NIL)];
        self.append_exports_of_variable_statement(v, &mut statements, node)?;
        Ok(single_or_many_of(v.factory_mut(), &statements))
    }

    /// Visits a top-level nested `for` statement as it may contain `var`
    /// declarations that are hoisted and may still be exported with
    /// `export {}`.
    // port: tsc/internal/transformers/moduletransforms/commonjsmodule.go:CommonJSModuleTransformer.visitTopLevelNestedForStatement
    fn visit_top_level_nested_for_statement(
        &self,
        v: &mut NodeVisitor<'_>,
        node: NodeId,
    ) -> Visited {
        let (initializer, condition, incrementor, statement) =
            for_statement_parts(v.factory(), node);
        if let Some(initializer) = initializer.filter(|&initializer| {
            let read = v.factory().node(initializer);
            tsr_ast::is_variable_declaration_list(&read)
                && read.flags() & node_flags::BLOCK_SCOPED == 0
        }) {
            let mut export_statements = Vec::new();
            self.append_exports_of_variable_declaration_list(
                v,
                &mut export_statements,
                initializer,
                false, /*isForInOrOfInitializer*/
            )?;
            if !export_statements.is_empty() {
                // given:
                //   export { x }
                //   for (var x = 0; ;) { }
                // emits:
                //   var x = 0;
                //   exports.x = x;
                //   for (; ;) { }

                let mut statements = Vec::new();
                let var_decl_list =
                    self.visit_node_with(v, Visitor::DiscardedValue, Some(initializer));
                let var_statement = v
                    .factory_mut()
                    .new_variable_statement(None /*modifiers*/, var_decl_list);
                statements.push(var_statement);
                statements.extend(export_statements);

                let condition = self.visit_node_with(v, Visitor::Main, condition);
                let incrementor = self.visit_node_with(v, Visitor::DiscardedValue, incrementor);
                let body = self.visit_iteration_body_with(v, Visitor::TopLevelNested, statement);
                statements.push(v.factory_mut().update_for_statement(
                    node,
                    None, /*initializer*/
                    condition,
                    incrementor,
                    body,
                ));
                return Ok(single_or_many_of(v.factory_mut(), &statements));
            }
        }
        Ok(Some(self.update_for_statement(
            v,
            node,
            initializer,
            condition,
            incrementor,
            statement,
        )))
    }

    /// `UpdateForStatement` over the visited parts, as `visitForStatement`
    /// writes it.
    fn update_for_statement(
        &self,
        v: &mut NodeVisitor<'_>,
        node: NodeId,
        initializer: Option<NodeId>,
        condition: Option<NodeId>,
        incrementor: Option<NodeId>,
        statement: Option<NodeId>,
    ) -> NodeId {
        let initializer = self.visit_node_with(v, Visitor::DiscardedValue, initializer);
        let condition = self.visit_node_with(v, Visitor::Main, condition);
        let incrementor = self.visit_node_with(v, Visitor::DiscardedValue, incrementor);
        let body = self.visit_iteration_body_with(v, Visitor::TopLevelNested, statement);
        v.factory_mut()
            .update_for_statement(node, initializer, condition, incrementor, body)
    }

    /// Visits a top-level nested `for..in` or `for..of` statement as it may
    /// contain `var` declarations that are hoisted and may still be exported
    /// with `export {}`.
    // port: tsc/internal/transformers/moduletransforms/commonjsmodule.go:CommonJSModuleTransformer.visitTopLevelNestedForInOrOfStatement
    fn visit_top_level_nested_for_in_or_of_statement(
        &self,
        v: &mut NodeVisitor<'_>,
        node: NodeId,
    ) -> Visited {
        let (await_modifier, initializer, expression, statement) =
            for_in_or_of_statement_parts(v.factory(), node);
        let initializer_node = initializer.expect(NIL);
        let is_var_declaration_list = {
            let read = v.factory().node(initializer_node);
            tsr_ast::is_variable_declaration_list(&read)
                && read.flags() & node_flags::BLOCK_SCOPED == 0
        };
        if is_var_declaration_list {
            let mut export_statements = Vec::new();
            self.append_exports_of_variable_declaration_list(
                v,
                &mut export_statements,
                initializer_node,
                true, /*isForInOrOfInitializer*/
            )?;
            if !export_statements.is_empty() {
                // given:
                //   export { x }
                //   for (var x in y) {
                //     ...
                //   }
                // emits:
                //   for (var x in y) {
                //     exports.x = x;
                //     ...
                //   }

                let initializer = self.visit_node_with(v, Visitor::DiscardedValue, initializer);
                let expression = self.visit_node_with(v, Visitor::Main, expression);
                let mut body = self
                    .visit_iteration_body_with(v, Visitor::TopLevelNested, statement)
                    .expect(NIL);
                let f = v.factory_mut();
                if tsr_ast::is_block(&f.node(body)) {
                    let (block_statements, multi_line) = {
                        let read = f.node(body);
                        let block = read.as_block().expect("Block payload");
                        (block.statements().expect(NIL), block.multi_line())
                    };
                    let mut body_statements = export_statements;
                    body_statements.extend(list_nodes(f, Some(block_statements)));
                    let body_statement_list = new_node_list(f, body_statements);
                    let loc = f.read_list(block_statements).loc();
                    f.set_list_location(body_statement_list, loc);
                    body = f.update_block(body, Some(body_statement_list), multi_line);
                } else {
                    let mut body_statements = export_statements;
                    body_statements.push(body);
                    let body_statement_list = new_node_list(f, body_statements);
                    body = f.new_block(Some(body_statement_list), true /*multiLine*/);
                }
                return Ok(Some(f.update_for_in_or_of_statement(
                    node,
                    await_modifier,
                    initializer,
                    expression,
                    Some(body),
                )));
            }
        }
        Ok(Some(self.update_for_in_or_of_statement(
            v,
            node,
            await_modifier,
            initializer,
            expression,
            statement,
        )))
    }

    /// `UpdateForInOrOfStatement` over the visited parts, as
    /// `visitForInOrOfStatement` writes it.
    fn update_for_in_or_of_statement(
        &self,
        v: &mut NodeVisitor<'_>,
        node: NodeId,
        await_modifier: Option<NodeId>,
        initializer: Option<NodeId>,
        expression: Option<NodeId>,
        statement: Option<NodeId>,
    ) -> NodeId {
        let initializer = self.visit_node_with(v, Visitor::DiscardedValue, initializer);
        let expression = self.visit_node_with(v, Visitor::Main, expression);
        let body = self.visit_iteration_body_with(v, Visitor::TopLevelNested, statement);
        v.factory_mut().update_for_in_or_of_statement(
            node,
            await_modifier,
            initializer,
            expression,
            body,
        )
    }

    /// Visits a top-level nested `do` statement as it may contain `var`
    /// declarations that are hoisted and may still be exported with
    /// `export {}`.
    // port: tsc/internal/transformers/moduletransforms/commonjsmodule.go:CommonJSModuleTransformer.visitTopLevelNestedDoStatement
    fn visit_top_level_nested_do_statement(&self, v: &mut NodeVisitor<'_>, node: NodeId) -> NodeId {
        let (statement, expression) = {
            let read = v.factory().node(node);
            let data = read.as_do_statement().expect("DoStatement payload");
            (data.statement(), data.expression())
        };
        let statement = self.visit_iteration_body_with(v, Visitor::TopLevelNested, statement);
        let expression = self.visit_node_with(v, Visitor::Main, expression);
        v.factory_mut()
            .update_do_statement(node, statement, expression)
    }

    /// Visits a top-level nested `while` statement as it may contain `var`
    /// declarations that are hoisted and may still be exported with
    /// `export {}`.
    // port: tsc/internal/transformers/moduletransforms/commonjsmodule.go:CommonJSModuleTransformer.visitTopLevelNestedWhileStatement
    fn visit_top_level_nested_while_statement(
        &self,
        v: &mut NodeVisitor<'_>,
        node: NodeId,
    ) -> NodeId {
        let (expression, statement) = {
            let read = v.factory().node(node);
            let data = read.as_while_statement().expect("WhileStatement payload");
            (data.expression(), data.statement())
        };
        let expression = self.visit_node_with(v, Visitor::Main, expression);
        let statement = self.visit_iteration_body_with(v, Visitor::TopLevelNested, statement);
        v.factory_mut()
            .update_while_statement(node, expression, statement)
    }

    /// Visits a top-level nested labeled statement as it may contain `var`
    /// declarations that are hoisted and may still be exported with
    /// `export {}`.
    // port: tsc/internal/transformers/moduletransforms/commonjsmodule.go:CommonJSModuleTransformer.visitTopLevelNestedLabeledStatement
    fn visit_top_level_nested_labeled_statement(
        &self,
        v: &mut NodeVisitor<'_>,
        node: NodeId,
    ) -> NodeId {
        let (label, statement) = {
            let read = v.factory().node(node);
            let data = read
                .as_labeled_statement()
                .expect("LabeledStatement payload");
            (data.label(), data.statement())
        };
        let statement = self
            .visit_embedded_statement_with(v, Visitor::TopLevelNested, statement)
            .unwrap_or_else(|| v.factory_mut().new_empty_statement());
        v.factory_mut()
            .update_labeled_statement(node, label, Some(statement))
    }

    /// Visits a top-level nested `with` statement as it may contain `var`
    /// declarations that are hoisted and may still be exported with
    /// `export {}`.
    // port: tsc/internal/transformers/moduletransforms/commonjsmodule.go:CommonJSModuleTransformer.visitTopLevelNestedWithStatement
    fn visit_top_level_nested_with_statement(
        &self,
        v: &mut NodeVisitor<'_>,
        node: NodeId,
    ) -> NodeId {
        let (expression, statement) = {
            let read = v.factory().node(node);
            let data = read.as_with_statement().expect("WithStatement payload");
            (data.expression(), data.statement())
        };
        let expression = self.visit_node_with(v, Visitor::Main, expression);
        let statement = self.visit_embedded_statement_with(v, Visitor::TopLevelNested, statement);
        v.factory_mut()
            .update_with_statement(node, expression, statement)
    }

    /// Visits a top-level nested `if` statement as it may contain `var`
    /// declarations that are hoisted and may still be exported with
    /// `export {}`.
    // port: tsc/internal/transformers/moduletransforms/commonjsmodule.go:CommonJSModuleTransformer.visitTopLevelNestedIfStatement
    fn visit_top_level_nested_if_statement(&self, v: &mut NodeVisitor<'_>, node: NodeId) -> NodeId {
        let (expression, then_statement, else_statement) = {
            let read = v.factory().node(node);
            let data = read.as_if_statement().expect("IfStatement payload");
            (
                data.expression(),
                data.then_statement(),
                data.else_statement(),
            )
        };
        let expression = self.visit_node_with(v, Visitor::Main, expression);
        let then_statement = self
            .visit_embedded_statement_with(v, Visitor::TopLevelNested, then_statement)
            .unwrap_or_else(|| {
                let f = v.factory_mut();
                let statements = new_node_list(f, Vec::new());
                f.new_block(Some(statements), false /*multiLine*/)
            });
        let else_statement =
            self.visit_embedded_statement_with(v, Visitor::TopLevelNested, else_statement);
        v.factory_mut()
            .update_if_statement(node, expression, Some(then_statement), else_statement)
    }

    /// Visits a top-level nested `switch` statement as it may contain `var`
    /// declarations that are hoisted and may still be exported with
    /// `export {}`.
    // port: tsc/internal/transformers/moduletransforms/commonjsmodule.go:CommonJSModuleTransformer.visitTopLevelNestedSwitchStatement
    fn visit_top_level_nested_switch_statement(
        &self,
        v: &mut NodeVisitor<'_>,
        node: NodeId,
    ) -> NodeId {
        let (expression, case_block) = {
            let read = v.factory().node(node);
            let data = read.as_switch_statement().expect("SwitchStatement payload");
            (data.expression(), data.case_block())
        };
        let expression = self.visit_node_with(v, Visitor::Main, expression);
        let case_block = self.visit_node_with(v, Visitor::TopLevelNested, case_block);
        v.factory_mut()
            .update_switch_statement(node, expression, case_block)
    }

    /// Visits a top-level nested case block as it may contain `var`
    /// declarations that are hoisted and may still be exported with
    /// `export {}`.
    // port: tsc/internal/transformers/moduletransforms/commonjsmodule.go:CommonJSModuleTransformer.visitTopLevelNestedCaseBlock
    fn visit_top_level_nested_case_block(
        &self,
        v: &mut NodeVisitor<'_>,
        node: NodeId,
    ) -> Option<NodeId> {
        self.visit_each_child_with(v, Visitor::TopLevelNested, node)
    }

    /// Visits a top-level nested `case` or `default` clause as it may contain
    /// `var` declarations that are hoisted and may still be exported with
    /// `export {}`.
    // port: tsc/internal/transformers/moduletransforms/commonjsmodule.go:CommonJSModuleTransformer.visitTopLevelNestedCaseOrDefaultClause
    fn visit_top_level_nested_case_or_default_clause(
        &self,
        v: &mut NodeVisitor<'_>,
        node: NodeId,
    ) -> NodeId {
        let (expression, statements) = {
            let read = v.factory().node(node);
            let data = read
                .as_case_or_default_clause()
                .expect("CaseOrDefaultClause payload");
            (data.expression(), data.statements())
        };
        let expression = self.visit_node_with(v, Visitor::Main, expression);
        let statements = self.visit_nodes_with(v, Visitor::TopLevelNested, statements);
        v.factory_mut()
            .update_case_or_default_clause(node, expression, statements)
    }

    /// Visits a top-level nested `try` statement as it may contain `var`
    /// declarations that are hoisted and may still be exported with
    /// `export {}`.
    // port: tsc/internal/transformers/moduletransforms/commonjsmodule.go:CommonJSModuleTransformer.visitTopLevelNestedTryStatement
    fn visit_top_level_nested_try_statement(
        &self,
        v: &mut NodeVisitor<'_>,
        node: NodeId,
    ) -> Option<NodeId> {
        self.visit_each_child_with(v, Visitor::TopLevelNested, node)
    }

    /// Visits a top-level nested `catch` clause as it may contain `var`
    /// declarations that are hoisted and may still be exported with
    /// `export {}`.
    // port: tsc/internal/transformers/moduletransforms/commonjsmodule.go:CommonJSModuleTransformer.visitTopLevelNestedCatchClause
    fn visit_top_level_nested_catch_clause(&self, v: &mut NodeVisitor<'_>, node: NodeId) -> NodeId {
        let (variable_declaration, block) = {
            let read = v.factory().node(node);
            let data = read.as_catch_clause().expect("CatchClause payload");
            (data.variable_declaration(), data.block())
        };
        let block = self.visit_node_with(v, Visitor::TopLevelNested, block);
        v.factory_mut()
            .update_catch_clause(node, variable_declaration, block)
    }

    /// Visits a top-level nested block as it may contain `var` declarations
    /// that are hoisted and may still be exported with `export {}`.
    // port: tsc/internal/transformers/moduletransforms/commonjsmodule.go:CommonJSModuleTransformer.visitTopLevelNestedBlock
    fn visit_top_level_nested_block(
        &self,
        v: &mut NodeVisitor<'_>,
        node: NodeId,
    ) -> Option<NodeId> {
        self.visit_each_child_with(v, Visitor::TopLevelNested, node)
    }

    // port: tsc/internal/transformers/moduletransforms/commonjsmodule.go:CommonJSModuleTransformer.visitForStatement
    fn visit_for_statement(&self, v: &mut NodeVisitor<'_>, node: NodeId) -> NodeId {
        let (initializer, condition, incrementor, statement) =
            for_statement_parts(v.factory(), node);
        self.update_for_statement(v, node, initializer, condition, incrementor, statement)
    }

    // port: tsc/internal/transformers/moduletransforms/commonjsmodule.go:CommonJSModuleTransformer.visitForInOrOfStatement
    fn visit_for_in_or_of_statement(&self, v: &mut NodeVisitor<'_>, node: NodeId) -> NodeId {
        let (await_modifier, initializer, expression, statement) =
            for_in_or_of_statement_parts(v.factory(), node);
        self.update_for_in_or_of_statement(
            v,
            node,
            await_modifier,
            initializer,
            expression,
            statement,
        )
    }

    /// Visits an expression statement whose value will be discarded at
    /// runtime.
    // port: tsc/internal/transformers/moduletransforms/commonjsmodule.go:CommonJSModuleTransformer.visitExpressionStatement
    fn visit_expression_statement(&self, v: &mut NodeVisitor<'_>, node: NodeId) -> Option<NodeId> {
        self.visit_each_child_with(v, Visitor::DiscardedValue, node)
    }

    /// Visits a `void` expression whose value will be discarded at runtime.
    // port: tsc/internal/transformers/moduletransforms/commonjsmodule.go:CommonJSModuleTransformer.visitVoidExpression
    fn visit_void_expression(&self, v: &mut NodeVisitor<'_>, node: NodeId) -> Option<NodeId> {
        self.visit_each_child_with(v, Visitor::DiscardedValue, node)
    }

    /// Visits a parenthesized expression whose value may be discarded at
    /// runtime.
    // port: tsc/internal/transformers/moduletransforms/commonjsmodule.go:CommonJSModuleTransformer.visitParenthesizedExpression
    fn visit_parenthesized_expression(
        &self,
        v: &mut NodeVisitor<'_>,
        node: NodeId,
        result_is_discarded: bool,
    ) -> NodeId {
        let expression = v.factory().node(node).expression();
        let expression =
            self.visit_node_with(v, discarded_or_main(result_is_discarded), expression);
        v.factory_mut()
            .update_parenthesized_expression(node, expression)
    }

    /// Visits a partially emitted expression whose value may be discarded at
    /// runtime.
    // port: tsc/internal/transformers/moduletransforms/commonjsmodule.go:CommonJSModuleTransformer.visitPartiallyEmittedExpression
    fn visit_partially_emitted_expression(
        &self,
        v: &mut NodeVisitor<'_>,
        node: NodeId,
        result_is_discarded: bool,
    ) -> NodeId {
        let expression = v.factory().node(node).expression();
        let expression =
            self.visit_node_with(v, discarded_or_main(result_is_discarded), expression);
        v.factory_mut()
            .update_partially_emitted_expression(node, expression)
    }

    /// Visits a binary expression whose value may be discarded, or which
    /// might contain an assignment to an exported identifier.
    // port: tsc/internal/transformers/moduletransforms/commonjsmodule.go:CommonJSModuleTransformer.visitBinaryExpression
    fn visit_binary_expression(
        &self,
        v: &mut NodeVisitor<'_>,
        node: NodeId,
        result_is_discarded: bool,
    ) -> Visited {
        if tsr_ast::is_destructuring_assignment(view(v.factory())?, node)? {
            return self.visit_destructuring_assignment(v, node, result_is_discarded);
        }

        if tsr_ast::is_assignment_expression(
            view(v.factory())?,
            node,
            false, /*excludeCompoundAssignment*/
        )? {
            return self.visit_assignment_expression(v, node);
        }

        if tsr_ast::utilities::is_comma_expression(view(v.factory())?, node)? {
            return Ok(Some(self.visit_comma_expression(
                v,
                node,
                result_is_discarded,
            )));
        }

        Ok(self.visit_each_child_with(v, Visitor::Main, node))
    }

    // port: tsc/internal/transformers/moduletransforms/commonjsmodule.go:CommonJSModuleTransformer.visitAssignmentExpression
    fn visit_assignment_expression(&self, v: &mut NodeVisitor<'_>, node: NodeId) -> Visited {
        // When we see an assignment expression whose left-hand side is an exported symbol,
        // we should ensure all exports of that symbol are updated with the correct value.
        //
        // - We do not transform generated identifiers unless they are file-level reserved names.
        // - We do not transform identifiers tagged with the LocalName flag.
        // - We only transform identifiers that are exported at the top level.
        let left = binary_parts(v.factory(), node).0.expect(NIL);
        if self.is_substitutable_identifier(v.factory(), left) {
            let exported_names = self.get_exports(v.factory(), left)?;
            if !exported_names.is_empty() {
                // For each additional export of the declaration, apply an export assignment.
                let mut expression = self
                    .visit_each_child_with(v, Visitor::Main, node)
                    .expect(NIL);
                let location = v.factory().node(node).range();
                for export_name in exported_names {
                    expression = self.create_export_expression(
                        v.factory_mut(),
                        export_name,
                        expression,
                        Some(location), /*location*/
                        false,          /*liveBinding*/
                    );
                }
                return Ok(Some(expression));
            }
        }

        Ok(self.visit_each_child_with(v, Visitor::Main, node))
    }

    /// `ast.IsIdentifier(node) && (!transformers.IsGeneratedIdentifier(node) ||
    /// isFileLevelReservedGeneratedIdentifier(node)) &&
    /// !transformers.IsLocalName(node)`, the test upstream writes out for an
    /// assignment target in two places.
    fn is_substitutable_identifier(&self, f: &dyn RuntimeFactory, node: NodeId) -> bool {
        tsr_ast::is_identifier(&f.node(node))
            && (!is_generated_identifier(&self.context, node)
                || is_file_level_reserved_generated_identifier(&self.context, node))
            && !is_local_name(&self.context, node)
    }

    /// Visits a destructuring assignment which might target an exported
    /// identifier.
    // port: tsc/internal/transformers/moduletransforms/commonjsmodule.go:CommonJSModuleTransformer.visitDestructuringAssignment
    fn visit_destructuring_assignment(
        &self,
        v: &mut NodeVisitor<'_>,
        node: NodeId,
        value_is_discarded: bool,
    ) -> Visited {
        let left = binary_parts(v.factory(), node).0.expect(NIL);
        if self.destructuring_needs_flattening(v.factory(), left)? {
            let ec = self.ec();
            // A failure inside the callback is recorded; the flattener still
            // needs an expression, so it receives the plain assignment.
            let callback = |m: &mut NodeVisitor<'_>,
                            name: NodeId,
                            value: NodeId,
                            location: Option<TextRange>|
             -> NodeId {
                match self.create_all_export_expressions(m.factory_mut(), name, value, location) {
                    Ok(expression) => expression,
                    Err(error) => {
                        self.failure.record(error);
                        self.ec()
                            .new_assignment_expression(m.factory_mut(), name, value)
                    }
                }
            };
            return self.with_visitor(v, Visitor::Main, |m| {
                flatten_destructuring_assignment(
                    m,
                    &ec,
                    node,
                    !value_is_discarded, /*needsValue*/
                    FlattenLevel::All,
                    Some(&callback),
                )
            });
        }
        Ok(self.visit_each_child_with(v, Visitor::Main, node))
    }

    /// destructuringNeedsFlattening checks whether a destructuring assignment
    /// target contains any exported identifiers that need to be flattened
    /// into individual export assignments.
    // port: tsc/internal/transformers/moduletransforms/commonjsmodule.go:CommonJSModuleTransformer.destructuringNeedsFlattening
    fn destructuring_needs_flattening(
        &self,
        f: &dyn RuntimeFactory,
        node: NodeId,
    ) -> Result<bool, Error> {
        let read = f.node(node);
        if tsr_ast::is_object_literal_expression(&read) {
            let properties = read.property_list();
            drop(read);
            for elem in list_nodes(f, properties) {
                let read = f.node(elem);
                match read.kind().known() {
                    Some(K::PropertyAssignment) => {
                        let initializer = read.initializer().expect(NIL);
                        drop(read);
                        if self.destructuring_needs_flattening(f, initializer)? {
                            return Ok(true);
                        }
                    }
                    Some(K::ShorthandPropertyAssignment) => {
                        let name = read.name().expect(NIL);
                        drop(read);
                        if self.destructuring_needs_flattening(f, name)? {
                            return Ok(true);
                        }
                    }
                    Some(K::SpreadAssignment) => {
                        let expression = read.expression().expect(NIL);
                        drop(read);
                        if self.destructuring_needs_flattening(f, expression)? {
                            return Ok(true);
                        }
                    }
                    Some(K::MethodDeclaration | K::GetAccessor | K::SetAccessor) => {
                        return Ok(false);
                    }
                    _ => {}
                }
            }
        } else if tsr_ast::is_array_literal_expression(&read) {
            let elements = read.element_list();
            drop(read);
            for elem in list_nodes(f, elements) {
                let read = f.node(elem);
                if tsr_ast::is_spread_element(&read) {
                    let expression = read.expression().expect(NIL);
                    drop(read);
                    if self.destructuring_needs_flattening(f, expression)? {
                        return Ok(true);
                    }
                } else {
                    drop(read);
                    if self.destructuring_needs_flattening(f, elem)? {
                        return Ok(true);
                    }
                }
            }
        } else if tsr_ast::is_identifier(&read) {
            drop(read);
            let exported_names = self.get_exports(f, node)?;
            if is_export_name(&self.context, node) {
                // The identifier is already wrapped to be an export reference; tolerate up to one
                // matching export.
                return Ok(exported_names.len() > 1);
            }
            if exported_names.is_empty() {
                return Ok(false);
            }
            // A single direct export whose export name matches the identifier text can be handled
            // natively: substitution will rewrite the identifier to `exports.X`, so no flattening
            // is needed. Re-aliased exports (where the export name differs from the local name) or
            // multi-exported names cannot be expressed natively in a destructuring assignment.
            if exported_names.len() == 1
                && self.is_direct_export(f, node)?
                && node_text(f, exported_names[0])? == node_text(f, node)?
            {
                return Ok(false);
            }
            return Ok(true);
        }
        Ok(false)
    }

    /// `exports.<name> = <value>` for a directly exported binding, as both
    /// branches of `createAllExportExpressions` write it.
    fn create_direct_export_assignment(
        &self,
        f: &mut dyn RuntimeFactory,
        name: NodeId,
        value: NodeId,
    ) -> NodeId {
        let mut ec = self.ec();
        let export_name = tsr_ast::clone_node(f, name);
        ec.add_emit_flags(
            export_name,
            emit_flags::NO_COMMENTS | emit_flags::NO_SOURCE_MAP,
        );
        let exports = new_identifier(f, b"exports");
        let property_access = f.new_property_access_expression(
            Some(exports),
            None, /*questionDotToken*/
            Some(export_name),
            node_flags::NONE,
        );
        ec.add_emit_flags(property_access, emit_flags::NO_COMMENTS);
        let expression = ec.new_assignment_expression(f, property_access, value);
        ec.assign_comment_and_source_map_ranges(f, expression, name);
        expression
    }

    /// createAllExportExpressions is the callback used during destructuring
    /// flattening to create export expressions for each exported identifier
    /// binding.
    // port: tsc/internal/transformers/moduletransforms/commonjsmodule.go:CommonJSModuleTransformer.createAllExportExpressions
    fn create_all_export_expressions(
        &self,
        f: &mut dyn RuntimeFactory,
        name: NodeId,
        value: NodeId,
        location: Option<TextRange>,
    ) -> Result<NodeId, Error> {
        let exported_names = self.get_exports(f, name)?;
        if !exported_names.is_empty() {
            // If the name is directly exported (i.e., `export let x`), assign to exports.name directly.
            // Otherwise, assign to the local binding first (i.e., `let x; export { x }`).
            let mut expression = if self.is_direct_export(f, name)? {
                // Create exports.name = value to handle the direct export assignment,
                // since the Go port doesn't have an onSubstituteNode mechanism to rewrite identifiers.
                self.create_direct_export_assignment(f, name, value)
            } else {
                self.ec().new_assignment_expression(f, name, value)
            };
            for export_name in exported_names {
                expression = self.create_export_expression(
                    f,
                    export_name,
                    expression,
                    location,
                    false, /*liveBinding*/
                );
            }
            return Ok(expression);
        }
        // If the identifier is directly exported but has no additional export aliases,
        // still write to exports.name.
        if self.is_direct_export(f, name)? {
            return Ok(self.create_direct_export_assignment(f, name, value));
        }
        Ok(self.ec().new_assignment_expression(f, name, value))
    }

    /// isDirectExport checks whether the identifier is directly exported from
    /// the source file (e.g., `export let x` or `export function f()`), as
    /// opposed to being re-exported via `export { x }` for a locally-declared
    /// variable.
    // port: tsc/internal/transformers/moduletransforms/commonjsmodule.go:CommonJSModuleTransformer.isDirectExport
    fn is_direct_export(&self, f: &dyn RuntimeFactory, name: NodeId) -> Result<bool, Error> {
        let original = self.context.most_original(name);
        let export_container = self
            .resolver
            .borrow_mut()
            .get_referenced_export_container(original, false /*prefixLocals*/)?;
        Ok(export_container.is_some_and(|container| f.node(container).kind() == K::SourceFile))
    }

    // port: tsc/internal/transformers/moduletransforms/commonjsmodule.go:CommonJSModuleTransformer.visitAssignmentProperty
    fn visit_assignment_property(&self, v: &mut NodeVisitor<'_>, node: NodeId) -> NodeId {
        let (name, initializer) = {
            let read = v.factory().node(node);
            (read.name(), read.initializer())
        };
        let name = self.visit_node_with(v, Visitor::Main, name);
        let initializer = self.visit_node_with(v, Visitor::AssignmentPattern, initializer);
        v.factory_mut().update_property_assignment(
            node,
            None, /*modifiers*/
            name,
            None, /*postfixToken*/
            None, /*typeNode*/
            initializer,
        )
    }

    // port: tsc/internal/transformers/moduletransforms/commonjsmodule.go:CommonJSModuleTransformer.visitShorthandAssignmentProperty
    fn visit_shorthand_assignment_property(
        &self,
        v: &mut NodeVisitor<'_>,
        node: NodeId,
    ) -> Result<NodeId, Error> {
        let (name, equals_token, object_assignment_initializer) =
            shorthand_parts(v.factory(), node);
        let mut target = self
            .visit_destructuring_assignment_target_no_stack(v, name.expect(NIL))?
            .expect(NIL);
        if tsr_ast::is_identifier(&v.factory().node(target)) {
            let initializer = self.visit_node_with(v, Visitor::Main, object_assignment_initializer);
            return Ok(v.factory_mut().update_shorthand_property_assignment(
                node,
                None, /*modifiers*/
                Some(target),
                None, /*postfixToken*/
                None, /*typeNode*/
                equals_token,
                initializer,
            ));
        }
        if object_assignment_initializer.is_some() {
            let equals_token =
                equals_token.unwrap_or_else(|| v.factory_mut().new_token(K::EqualsToken.into()));
            let right = self.visit_node_with(v, Visitor::Main, object_assignment_initializer);
            target = v.factory_mut().new_binary_expression(
                None, /*modifiers*/
                Some(target),
                None, /*typeNode*/
                Some(equals_token),
                right,
            );
        }
        let mut ec = self.ec();
        let f = v.factory_mut();
        let updated = f.new_property_assignment(
            None, /*modifiers*/
            name,
            None, /*postfixToken*/
            None, /*typeNode*/
            Some(target),
        );
        ec.set_original(updated, node);
        ec.assign_comment_and_source_map_ranges(f, updated, node);
        Ok(updated)
    }

    // port: tsc/internal/transformers/moduletransforms/commonjsmodule.go:CommonJSModuleTransformer.visitAssignmentRestProperty
    fn visit_assignment_rest_property(
        &self,
        v: &mut NodeVisitor<'_>,
        node: NodeId,
    ) -> Result<NodeId, Error> {
        let expression = v.factory().node(node).expression().expect(NIL);
        let expression = self.visit_destructuring_assignment_target(v, expression)?;
        Ok(v.factory_mut().update_spread_assignment(node, expression))
    }

    // port: tsc/internal/transformers/moduletransforms/commonjsmodule.go:CommonJSModuleTransformer.visitAssignmentRestElement
    fn visit_assignment_rest_element(
        &self,
        v: &mut NodeVisitor<'_>,
        node: NodeId,
    ) -> Result<NodeId, Error> {
        let expression = v.factory().node(node).expression().expect(NIL);
        let expression = self.visit_destructuring_assignment_target(v, expression)?;
        Ok(v.factory_mut().update_spread_element(node, expression))
    }

    // port: tsc/internal/transformers/moduletransforms/commonjsmodule.go:CommonJSModuleTransformer.visitAssignmentElement
    fn visit_assignment_element(&self, v: &mut NodeVisitor<'_>, node: NodeId) -> Visited {
        if tsr_ast::is_binary_expression(&v.factory().node(node)) {
            let (left, operator_token, right) = binary_parts(v.factory(), node);
            let operator_token = operator_token.expect(NIL);
            if v.factory().node(operator_token).kind() == K::EqualsToken {
                let left = self.visit_destructuring_assignment_target(v, left.expect(NIL))?;
                let right = self.visit_node_with(v, Visitor::Main, right);
                return Ok(Some(v.factory_mut().update_binary_expression(
                    node,
                    None, /*modifiers*/
                    left,
                    None, /*typeNode*/
                    Some(operator_token),
                    right,
                )));
            }
        }

        self.visit_destructuring_assignment_target_no_stack(v, node)
    }

    // port: tsc/internal/transformers/moduletransforms/commonjsmodule.go:CommonJSModuleTransformer.visitDestructuringAssignmentTarget
    fn visit_destructuring_assignment_target(
        &self,
        v: &mut NodeVisitor<'_>,
        node: NodeId,
    ) -> Visited {
        let grandparent_node = self.push_node(node);
        let result = match v.factory().node(node).kind().known() {
            Some(K::ObjectLiteralExpression | K::ArrayLiteralExpression) => {
                self.visit_assignment_pattern_no_stack(v, node)
            }
            _ => self.visit_destructuring_assignment_target_no_stack(v, node),
        };
        self.pop_node(grandparent_node);
        result
    }

    // port: tsc/internal/transformers/moduletransforms/commonjsmodule.go:CommonJSModuleTransformer.visitDestructuringAssignmentTargetNoStack
    fn visit_destructuring_assignment_target_no_stack(
        &self,
        v: &mut NodeVisitor<'_>,
        node: NodeId,
    ) -> Visited {
        if self.is_substitutable_identifier(v.factory(), node) {
            let f = v.factory_mut();
            let mut expression = self.visit_expression_identifier(f, node)?;
            let exported_names = self.get_exports(f, node)?;
            if !exported_names.is_empty() {
                // transforms:
                //  var x;
                //  export { x }
                //  { x: x } = y
                // to:
                //  { x: { set value(v) { exports.x = x = v; } }.value } = y

                let mut ec = self.ec();
                let value = ec.new_unique_name_ex(
                    f,
                    JsString::from_bytes(&b"value"[..]),
                    AutoGenerateOptions {
                        flags: g::OPTIMISTIC,
                        ..AutoGenerateOptions::default()
                    },
                );
                expression = ec.new_assignment_expression(f, expression, value);

                for export_name in exported_names {
                    expression = self.create_export_expression(
                        f,
                        export_name,
                        expression,
                        None,  /*location*/
                        false, /*liveBinding*/
                    );
                }

                let statement = f.new_expression_statement(Some(expression));
                let statement_list = new_node_list(f, vec![statement]);
                let param = f.new_parameter_declaration(
                    None, /*modifiers*/
                    None, /*dotDotDotToken*/
                    Some(value),
                    None, /*questionToken*/
                    None, /*type*/
                    None, /*initializer*/
                );
                let setter_name = new_identifier(f, b"value");
                let parameters = new_node_list(f, vec![param]);
                let body = f.new_block(Some(statement_list), false /*multiLine*/);
                let value_setter = f.new_set_accessor_declaration(
                    None, /*modifiers*/
                    Some(setter_name),
                    None, /*typeParameters*/
                    Some(parameters),
                    None, /*returnType*/
                    None, /*fullSignature*/
                    Some(body),
                );
                let property_list = new_node_list(f, vec![value_setter]);
                expression =
                    f.new_object_literal_expression(Some(property_list), false /*multiLine*/);
                let value_name = new_identifier(f, b"value");
                expression = f.new_property_access_expression(
                    Some(expression),
                    None, /*questionDotToken*/
                    Some(value_name),
                    node_flags::NONE,
                );
            }
            return Ok(Some(expression));
        }

        self.visit_no_stack(v, node, false /*resultIsDiscarded*/)
    }

    /// Visits a comma expression whose left-hand value is always discard, and
    /// whose right-hand value may be discarded at runtime.
    // port: tsc/internal/transformers/moduletransforms/commonjsmodule.go:CommonJSModuleTransformer.visitCommaExpression
    fn visit_comma_expression(
        &self,
        v: &mut NodeVisitor<'_>,
        node: NodeId,
        result_is_discarded: bool,
    ) -> NodeId {
        let (left, operator_token, right) = binary_parts(v.factory(), node);
        let left = self.visit_node_with(v, Visitor::DiscardedValue, left);
        let right = self.visit_node_with(v, discarded_or_main(result_is_discarded), right);
        v.factory_mut().update_binary_expression(
            node,
            None, /*modifiers*/
            left,
            None, /*typeNode*/
            operator_token,
            right,
        )
    }

    /// Visits a prefix unary expression that might modify an exported
    /// identifier.
    // port: tsc/internal/transformers/moduletransforms/commonjsmodule.go:CommonJSModuleTransformer.visitPrefixUnaryExpression
    fn visit_prefix_unary_expression(
        &self,
        v: &mut NodeVisitor<'_>,
        node: NodeId,
        _result_is_discarded: bool,
    ) -> Visited {
        // When we see a prefix increment expression whose operand is an exported
        // symbol, we should ensure all exports of that symbol are updated with the correct
        // value.
        //
        // - We do not transform generated identifiers for any reason.
        // - We do not transform identifiers tagged with the LocalName flag.
        // - We do not transform identifiers that were originally the name of an enum or
        //   namespace due to how they are transformed in TypeScript.
        // - We only transform identifiers that are exported at the top level.
        let (operator, operand) = {
            let read = v.factory().node(node);
            let data = read
                .as_prefix_unary_expression()
                .expect("PrefixUnaryExpression payload");
            (data.operator(), data.operand().expect(NIL))
        };
        if (operator == K::PlusPlusToken || operator == K::MinusMinusToken)
            && tsr_ast::is_identifier(&v.factory().node(operand))
            && !is_local_name(&self.context, operand)
        {
            let exported_names = self.get_exports(v.factory(), operand)?;
            if !exported_names.is_empty() {
                // given:
                //   var x = 0;
                //   export { x }
                //   ++x;
                // emits:
                //   var x = 0;
                //   exports.x = x;
                //   exports.x = ++x;
                // note:
                //   after the operation, `exports.x` will hold the value of `x` after the increment.

                let visited = self.visit_node_with(v, Visitor::Main, Some(operand));
                let mut ec = self.ec();
                let f = v.factory_mut();
                let mut expression = f.update_prefix_unary_expression(node, operator, visited);
                for export_name in exported_names {
                    expression = self.create_export_expression(
                        f,
                        export_name,
                        expression,
                        None,  /*location*/
                        false, /*liveBinding*/
                    );
                    ec.assign_comment_and_source_map_ranges(f, expression, node);
                }
                return Ok(Some(expression));
            }
        }
        Ok(self.visit_each_child_with(v, Visitor::Main, node))
    }

    /// Visits a postfix unary expression that might modify an exported
    /// identifier.
    // port: tsc/internal/transformers/moduletransforms/commonjsmodule.go:CommonJSModuleTransformer.visitPostfixUnaryExpression
    fn visit_postfix_unary_expression(
        &self,
        v: &mut NodeVisitor<'_>,
        node: NodeId,
        result_is_discarded: bool,
    ) -> Visited {
        // When we see a postfix increment expression whose operand is an exported
        // symbol, we should ensure all exports of that symbol are updated with the correct
        // value.
        //
        // - We do not transform generated identifiers for any reason.
        // - We do not transform identifiers tagged with the LocalName flag.
        // - We do not transform identifiers that were originally the name of an enum or
        //   namespace due to how they are transformed in TypeScript.
        // - We only transform identifiers that are exported at the top level.
        let (operand, operator) = {
            let read = v.factory().node(node);
            let data = read
                .as_postfix_unary_expression()
                .expect("PostfixUnaryExpression payload");
            (data.operand().expect(NIL), data.operator())
        };
        if (operator == K::PlusPlusToken || operator == K::MinusMinusToken)
            && tsr_ast::is_identifier(&v.factory().node(operand))
            && !is_local_name(&self.context, operand)
        {
            let exported_names = self.get_exports(v.factory(), operand)?;
            if !exported_names.is_empty() {
                // given (value is discarded):
                //   var x = 0;
                //   export { x }
                //   x++;
                // emits:
                //   var x = 0, y;
                //   exports.x = x;
                //   exports.x = (x++, x);
                // note:
                //   after the operation, `exports.x` will hold the value of `x` after the increment.
                //
                // given (value is not discarded):
                //   var x = 0, y;
                //   export { x }
                //   y = x++;
                // emits:
                //   var _a;
                //   var x = 0, y;
                //   exports.x = x;
                //   y = (exports.x = (_a = x++, x), _a);
                // note:
                //   after the operation, `exports.x` will hold the value of `x` after the increment, while
                //   `y` will hold the value of `x` before the increment.

                let mut temp = None;
                let visited = self.visit_node_with(v, Visitor::Main, Some(operand));
                let mut ec = self.ec();
                let f = v.factory_mut();
                let mut expression = f.update_postfix_unary_expression(node, visited, operator);
                if !result_is_discarded {
                    let new_temp = ec.new_temp_variable(f);
                    ec.add_variable_declaration(f, new_temp);
                    temp = Some(new_temp);

                    expression = ec.new_assignment_expression(f, new_temp, expression);
                    ec.assign_comment_and_source_map_ranges(f, expression, node);
                }

                let operand_clone = tsr_ast::clone_node(f, operand);
                expression = ec.new_comma_expression(f, expression, operand_clone);
                ec.assign_comment_and_source_map_ranges(f, expression, node);

                for export_name in exported_names {
                    expression = self.create_export_expression(
                        f,
                        export_name,
                        expression,
                        None,  /*location*/
                        false, /*liveBinding*/
                    );
                    ec.assign_comment_and_source_map_ranges(f, expression, node);
                }

                if let Some(temp) = temp {
                    expression = ec.new_comma_expression(f, expression, temp);
                    ec.assign_comment_and_source_map_ranges(f, expression, node);
                }

                return Ok(Some(expression));
            }
        }

        Ok(self.visit_each_child_with(v, Visitor::Main, node))
    }

    /// Visits a call expression that might reference an imported symbol and
    /// thus require an indirect call, or that might be an `import()` or
    /// `require()` call that may need to be rewritten.
    // port: tsc/internal/transformers/moduletransforms/commonjsmodule.go:CommonJSModuleTransformer.visitCallExpression
    fn visit_call_expression(&self, v: &mut NodeVisitor<'_>, node: NodeId) -> Visited {
        let mut needs_rewrite = false;
        if self
            .compiler_options
            .rewrite_relative_import_extensions
            .is_true()
        {
            let f = v.factory();
            if is_import_call(f, node)? && !list_nodes(f, f.node(node).argument_list()).is_empty()
                || is_in_js_file(Some(&f.node(node)))
                    && is_require_call(
                        view(f)?,
                        &f.node(node),
                        false, /*requireStringLiteralLikeArgument*/
                    )?
            {
                needs_rewrite = true;
            }
        }
        if is_import_call(v.factory(), node)? && self.should_transform_import_call(v.factory())? {
            return self.visit_import_call_expression(v, node, needs_rewrite);
        }
        if needs_rewrite {
            return Ok(Some(self.shim_or_rewrite_import_or_require_call(v, node)));
        }
        let (expression, question_dot_token, arguments, flags) = {
            let read = v.factory().node(node);
            let data = read.as_call_expression().expect("CallExpression payload");
            (
                data.expression().expect(NIL),
                data.question_dot_token(),
                data.arguments(),
                read.flags(),
            )
        };
        if tsr_ast::is_identifier(&v.factory().node(expression)) {
            // given:
            //   import { f } from "mod";
            //   f();
            // emits:
            //   const mod_1 = require("mod");
            //   (0, mod_1.f)();
            // note:
            //   the indirect call is applied by the printer by way of the `EFIndirectCall` emit flag.
            let visited = self.visit_expression_identifier(v.factory_mut(), expression)?;
            let arguments = self.visit_nodes_with(v, Visitor::Main, arguments);
            let updated = v.factory_mut().update_call_expression(
                node,
                Some(visited),
                question_dot_token,
                None, /*typeArguments*/
                arguments,
                flags,
            );
            if !tsr_ast::is_identifier(&v.factory().node(visited))
                && !is_helper_name(&self.context, expression)
            {
                self.ec().add_emit_flags(updated, emit_flags::INDIRECT_CALL);
            }
            return Ok(Some(updated));
        }
        Ok(self.visit_each_child_with(v, Visitor::Main, node))
    }

    // port: tsc/internal/transformers/moduletransforms/commonjsmodule.go:CommonJSModuleTransformer.shouldTransformImportCall
    fn should_transform_import_call(&self, f: &dyn RuntimeFactory) -> Result<bool, Error> {
        let current_source_file = self.current_source_file.get().expect(NIL);
        let file_name = f
            .read_source_file(current_source_file)?
            .file_name()
            .to_vec();
        let format = (self.get_emit_module_format_of_file)(&file_name)?;
        Ok(tsr_ast::utilities_modules::should_transform_import_call(
            &self.compiler_options,
            format,
        ))
    }

    // port: tsc/internal/transformers/moduletransforms/commonjsmodule.go:CommonJSModuleTransformer.visitImportCallExpression
    fn visit_import_call_expression(
        &self,
        v: &mut NodeVisitor<'_>,
        node: NodeId,
        rewrite_or_shim: bool,
    ) -> Visited {
        if self.module_kind == ModuleKind::NONE && self.language_version >= ScriptTarget::ES2020 {
            return Ok(self.visit_each_child_with(v, Visitor::Main, node));
        }

        let external_module_name = get_external_module_name_literal(
            v.factory_mut(),
            node,
            self.current_source_file.get().expect(NIL),
            None, /*resolver*/
            &self.compiler_options,
        )?;
        let first = list_nodes(v.factory(), v.factory().node(node).argument_list())
            .first()
            .copied();
        let first_argument = self.visit_node_with(v, Visitor::Main, first);

        // Only use the external module name if it differs from the first argument. This allows us to preserve the quote style of the argument on output.
        let f = v.factory_mut();
        let differs = match (external_module_name, first_argument) {
            (Some(_), None) => true,
            (Some(external_module_name), Some(first_argument)) => {
                !tsr_ast::is_string_literal(&f.node(first_argument))
                    || node_text(f, first_argument)? != node_text(f, external_module_name)?
            }
            (None, _) => false,
        };
        let argument = if differs {
            external_module_name
        } else if let Some(first_argument) = first_argument.filter(|_| rewrite_or_shim) {
            if tsr_ast::is_string_literal(&f.node(first_argument)) {
                let mut ec = self.ec();
                rewrite_module_specifier(&mut ec, f, Some(first_argument), &self.compiler_options)
            } else {
                Some(self.ec().new_rewrite_relative_import_extensions_helper(
                    f,
                    first_argument,
                    self.compiler_options.jsx == JsxEmit::PRESERVE,
                ))
            }
        } else {
            first_argument
        };
        Ok(Some(
            self.create_import_call_expression_common_js(f, argument),
        ))
    }

    // port: tsc/internal/transformers/moduletransforms/commonjsmodule.go:CommonJSModuleTransformer.createImportCallExpressionCommonJS
    fn create_import_call_expression_common_js(
        &self,
        f: &mut dyn RuntimeFactory,
        arg: Option<NodeId>,
    ) -> NodeId {
        // import(x)
        // emit as
        // Promise.resolve(`${x}`).then((s) => require(s)) /*CommonJS Require*/
        // We have to wrap require in then callback so that require is done in asynchronously
        // if we simply do require in resolve callback in Promise constructor. We will execute the loading immediately
        // If the arg is not inlineable, we have to evaluate and ToString() it in the current scope
        // Otherwise, we inline it in require() so that it's statically analyzable

        let need_sync_eval = arg.is_some_and(|arg| !is_simple_inlineable_expression(f, arg));

        let mut promise_resolve_arguments = Vec::new();
        if need_sync_eval {
            let head = f.new_template_head(JsString::default(), JsString::default(), 0);
            let tail = f.new_template_tail(JsString::default(), JsString::default(), 0);
            let span = f.new_template_span(arg, Some(tail));
            let spans = new_node_list(f, vec![span]);
            promise_resolve_arguments.push(f.new_template_expression(Some(head), Some(spans)));
        }
        let promise = new_identifier(f, b"Promise");
        let resolve = new_identifier(f, b"resolve");
        let promise_resolve = f.new_property_access_expression(
            Some(promise),
            None, /*questionDotToken*/
            Some(resolve),
            node_flags::NONE,
        );
        let promise_resolve_arguments = new_node_list(f, promise_resolve_arguments);
        let promise_resolve_call = f.new_call_expression(
            Some(promise_resolve),
            None, /*questionDotToken*/
            None, /*typeArguments*/
            Some(promise_resolve_arguments),
            node_flags::NONE,
        );

        let mut require_arguments = Vec::new();
        if need_sync_eval {
            require_arguments.push(new_identifier(f, b"s"));
        } else if let Some(arg) = arg {
            require_arguments.push(arg);
        }

        let require = new_identifier(f, b"require");
        let require_arguments = new_node_list(f, require_arguments);
        let require = f.new_call_expression(
            Some(require),
            None, /*questionDotToken*/
            None, /*typeArguments*/
            Some(require_arguments),
            node_flags::NONE,
        );
        let require_call = self.ec().new_import_star_helper(f, require);

        let mut parameters = Vec::new();
        if need_sync_eval {
            let s = new_identifier(f, b"s");
            parameters.push(f.new_parameter_declaration(
                None, /*modifiers*/
                None, /*dotDotDotToken*/
                Some(s),
                None, /*questionToken*/
                None, /*type*/
                None, /*initializer*/
            ));
        }

        let parameters = new_node_list(f, parameters);
        let arrow = f.new_token(K::EqualsGreaterThanToken.into());
        let function = f.new_arrow_function(
            None, /*modifiers*/
            None, /*typeParameters*/
            Some(parameters),
            None,        /*type*/
            None,        /*fullSignature*/
            Some(arrow), /*equalsGreaterThanToken*/
            Some(require_call),
        );

        let then = new_identifier(f, b"then");
        let callee = f.new_property_access_expression(
            Some(promise_resolve_call),
            None, /*questionDotToken*/
            Some(then),
            node_flags::NONE,
        );
        let arguments = new_node_list(f, vec![function]);
        f.new_call_expression(
            Some(callee),
            None, /*questionDotToken*/
            None, /*typeArguments*/
            Some(arguments),
            node_flags::NONE,
        )
    }

    // port: tsc/internal/transformers/moduletransforms/commonjsmodule.go:CommonJSModuleTransformer.shimOrRewriteImportOrRequireCall
    fn shim_or_rewrite_import_or_require_call(
        &self,
        v: &mut NodeVisitor<'_>,
        node: NodeId,
    ) -> NodeId {
        let (expression, question_dot_token, arguments, flags) = {
            let read = v.factory().node(node);
            let data = read.as_call_expression().expect("CallExpression payload");
            (
                data.expression(),
                data.question_dot_token(),
                data.arguments(),
                read.flags(),
            )
        };
        let expression = self.visit_node_with(v, Visitor::Main, expression);
        let mut arguments_list = arguments;
        let argument_nodes = list_nodes(v.factory(), arguments);
        if !argument_nodes.is_empty() {
            let mut first_argument = self
                .visit_node_with(v, Visitor::Main, Some(argument_nodes[0]))
                .expect(NIL);
            let first_argument_changed;
            let mut ec = self.ec();
            if is_string_literal_like(&v.factory().node(first_argument)) {
                let rewritten = rewrite_module_specifier(
                    &mut ec,
                    v.factory_mut(),
                    Some(first_argument),
                    &self.compiler_options,
                )
                .expect(NIL);
                first_argument_changed = rewritten != first_argument;
                first_argument = rewritten;
            } else {
                first_argument = ec.new_rewrite_relative_import_extensions_helper(
                    v.factory_mut(),
                    first_argument,
                    self.compiler_options.jsx == JsxEmit::PRESERVE,
                );
                first_argument_changed = true;
            }

            let (rest, rest_changed) =
                self.visit_slice_with(v, Visitor::Main, &argument_nodes[1..]);
            if first_argument_changed || rest_changed {
                let f = v.factory_mut();
                let mut new_arguments = vec![first_argument];
                new_arguments.extend(rest);
                let list = new_node_list(f, new_arguments);
                let loc = f.read_list(arguments.expect(NIL)).loc();
                f.set_list_location(list, loc);
                arguments_list = Some(list);
            }
        }

        v.factory_mut().update_call_expression(
            node,
            expression,
            question_dot_token,
            None, /*typeArguments*/
            arguments_list,
            flags,
        )
    }

    /// Visits a tagged template expression that might reference an imported
    /// symbol and thus require an indirect call.
    // port: tsc/internal/transformers/moduletransforms/commonjsmodule.go:CommonJSModuleTransformer.visitTaggedTemplateExpression
    fn visit_tagged_template_expression(&self, v: &mut NodeVisitor<'_>, node: NodeId) -> Visited {
        let (tag, template, flags) = {
            let read = v.factory().node(node);
            let data = read
                .as_tagged_template_expression()
                .expect("TaggedTemplateExpression payload");
            (data.tag().expect(NIL), data.template(), read.flags())
        };
        if tsr_ast::is_identifier(&v.factory().node(tag)) {
            // given:
            //   import { f } from "mod";
            //   f``;
            // emits:
            //   const mod_1 = require("mod");
            //   (0, mod_1.f) ``;
            // note:
            //   the indirect call is applied by the printer by way of the `EFIndirectCall` emit flag.

            let expression = self.visit_expression_identifier(v.factory_mut(), tag)?;
            let template = self.visit_node_with(v, Visitor::Main, template);
            let updated = v.factory_mut().update_tagged_template_expression(
                node,
                Some(expression),
                None, /*questionDotToken*/
                None, /*typeArguments*/
                template,
                flags,
            );
            if !tsr_ast::is_identifier(&v.factory().node(expression))
                && !is_helper_name(&self.context, tag)
            {
                self.ec().add_emit_flags(updated, emit_flags::INDIRECT_CALL);
            }
            return Ok(Some(updated));
        }
        Ok(self.visit_each_child_with(v, Visitor::Main, node))
    }

    /// Visits a shorthand property assignment that might reference an
    /// imported or exported symbol.
    // port: tsc/internal/transformers/moduletransforms/commonjsmodule.go:CommonJSModuleTransformer.visitShorthandPropertyAssignment
    fn visit_shorthand_property_assignment(
        &self,
        v: &mut NodeVisitor<'_>,
        node: NodeId,
    ) -> Visited {
        let (name, equals_token, object_assignment_initializer) =
            shorthand_parts(v.factory(), node);
        let name = name.expect(NIL);
        let exported_or_imported_name = self.visit_expression_identifier(v.factory_mut(), name)?;
        if exported_or_imported_name != name {
            // A shorthand property with an assignment initializer is probably part of a
            // destructuring assignment
            let mut expression = exported_or_imported_name;
            let mut ec = self.ec();
            if object_assignment_initializer.is_some() {
                let initializer = self
                    .visit_node_with(v, Visitor::Main, object_assignment_initializer)
                    .expect(NIL);
                expression = ec.new_assignment_expression(v.factory_mut(), expression, initializer);
            }
            let f = v.factory_mut();
            let assignment = f.new_property_assignment(
                None, /*modifiers*/
                Some(name),
                None, /*postfixToken*/
                None, /*typeNode*/
                Some(expression),
            );
            let loc = f.node(node).range();
            f.set_node_range(assignment, loc);
            ec.assign_comment_and_source_map_ranges(f, assignment, node);
            return Ok(Some(assignment));
        }
        let initializer = self.visit_node_with(v, Visitor::Main, object_assignment_initializer);
        Ok(Some(v.factory_mut().update_shorthand_property_assignment(
            node,
            None, /*modifiers*/
            Some(exported_or_imported_name),
            None, /*postfixToken*/
            None, /*typeNode*/
            equals_token,
            initializer,
        )))
    }

    /// Visits an identifier that, if it is in an expression position, might
    /// reference an imported or exported symbol.
    // port: tsc/internal/transformers/moduletransforms/commonjsmodule.go:CommonJSModuleTransformer.visitIdentifier
    fn visit_identifier(&self, v: &mut NodeVisitor<'_>, node: NodeId) -> Visited {
        if is_identifier_reference(v.factory(), node, self.parent_node.get().expect(NIL)) {
            return self
                .visit_expression_identifier(v.factory_mut(), node)
                .map(Some);
        }
        Ok(Some(node))
    }

    /// Visits an identifier in an expression position that might reference an
    /// imported or exported symbol.
    // port: tsc/internal/transformers/moduletransforms/commonjsmodule.go:CommonJSModuleTransformer.visitExpressionIdentifier
    fn visit_expression_identifier(
        &self,
        f: &mut dyn RuntimeFactory,
        node: NodeId,
    ) -> Result<NodeId, Error> {
        let mut ec = self.ec();
        let info = ec.auto_generate_info(node);
        if info.is_none_or(|info| info.flags.has_allow_name_substitution())
            && !is_helper_name(&ec, node)
            && !is_local_name(&ec, node)
            && !is_declaration_name_of_enum_or_namespace(&ec, f, node)
        {
            let export_container = self.resolver.borrow_mut().get_referenced_export_container(
                ec.most_original(node),
                is_export_name(&ec, node),
            )?;
            if export_container.is_some_and(|container| f.node(container).kind() == K::SourceFile) {
                let exports = new_identifier(f, b"exports");
                let name = tsr_ast::clone_node(f, node);
                let reference = f.new_property_access_expression(
                    Some(exports),
                    None, /*questionDotToken*/
                    Some(name),
                    node_flags::NONE,
                );
                ec.assign_comment_and_source_map_ranges(f, reference, node);
                let loc = f.node(node).range();
                f.set_node_range(reference, loc);
                return Ok(reference);
            }

            let import_declaration = self
                .resolver
                .borrow_mut()
                .get_referenced_import_declaration(ec.most_original(node))?;
            if let Some(import_declaration) = import_declaration {
                if tsr_ast::is_import_clause(&f.node(import_declaration)) {
                    // Resolver returns parse-tree declarations; Parent is used to find the owning import declaration.
                    let parent = f.node(import_declaration).parent().expect(NIL);
                    let target = ec.new_generated_name_for_node(f, parent);
                    let default = new_identifier(f, b"default");
                    let reference = f.new_property_access_expression(
                        Some(target),
                        None, /*questionDotToken*/
                        Some(default),
                        node_flags::NONE,
                    );
                    ec.assign_comment_and_source_map_ranges(f, reference, node);
                    let loc = f.node(node).range();
                    f.set_node_range(reference, loc);
                    return Ok(reference);
                }
                if tsr_ast::is_import_specifier(&f.node(import_declaration)) {
                    let name = f
                        .node(import_declaration)
                        .property_name_or_name()
                        .expect(NIL);
                    let decl = tsr_ast::utilities::find_ancestor(
                        view(f)?,
                        Some(import_declaration),
                        |read| tsr_ast::is_import_declaration(read),
                    )?;
                    let target =
                        ec.new_generated_name_for_node(f, decl.unwrap_or(import_declaration));
                    let reference = if tsr_ast::is_string_literal(&f.node(name)) {
                        let literal = ec.new_string_literal_from_node(f, name);
                        f.new_element_access_expression(
                            Some(target),
                            None, /*questionDotToken*/
                            Some(literal),
                            node_flags::NONE,
                        )
                    } else {
                        let reference_name = tsr_ast::clone_node(f, name);
                        ec.add_emit_flags(
                            reference_name,
                            emit_flags::NO_SOURCE_MAP | emit_flags::NO_COMMENTS,
                        );
                        f.new_property_access_expression(
                            Some(target),
                            None, /*questionDotToken*/
                            Some(reference_name),
                            node_flags::NONE,
                        )
                    };
                    ec.assign_comment_and_source_map_ranges(f, reference, node);
                    let loc = f.node(node).range();
                    f.set_node_range(reference, loc);
                    return Ok(reference);
                }
            }
        }
        Ok(node)
    }

    /// Gets the exported names of an identifier, if it is exported.
    // port: tsc/internal/transformers/moduletransforms/commonjsmodule.go:CommonJSModuleTransformer.getExports
    fn get_exports(&self, f: &dyn RuntimeFactory, name: NodeId) -> Result<Vec<NodeId>, Error> {
        let info = self.info();
        if !is_generated_identifier(&self.context, name) {
            let import_declaration = self
                .resolver
                .borrow_mut()
                .get_referenced_import_declaration(self.context.most_original(name))?;
            if let Some(import_declaration) = import_declaration {
                return Ok(info.exported_bindings.get(&import_declaration).to_vec());
            }

            // An exported namespace or enum may merge with an ambient declaration, which won't show up in .js emit, so
            // we analyze all value exports of a symbol.
            let mut bindings_set = HashSet::new();
            let mut bindings = Vec::new();
            let declarations = self
                .resolver
                .borrow_mut()
                .get_referenced_value_declarations(self.context.most_original(name))?;
            if let Some(declarations) = declarations {
                for declaration in declarations {
                    for &binding in info.exported_bindings.get(&declaration) {
                        if bindings_set.insert(binding) {
                            bindings.push(binding);
                        }
                    }
                }
                return Ok(bindings);
            }
        } else if is_file_level_reserved_generated_identifier(&self.context, name) {
            let export_specifiers = info.export_specifiers.get(&node_text(f, name)?);
            if !export_specifiers.is_empty() {
                let mut exported_names = Vec::with_capacity(export_specifiers.len());
                for &export_specifier in export_specifiers {
                    exported_names.push(f.node(export_specifier).name().expect(NIL));
                }
                return Ok(exported_names);
            }
        }
        Ok(Vec::new())
    }
}

/// The state `visitTopLevelVariableStatement`'s closures share: the
/// statements emitted so far, the pending variable declarations and
/// expressions, and the modifiers kept for a local name.
#[derive(Default)]
struct PendingVariableStatement {
    statements: Vec<NodeId>,
    variables: Vec<NodeId>,
    expressions: Vec<NodeId>,
    modifiers: Option<NodeListId>,
}

impl PendingVariableStatement {
    /// `commitPendingVariables`.
    fn commit_pending_variables(
        &mut self,
        ec: &mut EmitContext,
        f: &mut dyn RuntimeFactory,
        node: NodeId,
    ) {
        if !self.variables.is_empty() {
            let declaration_list = f
                .node(node)
                .as_variable_statement()
                .expect("VariableStatement payload")
                .declaration_list()
                .expect(NIL);
            let flags = f.node(declaration_list).flags();
            let variable_list = new_node_list(f, std::mem::take(&mut self.variables));
            let declaration_list =
                f.update_variable_declaration_list(declaration_list, Some(variable_list), flags);
            let statement =
                f.update_variable_statement(node, self.modifiers, Some(declaration_list));
            if !self.statements.is_empty() {
                ec.add_emit_flags(statement, emit_flags::NO_COMMENTS);
            }
            self.statements.push(statement);
        }
    }

    /// `commitPendingExpressions`.
    fn commit_pending_expressions(
        &mut self,
        ec: &mut EmitContext,
        f: &mut dyn RuntimeFactory,
        node: NodeId,
    ) {
        if !self.expressions.is_empty() {
            let expressions = std::mem::take(&mut self.expressions);
            let expression = ec.inline_expressions(f, &expressions);
            let statement = f.new_expression_statement(expression);
            ec.assign_comment_and_source_map_ranges(f, statement, node);
            if !self.statements.is_empty() {
                ec.add_emit_flags(statement, emit_flags::NO_COMMENTS);
            }
            self.statements.push(statement);
        }
    }

    /// `pushVariable`.
    fn push_variable(
        &mut self,
        ec: &mut EmitContext,
        f: &mut dyn RuntimeFactory,
        node: NodeId,
        variable: NodeId,
    ) {
        self.commit_pending_expressions(ec, f, node);
        self.variables.push(variable);
    }

    /// `pushExpression`.
    fn push_expression(
        &mut self,
        ec: &mut EmitContext,
        f: &mut dyn RuntimeFactory,
        node: NodeId,
        expression: NodeId,
    ) {
        self.commit_pending_variables(ec, f, node);
        self.expressions.push(expression);
    }
}
