//! `transformers/estransforms/utilities.go`: the class-declaration to
//! class-expression conversion, the not-null condition, the `super` access
//! state the async and async-generator transforms share, and the backing field
//! of an `accessor` property.
//!
//! Functions take the emit context (a handle: mutators run on a clone that
//! shares its tables) and the factory a transformer holds
//! (`visitor.factory()` / `visitor.factory_mut()`). The `ast` helpers whose
//! `tsr_ast` ports read through an `AstView` (`SkipOuterExpressions`,
//! `IsAssignmentExpression`, `IsSuperProperty`, `HasSyntacticModifier`) and the
//! printer's `RestoreOuterExpressions`, which needs the concrete builder, are
//! re-expressed here over `Factory` reads; they carry no port marker.
use crate::extract_modifiers;
use crate::transformer::Error;
use std::cell::{Cell, RefCell};
use tsr_ast::{
    is_assignment_operator, is_left_hand_side_expression_kind, modifier_flags, node_flags,
    utilities::{node_is_synthesized, range_is_synthesized},
    AstView, Factory, FactoryMethods, JsString, NodeId, NodeListId, NodeVisitor, RuntimeFactory,
    SyntaxKind as K,
};
use tsr_core::{collections::OrderedSet, TextRange};
use tsr_printer::{AutoGenerateOptions, EmitContext, EmitVisitorHooks};

pub(crate) const NIL: &str = "runtime error: invalid memory address or nil pointer dereference";

pub(super) use crate::utilities::is_simple_copiable_expression;

/// The storage view of the visitor's factory, which the AST predicates read;
/// a factory without one (a lazy JSDoc transaction) cannot be transformed.
pub(crate) fn view(factory: &dyn RuntimeFactory) -> Result<AstView<'_>, Error> {
    factory.ast_view().ok_or(Error::Unsupported(
        "a transform over a factory without AST storage",
    ))
}

/// `node.SubtreeFacts()`.
pub(crate) fn subtree_facts(factory: &dyn RuntimeFactory, node: NodeId) -> Result<u32, Error> {
    Ok(view(factory)?.subtree_facts(node))
}

/// `ast.SkipParentheses`.
pub(crate) fn skip_parentheses(
    factory: &dyn RuntimeFactory,
    node: NodeId,
) -> Result<NodeId, Error> {
    Ok(tsr_ast::skip_parentheses(view(factory)?, node)?)
}

/// Go's `list.Nodes` of an optional list: a nil list has no nodes. A nil
/// element is a nil dereference at its use, so it is rejected here.
pub(crate) fn list_nodes(factory: &dyn RuntimeFactory, list: Option<NodeListId>) -> Vec<NodeId> {
    let Some(list) = list else {
        return Vec::new();
    };
    let nodes = factory.read_list(list).nodes();
    factory
        .read_nodes(nodes)
        .iter()
        .map(|node| node.expect(NIL))
        .collect()
}

/// `NodeFactory.NewNodeList`: an undefined location.
pub(crate) fn new_node_list(factory: &mut dyn RuntimeFactory, nodes: Vec<NodeId>) -> NodeListId {
    let nodes = factory.alloc_nodes(nodes.into_iter().map(Some).collect());
    factory.alloc_list(TextRange::new(-1, -1), nodes)
}

/// `ast.IsOuterExpression(node, ast.OEKAll)` over factory reads: `OEKAll`
/// excludes the JSDoc-assertion, assignment and comma cases.
fn is_outer_expression_all(factory: &dyn Factory, node: NodeId) -> bool {
    matches!(
        factory.node(node).kind().known(),
        Some(
            K::ParenthesizedExpression
                | K::TypeAssertionExpression
                | K::AsExpression
                | K::SatisfiesExpression
                | K::ExpressionWithTypeArguments
                | K::NonNullExpression
                | K::PartiallyEmittedExpression
        )
    )
}

/// `ast.SkipOuterExpressions(node, ast.OEKAll)` over factory reads.
pub(crate) fn skip_outer_expressions_all(factory: &dyn Factory, mut node: NodeId) -> NodeId {
    while is_outer_expression_all(factory, node) {
        node = factory.node(node).expression().expect(NIL);
    }
    node
}

/// `printer.NodeFactory.isIgnorableParen`.
fn is_ignorable_paren(emit_context: &EmitContext, factory: &dyn Factory, node: NodeId) -> bool {
    let read = factory.node(node);
    read.kind() == K::ParenthesizedExpression
        && node_is_synthesized(&read)
        && range_is_synthesized(emit_context.source_map_range(factory, node))
        && range_is_synthesized(emit_context.comment_range_of(factory, node))
}

/// `printer.NodeFactory.updateOuterExpression`.
fn update_outer_expression(
    factory: &mut dyn RuntimeFactory,
    outer_expression: NodeId,
    expression: NodeId,
) -> NodeId {
    let read = factory.node(outer_expression);
    let kind = read.kind();
    match kind.known() {
        Some(K::ParenthesizedExpression) => {
            drop(read);
            factory.update_parenthesized_expression(outer_expression, Some(expression))
        }
        Some(K::TypeAssertionExpression) => {
            let type_node = read.type_node();
            drop(read);
            factory.update_type_assertion(outer_expression, type_node, Some(expression))
        }
        Some(K::AsExpression) => {
            let type_node = read.type_node();
            drop(read);
            factory.update_as_expression(outer_expression, Some(expression), type_node)
        }
        Some(K::SatisfiesExpression) => {
            let type_node = read.type_node();
            drop(read);
            factory.update_satisfies_expression(outer_expression, Some(expression), type_node)
        }
        Some(K::NonNullExpression) => {
            let flags = read.flags();
            drop(read);
            factory.update_non_null_expression(outer_expression, Some(expression), flags)
        }
        Some(K::ExpressionWithTypeArguments) => {
            let type_arguments = read.type_argument_list();
            drop(read);
            factory.update_expression_with_type_arguments(
                outer_expression,
                Some(expression),
                type_arguments,
            )
        }
        Some(K::PartiallyEmittedExpression) => {
            drop(read);
            factory.update_partially_emitted_expression(outer_expression, Some(expression))
        }
        _ => panic!("Unexpected outer expression kind: {kind:?}"),
    }
}

/// `printer.NodeFactory.RestoreOuterExpressions(outer, inner, ast.OEKAll)`
/// over factory reads.
pub(crate) fn restore_outer_expressions_all(
    emit_context: &EmitContext,
    factory: &mut dyn RuntimeFactory,
    outer_expression: Option<NodeId>,
    inner_expression: NodeId,
) -> NodeId {
    if let Some(outer_expression) = outer_expression {
        if is_outer_expression_all(factory, outer_expression)
            && !is_ignorable_paren(emit_context, factory, outer_expression)
        {
            let expression = factory.node(outer_expression).expression();
            let restored =
                restore_outer_expressions_all(emit_context, factory, expression, inner_expression);
            return update_outer_expression(factory, outer_expression, restored);
        }
    }
    inner_expression
}

/// `ast.IsAssignmentExpression` over factory reads (`IsLeftHandSideExpression`
/// skips partially emitted expressions first).
pub(crate) fn is_assignment_expression(
    factory: &dyn Factory,
    node: NodeId,
    exclude_compound_assignment: bool,
) -> bool {
    let read = factory.node(node);
    if read.kind() == K::BinaryExpression {
        let binary = read
            .as_binary_expression()
            .expect("BinaryExpression payload");
        let operator = factory.node(binary.operator_token().expect(NIL)).kind();
        if !(operator == K::EqualsToken
            || !exclude_compound_assignment && is_assignment_operator(operator))
        {
            return false;
        }
        let mut left = binary.left().expect(NIL);
        while factory.node(left).kind() == K::PartiallyEmittedExpression {
            left = factory.node(left).expression().expect(NIL);
        }
        return is_left_hand_side_expression_kind(factory.node(left).kind());
    }
    false
}

/// `ast.IsSuperProperty` over factory reads.
pub(crate) fn is_super_property(factory: &dyn Factory, node: NodeId) -> bool {
    let read = factory.node(node);
    matches!(
        read.kind().known(),
        Some(K::PropertyAccessExpression | K::ElementAccessExpression)
    ) && factory.node(read.expression().expect(NIL)).kind() == K::SuperKeyword
}

/// `ast.HasSyntacticModifier` over factory reads: the flags of the node's
/// modifier list.
pub(crate) fn has_syntactic_modifier(
    factory: &dyn RuntimeFactory,
    node: NodeId,
    flags: u32,
) -> bool {
    let modifiers = factory.node(node).modifiers();
    let modifier_flags = modifiers.map_or(modifier_flags::NONE, |list| {
        factory.read_list(list).modifier_flags()
    });
    modifier_flags & flags != 0
}

/// Go's `Node.Text()` of a member name.
fn member_name_text(factory: &dyn Factory, name: NodeId) -> JsString {
    let read = factory.node(name);
    if let Some(identifier) = read.as_identifier() {
        return identifier.text_owned();
    }
    if let Some(identifier) = read.as_private_identifier() {
        return identifier.text_owned();
    }
    panic!("Unhandled case in Node.Text: {:?}", read.kind())
}

// TODO(a8): estransforms.assignmentTargetContainsSuperProperty
/// Checks top-down whether an assignment target expression contains a super
/// property or element access (`super.x` or `super[x]`). A local copy of
/// `async.go`'s function, which the async unit ports.
fn assignment_target_contains_super_property(factory: &dyn RuntimeFactory, node: NodeId) -> bool {
    let read = factory.node(node);
    match read.kind().known() {
        Some(K::PropertyAccessExpression | K::ElementAccessExpression) => {
            return factory.node(read.expression().expect(NIL)).kind() == K::SuperKeyword;
        }
        // Upstream's separate parenthesized and spread-element cases.
        Some(K::ParenthesizedExpression | K::SpreadElement) => {
            return assignment_target_contains_super_property(
                factory,
                read.expression().expect(NIL),
            );
        }
        Some(K::ArrayLiteralExpression) => {
            let elements = list_nodes(factory, Some(read.element_list().expect(NIL)));
            return elements
                .into_iter()
                .any(|element| assignment_target_contains_super_property(factory, element));
        }
        Some(K::ObjectLiteralExpression) => {
            for property in list_nodes(factory, Some(read.property_list().expect(NIL))) {
                let property_read = factory.node(property);
                let target = match property_read.kind().known() {
                    Some(K::PropertyAssignment) => property_read.initializer(),
                    Some(K::ShorthandPropertyAssignment) => property_read.name(),
                    Some(K::SpreadAssignment) => property_read.expression(),
                    _ => continue,
                };
                if assignment_target_contains_super_property(factory, target.expect(NIL)) {
                    return true;
                }
            }
        }
        _ => {}
    }
    false
}

// TODO(a8): estransforms.isUpdateExpression
/// Whether a prefix or postfix unary expression is `++` or `--`. A local copy
/// of `async.go`'s function, which the async unit ports.
fn is_update_expression(factory: &dyn Factory, node: NodeId) -> bool {
    let read = factory.node(node);
    if let Some(prefix) = read.as_prefix_unary_expression() {
        let operator = prefix.operator();
        return operator == K::PlusPlusToken || operator == K::MinusMinusToken;
    }
    if let Some(postfix) = read.as_postfix_unary_expression() {
        let operator = postfix.operator();
        return operator == K::PlusPlusToken || operator == K::MinusMinusToken;
    }
    false
}

/// The class expression for a class declaration, without its `export` and
/// `default` modifiers, with the declaration as its original and location.
// port: tsc/internal/transformers/estransforms/utilities.go:convertClassDeclarationToClassExpression
pub fn convert_class_declaration_to_class_expression(
    emit_context: &EmitContext,
    factory: &mut dyn RuntimeFactory,
    node: NodeId,
) -> NodeId {
    let read = factory.node(node);
    let declaration = read
        .as_class_declaration()
        .expect("ClassDeclaration payload");
    let (modifiers, name, type_parameters, heritage_clauses, members, loc) = (
        read.modifiers(),
        read.name(),
        declaration.type_parameters(),
        declaration.heritage_clauses(),
        declaration.members(),
        read.range(),
    );
    drop(read);
    let modifiers = extract_modifiers(
        emit_context,
        factory,
        modifiers,
        !modifier_flags::EXPORT_DEFAULT,
    );
    let updated =
        factory.new_class_expression(modifiers, name, type_parameters, heritage_clauses, members);
    emit_context.clone().set_original(updated, node);
    factory.node_mut(updated).set_range(loc);
    updated
}

/// `left !== null && right !== void 0`, or with `invert`
/// `left === null || right === void 0`.
// port: tsc/internal/transformers/estransforms/utilities.go:createNotNullCondition
pub fn create_not_null_condition(
    emit_context: &EmitContext,
    factory: &mut dyn RuntimeFactory,
    left: NodeId,
    right: NodeId,
    invert: bool,
) -> NodeId {
    let mut token = K::ExclamationEqualsEqualsToken;
    let mut op = K::AmpersandAmpersandToken;
    if invert {
        token = K::EqualsEqualsEqualsToken;
        op = K::BarBarToken;
    }

    let left_token = factory.new_token(token.into());
    let null = factory.new_keyword_expression(K::NullKeyword.into());
    let left = factory.new_binary_expression(None, Some(left), None, Some(left_token), Some(null));
    let operator = factory.new_token(op.into());
    let right_token = factory.new_token(token.into());
    let void_zero = emit_context.new_void_zero_expression(factory);
    let right =
        factory.new_binary_expression(None, Some(right), None, Some(right_token), Some(void_zero));
    factory.new_binary_expression(None, Some(left), None, Some(operator), Some(right))
}

/// Tracks super property/element accesses and super property assignments
/// within async function or async generator bodies. Upstream embeds it in
/// both the async and the async-generator transformer; here those
/// transformers hold one in their shared state. The fields are the upstream
/// struct's, which the transformers save and restore around each function:
/// `captured_super_properties` is `None` outside one (upstream's nil set).
///
/// No cell is borrowed across a visit: substitution reads the bindings and
/// flags by copy.
pub struct SuperAccessState {
    emit_context: EmitContext,
    hooks: EmitVisitorHooks,

    /// Keeps track of property names accessed on super (`super.x`) within
    /// async functions.
    pub(crate) captured_super_properties: RefCell<Option<OrderedSet<JsString>>>,
    /// Whether the async function contains an element access on super (`super[x]`).
    pub(crate) has_super_element_access: Cell<bool>,
    pub(crate) has_super_property_assignment: Cell<bool>,

    pub(crate) super_binding: Cell<Option<NodeId>>,
    pub(crate) super_index_binding: Cell<Option<NodeId>>,
}

impl SuperAccessState {
    /// The state with no function entered, bound to the transformer's emit
    /// context. Upstream initializes the embedded struct in place and creates
    /// its visitor there; here the visitor is created per substitution over
    /// the factory the caller holds, with the context's visitor hooks.
    // port: tsc/internal/transformers/estransforms/utilities.go:superAccessState.initSuperAccessVisitor
    pub fn new(emit_context: &EmitContext) -> Self {
        Self {
            emit_context: emit_context.clone(),
            hooks: emit_context.visitor_hooks(),
            captured_super_properties: RefCell::new(None),
            has_super_element_access: Cell::new(false),
            has_super_property_assignment: Cell::new(false),
            super_binding: Cell::new(None),
            super_index_binding: Cell::new(None),
        }
    }

    /// Walks the async/generator body and replaces super property/element
    /// accesses with `_super`/`_superIndex` references. This is necessary
    /// because the async body ends up inside a generator function where
    /// `super` is not valid.
    // port: tsc/internal/transformers/estransforms/utilities.go:superAccessState.visitSuperAccessNode
    fn visit_super_access_node(
        &self,
        visitor: &mut NodeVisitor<'_>,
        node: Option<NodeId>,
    ) -> Option<NodeId> {
        let id = node.expect(NIL);
        let read = visitor.factory().node(id);
        match read.kind().known() {
            Some(K::CallExpression) => {
                let expression = read.expression().expect(NIL);
                drop(read);
                if is_super_property(visitor.factory(), expression) {
                    return Some(self.substitute_call_expression_with_super_access(visitor, id));
                }
                visitor.visit_each_child(node)
            }
            Some(K::PropertyAccessExpression) => {
                let expression = read.expression().expect(NIL);
                let name = read.name();
                drop(read);
                if visitor.factory().node(expression).kind() == K::SuperKeyword {
                    // super.x → _super.x
                    return Some(visitor.factory_mut().new_property_access_expression(
                        self.super_binding.get(),
                        None,
                        name,
                        node_flags::NONE,
                    ));
                }
                visitor.visit_each_child(node)
            }
            Some(K::ElementAccessExpression) => {
                let expression = read.expression().expect(NIL);
                let argument = read
                    .as_element_access_expression()
                    .expect("ElementAccessExpression payload")
                    .argument_expression();
                drop(read);
                if visitor.factory().node(expression).kind() == K::SuperKeyword {
                    // super[x] → _superIndex(x) or _superIndex(x).value
                    return Some(self.create_super_element_access_in_async_method(
                        visitor.factory_mut(),
                        argument,
                    ));
                }
                visitor.visit_each_child(node)
            }
            // Don't recurse into non-arrow function scopes or classes
            Some(
                K::FunctionExpression
                | K::FunctionDeclaration
                | K::MethodDeclaration
                | K::GetAccessor
                | K::SetAccessor
                | K::Constructor
                | K::ClassDeclaration
                | K::ClassExpression,
            ) => node,
            _ => {
                drop(read);
                visitor.visit_each_child(node)
            }
        }
    }

    /// Replaces the super accesses of `body` (see `visit_super_access_node`).
    // port: tsc/internal/transformers/estransforms/utilities.go:superAccessState.substituteSuperAccessesInBody
    pub fn substitute_super_accesses_in_body(
        &self,
        factory: &mut dyn RuntimeFactory,
        body: Option<NodeId>,
    ) -> Option<NodeId> {
        let visit = |visitor: &mut NodeVisitor<'_>, node: Option<NodeId>| {
            self.visit_super_access_node(visitor, node)
        };
        self.hooks
            .new_node_visitor(Some(&visit), factory)
            .visit_node(body)
    }

    /// Handles `super.x(args)` and `super[x](args)`; the arguments are visited
    /// with `visitor`.
    // port: tsc/internal/transformers/estransforms/utilities.go:superAccessState.substituteCallExpressionWithSuperAccess
    pub fn substitute_call_expression_with_super_access(
        &self,
        visitor: &mut NodeVisitor<'_>,
        call: NodeId,
    ) -> NodeId {
        let read = visitor.factory().node(call);
        let expression = read.expression().expect(NIL);
        let arguments = read.argument_list();
        let loc = read.range();
        drop(read);
        let expression_read = visitor.factory().node(expression);
        let target = match expression_read.kind().known() {
            Some(K::PropertyAccessExpression) => {
                // super.x(args) → _super.x.call(this, args)
                let name = expression_read.name();
                drop(expression_read);
                visitor.factory_mut().new_property_access_expression(
                    self.super_binding.get(),
                    None,
                    name,
                    node_flags::NONE,
                )
            }
            Some(K::ElementAccessExpression) => {
                // super[x](args) → _superIndex(x).call(this, args) or _superIndex(x).value.call(this, args)
                let argument = expression_read
                    .as_element_access_expression()
                    .expect("ElementAccessExpression payload")
                    .argument_expression();
                drop(expression_read);
                self.create_super_element_access_in_async_method(visitor.factory_mut(), argument)
            }
            _ => {
                drop(expression_read);
                return visitor.visit_each_child(Some(call)).expect(NIL);
            }
        };

        let factory = visitor.factory_mut();
        let call_name = factory.new_identifier(JsString::from_bytes(&b"call"[..]));
        let call_target = factory.new_property_access_expression(
            Some(target),
            None,
            Some(call_name),
            node_flags::NONE,
        );

        let mut all_args = vec![self.emit_context.new_this_expression(factory)];
        if arguments.is_some() {
            let visited_args = visitor.visit_nodes(arguments);
            if visited_args.is_some() {
                all_args.extend(list_nodes(visitor.factory(), visited_args));
            }
        }

        let factory = visitor.factory_mut();
        let all_args = new_node_list(factory, all_args);
        let result = factory.new_call_expression(
            Some(call_target),
            None,
            None,
            Some(all_args),
            node_flags::NONE,
        );
        factory.node_mut(result).set_range(loc);
        result
    }

    /// Creates `_superIndex(x)` or `_superIndex(x).value`.
    // port: tsc/internal/transformers/estransforms/utilities.go:superAccessState.createSuperElementAccessInAsyncMethod
    pub fn create_super_element_access_in_async_method(
        &self,
        factory: &mut dyn RuntimeFactory,
        argument_expression: Option<NodeId>,
    ) -> NodeId {
        let arguments = factory.alloc_nodes(vec![argument_expression]);
        let arguments = factory.alloc_list(TextRange::new(-1, -1), arguments);
        let super_index_call = factory.new_call_expression(
            self.super_index_binding.get(),
            None,
            None,
            Some(arguments),
            node_flags::NONE,
        );
        if self.has_super_property_assignment.get() {
            let value = factory.new_identifier(JsString::from_bytes(&b"value"[..]));
            return factory.new_property_access_expression(
                Some(super_index_call),
                None,
                Some(value),
                node_flags::NONE,
            );
        }
        super_index_call
    }

    /// Creates a variable named `_super` with accessor properties for the
    /// captured property names: a getter for each, and a setter too when the
    /// body assigns a super property.
    ///
    /// ```text
    /// const _super = Object.create(null, {
    ///     x: { get: () => super.x },                           // read-only
    ///     x: { get: () => super.x, set: (v) => super.x = v }, // read-write
    /// });
    /// ```
    ///
    /// Outside a function (a nil captured set) upstream's range over the set
    /// is a nil dereference.
    // port: tsc/internal/transformers/estransforms/utilities.go:superAccessState.createSuperAccessVariableStatement
    pub fn create_super_access_variable_statement(
        &self,
        factory: &mut dyn RuntimeFactory,
    ) -> NodeId {
        let names: Vec<JsString> = self
            .captured_super_properties
            .borrow()
            .as_ref()
            .expect(NIL)
            .values()
            .cloned()
            .collect();
        let has_super_property_assignment = self.has_super_property_assignment.get();
        let text = |bytes: &[u8]| JsString::from_bytes(bytes);
        let mut accessors = Vec::new();

        for name in names {
            let mut descriptor_properties = Vec::new();

            // getter: get: () => super.name
            let super_keyword = factory.new_keyword_expression(K::SuperKeyword.into());
            let property_name = factory.new_identifier(name.clone());
            let getter_body = factory.new_property_access_expression(
                Some(super_keyword),
                None,
                Some(property_name),
                node_flags::NONE,
            );
            let no_parameters = new_node_list(factory, Vec::new());
            let arrow = factory.new_token(K::EqualsGreaterThanToken.into());
            let getter_arrow = factory.new_arrow_function(
                None,
                None,
                Some(no_parameters),
                None,
                None,
                Some(arrow),
                Some(getter_body),
            );
            let get = factory.new_identifier(text(b"get"));
            let getter =
                factory.new_property_assignment(None, Some(get), None, None, Some(getter_arrow));
            descriptor_properties.push(getter);

            if has_super_property_assignment {
                // setter: set: v => super.name = v
                let v = factory.new_identifier(text(b"v"));
                let v_param =
                    factory.new_parameter_declaration(None, None, Some(v), None, None, None);
                let super_keyword = factory.new_keyword_expression(K::SuperKeyword.into());
                let property_name = factory.new_identifier(name.clone());
                let super_prop = factory.new_property_access_expression(
                    Some(super_keyword),
                    None,
                    Some(property_name),
                    node_flags::NONE,
                );
                let v = factory.new_identifier(text(b"v"));
                let assign_expr = self
                    .emit_context
                    .new_assignment_expression(factory, super_prop, v);
                let parameters = new_node_list(factory, vec![v_param]);
                let arrow = factory.new_token(K::EqualsGreaterThanToken.into());
                let setter_arrow = factory.new_arrow_function(
                    None,
                    None,
                    Some(parameters),
                    None,
                    None,
                    Some(arrow),
                    Some(assign_expr),
                );
                let set = factory.new_identifier(text(b"set"));
                let setter = factory.new_property_assignment(
                    None,
                    Some(set),
                    None,
                    None,
                    Some(setter_arrow),
                );
                descriptor_properties.push(setter);
            }

            let descriptor_properties = new_node_list(factory, descriptor_properties);
            let descriptor =
                factory.new_object_literal_expression(Some(descriptor_properties), false);
            let accessor_name = factory.new_identifier(name);
            let accessor = factory.new_property_assignment(
                None,
                Some(accessor_name),
                None,
                None,
                Some(descriptor),
            );
            accessors.push(accessor);
        }

        let accessors = new_node_list(factory, accessors);
        let descriptors_object = factory.new_object_literal_expression(Some(accessors), true);

        let object = factory.new_identifier(text(b"Object"));
        let create = factory.new_identifier(text(b"create"));
        let object_create = factory.new_property_access_expression(
            Some(object),
            None,
            Some(create),
            node_flags::NONE,
        );
        let null = factory.new_keyword_expression(K::NullKeyword.into());
        let arguments = new_node_list(factory, vec![null, descriptors_object]);
        let object_create_call = factory.new_call_expression(
            Some(object_create),
            None,
            None,
            Some(arguments),
            node_flags::NONE,
        );

        let decl = factory.new_variable_declaration(
            self.super_binding.get(),
            None,
            None,
            Some(object_create_call),
        );
        let declarations = new_node_list(factory, vec![decl]);
        let decl_list =
            factory.new_variable_declaration_list(Some(declarations), node_flags::CONST);
        factory.new_variable_statement(None, Some(decl_list))
    }

    /// Records super property/element accesses and super property
    /// assignments for the enclosing async method body. Called from both the
    /// main visitor and auxiliary visitors to ensure super accesses are
    /// tracked regardless of whether the node has transform flags.
    // port: tsc/internal/transformers/estransforms/utilities.go:superAccessState.trackSuperAccess
    pub fn track_super_access(&self, factory: &dyn RuntimeFactory, node: NodeId) {
        if self.captured_super_properties.borrow().is_none() {
            return;
        }
        let read = factory.node(node);
        match read.kind().known() {
            Some(K::PropertyAccessExpression) => {
                if factory.node(read.expression().expect(NIL)).kind() == K::SuperKeyword {
                    let name = member_name_text(factory, read.name().expect(NIL));
                    self.captured_super_properties
                        .borrow_mut()
                        .as_mut()
                        .expect(NIL)
                        .insert(name);
                }
            }
            Some(K::ElementAccessExpression) => {
                if factory.node(read.expression().expect(NIL)).kind() == K::SuperKeyword {
                    self.has_super_element_access.set(true);
                }
            }
            Some(K::BinaryExpression) => {
                let binary = read
                    .as_binary_expression()
                    .expect("BinaryExpression payload");
                let operator = factory.node(binary.operator_token().expect(NIL)).kind();
                if is_assignment_operator(operator)
                    && assignment_target_contains_super_property(factory, binary.left().expect(NIL))
                {
                    self.has_super_property_assignment.set(true);
                }
            }
            Some(K::PrefixUnaryExpression) => {
                let operand = read
                    .as_prefix_unary_expression()
                    .expect("PrefixUnaryExpression payload")
                    .operand();
                if is_update_expression(factory, node)
                    && assignment_target_contains_super_property(factory, operand.expect(NIL))
                {
                    self.has_super_property_assignment.set(true);
                }
            }
            Some(K::PostfixUnaryExpression) => {
                let operand = read
                    .as_postfix_unary_expression()
                    .expect("PostfixUnaryExpression payload")
                    .operand();
                if is_update_expression(factory, node)
                    && assignment_target_contains_super_property(factory, operand.expect(NIL))
                {
                    self.has_super_property_assignment.set(true);
                }
            }
            _ => {}
        }
    }
}

/// Creates a private backing field for an `accessor` property declaration.
// port: tsc/internal/transformers/estransforms/utilities.go:createAccessorPropertyBackingField
pub fn create_accessor_property_backing_field(
    emit_context: &EmitContext,
    factory: &mut dyn RuntimeFactory,
    node: NodeId,
    modifiers: Option<NodeListId>,
    initializer: Option<NodeId>,
) -> NodeId {
    let name = factory.node(node).name().expect(NIL);
    let backing_name = emit_context.clone().new_generated_private_name_for_node_ex(
        factory,
        name,
        AutoGenerateOptions {
            suffix: JsString::from_bytes(&b"_accessor_storage"[..]),
            ..AutoGenerateOptions::default()
        },
    );
    factory.update_property_declaration(
        node,
        modifiers,
        Some(backing_name),
        None, /*postfixToken*/
        None, /*typeNode*/
        initializer,
    )
}
