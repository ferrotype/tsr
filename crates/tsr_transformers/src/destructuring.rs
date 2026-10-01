//! `transformers/destructuring.go`: flattening destructuring assignments and
//! bindings into individual assignments or variable declarations.
//!
//! Upstream's flattener holds the transformer; here it holds the visitor the
//! caller's visit function received and a clone of the emit context. The
//! four mode callbacks upstream installs as function fields are a [`Mode`]
//! the flattener dispatches on. Subtree facts, the `ast` binding-pattern
//! utilities and the printer's `NewRestHelper` read through the factory's
//! builder ([`RuntimeFactory::ast_builder`]); a failed storage read is the
//! error, and a factory that is not a builder is a caller bug (a panic).
//!
//! Upstream's nil is `None`. A nil value can only reach a constructor here
//! when a visitor deletes an operand; the constructors keep it, and the places
//! upstream dereferences it panic with Go's message.
use crate::utilities::{
    interface_conversion, is_simple_copiable_expression, is_simple_inlineable_expression, NIL,
};
use crate::Error;
use tsr_ast::subtree_flags::{OBJECT_REST_OR_SPREAD, REST_OR_SPREAD};
use tsr_ast::utilities::{
    is_binding_pattern, is_literal_expression, is_property_name_literal,
    is_string_or_numeric_literal_like, node_is_synthesized,
};
use tsr_ast::utilities_middle::is_declaration_binding_element;
use tsr_ast::utilities_positions::try_get_property_name_of_binding_or_assignment_element;
use tsr_ast::utilities_tail::{
    get_rest_indicator_of_binding_or_assignment_element, is_empty_array_literal,
    is_empty_object_literal,
};
use tsr_ast::{
    node_flags, token_flags, AstBuilder, AstView, Factory, FactoryMethods, JsString, NodeId,
    NodeKind, NodeListId, NodeVisitor, RuntimeFactory, SyntaxKind as K,
};
use tsr_core::TextRange;
use tsr_printer::EmitContext;

const BUILDER: &str = "destructuring flattening reads through the transformer's AstBuilder";

/// Controls how deeply binding/assignment patterns are decomposed.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum FlattenLevel {
    /// Fully decompose all patterns into individual assignments/bindings.
    All,
    /// Only decompose patterns containing object rest elements.
    ObjectRest,
}

/// A callback used to create custom assignment expressions during
/// destructuring flattening. The target is always an Identifier, and the
/// callback can wrap the assignment with additional logic (e.g. export
/// expressions in CJS modules or namespace member assignments). It receives
/// the flattener's visitor, the name, the value and upstream's
/// `*core.TextRange` location, which the flattener always passes.
pub type CreateAssignmentCallback<'a> =
    dyn Fn(&mut NodeVisitor<'_>, NodeId, NodeId, Option<TextRange>) -> NodeId + 'a;

/// Flattens a destructuring assignment expression (or a variable declaration)
/// into a sequence of individual property/element access assignments.
/// Supports custom assignment callbacks for module export or namespace member
/// expressions. `None` only when the visitor deleted the assigned value.
// port: tsc/internal/transformers/destructuring.go:FlattenDestructuringAssignment
pub fn flatten_destructuring_assignment(
    visitor: &mut NodeVisitor<'_>,
    emit_context: &EmitContext,
    node: NodeId, // VariableDeclaration | DestructuringAssignment
    needs_value: bool,
    level: FlattenLevel,
    create_assignment_callback: Option<&CreateAssignmentCallback<'_>>,
) -> Result<Option<NodeId>, Error> {
    let mut f = Flattener::new(visitor, emit_context, level, Mode::Assignment);
    f.create_assignment_callback = create_assignment_callback;
    f.hoist_temp_variables = true;
    f.flatten_destructuring_assignment(node, needs_value)
}

/// A pending variable declaration during binding flattening.
struct PendingDecl {
    pending_expressions: Vec<NodeId>,
    name: NodeId,
    value: Option<NodeId>,
    location: TextRange,
    original: Option<NodeId>,
}

/// Flattens a binding pattern in a variable declaration or parameter into
/// individual variable declarations. Returns a single VariableDeclaration, a
/// SyntaxList of declarations, or `None`.
// port: tsc/internal/transformers/destructuring.go:FlattenDestructuringBinding
pub fn flatten_destructuring_binding(
    visitor: &mut NodeVisitor<'_>,
    emit_context: &EmitContext,
    node: NodeId, // VariableDeclaration | ParameterDeclaration | BindingElement
    rval: Option<NodeId>,
    level: FlattenLevel,
    hoist_temp_variables: bool,
    skip_initializer: bool,
) -> Result<Option<NodeId>, Error> {
    let mut f = Flattener::new(visitor, emit_context, level, Mode::Binding);
    f.hoist_temp_variables = hoist_temp_variables;
    f.flatten_destructuring_binding(node, rval, skip_initializer)
}

/// The mode callbacks: `emitBindingOrAssignment`,
/// `createArrayBindingOrAssignmentPattern`,
/// `createObjectBindingOrAssignmentPattern` and
/// `createArrayBindingOrAssignmentElement` of each entry point.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Mode {
    /// `FlattenDestructuringAssignment`'s callbacks.
    Assignment,
    /// `FlattenDestructuringBinding`'s callbacks.
    Binding,
}

/// Upstream's `flattener`, TypeScript's `FlattenContext` in destructuring.ts.
struct Flattener<'f, 'v, 'c> {
    visitor: &'f mut NodeVisitor<'v>,
    emit_context: EmitContext,
    level: FlattenLevel,

    create_assignment_callback: Option<&'c CreateAssignmentCallback<'c>>,

    // State
    expressions: Vec<NodeId>,
    declarations: Vec<PendingDecl>,
    has_transformed_prior_element: bool,
    hoist_temp_variables: bool,

    mode: Mode,
}

/// `NodeFactory.NewNodeList(nodes)`: an undefined location.
fn new_node_list(factory: &mut dyn RuntimeFactory, nodes: Vec<Option<NodeId>>) -> NodeListId {
    let nodes = factory.alloc_nodes(nodes);
    factory.alloc_list(TextRange::new(-1, -1), nodes)
}

/// `NodeFactory.NewAssignmentExpression` with upstream's nilable operands.
fn new_assignment(
    emit_context: &EmitContext,
    factory: &mut dyn RuntimeFactory,
    left: Option<NodeId>,
    right: Option<NodeId>,
) -> NodeId {
    if let (Some(left), Some(right)) = (left, right) {
        return emit_context.new_assignment_expression(factory, left, right);
    }
    let token = factory.new_token(K::EqualsToken.into());
    factory.new_binary_expression(None, left, None, Some(token), right)
}

/// `ast.IsAssignmentPattern`.
fn is_assignment_pattern(kind: NodeKind) -> bool {
    matches!(
        kind.known(),
        Some(K::ArrayLiteralExpression | K::ObjectLiteralExpression)
    )
}

/// `ast.GetElementsOfBindingOrAssignmentPattern`: the elements of a binding
/// or array literal pattern, the properties of an object literal, nil for
/// anything else.
fn elements_of_pattern(view: AstView<'_>, pattern: NodeId) -> Result<Vec<Option<NodeId>>, Error> {
    let read = view.node(pattern)?;
    let nodes = match read.kind().known() {
        Some(K::ObjectBindingPattern | K::ArrayBindingPattern | K::ArrayLiteralExpression) => {
            read.elements(view)?
        }
        Some(K::ObjectLiteralExpression) => read.properties(view)?,
        _ => return Ok(Vec::new()),
    };
    Ok(view.node_slice(nodes)?.iter().collect())
}

/// `node.AsBinaryExpression().Right`.
fn binary_right(view: AstView<'_>, node: NodeId) -> Result<Option<NodeId>, Error> {
    let read = view.node(node)?;
    Ok(read
        .as_binary_expression()
        .unwrap_or_else(|| interface_conversion(&read, "BinaryExpression"))
        .right())
}

/// `node.AsBinaryExpression().Left`.
fn binary_left(view: AstView<'_>, node: NodeId) -> Result<Option<NodeId>, Error> {
    let read = view.node(node)?;
    Ok(read
        .as_binary_expression()
        .unwrap_or_else(|| interface_conversion(&read, "BinaryExpression"))
        .left())
}

impl<'f, 'v> Flattener<'f, 'v, '_> {
    /// Upstream's entry points install the mode callbacks after construction;
    /// here they are the `mode`.
    // port: tsc/internal/transformers/destructuring.go:newFlattener
    fn new(
        visitor: &'f mut NodeVisitor<'v>,
        emit_context: &EmitContext,
        level: FlattenLevel,
        mode: Mode,
    ) -> Self {
        Self {
            visitor,
            emit_context: emit_context.clone(),
            level,
            create_assignment_callback: None,
            expressions: Vec::new(),
            declarations: Vec::new(),
            has_transformed_prior_element: false,
            hoist_temp_variables: false,
            mode,
        }
    }

    fn factory(&mut self) -> &mut dyn RuntimeFactory {
        self.visitor.factory_mut()
    }

    fn builder(&self) -> &AstBuilder {
        self.visitor.factory().ast_builder().expect(BUILDER)
    }

    fn view(&self) -> AstView<'_> {
        self.builder().view()
    }

    fn is_identifier(&self, node: NodeId) -> bool {
        tsr_ast::is_identifier(&self.visitor.factory().node(node))
    }

    fn range(&self, node: NodeId) -> TextRange {
        self.visitor.factory().node(node).range()
    }

    /// `value.Text()` of an identifier.
    fn identifier_text(&self, node: NodeId) -> JsString {
        let read = self.visitor.factory().node(node);
        read.as_identifier()
            .unwrap_or_else(|| interface_conversion(&read, "Identifier"))
            .text_owned()
    }

    fn subtree_facts(&self, node: NodeId) -> u32 {
        self.view().subtree_facts(node)
    }

    // The mode callbacks.

    fn emit_binding_or_assignment(
        &mut self,
        target: NodeId,
        value: Option<NodeId>,
        location: TextRange,
        original: Option<NodeId>,
    ) {
        match self.mode {
            Mode::Assignment => self.emit_assignment(target, value, location, original),
            Mode::Binding => self.emit_binding(target, value, location, original),
        }
    }

    fn create_array_binding_or_assignment_pattern(
        &mut self,
        elements: Vec<Option<NodeId>>,
    ) -> NodeId {
        match self.mode {
            Mode::Assignment => self.create_array_assignment_pattern(elements),
            Mode::Binding => self.create_array_binding_pattern(elements),
        }
    }

    fn create_object_binding_or_assignment_pattern(
        &mut self,
        elements: Vec<Option<NodeId>>,
    ) -> NodeId {
        match self.mode {
            Mode::Assignment => self.create_object_assignment_pattern(elements),
            Mode::Binding => self.create_object_binding_pattern(elements),
        }
    }

    fn create_array_binding_or_assignment_element(&mut self, expr: NodeId) -> NodeId {
        match self.mode {
            Mode::Assignment => Self::create_array_assignment_element(expr),
            Mode::Binding => self.create_array_binding_element(expr),
        }
    }

    // --- Assignment mode callbacks ---

    // port: tsc/internal/transformers/destructuring.go:flattener.createArrayAssignmentPattern
    fn create_array_assignment_pattern(&mut self, elements: Vec<Option<NodeId>>) -> NodeId {
        let list = new_node_list(self.factory(), elements);
        self.factory()
            .new_array_literal_expression(Some(list), false)
    }

    // port: tsc/internal/transformers/destructuring.go:flattener.createObjectAssignmentPattern
    fn create_object_assignment_pattern(&mut self, elements: Vec<Option<NodeId>>) -> NodeId {
        let list = new_node_list(self.factory(), elements);
        self.factory()
            .new_object_literal_expression(Some(list), false)
    }

    // port: tsc/internal/transformers/destructuring.go:flattener.createArrayAssignmentElement
    fn create_array_assignment_element(expr: NodeId) -> NodeId {
        expr
    }

    // port: tsc/internal/transformers/destructuring.go:flattener.emitAssignment
    fn emit_assignment(
        &mut self,
        target: NodeId,
        value: Option<NodeId>,
        location: TextRange,
        original: Option<NodeId>,
    ) {
        let expression = match self.create_assignment_callback {
            Some(callback) if self.is_identifier(target) => {
                callback(self.visitor, target, value.expect(NIL), Some(location))
            }
            _ => {
                let target = self.visitor.visit_node(Some(target));
                let expression = new_assignment(
                    &self.emit_context,
                    self.visitor.factory_mut(),
                    target,
                    value,
                );
                self.factory().set_node_range(expression, location);
                expression
            }
        };
        self.emit_context
            .set_original(expression, original.expect("Original cannot be nil."));
        self.emit_expression(expression);
    }

    // --- Binding mode callbacks ---

    // port: tsc/internal/transformers/destructuring.go:flattener.createArrayBindingPattern
    fn create_array_binding_pattern(&mut self, elements: Vec<Option<NodeId>>) -> NodeId {
        let list = new_node_list(self.factory(), elements);
        self.factory()
            .new_binding_pattern(K::ArrayBindingPattern.into(), Some(list))
    }

    // port: tsc/internal/transformers/destructuring.go:flattener.createObjectBindingPattern
    fn create_object_binding_pattern(&mut self, elements: Vec<Option<NodeId>>) -> NodeId {
        let list = new_node_list(self.factory(), elements);
        self.factory()
            .new_binding_pattern(K::ObjectBindingPattern.into(), Some(list))
    }

    // port: tsc/internal/transformers/destructuring.go:flattener.createArrayBindingElement
    fn create_array_binding_element(&mut self, expr: NodeId) -> NodeId {
        self.factory()
            .new_binding_element(None, None, Some(expr), None)
    }

    // port: tsc/internal/transformers/destructuring.go:flattener.emitBinding
    fn emit_binding(
        &mut self,
        target: NodeId,
        mut value: Option<NodeId>,
        location: TextRange,
        original: Option<NodeId>,
    ) {
        if !self.expressions.is_empty() {
            let mut expressions = std::mem::take(&mut self.expressions);
            expressions.push(value.expect(NIL));
            value = self
                .emit_context
                .inline_expressions(self.visitor.factory_mut(), &expressions);
        }
        self.declarations.push(PendingDecl {
            pending_expressions: Vec::new(),
            name: target,
            value,
            location,
            original,
        });
    }

    // --- Shared helpers ---

    // port: tsc/internal/transformers/destructuring.go:flattener.emitExpression
    fn emit_expression(&mut self, expr: NodeId) {
        self.expressions.push(expr);
    }

    // port: tsc/internal/transformers/destructuring.go:flattener.ensureIdentifier
    fn ensure_identifier(
        &mut self,
        value: Option<NodeId>,
        reuse_identifier_expressions: bool,
        location: TextRange,
    ) -> NodeId {
        if reuse_identifier_expressions {
            let value = value.expect(NIL);
            if self.is_identifier(value) {
                return value;
            }
        }
        let temp = self
            .emit_context
            .new_temp_variable(self.visitor.factory_mut());
        if self.hoist_temp_variables {
            self.emit_context
                .add_variable_declaration(self.visitor.factory_mut(), temp);
            let assign = new_assignment(
                &self.emit_context,
                self.visitor.factory_mut(),
                Some(temp),
                value,
            );
            self.factory().set_node_range(assign, location);
            self.emit_expression(assign);
        } else {
            self.emit_binding_or_assignment(temp, value, location, None);
        }
        temp
    }

    // port: tsc/internal/transformers/destructuring.go:flattener.createDefaultValueCheck
    fn create_default_value_check(
        &mut self,
        value: Option<NodeId>,
        default_value: NodeId,
        location: TextRange,
    ) -> NodeId {
        let value = self.ensure_identifier(value, true, location);
        let factory = self.visitor.factory_mut();
        let condition = self
            .emit_context
            .new_type_check(factory, value, b"undefined");
        let question_token = factory.new_token(K::QuestionToken.into());
        let colon_token = factory.new_token(K::ColonToken.into());
        factory.new_conditional_expression(
            Some(condition),
            Some(question_token),
            Some(default_value),
            Some(colon_token),
            Some(value),
        )
    }

    // port: tsc/internal/transformers/destructuring.go:flattener.createDestructuringPropertyAccess
    fn create_destructuring_property_access(
        &mut self,
        value: Option<NodeId>,
        property_name: NodeId,
    ) -> Result<NodeId, Error> {
        let (computed, literal, loc) = {
            let read = self.visitor.factory().node(property_name);
            (
                tsr_ast::is_computed_property_name(&read),
                is_string_or_numeric_literal_like(&read) || tsr_ast::is_big_int_literal(&read),
                read.range(),
            )
        };
        if computed {
            let expression = self.visitor.factory().node(property_name).expression();
            let visited = self.visitor.visit_node(expression);
            let argument_expression = self.ensure_identifier(visited, false, loc);
            return Ok(self.factory().new_element_access_expression(
                value,
                None,
                Some(argument_expression),
                node_flags::NONE,
            ));
        } else if literal {
            let argument_expression = tsr_ast::clone_node(self.factory(), property_name);
            return Ok(self.factory().new_element_access_expression(
                value,
                None,
                Some(argument_expression),
                node_flags::NONE,
            ));
        }
        let text = self.view().node_text(property_name)?.into_js_string();
        let name = self.factory().new_identifier(text);
        Ok(self
            .factory()
            .new_property_access_expression(value, None, Some(name), node_flags::NONE))
    }

    // --- Entry points ---

    // port: tsc/internal/transformers/destructuring.go:flattener.flattenDestructuringAssignment
    fn flatten_destructuring_assignment(
        &mut self,
        mut node: NodeId,
        needs_value: bool,
    ) -> Result<Option<NodeId>, Error> {
        let mut location = self.range(node);
        let mut value = None;
        if tsr_ast::is_destructuring_assignment(self.view(), node)? {
            value = binary_right(self.view(), node)?;
            loop {
                let left = binary_left(self.view(), node)?.expect(NIL);
                let left = self.view().node(left)?;
                if !(is_empty_array_literal(self.view(), &left)?
                    || is_empty_object_literal(self.view(), &left)?)
                {
                    break;
                }
                let right = value.expect(NIL);
                if tsr_ast::is_destructuring_assignment(self.view(), right)? {
                    node = right;
                    location = self.range(node);
                    value = binary_right(self.view(), node)?;
                } else {
                    return Ok(self.visitor.visit_node(value));
                }
            }
        }

        if value.is_some() {
            value = self.visitor.visit_node(value);
            let visited = value.expect(NIL);
            let assigns_to_name = self.is_identifier(visited)
                && binding_or_assignment_element_assigns_to_name(
                    self.view(),
                    node,
                    self.identifier_text(visited).as_bytes(),
                )?;
            if assigns_to_name
                || binding_or_assignment_element_contains_non_literal_computed_name(
                    self.view(),
                    node,
                )?
            {
                value = Some(self.ensure_identifier(value, false, location));
            } else if needs_value {
                value = Some(self.ensure_identifier(value, true, location));
            } else if node_is_synthesized(&self.visitor.factory().node(node)) {
                location = self.range(visited);
            }
        }

        let skip_initializer = tsr_ast::is_destructuring_assignment(self.view(), node)?;
        self.flatten_binding_or_assignment_element(node, value, location, skip_initializer)?;

        if let Some(value) = value {
            if needs_value {
                if self.expressions.is_empty() {
                    return Ok(Some(value));
                }
                self.expressions.push(value);
            }
        }

        let res = self
            .emit_context
            .inline_expressions(self.visitor.factory_mut(), &self.expressions);
        if res.is_some() {
            return Ok(res);
        }
        Ok(Some(self.factory().new_omitted_expression()))
    }

    // port: tsc/internal/transformers/destructuring.go:flattener.flattenDestructuringBinding
    fn flatten_destructuring_binding(
        &mut self,
        mut node: NodeId,
        rval: Option<NodeId>,
        skip_initializer: bool,
    ) -> Result<Option<NodeId>, Error> {
        if tsr_ast::is_variable_declaration(&self.visitor.factory().node(node)) {
            let initializer =
                get_initializer_of_binding_or_assignment_element(self.view(), Some(node))?;
            if let Some(initializer) = initializer {
                let assigns_to_name = self.is_identifier(initializer)
                    && binding_or_assignment_element_assigns_to_name(
                        self.view(),
                        node,
                        self.identifier_text(initializer).as_bytes(),
                    )?;
                if assigns_to_name
                    || binding_or_assignment_element_contains_non_literal_computed_name(
                        self.view(),
                        node,
                    )?
                {
                    let loc = self.range(initializer);
                    let visited = self.visitor.visit_node(Some(initializer));
                    let initializer = self.ensure_identifier(visited, false, loc);
                    let name = self.visitor.factory().node(node).name();
                    node = self.factory().update_variable_declaration(
                        node,
                        name,
                        None,
                        None,
                        Some(initializer),
                    );
                }
            }
        }

        let location = self.range(node);
        self.flatten_binding_or_assignment_element(node, rval, location, skip_initializer)?;

        if !self.expressions.is_empty() {
            let temp = self
                .emit_context
                .new_temp_variable(self.visitor.factory_mut());
            if self.hoist_temp_variables {
                let value = self
                    .emit_context
                    .inline_expressions(self.visitor.factory_mut(), &self.expressions);
                self.expressions = Vec::new();
                // `core.TextRange{}`.
                self.emit_binding_or_assignment(temp, value, TextRange::new(0, 0), None);
            } else {
                self.emit_context
                    .add_variable_declaration(self.visitor.factory_mut(), temp);
                let last_value = self
                    .declarations
                    .last()
                    .expect("runtime error: index out of range [-1]")
                    .value;
                let assignment = new_assignment(
                    &self.emit_context,
                    self.visitor.factory_mut(),
                    Some(temp),
                    last_value,
                );
                let last = self
                    .declarations
                    .last_mut()
                    .expect("runtime error: index out of range [-1]");
                last.pending_expressions.push(assignment);
                last.pending_expressions
                    .extend_from_slice(&self.expressions);
                last.value = Some(temp);
            }
        }

        let declarations = std::mem::take(&mut self.declarations);
        let mut decls = Vec::with_capacity(declarations.len());
        for pending in declarations {
            let mut expr = pending.value;
            if !pending.pending_expressions.is_empty() {
                let mut expressions = pending.pending_expressions;
                expressions.push(pending.value.expect(NIL));
                expr = self
                    .emit_context
                    .inline_expressions(self.visitor.factory_mut(), &expressions);
            }
            let decl =
                self.factory()
                    .new_variable_declaration(Some(pending.name), None, None, expr);
            self.factory().set_node_range(decl, pending.location);
            if let Some(original) = pending.original {
                self.emit_context.set_original(decl, original);
            }
            decls.push(decl);
        }

        if decls.len() == 1 {
            return Ok(Some(decls[0]));
        }
        if decls.is_empty() {
            return Ok(None);
        }
        let children = self
            .factory()
            .alloc_nodes(decls.into_iter().map(Some).collect());
        Ok(Some(self.factory().new_syntax_list(children)))
    }

    // --- Core flattening ---

    // port: tsc/internal/transformers/destructuring.go:flattener.flattenBindingOrAssignmentElement
    fn flatten_binding_or_assignment_element(
        &mut self,
        element: NodeId,
        mut value: Option<NodeId>,
        location: TextRange,
        skip_initializer: bool,
    ) -> Result<(), Error> {
        let Some(binding_target) = self.view().target_of_binding_or_assignment_element(element)
        else {
            return Ok(());
        };
        if !skip_initializer {
            let initializer =
                get_initializer_of_binding_or_assignment_element(self.view(), Some(element))?;
            let initializer = self.visitor.visit_node(initializer);
            if let Some(initializer) = initializer {
                if value.is_some() {
                    value = Some(self.create_default_value_check(value, initializer, location));
                    let target_kind = self.visitor.factory().node(binding_target).kind();
                    if !is_simple_copiable_expression(self.visitor.factory(), initializer)
                        && (tsr_ast::utilities::is_binding_pattern_kind(target_kind)
                            || is_assignment_pattern(target_kind))
                    {
                        value = Some(self.ensure_identifier(value, true, location));
                    }
                } else {
                    value = Some(initializer);
                }
            } else if value.is_none() {
                value = Some(
                    self.emit_context
                        .new_void_zero_expression(self.visitor.factory_mut()),
                );
            }
        }

        if is_object_binding_or_assignment_pattern(self.visitor.factory(), Some(binding_target)) {
            self.flatten_object_binding_or_assignment_pattern(
                element,
                binding_target,
                value,
                location,
            )
        } else if is_array_binding_or_assignment_pattern(
            self.visitor.factory(),
            Some(binding_target),
        ) {
            self.flatten_array_binding_or_assignment_pattern(
                element,
                binding_target,
                value,
                location,
            )
        } else {
            self.emit_binding_or_assignment(binding_target, value, location, Some(element));
            Ok(())
        }
    }

    // port: tsc/internal/transformers/destructuring.go:flattener.flattenObjectBindingOrAssignmentPattern
    fn flatten_object_binding_or_assignment_pattern(
        &mut self,
        parent: NodeId,
        pattern: NodeId,
        mut value: Option<NodeId>,
        location: TextRange,
    ) -> Result<(), Error> {
        let elements = elements_of_pattern(self.view(), pattern)?;
        let num_elements = elements.len();
        if num_elements != 1 {
            let reuse_identifier_expressions =
                !is_declaration_binding_element(&self.visitor.factory().node(parent))
                    || num_elements != 0;
            value = Some(self.ensure_identifier(value, reuse_identifier_expressions, location));
        }
        let mut binding_elements: Vec<Option<NodeId>> = Vec::new();
        let mut computed_temp_variables: Vec<NodeId> = Vec::new();
        for (i, &element) in elements.iter().enumerate() {
            let element = element.expect(NIL);
            if get_rest_indicator_of_binding_or_assignment_element(self.view(), element)?.is_none()
            {
                let property_name =
                    try_get_property_name_of_binding_or_assignment_element(self.view(), element)?;
                if self.level >= FlattenLevel::ObjectRest
                    && self.subtree_facts(element) & (REST_OR_SPREAD | OBJECT_REST_OR_SPREAD) == 0
                    && self.subtree_facts(
                        self.view()
                            .target_of_binding_or_assignment_element(element)
                            .expect(NIL),
                    ) & (REST_OR_SPREAD | OBJECT_REST_OR_SPREAD)
                        == 0
                    && !tsr_ast::is_computed_property_name(
                        &self.visitor.factory().node(property_name.expect(NIL)),
                    )
                {
                    binding_elements.push(self.visitor.visit_node(Some(element)));
                } else {
                    if !binding_elements.is_empty() {
                        let elements = std::mem::take(&mut binding_elements);
                        let target = self.create_object_binding_or_assignment_pattern(elements);
                        self.emit_binding_or_assignment(target, value, location, Some(pattern));
                    }
                    let property_name = property_name.expect(NIL);
                    let rhs_value =
                        self.create_destructuring_property_access(value, property_name)?;
                    if tsr_ast::is_computed_property_name(
                        &self.visitor.factory().node(property_name),
                    ) {
                        let read = self.visitor.factory().node(rhs_value);
                        let argument_expression = read
                            .as_element_access_expression()
                            .unwrap_or_else(|| {
                                interface_conversion(&read, "ElementAccessExpression")
                            })
                            .argument_expression()
                            .expect(NIL);
                        computed_temp_variables.push(argument_expression);
                    }
                    let element_location = self.range(element);
                    self.flatten_binding_or_assignment_element(
                        element,
                        Some(rhs_value),
                        element_location,
                        false,
                    )?;
                }
            } else if i == num_elements - 1 {
                if !binding_elements.is_empty() {
                    let elements = std::mem::take(&mut binding_elements);
                    let target = self.create_object_binding_or_assignment_pattern(elements);
                    self.emit_binding_or_assignment(target, value, location, Some(pattern));
                }
                // Every element is non-nil here: each earlier one was read.
                let all_elements: Vec<NodeId> = elements.iter().map(|e| e.expect(NIL)).collect();
                let pattern_location = self.range(pattern);
                // Upstream's nilable slice is nil until a computed name appends.
                let computed = (!computed_temp_variables.is_empty())
                    .then_some(computed_temp_variables.as_slice());
                let builder = self.visitor.factory_mut().ast_builder_mut().expect(BUILDER);
                let rhs_value = self.emit_context.new_rest_helper(
                    builder,
                    value.expect(NIL),
                    &all_elements,
                    computed,
                    pattern_location,
                )?;
                let element_location = self.range(element);
                self.flatten_binding_or_assignment_element(
                    element,
                    Some(rhs_value),
                    element_location,
                    false,
                )?;
            }
        }
        if !binding_elements.is_empty() {
            let target = self.create_object_binding_or_assignment_pattern(binding_elements);
            self.emit_binding_or_assignment(target, value, location, Some(pattern));
        }
        Ok(())
    }

    // port: tsc/internal/transformers/destructuring.go:flattener.flattenArrayBindingOrAssignmentPattern
    fn flatten_array_binding_or_assignment_pattern(
        &mut self,
        parent: NodeId,
        pattern: NodeId,
        mut value: Option<NodeId>,
        location: TextRange,
    ) -> Result<(), Error> {
        let elements = elements_of_pattern(self.view(), pattern)?;
        let num_elements = elements.len();
        if num_elements != 1 && (self.level < FlattenLevel::ObjectRest || num_elements == 0)
            || elements.iter().all(|element| {
                tsr_ast::is_omitted_expression(&self.visitor.factory().node(element.expect(NIL)))
            })
        {
            let reuse_identifier_expressions =
                !is_declaration_binding_element(&self.visitor.factory().node(parent))
                    || num_elements != 0;
            value = Some(self.ensure_identifier(value, reuse_identifier_expressions, location));
        }
        let mut binding_elements: Vec<Option<NodeId>> = Vec::new();
        // Upstream's `restIdElemPair`s: (id, element).
        let mut rest_containing_elements: Vec<(NodeId, NodeId)> = Vec::new();
        for (i, &element) in elements.iter().enumerate() {
            let element = element.expect(NIL);
            if self.level >= FlattenLevel::ObjectRest {
                if self.subtree_facts(element) & OBJECT_REST_OR_SPREAD != 0
                    || self.has_transformed_prior_element
                        && !is_simple_binding_or_assignment_element(self.builder(), element)?
                {
                    self.has_transformed_prior_element = true;
                    let temp = self
                        .emit_context
                        .new_temp_variable(self.visitor.factory_mut());
                    if self.hoist_temp_variables {
                        self.emit_context
                            .add_variable_declaration(self.visitor.factory_mut(), temp);
                    }
                    rest_containing_elements.push((temp, element));
                    binding_elements
                        .push(Some(self.create_array_binding_or_assignment_element(temp)));
                } else {
                    binding_elements.push(Some(element));
                }
            } else if tsr_ast::is_omitted_expression(&self.visitor.factory().node(element)) {
                // Upstream continues with the next element.
            } else if get_rest_indicator_of_binding_or_assignment_element(self.view(), element)?
                .is_none()
            {
                let index = self.factory().new_numeric_literal(
                    JsString::from_bytes(i.to_string().into_bytes()),
                    token_flags::NONE,
                );
                let rhs_value = self.factory().new_element_access_expression(
                    value,
                    None,
                    Some(index),
                    node_flags::NONE,
                );
                let element_location = self.range(element);
                self.flatten_binding_or_assignment_element(
                    element,
                    Some(rhs_value),
                    element_location,
                    false,
                )?;
            } else if i == num_elements - 1 {
                let rhs_value = self.emit_context.new_array_slice_call(
                    self.visitor.factory_mut(),
                    value.expect(NIL),
                    i as i64,
                );
                let element_location = self.range(element);
                self.flatten_binding_or_assignment_element(
                    element,
                    Some(rhs_value),
                    element_location,
                    false,
                )?;
            }
        }
        if !binding_elements.is_empty() {
            let target = self.create_array_binding_or_assignment_pattern(binding_elements);
            self.emit_binding_or_assignment(target, value, location, Some(pattern));
        }
        for (id, element) in rest_containing_elements {
            let element_location = self.range(element);
            self.flatten_binding_or_assignment_element(element, Some(id), element_location, false)?;
        }
        Ok(())
    }
}

// --- Exported helper functions ---

/// Checks if any target in a binding/assignment pattern assigns to the given name.
// port: tsc/internal/transformers/destructuring.go:BindingOrAssignmentElementAssignsToName
pub fn binding_or_assignment_element_assigns_to_name(
    view: AstView<'_>,
    element: NodeId,
    name: &[u8],
) -> Result<bool, Error> {
    let Some(target) = view.target_of_binding_or_assignment_element(element) else {
        return Ok(false);
    };
    let read = view.node(target)?;
    if is_binding_pattern(&read) || is_assignment_pattern(read.kind()) {
        return binding_or_assignment_pattern_assigns_to_name(view, target, name);
    } else if tsr_ast::is_identifier(&read) {
        return Ok(view.node_text(target)?.as_bytes() == name);
    }
    Ok(false)
}

// port: tsc/internal/transformers/destructuring.go:bindingOrAssignmentPatternAssignsToName
fn binding_or_assignment_pattern_assigns_to_name(
    view: AstView<'_>,
    pattern: NodeId,
    name: &[u8],
) -> Result<bool, Error> {
    for element in elements_of_pattern(view, pattern)? {
        if binding_or_assignment_element_assigns_to_name(view, element.expect(NIL), name)? {
            return Ok(true);
        }
    }
    Ok(false)
}

/// Checks if any element has a non-literal computed property name.
// port: tsc/internal/transformers/destructuring.go:BindingOrAssignmentElementContainsNonLiteralComputedName
pub fn binding_or_assignment_element_contains_non_literal_computed_name(
    view: AstView<'_>,
    element: NodeId,
) -> Result<bool, Error> {
    let property_name = try_get_property_name_of_binding_or_assignment_element(view, element)?;
    if let Some(property_name) = property_name {
        let read = view.node(property_name)?;
        if tsr_ast::is_computed_property_name(&read)
            && !is_literal_expression(&view.node(read.expression().expect(NIL))?)
        {
            return Ok(true);
        }
    }
    let Some(target) = view.target_of_binding_or_assignment_element(element) else {
        return Ok(false);
    };
    let read = view.node(target)?;
    Ok(
        (is_binding_pattern(&read) || is_assignment_pattern(read.kind()))
            && binding_or_assignment_pattern_contains_non_literal_computed_name(view, target)?,
    )
}

// port: tsc/internal/transformers/destructuring.go:bindingOrAssignmentPatternContainsNonLiteralComputedName
fn binding_or_assignment_pattern_contains_non_literal_computed_name(
    view: AstView<'_>,
    pattern: NodeId,
) -> Result<bool, Error> {
    for element in elements_of_pattern(view, pattern)? {
        if binding_or_assignment_element_contains_non_literal_computed_name(
            view,
            element.expect(NIL),
        )? {
            return Ok(true);
        }
    }
    Ok(false)
}

/// Returns the initializer/default value of a binding or assignment element.
// port: tsc/internal/transformers/destructuring.go:GetInitializerOfBindingOrAssignmentElement
pub fn get_initializer_of_binding_or_assignment_element(
    view: AstView<'_>,
    binding_element: Option<NodeId>,
) -> Result<Option<NodeId>, Error> {
    let Some(binding_element) = binding_element else {
        return Ok(None);
    };
    let read = view.node(binding_element)?;
    if is_declaration_binding_element(&read) {
        return Ok(read.initializer());
    }
    if tsr_ast::is_property_assignment(&read) {
        let initializer = read.initializer().expect(NIL);
        if tsr_ast::is_assignment_expression(view, initializer, true)? {
            return binary_right(view, initializer);
        }
        return Ok(None);
    }
    if tsr_ast::is_shorthand_property_assignment(&read) {
        return Ok(read
            .as_shorthand_property_assignment()
            .unwrap_or_else(|| interface_conversion(&read, "ShorthandPropertyAssignment"))
            .object_assignment_initializer());
    }
    if tsr_ast::is_assignment_expression(view, binding_element, true)? {
        return binary_right(view, binding_element);
    }
    if tsr_ast::is_spread_element(&read) {
        return get_initializer_of_binding_or_assignment_element(view, read.expression());
    }
    Ok(None)
}

// port: tsc/internal/transformers/destructuring.go:isObjectBindingOrAssignmentPattern
fn is_object_binding_or_assignment_pattern(factory: &dyn Factory, node: Option<NodeId>) -> bool {
    node.is_some_and(|node| {
        matches!(
            factory.node(node).kind().known(),
            Some(K::ObjectBindingPattern | K::ObjectLiteralExpression)
        )
    })
}

// port: tsc/internal/transformers/destructuring.go:isArrayBindingOrAssignmentPattern
fn is_array_binding_or_assignment_pattern(factory: &dyn Factory, node: Option<NodeId>) -> bool {
    node.is_some_and(|node| {
        matches!(
            factory.node(node).kind().known(),
            Some(K::ArrayBindingPattern | K::ArrayLiteralExpression)
        )
    })
}

// port: tsc/internal/transformers/destructuring.go:isSimpleBindingOrAssignmentElement
fn is_simple_binding_or_assignment_element(
    builder: &AstBuilder,
    element: NodeId,
) -> Result<bool, Error> {
    let view = builder.view();
    let target = view.target_of_binding_or_assignment_element(element);
    let Some(target) = target else {
        return Ok(true);
    };
    if tsr_ast::is_omitted_expression(&view.node(target)?) {
        return Ok(true);
    }
    let property_name = try_get_property_name_of_binding_or_assignment_element(view, element)?;
    if let Some(property_name) = property_name {
        if !is_property_name_literal(&view.node(property_name)?) {
            return Ok(false);
        }
    }
    let initializer = get_initializer_of_binding_or_assignment_element(view, Some(element))?;
    if let Some(initializer) = initializer {
        if !is_simple_inlineable_expression(builder, initializer) {
            return Ok(false);
        }
    }
    let read = view.node(target)?;
    if is_binding_pattern(&read) || is_assignment_pattern(read.kind()) {
        for element in elements_of_pattern(view, target)? {
            if !is_simple_binding_or_assignment_element(builder, element.expect(NIL))? {
                return Ok(false);
            }
        }
        return Ok(true);
    }
    Ok(tsr_ast::is_identifier(&read))
}
