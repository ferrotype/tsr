//! `transformers/estransforms/esdecorator.go`: lowers ECMAScript (TC39
//! stage 3) decorators to the `__esDecorate` and `__runInitializers` helpers.
//!
//! Class/Decorator evaluation order, as it pertains to this transformer:
//!
//! 1. Class decorators are evaluated outside of the private name scope of the class.
//! 2. `ClassHeritage` clause is evaluated outside of the private name scope of the class.
//! 3. The name of the class is assigned.
//! 4. For each member: member decorators are evaluated, then a computed
//!    property name is evaluated.
//! 5. Static non-field (method/getter/setter/auto-accessor) element decorators are applied
//! 6. Non-static non-field (method/getter/setter/auto-accessor) element decorators are applied
//! 7. Static field (excl. auto-accessor) element decorators are applied
//! 8. Non-static field (excl. auto-accessor) element decorators are applied
//! 9. Class decorators are applied
//! 10. Class binding is initialized
//! 11. Static method extra initializers are evaluated
//! 12. Static fields are initialized (incl. extra initializers) and static blocks are evaluated
//! 13. Class extra initializers are evaluated
//!
//! Class constructor evaluation order: instance method extra initializers are
//! evaluated, then each instance field/auto-accessor is initialized and its
//! extra initializers are evaluated.
//!
//! Upstream holds one `NodeVisitor` per visit function; here each is a
//! [`Visit`] variant, and a visit that upstream sends through a particular
//! visitor builds that visitor over the current factory
//! ([`EsDecoratorTransformer::with`]), since the visitor a callback receives
//! may be any of them. The lexical scope stack is a vector whose last entry
//! is upstream's `top` (an entry's `next` is the one before it). The state
//! cells are never borrowed across a visit.
use super::classfields::{
    class_has_class_this_assignment, expand_pre_or_postfix_increment_or_decrement_expression,
    find_computed_property_name_cache_assignment,
};
use super::classthis::is_class_this_assignment_block;
use super::namedevaluation::{
    class_has_declared_or_explicitly_assigned_name,
    inject_class_named_evaluation_helper_block_if_missing, is_class_named_evaluation_helper_block,
    is_named_evaluation_and, transform_named_evaluation,
};
use super::utilities::{
    create_accessor_property_backing_field, identifier_text, is_super_property, list_nodes,
    new_node_list, restore_outer_expressions_all, skip_outer_expressions_all, subtree_facts, NIL,
};
use crate::transformer::{Error, Failure, TransformOptions, Transformer};
use crate::utilities::{
    find_super_statement_index_path, get_non_assignment_operator_for_compound_assignment,
    is_generated_identifier, is_simple_inlineable_expression, move_range_past_decorators,
    move_range_past_modifiers, single_or_many,
};
use std::cell::{Cell, RefCell};
use std::rc::Rc;
use tsr_ast::{
    modifier_flags, node_flags, subtree_flags, AstBuilder, AstView, FactoryMethods, JsString,
    NodeId, NodeListId, NodeVisitor, RuntimeFactory, SyntaxKind as K,
};
use tsr_core::{LanguageVariant, ScriptTarget};
use tsr_printer::{
    emit_flags, generated_identifier_flags as g, AssignedNameOptions, AutoGenerateOptions,
    EmitContext, EmitVisitorHooks,
};

/// `lexicalEntryKind`: discriminates the kind of lexical scope entry. The
/// discriminants are upstream's `iota` values, which its assertion messages
/// print.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum LexicalEntryKind {
    Class = 0,
    ClassElement = 1,
    Name = 2,
    Other = 3,
}

/// `lexicalEntry`: a single entry in the lexical scope stack used to track
/// nested class declarations and their state during transformation.
struct LexicalEntry {
    kind: LexicalEntryKind,
    class_info_data: Option<ClassInfoRef>,
    saved_pending_expressions: Vec<NodeId>,
    class_this_data: Option<NodeId>,
    class_super_data: Option<NodeId>,
    depth: i32,
}

impl LexicalEntry {
    fn new(kind: LexicalEntryKind) -> Self {
        Self {
            kind,
            class_info_data: None,
            saved_pending_expressions: Vec::new(),
            class_this_data: None,
            class_super_data: None,
            depth: 0,
        }
    }
}

/// `memberInfo`: decoration-related data for a single class element.
#[allow(clippy::struct_field_names)] // upstream's field names
struct MemberInfo {
    /// used in class definition step 4.a
    member_decorators_name: NodeId,
    /// used in class definition step 12 and constructor evaluation step 2.a
    member_initializers_name: Option<NodeId>,
    /// used in class definition step 12 and constructor evaluation step 2.b
    member_extra_initializers_name: Option<NodeId>,
    member_descriptor_name: Option<NodeId>,
}

/// `classInfo`: all transformation data for a single decorated class.
/// Upstream's `class` field is written and never read.
#[derive(Default)]
struct ClassInfo {
    /// used in class definition step 2
    class_decorators_name: Option<NodeId>,
    /// used in class definition step 10
    class_descriptor_name: Option<NodeId>,
    /// used in class definition step 13
    class_extra_initializers_name: Option<NodeId>,
    /// `_classThis`, if needed.
    class_this: Option<NodeId>,
    /// `_classSuper`, if needed.
    class_super: Option<NodeId>,
    metadata_reference: Option<NodeId>,
    /// `collections.OrderedMap[*ast.Node, *memberInfo]`, in insertion order.
    member_infos: Vec<(NodeId, Rc<RefCell<MemberInfo>>)>,
    /// used in constructor evaluation step 1
    instance_method_extra_initializers_name: Option<NodeId>,
    /// used in class definition step 11
    static_method_extra_initializers_name: Option<NodeId>,
    static_non_field_decoration_statements: Vec<NodeId>,
    non_static_non_field_decoration_statements: Vec<NodeId>,
    static_field_decoration_statements: Vec<NodeId>,
    non_static_field_decoration_statements: Vec<NodeId>,
    has_static_initializers: bool,
    has_non_ambient_instance_fields: bool,
    has_static_private_class_elements: bool,
    pending_static_initializers: Vec<NodeId>,
    pending_instance_initializers: Vec<NodeId>,
}

impl ClassInfo {
    /// `OrderedMap.Set`: replaces the value of an existing key in place.
    fn set_member_info(&mut self, member: NodeId, info: Rc<RefCell<MemberInfo>>) {
        if let Some(entry) = self.member_infos.iter_mut().find(|(key, _)| *key == member) {
            entry.1 = info;
        } else {
            self.member_infos.push((member, info));
        }
    }
}

type ClassInfoRef = Rc<RefCell<ClassInfo>>;

/// Upstream's visitors, by the function each one calls.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Visit {
    /// `tx.Visitor()`: `visit`.
    Main,
    OuterThis,
    Discarded,
    Modifier,
    ExportStrippingModifier,
    ClassElement,
    NonConstructorClassElement,
    ConstructorClassElement,
    ArrayAssignment,
    ObjectAssignment,
    StaticOnlyModifier,
    AsyncOnlyModifier,
    AccessorStrippingModifier,
}

/// Upstream's `createDescriptorFunc` values.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Descriptor {
    Method,
    GetAccessor,
    SetAccessor,
    AccessorProperty,
}

/// `partialResult`.
#[derive(Default)]
struct PartialResult {
    modifiers: Option<NodeListId>,
    referenced_name: Option<NodeId>,
    name: Option<NodeId>,
    initializers_name: Option<NodeId>,
    extra_initializers_name: Option<NodeId>,
    descriptor_name: Option<NodeId>,
    this_arg: Option<NodeId>,
}

/// `esDecoratorTransformer`. Upstream's `compilerOptions` is read by the
/// constructor only.
struct EsDecoratorTransformer {
    emit_context: EmitContext,
    hooks: EmitVisitorHooks,
    failure: Failure,
    top: RefCell<Vec<LexicalEntry>>,
    class_info_stack: RefCell<Option<ClassInfoRef>>,
    class_this: Cell<Option<NodeId>>,
    class_super: Cell<Option<NodeId>>,
    pending_expressions: RefCell<Vec<NodeId>>,
    outer_this: Cell<Option<NodeId>>,
    should_transform_private_static_elements_in_file: Cell<bool>,
}

/// `debug.Assert(cond, message...)`: an empty message is upstream's assert
/// without one.
fn debug_assert(condition: bool, message: impl FnOnce() -> String) {
    if condition {
        return;
    }
    let message = message();
    assert!(!message.is_empty(), "Debug failure. False expression.");
    panic!("Debug failure. False expression: {message}");
}

/// The kind of `node`; an unknown raw kind reads as `Unknown`.
fn kind_of(factory: &dyn RuntimeFactory, node: NodeId) -> K {
    factory.node(node).kind().known().unwrap_or(K::Unknown)
}

fn text(bytes: &[u8]) -> JsString {
    JsString::from_bytes(bytes)
}

fn options(flags: g::Flags) -> AutoGenerateOptions {
    AutoGenerateOptions {
        flags,
        ..AutoGenerateOptions::default()
    }
}

// port: tsc/internal/transformers/estransforms/esdecorator.go:newESDecoratorTransformer
pub fn new_es_decorator_transformer<'a>(opts: &TransformOptions<'a>) -> Option<Transformer<'a>> {
    // When experimentalDecorators is set, the legacy decorator transformer handles all
    // decorators. When targeting ESNext with useDefineForClassFields, there's nothing to
    // transform. In either case every node would be returned unchanged, so skip entirely.
    if opts.compiler_options.experimental_decorators.is_true()
        || (opts.compiler_options.emit_script_target() >= ScriptTarget::ESNEXT
            && opts.compiler_options.use_define_for_class_fields())
    {
        return None;
    }
    let tx = EsDecoratorTransformer {
        emit_context: opts.context.clone(),
        hooks: opts.context.visitor_hooks(),
        failure: opts.failure.clone(),
        top: RefCell::new(Vec::new()),
        class_info_stack: RefCell::new(None),
        class_this: Cell::new(None),
        class_super: Cell::new(None),
        pending_expressions: RefCell::new(Vec::new()),
        outer_this: Cell::new(None),
        should_transform_private_static_elements_in_file: Cell::new(false),
    };
    Some(Transformer::new(
        move |visitor: &mut NodeVisitor<'_>, node: Option<NodeId>| {
            tx.dispatch(Visit::Main, visitor, node)
        },
        Some(opts.context.clone()),
        opts.failure.clone(),
    ))
}

impl EsDecoratorTransformer {
    /// The emit context handle, for its `&mut self` operations.
    fn ctx(&self) -> EmitContext {
        self.emit_context.clone()
    }

    /// Runs `f` with one of upstream's visitors over `visitor`'s factory.
    fn with<R>(
        &self,
        visitor: &mut NodeVisitor<'_>,
        which: Visit,
        f: impl FnOnce(&mut NodeVisitor<'_>) -> R,
    ) -> R {
        let visit = |visitor: &mut NodeVisitor<'_>, node: Option<NodeId>| -> Option<NodeId> {
            self.dispatch(which, visitor, node)
        };
        let mut created = self
            .hooks
            .new_node_visitor(Some(&visit), visitor.factory_mut());
        f(&mut created)
    }

    fn visit_node(
        &self,
        visitor: &mut NodeVisitor<'_>,
        which: Visit,
        node: Option<NodeId>,
    ) -> Option<NodeId> {
        self.with(visitor, which, |v| v.visit_node(node))
    }

    fn visit_nodes(
        &self,
        visitor: &mut NodeVisitor<'_>,
        which: Visit,
        list: Option<NodeListId>,
    ) -> Option<NodeListId> {
        self.with(visitor, which, |v| v.visit_nodes(list))
    }

    fn visit_modifiers(
        &self,
        visitor: &mut NodeVisitor<'_>,
        which: Visit,
        list: Option<NodeListId>,
    ) -> Option<NodeListId> {
        self.with(visitor, which, |v| v.visit_modifiers(list))
    }

    fn visit_each_child(
        &self,
        visitor: &mut NodeVisitor<'_>,
        which: Visit,
        node: NodeId,
    ) -> NodeId {
        self.with(visitor, which, |v| v.visit_each_child(Some(node)))
            .expect(NIL)
    }

    /// An AST query over the factory's storage view. A failed query is
    /// recorded, which fails the file, and answers `T::default()`.
    fn q<T: Default>(
        &self,
        factory: &dyn RuntimeFactory,
        query: impl FnOnce(AstView<'_>) -> Result<T, tsr_arena::Error>,
    ) -> T {
        let Some(view) = factory.ast_view() else {
            self.failure.record(Error::Unsupported(
                "a transform over a factory without AST storage",
            ));
            return T::default();
        };
        self.failure.ok(query(view)).unwrap_or_default()
    }

    /// A printer factory helper that needs the concrete builder. A failure is
    /// recorded and answered with a placeholder identifier: the file fails
    /// and no later visit runs, so the placeholder is never printed.
    fn with_builder(
        &self,
        visitor: &mut NodeVisitor<'_>,
        f: impl FnOnce(&mut EmitContext, &mut AstBuilder) -> Result<NodeId, tsr_printer::Error>,
    ) -> NodeId {
        let mut ctx = self.ctx();
        let result = match visitor.factory_mut().ast_builder_mut() {
            Some(builder) => f(&mut ctx, builder).map_err(Error::from),
            None => Err(Error::Unsupported(
                "a transform over a factory without AST storage",
            )),
        };
        match result {
            Ok(node) => node,
            Err(error) => {
                self.failure.record(error);
                visitor.factory_mut().new_identifier(JsString::default())
            }
        }
    }

    /// The node's kind; an unknown raw kind reads as `Unknown`.
    fn kind(visitor: &NodeVisitor<'_>, node: NodeId) -> K {
        kind_of(visitor.factory(), node)
    }

    fn name_of(visitor: &NodeVisitor<'_>, node: NodeId) -> Option<NodeId> {
        visitor.factory().node(node).name()
    }

    fn has_static_modifier(&self, visitor: &NodeVisitor<'_>, node: NodeId) -> bool {
        self.q(visitor.factory(), |view| {
            tsr_ast::utilities::has_static_modifier(view, node)
        })
    }

    fn has_accessor_modifier(&self, visitor: &NodeVisitor<'_>, node: NodeId) -> bool {
        self.q(visitor.factory(), |view| {
            tsr_ast::utilities::has_accessor_modifier(view, node)
        })
    }

    fn has_syntactic_modifier(&self, visitor: &NodeVisitor<'_>, node: NodeId, flags: u32) -> bool {
        self.q(visitor.factory(), |view| {
            tsr_ast::utilities::has_syntactic_modifier(view, node, flags)
        })
    }

    fn is_static(&self, visitor: &NodeVisitor<'_>, node: NodeId) -> bool {
        self.q(visitor.factory(), |view| {
            tsr_ast::utilities::is_static(view, node)
        })
    }

    fn is_private_identifier_class_element_declaration(
        &self,
        visitor: &NodeVisitor<'_>,
        node: NodeId,
    ) -> bool {
        self.q(visitor.factory(), |view| {
            tsr_ast::utilities::is_private_identifier_class_element_declaration(view, node)
        })
    }

    fn is_auto_accessor_property_declaration(
        &self,
        visitor: &NodeVisitor<'_>,
        node: NodeId,
    ) -> bool {
        self.q(visitor.factory(), |view| {
            tsr_ast::utilities::is_auto_accessor_property_declaration(view, node)
        })
    }

    fn is_assignment_expression(
        &self,
        visitor: &NodeVisitor<'_>,
        node: NodeId,
        exclude_compound_assignment: bool,
    ) -> bool {
        self.q(visitor.factory(), |view| {
            tsr_ast::is_assignment_expression(view, node, exclude_compound_assignment)
        })
    }

    fn is_left_hand_side_expression(&self, visitor: &NodeVisitor<'_>, node: NodeId) -> bool {
        self.q(visitor.factory(), |view| {
            tsr_ast::is_left_hand_side_expression(view, node)
        })
    }

    fn decorators(&self, visitor: &NodeVisitor<'_>, node: NodeId) -> Vec<NodeId> {
        self.q(visitor.factory(), |view| {
            tsr_ast::utilities_class::decorators(view, node)
        })
    }

    /// `isNamedEvaluationAnd(ec, node, isAnonymousClassNeedingAssignedName)`.
    fn is_named_evaluation_and_anonymous_class(
        &self,
        visitor: &NodeVisitor<'_>,
        node: NodeId,
    ) -> bool {
        let cb = |factory: &dyn RuntimeFactory, node: NodeId| {
            self.is_anonymous_class_needing_assigned_name(factory, node)
        };
        is_named_evaluation_and(&self.emit_context, visitor.factory(), node, Some(&cb))
    }

    /// `transformNamedEvaluation(ec, node, canIgnoreEmptyStringLiteralInAssignedName(classExpr), "")`.
    fn transform_named_evaluation(
        &self,
        visitor: &mut NodeVisitor<'_>,
        node: NodeId,
        class_expr: Option<NodeId>,
    ) -> NodeId {
        let ignore = self.can_ignore_empty_string_literal_in_assigned_name(visitor, class_expr);
        transform_named_evaluation(&self.emit_context, visitor.factory_mut(), node, ignore, b"")
    }

    //
    // Lexical scope
    //

    fn top_kind_message(top: &[LexicalEntry], expected: &str) -> String {
        let kind = top.last().expect(NIL).kind;
        format!(
            "Incorrect value for top.kind. Expected top.kind to be '{expected}' but got '{}' instead.",
            kind as i32
        )
    }

    // port: tsc/internal/transformers/estransforms/esdecorator.go:esDecoratorTransformer.updateState
    fn update_state(&self) {
        *self.class_info_stack.borrow_mut() = None;
        self.class_this.set(None);
        self.class_super.set(None);
        let top = self.top.borrow();
        let Some(entry) = top.last() else {
            return;
        };
        let len = top.len();
        match entry.kind {
            LexicalEntryKind::Class => {
                self.class_info_stack
                    .borrow_mut()
                    .clone_from(&entry.class_info_data);
            }
            LexicalEntryKind::ClassElement => {
                let next = &top[len.checked_sub(2).expect(NIL)];
                self.class_info_stack
                    .borrow_mut()
                    .clone_from(&next.class_info_data);
                self.class_this.set(entry.class_this_data);
                self.class_super.set(entry.class_super_data);
            }
            LexicalEntryKind::Name => {
                // `tx.top.next.next.next`: `top.next` and `top.next.next` are dereferenced.
                let _ = &top[len.checked_sub(3).expect(NIL)];
                if let Some(index) = len.checked_sub(4) {
                    let grandparent = &top[index];
                    if grandparent.kind == LexicalEntryKind::ClassElement {
                        let next = &top[index.checked_sub(1).expect(NIL)];
                        self.class_info_stack
                            .borrow_mut()
                            .clone_from(&next.class_info_data);
                        self.class_this.set(grandparent.class_this_data);
                        self.class_super.set(grandparent.class_super_data);
                    }
                }
            }
            LexicalEntryKind::Other => {}
        }
    }

    // port: tsc/internal/transformers/estransforms/esdecorator.go:esDecoratorTransformer.enterClass
    fn enter_class(&self, ci: Option<ClassInfoRef>) {
        let saved = std::mem::take(&mut *self.pending_expressions.borrow_mut());
        let mut entry = LexicalEntry::new(LexicalEntryKind::Class);
        entry.class_info_data = ci;
        entry.saved_pending_expressions = saved;
        self.top.borrow_mut().push(entry);
        self.update_state();
    }

    // port: tsc/internal/transformers/estransforms/esdecorator.go:esDecoratorTransformer.exitClass
    fn exit_class(&self) {
        {
            let mut top = self.top.borrow_mut();
            let kind = top.last().expect(NIL).kind;
            debug_assert(kind == LexicalEntryKind::Class, || {
                Self::top_kind_message(&top, "class")
            });
            let entry = top.pop().expect(NIL);
            *self.pending_expressions.borrow_mut() = entry.saved_pending_expressions;
        }
        self.update_state();
    }

    // port: tsc/internal/transformers/estransforms/esdecorator.go:esDecoratorTransformer.enterClassElement
    fn enter_class_element(&self, visitor: &NodeVisitor<'_>, node: NodeId) {
        {
            let top = self.top.borrow();
            let kind = top.last().expect(NIL).kind;
            debug_assert(kind == LexicalEntryKind::Class, || {
                Self::top_kind_message(&top, "class")
            });
        }
        let mut entry = LexicalEntry::new(LexicalEntryKind::ClassElement);
        let kind = Self::kind(visitor, node);
        if kind == K::ClassStaticBlockDeclaration
            || kind == K::PropertyDeclaration && self.has_static_modifier(visitor, node)
        {
            let top = self.top.borrow();
            if let Some(ci) = &top.last().expect(NIL).class_info_data {
                let ci = ci.borrow();
                entry.class_this_data = ci.class_this;
                entry.class_super_data = ci.class_super;
            }
        }
        self.top.borrow_mut().push(entry);
        self.update_state();
    }

    // port: tsc/internal/transformers/estransforms/esdecorator.go:esDecoratorTransformer.exitClassElement
    fn exit_class_element(&self) {
        {
            let mut top = self.top.borrow_mut();
            let kind = top.last().expect(NIL).kind;
            debug_assert(kind == LexicalEntryKind::ClassElement, || {
                Self::top_kind_message(&top, "class-element")
            });
            let len = top.len();
            let next = top.get(len.wrapping_sub(2)).expect(NIL).kind;
            debug_assert(next == LexicalEntryKind::Class, || {
                format!(
                    "Incorrect value for top.next.kind. Expected top.next.kind to be 'class' but got '{}' instead.",
                    next as i32
                )
            });
            top.pop();
        }
        self.update_state();
    }

    // port: tsc/internal/transformers/estransforms/esdecorator.go:esDecoratorTransformer.enterName
    fn enter_name(&self) {
        {
            let top = self.top.borrow();
            let kind = top.last().expect(NIL).kind;
            debug_assert(kind == LexicalEntryKind::ClassElement, || {
                Self::top_kind_message(&top, "class-element")
            });
        }
        self.top
            .borrow_mut()
            .push(LexicalEntry::new(LexicalEntryKind::Name));
        self.update_state();
    }

    // port: tsc/internal/transformers/estransforms/esdecorator.go:esDecoratorTransformer.exitName
    fn exit_name(&self) {
        {
            let mut top = self.top.borrow_mut();
            let kind = top.last().expect(NIL).kind;
            debug_assert(kind == LexicalEntryKind::Name, || {
                Self::top_kind_message(&top, "name")
            });
            top.pop();
        }
        self.update_state();
    }

    // port: tsc/internal/transformers/estransforms/esdecorator.go:esDecoratorTransformer.enterOther
    fn enter_other(&self) {
        let mut top = self.top.borrow_mut();
        if let Some(entry) = top
            .last_mut()
            .filter(|entry| entry.kind == LexicalEntryKind::Other)
        {
            debug_assert(self.pending_expressions.borrow().is_empty(), String::new);
            entry.depth += 1;
        } else {
            let saved = std::mem::take(&mut *self.pending_expressions.borrow_mut());
            let mut entry = LexicalEntry::new(LexicalEntryKind::Other);
            entry.saved_pending_expressions = saved;
            top.push(entry);
            drop(top);
            self.update_state();
        }
    }

    // port: tsc/internal/transformers/estransforms/esdecorator.go:esDecoratorTransformer.exitOther
    fn exit_other(&self) {
        let mut top = self.top.borrow_mut();
        let kind = top.last().expect(NIL).kind;
        debug_assert(kind == LexicalEntryKind::Other, || {
            Self::top_kind_message(&top, "other")
        });
        let entry = top.last_mut().expect(NIL);
        if entry.depth > 0 {
            debug_assert(self.pending_expressions.borrow().is_empty(), String::new);
            entry.depth -= 1;
        } else {
            let entry = top.pop().expect(NIL);
            *self.pending_expressions.borrow_mut() = entry.saved_pending_expressions;
            drop(top);
            self.update_state();
        }
    }

    //
    // Visitors
    //

    /// Every visitor's callback: the visit function of `which`.
    fn dispatch(
        &self,
        which: Visit,
        visitor: &mut NodeVisitor<'_>,
        node: Option<NodeId>,
    ) -> Option<NodeId> {
        let node = node.expect(NIL);
        if self.failure.is_set() {
            return Some(node);
        }
        match which {
            Visit::Main => self.visit(visitor, node),
            Visit::OuterThis => Some(self.outer_this_visit(visitor, node)),
            Visit::Discarded => self.discarded_value_visit(visitor, node),
            Visit::Modifier => Self::modifier_visitor_visit(visitor, node),
            Visit::ExportStrippingModifier => Self::export_stripping_modifier_visit(visitor, node),
            Visit::ClassElement => self.class_element_visitor_visit(visitor, node),
            Visit::NonConstructorClassElement => {
                self.non_constructor_class_element_visit(visitor, node)
            }
            Visit::ConstructorClassElement => self.constructor_class_element_visit(visitor, node),
            Visit::ArrayAssignment => Some(self.visit_array_assignment_element(visitor, node)),
            Visit::ObjectAssignment => Some(self.visit_object_assignment_element(visitor, node)),
            Visit::StaticOnlyModifier => {
                (Self::kind(visitor, node) == K::StaticKeyword).then_some(node)
            }
            Visit::AsyncOnlyModifier => {
                (Self::kind(visitor, node) == K::AsyncKeyword).then_some(node)
            }
            Visit::AccessorStrippingModifier => {
                (Self::kind(visitor, node) != K::AccessorKeyword).then_some(node)
            }
        }
    }

    // port: tsc/internal/transformers/estransforms/esdecorator.go:esDecoratorTransformer.visitSourceFile
    fn visit_source_file(&self, visitor: &mut NodeVisitor<'_>, node: NodeId) -> NodeId {
        self.top.borrow_mut().clear();
        self.should_transform_private_static_elements_in_file
            .set(false);
        let visited = self.visit_each_child(visitor, Visit::Main, node);
        let mut ctx = self.ctx();
        let helpers = ctx.read_emit_helpers();
        ctx.add_emit_helper(visited, &helpers);
        if self.should_transform_private_static_elements_in_file.get() {
            ctx.add_emit_flags(visited, emit_flags::TRANSFORM_PRIVATE_STATIC_ELEMENTS);
            self.should_transform_private_static_elements_in_file
                .set(false);
        }
        visited
    }

    // port: tsc/internal/transformers/estransforms/esdecorator.go:esDecoratorTransformer.outerThisVisit
    fn outer_this_visit(&self, visitor: &mut NodeVisitor<'_>, n: NodeId) -> NodeId {
        let facts = self.facts(visitor, n);
        let kind = Self::kind(visitor, n);
        if facts & subtree_flags::LEXICAL_THIS == 0 && kind != K::ThisKeyword {
            return n;
        }
        if kind == K::ThisKeyword {
            if self.outer_this.get().is_none() {
                let outer_this = self.ctx().new_unique_name_ex(
                    visitor.factory_mut(),
                    text(b"_outerThis"),
                    options(g::OPTIMISTIC),
                );
                self.outer_this.set(Some(outer_this));
            }
            return self.outer_this.get().expect(NIL);
        }
        self.visit_each_child(visitor, Visit::OuterThis, n)
    }

    /// `node.SubtreeFacts()`.
    fn facts(&self, visitor: &NodeVisitor<'_>, node: NodeId) -> u32 {
        match subtree_facts(visitor.factory(), node) {
            Ok(facts) => facts,
            Err(error) => {
                self.failure.record(error);
                0
            }
        }
    }

    // port: tsc/internal/transformers/estransforms/esdecorator.go:esDecoratorTransformer.shouldVisitNode
    fn should_visit_node(&self, visitor: &NodeVisitor<'_>, node: NodeId) -> bool {
        let facts = self.facts(visitor, node);
        facts & subtree_flags::DECORATORS != 0
            || (self.class_this.get().is_some() && facts & subtree_flags::LEXICAL_THIS != 0)
            || (self.class_this.get().is_some()
                && self.class_super.get().is_some()
                && facts & subtree_flags::LEXICAL_SUPER != 0)
    }

    // port: tsc/internal/transformers/estransforms/esdecorator.go:esDecoratorTransformer.visit
    fn visit(&self, visitor: &mut NodeVisitor<'_>, node: NodeId) -> Option<NodeId> {
        let kind = Self::kind(visitor, node);
        if kind == K::SourceFile {
            return Some(self.visit_source_file(visitor, node));
        }
        if !self.should_visit_node(visitor, node) {
            return Some(node);
        }
        match kind {
            // Decorators are elided. In Strada, a separate `modifierVisitor` drops decorators
            // before they reach `visitor` via visitEachChild. Here, `visit` serves as both
            // visitors, so decorators from modifier lists reach it directly.
            K::Decorator => None,
            K::ClassDeclaration => Some(self.visit_class_declaration(visitor, node)),
            K::ClassExpression => Some(self.visit_class_expression(visitor, node)),
            K::Constructor | K::PropertyDeclaration | K::ClassStaticBlockDeclaration => {
                panic!(
                    "Debug failure. Not supported outside of a class. Use 'classElementVisitor' instead."
                )
            }
            K::Parameter => Some(self.visit_parameter_declaration(visitor, node)),
            // Support NamedEvaluation to ensure the correct class name for class expressions.
            K::BinaryExpression => Some(self.visit_binary_expression(visitor, node, false)),
            K::PropertyAssignment | K::VariableDeclaration | K::BindingElement => {
                let initializer = visitor.factory().node(node).initializer();
                Some(self.visit_named_evaluation_site(visitor, node, initializer))
            }
            K::ExportAssignment => Some(self.visit_export_assignment(visitor, node)),
            K::ThisKeyword => Some(self.visit_this_expression(node)),
            K::ForStatement => Some(self.visit_for_statement(visitor, node)),
            K::ExpressionStatement => Some(self.visit_expression_statement(visitor, node)),
            K::ParenthesizedExpression => {
                Some(self.visit_parenthesized_expression(visitor, node, false))
            }
            K::PartiallyEmittedExpression => {
                Some(self.visit_partially_emitted_expression(visitor, node, false))
            }
            K::CallExpression => Some(self.visit_call_expression(visitor, node)),
            K::TaggedTemplateExpression => {
                Some(self.visit_tagged_template_expression(visitor, node))
            }
            K::PrefixUnaryExpression | K::PostfixUnaryExpression => {
                Some(self.visit_pre_or_postfix_unary_expression(visitor, node, false))
            }
            K::PropertyAccessExpression => {
                Some(self.visit_property_access_expression(visitor, node))
            }
            K::ElementAccessExpression => Some(self.visit_element_access_expression(visitor, node)),
            K::ComputedPropertyName => Some(self.visit_computed_property_name(visitor, node)),
            K::MethodDeclaration
            | K::SetAccessor
            | K::GetAccessor
            | K::FunctionExpression
            | K::FunctionDeclaration => {
                self.enter_other();
                let result = self.visit_each_child(visitor, Visit::Main, node);
                self.exit_other();
                Some(result)
            }
            _ => Some(self.visit_each_child(visitor, Visit::Main, node)),
        }
    }

    // port: tsc/internal/transformers/estransforms/esdecorator.go:esDecoratorTransformer.modifierVisitorVisit
    fn modifier_visitor_visit(visitor: &NodeVisitor<'_>, node: NodeId) -> Option<NodeId> {
        if Self::kind(visitor, node) == K::Decorator {
            return None;
        }
        Some(node)
    }

    // port: tsc/internal/transformers/estransforms/esdecorator.go:esDecoratorTransformer.classElementVisitorVisit
    fn class_element_visitor_visit(
        &self,
        visitor: &mut NodeVisitor<'_>,
        node: NodeId,
    ) -> Option<NodeId> {
        match Self::kind(visitor, node) {
            K::Constructor => Some(self.visit_constructor_declaration(visitor, node)),
            K::MethodDeclaration => Some(self.visit_method_declaration(visitor, node)),
            K::GetAccessor => Some(self.visit_get_accessor_declaration(visitor, node)),
            K::SetAccessor => Some(self.visit_set_accessor_declaration(visitor, node)),
            K::PropertyDeclaration => self.visit_property_declaration(visitor, node),
            K::ClassStaticBlockDeclaration => {
                self.visit_class_static_block_declaration(visitor, node)
            }
            _ => self.visit(visitor, node),
        }
    }

    // port: tsc/internal/transformers/estransforms/esdecorator.go:esDecoratorTransformer.discardedValueVisit
    fn discarded_value_visit(&self, visitor: &mut NodeVisitor<'_>, node: NodeId) -> Option<NodeId> {
        match Self::kind(visitor, node) {
            K::PrefixUnaryExpression | K::PostfixUnaryExpression => {
                Some(self.visit_pre_or_postfix_unary_expression(visitor, node, true))
            }
            K::BinaryExpression => Some(self.visit_binary_expression(visitor, node, true)),
            K::ParenthesizedExpression => {
                Some(self.visit_parenthesized_expression(visitor, node, true))
            }
            K::PartiallyEmittedExpression => {
                Some(self.visit_partially_emitted_expression(visitor, node, true))
            }
            _ => self.visit(visitor, node),
        }
    }

    // port: tsc/internal/transformers/estransforms/esdecorator.go:esDecoratorTransformer.nonConstructorClassElementVisit
    fn non_constructor_class_element_visit(
        &self,
        visitor: &mut NodeVisitor<'_>,
        node: NodeId,
    ) -> Option<NodeId> {
        if Self::kind(visitor, node) == K::Constructor {
            return Some(node); // skip constructors in pass 1
        }
        self.class_element_visitor_visit(visitor, node)
    }

    // port: tsc/internal/transformers/estransforms/esdecorator.go:esDecoratorTransformer.constructorClassElementVisit
    fn constructor_class_element_visit(
        &self,
        visitor: &mut NodeVisitor<'_>,
        node: NodeId,
    ) -> Option<NodeId> {
        if Self::kind(visitor, node) == K::Constructor {
            return self.class_element_visitor_visit(visitor, node);
        }
        Some(node)
    }

    // port: tsc/internal/transformers/estransforms/esdecorator.go:esDecoratorTransformer.exportStrippingModifierVisit
    fn export_stripping_modifier_visit(visitor: &NodeVisitor<'_>, node: NodeId) -> Option<NodeId> {
        if Self::kind(visitor, node) == K::ExportKeyword {
            return None;
        }
        Self::modifier_visitor_visit(visitor, node)
    }

    // port: tsc/internal/transformers/estransforms/esdecorator.go:getHelperVariableName
    fn get_helper_variable_name(&self, visitor: &NodeVisitor<'_>, node: NodeId) -> Vec<u8> {
        let factory = visitor.factory();
        let ec = &self.emit_context;
        let name = Self::name_of(visitor, node);
        let name_kind = name.map(|name| Self::kind(visitor, name));
        let node_kind = Self::kind(visitor, node);
        let mut declaration_name: Vec<u8> = match (name, name_kind) {
            (Some(name), Some(K::Identifier)) if !is_generated_identifier(ec, name) => {
                identifier_text(factory, name).as_bytes().to_vec()
            }
            (Some(name), Some(K::PrivateIdentifier)) if !ec.has_auto_generate_info(name) => {
                let text = identifier_text(factory, name);
                if text.len() > 1 {
                    text.as_bytes()[1..].to_vec()
                } else {
                    Vec::new()
                }
            }
            (Some(name), Some(K::StringLiteral))
                if tsr_scanner::is_identifier_text(
                    Self::string_literal_text(visitor, name).as_bytes(),
                    LanguageVariant::STANDARD,
                ) =>
            {
                Self::string_literal_text(visitor, name).as_bytes().to_vec()
            }
            _ if matches!(node_kind, K::ClassDeclaration | K::ClassExpression) => b"class".to_vec(),
            _ => b"member".to_vec(),
        };

        if node_kind == K::GetAccessor {
            declaration_name.splice(0..0, b"get_".iter().copied());
        }
        if node_kind == K::SetAccessor {
            declaration_name.splice(0..0, b"set_".iter().copied());
        }
        if name_kind == Some(K::PrivateIdentifier) {
            declaration_name.splice(0..0, b"private_".iter().copied());
        }
        if self.is_static(visitor, node) {
            declaration_name.splice(0..0, b"static_".iter().copied());
        }
        declaration_name.insert(0, b'_');
        declaration_name
    }

    fn string_literal_text(visitor: &NodeVisitor<'_>, node: NodeId) -> JsString {
        visitor
            .factory()
            .node(node)
            .as_string_literal()
            .expect("StringLiteral payload")
            .text_owned()
    }

    // port: tsc/internal/transformers/estransforms/esdecorator.go:esDecoratorTransformer.createHelperVariable
    fn create_helper_variable(
        &self,
        visitor: &mut NodeVisitor<'_>,
        node: NodeId,
        suffix: &[u8],
    ) -> NodeId {
        let mut name = self.get_helper_variable_name(visitor, node);
        name.push(b'_');
        name.extend_from_slice(suffix);
        self.ctx().new_unique_name_ex(
            visitor.factory_mut(),
            JsString::from_bytes(name),
            options(g::OPTIMISTIC | g::RESERVED_IN_NESTED_SCOPES),
        )
    }

    // port: tsc/internal/transformers/estransforms/esdecorator.go:esDecoratorTransformer.createLet
    fn create_let(
        factory: &mut dyn RuntimeFactory,
        name: NodeId,
        initializer: Option<NodeId>,
    ) -> NodeId {
        let declaration = factory.new_variable_declaration(Some(name), None, None, initializer);
        let declarations = new_node_list(factory, vec![declaration]);
        let list = factory.new_variable_declaration_list(Some(declarations), node_flags::LET);
        factory.new_variable_statement(None, Some(list))
    }

    /// `f.NewArrayLiteralExpression(f.NewNodeList(nil), false)`.
    fn new_empty_array(factory: &mut dyn RuntimeFactory) -> NodeId {
        let elements = new_node_list(factory, Vec::new());
        factory.new_array_literal_expression(Some(elements), false)
    }

    /// `f.NewToken(ast.KindNullKeyword)`.
    fn new_null(factory: &mut dyn RuntimeFactory) -> NodeId {
        factory.new_token(K::NullKeyword.into())
    }

    // port: tsc/internal/transformers/estransforms/esdecorator.go:esDecoratorTransformer.createClassInfo
    fn create_class_info(&self, visitor: &mut NodeVisitor<'_>, node: NodeId) -> ClassInfoRef {
        let mut ctx = self.ctx();
        let metadata_reference = ctx.new_unique_name_ex(
            visitor.factory_mut(),
            text(b"_metadata"),
            options(g::OPTIMISTIC | g::FILE_LEVEL),
        );
        let mut ci = ClassInfo {
            metadata_reference: Some(metadata_reference),
            ..ClassInfo::default()
        };

        // Before visiting we perform a first pass to collect information we'll need
        // as we descend.

        let members = list_nodes(
            visitor.factory(),
            visitor.factory().node(node).member_list(),
        );

        // If the class itself is decorated, create a _classThis binding
        if self.q(visitor.factory(), |view| {
            tsr_ast::utilities_class::node_is_decorated(view, false, node, None, None)
        }) {
            let needs_unique_class_this = members.iter().any(|&member| {
                (self.is_private_identifier_class_element_declaration(visitor, member)
                    || self.is_auto_accessor_property_declaration(visitor, member))
                    && self.has_static_modifier(visitor, member)
            });
            // We do not mark _classThis as FileLevel if it may be reused by class private fields, which requires the
            // ability access the captured `_classThis` of outer scopes.
            let mut flags = g::OPTIMISTIC | g::FILE_LEVEL;
            if needs_unique_class_this {
                flags = g::OPTIMISTIC | g::RESERVED_IN_NESTED_SCOPES;
            }
            ci.class_this = Some(ctx.new_unique_name_ex(
                visitor.factory_mut(),
                text(b"_classThis"),
                options(flags),
            ));
        }

        for member in members {
            let member_kind = Self::kind(visitor, member);
            if matches!(
                member_kind,
                K::MethodDeclaration | K::GetAccessor | K::SetAccessor
            ) && self.q(visitor.factory(), |view| {
                tsr_ast::utilities_class::node_or_child_is_decorated(
                    view,
                    false,
                    member,
                    Some(node),
                    None,
                )
            }) {
                if self.has_static_modifier(visitor, member) {
                    if ci.static_method_extra_initializers_name.is_none() {
                        let name = ctx.new_unique_name_ex(
                            visitor.factory_mut(),
                            text(b"_staticExtraInitializers"),
                            options(g::OPTIMISTIC | g::FILE_LEVEL),
                        );
                        ci.static_method_extra_initializers_name = Some(name);
                        let renamed_class_this = match ci.class_this {
                            Some(class_this) => class_this,
                            None => ctx.new_this_expression(visitor.factory_mut()),
                        };
                        let initializer = ctx.new_run_initializers_helper(
                            visitor.factory_mut(),
                            renamed_class_this,
                            name,
                            None,
                        );
                        self.set_range_of_name_or_past_decorators(visitor, initializer, node);
                        ci.pending_static_initializers.push(initializer);
                    }
                } else if ci.instance_method_extra_initializers_name.is_none() {
                    let name = ctx.new_unique_name_ex(
                        visitor.factory_mut(),
                        text(b"_instanceExtraInitializers"),
                        options(g::OPTIMISTIC | g::FILE_LEVEL),
                    );
                    ci.instance_method_extra_initializers_name = Some(name);
                    let this = ctx.new_this_expression(visitor.factory_mut());
                    let initializer =
                        ctx.new_run_initializers_helper(visitor.factory_mut(), this, name, None);
                    self.set_range_of_name_or_past_decorators(visitor, initializer, node);
                    ci.pending_instance_initializers.push(initializer);
                }
            }

            if member_kind == K::ClassStaticBlockDeclaration {
                if !is_class_named_evaluation_helper_block(&ctx, visitor.factory(), member) {
                    ci.has_static_initializers = true;
                }
            } else if member_kind == K::PropertyDeclaration {
                if self.has_static_modifier(visitor, member) {
                    ci.has_static_initializers = ci.has_static_initializers
                        || visitor.factory().node(member).initializer().is_some()
                        || self.q(visitor.factory(), |view| {
                            tsr_ast::utilities_middle::has_decorators(view, &view.node(member)?)
                        });
                } else {
                    ci.has_non_ambient_instance_fields = ci.has_non_ambient_instance_fields
                        || !self.has_syntactic_modifier(visitor, member, modifier_flags::AMBIENT);
                }
            }

            if (self.is_private_identifier_class_element_declaration(visitor, member)
                || self.is_auto_accessor_property_declaration(visitor, member))
                && self.has_static_modifier(visitor, member)
            {
                ci.has_static_private_class_elements = true;
            }

            // exit early if possible
            if ci.static_method_extra_initializers_name.is_some()
                && ci.instance_method_extra_initializers_name.is_some()
                && ci.has_static_initializers
                && ci.has_non_ambient_instance_fields
                && ci.has_static_private_class_elements
            {
                break;
            }
        }

        Rc::new(RefCell::new(ci))
    }

    /// `SetSourceMapRange(target, node.Name().Loc)`, or the range past the
    /// decorators of a nameless `node`.
    fn set_range_of_name_or_past_decorators(
        &self,
        visitor: &NodeVisitor<'_>,
        target: NodeId,
        node: NodeId,
    ) {
        let range = match Self::name_of(visitor, node) {
            Some(name) => visitor.factory().node(name).range(),
            None => move_range_past_decorators(visitor.factory(), node),
        };
        self.ctx().set_source_map_range(target, range);
    }

    // port: tsc/internal/transformers/estransforms/esdecorator.go:esDecoratorTransformer.transformClassLike
    #[allow(clippy::too_many_lines)] // upstream's single function, kept in its order
    fn transform_class_like(&self, visitor: &mut NodeVisitor<'_>, mut node: NodeId) -> NodeId {
        let mut ec = self.ctx();

        ec.start_variable_environment();

        // When a class has class decorators we end up transforming it into a statement that would otherwise give it an
        // assigned name. If the class doesn't have an assigned name, we'll give it an assigned name of `""`.
        if !class_has_declared_or_explicitly_assigned_name(&ec, visitor.factory(), node)
            && self.q(visitor.factory(), |view| {
                tsr_ast::utilities_class::class_or_constructor_parameter_is_decorated(
                    view, false, node,
                )
            })
        {
            let empty = visitor
                .factory_mut()
                .new_string_literal(JsString::default(), 0);
            node = inject_class_named_evaluation_helper_block_if_missing(
                &ec,
                visitor.factory_mut(),
                node,
                empty,
                None,
            );
        }

        let class_reference = self.with_builder(visitor, |ec, builder| {
            ec.get_local_name_ex(builder, Some(node), AssignedNameOptions::default())
        });
        let ci = self.create_class_info(visitor, node);
        let mut class_definition_statements: Vec<NodeId> = Vec::new();
        let mut leading_block_statements: Vec<NodeId> = Vec::new();
        let mut trailing_block_statements: Vec<NodeId> = Vec::new();
        let mut synthetic_constructor = None;
        let mut heritage_clauses = None;
        let mut should_transform_private_static_elements_in_class = false;

        // 1. Class decorators are evaluated outside the private name scope of the class.
        //
        // - Since class decorators don't have privileged access to private names defined inside the class,
        //   they must be evaluated outside of the class body.
        // - Since a class decorator can replace the class constructor, we must define a variable to keep track
        //   of the mutated class.
        // - Since a class decorator can add extra initializers, we must define a variable to keep track of
        //   extra initializers.
        let decorators = self.decorators(visitor, node);
        let class_decorators = self.transform_all_decorators_of_declaration(visitor, &decorators);
        if !class_decorators.is_empty() {
            debug_assert(ci.borrow().class_this.is_some(), String::new);

            let factory = visitor.factory_mut();
            let decorators_name = ec.new_unique_name_ex(
                factory,
                text(b"_classDecorators"),
                options(g::OPTIMISTIC | g::FILE_LEVEL),
            );
            let descriptor_name = ec.new_unique_name_ex(
                factory,
                text(b"_classDescriptor"),
                options(g::OPTIMISTIC | g::FILE_LEVEL),
            );
            let extra_initializers_name = ec.new_unique_name_ex(
                factory,
                text(b"_classExtraInitializers"),
                options(g::OPTIMISTIC | g::FILE_LEVEL),
            );
            {
                let mut ci = ci.borrow_mut();
                ci.class_decorators_name = Some(decorators_name);
                ci.class_descriptor_name = Some(descriptor_name);
                ci.class_extra_initializers_name = Some(extra_initializers_name);
            }

            let elements = new_node_list(factory, class_decorators.clone());
            let decorators_array = factory.new_array_literal_expression(Some(elements), false);
            let class_this = ci.borrow().class_this.expect(NIL);
            class_definition_statements.push(Self::create_let(
                factory,
                decorators_name,
                Some(decorators_array),
            ));
            class_definition_statements.push(Self::create_let(factory, descriptor_name, None));
            let empty = Self::new_empty_array(factory);
            class_definition_statements.push(Self::create_let(
                factory,
                extra_initializers_name,
                Some(empty),
            ));
            class_definition_statements.push(Self::create_let(factory, class_this, None));

            if !class_decorators.is_empty() && ci.borrow().has_static_private_class_elements {
                should_transform_private_static_elements_in_class = true;
                self.should_transform_private_static_elements_in_file
                    .set(true);
            }
        }

        // 2. ClassHeritage clause is evaluated outside of the private name scope of the class.
        let extends_clause = self.q(visitor.factory(), |view| {
            tsr_ast::utilities_class::get_heritage_clause(view, node, K::ExtendsKeyword)
        });
        let mut extends_element = None;
        if let Some(extends_clause) = extends_clause {
            let types = visitor
                .factory()
                .node(extends_clause)
                .as_heritage_clause()
                .expect("HeritageClause payload")
                .types();
            if let Some(&first) = list_nodes(visitor.factory(), types).first() {
                extends_element = Some(first);
            }
        }
        let mut extends_expression = None;
        if let Some(extends_element) = extends_element {
            let expression = visitor.factory().node(extends_element).expression();
            extends_expression = self.visit_node(visitor, Visit::Main, expression);
        }

        if let Some(extends_expression) = extends_expression {
            let factory = visitor.factory_mut();
            // Rewrite `super` in static initializers so that we can use the correct `this`.
            let class_super = ec.new_unique_name_ex(
                factory,
                text(b"_classSuper"),
                options(g::OPTIMISTIC | g::FILE_LEVEL),
            );
            ci.borrow_mut().class_super = Some(class_super);

            // Ensure we do not give the class or function an assigned name due to the variable by prefixing it
            // with `0, `.
            let unwrapped = skip_outer_expressions_all(factory, extends_expression);
            let mut safe_extends_expression = extends_expression;
            let unwrapped_kind = kind_of(factory, unwrapped);
            let unnamed = factory.node(unwrapped).name().is_none();
            if (matches!(unwrapped_kind, K::ClassExpression | K::FunctionExpression) && unnamed)
                || unwrapped_kind == K::ArrowFunction
            {
                let zero = factory.new_numeric_literal(text(b"0"), 0);
                safe_extends_expression =
                    ec.new_comma_expression(factory, zero, extends_expression);
            }
            class_definition_statements.push(Self::create_let(
                factory,
                class_super,
                Some(safe_extends_expression),
            ));

            let extends_element = extends_element.expect(NIL);
            let updated_extends_element = factory.update_expression_with_type_arguments(
                extends_element,
                Some(class_super),
                None,
            );
            let extends_clause = extends_clause.expect(NIL);
            let token = factory
                .node(extends_clause)
                .as_heritage_clause()
                .expect("HeritageClause payload")
                .token();
            let types = new_node_list(factory, vec![updated_extends_element]);
            let updated_extends_clause =
                factory.update_heritage_clause(extends_clause, token, Some(types));
            heritage_clauses = Some(new_node_list(factory, vec![updated_extends_clause]));
        }

        let class_this = ci.borrow().class_this;
        let renamed_class_this = match class_this {
            Some(class_this) => class_this,
            None => ec.new_this_expression(visitor.factory_mut()),
        };

        // 3. The name of the class is assigned.
        //
        // If the class did not have a name, the caller should have performed injectClassNamedEvaluationHelperBlockIfMissing
        // prior to calling this function if a name was needed.

        // 4. For each member:
        //    a. Member Decorators are evaluated
        //    b. Computed Property Name is evaluated, if present
        //
        // We visit members in two passes:
        // - The first pass visits methods, accessors, and fields to collect decorators and computed property names.
        // - The second pass visits the constructor to add instance initializers.
        //
        // NOTE: If there are no constructors, but there are instance initializers, a synthetic constructor is added.
        self.enter_class(Some(ci.clone()));

        let (metadata_reference, class_super) = {
            let ci = ci.borrow();
            (ci.metadata_reference.expect(NIL), ci.class_super)
        };
        leading_block_statements.push(self.create_metadata(
            visitor.factory_mut(),
            metadata_reference,
            class_super,
        ));

        // Since the constructor can appear anywhere in the class body and its transform depends on other class elements,
        // we must first visit all non-constructor members, then visit the constructor, all while maintaining document order.
        let member_list = visitor.factory().node(node).member_list();
        let members = self.visit_nodes(visitor, Visit::NonConstructorClassElement, member_list);
        let mut members = self
            .visit_nodes(visitor, Visit::ConstructorClassElement, members)
            .expect(NIL);

        // Handle pending expressions (computed property names and decorator evaluations)
        let pending = self.pending_expressions.borrow().clone();
        if !pending.is_empty() {
            // If a pending expression contains a lexical `this`, we'll need to capture the lexical `this` of the
            // container and transform it in the expression. This ensures we use the correct `this` in the resulting
            // class `static` block. We don't use substitution here because the size of the tree we are visiting
            // is likely to be small and doesn't justify the complexity of introducing substitution.
            self.outer_this.set(None);
            for mut expr in pending {
                // If a pending expression contains lexical `this`, capture it
                if self.facts(visitor, expr) & subtree_flags::LEXICAL_THIS != 0 {
                    expr = self
                        .visit_node(visitor, Visit::OuterThis, Some(expr))
                        .expect(NIL);
                }
                let statement = visitor.factory_mut().new_expression_statement(Some(expr));
                leading_block_statements.push(statement);
            }
            if let Some(outer_this) = self.outer_this.get() {
                let factory = visitor.factory_mut();
                let this = ec.new_this_expression(factory);
                let statement = Self::create_let(factory, outer_this, Some(this));
                class_definition_statements.insert(0, statement);
            }
            self.pending_expressions.borrow_mut().clear();
        }
        self.exit_class();

        // If there are instance initializers but no constructor, synthesize one
        if !ci.borrow().pending_instance_initializers.is_empty()
            && self
                .q(visitor.factory(), |view| {
                    tsr_ast::utilities_class::get_first_constructor_with_body(view, node)
                })
                .is_none()
        {
            let initializer_statements = self.prepare_constructor(visitor, &ci);
            if !initializer_statements.is_empty() {
                let factory = visitor.factory_mut();
                let is_derived_class = extends_element.is_some_and(|element| {
                    let expression = factory.node(element).expression().expect(NIL);
                    factory
                        .node(skip_outer_expressions_all(factory, expression))
                        .kind()
                        != K::NullKeyword
                });
                let mut constructor_statements = Vec::new();
                if is_derived_class {
                    let arguments = factory.new_identifier(text(b"arguments"));
                    let spread_arguments = factory.new_spread_element(Some(arguments));
                    let super_keyword = factory.new_keyword_expression(K::SuperKeyword.into());
                    let arguments_list = new_node_list(factory, vec![spread_arguments]);
                    let super_call = factory.new_call_expression(
                        Some(super_keyword),
                        None,
                        None,
                        Some(arguments_list),
                        node_flags::NONE,
                    );
                    constructor_statements.push(factory.new_expression_statement(Some(super_call)));
                }
                constructor_statements.extend(initializer_statements);
                let statements = new_node_list(factory, constructor_statements);
                let constructor_body = factory.new_block(Some(statements), true);
                let parameters = new_node_list(factory, Vec::new());
                synthetic_constructor = Some(factory.new_constructor_declaration(
                    None,
                    None,
                    Some(parameters),
                    None,
                    None,
                    Some(constructor_body),
                ));
            }
        }

        // Used in class definition steps 5,7,11
        let static_name = ci.borrow().static_method_extra_initializers_name;
        if let Some(name) = static_name {
            let factory = visitor.factory_mut();
            let empty = Self::new_empty_array(factory);
            class_definition_statements.push(Self::create_let(factory, name, Some(empty)));
        }

        // Used in class definition steps 6,8, and construction
        let instance_name = ci.borrow().instance_method_extra_initializers_name;
        if let Some(name) = instance_name {
            let factory = visitor.factory_mut();
            let empty = Self::new_empty_array(factory);
            class_definition_statements.push(Self::create_let(factory, name, Some(empty)));
        }

        // Used in class definition steps 7, 8, 12, and construction.
        // Emit member info variable declarations; the reference implementation emits static member vars first, then non-static.
        if !ci.borrow().member_infos.is_empty() {
            let statics = self.emit_member_info_declarations(visitor, &ci, true);
            class_definition_statements.extend(statics);
            let instances = self.emit_member_info_declarations(visitor, &ci, false);
            class_definition_statements.extend(instances);
        }

        {
            let ci = ci.borrow();
            // 5. Static non-field element decorators are applied
            leading_block_statements.extend_from_slice(&ci.static_non_field_decoration_statements);

            // 6. Non-static non-field element decorators are applied
            leading_block_statements
                .extend_from_slice(&ci.non_static_non_field_decoration_statements);

            // 7. Static field element decorators are applied
            leading_block_statements.extend_from_slice(&ci.static_field_decoration_statements);

            // 8. Non-static field element decorators are applied
            leading_block_statements.extend_from_slice(&ci.non_static_field_decoration_statements);
        }

        // 9. Class decorators are applied
        // 10. Class binding is initialized
        //
        // produces:
        //   __esDecorate(null, _classDescriptor = { value: this }, _classDecorators, { kind: "class", name: this.name, metadata }, null, _classExtraInitializers);
        let (descriptor_name, decorators_name, extra_initializers_name) = {
            let ci = ci.borrow();
            (
                ci.class_descriptor_name,
                ci.class_decorators_name,
                ci.class_extra_initializers_name,
            )
        };
        let factory = visitor.factory_mut();
        if let (
            Some(descriptor_name),
            Some(decorators_name),
            Some(extra_initializers_name),
            Some(class_this),
        ) = (
            descriptor_name,
            decorators_name,
            extra_initializers_name,
            class_this,
        ) {
            let value = factory.new_identifier(text(b"value"));
            let value_property = factory.new_property_assignment(
                None,
                Some(value),
                None,
                None,
                Some(renamed_class_this),
            );
            let properties = new_node_list(factory, vec![value_property]);
            let class_descriptor = factory.new_object_literal_expression(Some(properties), false);
            let class_descriptor_assignment =
                ec.new_assignment_expression(factory, descriptor_name, class_descriptor);
            let name = factory.new_identifier(text(b"name"));
            let class_name_reference = factory.new_property_access_expression(
                Some(renamed_class_this),
                None,
                Some(name),
                node_flags::NONE,
            );

            let context_obj = ec.new_es_decorate_class_context_object(
                factory,
                Some(class_name_reference),
                Some(metadata_reference),
            );

            let ctor = Self::new_null(factory);
            let initializers = Self::new_null(factory);
            let es_decorate_helper = ec.new_es_decorate_helper(
                factory,
                ctor,
                class_descriptor_assignment,
                decorators_name,
                context_obj,
                initializers,
                extra_initializers_name,
            );
            let es_decorate_statement = factory.new_expression_statement(Some(es_decorate_helper));
            let range = move_range_past_decorators(factory, node);
            ec.set_source_map_range(es_decorate_statement, range);
            leading_block_statements.push(es_decorate_statement);

            // produces:
            //   C = _classThis = _classDescriptor.value;
            let value = factory.new_identifier(text(b"value"));
            let class_descriptor_value_ref = factory.new_property_access_expression(
                Some(descriptor_name),
                None,
                Some(value),
                node_flags::NONE,
            );
            let class_this_assignment =
                ec.new_assignment_expression(factory, class_this, class_descriptor_value_ref);
            let class_reference_assignment =
                ec.new_assignment_expression(factory, class_reference, class_this_assignment);
            leading_block_statements
                .push(factory.new_expression_statement(Some(class_reference_assignment)));
        }

        // produces:
        //   if (metadata) Object.defineProperty(C, Symbol.metadata, { configurable: true, writable: true, value: metadata });
        leading_block_statements.push(self.create_symbol_metadata(
            factory,
            renamed_class_this,
            metadata_reference,
        ));

        // 11. Static extra initializers
        // 12. Static fields are initialized
        let pending_static_initializers =
            std::mem::take(&mut ci.borrow_mut().pending_static_initializers);
        for initializer in pending_static_initializers {
            let initializer_statement = factory.new_expression_statement(Some(initializer));
            let range = ec.source_map_range(factory, initializer);
            ec.set_source_map_range(initializer_statement, range);
            trailing_block_statements.push(initializer_statement);
        }

        // 13. Class extra initializers
        if let Some(extra_initializers_name) = extra_initializers_name {
            let run_class_initializers_helper = ec.new_run_initializers_helper(
                factory,
                renamed_class_this,
                extra_initializers_name,
                None,
            );
            let run_class_initializers_statement =
                factory.new_expression_statement(Some(run_class_initializers_helper));
            let range = match factory.node(node).name() {
                Some(name) => factory.node(name).range(),
                None => move_range_past_decorators(factory, node),
            };
            ec.set_source_map_range(run_class_initializers_statement, range);
            trailing_block_statements.push(run_class_initializers_statement);
        }

        // If there are no other static initializers to run, combine the leading and trailing block statements
        if !leading_block_statements.is_empty()
            && !trailing_block_statements.is_empty()
            && !ci.borrow().has_static_initializers
        {
            leading_block_statements.append(&mut trailing_block_statements);
        }

        // prepare a leading `static {}` block, if necessary
        //
        // produces:
        //   class C {
        //       static { ... }
        //       ...
        //   }
        let mut leading_static_block = None;
        if !leading_block_statements.is_empty() {
            let statements = new_node_list(factory, leading_block_statements);
            let block = factory.new_block(Some(statements), true);
            leading_static_block =
                Some(factory.new_class_static_block_declaration(None, Some(block)));
        }

        if let Some(leading_static_block) = leading_static_block {
            if should_transform_private_static_elements_in_class {
                // We use EFTransformPrivateStaticElements as a marker on a class static block
                // to inform the classFields transform that it shouldn't rename `this` to `_classThis` in the
                // transformed class static block.
                ec.set_emit_flags(
                    leading_static_block,
                    emit_flags::TRANSFORM_PRIVATE_STATIC_ELEMENTS,
                );
            }
        }

        // prepare a trailing `static {}` block, if necessary
        //
        // produces:
        //   class C {
        //       ...
        //       static { ... }
        //   }
        let mut trailing_static_block = None;
        if !trailing_block_statements.is_empty() {
            let statements = new_node_list(factory, trailing_block_statements);
            let block = factory.new_block(Some(statements), true);
            trailing_static_block =
                Some(factory.new_class_static_block_declaration(None, Some(block)));
        }

        // Assemble new members list
        if leading_static_block.is_some()
            || synthetic_constructor.is_some()
            || trailing_static_block.is_some()
        {
            let member_nodes = list_nodes(factory, Some(members));
            let mut new_members = Vec::with_capacity(member_nodes.len() + 3);

            // Find the existing NamedEvaluation helper block index
            let existing_named_evaluation_helper_block_index = member_nodes
                .iter()
                .position(|&member| is_class_named_evaluation_helper_block(&ec, factory, member));
            let split = existing_named_evaluation_helper_block_index.map_or(0, |index| index + 1);

            // add the leading `static {}` block
            if let Some(leading_static_block) = leading_static_block {
                // add the `static {}` block after any existing NamedEvaluation helper block, if one exists.
                new_members.extend_from_slice(&member_nodes[..split]);
                new_members.push(leading_static_block);
                new_members.extend_from_slice(&member_nodes[split..]);
            } else {
                new_members.extend_from_slice(&member_nodes);
            }

            // append the synthetic constructor, if necessary
            if let Some(synthetic_constructor) = synthetic_constructor {
                new_members.push(synthetic_constructor);
            }

            // append a trailing `static {}` block, if necessary
            if let Some(trailing_static_block) = trailing_static_block {
                new_members.push(trailing_static_block);
            }

            let members_list = new_node_list(factory, new_members);
            let loc = factory.read_list(members).loc();
            factory.set_list_location(members_list, loc);
            members = members_list;
        }

        let lexical_environment = ec.end_variable_environment(factory);

        let class_expression;
        if class_decorators.is_empty() {
            // produces:
            //   return <classExpression>;
            let name = factory.node(node).name();
            class_expression =
                factory.new_class_expression(None, name, None, heritage_clauses, Some(members));
            ec.set_original(class_expression, node);
            class_definition_statements.push(factory.new_return_statement(Some(class_expression)));
        } else {
            let mut expression =
                factory.new_class_expression(None, None, None, heritage_clauses, Some(members));
            ec.set_original(expression, node);
            if let Some(class_this) = class_this {
                expression =
                    inject_class_this_assignment_if_missing(&ec, factory, expression, class_this);
            }
            class_expression = expression;

            // We use `var` instead of `let` so we can leverage NamedEvaluation to define the class name
            // and still be able to ensure it is initialized prior to any use in `static {}`.

            // produces:
            //   (() => {
            //       let _classDecorators = [...];
            //       let _classDescriptor;
            //       let _classExtraInitializers = [];
            //       let _classThis;
            //       ...
            //       var C = class {
            //           static {
            //               __esDecorate(null, _classDescriptor = { value: this }, _classDecorators, ...);
            //               C = _classThis = _classDescriptor.value;
            //           }
            //           static x = 1;
            //           static y = C.x; // `C` will already be defined here.
            //           static { ... }
            //       };
            //       return C;
            //   })();

            let class_reference_declaration = factory.new_variable_declaration(
                Some(class_reference),
                None,
                None,
                Some(class_expression),
            );
            let declarations = new_node_list(factory, vec![class_reference_declaration]);
            let class_reference_var_decl_list =
                factory.new_variable_declaration_list(Some(declarations), node_flags::NONE);
            let return_expr = match class_this {
                Some(class_this) => {
                    ec.new_assignment_expression(factory, class_reference, class_this)
                }
                None => class_reference,
            };
            class_definition_statements
                .push(factory.new_variable_statement(None, Some(class_reference_var_decl_list)));
            class_definition_statements.push(factory.new_return_statement(Some(return_expr)));
        }

        if should_transform_private_static_elements_in_class {
            ec.add_emit_flags(
                class_expression,
                emit_flags::TRANSFORM_PRIVATE_STATIC_ELEMENTS,
            );
            let class_members = list_nodes(
                visitor.factory(),
                visitor.factory().node(class_expression).member_list(),
            );
            for member in class_members {
                if (self.is_private_identifier_class_element_declaration(visitor, member)
                    || self.is_auto_accessor_property_declaration(visitor, member))
                    && self.has_static_modifier(visitor, member)
                {
                    ec.add_emit_flags(member, emit_flags::TRANSFORM_PRIVATE_STATIC_ELEMENTS);
                }
            }
        }

        let factory = visitor.factory_mut();
        let merged_statements =
            ec.merge_environment(factory, class_definition_statements, &lexical_environment);
        ec.new_immediately_invoked_arrow_function(factory, merged_statements)
    }

    /// Generates let declarations for member decorator info variables,
    /// filtered by static/non-static.
    // port: tsc/internal/transformers/estransforms/esdecorator.go:esDecoratorTransformer.emitMemberInfoDeclarations
    fn emit_member_info_declarations(
        &self,
        visitor: &mut NodeVisitor<'_>,
        ci: &ClassInfoRef,
        is_static: bool,
    ) -> Vec<NodeId> {
        let entries = ci.borrow().member_infos.clone();
        let mut stmts = Vec::new();
        for (member, mi) in entries {
            if self.is_static(visitor, member) != is_static {
                continue;
            }
            let mi = mi.borrow();
            let factory = visitor.factory_mut();
            stmts.push(Self::create_let(factory, mi.member_decorators_name, None));
            if let Some(name) = mi.member_initializers_name {
                let empty = Self::new_empty_array(factory);
                stmts.push(Self::create_let(factory, name, Some(empty)));
            }
            if let Some(name) = mi.member_extra_initializers_name {
                let empty = Self::new_empty_array(factory);
                stmts.push(Self::create_let(factory, name, Some(empty)));
            }
            if let Some(name) = mi.member_descriptor_name {
                stmts.push(Self::create_let(factory, name, None));
            }
        }
        stmts
    }

    // port: tsc/internal/transformers/estransforms/esdecorator.go:isDecoratedClassLike
    fn is_decorated_class_like(&self, factory: &dyn RuntimeFactory, node: NodeId) -> bool {
        self.q(factory, |view| {
            tsr_ast::utilities_class::class_or_constructor_parameter_is_decorated(view, false, node)
        }) || self.q(factory, |view| {
            tsr_ast::utilities_class::child_is_decorated(view, false, node, None)
        })
    }

    // port: tsc/internal/transformers/estransforms/esdecorator.go:esDecoratorTransformer.visitClassDeclaration
    #[allow(clippy::too_many_lines)] // upstream's single function, kept in its order
    fn visit_class_declaration(&self, visitor: &mut NodeVisitor<'_>, node: NodeId) -> NodeId {
        if self.is_decorated_class_like(visitor.factory(), node) {
            let mut ec = self.ctx();
            let mut statements = Vec::new();

            let mut original_class = ec.most_original(node);
            if !matches!(
                Self::kind(visitor, original_class),
                K::ClassDeclaration | K::ClassExpression
            ) {
                original_class = node;
            }
            let class_name = match Self::name_of(visitor, original_class) {
                Some(name) => ec.new_string_literal_from_node(visitor.factory_mut(), name),
                None => visitor
                    .factory_mut()
                    .new_string_literal(text(b"default"), 0),
            };

            let is_export = self.has_syntactic_modifier(visitor, node, modifier_flags::EXPORT);
            let is_default = self.has_syntactic_modifier(visitor, node, modifier_flags::DEFAULT);

            let mut class_node = node;
            if Self::name_of(visitor, node).is_none() {
                class_node = inject_class_named_evaluation_helper_block_if_missing(
                    &ec,
                    visitor.factory_mut(),
                    class_node,
                    class_name,
                    None,
                );
            }

            if is_export && is_default {
                let iife = self.transform_class_like(visitor, class_node);
                if Self::name_of(visitor, class_node).is_some() {
                    // produces:
                    //   let C = (() => { ... })();
                    //   export default C;
                    let local_name = self.with_builder(visitor, |ec, builder| {
                        ec.get_local_name(builder, Some(class_node))
                    });
                    let factory = visitor.factory_mut();
                    let var_decl =
                        factory.new_variable_declaration(Some(local_name), None, None, Some(iife));
                    ec.set_original(var_decl, class_node);
                    let list = new_node_list(factory, vec![var_decl]);
                    let var_decls =
                        factory.new_variable_declaration_list(Some(list), node_flags::LET);
                    let var_statement = factory.new_variable_statement(None, Some(var_decls));
                    statements.push(var_statement);

                    let declaration_name = self.with_builder(visitor, |ec, builder| {
                        ec.get_declaration_name(builder, Some(class_node))
                    });
                    let factory = visitor.factory_mut();
                    let export_statement = ec.new_export_default(factory, declaration_name);
                    ec.set_original(export_statement, class_node);
                    ec.assign_comment_range(factory, export_statement, class_node);
                    let range = move_range_past_decorators(factory, class_node);
                    ec.set_source_map_range(export_statement, range);
                    statements.push(export_statement);
                } else {
                    // produces:
                    //   export default (() => { ... })();
                    let factory = visitor.factory_mut();
                    let export_statement = ec.new_export_default(factory, iife);
                    ec.set_original(export_statement, class_node);
                    ec.assign_comment_range(factory, export_statement, class_node);
                    let range = move_range_past_decorators(factory, class_node);
                    ec.set_source_map_range(export_statement, range);
                    statements.push(export_statement);
                }
            } else {
                debug_assert(Self::name_of(visitor, class_node).is_some(), || {
                    "A class declaration that is not a default export must have a name.".to_owned()
                });
                // produces:
                //   let C = (() => { ... })();
                let iife = self.transform_class_like(visitor, class_node);
                let class_modifiers = visitor.factory().node(class_node).modifiers();
                let modifiers =
                    self.visit_modifiers(visitor, Visit::ExportStrippingModifier, class_modifiers);

                let decl_name = self.with_builder(visitor, |ec, builder| {
                    ec.get_local_name_ex(
                        builder,
                        Some(class_node),
                        AssignedNameOptions {
                            allow_source_maps: true,
                            ..AssignedNameOptions::default()
                        },
                    )
                });
                let factory = visitor.factory_mut();
                let var_decl =
                    factory.new_variable_declaration(Some(decl_name), None, None, Some(iife));
                ec.set_original(var_decl, class_node);
                let list = new_node_list(factory, vec![var_decl]);
                let var_decls = factory.new_variable_declaration_list(Some(list), node_flags::LET);
                let var_statement = factory.new_variable_statement(modifiers, Some(var_decls));
                ec.set_original(var_statement, class_node);
                ec.assign_comment_range(factory, var_statement, class_node);
                statements.push(var_statement);

                if is_export {
                    // produces:
                    //   export { C };
                    let export_statement = ec.new_external_module_export(factory, decl_name);
                    ec.set_original(export_statement, class_node);
                    statements.push(export_statement);
                }
            }

            return single_or_many(visitor.factory_mut(), Some(&statements)).expect(NIL);
        }

        // Non-decorated class
        let (modifiers, name, heritage_clauses, members) = {
            let read = visitor.factory().node(node);
            (
                read.modifiers(),
                read.name(),
                read.as_class_declaration()
                    .expect("ClassDeclaration payload")
                    .heritage_clauses(),
                read.member_list(),
            )
        };
        let modifiers = self.visit_modifiers(visitor, Visit::Modifier, modifiers);
        let heritage_clauses = self.visit_nodes(visitor, Visit::Main, heritage_clauses);
        self.enter_class(None);
        let members = self.visit_nodes(visitor, Visit::ClassElement, members);
        self.exit_class();
        visitor.factory_mut().update_class_declaration(
            node,
            modifiers,
            name,
            None,
            heritage_clauses,
            members,
        )
    }

    // port: tsc/internal/transformers/estransforms/esdecorator.go:esDecoratorTransformer.visitClassExpression
    fn visit_class_expression(&self, visitor: &mut NodeVisitor<'_>, node: NodeId) -> NodeId {
        if self.is_decorated_class_like(visitor.factory(), node) {
            let iife = self.transform_class_like(visitor, node);
            self.ctx().set_original(iife, node);
            return iife;
        }

        let (modifiers, name, heritage_clauses, members) = {
            let read = visitor.factory().node(node);
            (
                read.modifiers(),
                read.name(),
                read.as_class_expression()
                    .expect("ClassExpression payload")
                    .heritage_clauses(),
                read.member_list(),
            )
        };
        let modifiers = self.visit_modifiers(visitor, Visit::Modifier, modifiers);
        let heritage_clauses = self.visit_nodes(visitor, Visit::Main, heritage_clauses);
        self.enter_class(None);
        let members = self.visit_nodes(visitor, Visit::ClassElement, members);
        self.exit_class();
        visitor.factory_mut().update_class_expression(
            node,
            modifiers,
            name,
            None,
            heritage_clauses,
            members,
        )
    }

    // port: tsc/internal/transformers/estransforms/esdecorator.go:esDecoratorTransformer.prepareConstructor
    fn prepare_constructor(&self, visitor: &mut NodeVisitor<'_>, ci: &ClassInfoRef) -> Vec<NodeId> {
        // Decorated instance members can add "extra" initializers to the instance. If a class contains any instance
        // fields, we'll inject the `__runInitializers()` call for these extra initializers into the initializer of
        // the first class member that will be initialized. However, if the class does not contain any fields that
        // we can piggyback on, we need to synthesize a `__runInitializers()` call in the constructor instead.
        let pending = ci.borrow().pending_instance_initializers.clone();
        if pending.is_empty() {
            return Vec::new();
        }
        let factory = visitor.factory_mut();
        let expression = self.emit_context.inline_expressions(factory, &pending);
        let statements = vec![factory.new_expression_statement(expression)];
        ci.borrow_mut().pending_instance_initializers.clear();
        statements
    }

    // port: tsc/internal/transformers/estransforms/esdecorator.go:esDecoratorTransformer.transformConstructorBodyWorker
    #[allow(clippy::too_many_arguments)] // upstream's positional signature
    fn transform_constructor_body_worker(
        &self,
        visitor: &mut NodeVisitor<'_>,
        mut statements_out: Vec<NodeId>,
        statements_in: &[NodeId],
        statement_offset: usize,
        super_path: &[usize],
        super_path_depth: usize,
        initializer_statements: &[NodeId],
    ) -> Vec<NodeId> {
        let super_statement_index = super_path[super_path_depth];
        // Visit statements before super
        if super_statement_index > statement_offset {
            for &s in &statements_in[statement_offset..super_statement_index] {
                statements_out.push(self.visit_node(visitor, Visit::Main, Some(s)).expect(NIL));
            }
        }

        let super_statement = statements_in[super_statement_index];
        if Self::kind(visitor, super_statement) == K::TryStatement {
            // Recurse into try block
            let (try_block_node, catch_clause, finally_block) = {
                let read = visitor.factory().node(super_statement);
                let data = read.as_try_statement().expect("TryStatement payload");
                (
                    data.try_block().expect(NIL),
                    data.catch_clause(),
                    data.finally_block(),
                )
            };
            let try_statements = list_nodes(
                visitor.factory(),
                Some(
                    visitor
                        .factory()
                        .node(try_block_node)
                        .statement_list()
                        .expect(NIL),
                ),
            );
            let try_block_statements = self.transform_constructor_body_worker(
                visitor,
                Vec::new(),
                &try_statements,
                0,
                super_path,
                super_path_depth + 1,
                initializer_statements,
            );

            let factory = visitor.factory_mut();
            let statements = new_node_list(factory, try_block_statements);
            let new_try_block = factory.new_block(Some(statements), true);
            // Use the original try block's range even though the statements may differ due to
            // injected initializer statements. This preserves source map fidelity for the enclosing
            // try statement.
            let loc = factory.node(try_block_node).range();
            factory.set_node_range(new_try_block, loc);

            let catch_clause = if catch_clause.is_some() {
                self.visit_node(visitor, Visit::Main, catch_clause)
            } else {
                None
            };
            let finally_block = if finally_block.is_some() {
                self.visit_node(visitor, Visit::Main, finally_block)
            } else {
                None
            };
            let updated = visitor.factory_mut().update_try_statement(
                super_statement,
                Some(new_try_block),
                catch_clause,
                finally_block,
            );
            statements_out.push(updated);
        } else {
            statements_out.push(
                self.visit_node(visitor, Visit::Main, Some(super_statement))
                    .expect(NIL),
            );
            statements_out.extend_from_slice(initializer_statements);
        }

        // Visit statements after super
        if super_statement_index + 1 < statements_in.len() {
            for &s in &statements_in[super_statement_index + 1..] {
                statements_out.push(self.visit_node(visitor, Visit::Main, Some(s)).expect(NIL));
            }
        }
        statements_out
    }

    // port: tsc/internal/transformers/estransforms/esdecorator.go:esDecoratorTransformer.visitConstructorDeclaration
    fn visit_constructor_declaration(&self, visitor: &mut NodeVisitor<'_>, node: NodeId) -> NodeId {
        self.enter_class_element(visitor, node);
        let (modifiers, parameters, ctor_body) = {
            let read = visitor.factory().node(node);
            (read.modifiers(), read.parameter_list(), read.body())
        };
        let modifiers = self.visit_modifiers(visitor, Visit::Modifier, modifiers);
        let parameters = self.visit_nodes(visitor, Visit::Main, parameters);

        let mut body = None;
        if let (Some(ctor_body), Some(ci)) = (ctor_body, self.class_info()) {
            // If there are instance extra initializers we need to add them to the body along with any
            // field initializers
            let initializer_statements = self.prepare_constructor(visitor, &ci);
            if !initializer_statements.is_empty() {
                let body_statements = list_nodes(
                    visitor.factory(),
                    Some(
                        visitor
                            .factory()
                            .node(ctor_body)
                            .statement_list()
                            .expect(NIL),
                    ),
                );
                let (prologue, rest) = self
                    .emit_context
                    .split_standard_prologue(visitor.factory(), &body_statements);
                let mut stmts = prologue.to_vec();
                let rest = rest.to_vec();

                let super_statement_indices =
                    find_super_statement_index_path(visitor.factory(), &rest, 0);
                if super_statement_indices.is_empty() {
                    stmts.extend_from_slice(&initializer_statements);
                    let slice = visitor
                        .factory_mut()
                        .alloc_nodes(rest.iter().copied().map(Some).collect());
                    let (visited, _) = self.with(visitor, Visit::Main, |v| v.visit_slice(slice));
                    stmts.extend(
                        visitor
                            .factory()
                            .read_nodes(visited)
                            .iter()
                            .map(|node| node.expect(NIL)),
                    );
                } else {
                    stmts = self.transform_constructor_body_worker(
                        visitor,
                        stmts,
                        &rest,
                        0,
                        &super_statement_indices,
                        0,
                        &initializer_statements,
                    );
                }

                let factory = visitor.factory_mut();
                let statements = new_node_list(factory, stmts);
                let new_body = factory.new_block(Some(statements), true);
                self.ctx().set_original(new_body, ctor_body);
                let loc = factory.node(ctor_body).range();
                factory.set_node_range(new_body, loc);
                body = Some(new_body);
            }
        }

        if body.is_none() {
            body = self.visit_node(visitor, Visit::Main, ctor_body);
        }
        self.exit_class_element();
        visitor
            .factory_mut()
            .update_constructor_declaration(node, modifiers, None, parameters, None, None, body)
    }

    // port: tsc/internal/transformers/estransforms/esdecorator.go:esDecoratorTransformer.finishClassElement
    fn finish_class_element(
        &self,
        visitor: &NodeVisitor<'_>,
        updated: NodeId,
        original: NodeId,
    ) -> NodeId {
        if updated != original {
            // While we emit the source map for the node after skipping decorators and modifiers,
            // we need to emit the comments for the original range.
            let mut ec = self.ctx();
            let factory = visitor.factory();
            ec.assign_comment_range(factory, updated, original);
            let range = move_range_past_decorators(factory, original);
            ec.set_source_map_range(updated, range);
        }
        updated
    }

    // port: tsc/internal/transformers/estransforms/esdecorator.go:esDecoratorTransformer.partialTransformClassElement
    #[allow(clippy::too_many_lines)] // upstream's single function, kept in its order
    fn partial_transform_class_element(
        &self,
        visitor: &mut NodeVisitor<'_>,
        member: NodeId,
        ci: Option<ClassInfoRef>,
        create_descriptor: Option<Descriptor>,
    ) -> PartialResult {
        let mut ec = self.ctx();
        let (member_modifiers, member_name) = {
            let read = visitor.factory().node(member);
            (read.modifiers(), read.name())
        };

        let Some(ci) = ci else {
            let modifiers = self.visit_modifiers(visitor, Visit::Modifier, member_modifiers);
            self.enter_name();
            let name = self.visit_property_name(visitor, member_name);
            self.exit_name();
            return PartialResult {
                modifiers,
                name,
                ..PartialResult::default()
            };
        };

        // Member decorators require privileged access to private names. However, computed property
        // evaluation occurs interspersed with decorator evaluation. This means that if we encounter
        // a computed property name we must inline decorator evaluation.

        // Collect decorators for this member. Decorator expressions evaluate outside the class body,
        // so `this` should NOT be replaced with `_classThis`.
        let saved_class_this = self.class_this.get();
        self.class_this.set(None);
        let decorators = self.decorators(visitor, member);
        let member_decorators = self.transform_all_decorators_of_declaration(visitor, &decorators);
        self.class_this.set(saved_class_this);
        let modifiers = self.visit_modifiers(visitor, Visit::Modifier, member_modifiers);

        let mut result = PartialResult {
            modifiers,
            ..PartialResult::default()
        };

        if !member_decorators.is_empty() {
            let member_decorators_name =
                self.create_helper_variable(visitor, member, b"decorators");
            let factory = visitor.factory_mut();
            let elements = new_node_list(factory, member_decorators);
            let member_decorators_array =
                factory.new_array_literal_expression(Some(elements), false);
            let member_decorators_assignment = ec.new_assignment_expression(
                factory,
                member_decorators_name,
                member_decorators_array,
            );
            let mi = Rc::new(RefCell::new(MemberInfo {
                member_decorators_name,
                member_initializers_name: None,
                member_extra_initializers_name: None,
                member_descriptor_name: None,
            }));
            ci.borrow_mut().set_member_info(member, mi.clone());
            self.pending_expressions
                .borrow_mut()
                .push(member_decorators_assignment);

            // 5. Static non-field (method/getter/setter/auto-accessor) element decorators are applied
            // 6. Non-static non-field (method/getter/setter/auto-accessor) element decorators are applied
            // 7. Static field (excl. auto-accessor) element decorators are applied
            // 8. Non-static field (excl. auto-accessor) element decorators are applied

            // Determine decorator kind
            let member_kind = Self::kind(visitor, member);
            let kind: &[u8] = match member_kind {
                K::GetAccessor => b"getter",
                K::SetAccessor => b"setter",
                K::MethodDeclaration => b"method",
                _ if self.is_auto_accessor_property_declaration(visitor, member) => b"accessor",
                K::PropertyDeclaration => b"field",
                _ => panic!("Debug failure. Unexpected class element kind."),
            };

            // Determine the property name for the context
            let mut property_name_computed = false;
            let mut property_name_expr = None;
            let name_kind = member_name.map(|name| Self::kind(visitor, name));
            if let (Some(name), Some(name_kind)) = (member_name, name_kind) {
                if name_kind == K::Identifier || name_kind == K::PrivateIdentifier {
                    property_name_computed = false;
                    property_name_expr = Some(name);
                } else if tsr_ast::utilities::is_property_name_literal_kind(name_kind.into()) {
                    property_name_computed = true;
                    property_name_expr =
                        Some(ec.new_string_literal_from_node(visitor.factory_mut(), name));
                } else if name_kind == K::ComputedPropertyName {
                    let expression = visitor.factory().node(name).expression().expect(NIL);
                    let expression_kind = Self::kind(visitor, expression);
                    if tsr_ast::utilities::is_property_name_literal_kind(expression_kind.into())
                        && expression_kind != K::Identifier
                    {
                        property_name_computed = true;
                        property_name_expr = Some(
                            ec.new_string_literal_from_node(visitor.factory_mut(), expression),
                        );
                    } else {
                        self.enter_name();
                        let (referenced_name, name) =
                            self.visit_referenced_property_name(visitor, name);
                        result.referenced_name = Some(referenced_name);
                        result.name = Some(name);
                        self.exit_name();
                        property_name_computed = true;
                        property_name_expr = result.referenced_name;
                    }
                }
            }

            let is_static = self.is_static(visitor, member);
            let is_private = name_kind == Some(K::PrivateIdentifier);
            let is_property = member_kind == K::PropertyDeclaration;
            let metadata_reference = ci.borrow().metadata_reference;
            let context_obj = ec.new_es_decorate_class_element_context_object(
                visitor.factory_mut(),
                kind,
                property_name_computed,
                property_name_expr,
                is_static,
                is_private,
                // 15.7.3 CreateDecoratorAccessObject (kind, name)
                // 2. If _kind_ is ~field~, ~method~, ~accessor~, or ~getter~, then ...
                is_property || member_kind == K::GetAccessor || member_kind == K::MethodDeclaration,
                // 3. If _kind_ is ~field~, ~accessor~, or ~setter~, then ...
                is_property || member_kind == K::SetAccessor,
                metadata_reference,
            );

            if matches!(
                member_kind,
                K::MethodDeclaration | K::GetAccessor | K::SetAccessor
            ) {
                // produces (public elements):
                //   __esDecorate(this, null, _static_member_decorators, { kind: "method", name: "...", static: true, private: false, access: { ... } }, _staticExtraInitializers);
                //   __esDecorate(this, null, _member_decorators, { kind: "method", name: "...", static: false, private: false, access: { ... } }, _instanceExtraInitializers);
                //
                // produces (private elements):
                //   __esDecorate(this, _static_member_descriptor = { value() { ... } }, _static_member_decorators, { kind: "method", name: "...", static: true, private: true, access: { ... } }, _staticExtraInitializers);
                //   __esDecorate(this, _member_descriptor = { value() { ... } }, _member_decorators, { kind: "method", name: "...", static: false, private: true, access: { ... } }, _instanceExtraInitializers);
                let method_extra_initializers_name = if is_static {
                    ci.borrow().static_method_extra_initializers_name
                } else {
                    ci.borrow().instance_method_extra_initializers_name
                };
                debug_assert(method_extra_initializers_name.is_some(), || {
                    "methodExtraInitializersName should be defined".to_owned()
                });

                let descriptor_arg = match create_descriptor {
                    Some(create_descriptor)
                        if self
                            .is_private_identifier_class_element_declaration(visitor, member) =>
                    {
                        // For private members, extract the method/accessor body into a descriptor object.
                        // Filter modifiers to only keep async.
                        let async_mods =
                            self.visit_modifiers(visitor, Visit::AsyncOnlyModifier, modifiers);
                        let descriptor =
                            self.create_descriptor(visitor, create_descriptor, member, async_mods);
                        let descriptor_name =
                            self.create_helper_variable(visitor, member, b"descriptor");
                        mi.borrow_mut().member_descriptor_name = Some(descriptor_name);
                        result.descriptor_name = Some(descriptor_name);
                        ec.new_assignment_expression(
                            visitor.factory_mut(),
                            descriptor_name,
                            descriptor,
                        )
                    }
                    _ => Self::new_null(visitor.factory_mut()),
                };

                let factory = visitor.factory_mut();
                let this = ec.new_this_expression(factory);
                let initializers = Self::new_null(factory);
                let es_decorate_expr = ec.new_es_decorate_helper(
                    factory,
                    this,
                    descriptor_arg,
                    member_decorators_name,
                    context_obj,
                    initializers,
                    method_extra_initializers_name.expect(NIL),
                );
                let es_decorate_statement =
                    factory.new_expression_statement(Some(es_decorate_expr));
                let range = move_range_past_decorators(factory, member);
                ec.set_source_map_range(es_decorate_statement, range);
                self.append_decoration_statement(visitor, &ci, member, es_decorate_statement);
            } else if member_kind == K::PropertyDeclaration {
                let initializers_name =
                    self.create_helper_variable(visitor, member, b"initializers");
                let extra_initializers_name =
                    self.create_helper_variable(visitor, member, b"extraInitializers");
                {
                    let mut mi = mi.borrow_mut();
                    mi.member_initializers_name = Some(initializers_name);
                    mi.member_extra_initializers_name = Some(extra_initializers_name);
                }
                result.initializers_name = Some(initializers_name);
                result.extra_initializers_name = Some(extra_initializers_name);
                if is_static {
                    result.this_arg = ci.borrow().class_this;
                }

                let ctor_arg = if self.is_auto_accessor_property_declaration(visitor, member) {
                    ec.new_this_expression(visitor.factory_mut())
                } else {
                    Self::new_null(visitor.factory_mut())
                };

                let descriptor_arg = match create_descriptor {
                    Some(create_descriptor)
                        if self
                            .is_private_identifier_class_element_declaration(visitor, member)
                            && self.has_accessor_modifier(visitor, member) =>
                    {
                        let descriptor =
                            self.create_descriptor(visitor, create_descriptor, member, None);
                        let descriptor_name =
                            self.create_helper_variable(visitor, member, b"descriptor");
                        mi.borrow_mut().member_descriptor_name = Some(descriptor_name);
                        result.descriptor_name = Some(descriptor_name);
                        ec.new_assignment_expression(
                            visitor.factory_mut(),
                            descriptor_name,
                            descriptor,
                        )
                    }
                    _ => Self::new_null(visitor.factory_mut()),
                };

                // produces:
                //   __esDecorate(null, null, _static_member_decorators, { kind: "field", name: "...", static: true, private: ..., access: { ... } }, _staticExtraInitializers);
                //   __esDecorate(null, null, _member_decorators, { kind: "field", name: "...", static: false, private: ..., access: { ... } }, _instanceExtraInitializers);
                let factory = visitor.factory_mut();
                let es_decorate_expr = ec.new_es_decorate_helper(
                    factory,
                    ctor_arg,
                    descriptor_arg,
                    member_decorators_name,
                    context_obj,
                    initializers_name,
                    extra_initializers_name,
                );
                let es_decorate_statement =
                    factory.new_expression_statement(Some(es_decorate_expr));
                let range = move_range_past_decorators(factory, member);
                ec.set_source_map_range(es_decorate_statement, range);
                self.append_decoration_statement(visitor, &ci, member, es_decorate_statement);
            }
        }

        if result.name.is_none() {
            self.enter_name();
            result.name = self.visit_property_name(visitor, member_name);
            self.exit_name();
        }

        let modifiers_empty = modifiers.is_none_or(|list| {
            let nodes = visitor.factory().read_list(list).nodes();
            visitor.factory().read_nodes(nodes).is_empty()
        });
        if modifiers_empty
            && matches!(
                Self::kind(visitor, member),
                K::MethodDeclaration | K::PropertyDeclaration
            )
        {
            // Don't emit leading comments on the name for methods and properties without modifiers, otherwise we
            // will end up printing duplicate comments.
            ec.set_emit_flags(result.name.expect(NIL), emit_flags::NO_LEADING_COMMENTS);
        }

        result
    }

    /// `createDescriptor(member, modifiers)`.
    fn create_descriptor(
        &self,
        visitor: &mut NodeVisitor<'_>,
        which: Descriptor,
        member: NodeId,
        modifiers: Option<NodeListId>,
    ) -> NodeId {
        match which {
            Descriptor::Method => self.create_method_descriptor_object(visitor, member, modifiers),
            Descriptor::GetAccessor => {
                self.create_get_accessor_descriptor_object(visitor, member, modifiers)
            }
            Descriptor::SetAccessor => {
                self.create_set_accessor_descriptor_object(visitor, member, modifiers)
            }
            Descriptor::AccessorProperty => {
                self.create_accessor_property_descriptor_object(visitor, member)
            }
        }
    }

    /// Appends an `__esDecorate` statement to the appropriate decoration
    /// statement list on `ci` based on the member's kind and static-ness.
    // port: tsc/internal/transformers/estransforms/esdecorator.go:esDecoratorTransformer.appendDecorationStatement
    fn append_decoration_statement(
        &self,
        visitor: &NodeVisitor<'_>,
        ci: &ClassInfoRef,
        member: NodeId,
        stmt: NodeId,
    ) {
        let member_kind = Self::kind(visitor, member);
        let is_method_or_accessor = matches!(
            member_kind,
            K::MethodDeclaration | K::GetAccessor | K::SetAccessor
        );
        if is_method_or_accessor || self.is_auto_accessor_property_declaration(visitor, member) {
            if self.is_static(visitor, member) {
                ci.borrow_mut()
                    .static_non_field_decoration_statements
                    .push(stmt);
            } else {
                ci.borrow_mut()
                    .non_static_non_field_decoration_statements
                    .push(stmt);
            }
        } else if member_kind == K::PropertyDeclaration
            && !self.is_auto_accessor_property_declaration(visitor, member)
        {
            if self.is_static(visitor, member) {
                ci.borrow_mut()
                    .static_field_decoration_statements
                    .push(stmt);
            } else {
                ci.borrow_mut()
                    .non_static_field_decoration_statements
                    .push(stmt);
            }
        } else {
            panic!("Debug failure. Unexpected class element kind.");
        }
    }

    /// `tx.classInfoStack`.
    fn class_info(&self) -> Option<ClassInfoRef> {
        self.class_info_stack.borrow().clone()
    }

    // port: tsc/internal/transformers/estransforms/esdecorator.go:esDecoratorTransformer.visitMethodDeclaration
    fn visit_method_declaration(&self, visitor: &mut NodeVisitor<'_>, node: NodeId) -> NodeId {
        self.enter_class_element(visitor, node);
        let result = self.partial_transform_class_element(
            visitor,
            node,
            self.class_info(),
            Some(Descriptor::Method),
        );
        if let Some(descriptor_name) = result.descriptor_name {
            self.exit_class_element();
            let forwarder = self.create_method_descriptor_forwarder(
                visitor,
                result.modifiers,
                result.name,
                descriptor_name,
            );
            return self.finish_class_element(visitor, forwarder, node);
        }
        let (parameters, body, asterisk_token) = {
            let read = visitor.factory().node(node);
            (
                read.parameter_list(),
                read.body(),
                read.as_method_declaration()
                    .expect("MethodDeclaration payload")
                    .asterisk_token(),
            )
        };
        let parameters = self.visit_nodes(visitor, Visit::Main, parameters);
        let body = self.visit_node(visitor, Visit::Main, body);
        self.exit_class_element();
        let updated = visitor.factory_mut().update_method_declaration(
            node,
            result.modifiers,
            asterisk_token,
            result.name,
            None,
            None,
            parameters,
            None,
            None,
            body,
        );
        self.finish_class_element(visitor, updated, node)
    }

    // port: tsc/internal/transformers/estransforms/esdecorator.go:esDecoratorTransformer.visitGetAccessorDeclaration
    fn visit_get_accessor_declaration(
        &self,
        visitor: &mut NodeVisitor<'_>,
        node: NodeId,
    ) -> NodeId {
        self.enter_class_element(visitor, node);
        let result = self.partial_transform_class_element(
            visitor,
            node,
            self.class_info(),
            Some(Descriptor::GetAccessor),
        );
        if let Some(descriptor_name) = result.descriptor_name {
            self.exit_class_element();
            let forwarder = self.create_get_accessor_descriptor_forwarder(
                visitor,
                result.modifiers,
                result.name,
                descriptor_name,
            );
            return self.finish_class_element(visitor, forwarder, node);
        }
        let (parameters, body) = {
            let read = visitor.factory().node(node);
            (read.parameter_list(), read.body())
        };
        let parameters = self.visit_nodes(visitor, Visit::Main, parameters);
        let body = self.visit_node(visitor, Visit::Main, body);
        self.exit_class_element();
        let updated = visitor.factory_mut().update_get_accessor_declaration(
            node,
            result.modifiers,
            result.name,
            None,
            parameters,
            None,
            None,
            body,
        );
        self.finish_class_element(visitor, updated, node)
    }

    // port: tsc/internal/transformers/estransforms/esdecorator.go:esDecoratorTransformer.visitSetAccessorDeclaration
    fn visit_set_accessor_declaration(
        &self,
        visitor: &mut NodeVisitor<'_>,
        node: NodeId,
    ) -> NodeId {
        self.enter_class_element(visitor, node);
        let result = self.partial_transform_class_element(
            visitor,
            node,
            self.class_info(),
            Some(Descriptor::SetAccessor),
        );
        if let Some(descriptor_name) = result.descriptor_name {
            self.exit_class_element();
            let forwarder = self.create_set_accessor_descriptor_forwarder(
                visitor,
                result.modifiers,
                result.name,
                descriptor_name,
            );
            return self.finish_class_element(visitor, forwarder, node);
        }
        let (parameters, body) = {
            let read = visitor.factory().node(node);
            (read.parameter_list(), read.body())
        };
        let parameters = self.visit_nodes(visitor, Visit::Main, parameters);
        let body = self.visit_node(visitor, Visit::Main, body);
        self.exit_class_element();
        let updated = visitor.factory_mut().update_set_accessor_declaration(
            node,
            result.modifiers,
            result.name,
            None,
            parameters,
            None,
            None,
            body,
        );
        self.finish_class_element(visitor, updated, node)
    }

    /// The `Body` of a static block: `Node.Body()` answers nil for one.
    fn static_block_body(visitor: &NodeVisitor<'_>, node: NodeId) -> NodeId {
        visitor
            .factory()
            .node(node)
            .as_class_static_block_declaration()
            .expect("ClassStaticBlockDeclaration payload")
            .body()
            .expect(NIL)
    }

    // port: tsc/internal/transformers/estransforms/esdecorator.go:esDecoratorTransformer.visitClassStaticBlockDeclaration
    fn visit_class_static_block_declaration(
        &self,
        visitor: &mut NodeVisitor<'_>,
        node: NodeId,
    ) -> Option<NodeId> {
        self.enter_class_element(visitor, node);
        let mut ec = self.ctx();

        let mut result;
        if is_class_named_evaluation_helper_block(&ec, visitor.factory(), node) {
            result = self.visit_each_child(visitor, Visit::Main, node);
            // Transfer AssignedName metadata to the new node so isClassNamedEvaluationHelperBlock
            // can still find it after visiting (visiting may create a new node when this->_classThis)
            if let Some(assigned_name) = ec.assigned_name(node) {
                if result != node {
                    ec.set_assigned_name(result, assigned_name);
                }
            }
        } else if is_class_this_assignment_block(&ec, visitor.factory(), node) {
            let saved_class_this = self.class_this.get();
            self.class_this.set(None);
            result = self.visit_each_child(visitor, Visit::Main, node);
            self.class_this.set(saved_class_this);
        } else {
            // Use a nested variable environment so temp vars generated during static block
            // content transformation (e.g., super access temps) stay scoped to the static block.
            ec.start_variable_environment();
            result = self.visit_each_child(visitor, Visit::Main, node);
            let var_statements = ec.end_variable_environment(visitor.factory_mut());
            if !var_statements.is_empty() {
                // Inject var declarations at the start of the static block's body
                let block_body = Self::static_block_body(visitor, result);
                let factory = visitor.factory_mut();
                let (statements, multi_line) = {
                    let read = factory.node(block_body);
                    let block = read.as_block().expect("Block payload");
                    (block.statements(), block.multi_line())
                };
                let mut new_stmts = var_statements;
                new_stmts.extend(list_nodes(factory, statements));
                let list = new_node_list(factory, new_stmts);
                let block = factory.new_block(Some(list), multi_line);
                result = factory.new_class_static_block_declaration(None, Some(block));
            }
            if let Some(ci) = self.class_info() {
                ci.borrow_mut().has_static_initializers = true;
                let pending = ci.borrow().pending_static_initializers.clone();
                if !pending.is_empty() {
                    // If we tried to inject the pending initializers into the current block, we might run into
                    // variable name collisions due to sharing this blocks scope. To avoid this, we inject a new
                    // static block that contains the pending initializers that precedes this block.
                    let factory = visitor.factory_mut();
                    let mut stmts = Vec::new();
                    for init in pending {
                        let init_stmt = factory.new_expression_statement(Some(init));
                        let range = ec.source_map_range(factory, init);
                        ec.set_source_map_range(init_stmt, range);
                        stmts.push(init_stmt);
                    }
                    let list = new_node_list(factory, stmts);
                    let body = factory.new_block(Some(list), true);
                    let static_block = factory.new_class_static_block_declaration(None, Some(body));
                    ci.borrow_mut().pending_static_initializers.clear();
                    // Return both the new static block and the original
                    self.exit_class_element();
                    return single_or_many(visitor.factory_mut(), Some(&[static_block, result]));
                }
            }
        }

        self.exit_class_element();
        Some(result)
    }

    // port: tsc/internal/transformers/estransforms/esdecorator.go:esDecoratorTransformer.visitPropertyDeclaration
    #[allow(clippy::too_many_lines)] // upstream's single function, kept in its order
    fn visit_property_declaration(
        &self,
        visitor: &mut NodeVisitor<'_>,
        mut node: NodeId,
    ) -> Option<NodeId> {
        if self.is_named_evaluation_and_anonymous_class(visitor, node) {
            let initializer = visitor.factory().node(node).initializer();
            node = self.transform_named_evaluation(visitor, node, initializer);
        }

        self.enter_class_element(visitor, node);

        // TODO(rbuckton): We support decorating `declare x` fields with legacyDecorators, but we currently don't
        //                 support them with esDecorators. We need to consider whether we will support them in the
        //                 future, and how. For now, these should be elided by the `ts` transform.
        debug_assert(
            !self.has_syntactic_modifier(visitor, node, modifier_flags::AMBIENT),
            || "Not yet implemented.".to_owned(),
        );

        // 10.2.1.3 RS: EvaluateBody
        //   Initializer : `=` AssignmentExpression
        //     ...
        //     3. If IsAnonymousFunctionDefinition(|AssignmentExpression|) is *true*, then
        //        a. Let _value_ be ? NamedEvaluation of |Initializer| with argument _functionObject_.[[ClassFieldInitializerName]].
        //     ...

        let mut ec = self.ctx();

        let create_descriptor = self
            .has_accessor_modifier(visitor, node)
            .then_some(Descriptor::AccessorProperty);
        let result = self.partial_transform_class_element(
            visitor,
            node,
            self.class_info(),
            create_descriptor,
        );

        ec.start_variable_environment();

        let node_initializer = visitor.factory().node(node).initializer();
        let mut initializer = self.visit_node(visitor, Visit::Main, node_initializer);
        if let Some(initializers_name) = result.initializers_name {
            let factory = visitor.factory_mut();
            let this_arg = match result.this_arg {
                Some(this_arg) => this_arg,
                None => ec.new_this_expression(factory),
            };
            let value = match initializer {
                Some(initializer) => initializer,
                None => ec.new_void_zero_expression(factory),
            };
            initializer = Some(ec.new_run_initializers_helper(
                factory,
                this_arg,
                initializers_name,
                Some(value),
            ));
        }

        let is_static = self.is_static(visitor, node);
        if is_static && initializer.is_some() {
            if let Some(ci) = self.class_info() {
                ci.borrow_mut().has_static_initializers = true;
            }
        }

        let declarations = ec.end_variable_environment(visitor.factory_mut());
        if !declarations.is_empty() {
            let factory = visitor.factory_mut();
            let mut stmts = declarations;
            stmts.push(factory.new_return_statement(initializer));
            initializer = Some(ec.new_immediately_invoked_arrow_function(factory, stmts));
        }

        if let Some(ci) = self.class_info() {
            if is_static {
                initializer = self.inject_pending_initializers(visitor, &ci, true, initializer);
                if let Some(extra_initializers_name) = result.extra_initializers_name {
                    let factory = visitor.factory_mut();
                    let class_this = ci.borrow().class_this;
                    let this_arg = match class_this {
                        Some(class_this) => class_this,
                        None => ec.new_this_expression(factory),
                    };
                    let run = ec.new_run_initializers_helper(
                        factory,
                        this_arg,
                        extra_initializers_name,
                        None,
                    );
                    ci.borrow_mut().pending_static_initializers.push(run);
                }
            } else {
                initializer = self.inject_pending_initializers(visitor, &ci, false, initializer);
                if let Some(extra_initializers_name) = result.extra_initializers_name {
                    let factory = visitor.factory_mut();
                    let this = ec.new_this_expression(factory);
                    let run = ec.new_run_initializers_helper(
                        factory,
                        this,
                        extra_initializers_name,
                        None,
                    );
                    ci.borrow_mut().pending_instance_initializers.push(run);
                }
            }
        }

        self.exit_class_element();

        if let Some(descriptor_name) = result
            .descriptor_name
            .filter(|_| self.has_accessor_modifier(visitor, node))
        {
            // given:
            //  accessor #x = 1;
            //
            // emits:
            //  static {
            //      _esDecorate(null, _private_x_descriptor = { get() { return this.#x_1; }, set(value) { this.#x_1 = value; } }, ...)
            //  }
            //  ...
            //  #x_1 = 1;
            //  get #x() { return _private_x_descriptor.get.call(this); }
            //  set #x(value) { _private_x_descriptor.set.call(this, value); }

            let comment_range = ec.comment_range_of(visitor.factory(), node);
            let source_map_range = ec.source_map_range(visitor.factory(), node);

            // Since we're creating two declarations where there was previously one, cache
            // the expression for any computed property names.
            let prop_name = Self::name_of(visitor, node).expect(NIL);
            let mut getter_name = result.name;
            let mut setter_name = result.name;
            if Self::kind(visitor, prop_name) == K::ComputedPropertyName {
                let prop_expression = visitor.factory().node(prop_name).expression().expect(NIL);
                if !is_simple_inlineable_expression(visitor.factory(), prop_expression) {
                    let cache_assignment = find_computed_property_name_cache_assignment(
                        &self.emit_context,
                        visitor.factory(),
                        prop_name,
                    );
                    if let Some(cache_assignment) = cache_assignment {
                        let expression =
                            self.visit_node(visitor, Visit::Main, Some(prop_expression));
                        let factory = visitor.factory_mut();
                        getter_name =
                            Some(factory.update_computed_property_name(prop_name, expression));
                        let left = factory
                            .node(cache_assignment)
                            .as_binary_expression()
                            .expect("BinaryExpression payload")
                            .left();
                        setter_name = Some(factory.update_computed_property_name(prop_name, left));
                    } else {
                        let factory = visitor.factory_mut();
                        let temp = ec.new_temp_variable(factory);
                        let expression_range = factory.node(prop_expression).range();
                        ec.set_source_map_range(temp, expression_range);
                        ec.add_variable_declaration(factory, temp);
                        let expression = self
                            .visit_node(visitor, Visit::Main, Some(prop_expression))
                            .expect(NIL);
                        let factory = visitor.factory_mut();
                        let assignment = ec.new_assignment_expression(factory, temp, expression);
                        ec.set_source_map_range(assignment, expression_range);
                        getter_name = Some(
                            factory.update_computed_property_name(prop_name, Some(assignment)),
                        );
                        setter_name =
                            Some(factory.update_computed_property_name(prop_name, Some(temp)));
                    }
                }
            }

            let modifiers_without_accessor =
                self.visit_modifiers(visitor, Visit::AccessorStrippingModifier, result.modifiers);

            let backing_field = create_accessor_property_backing_field(
                &ec,
                visitor.factory_mut(),
                node,
                modifiers_without_accessor,
                initializer,
            );
            ec.set_original(backing_field, node);
            ec.set_emit_flags(backing_field, emit_flags::NO_COMMENTS);
            ec.set_source_map_range(backing_field, source_map_range);
            let backing_name = Self::name_of(visitor, backing_field).expect(NIL);
            let name_range = ec.source_map_range(visitor.factory(), prop_name);
            ec.set_source_map_range(backing_name, name_range);

            let getter = self.create_get_accessor_descriptor_forwarder(
                visitor,
                modifiers_without_accessor,
                getter_name,
                descriptor_name,
            );
            ec.set_original(getter, node);
            ec.set_comment_range(getter, comment_range);
            ec.set_source_map_range(getter, source_map_range);

            let setter = self.create_set_accessor_descriptor_forwarder(
                visitor,
                modifiers_without_accessor,
                setter_name,
                descriptor_name,
            );
            ec.set_original(setter, node);
            ec.set_emit_flags(setter, emit_flags::NO_COMMENTS);
            ec.set_source_map_range(setter, source_map_range);

            return single_or_many(
                visitor.factory_mut(),
                Some(&[backing_field, getter, setter]),
            );
        }

        let updated = visitor.factory_mut().update_property_declaration(
            node,
            result.modifiers,
            result.name,
            None,
            None,
            initializer,
        );
        Some(self.finish_class_element(visitor, updated, node))
    }

    // port: tsc/internal/transformers/estransforms/esdecorator.go:esDecoratorTransformer.visitThisExpression
    fn visit_this_expression(&self, node: NodeId) -> NodeId {
        self.class_this.get().unwrap_or(node)
    }

    // port: tsc/internal/transformers/estransforms/esdecorator.go:esDecoratorTransformer.visitCallExpression
    fn visit_call_expression(&self, visitor: &mut NodeVisitor<'_>, node: NodeId) -> NodeId {
        let (expression, arguments, loc) = {
            let read = visitor.factory().node(node);
            (
                read.expression().expect(NIL),
                read.argument_list(),
                read.range(),
            )
        };
        if let Some(class_this) = self
            .class_this
            .get()
            .filter(|_| is_super_property(visitor.factory(), expression))
        {
            let expression = self
                .visit_node(visitor, Visit::Main, Some(expression))
                .expect(NIL);
            let arguments_list = self.visit_nodes(visitor, Visit::Main, arguments);
            let factory = visitor.factory_mut();
            let arguments = list_nodes(factory, arguments_list);
            let mut ec = self.ctx();
            let invocation =
                ec.new_function_call_call(factory, expression, Some(class_this), arguments);
            ec.set_original(invocation, node);
            factory.set_node_range(invocation, loc);
            return invocation;
        }
        self.visit_each_child(visitor, Visit::Main, node)
    }

    // port: tsc/internal/transformers/estransforms/esdecorator.go:esDecoratorTransformer.visitTaggedTemplateExpression
    fn visit_tagged_template_expression(
        &self,
        visitor: &mut NodeVisitor<'_>,
        node: NodeId,
    ) -> NodeId {
        let (tag, template, flags, loc) = {
            let read = visitor.factory().node(node);
            let data = read
                .as_tagged_template_expression()
                .expect("TaggedTemplateExpression payload");
            (
                data.tag().expect(NIL),
                data.template(),
                read.flags(),
                read.range(),
            )
        };
        if let Some(class_this) = self
            .class_this
            .get()
            .filter(|_| is_super_property(visitor.factory(), tag))
        {
            let tag = self.visit_node(visitor, Visit::Main, Some(tag)).expect(NIL);
            let mut ec = self.ctx();
            let bound_tag =
                ec.new_function_bind_call(visitor.factory_mut(), tag, class_this, Vec::new());
            ec.set_original(bound_tag, node);
            visitor.factory_mut().set_node_range(bound_tag, loc);
            let template = self.visit_node(visitor, Visit::Main, template);
            return visitor.factory_mut().update_tagged_template_expression(
                node,
                Some(bound_tag),
                None,
                None,
                template,
                flags,
            );
        }
        self.visit_each_child(visitor, Visit::Main, node)
    }

    // port: tsc/internal/transformers/estransforms/esdecorator.go:esDecoratorTransformer.visitPropertyAccessExpression
    fn visit_property_access_expression(
        &self,
        visitor: &mut NodeVisitor<'_>,
        node: NodeId,
    ) -> NodeId {
        let (expression, name) = {
            let read = visitor.factory().node(node);
            (read.expression().expect(NIL), read.name().expect(NIL))
        };
        if let (true, Some(class_this), Some(class_super)) = (
            is_super_property(visitor.factory(), node)
                && Self::kind(visitor, name) == K::Identifier,
            self.class_this.get(),
            self.class_super.get(),
        ) {
            let mut ec = self.ctx();
            let factory = visitor.factory_mut();
            let property_name = ec.new_string_literal_from_node(factory, name);
            let super_property =
                ec.new_reflect_get_call(factory, class_super, property_name, class_this);
            ec.set_original(super_property, expression);
            let loc = factory.node(expression).range();
            factory.set_node_range(super_property, loc);
            return super_property;
        }
        self.visit_each_child(visitor, Visit::Main, node)
    }

    // port: tsc/internal/transformers/estransforms/esdecorator.go:esDecoratorTransformer.visitElementAccessExpression
    fn visit_element_access_expression(
        &self,
        visitor: &mut NodeVisitor<'_>,
        node: NodeId,
    ) -> NodeId {
        let (expression, argument_expression) = {
            let read = visitor.factory().node(node);
            let data = read
                .as_element_access_expression()
                .expect("ElementAccessExpression payload");
            (data.expression().expect(NIL), data.argument_expression())
        };
        if let (true, Some(class_this), Some(class_super)) = (
            is_super_property(visitor.factory(), node),
            self.class_this.get(),
            self.class_super.get(),
        ) {
            let property_name = self
                .visit_node(visitor, Visit::Main, argument_expression)
                .expect(NIL);
            let mut ec = self.ctx();
            let factory = visitor.factory_mut();
            let super_property =
                ec.new_reflect_get_call(factory, class_super, property_name, class_this);
            ec.set_original(super_property, expression);
            let loc = factory.node(expression).range();
            factory.set_node_range(super_property, loc);
            return super_property;
        }
        self.visit_each_child(visitor, Visit::Main, node)
    }

    /// 8.6.3 RS: `IteratorBindingInitialization` and 14.3.3.3 RS:
    /// `KeyedBindingInitialization` of a `SingleNameBinding` with an anonymous
    /// function definition initializer.
    // port: tsc/internal/transformers/estransforms/esdecorator.go:esDecoratorTransformer.visitParameterDeclaration
    fn visit_parameter_declaration(&self, visitor: &mut NodeVisitor<'_>, node: NodeId) -> NodeId {
        let mut param_node = node;
        if self.is_named_evaluation_and_anonymous_class(visitor, param_node) {
            let initializer = visitor.factory().node(param_node).initializer();
            param_node = self.transform_named_evaluation(visitor, param_node, initializer);
        }

        let (dot_dot_dot_token, name, initializer) = {
            let read = visitor.factory().node(param_node);
            (
                read.as_parameter_declaration()
                    .expect("ParameterDeclaration payload")
                    .dot_dot_dot_token(),
                read.name(),
                read.initializer(),
            )
        };
        let name = self.visit_node(visitor, Visit::Main, name);
        let initializer = self.visit_node(visitor, Visit::Main, initializer);
        let factory = visitor.factory_mut();
        let updated = factory.update_parameter_declaration(
            param_node,
            None, // modifiers - strip all modifiers (including decorators)
            dot_dot_dot_token,
            name,
            None, // questionToken
            None, // type
            initializer,
        );
        if updated != param_node {
            // While we emit the source map for the node after skipping decorators and modifiers,
            // we need to emit the comments for the original range.
            let mut ec = self.ctx();
            let loc = factory.node(param_node).range();
            ec.set_comment_range(updated, loc);
            let new_loc = move_range_past_modifiers(factory, param_node);
            factory.set_node_range(updated, new_loc);
            ec.set_source_map_range(updated, new_loc);
            let updated_name = factory.node(updated).name().expect(NIL);
            ec.set_emit_flags(updated_name, emit_flags::NO_TRAILING_SOURCE_MAP);
        }
        updated
    }

    /// Replaces Strada's `visitPropertyAssignment`, `visitVariableDeclaration`
    /// and `visitBindingElement`, which all share the same logic: the
    /// `NamedEvaluation` of an anonymous function definition.
    // port: tsc/internal/transformers/estransforms/esdecorator.go:esDecoratorTransformer.visitNamedEvaluationSite
    fn visit_named_evaluation_site(
        &self,
        visitor: &mut NodeVisitor<'_>,
        mut node: NodeId,
        class_expr: Option<NodeId>,
    ) -> NodeId {
        if self.is_named_evaluation_and_anonymous_class(visitor, node) {
            node = self.transform_named_evaluation(visitor, node, class_expr);
        }
        self.visit_each_child(visitor, Visit::Main, node)
    }

    // port: tsc/internal/transformers/estransforms/esdecorator.go:isAnonymousClassNeedingAssignedName
    fn is_anonymous_class_needing_assigned_name(
        &self,
        factory: &dyn RuntimeFactory,
        node: NodeId,
    ) -> bool {
        let (kind, unnamed) = {
            let read = factory.node(node);
            (read.kind(), read.name().is_none())
        };
        kind == K::ClassExpression && unnamed && self.is_decorated_class_like(factory, node)
    }

    /// The IIFE produced for `(@dec class {})` will result in an assigned name of the form
    /// `var class_1 = class { };`, and thus the empty string cannot be ignored. However, The IIFE
    /// produced for `(class { @dec x; })` will not result in an assigned name since it
    /// transforms to `return class { };`, and thus the empty string *can* be ignored.
    // port: tsc/internal/transformers/estransforms/esdecorator.go:canIgnoreEmptyStringLiteralInAssignedName
    fn can_ignore_empty_string_literal_in_assigned_name(
        &self,
        visitor: &NodeVisitor<'_>,
        node: Option<NodeId>,
    ) -> bool {
        let Some(node) = node else {
            return false;
        };
        let factory = visitor.factory();
        let inner_expression = skip_outer_expressions_all(factory, node);
        let (kind, unnamed) = {
            let read = factory.node(inner_expression);
            (read.kind(), read.name().is_none())
        };
        kind == K::ClassExpression
            && unnamed
            && !self.q(factory, |view| {
                tsr_ast::utilities_class::class_or_constructor_parameter_is_decorated(
                    view,
                    false,
                    inner_expression,
                )
            })
    }

    // port: tsc/internal/transformers/estransforms/esdecorator.go:esDecoratorTransformer.visitForStatement
    fn visit_for_statement(&self, visitor: &mut NodeVisitor<'_>, node: NodeId) -> NodeId {
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
        let initializer = self.visit_node(visitor, Visit::Discarded, initializer);
        let condition = self.visit_node(visitor, Visit::Main, condition);
        let incrementor = self.visit_node(visitor, Visit::Discarded, incrementor);
        let statement = self.with(visitor, Visit::Main, |v| {
            self.ctx().visit_iteration_body(statement, v)
        });
        visitor.factory_mut().update_for_statement(
            node,
            initializer,
            condition,
            incrementor,
            statement,
        )
    }

    // port: tsc/internal/transformers/estransforms/esdecorator.go:esDecoratorTransformer.visitExpressionStatement
    fn visit_expression_statement(&self, visitor: &mut NodeVisitor<'_>, node: NodeId) -> NodeId {
        self.visit_each_child(visitor, Visit::Discarded, node)
    }

    /// The setter name of a `super` property: the visited argument of an
    /// element access, or a string literal of an identifier property name.
    fn super_property_name(&self, visitor: &mut NodeVisitor<'_>, node: NodeId) -> Option<NodeId> {
        let kind = Self::kind(visitor, node);
        if kind == K::ElementAccessExpression {
            let argument = visitor
                .factory()
                .node(node)
                .as_element_access_expression()
                .expect("ElementAccessExpression payload")
                .argument_expression();
            return self.visit_node(visitor, Visit::Main, argument);
        }
        if kind == K::PropertyAccessExpression {
            let name = Self::name_of(visitor, node).expect(NIL);
            if Self::kind(visitor, name) == K::Identifier {
                return Some(
                    self.ctx()
                        .new_string_literal_from_node(visitor.factory_mut(), name),
                );
            }
        }
        None
    }

    // port: tsc/internal/transformers/estransforms/esdecorator.go:esDecoratorTransformer.visitBinaryExpression
    #[allow(clippy::too_many_lines)] // upstream's single function, kept in its order
    fn visit_binary_expression(
        &self,
        visitor: &mut NodeVisitor<'_>,
        node: NodeId,
        discarded: bool,
    ) -> NodeId {
        let mut ec = self.ctx();
        let (left, operator_token, right, loc) = {
            let read = visitor.factory().node(node);
            let data = read
                .as_binary_expression()
                .expect("BinaryExpression payload");
            (
                data.left().expect(NIL),
                data.operator_token().expect(NIL),
                data.right(),
                read.range(),
            )
        };
        let operator = Self::kind(visitor, operator_token);

        if self.q(visitor.factory(), |view| {
            tsr_ast::is_destructuring_assignment(view, node)
        }) {
            let left = self.visit_assignment_pattern(visitor, left);
            let right = self.visit_node(visitor, Visit::Main, right);
            return visitor.factory_mut().update_binary_expression(
                node,
                None,
                Some(left),
                None,
                Some(operator_token),
                right,
            );
        }

        if self.is_assignment_expression(visitor, node, false) {
            // 13.15.2 RS: Evaluation
            //   AssignmentExpression : LeftHandSideExpression `=` AssignmentExpression
            //     1. If |LeftHandSideExpression| is neither an |ObjectLiteral| nor an |ArrayLiteral|, then
            //        a. Let _lref_ be ? Evaluation of |LeftHandSideExpression|.
            //        b. If IsAnonymousFunctionDefinition(|AssignmentExpression|) and IsIdentifierRef of |LeftHandSideExpression| are both *true*, then
            //           i. Let _rval_ be ? NamedEvaluation of |AssignmentExpression| with argument _lref_.[[ReferencedName]].
            //     ...
            //
            //   AssignmentExpression : LeftHandSideExpression `&&=`, `||=` or `??=` AssignmentExpression
            //     ...
            //     If IsAnonymousFunctionDefinition(|AssignmentExpression|) is *true* and IsIdentifierRef of |LeftHandSideExpression| is *true*, then
            //        a. Let _rval_ be ? NamedEvaluation of |AssignmentExpression| with argument _lref_.[[ReferencedName]].
            //     ...

            if self.is_named_evaluation_and_anonymous_class(visitor, node) {
                let node = self.transform_named_evaluation(visitor, node, right);
                return self.visit_each_child(visitor, Visit::Main, node);
            }

            if let (true, Some(class_this), Some(class_super)) = (
                is_super_property(visitor.factory(), left),
                self.class_this.get(),
                self.class_super.get(),
            ) {
                if let Some(mut setter_name) = self.super_property_name(visitor, left) {
                    // super.x = ...
                    // super.x += ...
                    // super[x] = ...
                    // super[x] += ...
                    let mut expression = self.visit_node(visitor, Visit::Main, right).expect(NIL);
                    let factory = visitor.factory_mut();
                    if tsr_ast::utilities::is_compound_assignment(operator.into()) {
                        let mut getter_name = setter_name;
                        if !is_simple_inlineable_expression(factory, setter_name) {
                            getter_name = ec.new_temp_variable(factory);
                            ec.add_variable_declaration(factory, getter_name);
                            setter_name =
                                ec.new_assignment_expression(factory, getter_name, setter_name);
                        }
                        let super_property_get =
                            ec.new_reflect_get_call(factory, class_super, getter_name, class_this);
                        ec.set_original(super_property_get, left);
                        let left_loc = factory.node(left).range();
                        factory.set_node_range(super_property_get, left_loc);
                        let operator_token = factory.new_token(
                            get_non_assignment_operator_for_compound_assignment(operator.into()),
                        );
                        expression = factory.new_binary_expression(
                            None,
                            Some(super_property_get),
                            None,
                            Some(operator_token),
                            Some(expression),
                        );
                        factory.set_node_range(expression, loc);
                    }
                    let mut temp = None;
                    if !discarded {
                        let t = ec.new_temp_variable(factory);
                        ec.add_variable_declaration(factory, t);
                        temp = Some(t);
                    }
                    if let Some(temp) = temp {
                        expression = ec.new_assignment_expression(factory, temp, expression);
                        factory.set_node_range(expression, loc);
                    }
                    expression = ec.new_reflect_set_call(
                        factory,
                        class_super,
                        setter_name,
                        expression,
                        class_this,
                    );
                    ec.set_original(expression, node);
                    factory.set_node_range(expression, loc);
                    if let Some(temp) = temp {
                        expression = ec.new_comma_expression(factory, expression, temp);
                        factory.set_node_range(expression, loc);
                    }
                    return expression;
                }
            }
        }

        if operator == K::CommaToken {
            let left = self.visit_node(visitor, Visit::Discarded, Some(left));
            let right = if discarded {
                self.visit_node(visitor, Visit::Discarded, right)
            } else {
                self.visit_node(visitor, Visit::Main, right)
            };
            return visitor.factory_mut().update_binary_expression(
                node,
                None,
                left,
                None,
                Some(operator_token),
                right,
            );
        }

        self.visit_each_child(visitor, Visit::Main, node)
    }

    // port: tsc/internal/transformers/estransforms/esdecorator.go:esDecoratorTransformer.visitPreOrPostfixUnaryExpression
    fn visit_pre_or_postfix_unary_expression(
        &self,
        visitor: &mut NodeVisitor<'_>,
        node: NodeId,
        discarded: bool,
    ) -> NodeId {
        let mut ec = self.ctx();

        let (operator, operand_node, loc) = {
            let read = visitor.factory().node(node);
            let loc = read.range();
            if let Some(data) = read.as_prefix_unary_expression() {
                (data.operator(), data.operand(), loc)
            } else {
                let data = read
                    .as_postfix_unary_expression()
                    .expect("PostfixUnaryExpression payload");
                (data.operator(), data.operand(), loc)
            }
        };
        let operator = operator.known().unwrap_or(K::Unknown);

        if operator == K::PlusPlusToken || operator == K::MinusMinusToken {
            let operand = match super::utilities::skip_parentheses(
                visitor.factory(),
                operand_node.expect(NIL),
            ) {
                Ok(operand) => operand,
                Err(error) => {
                    self.failure.record(error);
                    return node;
                }
            };
            if let (true, Some(class_this), Some(class_super)) = (
                is_super_property(visitor.factory(), operand),
                self.class_this.get(),
                self.class_super.get(),
            ) {
                if let Some(mut setter_name) = self.super_property_name(visitor, operand) {
                    let factory = visitor.factory_mut();
                    let mut getter_name = setter_name;
                    if !is_simple_inlineable_expression(factory, setter_name) {
                        getter_name = ec.new_temp_variable(factory);
                        ec.add_variable_declaration(factory, getter_name);
                        setter_name =
                            ec.new_assignment_expression(factory, getter_name, setter_name);
                    }

                    let mut expression =
                        ec.new_reflect_get_call(factory, class_super, getter_name, class_this);
                    ec.set_original(expression, node);
                    factory.set_node_range(expression, loc);

                    // If the result of this expression is discarded (i.e., it's in a position where the result
                    // will be otherwise unused, such as in an expression statement or the left side of a comma), we
                    // don't need to create an extra temp variable to hold the result:
                    //
                    //  source (discarded):
                    //    super.x++;
                    //  generated:
                    //    _a = Reflect.get(_super, "x"), _a++, Reflect.set(_super, "x", _a);
                    //
                    // Above, the temp variable `_a` is used to perform the correct coercion (i.e., number or
                    // bigint). Since the result of the postfix unary is discarded, we don't need to capture the
                    // result of the expression.
                    //
                    //  source (not discarded):
                    //    y = super.x++;
                    //  generated:
                    //    y = (_a = Reflect.get(_super, "x"), _b = _a++, Reflect.set(_super, "x", _a), _b);
                    //
                    // When the result isn't discarded, we introduce a new temp variable (`_b`) to capture the
                    // result of the operation so that we can provide it to `y` when the assignment is complete.
                    let mut temp = None;
                    if !discarded {
                        let t = ec.new_temp_variable(factory);
                        ec.add_variable_declaration(factory, t);
                        temp = Some(t);
                    }

                    expression = expand_pre_or_postfix_increment_or_decrement_expression(
                        factory, &ec, node, expression, temp,
                    );

                    expression = ec.new_reflect_set_call(
                        factory,
                        class_super,
                        setter_name,
                        expression,
                        class_this,
                    );
                    ec.set_original(expression, node);
                    factory.set_node_range(expression, loc);

                    if let Some(temp) = temp {
                        expression = ec.new_comma_expression(factory, expression, temp);
                        factory.set_node_range(expression, loc);
                    }

                    return expression;
                }
            }
        }

        self.visit_each_child(visitor, Visit::Main, node)
    }

    // port: tsc/internal/transformers/estransforms/esdecorator.go:esDecoratorTransformer.visitReferencedPropertyName
    fn visit_referenced_property_name(
        &self,
        visitor: &mut NodeVisitor<'_>,
        node: NodeId,
    ) -> (NodeId, NodeId) {
        let mut ec = self.ctx();
        let kind = Self::kind(visitor, node);
        if tsr_ast::utilities::is_property_name_literal_kind(kind.into())
            || kind == K::PrivateIdentifier
        {
            let referenced = ec.new_string_literal_from_node(visitor.factory_mut(), node);
            let visited = self
                .visit_node(visitor, Visit::Main, Some(node))
                .expect(NIL);
            return (referenced, visited);
        }

        let expression = visitor.factory().node(node).expression().expect(NIL);
        let expression_kind = Self::kind(visitor, expression);
        if tsr_ast::utilities::is_property_name_literal_kind(expression_kind.into())
            && expression_kind != K::Identifier
        {
            let referenced = ec.new_string_literal_from_node(visitor.factory_mut(), expression);
            let visited = self
                .visit_node(visitor, Visit::Main, Some(node))
                .expect(NIL);
            return (referenced, visited);
        }

        let referenced_name = ec.new_generated_name_for_node(visitor.factory_mut(), node);
        ec.add_variable_declaration(visitor.factory_mut(), referenced_name);

        let visited = self
            .visit_node(visitor, Visit::Main, Some(expression))
            .expect(NIL);
        let factory = visitor.factory_mut();
        let key = ec.new_prop_key_helper(factory, visited);
        let assignment = ec.new_assignment_expression(factory, referenced_name, key);
        let injected = self.inject_pending_expressions(visitor, assignment);
        let updated_name = visitor
            .factory_mut()
            .update_computed_property_name(node, Some(injected));
        (referenced_name, updated_name)
    }

    // port: tsc/internal/transformers/estransforms/esdecorator.go:esDecoratorTransformer.visitPropertyName
    fn visit_property_name(
        &self,
        visitor: &mut NodeVisitor<'_>,
        node: Option<NodeId>,
    ) -> Option<NodeId> {
        let id = node.expect(NIL);
        if Self::kind(visitor, id) == K::ComputedPropertyName {
            return Some(self.visit_computed_property_name(visitor, id));
        }
        self.visit_node(visitor, Visit::Main, node)
    }

    // port: tsc/internal/transformers/estransforms/esdecorator.go:esDecoratorTransformer.visitComputedPropertyName
    fn visit_computed_property_name(&self, visitor: &mut NodeVisitor<'_>, node: NodeId) -> NodeId {
        let expression = visitor.factory().node(node).expression();
        let mut expression = self
            .visit_node(visitor, Visit::Main, expression)
            .expect(NIL);
        if !is_simple_inlineable_expression(visitor.factory(), expression) {
            expression = self.inject_pending_expressions(visitor, expression);
        }
        visitor
            .factory_mut()
            .update_computed_property_name(node, Some(expression))
    }

    // port: tsc/internal/transformers/estransforms/esdecorator.go:esDecoratorTransformer.visitDestructuringAssignmentTarget
    fn visit_destructuring_assignment_target(
        &self,
        visitor: &mut NodeVisitor<'_>,
        node: NodeId,
    ) -> NodeId {
        let kind = Self::kind(visitor, node);
        if kind == K::ObjectLiteralExpression || kind == K::ArrayLiteralExpression {
            return self.visit_assignment_pattern(visitor, node);
        }

        if let (true, Some(class_this), Some(class_super)) = (
            is_super_property(visitor.factory(), node),
            self.class_this.get(),
            self.class_super.get(),
        ) {
            if let Some(property_name) = self.super_property_name(visitor, node) {
                let mut ec = self.ctx();
                let factory = visitor.factory_mut();
                let param_name = ec.new_temp_variable(factory);
                let set = ec.new_reflect_set_call(
                    factory,
                    class_super,
                    property_name,
                    param_name,
                    class_this,
                );
                let expression = ec.new_assignment_target_wrapper(factory, param_name, set);
                ec.set_original(expression, node);
                let loc = factory.node(node).range();
                factory.set_node_range(expression, loc);
                return expression;
            }
        }

        self.visit_each_child(visitor, Visit::Main, node)
    }

    /// 13.15.5.5 RS: `IteratorDestructuringAssignmentEvaluation` of an
    /// `AssignmentElement` with an anonymous function definition initializer.
    // port: tsc/internal/transformers/estransforms/esdecorator.go:esDecoratorTransformer.visitAssignmentElement
    fn visit_assignment_element(&self, visitor: &mut NodeVisitor<'_>, mut node: NodeId) -> NodeId {
        if self.is_assignment_expression(visitor, node, true /*excludeCompoundAssignment*/) {
            if self.is_named_evaluation_and_anonymous_class(visitor, node) {
                let right = visitor
                    .factory()
                    .node(node)
                    .as_binary_expression()
                    .expect("BinaryExpression payload")
                    .right();
                node = self.transform_named_evaluation(visitor, node, right);
            }
            let (left, operator_token, right) = {
                let read = visitor.factory().node(node);
                let data = read
                    .as_binary_expression()
                    .expect("BinaryExpression payload");
                (data.left().expect(NIL), data.operator_token(), data.right())
            };
            let assignment_target = self.visit_destructuring_assignment_target(visitor, left);
            let initializer = self.visit_node(visitor, Visit::Main, right);
            return visitor.factory_mut().update_binary_expression(
                node,
                None,
                Some(assignment_target),
                None,
                operator_token,
                initializer,
            );
        }
        self.visit_destructuring_assignment_target(visitor, node)
    }

    // port: tsc/internal/transformers/estransforms/esdecorator.go:esDecoratorTransformer.visitAssignmentRestElement
    fn visit_assignment_rest_element(&self, visitor: &mut NodeVisitor<'_>, node: NodeId) -> NodeId {
        let expression = visitor.factory().node(node).expression().expect(NIL);
        if self.is_left_hand_side_expression(visitor, expression) {
            let expression = self.visit_destructuring_assignment_target(visitor, expression);
            return visitor
                .factory_mut()
                .update_spread_element(node, Some(expression));
        }
        self.visit_each_child(visitor, Visit::Main, node)
    }

    // port: tsc/internal/transformers/estransforms/esdecorator.go:esDecoratorTransformer.visitArrayAssignmentElement
    fn visit_array_assignment_element(
        &self,
        visitor: &mut NodeVisitor<'_>,
        node: NodeId,
    ) -> NodeId {
        debug_assert(
            self.q(visitor.factory(), |view| {
                tsr_ast::utilities_positions::is_array_binding_or_assignment_element(view, node)
            }),
            String::new,
        );
        let kind = Self::kind(visitor, node);
        if kind == K::SpreadElement {
            return self.visit_assignment_rest_element(visitor, node);
        }
        if kind != K::OmittedExpression {
            return self.visit_assignment_element(visitor, node);
        }
        self.visit_each_child(visitor, Visit::Main, node)
    }

    /// `AssignmentProperty : PropertyName : AssignmentElement`
    /// (13.15.5.6 RS: `KeyedDestructuringAssignmentEvaluation`).
    // port: tsc/internal/transformers/estransforms/esdecorator.go:esDecoratorTransformer.visitAssignmentPropertyNode
    fn visit_assignment_property_node(
        &self,
        visitor: &mut NodeVisitor<'_>,
        node: NodeId,
    ) -> NodeId {
        let (name, initializer) = {
            let read = visitor.factory().node(node);
            (read.name(), read.initializer().expect(NIL))
        };
        let name = self.visit_node(visitor, Visit::Main, name);
        if self.is_assignment_expression(
            visitor,
            initializer,
            true, /*excludeCompoundAssignment*/
        ) {
            let assignment_element = self.visit_assignment_element(visitor, initializer);
            return visitor.factory_mut().update_property_assignment(
                node,
                None,
                name,
                None,
                None,
                Some(assignment_element),
            );
        }
        if self.is_left_hand_side_expression(visitor, initializer) {
            let assignment_element =
                self.visit_destructuring_assignment_target(visitor, initializer);
            return visitor.factory_mut().update_property_assignment(
                node,
                None,
                name,
                None,
                None,
                Some(assignment_element),
            );
        }
        self.visit_each_child(visitor, Visit::Main, node)
    }

    /// `AssignmentProperty : IdentifierReference Initializer?`
    /// (13.15.5.3 RS: `PropertyDestructuringAssignmentEvaluation`).
    // port: tsc/internal/transformers/estransforms/esdecorator.go:esDecoratorTransformer.visitShorthandAssignmentProperty
    fn visit_shorthand_assignment_property(
        &self,
        visitor: &mut NodeVisitor<'_>,
        mut node: NodeId,
    ) -> NodeId {
        if self.is_named_evaluation_and_anonymous_class(visitor, node) {
            let initializer = visitor
                .factory()
                .node(node)
                .as_shorthand_property_assignment()
                .expect("ShorthandPropertyAssignment payload")
                .object_assignment_initializer();
            node = self.transform_named_evaluation(visitor, node, initializer);
        }
        self.visit_each_child(visitor, Visit::Main, node)
    }

    // port: tsc/internal/transformers/estransforms/esdecorator.go:esDecoratorTransformer.visitAssignmentRestProperty
    fn visit_assignment_rest_property(
        &self,
        visitor: &mut NodeVisitor<'_>,
        node: NodeId,
    ) -> NodeId {
        let expression = visitor.factory().node(node).expression().expect(NIL);
        if self.is_left_hand_side_expression(visitor, expression) {
            let expression = self.visit_destructuring_assignment_target(visitor, expression);
            return visitor
                .factory_mut()
                .update_spread_assignment(node, Some(expression));
        }
        self.visit_each_child(visitor, Visit::Main, node)
    }

    // port: tsc/internal/transformers/estransforms/esdecorator.go:esDecoratorTransformer.visitObjectAssignmentElement
    fn visit_object_assignment_element(
        &self,
        visitor: &mut NodeVisitor<'_>,
        node: NodeId,
    ) -> NodeId {
        debug_assert(
            tsr_ast::utilities::is_object_binding_or_assignment_element(
                &visitor.factory().node(node),
            ),
            String::new,
        );
        match Self::kind(visitor, node) {
            K::SpreadAssignment => self.visit_assignment_rest_property(visitor, node),
            K::ShorthandPropertyAssignment => {
                self.visit_shorthand_assignment_property(visitor, node)
            }
            K::PropertyAssignment => self.visit_assignment_property_node(visitor, node),
            _ => self.visit_each_child(visitor, Visit::Main, node),
        }
    }

    // port: tsc/internal/transformers/estransforms/esdecorator.go:esDecoratorTransformer.visitAssignmentPattern
    fn visit_assignment_pattern(&self, visitor: &mut NodeVisitor<'_>, node: NodeId) -> NodeId {
        let array = {
            let read = visitor.factory().node(node);
            read.as_array_literal_expression()
                .map(|data| (data.elements(), data.multi_line()))
        };
        if let Some((elements, multi_line)) = array {
            let elements = self.visit_nodes(visitor, Visit::ArrayAssignment, elements);
            return visitor
                .factory_mut()
                .update_array_literal_expression(node, elements, multi_line);
        }
        let (properties, multi_line) = {
            let read = visitor.factory().node(node);
            let data = read
                .as_object_literal_expression()
                .expect("ObjectLiteralExpression payload");
            (data.properties(), data.multi_line())
        };
        let properties = self.visit_nodes(visitor, Visit::ObjectAssignment, properties);
        visitor
            .factory_mut()
            .update_object_literal_expression(node, properties, multi_line)
    }

    /// 16.2.3.7 RS: Evaluation of `export default AssignmentExpression;`.
    // port: tsc/internal/transformers/estransforms/esdecorator.go:esDecoratorTransformer.visitExportAssignment
    fn visit_export_assignment(&self, visitor: &mut NodeVisitor<'_>, node: NodeId) -> NodeId {
        let expression = visitor.factory().node(node).expression();
        self.visit_named_evaluation_site(visitor, node, expression)
    }

    /// 8.4.5 RS: `NamedEvaluation` of `( Expression )`.
    // port: tsc/internal/transformers/estransforms/esdecorator.go:esDecoratorTransformer.visitParenthesizedExpression
    fn visit_parenthesized_expression(
        &self,
        visitor: &mut NodeVisitor<'_>,
        node: NodeId,
        discarded: bool,
    ) -> NodeId {
        let expression = visitor.factory().node(node).expression();
        let which = if discarded {
            Visit::Discarded
        } else {
            Visit::Main
        };
        let expression = self.visit_node(visitor, which, expression);
        visitor
            .factory_mut()
            .update_parenthesized_expression(node, expression)
    }

    /// Emulates 8.4.5 RS: `NamedEvaluation`.
    // port: tsc/internal/transformers/estransforms/esdecorator.go:esDecoratorTransformer.visitPartiallyEmittedExpression
    fn visit_partially_emitted_expression(
        &self,
        visitor: &mut NodeVisitor<'_>,
        node: NodeId,
        discarded: bool,
    ) -> NodeId {
        let expression = visitor.factory().node(node).expression();
        let which = if discarded {
            Visit::Discarded
        } else {
            Visit::Main
        };
        let expression = self.visit_node(visitor, which, expression);
        visitor
            .factory_mut()
            .update_partially_emitted_expression(node, expression)
    }

    /// Prepends `pending` before `expression`, preserving parenthesization.
    /// A nil `expression` inlines the pending expressions alone.
    // port: tsc/internal/transformers/estransforms/esdecorator.go:esDecoratorTransformer.prependExpressions
    fn prepend_expressions(
        &self,
        factory: &mut dyn RuntimeFactory,
        pending: &[NodeId],
        expression: Option<NodeId>,
    ) -> Option<NodeId> {
        let ec = &self.emit_context;
        if pending.is_empty() {
            return expression;
        }
        let Some(expression) = expression else {
            return ec.inline_expressions(factory, pending);
        };
        if factory.node(expression).kind() == K::ParenthesizedExpression {
            let inner = factory.node(expression).expression().expect(NIL);
            let mut exprs = pending.to_vec();
            exprs.push(inner);
            let inlined = ec.inline_expressions(factory, &exprs);
            return Some(factory.update_parenthesized_expression(expression, inlined));
        }
        let mut exprs = pending.to_vec();
        exprs.push(expression);
        ec.inline_expressions(factory, &exprs)
    }

    // port: tsc/internal/transformers/estransforms/esdecorator.go:esDecoratorTransformer.injectPendingExpressions
    fn inject_pending_expressions(
        &self,
        visitor: &mut NodeVisitor<'_>,
        expression: NodeId,
    ) -> NodeId {
        let pending = self.pending_expressions.borrow().clone();
        let result = self.prepend_expressions(visitor.factory_mut(), &pending, Some(expression));
        debug_assert(result.is_some(), String::new);
        if result != Some(expression) {
            self.pending_expressions.borrow_mut().clear();
        }
        result.expect(NIL)
    }

    // port: tsc/internal/transformers/estransforms/esdecorator.go:esDecoratorTransformer.injectPendingInitializers
    fn inject_pending_initializers(
        &self,
        visitor: &mut NodeVisitor<'_>,
        ci: &ClassInfoRef,
        is_static: bool,
        expression: Option<NodeId>,
    ) -> Option<NodeId> {
        let pending = if is_static {
            ci.borrow().pending_static_initializers.clone()
        } else {
            ci.borrow().pending_instance_initializers.clone()
        };
        let result = self.prepend_expressions(visitor.factory_mut(), &pending, expression);
        if result != expression {
            let mut ci = ci.borrow_mut();
            if is_static {
                ci.pending_static_initializers.clear();
            } else {
                ci.pending_instance_initializers.clear();
            }
        }
        result
    }

    /// Transforms all of the decorators for a declaration into an array of expressions.
    // port: tsc/internal/transformers/estransforms/esdecorator.go:esDecoratorTransformer.transformAllDecoratorsOfDeclaration
    fn transform_all_decorators_of_declaration(
        &self,
        visitor: &mut NodeVisitor<'_>,
        decorators: &[NodeId],
    ) -> Vec<NodeId> {
        if decorators.is_empty() {
            return Vec::new();
        }
        let mut result = Vec::with_capacity(decorators.len());
        for &decorator in decorators {
            result.push(self.transform_decorator(visitor, decorator));
        }
        result
    }

    /// Transforms a decorator into an expression.
    // port: tsc/internal/transformers/estransforms/esdecorator.go:esDecoratorTransformer.transformDecorator
    fn transform_decorator(&self, visitor: &mut NodeVisitor<'_>, decorator: NodeId) -> NodeId {
        let expression = visitor.factory().node(decorator).expression();
        let expression = self
            .visit_node(visitor, Visit::Main, expression)
            .expect(NIL);
        let mut ec = self.ctx();
        ec.set_emit_flags(expression, emit_flags::NO_COMMENTS);

        // preserve the 'this' binding for an access expression
        let inner_expression = skip_outer_expressions_all(visitor.factory(), expression);
        if matches!(
            Self::kind(visitor, inner_expression),
            K::PropertyAccessExpression | K::ElementAccessExpression
        ) {
            let (target, this_arg) = self.create_call_binding(visitor, expression);
            let factory = visitor.factory_mut();
            let bind_call = ec.new_function_bind_call(factory, target, this_arg, Vec::new());
            return restore_outer_expressions_all(&ec, factory, Some(expression), bind_call);
        }
        expression
    }

    // port: tsc/internal/transformers/estransforms/esdecorator.go:esDecoratorTransformer.createCallBinding
    fn create_call_binding(
        &self,
        visitor: &mut NodeVisitor<'_>,
        expression: NodeId,
    ) -> (NodeId, NodeId) {
        let mut ec = self.ctx();
        let callee = skip_outer_expressions_all(visitor.factory(), expression);
        if is_super_property(visitor.factory(), callee) {
            return (callee, ec.new_this_expression(visitor.factory_mut()));
        }
        let callee_kind = Self::kind(visitor, callee);
        if callee_kind == K::SuperKeyword {
            return (callee, ec.new_this_expression(visitor.factory_mut()));
        }
        if ec.emit_flags(callee) & emit_flags::HELPER_NAME != 0 {
            return (callee, ec.new_void_zero_expression(visitor.factory_mut()));
        }
        if callee_kind == K::PropertyAccessExpression || callee_kind == K::ElementAccessExpression {
            let (receiver, callee_loc) = {
                let read = visitor.factory().node(callee);
                (read.expression().expect(NIL), read.range())
            };
            if self.should_be_captured_in_temp_variable(visitor, receiver) {
                let factory = visitor.factory_mut();
                let this_arg = ec.new_temp_variable(factory);
                ec.add_variable_declaration(factory, this_arg);
                let assign = ec.new_assignment_expression(factory, this_arg, receiver);
                let receiver_loc = factory.node(receiver).range();
                factory.set_node_range(assign, receiver_loc);
                let target = if callee_kind == K::PropertyAccessExpression {
                    let name = factory.node(callee).name();
                    factory.new_property_access_expression(
                        Some(assign),
                        None,
                        name,
                        node_flags::NONE,
                    )
                } else {
                    let argument = factory
                        .node(callee)
                        .as_element_access_expression()
                        .expect("ElementAccessExpression payload")
                        .argument_expression();
                    factory.new_element_access_expression(
                        Some(assign),
                        None,
                        argument,
                        node_flags::NONE,
                    )
                };
                factory.set_node_range(target, callee_loc);
                return (target, this_arg);
            }
            return (callee, receiver);
        }
        (
            expression,
            ec.new_void_zero_expression(visitor.factory_mut()),
        )
    }

    /// A simplified version of the general `shouldBeCapturedInTempVariable`
    /// from the node factory with `cacheIdentifiers` true, since
    /// `createCallBinding` in this transform always caches identifiers.
    // port: tsc/internal/transformers/estransforms/esdecorator.go:esDecoratorTransformer.shouldBeCapturedInTempVariable
    #[allow(clippy::match_same_arms)] // Keep each pinned case independently auditable.
    fn should_be_captured_in_temp_variable(&self, visitor: &NodeVisitor<'_>, node: NodeId) -> bool {
        let target = match super::utilities::skip_parentheses(visitor.factory(), node) {
            Ok(target) => target,
            Err(error) => {
                self.failure.record(error);
                return true;
            }
        };
        match Self::kind(visitor, target) {
            // cacheIdentifiers is always true for this transform's createCallBinding
            K::Identifier => true,
            K::ThisKeyword | K::NumericLiteral | K::BigIntLiteral | K::StringLiteral => false,
            _ => true,
        }
    }

    /// Creates a "value", "get", or "set" method for a pseudo-PropertyDescriptor
    /// object created for a private element.
    // port: tsc/internal/transformers/estransforms/esdecorator.go:esDecoratorTransformer.createDescriptorMethod
    #[allow(clippy::too_many_arguments)] // upstream's positional signature
    fn create_descriptor_method(
        &self,
        factory: &mut dyn RuntimeFactory,
        original: NodeId,
        name: NodeId, // PrivateIdentifier
        modifiers: Option<NodeListId>,
        asterisk_token: Option<NodeId>,
        kind: &[u8],
        parameters: Option<NodeListId>,
        body: Option<NodeId>,
    ) -> NodeId {
        let mut ec = self.ctx();

        let body = if let Some(body) = body {
            body
        } else {
            let statements = new_node_list(factory, Vec::new());
            factory.new_block(Some(statements), false)
        };

        let func_expr = factory.new_function_expression(
            modifiers,
            asterisk_token,
            None, // name
            None, // typeParameters
            parameters,
            None, // type
            None, // fullSignature
            Some(body),
        );
        ec.set_original(func_expr, original);
        let range = move_range_past_decorators(factory, original);
        ec.set_source_map_range(func_expr, range);
        ec.set_emit_flags(func_expr, emit_flags::NO_COMMENTS);

        let prefix: &[u8] = if kind == b"get" || kind == b"set" {
            kind
        } else {
            b""
        };
        let function_name = ec.new_string_literal_from_node(factory, name);
        let named_function =
            ec.new_set_function_name_helper(factory, func_expr, function_name, prefix);

        let kind_name = factory.new_identifier(text(kind));
        let method = factory.new_property_assignment(
            None,
            Some(kind_name),
            None,
            None,
            Some(named_function),
        );
        ec.set_original(method, original);
        let range = move_range_past_decorators(factory, original);
        ec.set_source_map_range(method, range);
        ec.set_emit_flags(method, emit_flags::NO_COMMENTS);
        method
    }

    /// Creates a pseudo-PropertyDescriptor object used when decorating a private MethodDeclaration.
    // port: tsc/internal/transformers/estransforms/esdecorator.go:esDecoratorTransformer.createMethodDescriptorObject
    fn create_method_descriptor_object(
        &self,
        visitor: &mut NodeVisitor<'_>,
        member: NodeId,
        modifiers: Option<NodeListId>,
    ) -> NodeId {
        let (parameters, body, name, asterisk_token) = {
            let read = visitor.factory().node(member);
            (
                read.parameter_list(),
                read.body(),
                read.name().expect(NIL),
                read.as_method_declaration()
                    .expect("MethodDeclaration payload")
                    .asterisk_token(),
            )
        };
        let parameters = self.visit_nodes(visitor, Visit::Main, parameters);
        let body = self.visit_node(visitor, Visit::Main, body);
        let factory = visitor.factory_mut();
        let method = self.create_descriptor_method(
            factory,
            member,
            name,
            modifiers,
            asterisk_token,
            b"value",
            parameters,
            body,
        );
        let properties = new_node_list(factory, vec![method]);
        factory.new_object_literal_expression(Some(properties), false)
    }

    /// Creates a pseudo-PropertyDescriptor object used when decorating a private GetAccessor.
    // port: tsc/internal/transformers/estransforms/esdecorator.go:esDecoratorTransformer.createGetAccessorDescriptorObject
    fn create_get_accessor_descriptor_object(
        &self,
        visitor: &mut NodeVisitor<'_>,
        member: NodeId,
        modifiers: Option<NodeListId>,
    ) -> NodeId {
        let (body, name) = {
            let read = visitor.factory().node(member);
            (read.body(), read.name().expect(NIL))
        };
        let body = self.visit_node(visitor, Visit::Main, body);
        let factory = visitor.factory_mut();
        let parameters = new_node_list(factory, Vec::new());
        let method = self.create_descriptor_method(
            factory,
            member,
            name,
            modifiers,
            None,
            b"get",
            Some(parameters),
            body,
        );
        let properties = new_node_list(factory, vec![method]);
        factory.new_object_literal_expression(Some(properties), false)
    }

    /// Creates a pseudo-PropertyDescriptor object used when decorating a private SetAccessor.
    // port: tsc/internal/transformers/estransforms/esdecorator.go:esDecoratorTransformer.createSetAccessorDescriptorObject
    fn create_set_accessor_descriptor_object(
        &self,
        visitor: &mut NodeVisitor<'_>,
        member: NodeId,
        modifiers: Option<NodeListId>,
    ) -> NodeId {
        let (parameters, body, name) = {
            let read = visitor.factory().node(member);
            (read.parameter_list(), read.body(), read.name().expect(NIL))
        };
        let parameters = self.visit_nodes(visitor, Visit::Main, parameters);
        let body = self.visit_node(visitor, Visit::Main, body);
        let factory = visitor.factory_mut();
        let method = self.create_descriptor_method(
            factory, member, name, modifiers, None, b"set", parameters, body,
        );
        let properties = new_node_list(factory, vec![method]);
        factory.new_object_literal_expression(Some(properties), false)
    }

    /// Creates a pseudo-PropertyDescriptor object used when decorating a private
    /// auto-accessor PropertyDeclaration. The descriptor contains get/set methods
    /// that access the generated backing field.
    // port: tsc/internal/transformers/estransforms/esdecorator.go:esDecoratorTransformer.createAccessorPropertyDescriptorObject
    fn create_accessor_property_descriptor_object(
        &self,
        visitor: &mut NodeVisitor<'_>,
        member: NodeId,
    ) -> NodeId {
        //  {
        //      get() { return this.${privateName}; },
        //      set(value) { this.${privateName} = value; },
        //  }
        let mut ec = self.ctx();
        let factory = visitor.factory_mut();
        let name = factory.node(member).name().expect(NIL);
        let backing_field_name = ec.new_generated_private_name_for_node_ex(
            factory,
            name,
            AutoGenerateOptions {
                suffix: text(b"_accessor_storage"),
                ..AutoGenerateOptions::default()
            },
        );

        let get_parameters = new_node_list(factory, Vec::new());
        let this = ec.new_this_expression(factory);
        let access = factory.new_property_access_expression(
            Some(this),
            None,
            Some(backing_field_name),
            node_flags::NONE,
        );
        let return_statement = factory.new_return_statement(Some(access));
        let get_statements = new_node_list(factory, vec![return_statement]);
        let get_body = factory.new_block(Some(get_statements), false);
        let get = self.create_descriptor_method(
            factory,
            member,
            name,
            None,
            None,
            b"get",
            Some(get_parameters),
            Some(get_body),
        );

        let value = factory.new_identifier(text(b"value"));
        let parameter =
            factory.new_parameter_declaration(None, None, Some(value), None, None, None);
        let set_parameters = new_node_list(factory, vec![parameter]);
        let this = ec.new_this_expression(factory);
        let access = factory.new_property_access_expression(
            Some(this),
            None,
            Some(backing_field_name),
            node_flags::NONE,
        );
        let value = factory.new_identifier(text(b"value"));
        let assignment = ec.new_assignment_expression(factory, access, value);
        let statement = factory.new_expression_statement(Some(assignment));
        let set_statements = new_node_list(factory, vec![statement]);
        let set_body = factory.new_block(Some(set_statements), false);
        let set = self.create_descriptor_method(
            factory,
            member,
            name,
            None,
            None,
            b"set",
            Some(set_parameters),
            Some(set_body),
        );

        let properties = new_node_list(factory, vec![get, set]);
        factory.new_object_literal_expression(Some(properties), false)
    }

    /// Creates a MethodDeclaration that forwards its invocation to a PropertyDescriptor object.
    // port: tsc/internal/transformers/estransforms/esdecorator.go:esDecoratorTransformer.createMethodDescriptorForwarder
    fn create_method_descriptor_forwarder(
        &self,
        visitor: &mut NodeVisitor<'_>,
        modifiers: Option<NodeListId>,
        name: Option<NodeId>,
        descriptor_name: NodeId,
    ) -> NodeId {
        let static_only = self.visit_modifiers(visitor, Visit::StaticOnlyModifier, modifiers);
        let factory = visitor.factory_mut();
        let parameters = new_node_list(factory, Vec::new());
        let value = factory.new_identifier(text(b"value"));
        let access = factory.new_property_access_expression(
            Some(descriptor_name),
            None,
            Some(value),
            node_flags::NONE,
        );
        let return_statement = factory.new_return_statement(Some(access));
        let statements = new_node_list(factory, vec![return_statement]);
        let body = factory.new_block(Some(statements), false);
        factory.new_get_accessor_declaration(
            static_only,
            name,
            None, // typeParameters
            Some(parameters),
            None, // type
            None, // fullSignature
            Some(body),
        )
    }

    /// Creates a GetAccessor that forwards its invocation to a PropertyDescriptor object.
    // port: tsc/internal/transformers/estransforms/esdecorator.go:esDecoratorTransformer.createGetAccessorDescriptorForwarder
    fn create_get_accessor_descriptor_forwarder(
        &self,
        visitor: &mut NodeVisitor<'_>,
        modifiers: Option<NodeListId>,
        name: Option<NodeId>,
        descriptor_name: NodeId,
    ) -> NodeId {
        let static_only = self.visit_modifiers(visitor, Visit::StaticOnlyModifier, modifiers);
        let ec = &self.emit_context;
        let factory = visitor.factory_mut();
        let parameters = new_node_list(factory, Vec::new());
        let get = factory.new_identifier(text(b"get"));
        let access = factory.new_property_access_expression(
            Some(descriptor_name),
            None,
            Some(get),
            node_flags::NONE,
        );
        let this = ec.new_this_expression(factory);
        let call = ec.new_function_call_call(factory, access, Some(this), Vec::new());
        let return_statement = factory.new_return_statement(Some(call));
        let statements = new_node_list(factory, vec![return_statement]);
        let body = factory.new_block(Some(statements), false);
        factory.new_get_accessor_declaration(
            static_only,
            name,
            None, // typeParameters
            Some(parameters),
            None, // type
            None, // fullSignature
            Some(body),
        )
    }

    /// Creates a SetAccessor that forwards its invocation to a PropertyDescriptor object.
    // port: tsc/internal/transformers/estransforms/esdecorator.go:esDecoratorTransformer.createSetAccessorDescriptorForwarder
    fn create_set_accessor_descriptor_forwarder(
        &self,
        visitor: &mut NodeVisitor<'_>,
        modifiers: Option<NodeListId>,
        name: Option<NodeId>,
        descriptor_name: NodeId,
    ) -> NodeId {
        let static_only = self.visit_modifiers(visitor, Visit::StaticOnlyModifier, modifiers);
        let ec = &self.emit_context;
        let factory = visitor.factory_mut();
        let value = factory.new_identifier(text(b"value"));
        let parameter =
            factory.new_parameter_declaration(None, None, Some(value), None, None, None);
        let parameters = new_node_list(factory, vec![parameter]);
        let set = factory.new_identifier(text(b"set"));
        let access = factory.new_property_access_expression(
            Some(descriptor_name),
            None,
            Some(set),
            node_flags::NONE,
        );
        let this = ec.new_this_expression(factory);
        let value = factory.new_identifier(text(b"value"));
        let call = ec.new_function_call_call(factory, access, Some(this), vec![value]);
        let return_statement = factory.new_return_statement(Some(call));
        let statements = new_node_list(factory, vec![return_statement]);
        let body = factory.new_block(Some(statements), false);
        factory.new_set_accessor_declaration(
            static_only,
            name,
            None, // typeParameters
            Some(parameters),
            None, // type
            None, // fullSignature
            Some(body),
        )
    }

    // port: tsc/internal/transformers/estransforms/esdecorator.go:esDecoratorTransformer.createMetadata
    fn create_metadata(
        &self,
        factory: &mut dyn RuntimeFactory,
        name: NodeId,
        class_super: Option<NodeId>,
    ) -> NodeId {
        let ec = &self.emit_context;

        let super_metadata = match class_super {
            Some(class_super) => Self::create_symbol_metadata_reference(factory, class_super),
            None => Self::new_null(factory),
        };

        let object = factory.new_identifier(text(b"Object"));
        let create = factory.new_identifier(text(b"create"));
        let callee = factory.new_property_access_expression(
            Some(object),
            None,
            Some(create),
            node_flags::NONE,
        );
        let arguments = new_node_list(factory, vec![super_metadata]);
        let object_create = factory.new_call_expression(
            Some(callee),
            None,
            None,
            Some(arguments),
            node_flags::NONE,
        );

        let symbol = factory.new_identifier(text(b"Symbol"));
        let type_check = ec.new_type_check(factory, symbol, b"function");
        let symbol = factory.new_identifier(text(b"Symbol"));
        let metadata = factory.new_identifier(text(b"metadata"));
        let symbol_metadata = factory.new_property_access_expression(
            Some(symbol),
            None,
            Some(metadata),
            node_flags::NONE,
        );
        let symbol_check = ec.new_logical_and_expression(factory, type_check, symbol_metadata);

        let question = factory.new_token(K::QuestionToken.into());
        let colon = factory.new_token(K::ColonToken.into());
        let void_zero = ec.new_void_zero_expression(factory);
        let conditional = factory.new_conditional_expression(
            Some(symbol_check),
            Some(question),
            Some(object_create),
            Some(colon),
            Some(void_zero),
        );

        let var_decl = factory.new_variable_declaration(Some(name), None, None, Some(conditional));
        let declarations = new_node_list(factory, vec![var_decl]);
        let var_decl_list =
            factory.new_variable_declaration_list(Some(declarations), node_flags::CONST);
        factory.new_variable_statement(None, Some(var_decl_list))
    }

    // port: tsc/internal/transformers/estransforms/esdecorator.go:esDecoratorTransformer.createSymbolMetadata
    fn create_symbol_metadata(
        &self,
        factory: &mut dyn RuntimeFactory,
        target: NodeId,
        value: NodeId,
    ) -> NodeId {
        let ec = &self.emit_context;

        // Object.defineProperty(target, Symbol.metadata, { configurable: true, writable: true, enumerable: true, value })
        let symbol = factory.new_identifier(text(b"Symbol"));
        let metadata = factory.new_identifier(text(b"metadata"));
        let symbol_metadata = factory.new_property_access_expression(
            Some(symbol),
            None,
            Some(metadata),
            node_flags::NONE,
        );

        let mut descriptor_props = Vec::with_capacity(4);
        for property in [&b"enumerable"[..], b"configurable", b"writable"] {
            let name = factory.new_identifier(text(property));
            let true_expression = ec.new_true_expression(factory);
            descriptor_props.push(factory.new_property_assignment(
                None,
                Some(name),
                None,
                None,
                Some(true_expression),
            ));
        }
        let value_name = factory.new_identifier(text(b"value"));
        descriptor_props.push(factory.new_property_assignment(
            None,
            Some(value_name),
            None,
            None,
            Some(value),
        ));
        let properties = new_node_list(factory, descriptor_props);
        let descriptor = factory.new_object_literal_expression(Some(properties), false);

        let object = factory.new_identifier(text(b"Object"));
        let define_property = factory.new_identifier(text(b"defineProperty"));
        let callee = factory.new_property_access_expression(
            Some(object),
            None,
            Some(define_property),
            node_flags::NONE,
        );
        let arguments = new_node_list(factory, vec![target, symbol_metadata, descriptor]);
        let define_property = factory.new_call_expression(
            Some(callee),
            None,
            None,
            Some(arguments),
            node_flags::NONE,
        );

        let statement = factory.new_expression_statement(Some(define_property));
        let if_statement = factory.new_if_statement(Some(value), Some(statement), None);
        self.ctx()
            .set_emit_flags(if_statement, emit_flags::SINGLE_LINE);
        if_statement
    }

    // port: tsc/internal/transformers/estransforms/esdecorator.go:esDecoratorTransformer.createSymbolMetadataReference
    fn create_symbol_metadata_reference(
        factory: &mut dyn RuntimeFactory,
        class_super: NodeId,
    ) -> NodeId {
        let symbol = factory.new_identifier(text(b"Symbol"));
        let metadata = factory.new_identifier(text(b"metadata"));
        let symbol_metadata = factory.new_property_access_expression(
            Some(symbol),
            None,
            Some(metadata),
            node_flags::NONE,
        );
        let element_access = factory.new_element_access_expression(
            Some(class_super),
            None,
            Some(symbol_metadata),
            node_flags::NONE,
        );
        let operator = factory.new_token(K::QuestionQuestionToken.into());
        let null = Self::new_null(factory);
        factory.new_binary_expression(None, Some(element_access), None, Some(operator), Some(null))
    }
}

// port: tsc/internal/transformers/estransforms/esdecorator.go:injectClassThisAssignmentIfMissing
fn inject_class_this_assignment_if_missing(
    emit_context: &EmitContext,
    factory: &mut dyn RuntimeFactory,
    node: NodeId,
    class_this: NodeId,
) -> NodeId {
    if class_has_class_this_assignment(emit_context, factory, node) {
        return node;
    }
    let mut ec = emit_context.clone();

    // Create: static { _classThis = this; }
    let this = ec.new_this_expression(factory);
    let expression = ec.new_assignment_expression(factory, class_this, this);
    let statement = factory.new_expression_statement(Some(expression));
    let statements = new_node_list(factory, vec![statement]);
    let body = factory.new_block(Some(statements), false);
    let static_block = factory.new_class_static_block_declaration(None, Some(body));
    ec.set_class_this(static_block, class_this);

    if let Some(name) = factory.node(node).name() {
        let loc = factory.node(name).range();
        ec.set_source_map_range(statement, loc);
    }

    let members = list_nodes(factory, factory.node(node).member_list());
    let mut new_members = Vec::with_capacity(1 + members.len());
    new_members.push(static_block);
    new_members.extend(members);
    let members_list = new_node_list(factory, new_members);
    let member_list = factory.node(node).member_list().expect(NIL);
    let loc = factory.read_list(member_list).loc();
    factory.set_list_location(members_list, loc);

    let kind = kind_of(factory, node);
    let (modifiers, name, heritage_clauses) = {
        let read = factory.node(node);
        let heritage_clauses = if let Some(data) = read.as_class_declaration() {
            data.heritage_clauses()
        } else {
            read.as_class_expression()
                .expect("ClassExpression payload")
                .heritage_clauses()
        };
        (read.modifiers(), read.name(), heritage_clauses)
    };
    let updated_node = if kind == K::ClassDeclaration {
        factory.update_class_declaration(
            node,
            modifiers,
            name,
            None,
            heritage_clauses,
            Some(members_list),
        )
    } else {
        factory.update_class_expression(
            node,
            modifiers,
            name,
            None,
            heritage_clauses,
            Some(members_list),
        )
    };
    ec.set_class_this(updated_node, class_this);
    updated_node
}
