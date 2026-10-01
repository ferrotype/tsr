//! `transformers/utilities.go`: the emit-name predicates, identifier-reference
//! classification, binding-pattern to assignment-pattern conversion, the
//! simple-expression predicates, the `super()` statement search, the
//! modifier-skipping ranges and the compound-assignment operator table.
//!
//! Functions take the emit context and the factory a transformer holds
//! (`visitor.factory()` / `visitor.factory_mut()`). The few `ast` helpers
//! whose `tsr_ast` ports read through an `AstView` (`GetSourceFileOfNode`,
//! `SkipParentheses`, `IsSuperCall`, `Node.ModifierNodes`, `Node.Arguments`,
//! `Node.Statements`) are re-expressed here over `Factory` reads; they carry no
//! port marker.
use crate::Error;
use tsr_ast::{
    utilities as ast, Factory, FactoryMethods, NodeId, NodeKind, NodeListId, NodeRead,
    RuntimeFactory, SyntaxKind as K,
};
use tsr_core::TextRange;
use tsr_printer::{emit_flags, EmitContext};

pub(crate) const NIL: &str = "runtime error: invalid memory address or nil pointer dereference";

// port: tsc/internal/transformers/utilities.go:IsGeneratedIdentifier
pub fn is_generated_identifier(emit_context: &EmitContext, name: NodeId) -> bool {
    emit_context.has_auto_generate_info(name)
}

// port: tsc/internal/transformers/utilities.go:IsHelperName
pub fn is_helper_name(emit_context: &EmitContext, name: NodeId) -> bool {
    emit_context.emit_flags(name) & emit_flags::HELPER_NAME != 0
}

// port: tsc/internal/transformers/utilities.go:IsLocalName
pub fn is_local_name(emit_context: &EmitContext, name: NodeId) -> bool {
    emit_context.emit_flags(name) & emit_flags::LOCAL_NAME != 0
}

// port: tsc/internal/transformers/utilities.go:IsExportName
pub fn is_export_name(emit_context: &EmitContext, name: NodeId) -> bool {
    emit_context.emit_flags(name) & emit_flags::EXPORT_NAME != 0
}

/// The nodes of `list`, Go's `NodeList.Nodes` (nil elements kept).
pub(crate) fn list_nodes(factory: &dyn RuntimeFactory, list: NodeListId) -> Vec<Option<NodeId>> {
    let nodes = factory.read_list(list).nodes();
    factory.read_nodes(nodes).iter().collect()
}

/// Upstream's `n.data.(*ast.X)` on a node of another kind.
pub(crate) fn interface_conversion(read: &NodeRead<'_>, expected: &str) -> ! {
    panic!(
        "interface conversion: ast.nodeData is *ast.{}, not *ast.{expected}",
        read.data_source().name()
    )
}

/// Whether `name`, an identifier whose parent is `parent`, is an
/// `IdentifierReference` there: an expression position rather than a
/// declaration name, property name or label.
// port: tsc/internal/transformers/utilities.go:IsIdentifierReference
pub fn is_identifier_reference(factory: &dyn RuntimeFactory, name: NodeId, parent: NodeId) -> bool {
    let read = factory.node(parent);
    let name = Some(name);
    match read.kind().known() {
        Some(
            K::BinaryExpression
            | K::PrefixUnaryExpression
            | K::PostfixUnaryExpression
            | K::YieldExpression
            | K::AsExpression
            | K::SatisfiesExpression
            | K::ElementAccessExpression
            | K::NonNullExpression
            | K::SpreadElement
            | K::SpreadAssignment
            | K::ParenthesizedExpression
            | K::ArrayLiteralExpression
            | K::DeleteExpression
            | K::TypeOfExpression
            | K::VoidExpression
            | K::AwaitExpression
            | K::TypeAssertionExpression
            | K::ExpressionWithTypeArguments
            | K::JsxSelfClosingElement
            | K::JsxSpreadAttribute
            | K::JsxExpression
            | K::PartiallyEmittedExpression,
        ) => {
            // all immediate children that can be `Identifier` would be instances of `IdentifierReference`
            true
        }
        Some(
            K::ComputedPropertyName
            | K::Decorator
            | K::IfStatement
            | K::DoStatement
            | K::WhileStatement
            | K::WithStatement
            | K::ReturnStatement
            | K::SwitchStatement
            | K::CaseClause
            | K::ThrowStatement
            | K::ExpressionStatement
            | K::ExportAssignment
            | K::PropertyAccessExpression
            | K::TemplateSpan,
        ) => {
            // only an `Expression()` child that can be `Identifier` would be an instance of `IdentifierReference`
            read.expression() == name
        }
        Some(
            K::VariableDeclaration
            | K::Parameter
            | K::BindingElement
            | K::PropertyDeclaration
            | K::PropertySignature
            | K::PropertyAssignment
            | K::EnumMember
            | K::JsxAttribute,
        ) => {
            // only an `Initializer()` child that can be `Identifier` would be an instance of `IdentifierReference`
            read.initializer() == name
        }
        Some(K::ShorthandPropertyAssignment) => {
            read.as_shorthand_property_assignment()
                .unwrap_or_else(|| interface_conversion(&read, "ShorthandPropertyAssignment"))
                .object_assignment_initializer()
                == name
        }
        Some(K::ForStatement) => {
            let data = read
                .as_for_statement()
                .unwrap_or_else(|| interface_conversion(&read, "ForStatement"));
            read.initializer() == name || data.condition() == name || data.incrementor() == name
        }
        Some(K::ForInStatement | K::ForOfStatement) => {
            read.initializer() == name || read.expression() == name
        }
        Some(K::ImportEqualsDeclaration) => {
            read.as_import_equals_declaration()
                .unwrap_or_else(|| interface_conversion(&read, "ImportEqualsDeclaration"))
                .module_reference()
                == name
        }
        Some(K::ArrowFunction) => read.body() == name,
        Some(K::ConditionalExpression) => {
            let data = read
                .as_conditional_expression()
                .unwrap_or_else(|| interface_conversion(&read, "ConditionalExpression"));
            data.condition() == name || data.when_true() == name || data.when_false() == name
        }
        Some(K::CallExpression | K::NewExpression) => {
            read.expression() == name
                || read
                    .argument_list()
                    .is_some_and(|list| list_nodes(factory, list).contains(&name))
        }
        Some(K::TaggedTemplateExpression) => {
            read.as_tagged_template_expression()
                .unwrap_or_else(|| interface_conversion(&read, "TaggedTemplateExpression"))
                .tag()
                == name
        }
        Some(K::ImportAttribute) => {
            read.as_import_attribute()
                .unwrap_or_else(|| interface_conversion(&read, "ImportAttribute"))
                .value()
                == name
        }
        Some(K::JsxOpeningElement | K::JsxClosingElement) => read.tag_name() == name,
        _ => false,
    }
}

/// The fields of `element.AsBindingElement()`.
struct BindingElementFields {
    dot_dot_dot_token: Option<NodeId>,
    property_name: Option<NodeId>,
    name: Option<NodeId>,
    initializer: Option<NodeId>,
}

impl BindingElementFields {
    fn read(factory: &dyn Factory, element: NodeId) -> Self {
        let read = factory.node(element);
        let data = read
            .as_binding_element()
            .unwrap_or_else(|| interface_conversion(&read, "BindingElement"));
        Self {
            dot_dot_dot_token: data.dot_dot_dot_token(),
            property_name: data.property_name(),
            name: data.name(),
            initializer: data.initializer(),
        }
    }
}

/// `SetOriginal` and `AssignCommentAndSourceMapRanges` from `original`, the
/// pair every conversion below applies to its result.
fn set_original_and_ranges(
    emit_context: &EmitContext,
    factory: &dyn Factory,
    node: NodeId,
    original: NodeId,
) {
    let mut ec = emit_context.clone();
    ec.set_original(node, original);
    ec.assign_comment_and_source_map_ranges(factory, node, original);
}

// port: tsc/internal/transformers/utilities.go:convertBindingElementToArrayAssignmentElement
fn convert_binding_element_to_array_assignment_element(
    emit_context: &EmitContext,
    factory: &mut dyn RuntimeFactory,
    element: NodeId,
) -> NodeId {
    let fields = BindingElementFields::read(factory, element);
    let Some(name) = fields.name else {
        let elision = factory.new_omitted_expression();
        set_original_and_ranges(emit_context, factory, elision, element);
        return elision;
    };
    if fields.dot_dot_dot_token.is_some() {
        let spread = factory.new_spread_element(Some(name));
        set_original_and_ranges(emit_context, factory, spread, element);
        return spread;
    }
    let expression = convert_binding_name_to_assignment_element_target(emit_context, factory, name);
    if let Some(initializer) = fields.initializer {
        let assignment = emit_context.new_assignment_expression(factory, expression, initializer);
        set_original_and_ranges(emit_context, factory, assignment, element);
        return assignment;
    }
    expression
}

// port: tsc/internal/transformers/utilities.go:convertBindingElementToObjectAssignmentElement
fn convert_binding_element_to_object_assignment_element(
    emit_context: &EmitContext,
    factory: &mut dyn RuntimeFactory,
    element: NodeId,
) -> NodeId {
    let fields = BindingElementFields::read(factory, element);
    if fields.dot_dot_dot_token.is_some() {
        let spread = factory.new_spread_assignment(fields.name);
        set_original_and_ranges(emit_context, factory, spread, element);
        return spread;
    }
    if let Some(property_name) = fields.property_name {
        let mut expression = convert_binding_name_to_assignment_element_target(
            emit_context,
            factory,
            fields.name.expect(NIL),
        );
        if let Some(initializer) = fields.initializer {
            expression = emit_context.new_assignment_expression(factory, expression, initializer);
        }
        let assignment = factory.new_property_assignment(
            None, /*modifiers*/
            Some(property_name),
            None, /*postfixToken*/
            None, /*typeNode*/
            Some(expression),
        );
        set_original_and_ranges(emit_context, factory, assignment, element);
        return assignment;
    }
    let equals_token = fields
        .initializer
        .map(|_| factory.new_token(K::EqualsToken.into()));
    let assignment = factory.new_shorthand_property_assignment(
        None, /*modifiers*/
        fields.name,
        None, /*postfixToken*/
        None, /*typeNode*/
        equals_token,
        fields.initializer,
    );
    set_original_and_ranges(emit_context, factory, assignment, element);
    assignment
}

/// Converts an array or object binding pattern into the equivalent array or
/// object literal assignment pattern; each result keeps its binding node as
/// its original and takes that node's comment and source-map ranges.
// port: tsc/internal/transformers/utilities.go:ConvertBindingPatternToAssignmentPattern
pub fn convert_binding_pattern_to_assignment_pattern(
    emit_context: &EmitContext,
    factory: &mut dyn RuntimeFactory,
    element: NodeId,
) -> NodeId {
    match factory.node(element).kind().known() {
        Some(K::ArrayBindingPattern) => {
            convert_binding_element_to_array_assignment_pattern(emit_context, factory, element)
        }
        Some(K::ObjectBindingPattern) => {
            convert_binding_element_to_object_assignment_pattern(emit_context, factory, element)
        }
        _ => panic!("Unknown binding pattern"),
    }
}

/// `element.Elements`: the pattern's list location and its nodes.
fn binding_pattern_elements(
    factory: &dyn RuntimeFactory,
    element: NodeId,
) -> (TextRange, Vec<Option<NodeId>>) {
    let read = factory.node(element);
    let elements = read
        .as_binding_pattern()
        .unwrap_or_else(|| interface_conversion(&read, "BindingPattern"))
        .elements()
        .expect(NIL);
    (
        factory.read_list(elements).loc(),
        list_nodes(factory, elements),
    )
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

// port: tsc/internal/transformers/utilities.go:convertBindingElementToObjectAssignmentPattern
fn convert_binding_element_to_object_assignment_pattern(
    emit_context: &EmitContext,
    factory: &mut dyn RuntimeFactory,
    element: NodeId,
) -> NodeId {
    let (loc, elements) = binding_pattern_elements(factory, element);
    let mut properties = Vec::with_capacity(elements.len());
    for binding_element in elements {
        properties.push(convert_binding_element_to_object_assignment_element(
            emit_context,
            factory,
            binding_element.expect(NIL),
        ));
    }
    let property_list = new_node_list_at(factory, properties, loc);
    let object =
        factory.new_object_literal_expression(Some(property_list), false /*multiLine*/);
    set_original_and_ranges(emit_context, factory, object, element);
    object
}

// port: tsc/internal/transformers/utilities.go:convertBindingElementToArrayAssignmentPattern
fn convert_binding_element_to_array_assignment_pattern(
    emit_context: &EmitContext,
    factory: &mut dyn RuntimeFactory,
    element: NodeId,
) -> NodeId {
    let (loc, binding_elements) = binding_pattern_elements(factory, element);
    let mut elements = Vec::with_capacity(binding_elements.len());
    for binding_element in binding_elements {
        elements.push(convert_binding_element_to_array_assignment_element(
            emit_context,
            factory,
            binding_element.expect(NIL),
        ));
    }
    let element_list = new_node_list_at(factory, elements, loc);
    let object = factory.new_array_literal_expression(Some(element_list), false /*multiLine*/);
    set_original_and_ranges(emit_context, factory, object, element);
    object
}

// port: tsc/internal/transformers/utilities.go:convertBindingNameToAssignmentElementTarget
fn convert_binding_name_to_assignment_element_target(
    emit_context: &EmitContext,
    factory: &mut dyn RuntimeFactory,
    element: NodeId,
) -> NodeId {
    if ast::is_binding_pattern(&factory.node(element)) {
        return convert_binding_pattern_to_assignment_pattern(emit_context, factory, element);
    }
    element
}

/// `name = initializer` for a variable declaration, its binding pattern
/// converted to an assignment pattern; `None` without an initializer.
// port: tsc/internal/transformers/utilities.go:ConvertVariableDeclarationToAssignmentExpression
pub fn convert_variable_declaration_to_assignment_expression(
    emit_context: &EmitContext,
    factory: &mut dyn RuntimeFactory,
    element: NodeId,
) -> Option<NodeId> {
    let (name, initializer) = {
        let read = factory.node(element);
        let data = read
            .as_variable_declaration()
            .unwrap_or_else(|| interface_conversion(&read, "VariableDeclaration"));
        (data.name(), data.initializer())
    };
    let initializer = initializer?;
    let expression =
        convert_binding_name_to_assignment_element_target(emit_context, factory, name.expect(NIL));
    let assignment = emit_context.new_assignment_expression(factory, expression, initializer);
    set_original_and_ranges(emit_context, factory, assignment, element);
    Some(assignment)
}

/// `nodes` as one node: nil for a nil slice, the node itself for one node,
/// otherwise a `SyntaxList` (also for an allocated empty slice).
// port: tsc/internal/transformers/utilities.go:SingleOrMany
pub fn single_or_many(
    factory: &mut dyn RuntimeFactory,
    nodes: Option<&[NodeId]>,
) -> Option<NodeId> {
    let nodes = nodes?;
    if nodes.len() == 1 {
        return Some(nodes[0]);
    }
    let children = factory.alloc_nodes(nodes.iter().copied().map(Some).collect());
    Some(factory.new_syntax_list(children))
}

/// Used in the module transformer to check if an expression is reasonably
/// without sideeffect, and thus better to copy into multiple places rather
/// than to cache in a temporary variable - this is mostly subjective beyond
/// the requirement that the expression not be sideeffecting.
///
/// Also used by the logical assignment downleveling transform to skip temp
/// variables when they're not needed.
// port: tsc/internal/transformers/utilities.go:IsSimpleCopiableExpression
pub fn is_simple_copiable_expression(factory: &dyn Factory, expression: NodeId) -> bool {
    let read = factory.node(expression);
    ast::is_string_literal_like(&read)
        || tsr_ast::is_numeric_literal(&read)
        || tsr_ast::is_keyword_kind(read.kind())
        || tsr_ast::is_identifier(&read)
}

/// `ast.GetSourceFileOfNode` over factory reads.
fn get_source_file_of_node(factory: &dyn Factory, mut node: Option<NodeId>) -> Option<NodeId> {
    while let Some(id) = node {
        let read = factory.node(id);
        if read.kind() == K::SourceFile {
            return Some(id);
        }
        node = read.parent();
    }
    None
}

/// Whether the most original node of `node` starts and ends on one line of
/// its source file. A storage failure reading that file is the error.
// port: tsc/internal/transformers/utilities.go:IsOriginalNodeSingleLine
pub fn is_original_node_single_line(
    emit_context: &EmitContext,
    factory: &dyn Factory,
    node: Option<NodeId>,
) -> Result<bool, Error> {
    let Some(node) = node else {
        return Ok(false);
    };
    // Upstream also returns false for a nil most-original node, which a
    // non-nil node never has.
    let original = emit_context.most_original(node);
    let Some(source) = get_source_file_of_node(factory, Some(original)) else {
        return Ok(false);
    };
    let (pos, end) = {
        let read = factory.node(original);
        (read.pos(), read.end())
    };
    // `scanner.GetECMALineOfPosition` over the file's cached ECMA line map.
    let source = factory.read_source_file(source)?;
    let line_map = source.ecma_line_map();
    let start_line =
        tsr_jsstring::scanner_positions::compute_line_of_position(line_map, pos as isize);
    let end_line =
        tsr_jsstring::scanner_positions::compute_line_of_position(line_map, end as isize);
    Ok(start_line == end_line)
}

/// A simple inlinable expression is an expression which can be copied into
/// multiple locations without risk of repeating any sideeffects and whose
/// value could not possibly change between any such locations.
// port: tsc/internal/transformers/utilities.go:IsSimpleInlineableExpression
pub fn is_simple_inlineable_expression(factory: &dyn Factory, expression: NodeId) -> bool {
    !tsr_ast::is_identifier(&factory.node(expression))
        && is_simple_copiable_expression(factory, expression)
}

/// Finds a path of indices to a statement containing a `super()` call: the
/// index in `statements` first, then the index inside each enclosing `try`
/// block. Empty when there is none (upstream's nil path).
// port: tsc/internal/transformers/utilities.go:FindSuperStatementIndexPath
pub fn find_super_statement_index_path(
    factory: &dyn RuntimeFactory,
    statements: &[NodeId],
    start: usize,
) -> Vec<usize> {
    let statements: Vec<Option<NodeId>> = statements.iter().copied().map(Some).collect();
    let mut indices = find_super_statement_index_path_worker(factory, &statements, start, &[])
        .unwrap_or_default();
    indices.reverse();
    indices
}

// port: tsc/internal/transformers/utilities.go:findSuperStatementIndexPathWorker
fn find_super_statement_index_path_worker(
    factory: &dyn RuntimeFactory,
    statements: &[Option<NodeId>],
    start: usize,
    indices: &[usize],
) -> Option<Vec<usize>> {
    for (i, &statement) in statements.iter().enumerate().skip(start) {
        let statement = statement.expect(NIL);
        if get_super_call_from_statement(factory, statement).is_some() {
            let mut path = indices.to_vec();
            path.push(i);
            return Some(path);
        } else if tsr_ast::is_try_statement(&factory.node(statement)) {
            let try_block = {
                let read = factory.node(statement);
                read.as_try_statement()
                    .unwrap_or_else(|| interface_conversion(&read, "TryStatement"))
                    .try_block()
                    .expect(NIL)
            };
            // `Node.Statements()`: nil without a statement list.
            let block_statements = factory
                .node(try_block)
                .statement_list()
                .map(|list| list_nodes(factory, list))
                .unwrap_or_default();
            if let Some(mut result) =
                find_super_statement_index_path_worker(factory, &block_statements, 0, indices)
            {
                result.push(i);
                return Some(result);
            }
        }
    }
    None
}

/// `ast.SkipParentheses` over factory reads.
fn skip_parentheses(factory: &dyn Factory, mut node: NodeId) -> NodeId {
    loop {
        let read = factory.node(node);
        if read.kind() != K::ParenthesizedExpression {
            return node;
        }
        node = read.expression().expect(NIL);
    }
}

/// Extracts the `super()` call expression from an expression statement, if any.
// port: tsc/internal/transformers/utilities.go:GetSuperCallFromStatement
pub fn get_super_call_from_statement(factory: &dyn Factory, statement: NodeId) -> Option<NodeId> {
    let read = factory.node(statement);
    if !tsr_ast::is_expression_statement(&read) {
        return None;
    }
    let expression = skip_parentheses(factory, read.expression().expect(NIL));
    // `ast.IsSuperCall`.
    let call = factory.node(expression);
    if call.kind() == K::CallExpression
        && factory.node(call.expression().expect(NIL)).kind() == K::SuperKeyword
    {
        return Some(expression);
    }
    None
}

/// `node.ModifierNodes()`: the nodes of the modifier list, nil without one.
fn modifier_nodes(factory: &dyn RuntimeFactory, node: NodeId) -> Option<Vec<Option<NodeId>>> {
    let modifiers = factory.node(node).modifiers();
    modifiers.map(|list| list_nodes(factory, list))
}

/// Returns a text range that starts past any modifiers on the node.
// port: tsc/internal/transformers/utilities.go:MoveRangePastModifiers
pub fn move_range_past_modifiers(factory: &dyn RuntimeFactory, node: NodeId) -> TextRange {
    let read = factory.node(node);
    if tsr_ast::is_property_declaration(&read) || tsr_ast::is_method_declaration(&read) {
        let name = factory.node(read.name().expect(NIL));
        return TextRange::new(i64::from(name.pos()), i64::from(read.end()));
    }

    let mut last_modifier = None;
    if ast::can_have_modifiers(&read) {
        // `core.LastOrNil(node.ModifierNodes())`.
        last_modifier = modifier_nodes(factory, node)
            .and_then(|nodes| nodes.last().copied())
            .flatten();
    }

    if let Some(last_modifier) = last_modifier {
        let last_end = factory.node(last_modifier).end();
        if !ast::position_is_synthesized(last_end as isize) {
            return TextRange::new(i64::from(last_end), i64::from(read.end()));
        }
    }
    move_range_past_decorators(factory, node)
}

/// Returns a text range that starts past any decorators on the node.
// port: tsc/internal/transformers/utilities.go:MoveRangePastDecorators
pub fn move_range_past_decorators(factory: &dyn RuntimeFactory, node: NodeId) -> TextRange {
    let read = factory.node(node);
    let mut last_decorator = None;
    if ast::can_have_modifiers(&read) {
        if let Some(nodes) = modifier_nodes(factory, node) {
            // `core.FindLast(nodes, ast.IsDecorator)`.
            last_decorator = nodes
                .into_iter()
                .rev()
                .find(|modifier| tsr_ast::is_decorator(&factory.node(modifier.expect(NIL))))
                .flatten();
        }
    }

    if let Some(last_decorator) = last_decorator {
        let last_end = factory.node(last_decorator).end();
        if !ast::position_is_synthesized(last_end as isize) {
            return TextRange::new(i64::from(last_end), i64::from(read.end()));
        }
    }
    read.range()
}

/// Returns the non-assignment operator of a compound assignment operator,
/// and any other kind unchanged.
// port: tsc/internal/transformers/utilities.go:GetNonAssignmentOperatorForCompoundAssignment
pub fn get_non_assignment_operator_for_compound_assignment(kind: NodeKind) -> NodeKind {
    let operator = match kind.known() {
        Some(K::PlusEqualsToken) => K::PlusToken,
        Some(K::MinusEqualsToken) => K::MinusToken,
        Some(K::AsteriskEqualsToken) => K::AsteriskToken,
        Some(K::AsteriskAsteriskEqualsToken) => K::AsteriskAsteriskToken,
        Some(K::SlashEqualsToken) => K::SlashToken,
        Some(K::PercentEqualsToken) => K::PercentToken,
        Some(K::LessThanLessThanEqualsToken) => K::LessThanLessThanToken,
        Some(K::GreaterThanGreaterThanEqualsToken) => K::GreaterThanGreaterThanToken,
        Some(K::GreaterThanGreaterThanGreaterThanEqualsToken) => {
            K::GreaterThanGreaterThanGreaterThanToken
        }
        Some(K::AmpersandEqualsToken) => K::AmpersandToken,
        Some(K::BarEqualsToken) => K::BarToken,
        Some(K::CaretEqualsToken) => K::CaretToken,
        Some(K::BarBarEqualsToken) => K::BarBarToken,
        Some(K::AmpersandAmpersandEqualsToken) => K::AmpersandAmpersandToken,
        Some(K::QuestionQuestionEqualsToken) => K::QuestionQuestionToken,
        _ => return kind,
    };
    operator.into()
}
