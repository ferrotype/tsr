//! `transformers/estransforms/classfields.go`: lowers class fields, private
//! names (`#x`, `#x in o`, private methods and accessors), class static
//! blocks, `accessor` fields and `this`/`super` in static initializers.
//!
//! Upstream keeps ten node visitors: the transformer's own (`tx.Visitor()`)
//! and nine created from the emit context, each with its own callback. Here a
//! visitor borrows the factory, so every method takes the visitor it was
//! reached through (`v`) and creates the visitor upstream names over that
//! visitor's factory ([`ClassFieldsTransformer::with`]); the emit-context
//! hooks are the same for all of them, as upstream's are.
//!
//! The class lexical environments form upstream's linked list of `Rc` nodes;
//! the class data of one is copied in and out of its `Cell` (upstream mutates
//! it through a pointer that never outlives one environment), and the private
//! identifier infos are values in the environment's maps, updated by key where
//! upstream writes through a pointer it read from the same map.
use super::classthis::is_class_this_assignment_block;
use super::namedevaluation::{
    class_has_explicitly_assigned_name, is_class_named_evaluation_helper_block,
    is_named_evaluation_and, transform_named_evaluation,
};
use super::utilities::{
    create_accessor_property_backing_field, identifier_text, is_assignment_expression,
    is_super_property, list_nodes, new_node_list, NIL,
};
use crate::extract_modifiers;
use crate::transformer::{Error, Failure, SharedReferenceResolver, TransformOptions, Transformer};
use crate::utilities::{
    find_super_statement_index_path, get_non_assignment_operator_for_compound_assignment,
    is_generated_identifier, is_simple_copiable_expression, is_simple_inlineable_expression,
    move_range_past_modifiers,
};
use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::sync::Arc;
use tsr_ast::subtree_flags;
use tsr_ast::utilities::outer_expression_kinds;
use tsr_ast::{
    clone_node, modifier_flags, node_flags, token_flags, AstView, Factory, FactoryMethods,
    JsString, NodeId, NodeKind, NodeListId, NodeVisitor, RuntimeFactory, SyntaxKind as K,
};
use tsr_core::{CompilerOptions, LanguageVariant, ScriptTarget, TextRange};
use tsr_printer::generated_identifier_flags as g;
use tsr_printer::{
    emit_flags, AutoGenerateOptions, EmitContext, EmitVisitorHooks, PrivateIdentifierKind,
};

/// Go's `classFacts`: facts about a class being transformed.
mod class_facts {
    pub(super) const NONE: u32 = 0;
    pub(super) const CLASS_WAS_DECORATED: u32 = 1 << 0;
    pub(super) const NEEDS_CLASS_CONSTRUCTOR_REFERENCE: u32 = 1 << 1;
    pub(super) const NEEDS_CLASS_SUPER_REFERENCE: u32 = 1 << 2;
    pub(super) const NEEDS_SUBSTITUTION_FOR_THIS_IN_CLASS_STATIC_FIELD: u32 = 1 << 3;
    pub(super) const WILL_HOIST_INITIALIZERS_TO_CONSTRUCTOR: u32 = 1 << 4;
}

/// Go's `privateIdentifierInfo`: what a private identifier transforms to.
#[derive(Clone, Copy, Debug)]
struct PrivateIdentifierInfo {
    kind: PrivateIdentifierKind,
    /// For an instance field, the WeakMap that stores it; for instance methods
    /// and accessors, the WeakSet used for brand checks; for static members,
    /// the constructor used for brand checks.
    brand_check_identifier: Option<NodeId>,
    is_static: bool,
    /// Reserved names (`#constructor`) and duplicates are invalid.
    is_valid: bool,
    /// The variable that stores a static field.
    variable_name: Option<NodeId>,
    /// The variable that holds a private method's implementation.
    method_name: Option<NodeId>,
    /// The variable that holds a private get accessor's implementation.
    getter_name: Option<NodeId>,
    /// The variable that holds a private set accessor's implementation.
    setter_name: Option<NodeId>,
}

impl PrivateIdentifierInfo {
    fn new(kind: PrivateIdentifierKind) -> Self {
        Self {
            kind,
            brand_check_identifier: None,
            is_static: false,
            is_valid: false,
            variable_name: None,
            method_name: None,
            getter_name: None,
            setter_name: None,
        }
    }
}

/// Go's `privateEnvironmentData`.
#[derive(Clone, Copy, Debug, Default)]
struct PrivateEnvironmentData {
    /// Prefixes generated variable names.
    class_name: Option<NodeId>,
    /// The brand check of private methods.
    weak_set_name: Option<NodeId>,
}

/// Go's `privateEnvironment`: one map for source names (by text) and one for
/// generated names (by the node they were generated for), so that two
/// auto-accessors' backing fields with the same text do not collide.
#[derive(Debug, Default)]
struct PrivateEnvironment {
    data: PrivateEnvironmentData,
    members: HashMap<JsString, PrivateIdentifierInfo>,
    generated_identifiers: Option<HashMap<NodeId, PrivateIdentifierInfo>>,
}

/// Go's `classLexicalEnvironment`.
#[derive(Clone, Copy, Debug, Default)]
struct ClassLexicalEnvironment {
    facts: u32,
    /// Brand checks of static members, and `this` in static initializers.
    class_constructor: Option<NodeId>,
    class_this: Option<NodeId>,
    /// `super` in static initializers.
    super_class_reference: Option<NodeId>,
}

/// Go's `classLexicalEnv`, a node of the linked list of class environments.
#[derive(Debug, Default)]
struct ClassLexicalEnv {
    previous: Option<Rc<ClassLexicalEnv>>,
    data: Cell<Option<ClassLexicalEnvironment>>,
    private_env: RefCell<Option<PrivateEnvironment>>,
}

/// Which of upstream's node visitors a visitor created here is.
#[derive(Clone, Copy)]
enum Visitor {
    /// `tx.Visitor()`, whose callback is `visit`.
    Main,
    Modifier,
    DiscardedValue,
    HeritageClause,
    AssignmentTarget,
    ClassElement,
    AccessorFieldResult,
    ArrayAssignmentElement,
    ObjectAssignmentElement,
    Substitution,
}

/// A visit function `setCurrentClassElementAnd` and
/// `setInIterationStatementAnd` call.
type VisitFn<'a> = fn(&ClassFieldsTransformer<'a>, &mut NodeVisitor<'_>, NodeId) -> Option<NodeId>;
/// A visit function `visitInNewClassLexicalEnvironment` calls.
type ClassVisitFn<'a> =
    fn(&ClassFieldsTransformer<'a>, &mut NodeVisitor<'_>, NodeId, u32) -> Option<NodeId>;

const NO_BUILDER: &str = "a transform over a factory without AST storage";

struct ClassFieldsTransformer<'a> {
    context: EmitContext,
    hooks: EmitVisitorHooks,
    failure: Failure,
    compiler_options: Arc<CompilerOptions>,
    resolver: SharedReferenceResolver<'a>,

    should_transform_initializers_using_set: bool,
    should_transform_initializers_using_define: bool,
    should_transform_initializers: bool,
    should_transform_private_elements_or_class_static_blocks: bool,
    should_transform_auto_accessors: bool,
    should_transform_this_in_static_initializers: bool,
    should_transform_super_in_static_initializers: bool,
    should_transform_private_static_elements_in_file: Cell<bool>,
    legacy_decorators: bool,

    /// Computed name expressions of elided names, inlined at the next
    /// execution site in document order.
    pending_expressions: RefCell<Vec<NodeId>>,
    /// Computed name statements and static property initializers emitted at
    /// the next execution site (decorated classes).
    pending_statements: RefCell<Vec<NodeId>>,
    lexical_environment: RefCell<Option<Rc<ClassLexicalEnv>>>,
    current_class_container: Cell<Option<NodeId>>,
    current_class_element: Cell<Option<NodeId>>,
    /// Class declarations to the alias that replaces references to them in
    /// static initializers.
    class_aliases: RefCell<HashMap<NodeId, NodeId>>,
    enclosing_class_declarations: RefCell<HashSet<NodeId>>,
    in_iteration_statement: Cell<bool>,
    /// Inside a computed property name, which upstream's `this` substitution
    /// evaluates in the outer class environment.
    inside_computed_property_name: Cell<bool>,
    parent_node: Cell<Option<NodeId>>,
    current_node: Cell<Option<NodeId>>,
}

// port: tsc/internal/transformers/estransforms/classfields.go:newClassFieldsTransformer
pub fn new_class_fields_transformer<'a>(opts: &TransformOptions<'a>) -> Option<Transformer<'a>> {
    let language_version = opts.compiler_options.emit_script_target();
    let use_define_for_class_fields = opts.compiler_options.use_define_for_class_fields();

    // When targeting ESNext+ with useDefineForClassFields (the default), there are no class
    // field transformations to perform and no prior transform sets EFTransformPrivateStaticElements,
    // so every node would be returned unchanged. Skip entirely.
    if language_version >= ScriptTarget::ESNEXT && use_define_for_class_fields {
        return None;
    }

    // Always transform field initializers using Set semantics when `useDefineForClassFields: false`.
    let should_transform_initializers_using_set = !use_define_for_class_fields;
    // Transform field initializers using Define semantics when `useDefineForClassFields: true` and target < ES2022.
    let should_transform_initializers_using_define =
        use_define_for_class_fields && language_version < ScriptTarget::ES2022;
    // Since target is always >= ES2015, the `super` flag is always the same as the `this` flag.
    let should_transform_this_in_static_initializers = language_version < ScriptTarget::ES2022;

    let tx = ClassFieldsTransformer {
        context: opts.context.clone(),
        hooks: opts.context.visitor_hooks(),
        failure: opts.failure.clone(),
        compiler_options: Arc::clone(&opts.compiler_options),
        resolver: Rc::clone(&opts.resolver),
        should_transform_initializers_using_set,
        should_transform_initializers_using_define,
        should_transform_initializers: should_transform_initializers_using_set
            || should_transform_initializers_using_define,
        // We need to transform private members and class static blocks when target < ES2022.
        should_transform_private_elements_or_class_static_blocks: language_version
            < ScriptTarget::ES2022,
        // We need to transform `accessor` fields when target < ESNext.
        should_transform_auto_accessors: language_version < ScriptTarget::ESNEXT,
        should_transform_this_in_static_initializers,
        should_transform_super_in_static_initializers: should_transform_this_in_static_initializers,
        should_transform_private_static_elements_in_file: Cell::new(false),
        legacy_decorators: opts.compiler_options.experimental_decorators.is_true(),
        pending_expressions: RefCell::new(Vec::new()),
        pending_statements: RefCell::new(Vec::new()),
        lexical_environment: RefCell::new(None),
        current_class_container: Cell::new(None),
        current_class_element: Cell::new(None),
        class_aliases: RefCell::new(HashMap::new()),
        enclosing_class_declarations: RefCell::new(HashSet::new()),
        in_iteration_statement: Cell::new(false),
        inside_computed_property_name: Cell::new(false),
        parent_node: Cell::new(None),
        current_node: Cell::new(None),
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

/// `Node.Kind` of a node read through `factory`.
fn kind_of(factory: &(impl Factory + ?Sized), node: NodeId) -> NodeKind {
    factory.node(node).kind()
}

/// `Node.Loc`.
fn loc_of(factory: &(impl tsr_ast::Factory + ?Sized), node: NodeId) -> TextRange {
    factory.node(node).range()
}

/// `node.Name()`, which the caller dereferences.
fn name_of(factory: &(impl tsr_ast::Factory + ?Sized), node: NodeId) -> NodeId {
    factory.node(node).name().expect(NIL)
}

/// `node.Expression()`, which the caller dereferences.
fn expression_of(factory: &(impl tsr_ast::Factory + ?Sized), node: NodeId) -> NodeId {
    factory.node(node).expression().expect(NIL)
}

/// The `Left`, `OperatorToken` kind and `Right` of a binary expression.
fn binary_parts(
    factory: &(impl tsr_ast::Factory + ?Sized),
    node: NodeId,
) -> (NodeId, NodeKind, NodeId, NodeId) {
    let read = factory.node(node);
    let binary = read
        .as_binary_expression()
        .expect("BinaryExpression payload");
    let operator_token = binary.operator_token().expect(NIL);
    (
        binary.left().expect(NIL),
        factory.node(operator_token).kind(),
        operator_token,
        binary.right().expect(NIL),
    )
}

/// The statements of a static block's body (its payload's `Body`; upstream's
/// `Node.Body()` answers nil for one).
fn static_block_statements(
    factory: &dyn RuntimeFactory,
    node: NodeId,
) -> (NodeListId, Vec<NodeId>) {
    let body = factory
        .node(node)
        .as_class_static_block_declaration()
        .expect("ClassStaticBlockDeclaration payload")
        .body()
        .expect(NIL);
    let statements = factory.node(body).statement_list().expect(NIL);
    (statements, list_nodes(factory, Some(statements)))
}

/// `node.Members()`.
fn members_of(factory: &dyn RuntimeFactory, node: NodeId) -> Vec<NodeId> {
    list_nodes(factory, factory.node(node).member_list())
}

impl<'a> ClassFieldsTransformer<'a> {
    /// Runs `f` with one of upstream's visitors over `v`'s factory.
    fn with<R>(
        &self,
        v: &mut NodeVisitor<'_>,
        which: Visitor,
        f: impl FnOnce(&mut NodeVisitor<'_>) -> R,
    ) -> R {
        let visit = |visitor: &mut NodeVisitor<'_>, node: Option<NodeId>| -> Option<NodeId> {
            let node = node?;
            if self.failure.is_set() {
                return Some(node);
            }
            match which {
                Visitor::Main => self.visit(visitor, node),
                Visitor::Modifier => self.visit_modifier(visitor, node),
                Visitor::DiscardedValue => self.visit_discarded_value(visitor, node),
                Visitor::HeritageClause => self.visit_heritage_clause(visitor, node),
                Visitor::AssignmentTarget => self.visit_assignment_target(visitor, node),
                Visitor::ClassElement => self.visit_class_element(visitor, node),
                Visitor::AccessorFieldResult => self.visit_accessor_field_result(visitor, node),
                Visitor::ArrayAssignmentElement => {
                    self.visit_array_assignment_element(visitor, node)
                }
                Visitor::ObjectAssignmentElement => {
                    self.visit_object_assignment_element(visitor, node)
                }
                Visitor::Substitution => self.visit_for_substitution(visitor, node),
            }
        };
        let mut created = self.hooks.new_node_visitor(Some(&visit), v.factory_mut());
        f(&mut created)
    }

    /// `tx.Visitor().VisitNode(node)`.
    fn visit_node(&self, v: &mut NodeVisitor<'_>, node: Option<NodeId>) -> Option<NodeId> {
        self.with(v, Visitor::Main, |m| m.visit_node(node))
    }

    /// `tx.Visitor().VisitNodes(list)`.
    fn visit_nodes(&self, v: &mut NodeVisitor<'_>, list: Option<NodeListId>) -> Option<NodeListId> {
        self.with(v, Visitor::Main, |m| m.visit_nodes(list))
    }

    /// `tx.Visitor().VisitEachChild(node)`.
    fn visit_each_child(&self, v: &mut NodeVisitor<'_>, node: NodeId) -> Option<NodeId> {
        self.with(v, Visitor::Main, |m| m.visit_each_child(Some(node)))
    }

    /// `visitor.VisitSlice(nodes)` with one of upstream's visitors.
    fn visit_slice_with(
        &self,
        v: &mut NodeVisitor<'_>,
        which: Visitor,
        nodes: &[NodeId],
    ) -> Vec<NodeId> {
        let slice = v
            .factory_mut()
            .alloc_nodes(nodes.iter().copied().map(Some).collect());
        let (visited, _) = self.with(v, which, |visitor| visitor.visit_slice(slice));
        v.factory()
            .read_nodes(visited)
            .iter()
            .map(|node| node.expect(NIL))
            .collect()
    }

    /// An AST query over the factory's parsed view. A failed query is
    /// recorded, which fails the file, and answers `T::default()`.
    fn q<T: Default>(
        &self,
        factory: &dyn RuntimeFactory,
        query: impl FnOnce(AstView<'_>) -> Result<T, tsr_arena::Error>,
    ) -> T {
        let Some(view) = factory.ast_view() else {
            self.failure.record(Error::Unsupported(NO_BUILDER));
            return T::default();
        };
        self.failure.ok(query(view)).unwrap_or_default()
    }

    /// [`Self::q`] for a query that answers a node: `fallback` after a
    /// failure.
    fn q_node(
        &self,
        factory: &dyn RuntimeFactory,
        fallback: NodeId,
        query: impl FnOnce(AstView<'_>) -> Result<NodeId, tsr_arena::Error>,
    ) -> NodeId {
        let Some(view) = factory.ast_view() else {
            self.failure.record(Error::Unsupported(NO_BUILDER));
            return fallback;
        };
        self.failure.ok(query(view)).unwrap_or(fallback)
    }

    /// `node.SubtreeFacts()`.
    fn subtree_facts(&self, factory: &dyn RuntimeFactory, node: NodeId) -> u32 {
        self.q(factory, |view| Ok(view.subtree_facts(node)))
    }

    fn has_static_modifier(&self, factory: &dyn RuntimeFactory, node: NodeId) -> bool {
        self.q(factory, |view| {
            tsr_ast::utilities::has_static_modifier(view, node)
        })
    }

    fn is_static(&self, factory: &dyn RuntimeFactory, node: NodeId) -> bool {
        self.q(factory, |view| tsr_ast::utilities::is_static(view, node))
    }

    fn has_accessor_modifier(&self, factory: &dyn RuntimeFactory, node: NodeId) -> bool {
        self.q(factory, |view| {
            tsr_ast::utilities::has_accessor_modifier(view, node)
        })
    }

    fn is_private_identifier_class_element_declaration(
        &self,
        factory: &dyn RuntimeFactory,
        node: NodeId,
    ) -> bool {
        self.q(factory, |view| {
            tsr_ast::utilities::is_private_identifier_class_element_declaration(view, node)
        })
    }

    fn is_auto_accessor_property_declaration(
        &self,
        factory: &dyn RuntimeFactory,
        node: NodeId,
    ) -> bool {
        self.q(factory, |view| {
            tsr_ast::utilities::is_auto_accessor_property_declaration(view, node)
        })
    }

    /// `ast.IsParameterPropertyDeclaration(node, parent)`.
    fn is_parameter_property_declaration(
        &self,
        factory: &dyn RuntimeFactory,
        node: NodeId,
        parent: NodeId,
    ) -> bool {
        self.q(factory, |view| {
            tsr_ast::utilities::is_parameter_property_declaration(view, node, parent)
        })
    }

    /// `ast.SkipParentheses`.
    fn skip_parentheses(&self, factory: &dyn RuntimeFactory, node: NodeId) -> NodeId {
        self.q_node(factory, node, |view| tsr_ast::skip_parentheses(view, node))
    }

    /// `tx.Factory().GetLocalName(node)`, which needs the concrete builder.
    fn get_local_name(&self, v: &mut NodeVisitor<'_>, node: NodeId) -> NodeId {
        let mut context = self.context.clone();
        let result = match v.factory_mut().ast_builder_mut() {
            Some(builder) => context
                .get_local_name(builder, Some(node))
                .map_err(Error::from),
            None => Err(Error::Unsupported(NO_BUILDER)),
        };
        match result {
            Ok(name) => name,
            Err(error) => {
                self.failure.record(error);
                node
            }
        }
    }

    /// `isNamedEvaluationAnd(tx.EmitContext(), node, tx.isAnonymousClassNeedingAssignedName)`.
    fn is_named_evaluation_needing_assigned_name(
        &self,
        factory: &dyn RuntimeFactory,
        node: NodeId,
    ) -> bool {
        let cb = |factory: &dyn RuntimeFactory, node: NodeId| {
            self.is_anonymous_class_needing_assigned_name_worker(factory, node)
        };
        is_named_evaluation_and(&self.context, factory, node, Some(&cb))
    }

    /// Returns true when private field temp variables should be declared as
    /// block-scoped (`let`) rather than function-scoped (`var`): when a class
    /// expression is directly inside a loop body. Replaces Strada's
    /// `resolver.hasNodeCheckFlag(node, NodeCheckFlags.BlockScopedBindingInLoop)`.
    // port: tsc/internal/transformers/estransforms/classfields.go:classFieldsTransformer.requiresBlockScopedVar
    fn requires_block_scoped_var(&self, factory: &dyn RuntimeFactory) -> bool {
        self.in_iteration_statement.get()
            && self
                .current_class_container
                .get()
                .is_some_and(|container| kind_of(factory, container) == K::ClassExpression)
    }

    /// Whether the class expression's temp variable must be block-scoped: a
    /// class expression with a non-static property with a computed property
    /// name inside a loop (the checker's `BlockScopedBindingInLoop` on the
    /// class node).
    // port: tsc/internal/transformers/estransforms/classfields.go:classFieldsTransformer.classExpressionNeedsBlockScopedTemp
    fn class_expression_needs_block_scoped_temp(&self, factory: &dyn RuntimeFactory) -> bool {
        if !self.requires_block_scoped_var(factory) {
            return false;
        }
        let container = self.current_class_container.get().expect(NIL);
        for member in members_of(factory, container) {
            if kind_of(factory, member) == K::PropertyDeclaration
                && !self.has_static_modifier(factory, member)
            {
                if let Some(name) = factory.node(member).name() {
                    if kind_of(factory, name) == K::ComputedPropertyName {
                        return true;
                    }
                }
            }
        }
        false
    }

    // port: tsc/internal/transformers/estransforms/classfields.go:classFieldsTransformer.visitSourceFile
    #[allow(clippy::unnecessary_wraps)] // a visit function; upstream's result is a nullable node
    fn visit_source_file(&self, v: &mut NodeVisitor<'_>, node: NodeId) -> Option<NodeId> {
        let is_declaration_file = match v.factory().read_source_file(node) {
            Ok(file) => file.is_declaration_file,
            Err(error) => {
                self.failure.record(error);
                return Some(node);
            }
        };
        if is_declaration_file {
            return Some(node);
        }
        *self.lexical_environment.borrow_mut() = None;
        self.should_transform_private_static_elements_in_file.set(
            self.context.emit_flags(node) & emit_flags::TRANSFORM_PRIVATE_STATIC_ELEMENTS != 0,
        );
        self.class_aliases.borrow_mut().clear();
        self.enclosing_class_declarations.borrow_mut().clear();
        let visited = self.visit_each_child(v, node).expect(NIL);
        let mut context = self.context.clone();
        let helpers = context.read_emit_helpers();
        context.add_emit_helper(visited, &helpers);
        self.class_aliases.borrow_mut().clear();
        self.enclosing_class_declarations.borrow_mut().clear();
        Some(visited)
    }

    // port: tsc/internal/transformers/estransforms/classfields.go:classFieldsTransformer.visitModifier
    fn visit_modifier(&self, v: &mut NodeVisitor<'_>, node: NodeId) -> Option<NodeId> {
        let read = v.node(node);
        if read.kind() == K::AccessorKeyword {
            if self.should_transform_auto_accessors_in_current_class() {
                return None;
            }
            return Some(node);
        }
        if tsr_ast::utilities::is_modifier(&read) {
            return Some(node);
        }
        None
    }

    /// Returns the grandparent node, which `pop_node` restores.
    // port: tsc/internal/transformers/estransforms/classfields.go:classFieldsTransformer.pushNode
    fn push_node(&self, node: NodeId) -> Option<NodeId> {
        let grandparent_node = self.parent_node.get();
        self.parent_node.set(self.current_node.get());
        self.current_node.set(Some(node));
        grandparent_node
    }

    // port: tsc/internal/transformers/estransforms/classfields.go:classFieldsTransformer.popNode
    fn pop_node(&self, grandparent_node: Option<NodeId>) {
        self.current_node.set(self.parent_node.get());
        self.parent_node.set(grandparent_node);
    }

    /// Visits nodes solely for class alias substitution in subtrees without
    /// class field or lexical `this`/`super` transforms: substitutes
    /// identifiers that reference class declarations with their aliases,
    /// skipping the name of a property access (Strada's `onSubstituteNode`
    /// only fires for `EmitHint.Expression`).
    // port: tsc/internal/transformers/estransforms/classfields.go:classFieldsTransformer.visitForSubstitution
    fn visit_for_substitution(&self, v: &mut NodeVisitor<'_>, node: NodeId) -> Option<NodeId> {
        let kind = kind_of(v, node);
        if kind == K::Identifier {
            return Some(self.visit_identifier(v, node));
        }
        if kind == K::PropertyAccessExpression && kind_of(v, name_of(v, node)) == K::Identifier {
            return self.visit_property_access_expression_for_substitution(v, node);
        }
        self.with(v, Visitor::Substitution, |s| s.visit_each_child(Some(node)))
    }

    /// The main visitor.
    // port: tsc/internal/transformers/estransforms/classfields.go:classFieldsTransformer.visit
    fn visit(&self, v: &mut NodeVisitor<'_>, node: NodeId) -> Option<NodeId> {
        let grandparent_node = self.push_node(node);
        let result = self.visit_worker(v, node);
        self.pop_node(grandparent_node);
        result
    }

    /// The body of `visit` between its `pushNode` and deferred `popNode`.
    fn visit_worker(&self, v: &mut NodeVisitor<'_>, node: NodeId) -> Option<NodeId> {
        if self.subtree_facts(v.factory(), node)
            & (subtree_flags::CLASS_FIELDS | subtree_flags::LEXICAL_THIS_OR_SUPER)
            == 0
        {
            if self.current_class_container.get().is_some()
                && !self.class_aliases.borrow().is_empty()
            {
                // Continue visiting for alias substitution even in non-class-field subtrees.
                return self.visit_for_substitution(v, node);
            }
            return Some(node);
        }

        match kind_of(v, node).known() {
            Some(K::SourceFile) => self.visit_source_file(v, node),
            Some(K::ClassDeclaration) => self.visit_class_declaration(v, node),
            Some(K::ClassExpression) => self.visit_class_expression(v, node),
            Some(K::ClassStaticBlockDeclaration | K::PropertyDeclaration) => {
                panic!("Use `classElementVisitor` instead.")
            }
            Some(K::PropertyAssignment) => self.visit_property_assignment(v, node),
            Some(K::VariableStatement) => self.visit_variable_statement(v, node),
            Some(K::VariableDeclaration) => self.visit_variable_declaration(v, node),
            Some(K::Parameter) => self.visit_parameter_declaration(v, node),
            Some(K::BindingElement) => self.visit_binding_element(v, node),
            Some(K::ExportAssignment) => self.visit_export_assignment(v, node),
            Some(K::PrivateIdentifier) => self.visit_private_identifier(v, node),
            Some(K::PropertyAccessExpression) => self.visit_property_access_expression(v, node),
            Some(K::ElementAccessExpression) => self.visit_element_access_expression(v, node),
            Some(K::PrefixUnaryExpression | K::PostfixUnaryExpression) => {
                self.visit_pre_or_postfix_unary_expression(v, node, false /*discarded*/)
            }
            Some(K::BinaryExpression) => {
                self.visit_binary_expression(v, node, false /*discarded*/)
            }
            Some(K::ParenthesizedExpression) => {
                self.visit_parenthesized_expression(v, node, false /*discarded*/)
            }
            Some(K::CallExpression) => self.visit_call_expression(v, node),
            Some(K::ExpressionStatement) => self.visit_expression_statement(v, node),
            Some(K::TaggedTemplateExpression) => self.visit_tagged_template_expression(v, node),
            Some(K::ForStatement) => self.visit_for_statement(v, node),
            Some(K::ForInStatement | K::ForOfStatement | K::DoStatement | K::WhileStatement) => {
                self.set_in_iteration_statement_and(true, Self::visit_each_child_of_node, v, node)
            }
            Some(K::ThisKeyword) => self.visit_this_expression(v, node),
            Some(K::FunctionDeclaration | K::FunctionExpression) => self
                .set_in_iteration_statement_and(
                    false,
                    Self::visit_function_expression_or_declaration,
                    v,
                    node,
                ),
            Some(K::Constructor | K::MethodDeclaration | K::GetAccessor | K::SetAccessor) => self
                .set_in_iteration_statement_and(
                    false,
                    Self::set_class_element_and_visit_each_child,
                    v,
                    node,
                ),
            _ => self.visit_each_child(v, node),
        }
    }

    /// Visits a node in an expression whose result is discarded.
    // port: tsc/internal/transformers/estransforms/classfields.go:classFieldsTransformer.visitDiscardedValue
    fn visit_discarded_value(&self, v: &mut NodeVisitor<'_>, node: NodeId) -> Option<NodeId> {
        match kind_of(v, node).known() {
            Some(K::PrefixUnaryExpression | K::PostfixUnaryExpression) => {
                self.visit_pre_or_postfix_unary_expression(v, node, true /*discarded*/)
            }
            Some(K::BinaryExpression) => {
                self.visit_binary_expression(v, node, true /*discarded*/)
            }
            Some(K::ParenthesizedExpression) => {
                self.visit_parenthesized_expression(v, node, true /*discarded*/)
            }
            _ => self.visit(v, node),
        }
    }

    /// Visits a node in a HeritageClause.
    // port: tsc/internal/transformers/estransforms/classfields.go:classFieldsTransformer.visitHeritageClause
    fn visit_heritage_clause(&self, v: &mut NodeVisitor<'_>, node: NodeId) -> Option<NodeId> {
        match kind_of(v, node).known() {
            Some(K::HeritageClause) => self.with(v, Visitor::HeritageClause, |h| {
                h.visit_each_child(Some(node))
            }),
            Some(K::ExpressionWithTypeArguments) => {
                self.visit_expression_with_type_arguments_in_heritage_clause(v, node)
            }
            _ => self.visit(v, node),
        }
    }

    /// Visits the assignment target of a destructuring assignment.
    // port: tsc/internal/transformers/estransforms/classfields.go:classFieldsTransformer.visitAssignmentTarget
    fn visit_assignment_target(&self, v: &mut NodeVisitor<'_>, node: NodeId) -> Option<NodeId> {
        match kind_of(v, node).known() {
            Some(K::ObjectLiteralExpression | K::ArrayLiteralExpression) => {
                self.visit_assignment_pattern(v, node)
            }
            _ => self.visit(v, node),
        }
    }

    /// The class data of the current environment when a `super` property in
    /// a static initializer is transformed: `shouldTransformSuperInStaticInitializers`,
    /// a current class element that is a static property or static block, and
    /// class data.
    fn static_super_data(
        &self,
        factory: &dyn RuntimeFactory,
        is_super_property: bool,
    ) -> Option<ClassLexicalEnvironment> {
        if !self.should_transform_super_in_static_initializers || !is_super_property {
            return None;
        }
        let element = self.current_class_element.get()?;
        if !self.is_static_property_declaration_or_class_static_block(factory, element) {
            return None;
        }
        self.lex_data()
    }

    // port: tsc/internal/transformers/estransforms/classfields.go:classFieldsTransformer.visitDestructuringAssignmentTarget
    fn visit_destructuring_assignment_target(
        &self,
        v: &mut NodeVisitor<'_>,
        node: NodeId,
    ) -> Option<NodeId> {
        let kind = kind_of(v, node);
        if kind == K::ObjectLiteralExpression || kind == K::ArrayLiteralExpression {
            return self.visit_assignment_pattern(v, node);
        }
        if kind == K::PropertyAccessExpression
            && kind_of(v, name_of(v, node)) == K::PrivateIdentifier
        {
            return self.wrap_private_identifier_for_destructuring_target(v, node);
        }
        let super_property =
            self.current_class_element.get().is_some() && is_super_property(v.factory(), node);
        if let Some(data) = self.static_super_data(v.factory(), super_property) {
            if data.facts & class_facts::CLASS_WAS_DECORATED != 0 {
                return Some(self.visit_invalid_super_property(v, node));
            }
            if let (Some(class_constructor), Some(super_class_reference)) =
                (data.class_constructor, data.super_class_reference)
            {
                let name = if kind == K::ElementAccessExpression {
                    let argument = v
                        .node(node)
                        .as_element_access_expression()
                        .expect("ElementAccessExpression payload")
                        .argument_expression();
                    self.visit_node(v, argument)
                } else if kind == K::PropertyAccessExpression
                    && kind_of(v, name_of(v, node)) == K::Identifier
                {
                    let name = name_of(v, node);
                    Some(
                        self.context
                            .clone()
                            .new_string_literal_from_node(v.factory_mut(), name),
                    )
                } else {
                    None
                };
                if let Some(name) = name {
                    let mut context = self.context.clone();
                    let f = v.factory_mut();
                    let temp = context.new_temp_variable(f);
                    let set_expr = context.new_reflect_set_call(
                        f,
                        super_class_reference,
                        name,
                        temp,
                        class_constructor,
                    );
                    return Some(context.new_assignment_target_wrapper(f, temp, set_expr));
                }
            }
        }
        self.visit_each_child(v, node)
    }

    /// Visits a member of a class.
    // port: tsc/internal/transformers/estransforms/classfields.go:classFieldsTransformer.visitClassElement
    fn visit_class_element(&self, v: &mut NodeVisitor<'_>, node: NodeId) -> Option<NodeId> {
        match kind_of(v, node).known() {
            Some(K::Constructor) => self.set_current_class_element_and(
                Some(node),
                Self::visit_constructor_declaration,
                v,
                node,
            ),
            Some(K::GetAccessor | K::SetAccessor | K::MethodDeclaration) => self
                .set_current_class_element_and(
                    Some(node),
                    Self::visit_method_or_accessor_declaration,
                    v,
                    node,
                ),
            Some(K::PropertyDeclaration) => self.set_current_class_element_and(
                Some(node),
                Self::visit_property_declaration,
                v,
                node,
            ),
            Some(K::ClassStaticBlockDeclaration) => self.set_current_class_element_and(
                Some(node),
                Self::visit_class_static_block_declaration,
                v,
                node,
            ),
            Some(K::ComputedPropertyName) => self.visit_computed_property_name(v, node),
            Some(K::SemicolonClassElement) => Some(node),
            _ => {
                if tsr_ast::utilities::is_modifier_like(&v.node(node)) {
                    return self.visit_modifier(v, node);
                }
                self.visit(v, node)
            }
        }
    }

    /// Visits a property name of a class member.
    // port: tsc/internal/transformers/estransforms/classfields.go:classFieldsTransformer.visitPropertyName
    fn visit_property_name(&self, v: &mut NodeVisitor<'_>, name: Option<NodeId>) -> Option<NodeId> {
        if let Some(id) = name {
            if kind_of(v, id) == K::ComputedPropertyName {
                return self.visit_computed_property_name(v, id);
            }
        }
        self.visit_node(v, name)
    }

    /// Visits the results of an auto-accessor field transformation in a
    /// second pass.
    // port: tsc/internal/transformers/estransforms/classfields.go:classFieldsTransformer.visitAccessorFieldResult
    fn visit_accessor_field_result(&self, v: &mut NodeVisitor<'_>, node: NodeId) -> Option<NodeId> {
        match kind_of(v, node).known() {
            Some(K::PropertyDeclaration) => self.transform_field_initializer(v, node),
            Some(K::GetAccessor | K::SetAccessor) => self.visit_class_element(v, node),
            _ => panic!(
                "Debug failure. Expected node to either be a PropertyDeclaration, GetAccessorDeclaration, or SetAccessorDeclaration\nNode {} was unexpected.",
                v.node(node).kind_string()
            ),
        }
    }

    /// Replaces Strada's `onSubstituteNode`/`trySubstituteClassAlias`:
    /// resolves the identifier to its declaration and substitutes the alias
    /// registered for that declaration.
    // port: tsc/internal/transformers/estransforms/classfields.go:classFieldsTransformer.visitIdentifier
    fn visit_identifier(&self, v: &mut NodeVisitor<'_>, node: NodeId) -> NodeId {
        let original = self.context.most_original(node);
        let declaration = self
            .resolver
            .borrow_mut()
            .get_referenced_value_declaration(original);
        let Some(declaration) = self.failure.ok(declaration) else {
            return node;
        };
        if let Some(declaration) = declaration {
            let alias = self.class_aliases.borrow().get(&declaration).copied();
            if let Some(alias) = alias {
                if self
                    .enclosing_class_declarations
                    .borrow()
                    .contains(&declaration)
                {
                    let clone = clone_node(v.factory_mut(), alias);
                    let loc = loc_of(v, node);
                    let mut context = self.context.clone();
                    context.set_source_map_range(clone, loc);
                    context.set_comment_range(clone, loc);
                    return clone;
                }
            }
        }
        node
    }

    /// Handles an undeclared private name: replaces it with an empty
    /// identifier to indicate a problem with the code. A private identifier
    /// in statement position (`#;`) is preserved by `visitExpressionStatement`
    /// so the runtime throws a SyntaxError.
    // port: tsc/internal/transformers/estransforms/classfields.go:classFieldsTransformer.visitPrivateIdentifier
    #[allow(clippy::unnecessary_wraps)] // a visit function; upstream's result is a nullable node
    fn visit_private_identifier(&self, v: &mut NodeVisitor<'_>, node: NodeId) -> Option<NodeId> {
        if !self.should_transform_private_elements_or_class_static_blocks {
            return Some(node);
        }
        if let Some(parent) = self.parent_node.get() {
            if self.q(v.factory(), |view| {
                tsr_ast::utilities::is_statement(view, parent)
            }) {
                return Some(node);
            }
        }
        let result = v.factory_mut().new_identifier(JsString::default());
        self.context.clone().set_original(result, node);
        Some(result)
    }

    /// Visits `#id in expr`.
    // port: tsc/internal/transformers/estransforms/classfields.go:classFieldsTransformer.transformPrivateIdentifierInInExpression
    fn transform_private_identifier_in_in_expression(
        &self,
        v: &mut NodeVisitor<'_>,
        node: NodeId,
    ) -> Option<NodeId> {
        let (left, _, _, right) = binary_parts(v, node);
        let info = self.access_private_identifier(v.factory(), left);
        if let Some(info) = info {
            let receiver = self.visit_node(v, Some(right)).expect(NIL);
            let mut context = self.context.clone();
            let result = context.new_class_private_field_in_helper(
                v.factory_mut(),
                info.brand_check_identifier.expect(NIL),
                receiver,
            );
            context.set_original(result, node);
            return Some(result);
        }
        // Private name has not been declared. Subsequent transformers will handle this error
        self.visit_each_child(v, node)
    }

    /// `transformNamedEvaluation(tx.EmitContext(), node, ignoreEmptyStringLiteral, assignedName)`.
    fn transform_named_evaluation(
        &self,
        v: &mut NodeVisitor<'_>,
        node: NodeId,
        ignore_empty_string_literal: bool,
        assigned_name: &[u8],
    ) -> NodeId {
        transform_named_evaluation(
            &self.context,
            v.factory_mut(),
            node,
            ignore_empty_string_literal,
            assigned_name,
        )
    }

    // port: tsc/internal/transformers/estransforms/classfields.go:classFieldsTransformer.visitPropertyAssignment
    fn visit_property_assignment(&self, v: &mut NodeVisitor<'_>, node: NodeId) -> Option<NodeId> {
        // 13.2.5.5 RS: PropertyDefinitionEvaluation
        //   PropertyAssignment : PropertyName `:` AssignmentExpression
        //     ...
        //     5. If IsAnonymousFunctionDefinition(|AssignmentExpression|) is *true* and _isProtoSetter_ is *false*, then
        //        a. Let _popValue_ be ? NamedEvaluation of |AssignmentExpression| with argument _propKey_.
        //     ...
        let mut node = node;
        if self.is_named_evaluation_needing_assigned_name(v.factory(), node) {
            node = self.transform_named_evaluation(
                v, node, false, /*ignoreEmptyStringLiteral*/
                b"",   /*assignedName*/
            );
        }
        self.visit_each_child(v, node)
    }

    // port: tsc/internal/transformers/estransforms/classfields.go:classFieldsTransformer.visitVariableStatement
    #[allow(clippy::unnecessary_wraps)] // a visit function; upstream's result is a nullable node
    fn visit_variable_statement(&self, v: &mut NodeVisitor<'_>, node: NodeId) -> Option<NodeId> {
        let saved_pending_statements = self.pending_statements.take();

        let visited_node = self.visit_each_child(v, node).expect(NIL);

        let pending_statements = self.pending_statements.replace(saved_pending_statements);
        if !pending_statements.is_empty() {
            let mut result = Vec::with_capacity(1 + pending_statements.len());
            result.push(Some(visited_node));
            result.extend(pending_statements.into_iter().map(Some));
            let f = v.factory_mut();
            let children = f.alloc_nodes(result);
            return Some(f.new_syntax_list(children));
        }
        Some(visited_node)
    }

    // port: tsc/internal/transformers/estransforms/classfields.go:classFieldsTransformer.visitVariableDeclaration
    fn visit_variable_declaration(&self, v: &mut NodeVisitor<'_>, node: NodeId) -> Option<NodeId> {
        // 14.3.1.2 RS: Evaluation
        //   LexicalBinding : BindingIdentifier Initializer
        //     ...
        //     3. If IsAnonymousFunctionDefinition(|Initializer|) is *true*, then
        //        a. Let _value_ be ? NamedEvaluation of |Initializer| with argument _bindingId_.
        //     ...
        //
        // 14.3.2.1 RS: Evaluation
        //   VariableDeclaration : BindingIdentifier Initializer
        //     ...
        //     3. If IsAnonymousFunctionDefinition(|Initializer|) is *true*, then
        //        a. Let _value_ be ? NamedEvaluation of |Initializer| with argument _bindingId_.
        //     ...
        let mut node = node;
        if self.is_named_evaluation_needing_assigned_name(v.factory(), node) {
            node = self.transform_named_evaluation(v, node, false, b"");
        }
        self.visit_each_child(v, node)
    }

    // port: tsc/internal/transformers/estransforms/classfields.go:classFieldsTransformer.visitParameterDeclaration
    fn visit_parameter_declaration(&self, v: &mut NodeVisitor<'_>, node: NodeId) -> Option<NodeId> {
        // 8.6.3 RS: IteratorBindingInitialization
        //   SingleNameBinding : BindingIdentifier Initializer?
        //     ...
        //     5. If |Initializer| is present and _v_ is *undefined*, then
        //        a. If IsAnonymousFunctionDefinition(|Initializer|) is *true*, then
        //           i. Set _v_ to ? NamedEvaluation of |Initializer| with argument _bindingId_.
        //     ...
        //
        // 14.3.3.3 RS: KeyedBindingInitialization
        //   SingleNameBinding : BindingIdentifier Initializer?
        //     ...
        //     4. If |Initializer| is present and _v_ is *undefined*, then
        //        a. If IsAnonymousFunctionDefinition(|Initializer|) is *true*, then
        //           i. Set _v_ to ? NamedEvaluation of |Initializer| with argument _bindingId_.
        //     ...
        let mut node = node;
        if self.is_named_evaluation_needing_assigned_name(v.factory(), node) {
            node = self.transform_named_evaluation(v, node, false, b"");
        }
        self.visit_each_child(v, node)
    }

    // port: tsc/internal/transformers/estransforms/classfields.go:classFieldsTransformer.visitBindingElement
    fn visit_binding_element(&self, v: &mut NodeVisitor<'_>, node: NodeId) -> Option<NodeId> {
        // 8.6.3 RS: IteratorBindingInitialization
        //   SingleNameBinding : BindingIdentifier Initializer?
        //     ...
        //     5. If |Initializer| is present and _v_ is *undefined*, then
        //        a. If IsAnonymousFunctionDefinition(|Initializer|) is *true*, then
        //           i. Set _v_ to ? NamedEvaluation of |Initializer| with argument _bindingId_.
        //     ...
        //
        // 14.3.3.3 RS: KeyedBindingInitialization
        //   SingleNameBinding : BindingIdentifier Initializer?
        //     ...
        //     4. If |Initializer| is present and _v_ is *undefined*, then
        //        a. If IsAnonymousFunctionDefinition(|Initializer|) is *true*, then
        //           i. Set _v_ to ? NamedEvaluation of |Initializer| with argument _bindingId_.
        //     ...
        let mut node = node;
        if self.is_named_evaluation_needing_assigned_name(v.factory(), node) {
            node = self.transform_named_evaluation(v, node, false, b"");
        }
        self.visit_each_child(v, node)
    }

    // port: tsc/internal/transformers/estransforms/classfields.go:classFieldsTransformer.visitExportAssignment
    fn visit_export_assignment(&self, v: &mut NodeVisitor<'_>, node: NodeId) -> Option<NodeId> {
        // 16.2.3.7 RS: Evaluation
        //   ExportDeclaration : `export` `default` AssignmentExpression `;`
        //     1. If IsAnonymousFunctionDefinition(|AssignmentExpression|) is *true*, then
        //        a. Let _value_ be ? NamedEvaluation of |AssignmentExpression| with argument `"default"`.
        //     ...

        // NOTE: Since emit for `export =` translates to `module.exports = ...`, the assigned name of the class
        // is `""`.
        let mut node = node;
        if self.is_named_evaluation_needing_assigned_name(v.factory(), node) {
            let is_export_equals = v
                .node(node)
                .as_export_assignment()
                .expect("ExportAssignment payload")
                .is_export_equals();
            let assigned_name: &[u8] = if is_export_equals { b"" } else { b"default" };
            node = self.transform_named_evaluation(
                v,
                node,
                true, /*ignoreEmptyStringLiteral*/
                assigned_name,
            );
        }
        self.visit_each_child(v, node)
    }

    // port: tsc/internal/transformers/estransforms/classfields.go:classFieldsTransformer.injectPendingExpressions
    fn inject_pending_expressions(&self, v: &mut NodeVisitor<'_>, expression: NodeId) -> NodeId {
        let mut expression = expression;
        let pending = self.pending_expressions.borrow().clone();
        if !pending.is_empty() {
            let context = self.context.clone();
            let mut pending = pending;
            if kind_of(v, expression) == K::ParenthesizedExpression {
                pending.push(expression_of(v, expression));
                let f = v.factory_mut();
                let inlined = context.inline_expressions(f, &pending);
                expression = f.update_parenthesized_expression(expression, inlined);
            } else {
                pending.push(expression);
                expression = context
                    .inline_expressions(v.factory_mut(), &pending)
                    .expect(NIL);
            }
            self.pending_expressions.borrow_mut().clear();
        }
        expression
    }

    /// Computed property names are evaluated in the enclosing scope, not the
    /// current class: replaces Strada's `onEmitNode` for a
    /// ComputedPropertyName, which switches to `lexicalEnvironment?.previous`.
    // port: tsc/internal/transformers/estransforms/classfields.go:classFieldsTransformer.visitComputedPropertyName
    #[allow(clippy::unnecessary_wraps)] // a visit function; upstream's result is a nullable node
    fn visit_computed_property_name(
        &self,
        v: &mut NodeVisitor<'_>,
        node: NodeId,
    ) -> Option<NodeId> {
        let saved_lexical_environment = self.lexical_environment.borrow().clone();
        let saved_inside_computed_property_name = self.inside_computed_property_name.get();
        self.inside_computed_property_name.set(true);
        if let Some(previous) = saved_lexical_environment
            .as_ref()
            .and_then(|environment| environment.previous.clone())
        {
            *self.lexical_environment.borrow_mut() = Some(previous);
        }
        let expression = v.node(node).expression();
        let expression = self.visit_node(v, expression);
        *self.lexical_environment.borrow_mut() = saved_lexical_environment;
        self.inside_computed_property_name
            .set(saved_inside_computed_property_name);
        let injected = self.inject_pending_expressions(v, expression.expect(NIL));
        Some(
            v.factory_mut()
                .update_computed_property_name(node, Some(injected)),
        )
    }

    // port: tsc/internal/transformers/estransforms/classfields.go:classFieldsTransformer.visitConstructorDeclaration
    fn visit_constructor_declaration(
        &self,
        v: &mut NodeVisitor<'_>,
        node: NodeId,
    ) -> Option<NodeId> {
        if let Some(container) = self.current_class_container.get() {
            return self.transform_constructor(v, Some(node), container);
        }
        self.visit_each_child(v, node)
    }

    // port: tsc/internal/transformers/estransforms/classfields.go:classFieldsTransformer.shouldTransformClassElementToWeakMap
    fn should_transform_class_element_to_weak_map(
        &self,
        factory: &dyn RuntimeFactory,
        node: NodeId,
    ) -> bool {
        if self.should_transform_private_elements_or_class_static_blocks {
            return true;
        }
        self.should_always_transform_private_static_elements(factory, node)
    }

    // port: tsc/internal/transformers/estransforms/classfields.go:classFieldsTransformer.shouldAlwaysTransformPrivateStaticElements
    fn should_always_transform_private_static_elements(
        &self,
        factory: &dyn RuntimeFactory,
        node: NodeId,
    ) -> bool {
        self.has_static_modifier(factory, node)
            && self.context.emit_flags(node) & emit_flags::TRANSFORM_PRIVATE_STATIC_ELEMENTS != 0
    }

    /// The emit flag on a class node (not a member): unlike
    /// `shouldAlwaysTransformPrivateStaticElements`, no static modifier is
    /// required, since class nodes do not have one.
    // port: tsc/internal/transformers/estransforms/classfields.go:classFieldsTransformer.nodeHasTransformPrivateStaticElementsFlag
    fn node_has_transform_private_static_elements_flag(&self, node: NodeId) -> bool {
        self.context.emit_flags(node) & emit_flags::TRANSFORM_PRIVATE_STATIC_ELEMENTS != 0
    }

    // port: tsc/internal/transformers/estransforms/classfields.go:classFieldsTransformer.visitMethodOrAccessorDeclaration
    fn visit_method_or_accessor_declaration(
        &self,
        v: &mut NodeVisitor<'_>,
        node: NodeId,
    ) -> Option<NodeId> {
        let has_decorators = self.q(v.factory(), |view| {
            tsr_ast::utilities_middle::has_decorators(view, &view.node(node)?)
        });
        assert!(!has_decorators, "Debug failure. False expression.");

        if !self.is_private_identifier_class_element_declaration(v.factory(), node)
            || !self.should_transform_class_element_to_weak_map(v.factory(), node)
        {
            return self.with(v, Visitor::ClassElement, |c| c.visit_each_child(Some(node)));
        }

        // leave invalid code untransformed
        let info = self.access_private_identifier(v.factory(), name_of(v, node));
        let info = info.unwrap_or_else(|| {
            panic!("Debug failure. False expression: Undeclared private name for property declaration.")
        });
        if !info.is_valid {
            return Some(node);
        }

        let function_name = self.get_hoisted_function_name(v, node);
        if let Some(function_name) = function_name {
            let modifiers = self.extract_non_static_non_accessor_modifiers(v, node);
            let mut context = self.context.clone();
            context.start_variable_environment();
            let saved = self.in_iteration_statement.get();
            self.in_iteration_statement.set(false);
            let (body_node, parameter_list, asterisk_token) = {
                let read = v.node(node);
                let asterisk_token = read
                    .as_method_declaration()
                    .and_then(|method| method.asterisk_token());
                (read.body(), read.parameter_list(), asterisk_token)
            };
            let (body, params) = self.with(v, Visitor::Main, |m| {
                let body = context.visit_function_body(body_node, m);
                let params = m.visit_nodes(parameter_list);
                (body, params)
            });
            self.in_iteration_statement.set(saved);

            let f = v.factory_mut();
            let func_expr = f.new_function_expression(
                modifiers,
                asterisk_token,
                Some(function_name),
                None,
                params,
                None,
                None,
                body,
            );
            let assignment = context.new_assignment_expression(f, function_name, func_expr);
            self.add_pending_expressions(&[assignment]);
        }

        // remove method declaration from class
        None
    }

    // port: tsc/internal/transformers/estransforms/classfields.go:classFieldsTransformer.extractNonStaticNonAccessorModifiers
    fn extract_non_static_non_accessor_modifiers(
        &self,
        v: &mut NodeVisitor<'_>,
        node: NodeId,
    ) -> Option<NodeListId> {
        let modifiers = v.node(node).modifiers();
        extract_modifiers(
            &self.context,
            v.factory_mut(),
            modifiers,
            !(modifier_flags::STATIC | modifier_flags::ACCESSOR),
        )
    }

    // port: tsc/internal/transformers/estransforms/classfields.go:classFieldsTransformer.setCurrentClassElementAnd
    fn set_current_class_element_and(
        &self,
        class_element: Option<NodeId>,
        visitor: VisitFn<'a>,
        v: &mut NodeVisitor<'_>,
        node: NodeId,
    ) -> Option<NodeId> {
        if class_element != self.current_class_element.get() {
            let saved = self.current_class_element.get();
            self.current_class_element.set(class_element);
            let result = visitor(self, v, node);
            self.current_class_element.set(saved);
            return result;
        }
        visitor(self, v, node)
    }

    /// `Visitor.VisitEachChild`, as a callback.
    // port: tsc/internal/transformers/estransforms/classfields.go:classFieldsTransformer.visitEachChildOfNode
    fn visit_each_child_of_node(&self, v: &mut NodeVisitor<'_>, node: NodeId) -> Option<NodeId> {
        self.visit_each_child(v, node)
    }

    // port: tsc/internal/transformers/estransforms/classfields.go:classFieldsTransformer.setInIterationStatementAnd
    fn set_in_iteration_statement_and(
        &self,
        in_iteration: bool,
        visitor: VisitFn<'a>,
        v: &mut NodeVisitor<'_>,
        node: NodeId,
    ) -> Option<NodeId> {
        if self.in_iteration_statement.get() != in_iteration {
            let saved = self.in_iteration_statement.get();
            self.in_iteration_statement.set(in_iteration);
            let result = visitor(self, v, node);
            self.in_iteration_statement.set(saved);
            return result;
        }
        visitor(self, v, node)
    }

    // port: tsc/internal/transformers/estransforms/classfields.go:classFieldsTransformer.clearClassElementAndVisitEachChild
    #[allow(dead_code)] // upstream's unused helper
    fn clear_class_element_and_visit_each_child(
        &self,
        v: &mut NodeVisitor<'_>,
        node: NodeId,
    ) -> Option<NodeId> {
        self.set_current_class_element_and(None, Self::visit_each_child_of_node, v, node)
    }

    /// Lexical environment scoping for function expressions and
    /// declarations, as Strada's `onEmitNode`: a function expression whose
    /// original node is a static member of the current class keeps the
    /// current class element (so `this` in a descriptor method the ES
    /// decorator transformer synthesized becomes `_classThis`); any other
    /// function clears it.
    // port: tsc/internal/transformers/estransforms/classfields.go:classFieldsTransformer.visitFunctionExpressionOrDeclaration
    fn visit_function_expression_or_declaration(
        &self,
        v: &mut NodeVisitor<'_>,
        node: NodeId,
    ) -> Option<NodeId> {
        if self.current_class_element.get().is_some() {
            let original = self.context.most_original(node);
            if original != node {
                if let Some(container) = self.current_class_container.get() {
                    for member in members_of(v.factory(), container) {
                        if self.context.most_original(member) == original
                            && self.is_static(v.factory(), member)
                        {
                            // The function expression originates from a static class member (e.g., a
                            // descriptor method synthesized by the ES decorator transformer for a
                            // static private auto-accessor). Preserve the current class element so
                            // that visitThisExpression can substitute `this` with `_classThis`.
                            // Non-static members must NOT preserve the class element because `this`
                            // inside their descriptor functions should remain dynamic.
                            return self.visit_each_child_of_node(v, node);
                        }
                    }
                }
            }
        }
        self.set_current_class_element_and(None, Self::visit_each_child_of_node, v, node)
    }

    // port: tsc/internal/transformers/estransforms/classfields.go:classFieldsTransformer.setClassElementAndVisitEachChild
    fn set_class_element_and_visit_each_child(
        &self,
        v: &mut NodeVisitor<'_>,
        node: NodeId,
    ) -> Option<NodeId> {
        self.set_current_class_element_and(Some(node), Self::visit_each_child_of_node, v, node)
    }

    // port: tsc/internal/transformers/estransforms/classfields.go:classFieldsTransformer.getHoistedFunctionName
    fn get_hoisted_function_name(&self, v: &mut NodeVisitor<'_>, node: NodeId) -> Option<NodeId> {
        let name = v.node(node).name();
        assert!(
            name.is_some_and(|name| kind_of(v, name) == K::PrivateIdentifier),
            "Debug failure. False expression."
        );
        let info = self.access_private_identifier(v.factory(), name.expect(NIL));
        let info = info.unwrap_or_else(|| {
            panic!("Debug failure. False expression: Undeclared private name for property declaration.")
        });
        if info.kind == PrivateIdentifierKind::Method {
            return info.method_name;
        }
        if info.kind == PrivateIdentifierKind::Accessor {
            let kind = kind_of(v, node);
            if kind == K::GetAccessor {
                return info.getter_name;
            }
            if kind == K::SetAccessor {
                return info.setter_name;
            }
        }
        None
    }

    // port: tsc/internal/transformers/estransforms/classfields.go:classFieldsTransformer.tryGetClassThis
    fn try_get_class_this(&self, factory: &dyn RuntimeFactory) -> Option<NodeId> {
        if let Some(class_this) = self.try_get_class_this_no_container() {
            return Some(class_this);
        }
        if let Some(container) = self.current_class_container.get() {
            return factory.node(container).name();
        }
        None
    }

    // port: tsc/internal/transformers/estransforms/classfields.go:classFieldsTransformer.tryGetClassThisNoContainer
    fn try_get_class_this_no_container(&self) -> Option<NodeId> {
        let lex = self.get_class_lexical_environment();
        if lex.class_this.is_some() {
            return lex.class_this;
        }
        if lex.class_constructor.is_some() {
            return lex.class_constructor;
        }
        None
    }

    /// Transforms an auto-accessor property:
    ///
    /// ```text
    /// accessor x = 1;
    /// ```
    ///
    /// into:
    ///
    /// ```text
    /// #x = 1;
    /// get x() { return this.#x; }
    /// set x(value) { this.#x = value; }
    /// ```
    // port: tsc/internal/transformers/estransforms/classfields.go:classFieldsTransformer.transformAutoAccessor
    #[allow(clippy::unnecessary_wraps)] // a visit function; upstream's result is a nullable node
    fn transform_auto_accessor(&self, v: &mut NodeVisitor<'_>, node: NodeId) -> Option<NodeId> {
        let mut context = self.context.clone();
        let comment_range = context.comment_range_of(v.factory(), node);
        let source_map_range = context.source_map_range(v.factory(), node);

        // Since we're creating two declarations where there was previously one, cache
        // the expression for any computed property names.
        let name = name_of(v, node);
        let mut getter_name = name;
        let mut setter_name = name;
        if kind_of(v, name) == K::ComputedPropertyName
            && !is_simple_inlineable_expression(v.factory(), expression_of(v, name))
        {
            let cache_assignment =
                find_computed_property_name_cache_assignment(&self.context, v.factory(), name);
            if let Some(cache_assignment) = cache_assignment {
                let visited = self.visit_node(v, Some(expression_of(v, name)));
                let (left, _, _, _) = binary_parts(v, cache_assignment);
                let f = v.factory_mut();
                getter_name = f.update_computed_property_name(name, visited);
                setter_name = f.update_computed_property_name(name, Some(left));
            } else {
                let name_expression = expression_of(v, name);
                let name_expression_loc = loc_of(v, name_expression);
                let temp = context.new_temp_variable(v.factory_mut());
                context.set_source_map_range(temp, name_expression_loc);
                context.add_variable_declaration(v.factory_mut(), temp);
                let expression = self.visit_node(v, Some(name_expression)).expect(NIL);
                let f = v.factory_mut();
                let assignment = context.new_assignment_expression(f, temp, expression);
                context.set_source_map_range(assignment, name_expression_loc);
                getter_name = f.update_computed_property_name(name, Some(assignment));
                setter_name = f.update_computed_property_name(name, Some(temp));
            }
        }

        let (node_modifiers, initializer) = {
            let read = v.node(node);
            (read.modifiers(), read.initializer())
        };
        let modifiers = self.with(v, Visitor::Modifier, |mv| {
            mv.visit_modifiers(node_modifiers)
        });
        let backing_field = create_accessor_property_backing_field(
            &self.context,
            v.factory_mut(),
            node,
            modifiers,
            initializer,
        );
        context.set_original(backing_field, node);
        context.add_emit_flags(backing_field, emit_flags::NO_COMMENTS);
        context.set_source_map_range(backing_field, source_map_range);

        let receiver = if self.is_static(v.factory(), node) {
            match self.try_get_class_this(v.factory()) {
                Some(receiver) => receiver,
                None => context.new_this_expression(v.factory_mut()),
            }
        } else {
            context.new_this_expression(v.factory_mut())
        };

        let getter =
            self.create_accessor_property_get_redirector(v, node, modifiers, getter_name, receiver);
        context.set_original(getter, node);
        context.set_comment_range(getter, comment_range);
        context.set_source_map_range(getter, source_map_range);

        // create a fresh copy of the modifiers so that we don't duplicate comments
        let setter_modifiers = modifiers.map(|modifiers| {
            let f = v.factory_mut();
            let flags = f.read_list(modifiers).modifier_flags();
            let nodes =
                tsr_ast::utilities_middle::create_modifiers_from_modifier_flags(flags, |kind| {
                    Some(f.new_modifier(kind))
                });
            let nodes = f.alloc_nodes(nodes.unwrap_or_default());
            f.new_modifier_list(nodes)
        });
        let setter = self.create_accessor_property_set_redirector(
            v,
            node,
            setter_modifiers,
            setter_name,
            receiver,
        );
        context.set_original(setter, node);
        context.add_emit_flags(setter, emit_flags::NO_COMMENTS);
        context.set_source_map_range(setter, source_map_range);

        // Visit the results in a second pass
        let visited = self.visit_slice_with(
            v,
            Visitor::AccessorFieldResult,
            &[backing_field, getter, setter],
        );
        let f = v.factory_mut();
        let children = f.alloc_nodes(visited.into_iter().map(Some).collect());
        Some(f.new_syntax_list(children))
    }

    // port: tsc/internal/transformers/estransforms/classfields.go:classFieldsTransformer.transformPrivateFieldInitializer
    fn transform_private_field_initializer(
        &self,
        v: &mut NodeVisitor<'_>,
        node: NodeId,
    ) -> Option<NodeId> {
        if self.should_transform_class_element_to_weak_map(v.factory(), node) {
            // If we are transforming private elements into WeakMap/WeakSet, we should elide the node.
            let info = self.access_private_identifier(v.factory(), name_of(v, node));
            let info = info.unwrap_or_else(|| {
                panic!("Debug failure. False expression: Undeclared private name for property declaration.")
            });

            // Leave invalid code untransformed
            if !info.is_valid {
                return Some(node);
            }

            // If we encounter a valid private static field and we're not transforming
            // class static blocks, convert to a static block initializer.
            if info.is_static && !self.should_transform_private_elements_or_class_static_blocks {
                // TODO: fix
                let this = self.context.new_this_expression(v.factory_mut());
                let statement = self.transform_property_or_class_static_block(v, node, this);
                if let Some(statement) = statement {
                    let f = v.factory_mut();
                    let statements = new_node_list(f, vec![statement]);
                    let block = f.new_block(Some(statements), true /*multiLine*/);
                    return Some(
                        f.new_class_static_block_declaration(None /*modifiers*/, Some(block)),
                    );
                }
            }

            return None;
        }

        if self.should_transform_initializers_using_set
            && !self.has_static_modifier(v.factory(), node)
            && self.lex_data().is_some_and(|data| {
                data.facts & class_facts::WILL_HOIST_INITIALIZERS_TO_CONSTRUCTOR != 0
            })
        {
            let (modifiers, name) = {
                let read = v.node(node);
                (read.modifiers(), read.name())
            };
            let modifiers = self.with(v, Visitor::Main, |m| m.visit_modifiers(modifiers));
            return Some(v.factory_mut().update_property_declaration(
                node, modifiers, name, None, /*postfixToken*/
                None, /*typeNode*/
                None, /*initializer*/
            ));
        }

        let mut node = node;
        if self.is_named_evaluation_needing_assigned_name(v.factory(), node) {
            node = self.transform_named_evaluation(v, node, false, b"");
        }

        let (modifiers, name, initializer) = {
            let read = v.node(node);
            (read.modifiers(), read.name(), read.initializer())
        };
        let modifiers = self.with(v, Visitor::Modifier, |mv| mv.visit_modifiers(modifiers));
        let name = self.visit_property_name(v, name);
        let initializer = self.visit_node(v, initializer);
        Some(v.factory_mut().update_property_declaration(
            node,
            modifiers,
            name,
            None, /*postfixToken*/
            None, /*typeNode*/
            initializer,
        ))
    }

    // port: tsc/internal/transformers/estransforms/classfields.go:classFieldsTransformer.transformPublicFieldInitializer
    fn transform_public_field_initializer(
        &self,
        v: &mut NodeVisitor<'_>,
        node: NodeId,
    ) -> Option<NodeId> {
        if self.should_transform_initializers
            && !self.is_auto_accessor_property_declaration(v.factory(), node)
        {
            // Elide the property declaration; the initializer will be moved to the constructor.
            // For computed property names, we still need to emit the expression.
            let (name, has_initializer) = {
                let read = v.node(node);
                (read.name(), read.initializer().is_some())
            };
            let expr = self.get_property_name_expression_if_needed(
                v,
                name.expect(NIL),
                has_initializer || self.compiler_options.use_define_for_class_fields(),
            );
            if let Some(expr) = expr {
                flatten_comma_list(v.factory(), expr, &mut |e| {
                    self.add_pending_expressions(&[e]);
                    true
                });
            }

            // When target >= ES2022 (i.e., !shouldTransformPrivateElementsOrClassStaticBlocks) and we
            // still need to transform initializers (useDefineForClassFields: false), static property
            // initializers must be converted into `static { this.x = ...; }` blocks so that `this`
            // refers to the class constructor inside the static block.
            if self.is_static(v.factory(), node)
                && !self.should_transform_private_elements_or_class_static_blocks
            {
                let this = self.context.new_this_expression(v.factory_mut());
                let initializer_statement =
                    self.transform_property_or_class_static_block(v, node, this);
                if let Some(initializer_statement) = initializer_statement {
                    let loc = loc_of(v, node);
                    let f = v.factory_mut();
                    let statements = new_node_list(f, vec![initializer_statement]);
                    let block = f.new_block(Some(statements), false);
                    let static_block =
                        f.new_class_static_block_declaration(None /*modifiers*/, Some(block));

                    let mut context = self.context.clone();
                    context.set_original(static_block, node);
                    context.set_comment_range(static_block, loc);

                    context.add_emit_flags(initializer_statement, emit_flags::NO_COMMENTS);
                    return Some(static_block);
                }
            }

            return None;
        }

        let (modifiers, name, initializer) = {
            let read = v.node(node);
            (read.modifiers(), read.name(), read.initializer())
        };
        let modifiers = self.with(v, Visitor::Modifier, |mv| mv.visit_modifiers(modifiers));
        let name = self.visit_property_name(v, name);
        let initializer = self.visit_node(v, initializer);
        Some(v.factory_mut().update_property_declaration(
            node,
            modifiers,
            name,
            None, /*postfixToken*/
            None, /*typeNode*/
            initializer,
        ))
    }

    // port: tsc/internal/transformers/estransforms/classfields.go:classFieldsTransformer.transformFieldInitializer
    fn transform_field_initializer(&self, v: &mut NodeVisitor<'_>, node: NodeId) -> Option<NodeId> {
        let has_decorators = self.q(v.factory(), |view| {
            tsr_ast::utilities_middle::has_decorators(view, &view.node(node)?)
        });
        assert!(
            !has_decorators,
            "Debug failure. False expression: Decorators should already have been transformed and elided."
        );
        if self.is_private_identifier_class_element_declaration(v.factory(), node) {
            return self.transform_private_field_initializer(v, node);
        }
        self.transform_public_field_initializer(v, node)
    }

    // port: tsc/internal/transformers/estransforms/classfields.go:classFieldsTransformer.shouldTransformAutoAccessorsInCurrentClass
    fn should_transform_auto_accessors_in_current_class(&self) -> bool {
        if self.should_transform_auto_accessors {
            return true;
        }
        // When targeting ESNext with useDefineForClassFields: false, auto-accessors are only
        // transformed if the current class will hoist initializers to the constructor.
        self.lex_data().is_some_and(|data| {
            data.facts & class_facts::WILL_HOIST_INITIALIZERS_TO_CONSTRUCTOR != 0
        })
    }

    // port: tsc/internal/transformers/estransforms/classfields.go:classFieldsTransformer.visitPropertyDeclaration
    fn visit_property_declaration(&self, v: &mut NodeVisitor<'_>, node: NodeId) -> Option<NodeId> {
        // If this is an auto-accessor, we defer to `transformAutoAccessor`. That function
        // will in turn call `transformFieldInitializer` as needed.
        if self.is_auto_accessor_property_declaration(v.factory(), node)
            && (self.should_transform_auto_accessors_in_current_class()
                || self.has_static_modifier(v.factory(), node)
                    && self.should_always_transform_private_static_elements(v.factory(), node))
        {
            return self.transform_auto_accessor(v, node);
        }
        self.transform_field_initializer(v, node)
    }

    // port: tsc/internal/transformers/estransforms/classfields.go:classFieldsTransformer.createPrivateIdentifierAccess
    fn create_private_identifier_access(
        &self,
        v: &mut NodeVisitor<'_>,
        info: PrivateIdentifierInfo,
        receiver: NodeId,
    ) -> NodeId {
        let receiver = self.visit_node(v, Some(receiver)).expect(NIL);
        self.create_private_identifier_access_helper(v, info, receiver)
    }

    // port: tsc/internal/transformers/estransforms/classfields.go:classFieldsTransformer.createPrivateIdentifierAccessHelper
    fn create_private_identifier_access_helper(
        &self,
        v: &mut NodeVisitor<'_>,
        info: PrivateIdentifierInfo,
        receiver: NodeId,
    ) -> NodeId {
        let mut context = self.context.clone();
        let end = v.node(receiver).end();
        context.set_comment_range(receiver, TextRange::new(-1, i64::from(end)));

        let f = v.factory_mut();
        let state = || info.brand_check_identifier.expect(NIL);
        match info.kind {
            PrivateIdentifierKind::Accessor => context.new_class_private_field_get_helper(
                f,
                receiver,
                state(),
                info.kind,
                info.getter_name,
            ),
            PrivateIdentifierKind::Method => context.new_class_private_field_get_helper(
                f,
                receiver,
                state(),
                info.kind,
                info.method_name,
            ),
            PrivateIdentifierKind::Field => {
                let field = if info.is_static {
                    info.variable_name
                } else {
                    None
                };
                context.new_class_private_field_get_helper(f, receiver, state(), info.kind, field)
            }
            PrivateIdentifierKind::Untransformed => panic!(
                "Debug failure. Access helpers should not be created for untransformed private elements"
            ),
        }
    }

    // port: tsc/internal/transformers/estransforms/classfields.go:classFieldsTransformer.visitPropertyAccessExpression
    fn visit_property_access_expression(
        &self,
        v: &mut NodeVisitor<'_>,
        node: NodeId,
    ) -> Option<NodeId> {
        let (expression, name) = {
            let read = v.node(node);
            (read.expression().expect(NIL), read.name().expect(NIL))
        };
        let name_kind = kind_of(v, name);
        if name_kind == K::PrivateIdentifier {
            let info = self.access_private_identifier(v.factory(), name);
            if let Some(info) = info {
                let result = self.create_private_identifier_access(v, info, expression);
                self.context.clone().set_original(result, node);
                let loc = loc_of(v, node);
                v.factory_mut().set_node_range(result, loc);
                return Some(result);
            }
        }
        let super_property = self.current_class_element.get().is_some()
            && is_super_property(v.factory(), node)
            && name_kind == K::Identifier;
        if let Some(data) = self.static_super_data(v.factory(), super_property) {
            if data.facts & class_facts::CLASS_WAS_DECORATED != 0 {
                return Some(self.visit_invalid_super_property(v, node));
            }
            if let (Some(class_constructor), Some(super_class_reference)) =
                (data.class_constructor, data.super_class_reference)
            {
                // converts `super.x` into `Reflect.get(_baseTemp, "x", _classTemp)`
                let mut context = self.context.clone();
                let f = v.factory_mut();
                let property_key = context.new_string_literal_from_node(f, name);
                let super_property = context.new_reflect_get_call(
                    f,
                    super_class_reference,
                    property_key,
                    class_constructor,
                );
                context.set_original(super_property, expression);
                let loc = loc_of(f, expression);
                f.set_node_range(super_property, loc);
                return Some(super_property);
            }
        }
        // Visit only the expression, not the name (when it's a regular identifier), to prevent
        // substitution of property names. Strada's onSubstituteNode only fires for
        // EmitHint.Expression, which excludes the .name of PropertyAccessExpression.
        // Private identifier names are still visited through VisitEachChild so they can be
        // transformed by visitPrivateIdentifier.
        if name_kind == K::Identifier {
            return self.visit_property_access_expression_for_substitution(v, node);
        }
        self.visit_each_child(v, node)
    }

    /// Visits only the expression of a property access, leaving the name
    /// unchanged, so the name is never substituted with a class alias.
    // port: tsc/internal/transformers/estransforms/classfields.go:classFieldsTransformer.visitPropertyAccessExpressionForSubstitution
    #[allow(clippy::unnecessary_wraps)] // a visit function; upstream's result is a nullable node
    fn visit_property_access_expression_for_substitution(
        &self,
        v: &mut NodeVisitor<'_>,
        node: NodeId,
    ) -> Option<NodeId> {
        let (expression, question_dot_token, name, flags) = {
            let read = v.node(node);
            let data = read
                .as_property_access_expression()
                .expect("PropertyAccessExpression payload");
            (
                data.expression(),
                data.question_dot_token(),
                data.name(),
                read.flags(),
            )
        };
        let visited = self.visit_node(v, expression);
        if visited != expression {
            return Some(v.factory_mut().update_property_access_expression(
                node,
                visited,
                question_dot_token,
                name,
                flags,
            ));
        }
        Some(node)
    }

    // port: tsc/internal/transformers/estransforms/classfields.go:classFieldsTransformer.visitElementAccessExpression
    fn visit_element_access_expression(
        &self,
        v: &mut NodeVisitor<'_>,
        node: NodeId,
    ) -> Option<NodeId> {
        let super_property =
            self.current_class_element.get().is_some() && is_super_property(v.factory(), node);
        if let Some(data) = self.static_super_data(v.factory(), super_property) {
            if data.facts & class_facts::CLASS_WAS_DECORATED != 0 {
                return Some(self.visit_invalid_super_property(v, node));
            }
            if let (Some(class_constructor), Some(super_class_reference)) =
                (data.class_constructor, data.super_class_reference)
            {
                let (expression, argument) = {
                    let read = v.node(node);
                    let data = read
                        .as_element_access_expression()
                        .expect("ElementAccessExpression payload");
                    (data.expression().expect(NIL), data.argument_expression())
                };
                // converts `super[x]` into `Reflect.get(_baseTemp, x, _classTemp)`
                let argument = self.visit_node(v, argument).expect(NIL);
                let mut context = self.context.clone();
                let f = v.factory_mut();
                let super_property = context.new_reflect_get_call(
                    f,
                    super_class_reference,
                    argument,
                    class_constructor,
                );
                context.set_original(super_property, expression);
                let loc = loc_of(f, expression);
                f.set_node_range(super_property, loc);
                return Some(super_property);
            }
        }
        self.visit_each_child(v, node)
    }

    /// The operator and operand of a prefix or postfix unary expression.
    fn unary_parts(factory: &dyn RuntimeFactory, node: NodeId) -> (NodeKind, NodeId, bool) {
        let read = factory.node(node);
        if let Some(prefix) = read.as_prefix_unary_expression() {
            return (prefix.operator(), prefix.operand().expect(NIL), true);
        }
        let postfix = read
            .as_postfix_unary_expression()
            .expect("PostfixUnaryExpression payload");
        (postfix.operator(), postfix.operand().expect(NIL), false)
    }

    // port: tsc/internal/transformers/estransforms/classfields.go:classFieldsTransformer.visitPreOrPostfixUnaryExpression
    fn visit_pre_or_postfix_unary_expression(
        &self,
        v: &mut NodeVisitor<'_>,
        node: NodeId,
        discarded: bool,
    ) -> Option<NodeId> {
        let (operator, operand, is_prefix) = Self::unary_parts(v.factory(), node);

        if operator == K::PlusPlusToken || operator == K::MinusMinusToken {
            let operand_skipped = self.skip_parentheses(v.factory(), operand);
            let skipped_kind = kind_of(v, operand_skipped);

            // Private identifier property access
            if skipped_kind == K::PropertyAccessExpression
                && kind_of(v, name_of(v, operand_skipped)) == K::PrivateIdentifier
            {
                let info = self.access_private_identifier(v.factory(), name_of(v, operand_skipped));
                if let Some(info) = info {
                    let receiver = self
                        .visit_node(v, Some(expression_of(v, operand_skipped)))
                        .expect(NIL);
                    let (read_expression, initialize_expression) =
                        self.create_copiable_receiver_expr(v, receiver);

                    let mut expression =
                        self.create_private_identifier_access_helper(v, info, read_expression);
                    let mut context = self.context.clone();
                    let temp = if !is_prefix && !discarded {
                        let temp = context.new_temp_variable(v.factory_mut());
                        context.add_variable_declaration(v.factory_mut(), temp);
                        Some(temp)
                    } else {
                        None
                    };
                    expression = expand_pre_or_postfix_increment_or_decrement_expression(
                        v.factory_mut(),
                        &self.context,
                        node,
                        expression,
                        temp,
                    );
                    let assign_receiver = initialize_expression.unwrap_or(read_expression);
                    expression = self.create_private_identifier_assignment(
                        v,
                        info,
                        assign_receiver,
                        expression,
                        K::EqualsToken.into(),
                    );
                    context.set_original(expression, node);
                    let loc = loc_of(v, node);
                    v.factory_mut().set_node_range(expression, loc);
                    if let Some(temp) = temp {
                        let f = v.factory_mut();
                        expression = context.new_comma_expression(f, expression, temp);
                        f.set_node_range(expression, loc);
                    }
                    return Some(expression);
                }
            } else {
                let super_property = self.current_class_element.get().is_some()
                    && is_super_property(v.factory(), operand_skipped);
                if let Some(data) = self.static_super_data(v.factory(), super_property) {
                    // converts `++super.a` into `(Reflect.set(_baseTemp, "a", (_a = Reflect.get(_baseTemp, "a", _classTemp), _b = ++_a), _classTemp), _b)`
                    // converts `++super[f()]` into `(Reflect.set(_baseTemp, _a = f(), (_b = Reflect.get(_baseTemp, _a, _classTemp), _c = ++_b), _classTemp), _c)`
                    // converts `--super.a` into `(Reflect.set(_baseTemp, "a", (_a = Reflect.get(_baseTemp, "a", _classTemp), _b = --_a), _classTemp), _b)`
                    // converts `--super[f()]` into `(Reflect.set(_baseTemp, _a = f(), (_b = Reflect.get(_baseTemp, _a, _classTemp), _c = --_b), _classTemp), _c)`
                    // converts `super.a++` into `(Reflect.set(_baseTemp, "a", (_a = Reflect.get(_baseTemp, "a", _classTemp), _b = _a++), _classTemp), _b)`
                    // converts `super[f()]++` into `(Reflect.set(_baseTemp, _a = f(), (_b = Reflect.get(_baseTemp, _a, _classTemp), _c = _b++), _classTemp), _c)`
                    // converts `super.a--` into `(Reflect.set(_baseTemp, "a", (_a = Reflect.get(_baseTemp, "a", _classTemp), _b = _a--), _classTemp), _b)`
                    // converts `super[f()]--` into `(Reflect.set(_baseTemp, _a = f(), (_b = Reflect.get(_baseTemp, _a, _classTemp), _c = _b--), _classTemp), _c)`
                    if data.facts & class_facts::CLASS_WAS_DECORATED != 0 {
                        let visited_expr = self.visit_invalid_super_property(v, operand_skipped);
                        let f = v.factory_mut();
                        if is_prefix {
                            return Some(f.update_prefix_unary_expression(
                                node,
                                operator,
                                Some(visited_expr),
                            ));
                        }
                        return Some(f.update_postfix_unary_expression(
                            node,
                            Some(visited_expr),
                            operator,
                        ));
                    }
                    if let (Some(class_constructor), Some(super_class_reference)) =
                        (data.class_constructor, data.super_class_reference)
                    {
                        let mut context = self.context.clone();
                        let mut setter_name = None;
                        let mut getter_name = None;
                        if skipped_kind == K::PropertyAccessExpression {
                            let name = name_of(v, operand_skipped);
                            if kind_of(v, name) == K::Identifier {
                                let literal =
                                    context.new_string_literal_from_node(v.factory_mut(), name);
                                getter_name = Some(literal);
                                setter_name = getter_name;
                            }
                        } else if skipped_kind == K::ElementAccessExpression {
                            let argument = v
                                .node(operand_skipped)
                                .as_element_access_expression()
                                .expect("ElementAccessExpression payload")
                                .argument_expression()
                                .expect(NIL);
                            if is_simple_inlineable_expression(v.factory(), argument) {
                                getter_name = Some(argument);
                                setter_name = getter_name;
                            } else {
                                let temp = context.new_temp_variable(v.factory_mut());
                                getter_name = Some(temp);
                                context.add_variable_declaration(v.factory_mut(), temp);
                                let visited = self.visit_node(v, Some(argument)).expect(NIL);
                                setter_name = Some(context.new_assignment_expression(
                                    v.factory_mut(),
                                    temp,
                                    visited,
                                ));
                            }
                        }
                        if let (Some(setter_name), Some(getter_name)) = (setter_name, getter_name) {
                            let f = v.factory_mut();
                            let mut expression = context.new_reflect_get_call(
                                f,
                                super_class_reference,
                                getter_name,
                                class_constructor,
                            );
                            let loc = loc_of(f, operand_skipped);
                            f.set_node_range(expression, loc);

                            let temp = if discarded {
                                None
                            } else {
                                let temp = context.new_temp_variable(f);
                                context.add_variable_declaration(f, temp);
                                Some(temp)
                            };
                            expression = expand_pre_or_postfix_increment_or_decrement_expression(
                                f,
                                &self.context,
                                node,
                                expression,
                                temp,
                            );
                            expression = context.new_reflect_set_call(
                                f,
                                super_class_reference,
                                setter_name,
                                expression,
                                class_constructor,
                            );
                            context.set_original(expression, node);
                            let loc = loc_of(f, node);
                            f.set_node_range(expression, loc);
                            if let Some(temp) = temp {
                                expression = context.new_comma_expression(f, expression, temp);
                                f.set_node_range(expression, loc);
                            }
                            return Some(expression);
                        }
                    }
                }
            }
        }
        self.visit_each_child(v, node)
    }

    // port: tsc/internal/transformers/estransforms/classfields.go:classFieldsTransformer.visitForStatement
    #[allow(clippy::unnecessary_wraps)] // a visit function; upstream's result is a nullable node
    fn visit_for_statement(&self, v: &mut NodeVisitor<'_>, node: NodeId) -> Option<NodeId> {
        let (initializer, condition, incrementor, statement) = {
            let read = v.node(node);
            let data = read.as_for_statement().expect("ForStatement payload");
            (
                data.initializer(),
                data.condition(),
                data.incrementor(),
                data.statement(),
            )
        };
        let initializer = self.with(v, Visitor::DiscardedValue, |d| d.visit_node(initializer));
        let condition = self.visit_node(v, condition);
        let incrementor = self.with(v, Visitor::DiscardedValue, |d| d.visit_node(incrementor));
        let saved = self.in_iteration_statement.get();
        self.in_iteration_statement.set(true);
        let mut context = self.context.clone();
        let body = self.with(v, Visitor::Main, |m| {
            context.visit_iteration_body(statement, m)
        });
        self.in_iteration_statement.set(saved);
        Some(
            v.factory_mut()
                .update_for_statement(node, initializer, condition, incrementor, body),
        )
    }

    // port: tsc/internal/transformers/estransforms/classfields.go:classFieldsTransformer.visitExpressionStatement
    #[allow(clippy::unnecessary_wraps)] // a visit function; upstream's result is a nullable node
    fn visit_expression_statement(&self, v: &mut NodeVisitor<'_>, node: NodeId) -> Option<NodeId> {
        // Preserve private identifiers that appear directly as the expression of an
        // ExpressionStatement (e.g., `#;`). This is error-recovery output from the parser
        // for invalid syntax. Keeping it ensures the runtime throws a SyntaxError rather
        // than silently succeeding with an empty statement.
        let expression = v.node(node).expression();
        if kind_of(v, expression.expect(NIL)) == K::PrivateIdentifier
            && self.should_transform_private_elements_or_class_static_blocks
        {
            return Some(node);
        }
        let visited = self.with(v, Visitor::DiscardedValue, |d| d.visit_node(expression));
        Some(v.factory_mut().update_expression_statement(node, visited))
    }

    /// The read expression of `receiver` and, when it is not simple, the
    /// assignment that initializes a temp to it.
    // port: tsc/internal/transformers/estransforms/classfields.go:classFieldsTransformer.createCopiableReceiverExpr
    fn create_copiable_receiver_expr(
        &self,
        v: &mut NodeVisitor<'_>,
        receiver: NodeId,
    ) -> (NodeId, Option<NodeId>) {
        let mut clone = receiver;
        if !tsr_ast::utilities::node_is_synthesized(&v.node(receiver)) {
            clone = clone_node(v.factory_mut(), receiver);
        }
        if is_simple_inlineable_expression(v.factory(), receiver) {
            return (clone, None);
        }
        let mut context = self.context.clone();
        let f = v.factory_mut();
        let read_expression = context.new_temp_variable(f);
        context.add_variable_declaration(f, read_expression);
        let initialize_expression = context.new_assignment_expression(f, read_expression, clone);
        (read_expression, Some(initialize_expression))
    }

    // port: tsc/internal/transformers/estransforms/classfields.go:classFieldsTransformer.visitCallExpression
    fn visit_call_expression(&self, v: &mut NodeVisitor<'_>, node: NodeId) -> Option<NodeId> {
        let (expression, question_dot_token, arguments, flags) = {
            let read = v.node(node);
            let data = read.as_call_expression().expect("CallExpression payload");
            (
                data.expression().expect(NIL),
                data.question_dot_token(),
                data.arguments(),
                read.flags(),
            )
        };
        if kind_of(v, expression) == K::PropertyAccessExpression
            && kind_of(v, name_of(v, expression)) == K::PrivateIdentifier
            && self
                .access_private_identifier(v.factory(), name_of(v, expression))
                .is_some()
        {
            // obj.#x()

            // Transform call expressions of private names to properly bind the `this` parameter.
            let (this_arg, target) = self.create_call_binding(v, expression);
            let visited_target = self.visit_node(v, Some(target));
            let visited_this_arg = self.visit_node(v, Some(this_arg)).expect(NIL);
            let visited_args = self.visit_nodes(v, arguments);
            let visited_args = list_nodes(v.factory(), visited_args);
            let mut all_args = Vec::with_capacity(1 + visited_args.len());
            all_args.push(visited_this_arg);
            all_args.extend(visited_args);
            let f = v.factory_mut();
            if flags & node_flags::OPTIONAL_CHAIN != 0 {
                let call = f.new_identifier(JsString::from_bytes(&b"call"[..]));
                let callee = f.new_property_access_expression(
                    visited_target,
                    question_dot_token,
                    Some(call),
                    node_flags::OPTIONAL_CHAIN,
                );
                let arguments = new_node_list(f, all_args);
                return Some(f.update_call_expression(
                    node,
                    Some(callee),
                    None, /*questionDotToken*/
                    None, /*typeArguments*/
                    Some(arguments),
                    flags,
                ));
            }
            let call = f.new_identifier(JsString::from_bytes(&b"call"[..]));
            let callee = f.new_property_access_expression(
                visited_target,
                None,
                Some(call),
                node_flags::NONE,
            );
            let arguments = new_node_list(f, all_args);
            return Some(f.update_call_expression(
                node,
                Some(callee),
                None, /*questionDotToken*/
                None, /*typeArguments*/
                Some(arguments),
                flags,
            ));
        }

        let super_property = self.current_class_element.get().is_some()
            && is_super_property(v.factory(), expression);
        if let Some(class_constructor) = self
            .static_super_data(v.factory(), super_property)
            .and_then(|data| data.class_constructor)
        {
            // super.x()
            // super[x]()

            // converts `super.f(...)` into `Reflect.get(_baseTemp, "f", _classTemp).call(_classTemp, ...)`
            let target = self.visit_node(v, Some(expression)).expect(NIL);
            let visited_args = self.visit_nodes(v, arguments);
            let visited_args = list_nodes(v.factory(), visited_args);
            let mut context = self.context.clone();
            let f = v.factory_mut();
            let invocation =
                context.new_function_call_call(f, target, Some(class_constructor), visited_args);
            context.set_original(invocation, node);
            let loc = loc_of(f, node);
            f.set_node_range(invocation, loc);
            return Some(invocation);
        }

        self.visit_each_child(v, node)
    }

    // port: tsc/internal/transformers/estransforms/classfields.go:classFieldsTransformer.visitTaggedTemplateExpression
    fn visit_tagged_template_expression(
        &self,
        v: &mut NodeVisitor<'_>,
        node: NodeId,
    ) -> Option<NodeId> {
        let (tag, template, flags) = {
            let read = v.node(node);
            let data = read
                .as_tagged_template_expression()
                .expect("TaggedTemplateExpression payload");
            (data.tag().expect(NIL), data.template(), read.flags())
        };
        if kind_of(v, tag) == K::PropertyAccessExpression
            && kind_of(v, name_of(v, tag)) == K::PrivateIdentifier
            && self
                .access_private_identifier(v.factory(), name_of(v, tag))
                .is_some()
        {
            // Bind the `this` correctly for tagged template literals when the tag is a private identifier property access.
            let (this_arg, target) = self.create_call_binding(v, tag);
            let visited_target = self.visit_node(v, Some(target));
            let bind = v
                .factory_mut()
                .new_identifier(JsString::from_bytes(&b"bind"[..]));
            let callee = v.factory_mut().new_property_access_expression(
                visited_target,
                None,
                Some(bind),
                node_flags::NONE,
            );
            let visited_this_arg = self.visit_node(v, Some(this_arg)).expect(NIL);
            let f = v.factory_mut();
            let arguments = new_node_list(f, vec![visited_this_arg]);
            let bind_expr = f.new_call_expression(
                Some(callee),
                None, /*questionDotToken*/
                None, /*typeArguments*/
                Some(arguments),
                node_flags::NONE,
            );
            let template = self.visit_node(v, template);
            return Some(v.factory_mut().update_tagged_template_expression(
                node,
                Some(bind_expr),
                None, /*questionDotToken*/
                None, /*typeArguments*/
                template,
                flags,
            ));
        }

        let super_property =
            self.current_class_element.get().is_some() && is_super_property(v.factory(), tag);
        if let Some(class_constructor) = self
            .static_super_data(v.factory(), super_property)
            .and_then(|data| data.class_constructor)
        {
            // converts `` super.f`x` `` into `` Reflect.get(_baseTemp, "f", _classTemp).bind(_classTemp)`x` ``
            let target = self.visit_node(v, Some(tag)).expect(NIL);
            let mut context = self.context.clone();
            let f = v.factory_mut();
            let invocation =
                context.new_function_bind_call(f, target, class_constructor, Vec::new());
            context.set_original(invocation, node);
            let loc = loc_of(f, node);
            f.set_node_range(invocation, loc);
            let template = self.visit_node(v, template);
            return Some(v.factory_mut().update_tagged_template_expression(
                node,
                Some(invocation),
                None, /*questionDotToken*/
                None, /*typeArguments*/
                template,
                flags,
            ));
        }

        self.visit_each_child(v, node)
    }

    // port: tsc/internal/transformers/estransforms/classfields.go:classFieldsTransformer.transformClassStaticBlockDeclaration
    fn transform_class_static_block_declaration(
        &self,
        v: &mut NodeVisitor<'_>,
        node: NodeId,
    ) -> Option<NodeId> {
        if self.should_transform_private_elements_or_class_static_blocks {
            if is_class_this_assignment_block(&self.context, v.factory(), node) {
                let (_, statements) = static_block_statements(v.factory(), node);
                let first = expression_of(v, statements[0]);
                let result = self.visit_node(v, Some(first)).expect(NIL);
                // If the generated `_classThis` assignment is a noop (i.e., `_classThis = _classThis`), we can
                // eliminate the expression
                if is_assignment_expression(
                    v.factory(),
                    result,
                    true, /*excludeCompoundAssignment*/
                ) {
                    let (left, _, _, right) = binary_parts(v, result);
                    if left == right {
                        return None;
                    }
                }
                return Some(result);
            }

            if is_class_named_evaluation_helper_block(&self.context, v.factory(), node) {
                let (_, statements) = static_block_statements(v.factory(), node);
                let first = expression_of(v, statements[0]);
                return self.visit_node(v, Some(first));
            }

            let mut context = self.context.clone();
            context.start_variable_environment();
            let (statement_list, statements) = static_block_statements(v.factory(), node);
            let statements =
                self.set_current_class_element_and_visit_statements(v, node, &statements);
            let f = v.factory_mut();
            let statements = context.end_and_merge_variable_environment(f, statements);

            let iife = context.new_immediately_invoked_arrow_function(f, statements);
            let callee = f.node(iife).expression().expect(NIL);
            let arrow_function = self.skip_parentheses(v.factory(), callee);
            context.set_original(arrow_function, node);
            context.add_emit_flags(arrow_function, emit_flags::NO_LEXICAL_ARGUMENTS);
            // Preserve the statement list source range so the printer can emit detached comments
            // (e.g., `// do` inside an otherwise empty static block)
            let f = v.factory_mut();
            let arrow_body = f.node(arrow_function).body().expect(NIL);
            let arrow_statements = f.node(arrow_body).statement_list().expect(NIL);
            let loc = f.read_list(statement_list).loc();
            f.set_list_location(arrow_statements, loc);
            context.set_original(iife, node);
            context.assign_source_map_range(f, iife, node);
            context.add_emit_flags(arrow_function, emit_flags::NO_LEXICAL_THIS);
            return Some(iife);
        }
        None
    }

    // port: tsc/internal/transformers/estransforms/classfields.go:classFieldsTransformer.setCurrentClassElementAndVisitStatements
    fn set_current_class_element_and_visit_statements(
        &self,
        v: &mut NodeVisitor<'_>,
        class_element: NodeId,
        statements: &[NodeId],
    ) -> Vec<NodeId> {
        let saved_current_class_element = self.current_class_element.get();
        self.current_class_element.set(Some(class_element));
        let result = self.visit_slice_with(v, Visitor::Main, statements);
        self.current_class_element.set(saved_current_class_element);
        result
    }

    // port: tsc/internal/transformers/estransforms/classfields.go:classFieldsTransformer.isAnonymousClassNeedingAssignedNameWorker
    fn is_anonymous_class_needing_assigned_name_worker(
        &self,
        factory: &dyn RuntimeFactory,
        node: NodeId,
    ) -> bool {
        if kind_of(factory, node) == K::ClassExpression && factory.node(node).name().is_none() {
            let static_properties_or_class_static_blocks =
                self.get_static_properties_and_class_static_block(factory, node);
            if static_properties_or_class_static_blocks
                .iter()
                .any(|&n| is_class_named_evaluation_helper_block(&self.context, factory, n))
            {
                return false;
            }
            let has_transformable_statics = (self
                .should_transform_private_elements_or_class_static_blocks
                || self.node_has_transform_private_static_elements_flag(node))
                && static_properties_or_class_static_blocks
                    .iter()
                    .any(|&n| self.is_transformable_static(factory, n));
            return has_transformable_statics;
        }
        false
    }

    /// The `core.Some` predicate of upstream's transformable statics: a static
    /// block, a private element, or an initialized property when
    /// initializers are transformed.
    fn is_transformable_static(&self, factory: &dyn RuntimeFactory, node: NodeId) -> bool {
        kind_of(factory, node) == K::ClassStaticBlockDeclaration
            || self.is_private_identifier_class_element_declaration(factory, node)
            || self.should_transform_initializers
                && tsr_ast::utilities_middle::is_initialized_property(&factory.node(node))
    }

    // port: tsc/internal/transformers/estransforms/classfields.go:classFieldsTransformer.visitBinaryExpression
    fn visit_binary_expression(
        &self,
        v: &mut NodeVisitor<'_>,
        node: NodeId,
        discarded: bool,
    ) -> Option<NodeId> {
        if self.q(v.factory(), |view| {
            tsr_ast::is_destructuring_assignment(view, node)
        }) {
            // ({ x: obj.#x } = ...)
            // ({ x: super.x } = ...)
            // ({ x: super[x] } = ...)
            let saved_pending_expressions = self.pending_expressions.take();
            let (left, _, operator_token, right) = binary_parts(v, node);
            let left = self.with(v, Visitor::AssignmentTarget, |a| a.visit_node(Some(left)));
            let right = self.visit_node(v, Some(right));
            let f = v.factory_mut();
            let updated =
                f.update_binary_expression(node, None, left, None, Some(operator_token), right);
            let pending = self.pending_expressions.take();
            let result = if pending.is_empty() {
                updated
            } else {
                let mut exprs = pending;
                exprs.push(updated);
                self.context.inline_expressions(f, &exprs).expect(NIL)
            };
            *self.pending_expressions.borrow_mut() = saved_pending_expressions;
            return Some(result);
        }

        let mut node = node;
        if is_assignment_expression(v.factory(), node, false /*excludeCompound*/) {
            // 13.15.2 RS: Evaluation
            //   AssignmentExpression : LeftHandSideExpression `=` AssignmentExpression
            //     1. If |LeftHandSideExpression| is neither an |ObjectLiteral| nor an |ArrayLiteral|, then
            //        a. Let _lref_ be ? Evaluation of |LeftHandSideExpression|.
            //        b. If IsAnonymousFunctionDefinition(|AssignmentExpression|) and IsIdentifierRef of |LeftHandSideExpression| are both *true*, then
            //           i. Let _rval_ be ? NamedEvaluation of |AssignmentExpression| with argument _lref_.[[ReferencedName]].
            //     ...
            //
            //   AssignmentExpression : LeftHandSideExpression `&&=` AssignmentExpression
            //     ...
            //     5. If IsAnonymousFunctionDefinition(|AssignmentExpression|) is *true* and IsIdentifierRef of |LeftHandSideExpression| is *true*, then
            //        a. Let _rval_ be ? NamedEvaluation of |AssignmentExpression| with argument _lref_.[[ReferencedName]].
            //     ...
            //
            //   AssignmentExpression : LeftHandSideExpression `||=` AssignmentExpression
            //     ...
            //     5. If IsAnonymousFunctionDefinition(|AssignmentExpression|) is *true* and IsIdentifierRef of |LeftHandSideExpression| is *true*, then
            //        a. Let _rval_ be ? NamedEvaluation of |AssignmentExpression| with argument _lref_.[[ReferencedName]].
            //     ...
            //
            //   AssignmentExpression : LeftHandSideExpression `??=` AssignmentExpression
            //     ...
            //     4. If IsAnonymousFunctionDefinition(|AssignmentExpression|) is *true* and IsIdentifierRef of |LeftHandSideExpression| is *true*, then
            //        a. Let _rval_ be ? NamedEvaluation of |AssignmentExpression| with argument _lref_.[[ReferencedName]].
            //     ...
            if self.is_named_evaluation_needing_assigned_name(v.factory(), node) {
                node = self.transform_named_evaluation(v, node, false, b"");
                assert!(
                    is_assignment_expression(v.factory(), node, false),
                    "Debug failure. False expression."
                );
            }

            let (node_left, operator, operator_token, node_right) = binary_parts(v, node);
            let left = self.q_node(v.factory(), node_left, |view| {
                tsr_ast::utilities::skip_outer_expressions(
                    view,
                    node_left,
                    outer_expression_kinds::PARTIALLY_EMITTED_EXPRESSIONS
                        | outer_expression_kinds::PARENTHESES,
                )
            });
            if kind_of(v, left) == K::PropertyAccessExpression
                && kind_of(v, name_of(v, left)) == K::PrivateIdentifier
            {
                // obj.#x = ...
                let info = self.access_private_identifier(v.factory(), name_of(v, left));
                if let Some(info) = info {
                    let receiver = expression_of(v, left);
                    let result = self.create_private_identifier_assignment(
                        v, info, receiver, node_right, operator,
                    );
                    self.context.clone().set_original(result, node);
                    let loc = loc_of(v, node);
                    v.factory_mut().set_node_range(result, loc);
                    return Some(result);
                }
            } else {
                let super_property = self.current_class_element.get().is_some()
                    && is_super_property(v.factory(), node_left);
                if let Some(data) = self.static_super_data(v.factory(), super_property) {
                    // super.x = ...
                    // super[x] = ...
                    // super.x += ...
                    // super.x -= ...
                    if data.facts & class_facts::CLASS_WAS_DECORATED != 0 {
                        let left = self.visit_invalid_super_property(v, node_left);
                        let right = self.visit_node(v, Some(node_right));
                        return Some(v.factory_mut().update_binary_expression(
                            node,
                            None,
                            Some(left),
                            None,
                            Some(operator_token),
                            right,
                        ));
                    }
                    if let (Some(class_constructor), Some(super_class_reference)) =
                        (data.class_constructor, data.super_class_reference)
                    {
                        let left_kind = kind_of(v, node_left);
                        let mut setter_name = if left_kind == K::ElementAccessExpression {
                            let argument = v
                                .node(node_left)
                                .as_element_access_expression()
                                .expect("ElementAccessExpression payload")
                                .argument_expression();
                            self.visit_node(v, argument)
                        } else if left_kind == K::PropertyAccessExpression
                            && kind_of(v, name_of(v, node_left)) == K::Identifier
                        {
                            let name = name_of(v, node_left);
                            Some(
                                self.context
                                    .clone()
                                    .new_string_literal_from_node(v.factory_mut(), name),
                            )
                        } else {
                            None
                        };
                        if let Some(mut setter) = setter_name.take() {
                            // converts `super.x = 1` into `(Reflect.set(_baseTemp, "x", _a = 1, _classTemp), _a)`
                            // converts `super[f()] = 1` into `(Reflect.set(_baseTemp, f(), _a = 1, _classTemp), _a)`
                            // converts `super.x += 1` into `(Reflect.set(_baseTemp, "x", _a = Reflect.get(_baseTemp, "x", _classtemp) + 1, _classTemp), _a)`
                            // converts `super[f()] += 1` into `(Reflect.set(_baseTemp, _a = f(), _b = Reflect.get(_baseTemp, _a, _classtemp) + 1, _classTemp), _b)`

                            let mut expression = self.visit_node(v, Some(node_right)).expect(NIL);
                            let mut context = self.context.clone();
                            let f = v.factory_mut();
                            if tsr_ast::utilities::is_compound_assignment(operator) {
                                let mut getter_name = setter;
                                if !is_simple_inlineable_expression(f, setter) {
                                    getter_name = context.new_temp_variable(f);
                                    context.add_variable_declaration(f, getter_name);
                                    setter =
                                        context.new_assignment_expression(f, getter_name, setter);
                                }
                                let super_property_get = context.new_reflect_get_call(
                                    f,
                                    super_class_reference,
                                    getter_name,
                                    class_constructor,
                                );
                                context.set_original(super_property_get, node_left);
                                let left_loc = loc_of(f, node_left);
                                f.set_node_range(super_property_get, left_loc);
                                let token = f.new_token(
                                    get_non_assignment_operator_for_compound_assignment(operator),
                                );
                                expression = f.new_binary_expression(
                                    None,
                                    Some(super_property_get),
                                    None,
                                    Some(token),
                                    Some(expression),
                                );
                                let loc = loc_of(f, node);
                                f.set_node_range(expression, loc);
                            }

                            let temp = if discarded {
                                None
                            } else {
                                let temp = context.new_temp_variable(f);
                                context.add_variable_declaration(f, temp);
                                Some(temp)
                            };
                            let loc = loc_of(f, node);
                            if let Some(temp) = temp {
                                expression = context.new_assignment_expression(f, temp, expression);
                                f.set_node_range(expression, loc);
                            }

                            expression = context.new_reflect_set_call(
                                f,
                                super_class_reference,
                                setter,
                                expression,
                                class_constructor,
                            );
                            context.set_original(expression, node);
                            f.set_node_range(expression, loc);

                            if let Some(temp) = temp {
                                expression = context.new_comma_expression(f, expression, temp);
                                f.set_node_range(expression, loc);
                            }
                            return Some(expression);
                        }
                    }
                }
            }
        }

        let (left, operator, _, _) = binary_parts(v, node);
        if operator == K::InKeyword && kind_of(v, left) == K::PrivateIdentifier {
            // #x in obj
            return self.transform_private_identifier_in_in_expression(v, node);
        }

        self.visit_each_child(v, node)
    }

    // port: tsc/internal/transformers/estransforms/classfields.go:classFieldsTransformer.visitParenthesizedExpression
    #[allow(clippy::unnecessary_wraps)] // a visit function; upstream's result is a nullable node
    fn visit_parenthesized_expression(
        &self,
        v: &mut NodeVisitor<'_>,
        node: NodeId,
        discarded: bool,
    ) -> Option<NodeId> {
        // 8.4.5 RS: NamedEvaluation
        //   ParenthesizedExpression : `(` Expression `)`
        //     ...
        //     2. Return ? NamedEvaluation of |Expression| with argument _name_.
        let expression = v.node(node).expression();
        if discarded {
            let expression = self.with(v, Visitor::DiscardedValue, |d| d.visit_node(expression));
            return Some(
                v.factory_mut()
                    .update_parenthesized_expression(node, expression),
            );
        }
        let expression = self.visit_node(v, expression);
        Some(
            v.factory_mut()
                .update_parenthesized_expression(node, expression),
        )
    }

    // port: tsc/internal/transformers/estransforms/classfields.go:classFieldsTransformer.createPrivateIdentifierAssignment
    fn create_private_identifier_assignment(
        &self,
        v: &mut NodeVisitor<'_>,
        info: PrivateIdentifierInfo,
        receiver: NodeId,
        right: NodeId,
        operator: NodeKind,
    ) -> NodeId {
        let mut receiver = self.visit_node(v, Some(receiver)).expect(NIL);
        let mut right = self.visit_node(v, Some(right)).expect(NIL);

        if tsr_ast::utilities::is_compound_assignment(operator) {
            let (read_expression, initialize_expression) =
                self.create_copiable_receiver_expr(v, receiver);
            receiver = initialize_expression.unwrap_or(read_expression);
            let access = self.create_private_identifier_access_helper(v, info, read_expression);
            let f = v.factory_mut();
            let token = f.new_token(get_non_assignment_operator_for_compound_assignment(
                operator,
            ));
            right = f.new_binary_expression(None, Some(access), None, Some(token), Some(right));
        }

        let mut context = self.context.clone();
        let end = v.node(receiver).end();
        context.set_comment_range(receiver, TextRange::new(-1, i64::from(end)));

        let f = v.factory_mut();
        let state = || info.brand_check_identifier.expect(NIL);
        match info.kind {
            PrivateIdentifierKind::Accessor => context.new_class_private_field_set_helper(
                f,
                receiver,
                state(),
                right,
                info.kind,
                info.setter_name,
            ),
            PrivateIdentifierKind::Method => context.new_class_private_field_set_helper(
                f,
                receiver,
                state(),
                right,
                info.kind,
                None,
            ),
            PrivateIdentifierKind::Field => {
                let field = if info.is_static {
                    info.variable_name
                } else {
                    None
                };
                context.new_class_private_field_set_helper(
                    f,
                    receiver,
                    state(),
                    right,
                    info.kind,
                    field,
                )
            }
            PrivateIdentifierKind::Untransformed => panic!(
                "Debug failure. Access helpers should not be created for untransformed private elements"
            ),
        }
    }

    // port: tsc/internal/transformers/estransforms/classfields.go:classFieldsTransformer.getPrivateInstanceMethodsAndAccessors
    fn get_private_instance_methods_and_accessors(
        &self,
        factory: &dyn RuntimeFactory,
        node: NodeId,
    ) -> Vec<NodeId> {
        members_of(factory, node)
            .into_iter()
            .filter(|&member| {
                self.q(factory, |view| {
                    is_non_static_method_or_accessor_with_private_name(view, member)
                })
            })
            .collect()
    }

    /// Whether `n`, or a node under it except a property access name, is an
    /// identifier other than `class_name` that resolves to `class_original`.
    fn contains_constructor_reference(
        &self,
        factory: &dyn RuntimeFactory,
        n: NodeId,
        class_name: Option<NodeId>,
        class_original: NodeId,
    ) -> bool {
        let kind = kind_of(factory, n);
        if kind == K::Identifier && Some(n) != class_name {
            let declaration = self
                .resolver
                .borrow_mut()
                .get_referenced_value_declaration(n);
            if let Some(declaration) = self.failure.ok(declaration) {
                if declaration == Some(class_original) {
                    return true;
                }
            }
        }
        // For PropertyAccessExpression, only check the expression, not the name.
        // The .Name() is a property access name, not a value reference to the class.
        if kind == K::PropertyAccessExpression {
            return self.contains_constructor_reference(
                factory,
                expression_of(factory, n),
                class_name,
                class_original,
            );
        }
        for child in immediate_children(factory, n) {
            if self.contains_constructor_reference(factory, child, class_name, class_original) {
                return true;
            }
        }
        false
    }

    /// Whether a class member's body contains an identifier that resolves to
    /// the class declaration. Replaces Strada's
    /// `resolver.hasNodeCheckFlag(member, NodeCheckFlags.ContainsConstructorReference)`.
    /// Only member bodies are checked, not computed property names, which are
    /// evaluated while the binding is still correct.
    // port: tsc/internal/transformers/estransforms/classfields.go:classFieldsTransformer.memberContainsConstructorReference
    fn member_contains_constructor_reference(
        &self,
        factory: &dyn RuntimeFactory,
        member: NodeId,
        class_decl: NodeId,
    ) -> bool {
        let class_original = self.context.most_original(class_decl);
        let class_name = self.q(factory, |view| {
            tsr_ast::get_name_of_declaration(view, Some(class_decl))
        });
        // Check only the body/initializer of the member, not the name (which may be
        // a computed property name that shouldn't trigger alias substitution).
        let read = factory.node(member);
        if read.kind() == K::ClassStaticBlockDeclaration {
            let body = read
                .as_class_static_block_declaration()
                .expect("ClassStaticBlockDeclaration payload")
                .body();
            drop(read);
            if let Some(body) = body {
                if self.contains_constructor_reference(factory, body, class_name, class_original) {
                    return true;
                }
            }
        } else {
            let body = read.body();
            drop(read);
            if let Some(body) = body {
                if self.contains_constructor_reference(factory, body, class_name, class_original) {
                    return true;
                }
            }
        }
        if kind_of(factory, member) == K::PropertyDeclaration {
            if let Some(initializer) = factory.node(member).initializer() {
                if self.contains_constructor_reference(
                    factory,
                    initializer,
                    class_name,
                    class_original,
                ) {
                    return true;
                }
            }
        }
        false
    }

    /// Whether any member of a class references the class's own constructor.
    /// Replaces Strada's
    /// `resolver.hasNodeCheckFlag(node, NodeCheckFlags.ContainsConstructorReference)`.
    // port: tsc/internal/transformers/estransforms/classfields.go:classFieldsTransformer.classContainsConstructorReference
    fn class_contains_constructor_reference(
        &self,
        factory: &dyn RuntimeFactory,
        node: NodeId,
    ) -> bool {
        for member in members_of(factory, node) {
            if self.member_contains_constructor_reference(factory, member, node) {
                return true;
            }
        }
        false
    }

    // port: tsc/internal/transformers/estransforms/classfields.go:classFieldsTransformer.getClassFacts
    #[allow(clippy::if_same_then_else)] // upstream's branch order
    fn get_class_facts(&self, factory: &dyn RuntimeFactory, node: NodeId) -> u32 {
        let mut facts = class_facts::NONE;

        let original = self.context.most_original(node);
        if tsr_ast::utilities::is_class_like(&factory.node(original))
            && self.q(factory, |view| {
                tsr_ast::utilities_class::class_or_constructor_parameter_is_decorated(
                    view,
                    self.legacy_decorators, /*useLegacyDecorators*/
                    original,
                )
            })
        {
            facts |= class_facts::CLASS_WAS_DECORATED;
        }

        if self.should_transform_private_elements_or_class_static_blocks
            && (class_has_class_this_assignment(&self.context, factory, node)
                || class_has_explicitly_assigned_name(&self.context, factory, node))
        {
            facts |= class_facts::NEEDS_CLASS_CONSTRUCTOR_REFERENCE;
        }

        let mut contains_public_instance_fields = false;
        let mut contains_initialized_public_instance_fields = false;
        let mut contains_instance_private_elements = false;
        let mut contains_instance_auto_accessors = false;

        for member in members_of(factory, node) {
            if self.is_static(factory, member) {
                let name = factory.node(member).name();
                if name.is_some_and(|name| {
                    kind_of(factory, name) == K::PrivateIdentifier
                        || self.is_auto_accessor_property_declaration(factory, member)
                }) && self.should_transform_private_elements_or_class_static_blocks
                {
                    facts |= class_facts::NEEDS_CLASS_CONSTRUCTOR_REFERENCE;
                } else if self.is_auto_accessor_property_declaration(factory, member)
                    && self.should_transform_auto_accessors
                    && factory.node(node).name().is_none()
                    && self.context.class_this(node).is_none()
                {
                    facts |= class_facts::NEEDS_CLASS_CONSTRUCTOR_REFERENCE;
                }
                let member_kind = kind_of(factory, member);
                if member_kind == K::PropertyDeclaration
                    || member_kind == K::ClassStaticBlockDeclaration
                {
                    let member_facts = self.subtree_facts(factory, member);
                    if self.should_transform_this_in_static_initializers
                        && member_facts & subtree_flags::LEXICAL_THIS != 0
                    {
                        facts |= class_facts::NEEDS_SUBSTITUTION_FOR_THIS_IN_CLASS_STATIC_FIELD;
                        if facts & class_facts::CLASS_WAS_DECORATED == 0 {
                            facts |= class_facts::NEEDS_CLASS_CONSTRUCTOR_REFERENCE;
                        }
                    }
                    if self.should_transform_super_in_static_initializers
                        && member_facts & subtree_flags::LEXICAL_SUPER != 0
                        && facts & class_facts::CLASS_WAS_DECORATED == 0
                    {
                        facts |= class_facts::NEEDS_CLASS_CONSTRUCTOR_REFERENCE
                            | class_facts::NEEDS_CLASS_SUPER_REFERENCE;
                    }
                }
            } else if !self.q(factory, |view| {
                tsr_ast::utilities_class::has_abstract_modifier(
                    view,
                    self.context.most_original(member),
                )
            }) {
                if self.is_auto_accessor_property_declaration(factory, member) {
                    contains_instance_auto_accessors = true;
                    contains_instance_private_elements = contains_instance_private_elements
                        || self.is_private_identifier_class_element_declaration(factory, member);
                } else if self.is_private_identifier_class_element_declaration(factory, member) {
                    contains_instance_private_elements = true;
                    if self.member_contains_constructor_reference(factory, member, node) {
                        facts |= class_facts::NEEDS_CLASS_CONSTRUCTOR_REFERENCE;
                    }
                } else if kind_of(factory, member) == K::PropertyDeclaration {
                    contains_public_instance_fields = true;
                    contains_initialized_public_instance_fields =
                        contains_initialized_public_instance_fields
                            || factory.node(member).initializer().is_some();
                }
            }
        }

        let will_hoist_initializers_to_constructor =
            (self.should_transform_initializers_using_define && contains_public_instance_fields)
                || (self.should_transform_initializers_using_set
                    && contains_initialized_public_instance_fields)
                || (self.should_transform_private_elements_or_class_static_blocks
                    && contains_instance_private_elements)
                || (self.should_transform_private_elements_or_class_static_blocks
                    && contains_instance_auto_accessors
                    && self.should_transform_auto_accessors);

        if will_hoist_initializers_to_constructor {
            facts |= class_facts::WILL_HOIST_INITIALIZERS_TO_CONSTRUCTOR;
        }

        facts
    }

    // port: tsc/internal/transformers/estransforms/classfields.go:classFieldsTransformer.visitExpressionWithTypeArgumentsInHeritageClause
    fn visit_expression_with_type_arguments_in_heritage_clause(
        &self,
        v: &mut NodeVisitor<'_>,
        node: NodeId,
    ) -> Option<NodeId> {
        let facts = self.lex_data().map_or(class_facts::NONE, |data| data.facts);
        if facts & class_facts::NEEDS_CLASS_SUPER_REFERENCE != 0 {
            let mut context = self.context.clone();
            let temp = context.new_temp_variable_ex(
                v.factory_mut(),
                AutoGenerateOptions {
                    flags: g::RESERVED_IN_NESTED_SCOPES,
                    ..AutoGenerateOptions::default()
                },
            );
            context.add_variable_declaration(v.factory_mut(), temp);
            self.update_class_lexical_environment(|data| data.super_class_reference = Some(temp));
            let expression = v.node(node).expression();
            let visited = self.visit_node(v, expression).expect(NIL);
            let f = v.factory_mut();
            let assignment = context.new_assignment_expression(f, temp, visited);
            return Some(f.update_expression_with_type_arguments(
                node,
                Some(assignment),
                None, /*typeArguments*/
            ));
        }
        self.with(v, Visitor::HeritageClause, |h| {
            h.visit_each_child(Some(node))
        })
    }

    // port: tsc/internal/transformers/estransforms/classfields.go:classFieldsTransformer.visitInNewClassLexicalEnvironment
    fn visit_in_new_class_lexical_environment(
        &self,
        v: &mut NodeVisitor<'_>,
        node: NodeId,
        visitor: ClassVisitFn<'a>,
    ) -> Option<NodeId> {
        let saved_current_class_container = self.current_class_container.get();
        let saved_pending_expressions = self.pending_expressions.take();
        let saved_lexical_environment = self.lexical_environment.borrow().clone();
        self.current_class_container.set(Some(node));
        self.start_class_lexical_environment();
        let original = self.context.most_original(node);
        self.enclosing_class_declarations
            .borrow_mut()
            .insert(original);

        if self.should_transform_private_elements_or_class_static_blocks
            || self.node_has_transform_private_static_elements_flag(node)
        {
            let name = self.q(v.factory(), |view| {
                tsr_ast::get_name_of_declaration(view, Some(node))
            });
            if let Some(name) = name.filter(|&name| kind_of(v, name) == K::Identifier) {
                self.get_private_identifier_environment(|env| env.data.class_name = Some(name));
            } else if let Some(assigned_name) = self.context.assigned_name(node) {
                if kind_of(v, assigned_name) == K::StringLiteral {
                    // If the assigned name has a textSourceNode that is an identifier, use it directly.
                    let text_source_node = self
                        .context
                        .text_source(assigned_name)
                        .filter(|&source| kind_of(v, source) == K::Identifier);
                    if let Some(text_source_node) = text_source_node {
                        self.get_private_identifier_environment(|env| {
                            env.data.class_name = Some(text_source_node);
                        });
                    } else {
                        let text = v
                            .node(assigned_name)
                            .as_string_literal()
                            .expect("StringLiteral payload")
                            .text_owned();
                        if tsr_scanner::is_identifier_text(
                            text.as_bytes(),
                            LanguageVariant::STANDARD,
                        ) {
                            // If the text is a valid identifier, create an identifier from it.
                            let prefix_name = v.factory_mut().new_identifier(text);
                            self.get_private_identifier_environment(|env| {
                                env.data.class_name = Some(prefix_name);
                            });
                        }
                    }
                }
            }
        }

        if self.should_transform_private_elements_or_class_static_blocks {
            let private_instance_methods_and_accessors =
                self.get_private_instance_methods_and_accessors(v.factory(), node);
            if let Some(&first) = private_instance_methods_and_accessors.first() {
                let first_name = name_of(v, first);
                let weak_set_name =
                    self.create_hoisted_variable_for_class(v, b"instances", first_name, b"");
                self.get_private_identifier_environment(|env| {
                    env.data.weak_set_name = Some(weak_set_name);
                });
            }
        }

        let facts = self.get_class_facts(v.factory(), node);
        if facts != class_facts::NONE {
            self.update_class_lexical_environment(|data| data.facts = facts);
        }

        let result = visitor(self, v, node, facts);
        self.enclosing_class_declarations
            .borrow_mut()
            .remove(&original);
        self.end_class_lexical_environment();
        let restored = self.lexical_environment.borrow().clone();
        assert!(
            match (&restored, &saved_lexical_environment) {
                (Some(a), Some(b)) => Rc::ptr_eq(a, b),
                (None, None) => true,
                _ => false,
            },
            "Debug failure. False expression."
        );
        self.current_class_container
            .set(saved_current_class_container);
        *self.pending_expressions.borrow_mut() = saved_pending_expressions;
        *self.lexical_environment.borrow_mut() = saved_lexical_environment;
        result
    }

    // port: tsc/internal/transformers/estransforms/classfields.go:classFieldsTransformer.visitClassDeclaration
    fn visit_class_declaration(&self, v: &mut NodeVisitor<'_>, node: NodeId) -> Option<NodeId> {
        self.visit_in_new_class_lexical_environment(
            v,
            node,
            Self::visit_class_declaration_in_new_class_lexical_environment,
        )
    }

    // port: tsc/internal/transformers/estransforms/classfields.go:classFieldsTransformer.visitClassDeclarationInNewClassLexicalEnvironment
    #[allow(clippy::unnecessary_wraps)] // a visit function; upstream's result is a nullable node
    fn visit_class_declaration_in_new_class_lexical_environment(
        &self,
        v: &mut NodeVisitor<'_>,
        node: NodeId,
        facts: u32,
    ) -> Option<NodeId> {
        let mut context = self.context.clone();
        // If a class has private static fields, or a static field has a `this` or `super` reference,
        // then we need to allocate a temp variable to hold on to that reference.
        let mut pending_class_reference_assignment = None;
        if facts & class_facts::NEEDS_CLASS_CONSTRUCTOR_REFERENCE != 0 {
            // If we aren't transforming class static blocks, then we can't reuse `_classThis` since in
            // `class C { ... static { _classThis = ... } }; _classThis = C` the outer assignment would occur *after*
            // class static blocks evaluate and would overwrite the replacement constructor produced by class
            // decorators.

            // If we are transforming class static blocks, then we can reuse `_classThis` since the assignment
            // will be evaluated *before* the transformed static blocks are evaluated and thus won't overwrite
            // the replacement constructor.
            let class_this = context
                .class_this(node)
                .filter(|_| self.should_transform_private_elements_or_class_static_blocks);
            if let Some(class_this) = class_this {
                self.update_class_lexical_environment(|data| {
                    data.class_constructor = Some(class_this);
                });
                let local_name = self.get_local_name(v, node);
                pending_class_reference_assignment = Some(context.new_assignment_expression(
                    v.factory_mut(),
                    class_this,
                    local_name,
                ));
            } else {
                let f = v.factory_mut();
                let temp = context.new_temp_variable_ex(
                    f,
                    AutoGenerateOptions {
                        flags: g::RESERVED_IN_NESTED_SCOPES,
                        ..AutoGenerateOptions::default()
                    },
                );
                context.add_variable_declaration(f, temp);
                let clone = clone_node(f, temp);
                self.update_class_lexical_environment(|data| data.class_constructor = Some(clone));
                let local_name = self.get_local_name(v, node);
                pending_class_reference_assignment =
                    Some(context.new_assignment_expression(v.factory_mut(), temp, local_name));
            }
        }

        if let Some(class_this) = context.class_this(node) {
            self.update_class_lexical_environment(|data| data.class_this = Some(class_this));
        }

        let is_class_with_constructor_reference =
            self.class_contains_constructor_reference(v.factory(), node);

        // Register class alias BEFORE visiting members (Strada registers after, since its
        // onSubstituteNode runs at emit time; we substitute eagerly during transformation).
        let alias = self.get_class_lexical_environment().class_constructor;
        if is_class_with_constructor_reference {
            if let Some(alias) = alias {
                self.class_aliases
                    .borrow_mut()
                    .insert(context.most_original(node), alias);
            }
        }

        let (node_modifiers, node_heritage_clauses, node_name) = {
            let read = v.node(node);
            let data = read
                .as_class_declaration()
                .expect("ClassDeclaration payload");
            (data.modifiers(), data.heritage_clauses(), data.name())
        };
        let mut modifiers = self.with(v, Visitor::Modifier, |mv| {
            mv.visit_modifiers(node_modifiers)
        });
        let heritage_clauses = self.with(v, Visitor::HeritageClause, |h| {
            h.visit_nodes(node_heritage_clauses)
        });
        let (members, members_prologue) = self.transform_class_members(v, node);

        let mut statements = Vec::new();

        if let Some(assignment) = pending_class_reference_assignment {
            self.pending_expressions.borrow_mut().insert(0, assignment);
        }

        // Write any pending expressions from elided or moved computed property names
        let pending = self.pending_expressions.borrow().clone();
        if !pending.is_empty() {
            let f = v.factory_mut();
            let inlined = context.inline_expressions(f, &pending);
            statements.push(f.new_expression_statement(inlined));
        }

        // A class declaration without a name needs a generated name if it has static
        // initialized properties, since those will be moved outside the class body and
        // need to reference the class by name.
        let mut name = node_name;

        if self.should_transform_initializers_using_set
            || self.should_transform_private_elements_or_class_static_blocks
        {
            // Emit static property assignment. Because classDeclaration is lexically evaluated,
            // it is safe to emit static property assignment after classDeclaration
            // From ES6 specification:
            //   HasLexicalDeclaration (N) : Determines if the argument identifier has a binding in this environment record that was created using
            //                               a lexical declaration such as a LexicalDeclaration or a ClassDeclaration.
            let static_properties =
                self.get_static_properties_and_class_static_block(v.factory(), node);
            if !static_properties.is_empty() {
                if name.is_none() {
                    name = Some(context.new_generated_name_for_node(v.factory_mut(), node));
                }
                let local_name = self.get_local_name(v, node);
                statements = self.add_property_or_class_static_block_statements(
                    v,
                    statements,
                    &static_properties,
                    local_name,
                );
            }
        }

        let is_export =
            super::utilities::has_syntactic_modifier(v.factory(), node, modifier_flags::EXPORT);
        let is_default =
            super::utilities::has_syntactic_modifier(v.factory(), node, modifier_flags::DEFAULT);

        if !statements.is_empty() && is_export && is_default {
            modifiers = extract_modifiers(
                &self.context,
                v.factory_mut(),
                modifiers,
                !modifier_flags::EXPORT_DEFAULT,
            );
            let local_name = self.get_local_name(v, node);
            let export_assignment = v.factory_mut().new_export_assignment(
                None,
                false, /*isExportEquals*/
                None,  /*typeNode*/
                Some(local_name),
            );
            statements.push(export_assignment);
        }

        let f = v.factory_mut();
        let updated_class = f.update_class_declaration(
            node,
            modifiers,
            name,
            None, /*typeParameters*/
            heritage_clauses,
            members,
        );

        let mut result = Vec::with_capacity(1 + statements.len() + 1);
        if let Some(members_prologue) = members_prologue {
            result.push(Some(f.new_expression_statement(Some(members_prologue))));
        }
        result.push(Some(updated_class));
        result.extend(statements.into_iter().map(Some));
        let children = f.alloc_nodes(result);
        Some(f.new_syntax_list(children))
    }

    // port: tsc/internal/transformers/estransforms/classfields.go:classFieldsTransformer.visitClassExpression
    fn visit_class_expression(&self, v: &mut NodeVisitor<'_>, node: NodeId) -> Option<NodeId> {
        self.visit_in_new_class_lexical_environment(
            v,
            node,
            Self::visit_class_expression_in_new_class_lexical_environment,
        )
    }

    /// `temp` as the class expression's temp: a `let` when it must be
    /// block-scoped, otherwise a `var`.
    fn add_class_expression_temp_declaration(&self, v: &mut NodeVisitor<'_>, temp: NodeId) {
        let mut context = self.context.clone();
        if self.class_expression_needs_block_scoped_temp(v.factory()) {
            context.add_lexical_declaration(v.factory_mut(), temp);
        } else {
            context.add_variable_declaration(v.factory_mut(), temp);
        }
    }

    /// A temp variable reserved in nested scopes.
    fn new_reserved_temp_variable(&self, v: &mut NodeVisitor<'_>) -> NodeId {
        self.context.clone().new_temp_variable_ex(
            v.factory_mut(),
            AutoGenerateOptions {
                flags: g::RESERVED_IN_NESTED_SCOPES,
                ..AutoGenerateOptions::default()
            },
        )
    }

    // port: tsc/internal/transformers/estransforms/classfields.go:classFieldsTransformer.visitClassExpressionInNewClassLexicalEnvironment
    fn visit_class_expression_in_new_class_lexical_environment(
        &self,
        v: &mut NodeVisitor<'_>,
        node: NodeId,
        facts: u32,
    ) -> Option<NodeId> {
        let mut context = self.context.clone();

        // If this class expression is a transformation of a decorated class declaration,
        // then we want to output the pendingExpressions as statements, not as inlined
        // expressions with the class statement.
        //
        // In this case, we use pendingStatements to produce the same output as the
        // class declaration transformation. The VariableStatement visitor will insert
        // these statements after the class expression variable statement.
        let is_decorated_class_declaration = facts & class_facts::CLASS_WAS_DECORATED != 0;

        if let Some(class_this) = context.class_this(node) {
            self.update_class_lexical_environment(|data| data.class_this = Some(class_this));
        }

        let mut temp = None;
        if facts & class_facts::NEEDS_CLASS_CONSTRUCTOR_REFERENCE != 0 {
            let class_this = context.class_this(node).filter(|_| {
                self.should_transform_private_elements_or_class_static_blocks
                    || self.node_has_transform_private_static_elements_flag(node)
            });
            if let Some(class_this) = class_this {
                self.update_class_lexical_environment(|data| {
                    data.class_constructor = Some(class_this);
                });
                temp = Some(class_this);
            } else {
                let created = self.new_reserved_temp_variable(v);
                self.add_class_expression_temp_declaration(v, created);
                let clone = clone_node(v.factory_mut(), created);
                self.update_class_lexical_environment(|data| data.class_constructor = Some(clone));
                temp = Some(created);
            }
        }

        let static_properties_or_class_static_blocks =
            self.get_static_properties_and_class_static_block(v.factory(), node);

        // Pre-compute whether the class expression will need a temp variable wrapper.
        // Strada registers class aliases AFTER transformClassMembers (since onSubstituteNode runs
        // at emit time), but we must predict this before visiting members since we substitute
        // eagerly. This requires pre-detecting willHavePrivatePendingExpressions.
        let mut is_class_with_constructor_reference = false;
        let mut has_transformable_statics = false;
        let mut defer_temp_declaration = false;
        if !is_decorated_class_declaration {
            is_class_with_constructor_reference =
                self.class_contains_constructor_reference(v.factory(), node);
            has_transformable_statics = (self
                .should_transform_private_elements_or_class_static_blocks
                || self.node_has_transform_private_static_elements_flag(node))
                && static_properties_or_class_static_blocks
                    .iter()
                    .any(|&n| self.is_transformable_static(v.factory(), n));

            // Private instance elements (fields, methods, accessors) transformed to
            // WeakMap/WeakSet will add initialization expressions to pendingExpressions
            // during transformClassMembers. Pre-detect this so we know whether the class
            // will be wrapped with a temp variable.
            let will_have_private_pending_expressions = self
                .should_transform_private_elements_or_class_static_blocks
                && members_of(v.factory(), node).into_iter().any(|n| {
                    self.is_private_identifier_class_element_declaration(v.factory(), n)
                        && !self.has_static_modifier(v.factory(), n)
                        && self.should_transform_class_element_to_weak_map(v.factory(), n)
                });
            let will_need_temp_wrapper =
                has_transformable_statics || will_have_private_pending_expressions;

            // Register class alias BEFORE visiting members (Strada registers after, since its
            // onSubstituteNode runs at emit time). Only register when the class will be wrapped
            // with a temp, matching Strada's conditional registration.
            if is_class_with_constructor_reference
                && will_need_temp_wrapper
                && self
                    .get_class_lexical_environment()
                    .class_constructor
                    .is_none()
            {
                // Create temp early so the alias is available during member visiting, even though in the Strada
                // reference the temp would be created later in the pendingExpressions branch.
                let created = self.new_reserved_temp_variable(v);
                // Defer AddVariableDeclaration to preserve Strada's variable declaration ordering.
                defer_temp_declaration = true;
                let clone = clone_node(v.factory_mut(), created);
                self.update_class_lexical_environment(|data| data.class_constructor = Some(clone));
                temp = Some(created);
            }
            let alias = self.get_class_lexical_environment().class_constructor;
            if is_class_with_constructor_reference && will_need_temp_wrapper {
                if let Some(alias) = alias {
                    self.class_aliases
                        .borrow_mut()
                        .insert(context.most_original(node), alias);
                }
            }
        }

        let (node_modifiers, node_heritage_clauses, node_name) = {
            let read = v.node(node);
            let data = read.as_class_expression().expect("ClassExpression payload");
            (data.modifiers(), data.heritage_clauses(), data.name())
        };
        let modifiers = self.with(v, Visitor::Modifier, |mv| {
            mv.visit_modifiers(node_modifiers)
        });
        let heritage_clauses = self.with(v, Visitor::HeritageClause, |h| {
            h.visit_nodes(node_heritage_clauses)
        });
        let (members, members_prologue) = self.transform_class_members(v, node);

        if defer_temp_declaration {
            self.add_class_expression_temp_declaration(v, temp.expect(NIL));
        }

        let class_expression = v.factory_mut().update_class_expression(
            node,
            modifiers,
            node_name,
            None, /*typeParameters*/
            heritage_clauses,
            members,
        );

        let mut expressions = Vec::new();
        if let Some(members_prologue) = members_prologue {
            expressions.push(members_prologue);
        }

        if is_decorated_class_declaration {
            // Decorated class declaration path: emit static properties as separate statements
            // via pendingStatements, matching the class declaration output structure.

            // Write any pending expressions from elided or moved computed property names
            let pending = self.pending_expressions.borrow().clone();
            for expr in pending {
                let statement = v.factory_mut().new_expression_statement(Some(expr));
                self.pending_statements.borrow_mut().push(statement);
            }

            // Emit static properties as statements (via pendingStatements) using the class's
            // internal name as the receiver, matching the class declaration output structure.
            if !static_properties_or_class_static_blocks.is_empty() {
                let class_this_or_name = match context.class_this(node) {
                    Some(class_this) => class_this,
                    None => self.get_local_name(v, node),
                };
                let pending_statements = self.pending_statements.take();
                let pending_statements = self.add_property_or_class_static_block_statements(
                    v,
                    pending_statements,
                    &static_properties_or_class_static_blocks,
                    class_this_or_name,
                );
                *self.pending_statements.borrow_mut() = pending_statements;
            }

            let f = v.factory_mut();
            if let Some(temp) = temp {
                expressions.push(context.new_assignment_expression(f, temp, class_expression));
            } else if let Some(class_this) = context
                .class_this(node)
                .filter(|_| self.should_transform_private_elements_or_class_static_blocks)
            {
                expressions.push(context.new_assignment_expression(
                    f,
                    class_this,
                    class_expression,
                ));
            } else {
                expressions.push(class_expression);
            }
        } else if has_transformable_statics || !self.pending_expressions.borrow().is_empty() {
            let temp = if let Some(temp) = temp {
                temp
            } else {
                let created = self.new_reserved_temp_variable(v);
                self.add_class_expression_temp_declaration(v, created);
                let clone = clone_node(v.factory_mut(), created);
                self.update_class_lexical_environment(|data| {
                    data.class_constructor = Some(clone);
                });
                if is_class_with_constructor_reference {
                    let alias = self.get_class_lexical_environment().class_constructor;
                    self.class_aliases
                        .borrow_mut()
                        .insert(context.most_original(node), alias.expect(NIL));
                }
                created
            };

            expressions.push(context.new_assignment_expression(
                v.factory_mut(),
                temp,
                class_expression,
            ));

            // Add any pending expressions leftover from elided or relocated computed property names
            expressions.extend(self.pending_expressions.borrow().iter().copied());

            expressions.extend(
                self.generate_initialized_property_expressions_or_class_static_block(
                    v,
                    &static_properties_or_class_static_blocks,
                    temp,
                ),
            );
            expressions.push(clone_node(v.factory_mut(), temp));
        } else {
            expressions.push(class_expression);
        }

        if expressions.len() > 1 {
            context.add_emit_flags(class_expression, emit_flags::INDENTED);
            for &expr in &expressions {
                context.add_emit_flags(expr, emit_flags::START_ON_NEW_LINE);
            }
        }
        context.inline_expressions(v.factory_mut(), &expressions)
    }

    // port: tsc/internal/transformers/estransforms/classfields.go:classFieldsTransformer.visitClassStaticBlockDeclaration
    fn visit_class_static_block_declaration(
        &self,
        v: &mut NodeVisitor<'_>,
        node: NodeId,
    ) -> Option<NodeId> {
        if !self.should_transform_private_elements_or_class_static_blocks {
            return self.visit_each_child(v, node);
        }
        // ClassStaticBlockDeclaration for classes are transformed in visitClassDeclaration/visitClassExpression.
        None
    }

    /// Replaces Strada's `substituteThisExpression`/`onSubstituteNode`:
    /// Strada substitutes `this` at emit time; this does it while
    /// transforming. (Strada's `noSubstitution` set is not needed:
    /// `transformAutoAccessor` passes the receiver directly.)
    // port: tsc/internal/transformers/estransforms/classfields.go:classFieldsTransformer.visitThisExpression
    #[allow(clippy::unnecessary_wraps)] // a visit function; upstream's result is a nullable node
    fn visit_this_expression(&self, v: &mut NodeVisitor<'_>, node: NodeId) -> Option<NodeId> {
        if self.inside_computed_property_name.get()
            && self.should_transform_this_in_static_initializers
        {
            if let Some(data) = self.lex_data() {
                // Don't replace `this` in computed property names for ES-decorated classes.
                // The esDecorator transformer wraps them in an arrow IIFE where `this` already
                // refers to the correct outer scope.
                if data.facts & class_facts::CLASS_WAS_DECORATED == 0 || self.legacy_decorators {
                    if let Some(class_this) = self.try_get_class_this_no_container() {
                        return Some(class_this);
                    }
                }
            }
        }
        if self.should_transform_this_in_static_initializers {
            if let Some(element) = self.current_class_element.get() {
                let element_kind = kind_of(v, element);
                if (element_kind == K::ClassStaticBlockDeclaration
                    || (element_kind == K::PropertyDeclaration
                        && self.has_static_modifier(v.factory(), element)))
                    && self.lex_data().is_some()
                {
                    if let Some(class_this) = self.try_get_class_this_no_container() {
                        return Some(class_this);
                    }
                    // When the class was decorated with legacy decorators and no class constructor
                    // reference is available, the decorator may replace the constructor, so `this`
                    // cannot reliably point to the class. Use `(void 0)` instead.
                    let decorated = self
                        .lex_data()
                        .is_some_and(|data| data.facts & class_facts::CLASS_WAS_DECORATED != 0);
                    if decorated && self.legacy_decorators {
                        let f = v.factory_mut();
                        let void_zero = self.context.new_void_zero_expression(f);
                        return Some(f.new_parenthesized_expression(Some(void_zero)));
                    }
                }
            }
        }
        Some(node)
    }

    // port: tsc/internal/transformers/estransforms/classfields.go:classFieldsTransformer.transformClassMembers
    fn transform_class_members(
        &self,
        v: &mut NodeVisitor<'_>,
        node: NodeId,
    ) -> (Option<NodeListId>, Option<NodeId>) {
        let mut context = self.context.clone();
        let should_transform_private_static_elements_in_class =
            context.emit_flags(node) & emit_flags::TRANSFORM_PRIVATE_STATIC_ELEMENTS != 0;
        let mut prologue = None;

        // Declare private names
        if self.should_transform_private_elements_or_class_static_blocks
            || self.should_transform_private_static_elements_in_file.get()
        {
            for member in members_of(v.factory(), node) {
                if self.is_private_identifier_class_element_declaration(v.factory(), member) {
                    if self.should_transform_class_element_to_weak_map(v.factory(), member) {
                        self.add_private_identifier_to_environment(v, member);
                    } else {
                        let name = name_of(v, member);
                        self.get_private_identifier_environment(|env| {
                            self.set_private_identifier(
                                v.factory(),
                                env,
                                name,
                                PrivateIdentifierInfo::new(PrivateIdentifierKind::Untransformed),
                            );
                        });
                    }
                }
            }

            if self.should_transform_private_elements_or_class_static_blocks
                && !self
                    .get_private_instance_methods_and_accessors(v.factory(), node)
                    .is_empty()
            {
                self.create_brand_check_weak_set_for_private_methods(v);
            }

            if self.should_transform_auto_accessors_in_current_class() {
                for member in members_of(v.factory(), node) {
                    if self.is_auto_accessor_property_declaration(v.factory(), member) {
                        let member_name = name_of(v, member);
                        let storage_name = context.new_generated_private_name_for_node_ex(
                            v.factory_mut(),
                            member_name,
                            AutoGenerateOptions {
                                suffix: JsString::from_bytes(&b"_accessor_storage"[..]),
                                ..AutoGenerateOptions::default()
                            },
                        );
                        if self.should_transform_private_elements_or_class_static_blocks
                            || should_transform_private_static_elements_in_class
                                && self.has_static_modifier(v.factory(), member)
                        {
                            self.add_private_identifier_property_declaration_to_environment(
                                v,
                                member,
                                storage_name,
                            );
                        } else {
                            // Only register as untransformed if it hasn't already been registered
                            // by the first loop (e.g., if esDecorators expanded a private auto-accessor
                            // into a backing field with the same generated name).
                            self.get_private_identifier_environment(|env| {
                                if self
                                    .get_private_identifier(v.factory(), env, storage_name)
                                    .is_none()
                                {
                                    self.set_private_identifier(
                                        v.factory(),
                                        env,
                                        storage_name,
                                        PrivateIdentifierInfo::new(
                                            PrivateIdentifierKind::Untransformed,
                                        ),
                                    );
                                }
                            });
                        }
                    }
                }
            }
        }

        let member_list = v.node(node).member_list();
        let mut members = self.with(v, Visitor::ClassElement, |c| c.visit_nodes(member_list));
        let member_nodes = list_nodes(v.factory(), members);

        // Create a synthetic constructor if necessary
        let mut synthetic_constructor = None;
        if !member_nodes
            .iter()
            .any(|&member| kind_of(v, member) == K::Constructor)
        {
            synthetic_constructor = self.transform_constructor(v, None, node);
        }

        // If there are pending expressions create a class static block in which to evaluate them, but only if
        // class static blocks are not also being transformed. This block will be injected at the top of the class
        // to ensure that expressions from computed property names are evaluated before any other static
        // initializers.
        let mut synthetic_static_block = None;
        let pending = self.pending_expressions.borrow().clone();
        if !self.should_transform_private_elements_or_class_static_blocks && !pending.is_empty() {
            let f = v.factory_mut();
            let inlined = context.inline_expressions(f, &pending);
            let mut statement = f.new_expression_statement(inlined);
            if self.subtree_facts(v.factory(), statement) & subtree_flags::LEXICAL_THIS_OR_SUPER
                != 0
            {
                // If there are `this` or `super` references from computed property names, shift the expression
                // into an arrow function to be evaluated in the outer scope so that `this` and `super` are
                // properly captured.
                let f = v.factory_mut();
                let temp = context.new_temp_variable(f);
                context.add_variable_declaration(f, temp);
                let parameters = new_node_list(f, Vec::new());
                let arrow_token = f.new_token(K::EqualsGreaterThanToken.into());
                let body_statements = new_node_list(f, vec![statement]);
                let body = f.new_block(Some(body_statements), false /*multiline*/);
                let arrow = f.new_arrow_function(
                    None,             /*modifiers*/
                    None,             /*typeParameters*/
                    Some(parameters), /*parameters*/
                    None,             /*returnType*/
                    None,             /*fullSignature*/
                    Some(arrow_token),
                    Some(body),
                );
                prologue = Some(context.new_assignment_expression(f, temp, arrow));
                let arguments = new_node_list(f, Vec::new());
                let call = f.new_call_expression(
                    Some(temp),
                    None, /*questionDotToken*/
                    None, /*typeArguments*/
                    Some(arguments),
                    node_flags::NONE,
                );
                statement = f.new_expression_statement(Some(call));
            }

            let f = v.factory_mut();
            let statements = new_node_list(f, vec![statement]);
            let block = f.new_block(Some(statements), false /*multiline*/);
            synthetic_static_block =
                Some(f.new_class_static_block_declaration(None /*modifiers*/, Some(block)));
            self.pending_expressions.borrow_mut().clear();
        }

        // If we created a synthetic constructor or class static block, add them to the visited members
        if synthetic_constructor.is_some() || synthetic_static_block.is_some() {
            let mut members_array = Vec::with_capacity(member_nodes.len() + 2);

            // Find and preserve classThis assignment block and named evaluation helper block at the top
            let class_this_idx = member_nodes
                .iter()
                .position(|&n| is_class_this_assignment_block(&context, v.factory(), n));
            let named_eval_idx = member_nodes
                .iter()
                .position(|&n| is_class_named_evaluation_helper_block(&context, v.factory(), n));

            if let Some(index) = class_this_idx {
                members_array.push(member_nodes[index]);
            }
            if let Some(index) = named_eval_idx {
                members_array.push(member_nodes[index]);
            }
            if let Some(constructor) = synthetic_constructor {
                members_array.push(constructor);
            }
            if let Some(block) = synthetic_static_block {
                members_array.push(block);
            }

            for (i, &member) in member_nodes.iter().enumerate() {
                if Some(i) != class_this_idx && Some(i) != named_eval_idx {
                    members_array.push(member);
                }
            }
            let f = v.factory_mut();
            let list = new_node_list(f, members_array);
            let loc = f.read_list(member_list.expect(NIL)).loc();
            f.set_list_location(list, loc);
            members = Some(list);
        }

        (members, prologue)
    }

    // port: tsc/internal/transformers/estransforms/classfields.go:classFieldsTransformer.createBrandCheckWeakSetForPrivateMethods
    fn create_brand_check_weak_set_for_private_methods(&self, v: &mut NodeVisitor<'_>) {
        let weak_set_name = self.get_private_identifier_environment(|env| env.data.weak_set_name);
        let weak_set_name = weak_set_name.unwrap_or_else(|| {
            panic!(
                "Debug failure. False expression: weakSetName should be set in private identifier environment"
            )
        });

        let f = v.factory_mut();
        let weak_set = f.new_identifier(JsString::from_bytes(&b"WeakSet"[..]));
        let arguments = new_node_list(f, Vec::new());
        let new_expression =
            f.new_new_expression(Some(weak_set), None /*typeArguments*/, Some(arguments));
        let assignment = self
            .context
            .new_assignment_expression(f, weak_set_name, new_expression);
        self.add_pending_expressions(&[assignment]);
    }

    // port: tsc/internal/transformers/estransforms/classfields.go:classFieldsTransformer.transformConstructor
    fn transform_constructor(
        &self,
        v: &mut NodeVisitor<'_>,
        constructor: Option<NodeId>,
        container: NodeId,
    ) -> Option<NodeId> {
        // NOTE: The Strada reference pre-visits the constructor via `visitNode(constructor, visitor)` before
        // checking WillHoistInitializersToConstructor. This is not done here because Go's variable environment
        // (StartVariableEnvironment/EndAndMergeVariableEnvironment) is scoped inside transformConstructorBody.
        // Pre-visiting would hoist variables outside that scope, causing them to appear after field initializers
        // instead of before. Instead, we visit parameters and body separately within the correct scopes.
        if self.lex_data().is_none_or(|data| {
            data.facts & class_facts::WILL_HOIST_INITIALIZERS_TO_CONSTRUCTOR == 0
        }) {
            if let Some(constructor) = constructor {
                return self.visit_each_child(v, constructor);
            }
            return None;
        }

        let extends_clause_element = self.q(v.factory(), |view| {
            tsr_ast::utilities_class::get_class_extends_heritage_element(view, container)
        });
        let is_derived_class = extends_clause_element.is_some_and(|element| {
            let expression = expression_of(v, element);
            let skipped = self.q_node(v.factory(), expression, |view| {
                tsr_ast::utilities::skip_outer_expressions(
                    view,
                    expression,
                    outer_expression_kinds::ALL,
                )
            });
            kind_of(v, skipped) != K::NullKeyword
        });

        let mut parameters = None;
        if let Some(constructor) = constructor {
            let parameter_list = v.node(constructor).parameter_list();
            parameters = self.visit_nodes(v, parameter_list);
        }

        let body = self.transform_constructor_body(v, container, constructor, is_derived_class);
        let Some(body) = body else {
            if let Some(constructor) = constructor {
                return self.visit_each_child(v, constructor);
            }
            return None;
        };

        if let Some(constructor) = constructor {
            assert!(parameters.is_some(), "Debug failure. False expression.");
            return Some(v.factory_mut().update_constructor_declaration(
                constructor,
                None, /*modifiers*/
                None, /*typeParameters*/
                parameters,
                None, /*returnType*/
                None, /*fullSignature*/
                Some(body),
            ));
        }

        let f = v.factory_mut();
        let parameters = match parameters {
            Some(parameters) => parameters,
            None => new_node_list(f, Vec::new()),
        };

        let result = f.new_constructor_declaration(
            None, /*modifiers*/
            None, /*typeParameters*/
            Some(parameters),
            None, /*returnType*/
            None, /*fullSignature*/
            Some(body),
        );
        let loc = loc_of(f, container);
        f.set_node_range(result, loc);
        Some(result)
    }

    // port: tsc/internal/transformers/estransforms/classfields.go:classFieldsTransformer.transformConstructorBodyWorker
    #[allow(clippy::too_many_arguments)] // upstream's signature
    fn transform_constructor_body_worker(
        &self,
        v: &mut NodeVisitor<'_>,
        mut statements_out: Vec<NodeId>,
        statements_in: &[NodeId],
        statement_offset: usize,
        super_path: &[usize],
        super_path_depth: usize,
        initializer_statements: &[NodeId],
        constructor: NodeId,
    ) -> Vec<NodeId> {
        let super_statement_index = super_path[super_path_depth];
        let super_statement = statements_in[super_statement_index];

        // Visit statements before super
        let visited = self.visit_slice_with(
            v,
            Visitor::Main,
            &statements_in[statement_offset..super_statement_index],
        );
        statements_out.extend(visited);
        let mut statement_offset = super_statement_index + 1;

        if kind_of(v, super_statement) == K::TryStatement {
            let (try_block, catch_clause, finally_block) = {
                let read = v.node(super_statement);
                let data = read.as_try_statement().expect("TryStatement payload");
                (
                    data.try_block().expect(NIL),
                    data.catch_clause(),
                    data.finally_block(),
                )
            };
            let (try_statements, multi_line) = {
                let read = v.node(try_block);
                let data = read.as_block().expect("Block payload");
                (data.statements().expect(NIL), data.multi_line())
            };
            let try_statement_nodes = list_nodes(v.factory(), Some(try_statements));
            let try_block_statements = self.transform_constructor_body_worker(
                v,
                Vec::new(),
                &try_statement_nodes,
                0, /*statementOffset*/
                super_path,
                super_path_depth + 1,
                initializer_statements,
                constructor,
            );
            let f = v.factory_mut();
            let try_statement_list = new_node_list(f, try_block_statements);
            let loc = f.read_list(try_statements).loc();
            f.set_list_location(try_statement_list, loc);

            let catch_clause = self.visit_node(v, catch_clause);
            let finally_block = self.visit_node(v, finally_block);

            let f = v.factory_mut();
            let block = f.update_block(try_block, Some(try_statement_list), multi_line);
            let updated =
                f.update_try_statement(super_statement, Some(block), catch_clause, finally_block);
            statements_out.push(updated);
        } else {
            let visited = self.visit_slice_with(
                v,
                Visitor::Main,
                &statements_in[super_statement_index..=super_statement_index],
            );
            statements_out.extend(visited);

            // Add the property initializers. Transforms this:
            //
            //  public x = 1;
            //
            // Into this:
            //
            //  constructor() {
            //      this.x = 1;
            //  }
            //
            // If we do useDefineForClassFields, they'll be converted elsewhere.
            // We instead *remove* them from the transformed output at this stage.

            // parameter-property assignments should occur immediately after the prologue and `super()`,
            // so only count the statements that immediately follow.
            while statement_offset < statements_in.len() {
                let stmt = statements_in[statement_offset];
                let orig = self.context.most_original(stmt);
                if self.is_parameter_property_declaration(v.factory(), orig, constructor) {
                    statement_offset += 1;
                } else {
                    break;
                }
            }

            statements_out.extend_from_slice(initializer_statements);
        }

        // Visit remaining statements
        let visited = self.visit_slice_with(v, Visitor::Main, &statements_in[statement_offset..]);
        statements_out.extend(visited);
        statements_out
    }

    // port: tsc/internal/transformers/estransforms/classfields.go:classFieldsTransformer.transformConstructorBody
    fn transform_constructor_body(
        &self,
        v: &mut NodeVisitor<'_>,
        container: NodeId,
        constructor: Option<NodeId>,
        is_derived_class: bool,
    ) -> Option<NodeId> {
        let use_define_for_class_fields = self.compiler_options.use_define_for_class_fields();
        let instance_properties = self.get_properties(
            v.factory(),
            container,
            false, /*requireInitializer*/
            false, /*isStatic*/
        );
        let mut properties = instance_properties.clone();
        if !use_define_for_class_fields {
            properties.retain(|&prop| {
                let read = v.node(prop);
                read.initializer().is_some()
                    || read
                        .name()
                        .is_some_and(|name| kind_of(v, name) == K::PrivateIdentifier)
                    || self.has_accessor_modifier(v.factory(), prop)
            });
        }

        let private_methods_and_accessors =
            self.get_private_instance_methods_and_accessors(v.factory(), container);
        let needs_constructor_body =
            !properties.is_empty() || !private_methods_and_accessors.is_empty();

        let mut context = self.context.clone();

        // Only generate synthetic constructor when there are property initializers to move.
        if constructor.is_none() && !needs_constructor_body {
            return self.with(v, Visitor::Main, |m| context.visit_function_body(None, m));
        }

        context.start_variable_environment();

        let needs_synthetic_constructor = constructor.is_none() && is_derived_class;
        let mut statements = Vec::new();

        // Add the property initializers. Transforms this:
        //
        //  public x = 1;
        //
        // Into this:
        //
        //  constructor() {
        //      this.x = 1;
        //  }
        //
        let mut initializer_statements = Vec::new();
        let receiver = context.new_this_expression(v.factory_mut());

        // private methods can be called in property initializers, they should execute first
        initializer_statements = self.add_instance_method_statements(
            v,
            initializer_statements,
            &private_methods_and_accessors,
            receiver,
        );

        if let Some(constructor) = constructor {
            let parameter_properties: Vec<NodeId> = instance_properties
                .iter()
                .copied()
                .filter(|&prop| {
                    self.is_parameter_property_declaration(
                        v.factory(),
                        self.context.most_original(prop),
                        constructor,
                    )
                })
                .collect();
            let non_parameter_properties: Vec<NodeId> = properties
                .iter()
                .copied()
                .filter(|&prop| {
                    !self.is_parameter_property_declaration(
                        v.factory(),
                        self.context.most_original(prop),
                        constructor,
                    )
                })
                .collect();
            initializer_statements = self.add_property_or_class_static_block_statements(
                v,
                initializer_statements,
                &parameter_properties,
                receiver,
            );
            initializer_statements = self.add_property_or_class_static_block_statements(
                v,
                initializer_statements,
                &non_parameter_properties,
                receiver,
            );
        } else {
            initializer_statements = self.add_property_or_class_static_block_statements(
                v,
                initializer_statements,
                &properties,
                receiver,
            );
        }

        let constructor_body = constructor.and_then(|constructor| v.node(constructor).body());
        if let (Some(constructor), Some(body)) = (constructor, constructor_body) {
            let body_statements = v.node(body).statement_list();
            let body_statements = list_nodes(v.factory(), body_statements);

            // Copy prologue
            for &stmt in &body_statements {
                if self.q(v.factory(), |view| {
                    tsr_ast::utilities::is_prologue_directive(view, stmt)
                }) {
                    statements.push(stmt);
                } else {
                    break;
                }
            }
            let mut statement_offset = statements.len();

            let super_path =
                find_super_statement_index_path(v.factory(), &body_statements, statement_offset);
            if super_path.is_empty() {
                // parameter-property assignments should occur immediately after the prologue and `super()`,
                // so only count the statements that immediately follow.
                while statement_offset < body_statements.len() {
                    let stmt = body_statements[statement_offset];
                    let orig = self.context.most_original(stmt);
                    if self.is_parameter_property_declaration(v.factory(), orig, constructor) {
                        statement_offset += 1;
                    } else {
                        break;
                    }
                }
                statements.extend(initializer_statements);
                let visited =
                    self.visit_slice_with(v, Visitor::Main, &body_statements[statement_offset..]);
                statements.extend(visited);
            } else {
                statements = self.transform_constructor_body_worker(
                    v,
                    statements,
                    &body_statements,
                    statement_offset,
                    &super_path,
                    0,
                    &initializer_statements,
                    constructor,
                );
            }
        } else {
            if needs_synthetic_constructor {
                // Add a synthetic `super` call:
                //
                //  super(...arguments);
                //
                let f = v.factory_mut();
                let super_keyword = f.new_keyword_expression(K::SuperKeyword.into());
                let arguments_name = f.new_identifier(JsString::from_bytes(&b"arguments"[..]));
                let spread = f.new_spread_element(Some(arguments_name));
                let arguments = new_node_list(f, vec![spread]);
                let call = f.new_call_expression(
                    Some(super_keyword),
                    None, /*typeArguments*/
                    None, /*questionDotToken*/
                    Some(arguments),
                    node_flags::NONE,
                );
                let super_call = f.new_expression_statement(Some(call));
                statements.push(super_call);
            }
            statements.extend(initializer_statements);
        }

        let statements = context.end_and_merge_variable_environment(v.factory_mut(), statements);

        if statements.is_empty() && constructor.is_none() {
            return None;
        }

        let f = v.factory_mut();
        let body_parts = constructor_body.map(|body| {
            let read = f.node(body);
            let data = read.as_block().expect("Block payload");
            let list = data.statements().expect(NIL);
            (list, data.multi_line())
        });
        let multi_line = match body_parts {
            Some((list, multi_line))
                if f.read_nodes(f.read_list(list).nodes()).len() >= statements.len() =>
            {
                multi_line
            }
            _ => !statements.is_empty(),
        };

        let statement_list = new_node_list(f, statements);
        if let Some((list, _)) = body_parts {
            let loc = f.read_list(list).loc();
            f.set_list_location(statement_list, loc);
        } else {
            let member_list = f.node(container).member_list().expect(NIL);
            let loc = f.read_list(member_list).loc();
            f.set_list_location(statement_list, TextRange::new(loc.pos(), loc.end()));
        }

        let block = f.new_block(Some(statement_list), multi_line);
        if let Some(body) = constructor_body {
            let loc = loc_of(f, body);
            f.set_node_range(block, loc);
        }
        Some(block)
    }

    /// Generates assignment statements for property initializers.
    // port: tsc/internal/transformers/estransforms/classfields.go:classFieldsTransformer.addPropertyOrClassStaticBlockStatements
    fn add_property_or_class_static_block_statements(
        &self,
        v: &mut NodeVisitor<'_>,
        mut statements: Vec<NodeId>,
        properties: &[NodeId],
        receiver: NodeId,
    ) -> Vec<NodeId> {
        for &property in properties {
            if self.is_static(v.factory(), property)
                && !self.should_transform_private_elements_or_class_static_blocks
            {
                continue;
            }
            let statement = self.transform_property_or_class_static_block(v, property, receiver);
            if let Some(statement) = statement {
                statements.push(statement);
            }
        }
        statements
    }

    // port: tsc/internal/transformers/estransforms/classfields.go:classFieldsTransformer.transformPropertyOrClassStaticBlock
    fn transform_property_or_class_static_block(
        &self,
        v: &mut NodeVisitor<'_>,
        property: NodeId,
        receiver: NodeId,
    ) -> Option<NodeId> {
        let expression = if kind_of(v, property) == K::ClassStaticBlockDeclaration {
            self.set_current_class_element_and(
                Some(property),
                Self::transform_class_static_block_declaration,
                v,
                property,
            )
        } else {
            self.transform_property(v, property, receiver)
        };
        let expression = expression?;

        let mut context = self.context.clone();
        let f = v.factory_mut();
        let statement = f.new_expression_statement(Some(expression));
        context.set_original(statement, property);
        context.add_emit_flags(
            statement,
            context.emit_flags(property) & emit_flags::NO_COMMENTS,
        );
        let property_loc = loc_of(f, property);
        context.set_comment_range(statement, property_loc);

        let property_original_node = context.most_original(property);
        if kind_of(f, property_original_node) == K::Parameter {
            let loc = loc_of(f, property_original_node);
            context.set_source_map_range(statement, loc);
            context.add_emit_flags(statement, emit_flags::NO_COMMENTS);
        } else {
            let range = move_range_past_modifiers(f, property);
            context.set_source_map_range(statement, range);
        }

        // `setOriginalNode` *copies* the `emitNode` from `property`, so now both
        // `statement` and `expression` have a copy of the synthesized comments.
        // Drop the comments from expression to avoid printing them twice.
        context.set_synthetic_leading_comments(expression, Vec::new());
        context.set_synthetic_trailing_comments(expression, Vec::new());

        // If the property was originally an auto-accessor, don't emit comments here since they will be attached to
        // the synthesized getter.
        if self.has_accessor_modifier(v.factory(), property_original_node) {
            context.add_emit_flags(statement, emit_flags::NO_COMMENTS);
        }

        Some(statement)
    }

    /// Generates assignment expressions for property initializers.
    // port: tsc/internal/transformers/estransforms/classfields.go:classFieldsTransformer.generateInitializedPropertyExpressionsOrClassStaticBlock
    fn generate_initialized_property_expressions_or_class_static_block(
        &self,
        v: &mut NodeVisitor<'_>,
        properties_or_class_static_blocks: &[NodeId],
        receiver: NodeId,
    ) -> Vec<NodeId> {
        let mut expressions = Vec::new();
        for &property in properties_or_class_static_blocks {
            let expression = if kind_of(v, property) == K::ClassStaticBlockDeclaration {
                self.set_current_class_element_and(
                    Some(property),
                    Self::transform_class_static_block_declaration,
                    v,
                    property,
                )
            } else {
                self.transform_property(v, property, receiver)
            };
            let Some(expression) = expression else {
                continue;
            };
            let mut context = self.context.clone();
            context.set_original_ex(expression, property, true /*allowOverwrite*/);
            context.assign_comment_and_source_map_ranges(v.factory(), expression, property);
            expressions.push(expression);
        }
        expressions
    }

    /// Transforms a property initializer into an assignment expression.
    // port: tsc/internal/transformers/estransforms/classfields.go:classFieldsTransformer.transformProperty
    fn transform_property(
        &self,
        v: &mut NodeVisitor<'_>,
        property: NodeId,
        receiver: NodeId,
    ) -> Option<NodeId> {
        let saved_current_class_element = self.current_class_element.get();
        let transformed = self.transform_property_worker(v, property, receiver);
        let mut context = self.context.clone();
        let is_static = self.has_static_modifier(v.factory(), property);
        if let Some(transformed) = transformed {
            if is_static {
                context.add_emit_flags(transformed, emit_flags::NO_LEXICAL_THIS);
            }
            if is_static
                && self
                    .lex_data()
                    .is_some_and(|data| data.facts != class_facts::NONE)
            {
                // capture the lexical environment for the member
                context.set_original(transformed, property);
                let name = name_of(v, property);
                let range = context.source_map_range(v.factory(), name);
                context.set_source_map_range(transformed, range);
            }
        }
        self.current_class_element.set(saved_current_class_element);
        transformed
    }

    // port: tsc/internal/transformers/estransforms/classfields.go:classFieldsTransformer.transformPropertyWorker
    fn transform_property_worker(
        &self,
        v: &mut NodeVisitor<'_>,
        property: NodeId,
        receiver: NodeId,
    ) -> Option<NodeId> {
        // We generate a name here in order to reuse the value cached by the relocated computed name expression (which uses the same generated name)
        let emit_assignment = !self.compiler_options.use_define_for_class_fields();
        let mut context = self.context.clone();

        let mut property = property;
        if self.is_named_evaluation_needing_assigned_name(v.factory(), property) {
            property = self.transform_named_evaluation(v, property, false, b"");
        }

        let mut property_name = name_of(v, property);
        if self.has_accessor_modifier(v.factory(), property) {
            property_name = context.new_generated_private_name_for_node_ex(
                v.factory_mut(),
                property_name,
                AutoGenerateOptions {
                    suffix: JsString::from_bytes(&b"_accessor_storage"[..]),
                    ..AutoGenerateOptions::default()
                },
            );
        } else if kind_of(v, property_name) == K::ComputedPropertyName
            && !is_simple_inlineable_expression(v.factory(), expression_of(v, property_name))
        {
            let f = v.factory_mut();
            let generated = context.new_generated_name_for_node(f, property_name);
            property_name = f.update_computed_property_name(property_name, Some(generated));
        }

        if self.has_static_modifier(v.factory(), property) {
            self.current_class_element.set(Some(property));
        }

        let initializer_node = v.node(property).initializer();
        if kind_of(v, property_name) == K::PrivateIdentifier
            && self.should_transform_class_element_to_weak_map(v.factory(), property)
        {
            let info = self.access_private_identifier(v.factory(), property_name);
            if let Some(info) = info {
                if info.kind == PrivateIdentifierKind::Field {
                    if !info.is_static {
                        let initializer = self.visit_node(v, initializer_node);
                        return Some(create_private_instance_field_initializer(
                            &self.context,
                            v.factory_mut(),
                            receiver,
                            initializer,
                            info.brand_check_identifier.expect(NIL),
                        ));
                    }
                    let initializer = self.visit_node(v, initializer_node);
                    return Some(create_private_static_field_initializer(
                        &self.context,
                        v.factory_mut(),
                        info.variable_name.expect(NIL),
                        initializer,
                    ));
                }
                return None;
            }
            panic!("Debug failure. Undeclared private name for property declaration.");
        }

        if (kind_of(v, property_name) == K::PrivateIdentifier
            || self.has_static_modifier(v.factory(), property))
            && initializer_node.is_none()
        {
            return None;
        }

        // TODO: can we get rid of this original checking and better coordinate with runtimesyntax?
        let property_original_node = context.most_original(property);
        if self.q(v.factory(), |view| {
            tsr_ast::utilities_class::has_abstract_modifier(view, property_original_node)
        }) {
            return None;
        }

        let mut initializer = self.visit_node(v, initializer_node);
        let original_parent = v.node(property_original_node).parent();
        let is_parameter_property = original_parent.is_some_and(|parent| {
            self.is_parameter_property_declaration(v.factory(), property_original_node, parent)
        });
        if is_parameter_property && kind_of(v, property_name) == K::Identifier {
            // A parameter-property declaration always overrides the initializer. The only time a parameter-property
            // declaration *should* have an initializer is when decorators have added initializers that need to run before
            // any other initializer
            let local_name = clone_node(v.factory_mut(), property_name);
            if let Some(mut init) = initializer {
                // unwrap `(__runInitializers(this, _instanceExtraInitializers), void 0)`
                if kind_of(v, init) == K::ParenthesizedExpression {
                    let inner = expression_of(v, init);
                    let is_comma = kind_of(v, inner) == K::BinaryExpression
                        && binary_parts(v, inner).1 == K::CommaToken;
                    if is_comma {
                        let (left, _, _, right) = binary_parts(v, inner);
                        if context.is_call_to_helper(v.factory(), left, b"__runInitializers")
                            && kind_of(v, right) == K::VoidExpression
                            && kind_of(v, expression_of(v, right)) == K::NumericLiteral
                        {
                            init = left;
                        }
                    }
                }
                initializer = context.inline_expressions(v.factory_mut(), &[init, local_name]);
            } else {
                initializer = Some(local_name);
            }
            context.add_emit_flags(
                property_name,
                emit_flags::NO_COMMENTS | emit_flags::NO_SOURCE_MAP,
            );
            let original_name_loc = loc_of(v, name_of(v, property_original_node));
            context.set_source_map_range(local_name, original_name_loc);
            context.add_emit_flags(local_name, emit_flags::NO_COMMENTS);
        } else if initializer.is_none() {
            initializer = Some(context.new_void_zero_expression(v.factory_mut()));
        }
        let initializer = initializer.expect(NIL);

        if emit_assignment || kind_of(v, property_name) == K::PrivateIdentifier {
            let f = v.factory_mut();
            let member_access = create_member_access_for_property_name(
                f,
                &self.context,
                receiver,
                property_name,
                property_name,
            );
            context.add_emit_flags(member_access, emit_flags::NO_LEADING_COMMENTS);
            return Some(context.new_assignment_expression(f, member_access, initializer));
        }

        // useDefineForClassFields: Object.defineProperty
        let f = v.factory_mut();
        let name = match kind_of(f, property_name).known() {
            Some(K::ComputedPropertyName) => expression_of(f, property_name),
            Some(K::Identifier) => {
                let text = identifier_text(f, property_name);
                f.new_string_literal(text, token_flags::NONE)
            }
            _ => property_name,
        };
        let property_assignment = |f: &mut dyn RuntimeFactory, key: &'static [u8], value| {
            let key = f.new_identifier(JsString::from_bytes(key));
            f.new_property_assignment(None, Some(key), None, None, Some(value))
        };
        let enumerable_value = context.new_true_expression(f);
        let enumerable = property_assignment(f, b"enumerable", enumerable_value);
        let configurable_value = context.new_true_expression(f);
        let configurable = property_assignment(f, b"configurable", configurable_value);
        let writable_value = context.new_true_expression(f);
        let writable = property_assignment(f, b"writable", writable_value);
        let value = property_assignment(f, b"value", initializer);
        let properties = new_node_list(f, vec![enumerable, configurable, writable, value]);
        let descriptor = f.new_object_literal_expression(Some(properties), true);
        Some(context.new_object_define_property_call(f, receiver, name, descriptor))
    }

    /// Generates the brand-check initializer of private methods.
    // port: tsc/internal/transformers/estransforms/classfields.go:classFieldsTransformer.addInstanceMethodStatements
    fn add_instance_method_statements(
        &self,
        v: &mut NodeVisitor<'_>,
        mut statements: Vec<NodeId>,
        methods: &[NodeId],
        receiver: NodeId,
    ) -> Vec<NodeId> {
        if !self.should_transform_private_elements_or_class_static_blocks || methods.is_empty() {
            return statements;
        }

        let weak_set_name = self.get_private_identifier_environment(|env| env.data.weak_set_name);
        let weak_set_name = weak_set_name.unwrap_or_else(|| {
            panic!(
                "Debug failure. False expression: weakSetName should be set in private identifier environment"
            )
        });

        let f = v.factory_mut();
        let initializer =
            create_private_instance_method_initializer(&self.context, f, receiver, weak_set_name);
        statements.push(f.new_expression_statement(Some(initializer)));
        statements
    }

    // port: tsc/internal/transformers/estransforms/classfields.go:classFieldsTransformer.visitInvalidSuperProperty
    fn visit_invalid_super_property(&self, v: &mut NodeVisitor<'_>, node: NodeId) -> NodeId {
        let flags = v.node(node).flags();
        if kind_of(v, node) == K::PropertyAccessExpression {
            let name = v.node(node).name();
            let f = v.factory_mut();
            let void_zero = self.context.new_void_zero_expression(f);
            return f.update_property_access_expression(node, Some(void_zero), None, name, flags);
        }
        let argument = v
            .node(node)
            .as_element_access_expression()
            .expect("ElementAccessExpression payload")
            .argument_expression();
        let void_zero = self.context.new_void_zero_expression(v.factory_mut());
        let argument = self.visit_node(v, argument);
        v.factory_mut().update_element_access_expression(
            node,
            Some(void_zero),
            None,
            argument,
            flags,
        )
    }

    /// Transforms a computed property name, then returns either an expression
    /// that caches its value or the expression itself when the value is
    /// unused or safe to inline into several locations. `should_hoist`: the
    /// expression is reused (for an initializer or a decorator).
    // port: tsc/internal/transformers/estransforms/classfields.go:classFieldsTransformer.getPropertyNameExpressionIfNeeded
    fn get_property_name_expression_if_needed(
        &self,
        v: &mut NodeVisitor<'_>,
        name: NodeId,
        should_hoist: bool,
    ) -> Option<NodeId> {
        if kind_of(v, name) != K::ComputedPropertyName {
            return None;
        }
        let cache_assignment =
            find_computed_property_name_cache_assignment(&self.context, v.factory(), name);
        // Switch to outer lex env for computed property name expressions, matching
        // Strada reference's onEmitNode behavior for ComputedPropertyName.
        let saved_lexical_environment = self.lexical_environment.borrow().clone();
        let saved_inside_computed_property_name = self.inside_computed_property_name.get();
        self.inside_computed_property_name.set(true);
        if let Some(previous) = saved_lexical_environment
            .as_ref()
            .and_then(|environment| environment.previous.clone())
        {
            *self.lexical_environment.borrow_mut() = Some(previous);
        }
        let expression = self.visit_node(v, Some(expression_of(v, name))).expect(NIL);
        *self.lexical_environment.borrow_mut() = saved_lexical_environment;
        self.inside_computed_property_name
            .set(saved_inside_computed_property_name);
        let inner_expression = self.q_node(v.factory(), expression, |view| {
            tsr_ast::skip_partially_emitted_expressions(view, expression)
        });
        let inlinable = is_simple_inlineable_expression(v.factory(), inner_expression);
        let already_transformed = cache_assignment.is_some()
            || (is_assignment_expression(
                v.factory(),
                inner_expression,
                true, /*excludeCompoundAssignment*/
            ) && {
                let (left, _, _, _) = binary_parts(v, inner_expression);
                kind_of(v, left) == K::Identifier && is_generated_identifier(&self.context, left)
            });
        if !already_transformed && !inlinable && should_hoist {
            let mut context = self.context.clone();
            let generated_name = context.new_generated_name_for_node(v.factory_mut(), name);
            if self.requires_block_scoped_var(v.factory()) {
                context.add_lexical_declaration(v.factory_mut(), generated_name);
            } else {
                context.add_variable_declaration(v.factory_mut(), generated_name);
            }
            return Some(context.new_assignment_expression(
                v.factory_mut(),
                generated_name,
                expression,
            ));
        }
        if inlinable || kind_of(v, inner_expression) == K::Identifier {
            return None;
        }
        Some(expression)
    }

    // port: tsc/internal/transformers/estransforms/classfields.go:classFieldsTransformer.startClassLexicalEnvironment
    fn start_class_lexical_environment(&self) {
        let previous = self.lexical_environment.borrow().clone();
        *self.lexical_environment.borrow_mut() = Some(Rc::new(ClassLexicalEnv {
            previous,
            ..ClassLexicalEnv::default()
        }));
    }

    // port: tsc/internal/transformers/estransforms/classfields.go:classFieldsTransformer.endClassLexicalEnvironment
    fn end_class_lexical_environment(&self) {
        let previous = self
            .lexical_environment
            .borrow()
            .as_ref()
            .expect(NIL)
            .previous
            .clone();
        *self.lexical_environment.borrow_mut() = previous;
    }

    /// The current environment, which upstream asserts is present.
    fn current_lexical_environment(&self) -> Rc<ClassLexicalEnv> {
        self.lexical_environment
            .borrow()
            .clone()
            .unwrap_or_else(|| panic!("Debug failure. False expression."))
    }

    /// The class data of the current environment, created when absent.
    // port: tsc/internal/transformers/estransforms/classfields.go:classFieldsTransformer.getClassLexicalEnvironment
    fn get_class_lexical_environment(&self) -> ClassLexicalEnvironment {
        let environment = self.current_lexical_environment();
        let data = environment.data.get().unwrap_or_default();
        environment.data.set(Some(data));
        data
    }

    /// A write through the pointer `getClassLexicalEnvironment` returns.
    fn update_class_lexical_environment(&self, update: impl FnOnce(&mut ClassLexicalEnvironment)) {
        let environment = self.current_lexical_environment();
        let mut data = environment.data.get().unwrap_or_default();
        update(&mut data);
        environment.data.set(Some(data));
    }

    /// `tx.lexicalEnvironment.data` when both are present.
    fn lex_data(&self) -> Option<ClassLexicalEnvironment> {
        self.lexical_environment
            .borrow()
            .as_ref()
            .and_then(|environment| environment.data.get())
    }

    /// Runs `f` on the private environment of the current environment,
    /// created when absent.
    // port: tsc/internal/transformers/estransforms/classfields.go:classFieldsTransformer.getPrivateIdentifierEnvironment
    fn get_private_identifier_environment<R>(
        &self,
        f: impl FnOnce(&mut PrivateEnvironment) -> R,
    ) -> R {
        let environment = self.current_lexical_environment();
        let mut private_env = environment.private_env.borrow_mut();
        f(private_env.get_or_insert_with(PrivateEnvironment::default))
    }

    // port: tsc/internal/transformers/estransforms/classfields.go:classFieldsTransformer.addPendingExpressions
    fn add_pending_expressions(&self, exprs: &[NodeId]) {
        self.pending_expressions
            .borrow_mut()
            .extend_from_slice(exprs);
    }

    // port: tsc/internal/transformers/estransforms/classfields.go:classFieldsTransformer.addPrivateIdentifierPropertyDeclarationToEnvironment
    fn add_private_identifier_property_declaration_to_environment(
        &self,
        v: &mut NodeVisitor<'_>,
        node: NodeId,
        name: NodeId,
    ) {
        let lex = self.get_class_lexical_environment();
        let is_static = self.has_static_modifier(v.factory(), node);
        let previous_info = self.get_private_identifier_environment(|env| {
            self.get_private_identifier(v.factory(), env, name)
        });
        let is_valid = !self.is_reserved_private_name(v.factory(), name) && previous_info.is_none();

        if is_static {
            let brand_check_identifier = lex.class_this.or(lex.class_constructor);
            let variable_name = self.create_hoisted_variable_for_private_name(v, name, b"");
            self.get_private_identifier_environment(|env| {
                self.set_private_identifier(
                    v.factory(),
                    env,
                    name,
                    PrivateIdentifierInfo {
                        is_static: true,
                        brand_check_identifier,
                        variable_name: Some(variable_name),
                        is_valid,
                        ..PrivateIdentifierInfo::new(PrivateIdentifierKind::Field)
                    },
                );
            });
        } else {
            let weak_map_name = self.create_hoisted_variable_for_private_name(v, name, b"");
            self.get_private_identifier_environment(|env| {
                self.set_private_identifier(
                    v.factory(),
                    env,
                    name,
                    PrivateIdentifierInfo {
                        is_static: false,
                        brand_check_identifier: Some(weak_map_name),
                        is_valid,
                        ..PrivateIdentifierInfo::new(PrivateIdentifierKind::Field)
                    },
                );
            });
            let f = v.factory_mut();
            let weak_map = f.new_identifier(JsString::from_bytes(&b"WeakMap"[..]));
            let arguments = new_node_list(f, Vec::new());
            let new_expression =
                f.new_new_expression(Some(weak_map), None /*typeArguments*/, Some(arguments));
            let assignment =
                self.context
                    .new_assignment_expression(f, weak_map_name, new_expression);
            self.add_pending_expressions(&[assignment]);
        }
    }

    /// The brand check of a static (`classThis` or the class constructor) or
    /// instance (the WeakSet) private method or accessor.
    fn method_brand_check_identifier(
        lex: ClassLexicalEnvironment,
        weak_set_name: Option<NodeId>,
        is_static: bool,
        check_weak_set: bool,
    ) -> Option<NodeId> {
        if is_static {
            let brand_check_identifier = lex.class_this.or(lex.class_constructor);
            assert!(
                brand_check_identifier.is_some(),
                "Debug failure. False expression: classConstructor should be set in private identifier environment"
            );
            return brand_check_identifier;
        }
        if check_weak_set {
            assert!(
                weak_set_name.is_some(),
                "Debug failure. False expression: weakSetName should be set in private identifier environment"
            );
        }
        weak_set_name
    }

    // port: tsc/internal/transformers/estransforms/classfields.go:classFieldsTransformer.addPrivateIdentifierMethodToEnvironment
    fn add_private_identifier_method_to_environment(
        &self,
        v: &mut NodeVisitor<'_>,
        name: NodeId,
        lex: ClassLexicalEnvironment,
        is_static: bool,
        is_valid: bool,
    ) {
        let method_name = self.create_hoisted_variable_for_private_name(v, name, b"");
        let weak_set_name = self.get_private_identifier_environment(|env| env.data.weak_set_name);
        let brand_check_identifier =
            Self::method_brand_check_identifier(lex, weak_set_name, is_static, false);
        self.get_private_identifier_environment(|env| {
            self.set_private_identifier(
                v.factory(),
                env,
                name,
                PrivateIdentifierInfo {
                    method_name: Some(method_name),
                    brand_check_identifier,
                    is_static,
                    is_valid,
                    ..PrivateIdentifierInfo::new(PrivateIdentifierKind::Method)
                },
            );
        });
    }

    // port: tsc/internal/transformers/estransforms/classfields.go:classFieldsTransformer.addPrivateIdentifierGetAccessorToEnvironment
    fn add_private_identifier_get_accessor_to_environment(
        &self,
        v: &mut NodeVisitor<'_>,
        name: NodeId,
        lex: ClassLexicalEnvironment,
        is_static: bool,
        is_valid: bool,
        previous_info: Option<PrivateIdentifierInfo>,
    ) {
        let getter_name = self.create_hoisted_variable_for_private_name(v, name, b"_get");
        let weak_set_name = self.get_private_identifier_environment(|env| env.data.weak_set_name);
        let brand_check_identifier =
            Self::method_brand_check_identifier(lex, weak_set_name, is_static, true);

        self.get_private_identifier_environment(|env| {
            if let Some(previous) = previous_info.filter(|previous| {
                previous.kind == PrivateIdentifierKind::Accessor
                    && previous.is_static == is_static
                    && previous.getter_name.is_none()
            }) {
                self.set_private_identifier(
                    v.factory(),
                    env,
                    name,
                    PrivateIdentifierInfo {
                        getter_name: Some(getter_name),
                        ..previous
                    },
                );
            } else {
                self.set_private_identifier(
                    v.factory(),
                    env,
                    name,
                    PrivateIdentifierInfo {
                        getter_name: Some(getter_name),
                        brand_check_identifier,
                        is_static,
                        is_valid,
                        ..PrivateIdentifierInfo::new(PrivateIdentifierKind::Accessor)
                    },
                );
            }
        });
    }

    // port: tsc/internal/transformers/estransforms/classfields.go:classFieldsTransformer.addPrivateIdentifierSetAccessorToEnvironment
    fn add_private_identifier_set_accessor_to_environment(
        &self,
        v: &mut NodeVisitor<'_>,
        name: NodeId,
        lex: ClassLexicalEnvironment,
        is_static: bool,
        is_valid: bool,
        previous_info: Option<PrivateIdentifierInfo>,
    ) {
        let setter_name = self.create_hoisted_variable_for_private_name(v, name, b"_set");
        let weak_set_name = self.get_private_identifier_environment(|env| env.data.weak_set_name);
        let brand_check_identifier =
            Self::method_brand_check_identifier(lex, weak_set_name, is_static, true);

        self.get_private_identifier_environment(|env| {
            if let Some(previous) = previous_info.filter(|previous| {
                previous.kind == PrivateIdentifierKind::Accessor
                    && previous.is_static == is_static
                    && previous.setter_name.is_none()
            }) {
                self.set_private_identifier(
                    v.factory(),
                    env,
                    name,
                    PrivateIdentifierInfo {
                        setter_name: Some(setter_name),
                        ..previous
                    },
                );
            } else {
                self.set_private_identifier(
                    v.factory(),
                    env,
                    name,
                    PrivateIdentifierInfo {
                        setter_name: Some(setter_name),
                        brand_check_identifier,
                        is_static,
                        is_valid,
                        ..PrivateIdentifierInfo::new(PrivateIdentifierKind::Accessor)
                    },
                );
            }
        });
    }

    // port: tsc/internal/transformers/estransforms/classfields.go:classFieldsTransformer.addPrivateIdentifierAutoAccessorToEnvironment
    fn add_private_identifier_auto_accessor_to_environment(
        &self,
        v: &mut NodeVisitor<'_>,
        name: NodeId,
        lex: ClassLexicalEnvironment,
        is_static: bool,
        is_valid: bool,
    ) {
        let getter_name = self.create_hoisted_variable_for_private_name(v, name, b"_get");
        let setter_name = self.create_hoisted_variable_for_private_name(v, name, b"_set");
        let weak_set_name = self.get_private_identifier_environment(|env| env.data.weak_set_name);
        let brand_check_identifier =
            Self::method_brand_check_identifier(lex, weak_set_name, is_static, true);

        self.get_private_identifier_environment(|env| {
            self.set_private_identifier(
                v.factory(),
                env,
                name,
                PrivateIdentifierInfo {
                    getter_name: Some(getter_name),
                    setter_name: Some(setter_name),
                    brand_check_identifier,
                    is_static,
                    is_valid,
                    ..PrivateIdentifierInfo::new(PrivateIdentifierKind::Accessor)
                },
            );
        });
    }

    // port: tsc/internal/transformers/estransforms/classfields.go:classFieldsTransformer.addPrivateIdentifierToEnvironment
    fn add_private_identifier_to_environment(&self, v: &mut NodeVisitor<'_>, node: NodeId) {
        let lex = self.get_class_lexical_environment();
        let name = name_of(v, node);
        let is_static = self.has_static_modifier(v.factory(), node);
        let previous_info = self.get_private_identifier_environment(|env| {
            self.get_private_identifier(v.factory(), env, name)
        });
        let is_valid = !self.is_reserved_private_name(v.factory(), name) && previous_info.is_none();

        let kind = kind_of(v, node);
        if self.is_auto_accessor_property_declaration(v.factory(), node) {
            self.add_private_identifier_auto_accessor_to_environment(
                v, name, lex, is_static, is_valid,
            );
        } else if kind == K::PropertyDeclaration {
            self.add_private_identifier_property_declaration_to_environment(v, node, name);
        } else if kind == K::MethodDeclaration {
            self.add_private_identifier_method_to_environment(v, name, lex, is_static, is_valid);
        } else if kind == K::GetAccessor {
            self.add_private_identifier_get_accessor_to_environment(
                v,
                name,
                lex,
                is_static,
                is_valid,
                previous_info,
            );
        } else if kind == K::SetAccessor {
            self.add_private_identifier_set_accessor_to_environment(
                v,
                name,
                lex,
                is_static,
                is_valid,
                previous_info,
            );
        }
    }

    // port: tsc/internal/transformers/estransforms/classfields.go:classFieldsTransformer.setPrivateIdentifier
    fn set_private_identifier(
        &self,
        factory: &dyn RuntimeFactory,
        env: &mut PrivateEnvironment,
        name: NodeId,
        info: PrivateIdentifierInfo,
    ) {
        if self.context.has_auto_generate_info(name) {
            let key = self.context.node_for_generated_name(factory, name);
            env.generated_identifiers
                .get_or_insert_with(HashMap::new)
                .insert(key, info);
        } else {
            env.members.insert(identifier_text(factory, name), info);
        }
    }

    // port: tsc/internal/transformers/estransforms/classfields.go:classFieldsTransformer.getPrivateIdentifier
    fn get_private_identifier(
        &self,
        factory: &dyn RuntimeFactory,
        env: &PrivateEnvironment,
        name: NodeId,
    ) -> Option<PrivateIdentifierInfo> {
        if self.context.has_auto_generate_info(name) {
            let key = self.context.node_for_generated_name(factory, name);
            return env
                .generated_identifiers
                .as_ref()
                .and_then(|identifiers| identifiers.get(&key).copied());
        }
        env.members.get(&identifier_text(factory, name)).copied()
    }

    /// Records `identifier` in the current `let` or `var` environment.
    fn add_hoisted_variable(&self, v: &mut NodeVisitor<'_>, identifier: NodeId) {
        let mut context = self.context.clone();
        if self.requires_block_scoped_var(v.factory()) {
            context.add_lexical_declaration(v.factory_mut(), identifier);
        } else {
            context.add_variable_declaration(v.factory_mut(), identifier);
        }
    }

    /// `"_" + className + "_"`, or `"_"` without a class name.
    fn hoisted_variable_prefix(&self, factory: &dyn RuntimeFactory) -> Vec<u8> {
        let class_name = self.get_private_identifier_environment(|env| env.data.class_name);
        let mut prefix = vec![b'_'];
        if let Some(class_name) = class_name {
            prefix.extend_from_slice(identifier_text(factory, class_name).as_bytes());
            prefix.push(b'_');
        }
        prefix
    }

    // port: tsc/internal/transformers/estransforms/classfields.go:classFieldsTransformer.createHoistedVariableForClass
    fn create_hoisted_variable_for_class(
        &self,
        v: &mut NodeVisitor<'_>,
        name_text: &[u8],
        _node: NodeId,
        suffix: &[u8],
    ) -> NodeId {
        let mut text = self.hoisted_variable_prefix(v.factory());
        text.extend_from_slice(name_text);
        let identifier = self.context.clone().new_unique_name_ex(
            v.factory_mut(),
            JsString::from_bytes(text),
            AutoGenerateOptions {
                flags: g::OPTIMISTIC | g::RESERVED_IN_NESTED_SCOPES,
                suffix: JsString::from_bytes(suffix),
                ..AutoGenerateOptions::default()
            },
        );
        self.add_hoisted_variable(v, identifier);
        identifier
    }

    // port: tsc/internal/transformers/estransforms/classfields.go:classFieldsTransformer.createHoistedVariableForClassFromNode
    fn create_hoisted_variable_for_class_from_node(
        &self,
        v: &mut NodeVisitor<'_>,
        name: NodeId,
        suffix: &[u8],
    ) -> NodeId {
        let prefix = self.hoisted_variable_prefix(v.factory());
        let identifier = self.context.clone().new_generated_name_for_node_ex(
            v.factory_mut(),
            name,
            AutoGenerateOptions {
                flags: g::OPTIMISTIC | g::RESERVED_IN_NESTED_SCOPES,
                prefix: JsString::from_bytes(prefix),
                suffix: JsString::from_bytes(suffix),
            },
        );
        self.add_hoisted_variable(v, identifier);
        identifier
    }

    // port: tsc/internal/transformers/estransforms/classfields.go:classFieldsTransformer.createHoistedVariableForPrivateName
    fn create_hoisted_variable_for_private_name(
        &self,
        v: &mut NodeVisitor<'_>,
        name: NodeId,
        suffix: &[u8],
    ) -> NodeId {
        // If the name is a generated identifier (e.g., auto-accessor backing field),
        // use node-based name generation so the emitter can resolve the name properly.
        if self.context.has_auto_generate_info(name) {
            return self.create_hoisted_variable_for_class_from_node(v, name, suffix);
        }
        let text = identifier_text(v.factory(), name);
        let mut text = text.as_bytes();
        if text.first() == Some(&b'#') {
            text = &text[1..]; // strip leading '#'
        }
        let text = text.to_vec();
        self.create_hoisted_variable_for_class(v, &text, name, suffix)
    }

    /// Accesses an already defined PrivateIdentifier in the current
    /// PrivateIdentifierEnvironment.
    // port: tsc/internal/transformers/estransforms/classfields.go:classFieldsTransformer.accessPrivateIdentifier
    fn access_private_identifier(
        &self,
        factory: &dyn RuntimeFactory,
        name: NodeId,
    ) -> Option<PrivateIdentifierInfo> {
        let mut environment = self.lexical_environment.borrow().clone();
        while let Some(env) = environment {
            if let Some(private_env) = env.private_env.borrow().as_ref() {
                if let Some(info) = self.get_private_identifier(factory, private_env, name) {
                    if info.kind == PrivateIdentifierKind::Untransformed {
                        return None;
                    }
                    return Some(info);
                }
            }
            environment = env.previous.as_ref().map(Rc::clone);
        }
        None
    }

    // port: tsc/internal/transformers/estransforms/classfields.go:classFieldsTransformer.wrapPrivateIdentifierForDestructuringTarget
    fn wrap_private_identifier_for_destructuring_target(
        &self,
        v: &mut NodeVisitor<'_>,
        node: NodeId,
    ) -> Option<NodeId> {
        let (expression, name) = {
            let read = v.node(node);
            (read.expression().expect(NIL), read.name().expect(NIL))
        };
        let mut context = self.context.clone();
        let parameter = context.new_generated_name_for_node(v.factory_mut(), node);
        let info = self.access_private_identifier(v.factory(), name);
        let Some(info) = info else {
            return self.visit_each_child(v, node);
        };
        let mut receiver = expression;
        // We cannot copy `this` or `super` into the function because they will be bound
        // differently inside the function.
        let expression_kind = kind_of(v, expression);
        let is_this_or_super_property =
            expression_kind == K::ThisKeyword || expression_kind == K::SuperKeyword;
        if is_this_or_super_property || !is_simple_copiable_expression(v.factory(), expression) {
            receiver = self.new_reserved_temp_variable(v);
            context.add_variable_declaration(v.factory_mut(), receiver);
            let visited = self.visit_node(v, Some(expression)).expect(NIL);
            let assignment = context.new_assignment_expression(v.factory_mut(), receiver, visited);
            self.pending_expressions.borrow_mut().push(assignment);
        }
        let assign_expr = self.create_private_identifier_assignment(
            v,
            info,
            receiver,
            parameter,
            K::EqualsToken.into(),
        );
        Some(context.new_assignment_target_wrapper(v.factory_mut(), parameter, assign_expr))
    }

    // port: tsc/internal/transformers/estransforms/classfields.go:classFieldsTransformer.visitAssignmentElement
    fn visit_assignment_element(&self, v: &mut NodeVisitor<'_>, node: NodeId) -> Option<NodeId> {
        // 13.15.5.5 RS: IteratorDestructuringAssignmentEvaluation
        //   AssignmentElement : DestructuringAssignmentTarget Initializer?
        //     ...
        //     4. If |Initializer| is present and _value_ is *undefined*, then
        //        a. If IsAnonymousFunctionDefinition(|Initializer|) and IsIdentifierRef of |DestructuringAssignmentTarget| are both *true*, then
        //           i. Let _v_ be ? NamedEvaluation of |Initializer| with argument _lref_.[[ReferencedName]].
        //     ...
        let mut node = node;
        if self.is_named_evaluation_needing_assigned_name(v.factory(), node) {
            node = self.transform_named_evaluation(
                v, node, false, /*ignoreEmptyStringLiteral*/
                b"",   /*assignedName*/
            );
        }
        if is_assignment_expression(v.factory(), node, true /*excludeCompoundAssignment*/) {
            let (left, _, operator_token, right) = binary_parts(v, node);
            let left = self.visit_destructuring_assignment_target(v, left);
            let right = self.visit_node(v, Some(right));
            return Some(v.factory_mut().update_binary_expression(
                node,
                None,
                left,
                None,
                Some(operator_token),
                right,
            ));
        }
        self.visit_destructuring_assignment_target(v, node)
    }

    /// Whether `node` is a left-hand-side expression.
    fn is_left_hand_side_expression(&self, factory: &dyn RuntimeFactory, node: NodeId) -> bool {
        self.q(factory, |view| {
            tsr_ast::is_left_hand_side_expression(view, node)
        })
    }

    // port: tsc/internal/transformers/estransforms/classfields.go:classFieldsTransformer.visitAssignmentRestElement
    fn visit_assignment_rest_element(
        &self,
        v: &mut NodeVisitor<'_>,
        node: NodeId,
    ) -> Option<NodeId> {
        let expression = expression_of(v, node);
        if self.is_left_hand_side_expression(v.factory(), expression) {
            let expr = self.visit_destructuring_assignment_target(v, expression);
            return Some(v.factory_mut().update_spread_element(node, expr));
        }
        self.visit_each_child(v, node)
    }

    // port: tsc/internal/transformers/estransforms/classfields.go:classFieldsTransformer.visitArrayAssignmentElement
    fn visit_array_assignment_element(
        &self,
        v: &mut NodeVisitor<'_>,
        node: NodeId,
    ) -> Option<NodeId> {
        if self.q(v.factory(), |view| {
            tsr_ast::utilities_positions::is_array_binding_or_assignment_element(view, node)
        }) {
            let kind = kind_of(v, node);
            if kind == K::SpreadElement {
                return self.visit_assignment_rest_element(v, node);
            }
            if kind != K::OmittedExpression {
                return self.visit_assignment_element(v, node);
            }
        }
        self.visit_each_child(v, node)
    }

    // port: tsc/internal/transformers/estransforms/classfields.go:classFieldsTransformer.visitAssignmentProperty
    fn visit_assignment_property(&self, v: &mut NodeVisitor<'_>, node: NodeId) -> Option<NodeId> {
        // AssignmentProperty : PropertyName `:` AssignmentElement
        // AssignmentElement : DestructuringAssignmentTarget Initializer?

        // 13.15.5.6 RS: KeyedDestructuringAssignmentEvaluation
        //   AssignmentElement : DestructuringAssignmentTarget Initializer?
        //     ...
        //     3. If |Initializer| is present and _v_ is *undefined*, then
        //        a. If IsAnonymousfunctionDefinition(|Initializer|) and IsIdentifierRef of |DestructuringAssignmentTarget| are both *true*, then
        //           i. Let _rhsValue_ be ? NamedEvaluation of |Initializer| with argument _lref_.[[ReferencedName]].
        //     ...
        let (name, init) = {
            let read = v.node(node);
            (read.name(), read.initializer().expect(NIL))
        };
        let name = self.visit_node(v, name);
        if is_assignment_expression(v.factory(), init, true /*excludeCompoundAssignment*/) {
            let assign_elem = self.visit_assignment_element(v, init);
            return Some(v.factory_mut().update_property_assignment(
                node,
                None,
                name,
                None,
                None,
                assign_elem,
            ));
        }
        if self.is_left_hand_side_expression(v.factory(), init) {
            let target = self.visit_destructuring_assignment_target(v, init);
            return Some(
                v.factory_mut()
                    .update_property_assignment(node, None, name, None, None, target),
            );
        }
        self.visit_each_child(v, node)
    }

    // port: tsc/internal/transformers/estransforms/classfields.go:classFieldsTransformer.visitShorthandAssignmentProperty
    fn visit_shorthand_assignment_property(
        &self,
        v: &mut NodeVisitor<'_>,
        node: NodeId,
    ) -> Option<NodeId> {
        // AssignmentProperty : IdentifierReference Initializer?

        // 13.15.5.3 RS: PropertyDestructuringAssignmentEvaluation
        //   AssignmentProperty : IdentifierReference Initializer?
        //     ...
        //     4. If |Initializer?| is present and _v_ is *undefined*, then
        //        a. If IsAnonymousFunctionDefinition(|Initializer|) is *true*, then
        //           i. Set _v_ to ? NamedEvaluation of |Initializer| with argument _P_.
        //     ...
        let mut node = node;
        if self.is_named_evaluation_needing_assigned_name(v.factory(), node) {
            node = self.transform_named_evaluation(
                v, node, false, /*ignoreEmptyStringLiteral*/
                b"",   /*assignedName*/
            );
        }
        self.visit_each_child(v, node)
    }

    // port: tsc/internal/transformers/estransforms/classfields.go:classFieldsTransformer.visitAssignmentRestProperty
    fn visit_assignment_rest_property(
        &self,
        v: &mut NodeVisitor<'_>,
        node: NodeId,
    ) -> Option<NodeId> {
        let expression = expression_of(v, node);
        if self.is_left_hand_side_expression(v.factory(), expression) {
            let expr = self.visit_destructuring_assignment_target(v, expression);
            return Some(v.factory_mut().update_spread_assignment(node, expr));
        }
        self.visit_each_child(v, node)
    }

    // port: tsc/internal/transformers/estransforms/classfields.go:classFieldsTransformer.visitObjectAssignmentElement
    fn visit_object_assignment_element(
        &self,
        v: &mut NodeVisitor<'_>,
        node: NodeId,
    ) -> Option<NodeId> {
        assert!(
            tsr_ast::utilities::is_object_binding_or_assignment_element(&v.node(node)),
            "Debug failure. False expression."
        );
        match kind_of(v, node).known() {
            Some(K::SpreadAssignment) => self.visit_assignment_rest_property(v, node),
            Some(K::ShorthandPropertyAssignment) => {
                self.visit_shorthand_assignment_property(v, node)
            }
            Some(K::PropertyAssignment) => self.visit_assignment_property(v, node),
            _ => self.visit_each_child(v, node),
        }
    }

    // port: tsc/internal/transformers/estransforms/classfields.go:classFieldsTransformer.visitAssignmentPattern
    #[allow(clippy::unnecessary_wraps)] // a visit function; upstream's result is a nullable node
    fn visit_assignment_pattern(&self, v: &mut NodeVisitor<'_>, node: NodeId) -> Option<NodeId> {
        if kind_of(v, node) == K::ArrayLiteralExpression {
            // Transforms private names in destructuring assignment array bindings.
            // Transforms SuperProperty assignments in destructuring assignment array bindings in static initializers.
            //
            // Source:
            // ([ this.#myProp ] = [ "hello" ]);
            //
            // Transformation:
            // [ { set value(x) { this.#myProp = x; } }.value ] = [ "hello" ];
            let (elements, multi_line) = {
                let read = v.node(node);
                let data = read
                    .as_array_literal_expression()
                    .expect("ArrayLiteralExpression payload");
                (data.elements(), data.multi_line())
            };
            let elements = self.with(v, Visitor::ArrayAssignmentElement, |a| {
                a.visit_nodes(elements)
            });
            return Some(
                v.factory_mut()
                    .update_array_literal_expression(node, elements, multi_line),
            );
        }
        // Transforms private names in destructuring assignment object bindings.
        // Transforms SuperProperty assignments in destructuring assignment object bindings in static initializers.
        //
        // Source:
        // ({ stringProperty: this.#myProp } = { stringProperty: "hello" });
        //
        // Transformation:
        // ({ stringProperty: { set value(x) { this.#myProp = x; } }.value }) = { stringProperty: "hello" };
        let (properties, multi_line) = {
            let read = v.node(node);
            let data = read
                .as_object_literal_expression()
                .expect("ObjectLiteralExpression payload");
            (data.properties(), data.multi_line())
        };
        let properties = self.with(v, Visitor::ObjectAssignmentElement, |o| {
            o.visit_nodes(properties)
        });
        Some(
            v.factory_mut()
                .update_object_literal_expression(node, properties, multi_line),
        )
    }

    // port: tsc/internal/transformers/estransforms/classfields.go:classFieldsTransformer.isReservedPrivateName
    fn is_reserved_private_name(&self, factory: &dyn RuntimeFactory, node: NodeId) -> bool {
        !(kind_of(factory, node) == K::PrivateIdentifier
            && self.context.has_auto_generate_info(node))
            && identifier_text(factory, node).as_bytes() == b"#constructor"
    }

    /// `isStaticPropertyDeclarationOrClassStaticBlock`.
    fn is_static_property_declaration_or_class_static_block(
        &self,
        factory: &dyn RuntimeFactory,
        node: NodeId,
    ) -> bool {
        self.q(factory, |view| {
            is_static_property_declaration_or_class_static_block(view, node)
        })
    }

    // port: tsc/internal/transformers/estransforms/classfields.go:classFieldsTransformer.getProperties
    fn get_properties(
        &self,
        factory: &dyn RuntimeFactory,
        node: NodeId,
        require_initializer: bool,
        is_static: bool,
    ) -> Vec<NodeId> {
        let mut result = Vec::new();
        for member in members_of(factory, node) {
            if kind_of(factory, member) == K::PropertyDeclaration
                && (!require_initializer || factory.node(member).initializer().is_some())
                && self.has_static_modifier(factory, member) == is_static
            {
                result.push(member);
            }
        }
        result
    }

    // port: tsc/internal/transformers/estransforms/classfields.go:classFieldsTransformer.getStaticPropertiesAndClassStaticBlock
    fn get_static_properties_and_class_static_block(
        &self,
        factory: &dyn RuntimeFactory,
        node: NodeId,
    ) -> Vec<NodeId> {
        let mut result = Vec::new();
        for member in members_of(factory, node) {
            let kind = kind_of(factory, member);
            if kind == K::ClassStaticBlockDeclaration
                || (kind == K::PropertyDeclaration && self.has_static_modifier(factory, member))
            {
                result.push(member);
            }
        }
        result
    }

    // port: tsc/internal/transformers/estransforms/classfields.go:classFieldsTransformer.createCallBinding
    fn create_call_binding(&self, v: &mut NodeVisitor<'_>, node: NodeId) -> (NodeId, NodeId) {
        let mut context = self.context.clone();
        if is_super_property(v.factory(), node) {
            return (context.new_this_expression(v.factory_mut()), node);
        }
        if kind_of(v, node) == K::PropertyAccessExpression {
            let (expression, name) = {
                let read = v.node(node);
                (read.expression().expect(NIL), read.name())
            };
            let captured = self.q(v.factory(), |view| {
                should_be_captured_in_temp_variable(view, expression)
            });
            if captured {
                let f = v.factory_mut();
                let this_arg = context.new_temp_variable(f);
                context.add_variable_declaration(f, this_arg);
                let assignment = context.new_assignment_expression(f, this_arg, expression);
                // TODO: do we even need these?
                let parenthesized = f.new_parenthesized_expression(Some(assignment));
                let target = f.new_property_access_expression(
                    Some(parenthesized),
                    None,
                    name,
                    node_flags::NONE,
                );
                return (this_arg, target);
            }
            return (expression, node);
        }
        let this_arg = context.new_void_zero_expression(v.factory_mut());
        (this_arg, node)
    }

    // port: tsc/internal/transformers/estransforms/classfields.go:classFieldsTransformer.createAccessorPropertyGetRedirector
    fn create_accessor_property_get_redirector(
        &self,
        v: &mut NodeVisitor<'_>,
        node: NodeId,
        modifiers: Option<NodeListId>,
        name: NodeId,
        receiver: NodeId,
    ) -> NodeId {
        let node_name = name_of(v, node);
        let f = v.factory_mut();
        let backing_field_name = self.context.clone().new_generated_private_name_for_node_ex(
            f,
            node_name,
            AutoGenerateOptions {
                suffix: JsString::from_bytes(&b"_accessor_storage"[..]),
                ..AutoGenerateOptions::default()
            },
        );
        let return_expr = f.new_property_access_expression(
            Some(receiver),
            None,
            Some(backing_field_name),
            node_flags::NONE,
        );
        let return_stmt = f.new_return_statement(Some(return_expr));
        let statements = new_node_list(f, vec![return_stmt]);
        let body = f.new_block(Some(statements), false);
        let parameters = new_node_list(f, Vec::new());
        f.new_get_accessor_declaration(
            modifiers,
            Some(name),
            None, /*typeParameters*/
            Some(parameters),
            None, /*returnType*/
            None, /*fullSignature*/
            Some(body),
        )
    }

    // port: tsc/internal/transformers/estransforms/classfields.go:classFieldsTransformer.createAccessorPropertySetRedirector
    fn create_accessor_property_set_redirector(
        &self,
        v: &mut NodeVisitor<'_>,
        node: NodeId,
        modifiers: Option<NodeListId>,
        name: NodeId,
        receiver: NodeId,
    ) -> NodeId {
        let node_name = name_of(v, node);
        let f = v.factory_mut();
        let backing_field_name = self.context.clone().new_generated_private_name_for_node_ex(
            f,
            node_name,
            AutoGenerateOptions {
                suffix: JsString::from_bytes(&b"_accessor_storage"[..]),
                ..AutoGenerateOptions::default()
            },
        );
        let value = f.new_identifier(JsString::from_bytes(&b"value"[..]));
        let value_param = f.new_parameter_declaration(
            None, /*modifiers*/
            None, /*dotDotDotToken*/
            Some(value),
            None, /*questionToken*/
            None, /*typeNode*/
            None, /*initializer*/
        );
        let target = f.new_property_access_expression(
            Some(receiver),
            None,
            Some(backing_field_name),
            node_flags::NONE,
        );
        let value = f.new_identifier(JsString::from_bytes(&b"value"[..]));
        let assign_expr = self.context.new_assignment_expression(f, target, value);
        let expr_stmt = f.new_expression_statement(Some(assign_expr));
        let statements = new_node_list(f, vec![expr_stmt]);
        let body = f.new_block(Some(statements), false);
        let parameters = new_node_list(f, vec![value_param]);
        f.new_set_accessor_declaration(
            modifiers,
            Some(name),
            None, /*typeParameters*/
            Some(parameters),
            None, /*returnType*/
            None, /*fullSignature*/
            Some(body),
        )
    }
}

/// The immediate children of `node`, in `ForEachChild` order.
fn immediate_children(factory: &dyn RuntimeFactory, node: NodeId) -> Vec<NodeId> {
    struct Children<'f> {
        factory: &'f dyn RuntimeFactory,
        nodes: Vec<NodeId>,
    }
    impl tsr_ast::ChildVisitor for Children<'_> {
        fn visit_node(&mut self, node: NodeId) -> std::ops::ControlFlow<()> {
            self.nodes.push(node);
            std::ops::ControlFlow::Continue(())
        }
        fn visit_list(&mut self, list: NodeListId) -> std::ops::ControlFlow<()> {
            self.visit_node_slice(self.factory.read_list(list).nodes())
        }
        fn visit_node_slice(&mut self, nodes: tsr_ast::NodeSlice) -> std::ops::ControlFlow<()> {
            self.nodes
                .extend(self.factory.read_nodes(nodes).iter().flatten());
            std::ops::ControlFlow::Continue(())
        }
    }
    let mut children = Children {
        factory,
        nodes: Vec::new(),
    };
    let _ = factory.node(node).for_each_child(&mut children);
    children.nodes
}

// port: tsc/internal/transformers/estransforms/classfields.go:createPrivateStaticFieldInitializer
fn create_private_static_field_initializer(
    context: &EmitContext,
    factory: &mut dyn RuntimeFactory,
    variable_name: NodeId,
    initializer: Option<NodeId>,
) -> NodeId {
    let initializer = match initializer {
        Some(initializer) => initializer,
        None => context.new_void_zero_expression(factory),
    };
    let value = factory.new_identifier(JsString::from_bytes(&b"value"[..]));
    let property =
        factory.new_property_assignment(None, Some(value), None, None, Some(initializer));
    let properties = new_node_list(factory, vec![property]);
    let object = factory.new_object_literal_expression(Some(properties), false);
    context.new_assignment_expression(factory, variable_name, object)
}

// port: tsc/internal/transformers/estransforms/classfields.go:createPrivateInstanceFieldInitializer
fn create_private_instance_field_initializer(
    context: &EmitContext,
    factory: &mut dyn RuntimeFactory,
    receiver: NodeId,
    initializer: Option<NodeId>,
    weak_map_name: NodeId,
) -> NodeId {
    let initializer = match initializer {
        Some(initializer) => initializer,
        None => context.new_void_zero_expression(factory),
    };
    let set = factory.new_identifier(JsString::from_bytes(&b"set"[..]));
    context.new_method_call(factory, weak_map_name, set, vec![receiver, initializer])
}

// port: tsc/internal/transformers/estransforms/classfields.go:createPrivateInstanceMethodInitializer
fn create_private_instance_method_initializer(
    context: &EmitContext,
    factory: &mut dyn RuntimeFactory,
    receiver: NodeId,
    weak_set_name: NodeId,
) -> NodeId {
    let add = factory.new_identifier(JsString::from_bytes(&b"add"[..]));
    context.new_method_call(factory, weak_set_name, add, vec![receiver])
}

// port: tsc/internal/transformers/estransforms/classfields.go:isStaticPropertyDeclarationOrClassStaticBlock
fn is_static_property_declaration_or_class_static_block(
    view: AstView<'_>,
    node: NodeId,
) -> Result<bool, tsr_arena::Error> {
    let kind = view.node(node)?.kind();
    Ok(kind == K::ClassStaticBlockDeclaration
        || (kind == K::PropertyDeclaration && tsr_ast::utilities::has_static_modifier(view, node)?))
}

/// Whether a class has a static block that is a class-this assignment.
// port: tsc/internal/transformers/estransforms/classfields.go:classHasClassThisAssignment
pub(crate) fn class_has_class_this_assignment(
    emit_context: &EmitContext,
    factory: &dyn RuntimeFactory,
    node: NodeId,
) -> bool {
    for member in members_of(factory, node) {
        if is_class_this_assignment_block(emit_context, factory, member) {
            return true;
        }
    }
    false
}

// port: tsc/internal/transformers/estransforms/classfields.go:isNonStaticMethodOrAccessorWithPrivateName
fn is_non_static_method_or_accessor_with_private_name(
    view: AstView<'_>,
    member: NodeId,
) -> Result<bool, tsr_arena::Error> {
    if tsr_ast::utilities::is_static(view, member)? {
        return Ok(false);
    }
    let read = view.node(member)?;
    if !(tsr_ast::utilities::is_method_or_accessor(&read)
        || tsr_ast::utilities::is_auto_accessor_property_declaration(view, member)?)
    {
        return Ok(false);
    }
    let name = read.name().expect(NIL);
    Ok(view.node(name)?.kind() == K::PrivateIdentifier)
}

// port: tsc/internal/transformers/estransforms/classfields.go:createMemberAccessForPropertyName
fn create_member_access_for_property_name(
    factory: &mut dyn RuntimeFactory,
    emit_context: &EmitContext,
    receiver: NodeId,
    name: NodeId,
    location: NodeId,
) -> NodeId {
    let name_kind = kind_of(factory, name);
    if name_kind == K::ComputedPropertyName {
        let argument = expression_of(factory, name);
        let expression = factory.new_element_access_expression(
            Some(receiver),
            None,
            Some(argument),
            node_flags::NONE,
        );
        let loc = loc_of(factory, location);
        factory.set_node_range(expression, loc);
        return expression;
    }
    let expression = if name_kind == K::Identifier || name_kind == K::PrivateIdentifier {
        factory.new_property_access_expression(Some(receiver), None, Some(name), node_flags::NONE)
    } else {
        // string or numeric literal
        factory.new_element_access_expression(Some(receiver), None, Some(name), node_flags::NONE)
    };
    let loc = loc_of(factory, name);
    let mut emit_context = emit_context.clone();
    emit_context.set_comment_range(expression, loc);
    emit_context.set_source_map_range(expression, loc);
    emit_context.add_emit_flags(expression, emit_flags::NO_NESTED_SOURCE_MAPS);
    expression
}

// port: tsc/internal/transformers/estransforms/classfields.go:shouldBeCapturedInTempVariable
fn should_be_captured_in_temp_variable(
    view: AstView<'_>,
    node: NodeId,
) -> Result<bool, tsr_arena::Error> {
    let target = tsr_ast::skip_parentheses(view, node)?;
    Ok(!matches!(
        view.node(target)?.kind().known(),
        Some(
            K::Identifier
                | K::ThisKeyword
                | K::NumericLiteral
                | K::BigIntLiteral
                | K::StringLiteral
        )
    ))
}

/// Decomposes a comma expression tree into a sequence of expressions, each
/// passed to `yield_`; upstream's `iter.Seq`.
// port: tsc/internal/transformers/estransforms/classfields.go:flattenCommaList
fn flatten_comma_list(
    factory: &dyn RuntimeFactory,
    node: NodeId,
    yield_: &mut dyn FnMut(NodeId) -> bool,
) {
    flatten_comma_list_worker(factory, node, yield_);
}

// port: tsc/internal/transformers/estransforms/classfields.go:flattenCommaListWorker
fn flatten_comma_list_worker(
    factory: &dyn RuntimeFactory,
    node: NodeId,
    yield_: &mut dyn FnMut(NodeId) -> bool,
) -> bool {
    let read = factory.node(node);
    if read.kind() == K::ParenthesizedExpression && tsr_ast::utilities::node_is_synthesized(&read) {
        let expression = read.expression().expect(NIL);
        drop(read);
        return flatten_comma_list_worker(factory, expression, yield_);
    }
    if read.kind() == K::BinaryExpression {
        drop(read);
        let (left, operator, _, right) = binary_parts(factory, node);
        if operator == K::CommaToken {
            return flatten_comma_list_worker(factory, left, yield_)
                && flatten_comma_list_worker(factory, right, yield_);
        }
        return yield_(node);
    }
    yield_(node)
}

/// The assignment `name`'s expression caches its value in, as the binary
/// expression node.
// port: tsc/internal/transformers/estransforms/classfields.go:findComputedPropertyNameCacheAssignment
pub(crate) fn find_computed_property_name_cache_assignment(
    _emit_context: &EmitContext,
    factory: &dyn RuntimeFactory,
    name: NodeId,
) -> Option<NodeId> {
    let mut node = expression_of(factory, name);
    loop {
        // `ast.SkipOuterExpressions(node, 0)` skips nothing.
        if kind_of(factory, node) == K::BinaryExpression {
            let (left, operator, _, right) = binary_parts(factory, node);
            if operator == K::CommaToken {
                node = right;
                continue;
            }
            if is_assignment_expression(factory, node, true /*excludeCompoundAssignment*/)
                && kind_of(factory, left) == K::Identifier
            {
                return Some(node);
            }
        }
        break;
    }
    None
}

// port: tsc/internal/transformers/estransforms/classfields.go:expandPreOrPostfixIncrementOrDecrementExpression
pub(crate) fn expand_pre_or_postfix_increment_or_decrement_expression(
    factory: &mut dyn RuntimeFactory,
    emit_context: &EmitContext,
    node: NodeId,
    expression: NodeId,
    result_variable: Option<NodeId>,
) -> NodeId {
    let (operator, operand, is_prefix) = ClassFieldsTransformer::unary_parts(factory, node);
    let mut emit_context = emit_context.clone();

    let temp = emit_context.new_temp_variable(factory);
    emit_context.add_variable_declaration(factory, temp);
    let mut expression = emit_context.new_assignment_expression(factory, temp, expression);
    let operand_loc = loc_of(factory, operand);
    factory.set_node_range(expression, operand_loc);

    let node_loc = loc_of(factory, node);
    let mut operation = if is_prefix {
        factory.new_prefix_unary_expression(operator, Some(temp))
    } else {
        factory.new_postfix_unary_expression(Some(temp), operator)
    };
    factory.set_node_range(operation, node_loc);

    if let Some(result_variable) = result_variable {
        operation = emit_context.new_assignment_expression(factory, result_variable, operation);
        factory.set_node_range(operation, node_loc);
    }

    expression = emit_context.new_comma_expression(factory, expression, operation);
    factory.set_node_range(expression, node_loc);

    if !is_prefix {
        expression = emit_context.new_comma_expression(factory, expression, temp);
        factory.set_node_range(expression, node_loc);
    }

    expression
}
