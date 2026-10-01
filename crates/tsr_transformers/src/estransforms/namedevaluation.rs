//! `transformers/estransforms/namedevaluation.go`: the ECMAScript
//! `NamedEvaluation` of anonymous classes and functions, as the class fields,
//! decorators and `using` transforms perform it.
//!
//! Predicates take the emit context and the factory a transformer reads
//! through (`visitor.factory()`); transforms take the factory it builds with
//! (`visitor.factory_mut()`). The emit context is a handle: mutators run on a
//! clone that shares its tables, so `&EmitContext` suffices.
use super::classthis::is_class_this_assignment_block;
use super::utilities::{
    has_syntactic_modifier, list_nodes, new_node_list, restore_outer_expressions_all,
    skip_outer_expressions_all, NIL,
};
use tsr_ast::{
    modifier_flags, token_flags, utilities::is_property_name_literal,
    utilities_tail::is_proto_setter, FactoryMethods, JsString, NodeId, RuntimeFactory,
    SyntaxKind as K,
};
use tsr_printer::EmitContext;

/// The callback of [`is_named_evaluation_and`]: called with the anonymous
/// function definition (a class expression, function expression or arrow
/// function) and the factory it was read through.
pub type AnonymousFunctionDefinitionCallback<'c> = &'c dyn Fn(&dyn RuntimeFactory, NodeId) -> bool;

/// `ast.IsNamedEvaluationSource` over factory reads.
fn is_named_evaluation_source(factory: &dyn RuntimeFactory, node: NodeId) -> bool {
    let read = factory.node(node);
    let is_identifier = |id: Option<NodeId>| factory.node(id.expect(NIL)).kind() == K::Identifier;
    match read.kind().known() {
        Some(K::PropertyAssignment) => !is_proto_setter(&factory.node(read.name().expect(NIL))),
        Some(K::ShorthandPropertyAssignment) => read
            .as_shorthand_property_assignment()
            .expect("ShorthandPropertyAssignment payload")
            .object_assignment_initializer()
            .is_some(),
        Some(K::VariableDeclaration) => is_identifier(read.name()) && read.initializer().is_some(),
        Some(K::Parameter) => {
            is_identifier(read.name())
                && read.initializer().is_some()
                && read
                    .as_parameter_declaration()
                    .expect("ParameterDeclaration payload")
                    .dot_dot_dot_token()
                    .is_none()
        }
        Some(K::BindingElement) => {
            is_identifier(read.name())
                && read.initializer().is_some()
                && read
                    .as_binding_element()
                    .expect("BindingElement payload")
                    .dot_dot_dot_token()
                    .is_none()
        }
        Some(K::PropertyDeclaration) => read.initializer().is_some(),
        Some(K::BinaryExpression) => {
            let binary = read
                .as_binary_expression()
                .expect("BinaryExpression payload");
            match factory
                .node(binary.operator_token().expect(NIL))
                .kind()
                .known()
            {
                Some(
                    K::EqualsToken
                    | K::AmpersandAmpersandEqualsToken
                    | K::BarBarEqualsToken
                    | K::QuestionQuestionEqualsToken,
                ) => is_identifier(binary.left()),
                _ => false,
            }
        }
        Some(K::ExportAssignment) => true,
        _ => false,
    }
}

/// Gets whether a node is a `static {}` block containing only a single call
/// to the `__setFunctionName` helper where that call's second argument is the
/// value stored in the `assignedName` property of the block's `EmitNode`.
// port: tsc/internal/transformers/estransforms/namedevaluation.go:isClassNamedEvaluationHelperBlock
pub fn is_class_named_evaluation_helper_block(
    emit_context: &EmitContext,
    factory: &dyn RuntimeFactory,
    node: NodeId,
) -> bool {
    let read = factory.node(node);
    if read.kind() != K::ClassStaticBlockDeclaration {
        return false;
    }
    // The payload's `Body`: `Node.Body()` answers nil for a static block.
    let body = read
        .as_class_static_block_declaration()
        .expect("ClassStaticBlockDeclaration payload")
        .body()
        .expect(NIL);
    let statements = list_nodes(factory, factory.node(body).statement_list());
    if statements.len() != 1 {
        return false;
    }

    let statement = factory.node(statements[0]);
    if statement.kind() == K::ExpressionStatement {
        let expression = statement.expression().expect(NIL);
        if emit_context.is_call_to_helper(factory, expression, b"__setFunctionName") {
            // `arguments.Nodes`: a nil list is a nil dereference.
            let arguments = factory.node(expression).argument_list().expect(NIL);
            let arguments = list_nodes(factory, Some(arguments));
            return arguments.len() >= 2 && Some(arguments[1]) == emit_context.assigned_name(node);
        }
    }
    false
}

/// Gets whether a `ClassLikeDeclaration` has a `static {}` block containing
/// only a single call to the `__setFunctionName` helper.
// port: tsc/internal/transformers/estransforms/namedevaluation.go:classHasExplicitlyAssignedName
pub fn class_has_explicitly_assigned_name(
    emit_context: &EmitContext,
    factory: &dyn RuntimeFactory,
    node: NodeId,
) -> bool {
    if emit_context.assigned_name(node).is_some() {
        for member in list_nodes(factory, factory.node(node).member_list()) {
            if is_class_named_evaluation_helper_block(emit_context, factory, member) {
                return true;
            }
        }
    }
    false
}

/// Gets whether a `ClassLikeDeclaration` has a declared name or contains a
/// `static {}` block containing only a single call to the `__setFunctionName`
/// helper.
// port: tsc/internal/transformers/estransforms/namedevaluation.go:classHasDeclaredOrExplicitlyAssignedName
pub fn class_has_declared_or_explicitly_assigned_name(
    emit_context: &EmitContext,
    factory: &dyn RuntimeFactory,
    node: NodeId,
) -> bool {
    factory.node(node).name().is_some()
        || class_has_explicitly_assigned_name(emit_context, factory, node)
}

/// Indicates whether an expression is an anonymous function definition
/// (<https://tc39.es/ecma262/#sec-isanonymousfunctiondefinition>); with `cb`,
/// whether `cb` also accepts that definition.
// port: tsc/internal/transformers/estransforms/namedevaluation.go:isAnonymousFunctionDefinition
pub fn is_anonymous_function_definition(
    emit_context: &EmitContext,
    factory: &dyn RuntimeFactory,
    node: Option<NodeId>,
    cb: Option<AnonymousFunctionDefinitionCallback<'_>>,
) -> bool {
    let node = skip_outer_expressions_all(factory, node.expect(NIL));
    let read = factory.node(node);
    match read.kind().known() {
        Some(K::ClassExpression) => {
            drop(read);
            if class_has_declared_or_explicitly_assigned_name(emit_context, factory, node) {
                return false;
            }
        }
        Some(K::FunctionExpression) => {
            if read.name().is_some() {
                return false;
            }
        }
        Some(K::ArrowFunction) => {}
        _ => return false,
    }
    if let Some(cb) = cb {
        return cb(factory, node);
    }
    true
}

/// Whether `node` is a `NamedEvaluation` of an anonymous function definition.
// port: tsc/internal/transformers/estransforms/namedevaluation.go:isNamedEvaluation
pub fn is_named_evaluation(
    emit_context: &EmitContext,
    factory: &dyn RuntimeFactory,
    node: NodeId,
) -> bool {
    is_named_evaluation_and(emit_context, factory, node, None)
}

/// [`is_named_evaluation`] whose anonymous function definition `cb` also
/// accepts.
// port: tsc/internal/transformers/estransforms/namedevaluation.go:isNamedEvaluationAnd
pub fn is_named_evaluation_and(
    emit_context: &EmitContext,
    factory: &dyn RuntimeFactory,
    node: NodeId,
    cb: Option<AnonymousFunctionDefinitionCallback<'_>>,
) -> bool {
    if !is_named_evaluation_source(factory, node) {
        return false;
    }
    let read = factory.node(node);
    let expression = match read.kind().known() {
        Some(K::ShorthandPropertyAssignment) => read
            .as_shorthand_property_assignment()
            .expect("ShorthandPropertyAssignment payload")
            .object_assignment_initializer(),
        Some(
            K::PropertyAssignment
            | K::VariableDeclaration
            | K::Parameter
            | K::BindingElement
            | K::PropertyDeclaration,
        ) => read.initializer(),
        Some(K::BinaryExpression) => read
            .as_binary_expression()
            .expect("BinaryExpression payload")
            .right(),
        Some(K::ExportAssignment) => read.expression(),
        _ => panic!("Debug failure. Unhandled case in isNamedEvaluation"),
    };
    drop(read);
    is_anonymous_function_definition(emit_context, factory, expression, cb)
}

fn new_string_literal(factory: &mut dyn RuntimeFactory, text: &[u8]) -> NodeId {
    factory.new_string_literal(JsString::from_bytes(text), token_flags::NONE)
}

/// Gets a string literal to use as the assigned name of an anonymous class or
/// function declaration.
// port: tsc/internal/transformers/estransforms/namedevaluation.go:getAssignedNameOfIdentifier
pub(crate) fn get_assigned_name_of_identifier(
    emit_context: &EmitContext,
    factory: &mut dyn RuntimeFactory,
    name: Option<NodeId>,
    expression: Option<NodeId>,
) -> NodeId {
    let original =
        emit_context.most_original(skip_outer_expressions_all(factory, expression.expect(NIL)));
    let read = factory.node(original);
    if (read.kind() == K::ClassDeclaration || read.kind() == K::FunctionDeclaration)
        && read.name().is_none()
        && has_syntactic_modifier(factory, original, modifier_flags::DEFAULT)
    {
        drop(read);
        return new_string_literal(factory, b"default");
    }
    drop(read);
    emit_context
        .clone()
        .new_string_literal_from_node(factory, name.expect(NIL))
}

/// The assigned name of a property name, and the name to use in its place: a
/// computed name is rewritten to capture its key in a hoisted variable.
// port: tsc/internal/transformers/estransforms/namedevaluation.go:getAssignedNameOfPropertyName
pub(crate) fn get_assigned_name_of_property_name(
    emit_context: &EmitContext,
    factory: &mut dyn RuntimeFactory,
    name: Option<NodeId>,
    assigned_name_text: &[u8],
) -> (NodeId, Option<NodeId>) {
    let mut ec = emit_context.clone();
    if !assigned_name_text.is_empty() {
        let assigned_name = new_string_literal(factory, assigned_name_text);
        return (assigned_name, name);
    }

    let name_id = name.expect(NIL);
    let read = factory.node(name_id);
    if is_property_name_literal(&read) || read.kind() == K::PrivateIdentifier {
        drop(read);
        let assigned_name = ec.new_string_literal_from_node(factory, name_id);
        return (assigned_name, name);
    }

    let expression = read.expression().expect(NIL);
    let name_kind = read.kind();
    drop(read);
    let expression_read = factory.node(expression);
    if is_property_name_literal(&expression_read) && expression_read.kind() != K::Identifier {
        drop(expression_read);
        let assigned_name = ec.new_string_literal_from_node(factory, expression);
        return (assigned_name, name);
    }
    drop(expression_read);

    assert!(
        name_kind == K::ComputedPropertyName,
        "Debug failure. False expression: Expected computed property name"
    );

    let assigned_name = ec.new_generated_name_for_node(factory, name_id);
    ec.add_variable_declaration(factory, assigned_name);

    let key = ec.new_prop_key_helper(factory, expression);
    let assignment = ec.new_assignment_expression(factory, assigned_name, key);
    let updated_name = factory.update_computed_property_name(name_id, Some(assignment));
    (assigned_name, Some(updated_name))
}

/// Creates a class `static {}` block used to dynamically set the name of a
/// class.
///
/// `assigned_name` is the expression used to resolve the assigned name at
/// runtime; it should not produce side effects. `this_expression` overrides
/// the expression to use for the actual `this` reference. This can be used to
/// provide an expression that has already had its `EmitFlags` set or may have
/// been tracked to prevent substitution.
// port: tsc/internal/transformers/estransforms/namedevaluation.go:createClassNamedEvaluationHelperBlock
pub fn create_class_named_evaluation_helper_block(
    emit_context: &EmitContext,
    factory: &mut dyn RuntimeFactory,
    assigned_name: NodeId,
    this_expression: Option<NodeId>,
) -> NodeId {
    // produces:
    //
    //  static { __setFunctionName(this, "C"); }
    //

    let this_expression =
        this_expression.unwrap_or_else(|| emit_context.new_this_expression(factory));

    let mut ec = emit_context.clone();
    let expression = ec.new_set_function_name_helper(
        factory,
        this_expression,
        assigned_name,
        b"", /*prefix*/
    );
    let statement = factory.new_expression_statement(Some(expression));
    let statements = new_node_list(factory, vec![statement]);
    let body = factory.new_block(Some(statements), false /*multiLine*/);
    let block = factory.new_class_static_block_declaration(None /*modifiers*/, Some(body));

    // We use `emitNode.assignedName` to indicate this is a NamedEvaluation helper block
    // and to stash the expression used to resolve the assigned name.
    ec.set_assigned_name(block, assigned_name);
    block
}

/// Injects a class `static {}` block used to dynamically set the name of a
/// class, if one does not already exist.
// port: tsc/internal/transformers/estransforms/namedevaluation.go:injectClassNamedEvaluationHelperBlockIfMissing
pub fn inject_class_named_evaluation_helper_block_if_missing(
    emit_context: &EmitContext,
    factory: &mut dyn RuntimeFactory,
    node: NodeId,
    assigned_name: NodeId,
    this_expression: Option<NodeId>,
) -> NodeId {
    // given:
    //
    //  let C = class {
    //  };
    //
    // produces:
    //
    //  let C = class {
    //      static { __setFunctionName(this, "C"); }
    //  };

    // NOTE: If the class has a `_classThis` assignment block, this helper will be injected after that block.

    if class_has_explicitly_assigned_name(emit_context, factory, node) {
        return node;
    }

    let mut ec = emit_context.clone();
    let named_evaluation_block = create_class_named_evaluation_helper_block(
        emit_context,
        factory,
        assigned_name,
        this_expression,
    );
    if let Some(name) = factory.node(node).name() {
        // Upstream reads `namedEvaluationBlock.Body()`, which is nil for a
        // static block (it does not embed `BodyBase`): a named class panics
        // here at the pin.
        let body = factory.node(named_evaluation_block).body().expect(NIL);
        let statement = list_nodes(factory, factory.node(body).statement_list())[0];
        let loc = factory.node(name).range();
        ec.set_source_map_range(statement, loc);
    }

    let members = list_nodes(factory, factory.node(node).member_list());
    let insertion_index = members
        .iter()
        .position(|&member| is_class_this_assignment_block(emit_context, factory, member))
        .map_or(0, |index| index + 1);
    let mut updated_members = members[..insertion_index].to_vec();
    updated_members.push(named_evaluation_block);
    updated_members.extend_from_slice(&members[insertion_index..]);
    let members_list = new_node_list(factory, updated_members);
    // `node.MemberList().Loc`: a nil list is a nil dereference.
    let member_list = factory.node(node).member_list().expect(NIL);
    let loc = factory.read_list(member_list).loc();
    factory.set_list_location(members_list, loc);

    let old_node = node;
    let read = factory.node(node);
    let (kind, modifiers, name, type_parameters) = (
        read.kind(),
        read.modifiers(),
        read.name(),
        read.type_parameter_list(),
    );
    let node = if kind == K::ClassDeclaration {
        let heritage_clauses = read
            .as_class_declaration()
            .expect("ClassDeclaration payload")
            .heritage_clauses();
        drop(read);
        factory.update_class_declaration(
            node,
            modifiers,
            name,
            type_parameters,
            heritage_clauses,
            Some(members_list),
        )
    } else {
        let heritage_clauses = read
            .as_class_expression()
            .unwrap_or_else(|| {
                panic!(
                    "interface conversion: ast.nodeData is *ast.{}, not *ast.ClassExpression",
                    read.data_source().name()
                )
            })
            .heritage_clauses();
        drop(read);
        factory.update_class_expression(
            node,
            modifiers,
            name,
            type_parameters,
            heritage_clauses,
            Some(members_list),
        )
    };

    ec.set_assigned_name(node, assigned_name);

    // Transfer ClassThis from old to new node, since UpdateClassExpression creates
    // a new node that won't have ClassThis set on it.
    if let Some(class_this) = ec.class_this(old_node) {
        ec.set_class_this(node, class_this);
    }

    node
}

/// Assigns `assigned_name` to the anonymous function definition `expression`
/// wraps (through its outer expressions); an empty string literal is left
/// unassigned when `ignore_empty_string_literal`.
// port: tsc/internal/transformers/estransforms/namedevaluation.go:finishTransformNamedEvaluation
pub(crate) fn finish_transform_named_evaluation(
    emit_context: &EmitContext,
    factory: &mut dyn RuntimeFactory,
    expression: Option<NodeId>,
    assigned_name: NodeId,
    ignore_empty_string_literal: bool,
) -> Option<NodeId> {
    if ignore_empty_string_literal {
        let read = factory.node(assigned_name);
        if let Some(literal) = read.as_string_literal() {
            if literal.text().is_empty() {
                return expression;
            }
        }
    }

    let expression = expression.expect(NIL);
    let inner_expression = skip_outer_expressions_all(factory, expression);

    let updated_expression = if factory.node(inner_expression).kind() == K::ClassExpression {
        inject_class_named_evaluation_helper_block_if_missing(
            emit_context,
            factory,
            inner_expression,
            assigned_name,
            None, /*thisExpression*/
        )
    } else {
        emit_context.clone().new_set_function_name_helper(
            factory,
            inner_expression,
            assigned_name,
            b"", /*prefix*/
        )
    };

    Some(restore_outer_expressions_all(
        emit_context,
        factory,
        Some(expression),
        updated_expression,
    ))
}

/// The assigned name text given by the caller, or the one an identifier gives.
fn assigned_name_or_identifier(
    emit_context: &EmitContext,
    factory: &mut dyn RuntimeFactory,
    assigned_name_text: &[u8],
    name: Option<NodeId>,
    expression: Option<NodeId>,
) -> NodeId {
    if assigned_name_text.is_empty() {
        get_assigned_name_of_identifier(emit_context, factory, name, expression)
    } else {
        new_string_literal(factory, assigned_name_text)
    }
}

// port: tsc/internal/transformers/estransforms/namedevaluation.go:transformNamedEvaluationOfPropertyAssignment
fn transform_named_evaluation_of_property_assignment(
    emit_context: &EmitContext,
    factory: &mut dyn RuntimeFactory,
    node: NodeId,
    ignore_empty_string_literal: bool,
    assigned_name_text: &[u8],
) -> NodeId {
    // 13.2.5.5 RS: PropertyDefinitionEvaluation
    //   PropertyAssignment : PropertyName `:` AssignmentExpression
    //     ...
    //     5. If IsAnonymousFunctionDefinition(|AssignmentExpression|) is *true* and _isProtoSetter_ is *false*, then
    //        a. Let _popValue_ be ? NamedEvaluation of |AssignmentExpression| with argument _propKey_.
    //     ...

    let read = factory.node(node);
    let (name, initializer) = (read.name(), read.initializer());
    drop(read);
    let (assigned_name, name) =
        get_assigned_name_of_property_name(emit_context, factory, name, assigned_name_text);
    let initializer = finish_transform_named_evaluation(
        emit_context,
        factory,
        initializer,
        assigned_name,
        ignore_empty_string_literal,
    );
    factory.update_property_assignment(
        node,
        None, /*modifiers*/
        name,
        None, /*postfixToken*/
        None, /*typeNode*/
        initializer,
    )
}

// port: tsc/internal/transformers/estransforms/namedevaluation.go:transformNamedEvaluationOfShorthandAssignmentProperty
fn transform_named_evaluation_of_shorthand_assignment_property(
    emit_context: &EmitContext,
    factory: &mut dyn RuntimeFactory,
    node: NodeId,
    ignore_empty_string_literal: bool,
    assigned_name_text: &[u8],
) -> NodeId {
    // 13.15.5.3 RS: PropertyDestructuringAssignmentEvaluation
    //   AssignmentProperty : IdentifierReference Initializer?
    //     ...
    //     4. If |Initializer?| is present and _v_ is *undefined*, then
    //        a. If IsAnonymousFunctionDefinition(|Initializer|) is *true*, then
    //           i. Set _v_ to ? NamedEvaluation of |Initializer| with argument _P_.
    //     ...

    let read = factory.node(node);
    let shorthand = read
        .as_shorthand_property_assignment()
        .expect("ShorthandPropertyAssignment payload");
    let (name, equals_token, object_assignment_initializer) = (
        read.name(),
        shorthand.equals_token(),
        shorthand.object_assignment_initializer(),
    );
    drop(read);
    let assigned_name = assigned_name_or_identifier(
        emit_context,
        factory,
        assigned_name_text,
        name,
        object_assignment_initializer,
    );
    let object_assignment_initializer = finish_transform_named_evaluation(
        emit_context,
        factory,
        object_assignment_initializer,
        assigned_name,
        ignore_empty_string_literal,
    );
    factory.update_shorthand_property_assignment(
        node,
        None, /*modifiers*/
        name,
        None, /*postfixToken*/
        None, /*typeNode*/
        equals_token,
        object_assignment_initializer,
    )
}

// port: tsc/internal/transformers/estransforms/namedevaluation.go:transformNamedEvaluationOfVariableDeclaration
fn transform_named_evaluation_of_variable_declaration(
    emit_context: &EmitContext,
    factory: &mut dyn RuntimeFactory,
    node: NodeId,
    ignore_empty_string_literal: bool,
    assigned_name_text: &[u8],
) -> NodeId {
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

    let read = factory.node(node);
    let (name, initializer) = (read.name(), read.initializer());
    drop(read);
    let assigned_name =
        assigned_name_or_identifier(emit_context, factory, assigned_name_text, name, initializer);
    let initializer = finish_transform_named_evaluation(
        emit_context,
        factory,
        initializer,
        assigned_name,
        ignore_empty_string_literal,
    );
    factory.update_variable_declaration(
        node,
        name,
        None, /*exclamationToken*/
        None, /*typeNode*/
        initializer,
    )
}

// port: tsc/internal/transformers/estransforms/namedevaluation.go:transformNamedEvaluationOfParameterDeclaration
fn transform_named_evaluation_of_parameter_declaration(
    emit_context: &EmitContext,
    factory: &mut dyn RuntimeFactory,
    node: NodeId,
    ignore_empty_string_literal: bool,
    assigned_name_text: &[u8],
) -> NodeId {
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

    let read = factory.node(node);
    let dot_dot_dot_token = read
        .as_parameter_declaration()
        .expect("ParameterDeclaration payload")
        .dot_dot_dot_token();
    let (name, initializer) = (read.name(), read.initializer());
    drop(read);
    let assigned_name =
        assigned_name_or_identifier(emit_context, factory, assigned_name_text, name, initializer);
    let initializer = finish_transform_named_evaluation(
        emit_context,
        factory,
        initializer,
        assigned_name,
        ignore_empty_string_literal,
    );
    factory.update_parameter_declaration(
        node,
        None, /*modifiers*/
        dot_dot_dot_token,
        name,
        None, /*questionToken*/
        None, /*typeNode*/
        initializer,
    )
}

// port: tsc/internal/transformers/estransforms/namedevaluation.go:transformNamedEvaluationOfBindingElement
fn transform_named_evaluation_of_binding_element(
    emit_context: &EmitContext,
    factory: &mut dyn RuntimeFactory,
    node: NodeId,
    ignore_empty_string_literal: bool,
    assigned_name_text: &[u8],
) -> NodeId {
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

    let read = factory.node(node);
    let dot_dot_dot_token = read
        .as_binding_element()
        .expect("BindingElement payload")
        .dot_dot_dot_token();
    let (property_name, name, initializer) =
        (read.property_name(), read.name(), read.initializer());
    drop(read);
    let assigned_name =
        assigned_name_or_identifier(emit_context, factory, assigned_name_text, name, initializer);
    let initializer = finish_transform_named_evaluation(
        emit_context,
        factory,
        initializer,
        assigned_name,
        ignore_empty_string_literal,
    );
    factory.update_binding_element(node, dot_dot_dot_token, property_name, name, initializer)
}

// port: tsc/internal/transformers/estransforms/namedevaluation.go:transformNamedEvaluationOfPropertyDeclaration
fn transform_named_evaluation_of_property_declaration(
    emit_context: &EmitContext,
    factory: &mut dyn RuntimeFactory,
    node: NodeId,
    ignore_empty_string_literal: bool,
    assigned_name_text: &[u8],
) -> NodeId {
    // 10.2.1.3 RS: EvaluateBody
    //   Initializer : `=` AssignmentExpression
    //     ...
    //     3. If IsAnonymousFunctionDefinition(|AssignmentExpression|) is *true*, then
    //        a. Let _value_ be ? NamedEvaluation of |Initializer| with argument _functionObject_.[[ClassFieldInitializerName]].
    //     ...

    let read = factory.node(node);
    let (name, initializer) = (read.name(), read.initializer());
    drop(read);
    let (assigned_name, name) =
        get_assigned_name_of_property_name(emit_context, factory, name, assigned_name_text);
    let initializer = finish_transform_named_evaluation(
        emit_context,
        factory,
        initializer,
        assigned_name,
        ignore_empty_string_literal,
    );
    let modifiers = factory.node(node).modifiers();
    factory.update_property_declaration(
        node,
        modifiers,
        name,
        None, /*postfixToken*/
        None, /*typeNode*/
        initializer,
    )
}

// port: tsc/internal/transformers/estransforms/namedevaluation.go:transformNamedEvaluationOfAssignmentExpression
fn transform_named_evaluation_of_assignment_expression(
    emit_context: &EmitContext,
    factory: &mut dyn RuntimeFactory,
    node: NodeId,
    ignore_empty_string_literal: bool,
    assigned_name_text: &[u8],
) -> NodeId {
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

    let read = factory.node(node);
    let binary = read
        .as_binary_expression()
        .expect("BinaryExpression payload");
    let (left, operator_token, right) = (binary.left(), binary.operator_token(), binary.right());
    drop(read);
    let assigned_name =
        assigned_name_or_identifier(emit_context, factory, assigned_name_text, left, right);
    let right = finish_transform_named_evaluation(
        emit_context,
        factory,
        right,
        assigned_name,
        ignore_empty_string_literal,
    );
    factory.update_binary_expression(
        node,
        None, /*modifiers*/
        left,
        None, /*typeNode*/
        operator_token,
        right,
    )
}

// port: tsc/internal/transformers/estransforms/namedevaluation.go:transformNamedEvaluationOfExportAssignment
fn transform_named_evaluation_of_export_assignment(
    emit_context: &EmitContext,
    factory: &mut dyn RuntimeFactory,
    node: NodeId,
    ignore_empty_string_literal: bool,
    assigned_name_text: &[u8],
) -> NodeId {
    // 16.2.3.7 RS: Evaluation
    //   ExportDeclaration : `export` `default` AssignmentExpression `;`
    //     1. If IsAnonymousFunctionDefinition(|AssignmentExpression|) is *true*, then
    //        a. Let _value_ be ? NamedEvaluation of |AssignmentExpression| with argument `"default"`.
    //     ...

    // NOTE: Since emit for `export =` translates to `module.exports = ...`, the assigned name of the class or function
    // is `""`.

    let read = factory.node(node);
    let is_export_equals = read
        .as_export_assignment()
        .expect("ExportAssignment payload")
        .is_export_equals();
    let expression = read.expression();
    drop(read);
    let assigned_name = if !assigned_name_text.is_empty() {
        new_string_literal(factory, assigned_name_text)
    } else if is_export_equals {
        new_string_literal(factory, b"")
    } else {
        new_string_literal(factory, b"default")
    };
    let expression = finish_transform_named_evaluation(
        emit_context,
        factory,
        expression,
        assigned_name,
        ignore_empty_string_literal,
    );
    factory.update_export_assignment(
        node,
        None, /*modifiers*/
        is_export_equals,
        None, /*typeNode*/
        expression,
    )
}

/// Performs a shallow transformation of a `NamedEvaluation` node, such that a
/// valid name will be assigned. An empty `assigned_name` derives the name
/// from the node.
// port: tsc/internal/transformers/estransforms/namedevaluation.go:transformNamedEvaluation
pub fn transform_named_evaluation(
    emit_context: &EmitContext,
    factory: &mut dyn RuntimeFactory,
    node: NodeId,
    ignore_empty_string_literal: bool,
    assigned_name: &[u8],
) -> NodeId {
    let transform = match factory.node(node).kind().known() {
        Some(K::PropertyAssignment) => transform_named_evaluation_of_property_assignment,
        Some(K::ShorthandPropertyAssignment) => {
            transform_named_evaluation_of_shorthand_assignment_property
        }
        Some(K::VariableDeclaration) => transform_named_evaluation_of_variable_declaration,
        Some(K::Parameter) => transform_named_evaluation_of_parameter_declaration,
        Some(K::BindingElement) => transform_named_evaluation_of_binding_element,
        Some(K::PropertyDeclaration) => transform_named_evaluation_of_property_declaration,
        Some(K::BinaryExpression) => transform_named_evaluation_of_assignment_expression,
        Some(K::ExportAssignment) => transform_named_evaluation_of_export_assignment,
        _ => panic!("Debug failure. Unhandled case in transformNamedEvaluation"),
    };
    transform(
        emit_context,
        factory,
        node,
        ignore_empty_string_literal,
        assigned_name,
    )
}
