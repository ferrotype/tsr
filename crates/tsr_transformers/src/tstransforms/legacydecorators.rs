//! `transformers/tstransforms/legacydecorators.go`: lowers
//! `experimentalDecorators` decorators to `__decorate` and `__param` helper
//! calls after the class, rewriting a decorated class declaration to a `let`
//! bound class expression (with a class alias when the class body refers to
//! the class itself).
//!
//! Upstream's `elideNodes` has no caller and no Rust counterpart.
use crate::estransforms::utilities::{identifier_text, list_nodes, new_node_list, view, NIL};
use crate::transformer::{Error, Failure, SharedReferenceResolver, TransformOptions, Transformer};
use crate::utilities::{
    is_generated_identifier, is_simple_inlineable_expression, move_range_past_modifiers,
};
use std::cell::RefCell;
use std::collections::HashMap;
use std::ops::ControlFlow;
use std::rc::Rc;
use tsr_ast::utilities::{
    has_accessor_modifier, has_static_modifier, has_syntactic_modifier, is_static,
};
use tsr_ast::utilities_class::{
    child_is_decorated, class_or_constructor_parameter_is_decorated, decorators,
    get_all_accessor_declarations, get_first_constructor_with_body, is_this_parameter,
    node_or_child_is_decorated,
};
use tsr_ast::utilities_middle::has_decorators;
use tsr_ast::{
    modifier_flags, node_flags, subtree_flags, token_flags, AstBuilder, ChildVisitor,
    FactoryMethods, JsString, NodeId, NodeListId, NodeSlice, NodeVisitor, RuntimeFactory,
    SyntaxKind as K,
};
use tsr_core::{ScriptTarget, TextRange};
use tsr_printer::{emit_flags, AssignedNameOptions, EmitContext, NameOptions};

const BUILDER: &str = "the legacy decorators transform needs the transformer's AstBuilder";

/// `LegacyDecoratorsTransformer`.
struct LegacyDecoratorsTransformer<'a> {
    language_version: ScriptTarget,
    reference_resolver: SharedReferenceResolver<'a>,
    emit_context: EmitContext,
    failure: Failure,

    /// A map that keeps track of aliases created for classes with decorators
    /// to avoid issues with the double-binding behavior of classes. Upstream's
    /// nil map is `None`.
    class_aliases: RefCell<Option<HashMap<NodeId, NodeId>>>,
    enclosing_classes: RefCell<Vec<NodeId>>,
}

// port: tsc/internal/transformers/tstransforms/legacydecorators.go:NewLegacyDecoratorsTransformer
pub fn new_legacy_decorators_transformer<'a>(
    opts: &TransformOptions<'a>,
) -> Option<Transformer<'a>> {
    let tx = Rc::new(LegacyDecoratorsTransformer {
        language_version: opts.compiler_options.emit_script_target(),
        reference_resolver: opts.resolver.clone(),
        emit_context: opts.context.clone(),
        failure: opts.failure.clone(),
        class_aliases: RefCell::new(None),
        enclosing_classes: RefCell::new(Vec::new()),
    });
    Some(Transformer::new(
        move |visitor: &mut NodeVisitor<'_>, node: Option<NodeId>| {
            let id = node.expect(NIL);
            if tx.failure.is_set() {
                return node;
            }
            match tx.visit(visitor, id) {
                Ok(result) => result,
                Err(error) => {
                    tx.failure.record(error);
                    node
                }
            }
        },
        Some(opts.context.clone()),
        opts.failure.clone(),
    ))
}

/// Go's `allDecorators`: no reader distinguishes a nil slice from an empty
/// one.
struct AllDecorators {
    decorators: Vec<NodeId>,
    parameters: Vec<Vec<NodeId>>,
}

/// The concrete builder the printer's name helpers take.
fn builder<'v>(visitor: &'v mut NodeVisitor<'_>) -> &'v mut AstBuilder {
    visitor.factory_mut().ast_builder_mut().expect(BUILDER)
}

/// Go's `node.ForEachChild` order, lists element by element.
fn children(factory: &dyn RuntimeFactory, node: NodeId) -> Vec<NodeId> {
    struct Children<'a> {
        factory: &'a dyn RuntimeFactory,
        nodes: Vec<NodeId>,
    }
    impl ChildVisitor for Children<'_> {
        fn visit_node(&mut self, node: NodeId) -> ControlFlow<()> {
            self.nodes.push(node);
            ControlFlow::Continue(())
        }
        fn visit_list(&mut self, list: NodeListId) -> ControlFlow<()> {
            self.visit_node_slice(self.factory.read_list(list).nodes())
        }
        fn visit_node_slice(&mut self, nodes: NodeSlice) -> ControlFlow<()> {
            self.nodes
                .extend(self.factory.read_nodes(nodes).iter().flatten());
            ControlFlow::Continue(())
        }
    }
    let mut visitor = Children {
        factory,
        nodes: Vec::new(),
    };
    let _ = factory.node(node).for_each_child(&mut visitor);
    visitor.nodes
}

/// Inline `ast.CanHaveDecorators`, which `tsr_ast` does not export.
fn can_have_decorators(factory: &dyn RuntimeFactory, node: NodeId) -> bool {
    matches!(
        factory.node(node).kind().known(),
        Some(
            K::Parameter
                | K::PropertyDeclaration
                | K::MethodDeclaration
                | K::GetAccessor
                | K::SetAccessor
                | K::ClassExpression
                | K::ClassDeclaration
        )
    )
}

/// `ast.HasDecorators(node)`.
fn node_has_decorators(factory: &dyn RuntimeFactory, node: NodeId) -> Result<bool, Error> {
    let view = view(factory)?;
    Ok(has_decorators(view, &view.node(node)?)?)
}

/// `node.Decorators()`.
fn node_decorators(factory: &dyn RuntimeFactory, node: NodeId) -> Result<Vec<NodeId>, Error> {
    Ok(decorators(view(factory)?, node)?)
}

/// The members of a class (`node.Members.Nodes`): a nil list is a nil
/// dereference.
fn class_members(factory: &dyn RuntimeFactory, node: NodeId) -> Vec<NodeId> {
    let members = factory.node(node).member_list().expect(NIL);
    list_nodes(factory, Some(members))
}

// port: tsc/internal/transformers/tstransforms/legacydecorators.go:elideModifiers
fn elide_modifiers(
    factory: &mut dyn RuntimeFactory,
    nodes: Option<NodeListId>,
) -> Option<NodeListId> {
    let nodes = nodes?;
    if factory.read_list(nodes).nodes().is_empty() {
        return Some(nodes);
    }
    let empty = factory.alloc_nodes(Vec::new());
    let replacement = factory.new_modifier_list(empty);
    let loc = factory.read_list(nodes).loc();
    factory.set_list_location(replacement, loc);
    Some(replacement)
}

// port: tsc/internal/transformers/tstransforms/legacydecorators.go:isClassStaticBlockDeclarationOrStaticProperty
fn is_class_static_block_declaration_or_static_property(
    factory: &dyn RuntimeFactory,
    node: NodeId,
) -> Result<bool, Error> {
    let read = factory.node(node);
    Ok(tsr_ast::is_class_static_block_declaration(&read)
        || (tsr_ast::is_property_declaration(&read) && has_static_modifier(view(factory)?, node)?))
}

// port: tsc/internal/transformers/tstransforms/legacydecorators.go:isNotExportOrDefaultOrDecorator
fn is_not_export_or_default_or_decorator(factory: &dyn RuntimeFactory, node: NodeId) -> bool {
    let read = factory.node(node);
    !(tsr_ast::is_decorator(&read)
        || read.kind() == K::ExportKeyword
        || read.kind() == K::DefaultKeyword)
}

// port: tsc/internal/transformers/tstransforms/legacydecorators.go:decoratorContainsPrivateIdentifierInExpression
fn decorator_contains_private_identifier_in_expression(
    factory: &dyn RuntimeFactory,
    decorator: NodeId,
) -> Result<bool, Error> {
    Ok(
        view(factory)?.subtree_facts(decorator) & subtree_flags::PRIVATE_IDENTIFIER_IN_EXPRESSION
            != 0,
    )
}

// port: tsc/internal/transformers/tstransforms/legacydecorators.go:parameterDecoratorsContainPrivateIdentifierInExpression
fn parameter_decorators_contain_private_identifier_in_expression(
    factory: &dyn RuntimeFactory,
    parameter_decorators: &[NodeId],
) -> Result<bool, Error> {
    for &decorator in parameter_decorators {
        if decorator_contains_private_identifier_in_expression(factory, decorator)? {
            return Ok(true);
        }
    }
    Ok(false)
}

// port: tsc/internal/transformers/tstransforms/legacydecorators.go:hasClassElementWithDecoratorContainingPrivateIdentifierInExpression
fn has_class_element_with_decorator_containing_private_identifier_in_expression(
    factory: &dyn RuntimeFactory,
    node: NodeId,
) -> Result<bool, Error> {
    let members = list_nodes(factory, factory.node(node).member_list());
    if members.is_empty() {
        return Ok(false);
    }
    for member in members {
        if !can_have_decorators(factory, member) {
            continue;
        }
        let Some(all_decorators) =
            get_all_decorators_of_class_element(factory, member, node, true)?
        else {
            continue;
        };
        for &decorator in &all_decorators.decorators {
            if decorator_contains_private_identifier_in_expression(factory, decorator)? {
                return Ok(true);
            }
        }
        for parameter in &all_decorators.parameters {
            if parameter_decorators_contain_private_identifier_in_expression(factory, parameter)? {
                return Ok(true);
            }
        }
    }
    Ok(false)
}

/// Gets the decorators of a class and of the parameters of its constructor.
// port: tsc/internal/transformers/tstransforms/legacydecorators.go:getAllDecoratorsOfClass
fn get_all_decorators_of_class(
    factory: &dyn RuntimeFactory,
    node: NodeId,
    use_legacy_decorators: bool,
) -> Result<Option<AllDecorators>, Error> {
    let decorators = node_decorators(factory, node)?;
    let mut parameters = Vec::new();
    if use_legacy_decorators {
        let constructor = get_first_constructor_with_body(view(factory)?, node)?;
        parameters = get_decorators_of_parameters(factory, constructor)?;
    }
    if decorators.is_empty() && parameters.is_empty() {
        return Ok(None);
    }
    Ok(Some(AllDecorators {
        decorators,
        parameters,
    }))
}

/// Gets the decorators of a class member and of its parameters.
// port: tsc/internal/transformers/tstransforms/legacydecorators.go:getAllDecoratorsOfClassElement
fn get_all_decorators_of_class_element(
    factory: &dyn RuntimeFactory,
    member: NodeId,
    parent: NodeId,
    use_legacy_decorators: bool,
) -> Result<Option<AllDecorators>, Error> {
    match factory.node(member).kind().known() {
        Some(K::GetAccessor | K::SetAccessor) => {
            if !use_legacy_decorators {
                return get_all_decorators_of_method(factory, member, false);
            }
            get_all_decorators_of_accessors(factory, member, parent, true)
        }
        Some(K::MethodDeclaration) => {
            get_all_decorators_of_method(factory, member, use_legacy_decorators)
        }
        Some(K::PropertyDeclaration) => get_all_decorators_of_property(factory, member),
        _ => Ok(None),
    }
}

/// Gets the decorators of an accessor pair (on whichever accessor declares
/// them first) and of the set accessor's parameters.
// port: tsc/internal/transformers/tstransforms/legacydecorators.go:getAllDecoratorsOfAccessors
fn get_all_decorators_of_accessors(
    factory: &dyn RuntimeFactory,
    accessor: NodeId,
    parent: NodeId,
    use_legacy_decorators: bool,
) -> Result<Option<AllDecorators>, Error> {
    if factory.node(accessor).body().is_none() {
        return Ok(None);
    }
    let members = class_members(factory, parent);
    let decls = get_all_accessor_declarations(view(factory)?, &members, accessor)?;
    let first_accessor = decls.first_accessor.expect(NIL);
    let mut first_accessor_with_decorators = None;
    if node_has_decorators(factory, first_accessor)? {
        first_accessor_with_decorators = Some(first_accessor);
    } else if let Some(second_accessor) = decls.second_accessor {
        if node_has_decorators(factory, second_accessor)? {
            first_accessor_with_decorators = Some(second_accessor);
        }
    }

    let Some(first_accessor_with_decorators) = first_accessor_with_decorators else {
        return Ok(None);
    };
    if accessor != first_accessor_with_decorators {
        return Ok(None);
    }

    let decorators = node_decorators(factory, first_accessor_with_decorators)?;
    let mut parameters = Vec::new();
    if use_legacy_decorators && decls.set_accessor.is_some() {
        parameters = get_decorators_of_parameters(factory, decls.set_accessor)?;
    }

    if decorators.is_empty() && parameters.is_empty() {
        return Ok(None);
    }

    Ok(Some(AllDecorators {
        decorators,
        parameters,
    }))
}

// port: tsc/internal/transformers/tstransforms/legacydecorators.go:getAllDecoratorsOfProperty
fn get_all_decorators_of_property(
    factory: &dyn RuntimeFactory,
    property: NodeId,
) -> Result<Option<AllDecorators>, Error> {
    let decorators = node_decorators(factory, property)?;
    if decorators.is_empty() {
        return Ok(None);
    }
    Ok(Some(AllDecorators {
        decorators,
        parameters: Vec::new(),
    }))
}

// port: tsc/internal/transformers/tstransforms/legacydecorators.go:getAllDecoratorsOfMethod
fn get_all_decorators_of_method(
    factory: &dyn RuntimeFactory,
    method: NodeId,
    use_legacy_decorators: bool,
) -> Result<Option<AllDecorators>, Error> {
    if factory.node(method).body().is_none() {
        return Ok(None);
    }
    let decorators = node_decorators(factory, method)?;
    let mut parameters = Vec::new();
    if use_legacy_decorators {
        parameters = get_decorators_of_parameters(factory, Some(method))?;
    }
    if decorators.is_empty() && parameters.is_empty() {
        return Ok(None);
    }
    Ok(Some(AllDecorators {
        decorators,
        parameters,
    }))
}

/// Gets an array of arrays of decorators for the parameters of a
/// function-like node. The offset into the result array corresponds to the
/// offset of the parameter (a `this` parameter excluded); empty when no
/// parameter is decorated.
// port: tsc/internal/transformers/tstransforms/legacydecorators.go:getDecoratorsOfParameters
fn get_decorators_of_parameters(
    factory: &dyn RuntimeFactory,
    node: Option<NodeId>,
) -> Result<Vec<Vec<NodeId>>, Error> {
    let mut decorators: Vec<Vec<NodeId>> = Vec::new();
    if let Some(node) = node {
        let parameters = list_nodes(factory, factory.node(node).parameter_list());
        let first_parameter_is_this =
            !parameters.is_empty() && is_this_parameter(view(factory)?, parameters[0])?;
        let first_parameter_offset = usize::from(first_parameter_is_this);
        let num_parameters = parameters.len() - first_parameter_offset;
        for i in 0..num_parameters {
            let p = parameters[i + first_parameter_offset];
            if !decorators.is_empty() || node_has_decorators(factory, p)? {
                if decorators.is_empty() {
                    decorators = vec![Vec::new(); num_parameters];
                }
                decorators[i] = node_decorators(factory, p)?;
            }
        }
    }
    Ok(decorators)
}

/// Whether a class member is a static (or instance) member that is
/// decorated or has decorated parameters.
// port: tsc/internal/transformers/tstransforms/legacydecorators.go:isDecoratedClassElement
fn is_decorated_class_element(
    factory: &dyn RuntimeFactory,
    member: NodeId,
    is_static_element: bool,
    parent: NodeId,
) -> Result<bool, Error> {
    let view = view(factory)?;
    Ok(is_static_element == is_static(view, member)?
        && node_or_child_is_decorated(view, true, member, Some(parent), None)?)
}

/// The static (or instance) members of a class that are decorated or have
/// decorated parameters.
// port: tsc/internal/transformers/tstransforms/legacydecorators.go:getDecoratedClassElements
fn get_decorated_class_elements(
    factory: &dyn RuntimeFactory,
    node: NodeId,
    is_static_element: bool,
) -> Result<Vec<NodeId>, Error> {
    let mut members = Vec::new();
    for member in list_nodes(factory, factory.node(node).member_list()) {
        if is_decorated_class_element(factory, member, is_static_element, node)? {
            members.push(member);
        }
    }
    Ok(members)
}

impl LegacyDecoratorsTransformer<'_> {
    // port: tsc/internal/transformers/tstransforms/legacydecorators.go:LegacyDecoratorsTransformer.visit
    fn visit(&self, v: &mut NodeVisitor<'_>, node: NodeId) -> Result<Option<NodeId>, Error> {
        // we have to visit all identifiers in classes, just in case they require substitution
        if view(v.factory())?.subtree_facts(node) & subtree_flags::DECORATORS == 0
            && self.enclosing_classes.borrow().is_empty()
        {
            return Ok(Some(node));
        }

        let kind = v.factory().node(node).kind();
        match kind.known() {
            Some(K::Identifier) => self.visit_identifier(node).map(Some),
            Some(K::PropertyAccessExpression) => {
                Ok(Some(Self::visit_property_access_expression(v, node)))
            }
            // Decorators are elided. They will be emitted as part of `visitClassDeclaration`.
            Some(K::Decorator) => Ok(None),
            Some(K::ClassDeclaration) => self.visit_class_declaration(v, node),
            Some(K::ClassExpression) => Ok(Some(Self::visit_class_expression(v, node))),
            Some(K::Constructor) => Ok(Some(Self::visit_constructor_declaration(v, node))),
            Some(K::MethodDeclaration) => self.visit_method_declaration(v, node).map(Some),
            Some(K::SetAccessor) => self.visit_set_accessor_declaration(v, node).map(Some),
            Some(K::GetAccessor) => self.visit_get_accessor_declaration(v, node).map(Some),
            Some(K::PropertyDeclaration) => self.visit_property_declaration(v, node),
            Some(K::Parameter) => Ok(Some(self.visit_paramer_declaration(v, node))),
            Some(K::SourceFile) => {
                *self.class_aliases.borrow_mut() = Some(HashMap::new());
                self.enclosing_classes.borrow_mut().clear();
                let result = v.visit_each_child(Some(node));
                let mut ec = self.emit_context.clone();
                let helpers = ec.read_emit_helpers();
                ec.add_emit_helper(result.expect(NIL), &helpers);
                *self.class_aliases.borrow_mut() = None;
                self.enclosing_classes.borrow_mut().clear();
                Ok(result)
            }
            _ => Ok(v.visit_each_child(Some(node))),
        }
    }

    // port: tsc/internal/transformers/tstransforms/legacydecorators.go:LegacyDecoratorsTransformer.visitIdentifier
    fn visit_identifier(&self, node: NodeId) -> Result<NodeId, Error> {
        // takes the place of `substituteIdentifier` in the strada transform
        let enclosing_classes = self.enclosing_classes.borrow().clone();
        for d in enclosing_classes {
            let alias = self
                .class_aliases
                .borrow()
                .as_ref()
                .and_then(|aliases| aliases.get(&d).copied());
            let Some(alias) = alias else {
                continue;
            };
            let original = self.emit_context.most_original(node);
            let declaration = self
                .reference_resolver
                .borrow_mut()
                .get_referenced_value_declaration(original)?;
            if declaration == Some(self.emit_context.most_original(d)) {
                return Ok(alias);
            }
        }
        Ok(node)
    }

    // port: tsc/internal/transformers/tstransforms/legacydecorators.go:LegacyDecoratorsTransformer.visitPropertyAccessExpression
    fn visit_property_access_expression(v: &mut NodeVisitor<'_>, node: NodeId) -> NodeId {
        // Visit the expression but not the name, since property access names should not be substituted.
        // Strada's onSubstituteNode only fires for EmitHint.Expression, which excludes the
        // .name of PropertyAccessExpression.
        let (n, flags) = {
            let read = v.factory().node(node);
            let data = read
                .data_source()
                .as_property_access_expression()
                .expect("interface conversion: ast.nodeData is not *ast.PropertyAccessExpression")
                .to_owned();
            (data, read.flags())
        };
        let expression = v.visit_node(n.expression);
        if expression != n.expression {
            return v.factory_mut().update_property_access_expression(
                node,
                expression,
                n.question_dot_token,
                n.name,
                flags,
            );
        }
        node
    }

    // port: tsc/internal/transformers/tstransforms/legacydecorators.go:LegacyDecoratorsTransformer.finishClassElement
    fn finish_class_element(
        &self,
        factory: &dyn RuntimeFactory,
        updated: NodeId,
        original: NodeId,
    ) -> NodeId {
        if updated != original {
            // While we emit the source map for the node after skipping decorators and modifiers,
            // we need to emit the comments for the original range.
            let mut ec = self.emit_context.clone();
            ec.set_comment_range(updated, factory.node(original).range());
            ec.set_source_map_range(updated, move_range_past_modifiers(factory, original));
        }
        updated
    }

    // port: tsc/internal/transformers/tstransforms/legacydecorators.go:LegacyDecoratorsTransformer.visitParamerDeclaration
    fn visit_paramer_declaration(&self, v: &mut NodeVisitor<'_>, node: NodeId) -> NodeId {
        let n = v
            .factory()
            .node(node)
            .data_source()
            .as_parameter_declaration()
            .expect("interface conversion: ast.nodeData is not *ast.ParameterDeclaration")
            .to_owned();
        let modifiers = elide_modifiers(v.factory_mut(), n.modifiers);
        let name = v.visit_node(n.name);
        let initializer = v.visit_node(n.initializer);
        let f = v.factory_mut();
        let updated = f.update_parameter_declaration(
            node,
            modifiers,
            n.dot_dot_dot_token,
            name,
            None,
            None,
            initializer,
        );
        if updated != node {
            // While we emit the source map for the node after skipping decorators and modifiers,
            // we need to emit the comments for the original range.
            let mut ec = self.emit_context.clone();
            ec.set_comment_range(updated, f.node(node).range());
            let new_loc = move_range_past_modifiers(f, node);
            f.set_node_range(updated, new_loc);
            ec.set_source_map_range(updated, new_loc);
            let updated_name = f.node(updated).name().expect(NIL);
            ec.set_emit_flags(updated_name, emit_flags::NO_TRAILING_SOURCE_MAP);
        }
        updated
    }

    /// Visits the property name of a class element, for use when emitting
    /// property initializers. For a computed property on a node with
    /// decorators, a temporary value is stored for later use.
    // port: tsc/internal/transformers/tstransforms/legacydecorators.go:LegacyDecoratorsTransformer.visitPropertyNameOfClassElement
    fn visit_property_name_of_class_element(
        &self,
        v: &mut NodeVisitor<'_>,
        member: NodeId,
    ) -> Result<Option<NodeId>, Error> {
        let name = v.factory().node(member).name().expect(NIL);
        if tsr_ast::is_computed_property_name(&v.factory().node(name))
            && node_has_decorators(v.factory(), member)?
        {
            let inner = v.factory().node(name).expression();
            let expression = v.visit_node(inner).expect(NIL);
            let inner_expression =
                tsr_ast::skip_partially_emitted_expressions(view(v.factory())?, expression)?;
            if !is_simple_inlineable_expression(v.factory(), inner_expression) {
                let mut ec = self.emit_context.clone();
                let f = v.factory_mut();
                let generated_name = ec.new_generated_name_for_node(f, name);
                ec.add_variable_declaration(f, generated_name);
                let assignment = ec.new_assignment_expression(f, generated_name, expression);
                return Ok(Some(
                    f.update_computed_property_name(name, Some(assignment)),
                ));
            }
        }
        Ok(v.visit_node(Some(name)))
    }

    // port: tsc/internal/transformers/tstransforms/legacydecorators.go:LegacyDecoratorsTransformer.visitPropertyDeclaration
    fn visit_property_declaration(
        &self,
        v: &mut NodeVisitor<'_>,
        node: NodeId,
    ) -> Result<Option<NodeId>, Error> {
        if v.factory().node(node).flags() & node_flags::AMBIENT != 0 {
            return Ok(None);
        }
        if has_syntactic_modifier(
            view(v.factory())?,
            node,
            modifier_flags::AMBIENT | modifier_flags::ABSTRACT,
        )? {
            return Ok(None);
        }

        let n = v
            .factory()
            .node(node)
            .data_source()
            .as_property_declaration()
            .expect("interface conversion: ast.nodeData is not *ast.PropertyDeclaration")
            .to_owned();
        let modifiers = v.visit_modifiers(n.modifiers);
        let name = self.visit_property_name_of_class_element(v, node)?;
        let initializer = v.visit_node(n.initializer);
        let updated = v.factory_mut().update_property_declaration(
            node,
            modifiers,
            name,
            None,
            None,
            initializer,
        );
        Ok(Some(self.finish_class_element(v.factory(), updated, node)))
    }

    // port: tsc/internal/transformers/tstransforms/legacydecorators.go:LegacyDecoratorsTransformer.visitGetAccessorDeclaration
    fn visit_get_accessor_declaration(
        &self,
        v: &mut NodeVisitor<'_>,
        node: NodeId,
    ) -> Result<NodeId, Error> {
        let n = v
            .factory()
            .node(node)
            .data_source()
            .as_get_accessor_declaration()
            .expect("interface conversion: ast.nodeData is not *ast.GetAccessorDeclaration")
            .to_owned();
        let modifiers = v.visit_modifiers(n.modifiers);
        let name = self.visit_property_name_of_class_element(v, node)?;
        let parameters = v.visit_nodes(n.parameters);
        let body = v.visit_node(n.body);
        let updated = v.factory_mut().update_get_accessor_declaration(
            node, modifiers, name, None, parameters, None, None, body,
        );
        Ok(self.finish_class_element(v.factory(), updated, node))
    }

    // port: tsc/internal/transformers/tstransforms/legacydecorators.go:LegacyDecoratorsTransformer.visitSetAccessorDeclaration
    fn visit_set_accessor_declaration(
        &self,
        v: &mut NodeVisitor<'_>,
        node: NodeId,
    ) -> Result<NodeId, Error> {
        let n = v
            .factory()
            .node(node)
            .data_source()
            .as_set_accessor_declaration()
            .expect("interface conversion: ast.nodeData is not *ast.SetAccessorDeclaration")
            .to_owned();
        let modifiers = v.visit_modifiers(n.modifiers);
        let name = self.visit_property_name_of_class_element(v, node)?;
        let parameters = v.visit_nodes(n.parameters);
        let body = v.visit_node(n.body);
        let updated = v.factory_mut().update_set_accessor_declaration(
            node, modifiers, name, None, parameters, None, None, body,
        );
        Ok(self.finish_class_element(v.factory(), updated, node))
    }

    // port: tsc/internal/transformers/tstransforms/legacydecorators.go:LegacyDecoratorsTransformer.visitMethodDeclaration
    fn visit_method_declaration(
        &self,
        v: &mut NodeVisitor<'_>,
        node: NodeId,
    ) -> Result<NodeId, Error> {
        let n = v
            .factory()
            .node(node)
            .data_source()
            .as_method_declaration()
            .expect("interface conversion: ast.nodeData is not *ast.MethodDeclaration")
            .to_owned();
        let modifiers = v.visit_modifiers(n.modifiers);
        let name = self.visit_property_name_of_class_element(v, node)?;
        let parameters = v.visit_nodes(n.parameters);
        let body = v.visit_node(n.body);
        let updated = v.factory_mut().update_method_declaration(
            node,
            modifiers,
            n.asterisk_token,
            name,
            None,
            None,
            parameters,
            None,
            None,
            body,
        );
        Ok(self.finish_class_element(v.factory(), updated, node))
    }

    // port: tsc/internal/transformers/tstransforms/legacydecorators.go:LegacyDecoratorsTransformer.visitConstructorDeclaration
    fn visit_constructor_declaration(v: &mut NodeVisitor<'_>, node: NodeId) -> NodeId {
        let n = v
            .factory()
            .node(node)
            .data_source()
            .as_constructor_declaration()
            .expect("interface conversion: ast.nodeData is not *ast.ConstructorDeclaration")
            .to_owned();
        let modifiers = v.visit_modifiers(n.modifiers);
        let parameters = v.visit_nodes(n.parameters);
        let body = v.visit_node(n.body);
        v.factory_mut()
            .update_constructor_declaration(node, modifiers, None, parameters, None, None, body)
    }

    // port: tsc/internal/transformers/tstransforms/legacydecorators.go:LegacyDecoratorsTransformer.visitClassExpression
    fn visit_class_expression(v: &mut NodeVisitor<'_>, node: NodeId) -> NodeId {
        // Legacy decorators were not supported on class expressions
        let n = v
            .factory()
            .node(node)
            .data_source()
            .as_class_expression()
            .expect("interface conversion: ast.nodeData is not *ast.ClassExpression")
            .to_owned();
        let modifiers = v.visit_modifiers(n.modifiers);
        let heritage_clauses = v.visit_nodes(n.heritage_clauses);
        let members = v.visit_nodes(n.members);
        v.factory_mut().update_class_expression(
            node,
            modifiers,
            n.name,
            None,
            heritage_clauses,
            members,
        )
    }

    // port: tsc/internal/transformers/tstransforms/legacydecorators.go:LegacyDecoratorsTransformer.visitClassDeclaration
    fn visit_class_declaration(
        &self,
        v: &mut NodeVisitor<'_>,
        node: NodeId,
    ) -> Result<Option<NodeId>, Error> {
        let decorated =
            class_or_constructor_parameter_is_decorated(view(v.factory())?, true, node)?;
        if !(decorated || child_is_decorated(view(v.factory())?, true, node, None)?) {
            return Ok(v.visit_each_child(Some(node)));
        }

        let name = v.factory().node(node).name();
        if decorated {
            return self.transform_class_declaration_with_class_decorators(v, node, name);
        }
        self.transform_class_declaration_without_class_decorators(v, node, name)
    }

    /// Transforms a non-decorated class declaration.
    // port: tsc/internal/transformers/tstransforms/legacydecorators.go:LegacyDecoratorsTransformer.transformClassDeclarationWithoutClassDecorators
    fn transform_class_declaration_without_class_decorators(
        &self,
        v: &mut NodeVisitor<'_>,
        node: NodeId,
        mut name: Option<NodeId>,
    ) -> Result<Option<NodeId>, Error> {
        //  ${modifiers} class ${name} ${heritageClauses} {
        //      ${members}
        //  }
        let n = v
            .factory()
            .node(node)
            .data_source()
            .as_class_declaration()
            .expect("interface conversion: ast.nodeData is not *ast.ClassDeclaration")
            .to_owned();
        let modifiers = v.visit_modifiers(n.modifiers);
        let heritage_clauses = v.visit_nodes(n.heritage_clauses);
        let initial_members = v.visit_nodes(n.members);
        let (members, decoration_statements) =
            self.transform_decorators_of_class_elements(v, node, initial_members)?;

        if name.is_none() && !decoration_statements.is_empty() {
            let mut ec = self.emit_context.clone();
            name = Some(ec.new_generated_name_for_node(v.factory_mut(), node));
        }

        let f = v.factory_mut();
        let updated =
            f.update_class_declaration(node, modifiers, name, None, heritage_clauses, members);

        if decoration_statements.is_empty() {
            return Ok(Some(updated));
        }
        let mut statements = vec![Some(updated)];
        statements.extend(decoration_statements.into_iter().map(Some));
        let children = f.alloc_nodes(statements);
        Ok(Some(f.new_syntax_list(children)))
    }

    // port: tsc/internal/transformers/tstransforms/legacydecorators.go:LegacyDecoratorsTransformer.popEnclosingClass
    fn pop_enclosing_class(&self) {
        self.enclosing_classes
            .borrow_mut()
            .pop()
            .expect("runtime error: slice bounds out of range [:-1]");
    }

    // port: tsc/internal/transformers/tstransforms/legacydecorators.go:LegacyDecoratorsTransformer.pushEnclosingClass
    fn push_enclosing_class(&self, cls: NodeId) {
        self.enclosing_classes.borrow_mut().push(cls);
    }

    /// Transforms a decorated class declaration and returns the resulting
    /// statements; a class whose body refers to the class itself gets an
    /// alias to avoid issues with double-binding.
    ///
    /// `@dec class C {}` becomes `let C = class C {}; C = __decorate([dec], C);`
    /// (an exported class adds `export { C };`, a default-exported one
    /// `export default C;`); with a self reference the class is bound to an
    /// alias as well: `let C = C_1 = class C { ... }; C = C_1 =
    /// __decorate([dec], C); var C_1;`.
    // port: tsc/internal/transformers/tstransforms/legacydecorators.go:LegacyDecoratorsTransformer.transformClassDeclarationWithClassDecorators
    fn transform_class_declaration_with_class_decorators(
        &self,
        v: &mut NodeVisitor<'_>,
        node: NodeId,
        name: Option<NodeId>,
    ) -> Result<Option<NodeId>, Error> {
        let (is_export, is_default) = {
            let view = view(v.factory())?;
            (
                has_syntactic_modifier(view, node, modifier_flags::EXPORT)?,
                has_syntactic_modifier(view, node, modifier_flags::DEFAULT)?,
            )
        };
        let mut modifiers = None;
        if let Some(node_modifiers) = v.factory().node(node).modifiers() {
            let f = v.factory_mut();
            let nodes = list_nodes(f, Some(node_modifiers));
            if !nodes.is_empty() {
                let modifier_nodes: Vec<Option<NodeId>> = nodes
                    .iter()
                    .copied()
                    .filter(|&modifier| is_not_export_or_default_or_decorator(f, modifier))
                    .map(Some)
                    .collect();
                if modifier_nodes.len() == nodes.len() {
                    modifiers = Some(node_modifiers);
                } else {
                    let slice = f.alloc_nodes(modifier_nodes);
                    let list = f.new_modifier_list(slice);
                    let loc = f.read_list(node_modifiers).loc();
                    f.set_list_location(list, loc);
                    modifiers = Some(list);
                }
            }
        }

        let location = move_range_past_modifiers(v.factory(), node);
        let class_alias = self.get_class_alias_if_needed(v, node)?;
        if class_alias.is_some() {
            self.push_enclosing_class(node);
        }
        let result = self.transform_decorated_class_body(
            v,
            node,
            &DecoratedClass {
                name,
                is_export,
                is_default,
                modifiers,
                location,
                class_alias,
            },
        );
        // Upstream's deferred `popEnclosingClass`.
        if class_alias.is_some() {
            self.pop_enclosing_class();
        }
        result
    }

    /// The rest of `transformClassDeclarationWithClassDecorators`, run while
    /// the class alias (if any) is pushed.
    fn transform_decorated_class_body(
        &self,
        v: &mut NodeVisitor<'_>,
        node: NodeId,
        class: &DecoratedClass,
    ) -> Result<Option<NodeId>, Error> {
        let DecoratedClass {
            name,
            is_export,
            is_default,
            modifiers,
            location,
            class_alias,
        } = *class;
        let mut ec = self.emit_context.clone();

        // When we used to transform to ES5/3 this would be moved inside an IIFE and should reference the name
        // without any block-scoped variable collision handling - but we don't support that anymore, so we always
        // use the local name for the class
        let decl_name = ec.get_local_name_ex(
            builder(v),
            Some(node),
            AssignedNameOptions {
                allow_comments: false,
                allow_source_maps: true,
                ignore_assigned_name: false,
            },
        )?;

        //  ... = class ${name} ${heritageClauses} {
        //      ${members}
        //  }
        let n = v
            .factory()
            .node(node)
            .data_source()
            .as_class_declaration()
            .expect("interface conversion: ast.nodeData is not *ast.ClassDeclaration")
            .to_owned();
        let heritage_clauses = v.visit_nodes(n.heritage_clauses);
        let members = v.visit_nodes(n.members);

        let (mut members, decoration_statements) =
            self.transform_decorators_of_class_elements(v, node, members)?;

        // If we're emitting to ES2022 or later then we need to reassign the class alias before
        // static initializers are evaluated.
        let mut assign_class_alias_in_static_block = false;
        if self.language_version >= ScriptTarget::ES2022 && class_alias.is_some() {
            for member in list_nodes(v.factory(), members) {
                if is_class_static_block_declaration_or_static_property(v.factory(), member)? {
                    assign_class_alias_in_static_block = true;
                    break;
                }
            }
        }
        let f = v.factory_mut();
        if assign_class_alias_in_static_block {
            let this = f.new_keyword_expression(K::ThisKeyword.into());
            let assignment = ec.new_assignment_expression(f, class_alias.expect(NIL), this);
            let statement = f.new_expression_statement(Some(assignment));
            let statements = new_node_list(f, vec![statement]);
            let block = f.new_block(Some(statements), false);
            let static_block = f.new_class_static_block_declaration(None, Some(block));
            let mut member_list = vec![static_block];
            member_list.extend(list_nodes(f, members));
            let new_list = new_node_list(f, member_list);
            let loc = f.read_list(members.expect(NIL)).loc();
            f.set_list_location(new_list, loc);
            members = Some(new_list);
        }

        let mut expr_name = name;
        if name.is_some_and(|name| is_generated_identifier(&ec, name)) {
            expr_name = None;
        }
        let class_expression =
            f.new_class_expression(modifiers, expr_name, None, heritage_clauses, members);

        ec.set_original(class_expression, node);
        f.set_node_range(class_expression, location);

        //  let ${name} = ${classExpression} where name is either declaredName if the class doesn't contain self-reference
        //                                         or decoratedClassAlias if the class contain self-reference.
        let mut var_initializer = class_expression;
        if let Some(class_alias) = class_alias {
            if !assign_class_alias_in_static_block {
                var_initializer = ec.new_assignment_expression(f, class_alias, class_expression);
            }
        }
        let var_decl =
            f.new_variable_declaration(Some(decl_name), None, None, Some(var_initializer));
        ec.set_original(var_decl, node);

        let declarations = new_node_list(f, vec![var_decl]);
        let var_decl_list = f.new_variable_declaration_list(Some(declarations), node_flags::LET);
        let var_statement = f.new_variable_statement(None, Some(var_decl_list));
        ec.set_original(var_statement, node);
        f.set_node_range(var_statement, location);
        let node_loc = f.node(node).range();
        ec.set_comment_range(var_statement, node_loc);

        let mut statements = vec![Some(var_statement)];
        statements.extend(decoration_statements.into_iter().map(Some));
        statements.push(self.get_constructor_decoration_statement(v, node)?);

        if is_export {
            let export_statement = if is_default {
                ec.new_export_default(v.factory_mut(), decl_name)
            } else {
                let declaration_name = ec.get_declaration_name(builder(v), Some(node))?;
                ec.new_external_module_export(v.factory_mut(), declaration_name)
            };
            statements.push(Some(export_statement));
        }

        if statements.len() == 1 {
            return Ok(statements[0]);
        }
        let f = v.factory_mut();
        let children = f.alloc_nodes(statements);
        Ok(Some(f.new_syntax_list(children)))
    }

    // port: tsc/internal/transformers/tstransforms/legacydecorators.go:LegacyDecoratorsTransformer.hasInternalStaticReference
    fn has_internal_static_reference(
        &self,
        factory: &dyn RuntimeFactory,
        node: NodeId,
    ) -> Result<bool, Error> {
        let class_node = self.emit_context.most_original(node);
        for member in class_members(factory, node) {
            for child in children(factory, member) {
                if self.is_or_contains_static_self_reference(factory, child, class_node)? {
                    return Ok(true);
                }
            }
        }
        Ok(false)
    }

    /// The recursive closure of `hasInternalStaticReference`.
    fn is_or_contains_static_self_reference(
        &self,
        factory: &dyn RuntimeFactory,
        n: NodeId,
        class_node: NodeId,
    ) -> Result<bool, Error> {
        if tsr_ast::is_identifier(&factory.node(n)) {
            let original = self.emit_context.most_original(n);
            let declaration = self
                .reference_resolver
                .borrow_mut()
                .get_referenced_value_declaration(original)?;
            if declaration == Some(class_node) {
                return Ok(true);
            }
        }
        // For PropertyAccessExpression, only check the expression, not the name.
        // The .Name() is a property access name, not a value reference to the class.
        if tsr_ast::is_property_access_expression(&factory.node(n)) {
            let expression = factory.node(n).expression().expect(NIL);
            return self.is_or_contains_static_self_reference(factory, expression, class_node);
        }
        for child in children(factory, n) {
            if self.is_or_contains_static_self_reference(factory, child, class_node)? {
                return Ok(true);
            }
        }
        Ok(false)
    }

    /// Gets a local alias for a class declaration if it is a decorated class
    /// with an internal reference to the static side of the class. This is
    /// necessary to avoid issues with double-binding semantics for the class
    /// name.
    // port: tsc/internal/transformers/tstransforms/legacydecorators.go:LegacyDecoratorsTransformer.getClassAliasIfNeeded
    fn get_class_alias_if_needed(
        &self,
        v: &mut NodeVisitor<'_>,
        node: NodeId,
    ) -> Result<Option<NodeId>, Error> {
        if !self.has_internal_static_reference(v.factory(), node)? {
            return Ok(None);
        }
        let mut ec = self.emit_context.clone();
        let mut name_text = JsString::from_bytes(b"default".to_vec());
        if let Some(name) = v.factory().node(node).name() {
            if !is_generated_identifier(&ec, name) {
                name_text = identifier_text(v.factory(), name);
            }
        }

        let f = v.factory_mut();
        let class_alias = ec.new_unique_name(f, name_text);
        ec.add_variable_declaration(f, class_alias);
        self.class_aliases
            .borrow_mut()
            .as_mut()
            .expect("assignment to entry in nil map")
            .insert(node, class_alias);

        Ok(Some(class_alias))
    }

    /// Generates a `__decorate` helper call statement for a class constructor.
    // port: tsc/internal/transformers/tstransforms/legacydecorators.go:LegacyDecoratorsTransformer.getConstructorDecorationStatement
    fn get_constructor_decoration_statement(
        &self,
        v: &mut NodeVisitor<'_>,
        node: NodeId,
    ) -> Result<Option<NodeId>, Error> {
        let expression = self.generate_constructor_decoration_expression(v, node)?;
        if let Some(expression) = expression {
            let result = v.factory_mut().new_expression_statement(Some(expression));
            self.emit_context.clone().set_original(result, node);
            return Ok(Some(result));
        }
        Ok(None)
    }

    /// Generates a `__decorate` helper call for a class constructor.
    // port: tsc/internal/transformers/tstransforms/legacydecorators.go:LegacyDecoratorsTransformer.generateConstructorDecorationExpression
    fn generate_constructor_decoration_expression(
        &self,
        v: &mut NodeVisitor<'_>,
        node: NodeId,
    ) -> Result<Option<NodeId>, Error> {
        let all_decorators = get_all_decorators_of_class(v.factory(), node, true)?;
        // Decorator expressions are evaluated outside the class body, so references to the
        // class name should use the original binding, not the class alias. In Strada, this is
        // handled by NodeCheckFlags.ConstructorReference which is only set for identifiers
        // inside the class body. Since Corsa lacks per-node flags, we temporarily pop the
        // enclosing class to prevent alias substitution during decorator expression visiting.
        let has_alias = self.enclosing_classes.borrow().last() == Some(&node);
        if has_alias {
            self.pop_enclosing_class();
        }
        let decorator_expressions =
            self.transform_all_decorators_of_declaration(v, all_decorators.as_ref());
        if has_alias {
            self.push_enclosing_class(node);
        }
        if decorator_expressions.is_empty() {
            return Ok(None);
        }

        let class_alias = self
            .class_aliases
            .borrow()
            .as_ref()
            .and_then(|aliases| aliases.get(&node).copied());

        // When we used to transform to ES5/3 this would be moved inside an IIFE and should reference the name
        // without any block-scoped variable collision handling - but we don't support that anymore, so we always
        // use the local name for the class
        let mut ec = self.emit_context.clone();
        let local_name = ec.get_declaration_name_ex(
            builder(v),
            Some(node),
            NameOptions {
                allow_comments: false,
                allow_source_maps: true,
            },
        )?;
        let f = v.factory_mut();
        let decorate = ec.new_decorate_helper(f, decorator_expressions, local_name, None, None);
        let mut assignment_target = decorate;
        if let Some(class_alias) = class_alias {
            assignment_target = ec.new_assignment_expression(f, class_alias, decorate);
        }
        let expression = ec.new_assignment_expression(f, local_name, assignment_target);
        ec.set_emit_flags(expression, emit_flags::NO_COMMENTS);
        ec.set_source_map_range(expression, move_range_past_modifiers(f, node));
        Ok(Some(expression))
    }

    // port: tsc/internal/transformers/tstransforms/legacydecorators.go:LegacyDecoratorsTransformer.transformDecoratorsOfClassElements
    fn transform_decorators_of_class_elements(
        &self,
        v: &mut NodeVisitor<'_>,
        node: NodeId,
        mut members: Option<NodeListId>,
    ) -> Result<(Option<NodeListId>, Vec<NodeId>), Error> {
        let mut decoration_statements =
            self.get_class_element_decoration_statements(v, node, false)?;
        decoration_statements.extend(self.get_class_element_decoration_statements(v, node, true)?);
        if has_class_element_with_decorator_containing_private_identifier_in_expression(
            v.factory(),
            node,
        )? {
            let f = v.factory_mut();
            let mut member_nodes = list_nodes(f, members);
            let statements = new_node_list(f, decoration_statements);
            let block = f.new_block(Some(statements), true);
            member_nodes.push(f.new_class_static_block_declaration(None, Some(block)));
            members = Some(new_node_list(f, member_nodes));
            decoration_statements = Vec::new();
        }

        Ok((members, decoration_statements))
    }

    /// Generates statements used to apply decorators to either the static or
    /// instance members of a class.
    // port: tsc/internal/transformers/tstransforms/legacydecorators.go:LegacyDecoratorsTransformer.getClassElementDecorationStatements
    fn get_class_element_decoration_statements(
        &self,
        v: &mut NodeVisitor<'_>,
        node: NodeId,
        is_static_element: bool,
    ) -> Result<Vec<NodeId>, Error> {
        let exprs =
            self.generate_class_element_decoration_expressions(v, node, is_static_element)?;
        let mut statements = Vec::new();
        for e in exprs {
            statements.push(v.factory_mut().new_expression_statement(Some(e)));
        }
        Ok(statements)
    }

    /// Generates expressions used to apply decorators to either the static or
    /// instance members of a class.
    // port: tsc/internal/transformers/tstransforms/legacydecorators.go:LegacyDecoratorsTransformer.generateClassElementDecorationExpressions
    fn generate_class_element_decoration_expressions(
        &self,
        v: &mut NodeVisitor<'_>,
        node: NodeId,
        is_static_element: bool,
    ) -> Result<Vec<NodeId>, Error> {
        let members = get_decorated_class_elements(v.factory(), node, is_static_element)?;
        let mut expressions = Vec::new();
        for member in members {
            if let Some(expr) =
                self.generate_class_element_decoration_expression(v, node, member)?
            {
                expressions.push(expr);
            }
        }
        Ok(expressions)
    }

    /// Generates an expression used to evaluate class element decorators at
    /// runtime: `__decorate([dec, __param(0, dec2)], C.prototype, "method",
    /// null)` for a method or accessor, `..., void 0)` for a property.
    // port: tsc/internal/transformers/tstransforms/legacydecorators.go:LegacyDecoratorsTransformer.generateClassElementDecorationExpression
    fn generate_class_element_decoration_expression(
        &self,
        v: &mut NodeVisitor<'_>,
        node: NodeId,
        member: NodeId,
    ) -> Result<Option<NodeId>, Error> {
        let all_decorators = get_all_decorators_of_class_element(v.factory(), member, node, true)?;
        let decorator_expressions =
            self.transform_all_decorators_of_declaration(v, all_decorators.as_ref());
        if decorator_expressions.is_empty() {
            return Ok(None);
        }

        let prefix = self.get_class_member_prefix(v, node, member)?;
        let generate_name = v.factory().node(member).flags() & node_flags::AMBIENT == 0;
        let member_name = self.get_expression_for_property_name(v, member, generate_name);
        let is_plain_property = tsr_ast::is_property_declaration(&v.factory().node(member))
            && !has_accessor_modifier(view(v.factory())?, member)?;
        let mut ec = self.emit_context.clone();
        let f = v.factory_mut();
        let descriptor = if is_plain_property {
            // We emit `void 0` here to indicate to `__decorate` that it can invoke `Object.defineProperty` directly, but that it
            // should not invoke `Object.getOwnPropertyDescriptor`.
            ec.new_void_zero_expression(f)
        } else {
            // We emit `null` here to indicate to `__decorate` that it can invoke `Object.getOwnPropertyDescriptor` directly.
            // We have this extra argument here so that we can inject an explicit property descriptor at a later date.
            f.new_keyword_expression(K::NullKeyword.into())
        };

        let helper = ec.new_decorate_helper(
            f,
            decorator_expressions,
            prefix,
            Some(member_name),
            Some(descriptor),
        );

        ec.set_emit_flags(helper, emit_flags::NO_COMMENTS);
        ec.set_source_map_range(helper, move_range_past_modifiers(f, member));
        Ok(Some(helper))
    }

    // port: tsc/internal/transformers/tstransforms/legacydecorators.go:LegacyDecoratorsTransformer.isSyntheticMetadataDecorator
    fn is_synthetic_metadata_decorator(&self, factory: &dyn RuntimeFactory, node: NodeId) -> bool {
        let expression = factory.node(node).expression().expect(NIL);
        self.emit_context
            .is_call_to_helper(factory, expression, b"__metadata")
    }

    /// Transforms all of the decorators for a declaration into an array of
    /// expressions.
    // port: tsc/internal/transformers/tstransforms/legacydecorators.go:LegacyDecoratorsTransformer.transformAllDecoratorsOfDeclaration
    fn transform_all_decorators_of_declaration(
        &self,
        v: &mut NodeVisitor<'_>,
        all_decorators: Option<&AllDecorators>,
    ) -> Vec<NodeId> {
        let Some(all_decorators) = all_decorators else {
            return Vec::new();
        };

        // ensure that metadata decorators are last
        let mut metadata = Vec::new();
        let mut decorators = Vec::new();
        for &decorator in &all_decorators.decorators {
            if self.is_synthetic_metadata_decorator(v.factory(), decorator) {
                metadata.push(decorator);
            } else {
                decorators.push(decorator);
            }
        }

        let mut decorator_expressions = Self::transform_decorators(v, &decorators);
        decorator_expressions
            .extend(self.transform_decorators_of_parameters(v, &all_decorators.parameters));
        decorator_expressions.extend(Self::transform_decorators(v, &metadata));
        decorator_expressions
    }

    // port: tsc/internal/transformers/tstransforms/legacydecorators.go:LegacyDecoratorsTransformer.transformDecoratorsOfParameters
    fn transform_decorators_of_parameters(
        &self,
        v: &mut NodeVisitor<'_>,
        parameters: &[Vec<NodeId>],
    ) -> Vec<NodeId> {
        let mut results = Vec::new();
        for (i, decorators) in parameters.iter().enumerate() {
            for &decorator in decorators {
                let expression = v.factory().node(decorator).expression();
                let visited = v.visit_node(expression).expect(NIL);
                let location = v.factory().node(expression.expect(NIL)).range();
                let offset = i64::try_from(i).expect("a parameter offset fits in a Go int");
                let mut ec = self.emit_context.clone();
                let helper = ec.new_param_helper(v.factory_mut(), visited, offset, location);
                ec.set_emit_flags(helper, emit_flags::NO_COMMENTS);
                results.push(helper);
            }
        }
        results
    }

    /// Transforms a list of decorators into their visited expressions.
    // port: tsc/internal/transformers/tstransforms/legacydecorators.go:LegacyDecoratorsTransformer.transformDecorators
    fn transform_decorators(v: &mut NodeVisitor<'_>, decorators: &[NodeId]) -> Vec<NodeId> {
        let mut results = Vec::new();
        for &d in decorators {
            let expression = v.factory().node(d).expression();
            results.push(v.visit_node(expression).expect(NIL));
        }
        results
    }

    // port: tsc/internal/transformers/tstransforms/legacydecorators.go:LegacyDecoratorsTransformer.getClassMemberPrefix
    fn get_class_member_prefix(
        &self,
        v: &mut NodeVisitor<'_>,
        node: NodeId,
        member: NodeId,
    ) -> Result<NodeId, Error> {
        if is_static(view(v.factory())?, member)? {
            return Ok(self
                .emit_context
                .clone()
                .get_declaration_name(builder(v), Some(node))?);
        }
        self.get_class_prototype(v, node)
    }

    // port: tsc/internal/transformers/tstransforms/legacydecorators.go:LegacyDecoratorsTransformer.getClassPrototype
    fn get_class_prototype(&self, v: &mut NodeVisitor<'_>, node: NodeId) -> Result<NodeId, Error> {
        let declaration_name = self
            .emit_context
            .clone()
            .get_declaration_name(builder(v), Some(node))?;
        let f = v.factory_mut();
        let prototype = f.new_identifier(JsString::from_bytes(b"prototype".to_vec()));
        Ok(f.new_property_access_expression(
            Some(declaration_name),
            None,
            Some(prototype),
            node_flags::NONE,
        ))
    }

    // port: tsc/internal/transformers/tstransforms/legacydecorators.go:LegacyDecoratorsTransformer.getExpressionForPropertyName
    fn get_expression_for_property_name(
        &self,
        v: &mut NodeVisitor<'_>,
        member: NodeId,
        generate_name_for_computed_property_name: bool,
    ) -> NodeId {
        let f = v.factory_mut();
        let name = f.node(member).name().expect(NIL);
        let (is_private, is_computed, is_identifier) = {
            let read = f.node(name);
            (
                tsr_ast::is_private_identifier(&read),
                tsr_ast::is_computed_property_name(&read),
                tsr_ast::is_identifier(&read),
            )
        };
        if is_private {
            f.new_identifier(JsString::default())
        } else if is_computed {
            let expression = f.node(name).expression().expect(NIL);
            if generate_name_for_computed_property_name
                && !is_simple_inlineable_expression(f, expression)
            {
                return self
                    .emit_context
                    .clone()
                    .new_generated_name_for_node(f, name);
            }
            expression
        } else if is_identifier {
            let text = identifier_text(f, name);
            f.new_string_literal(text, token_flags::NONE)
        } else {
            tsr_ast::deep_clone_node(f, Some(name)).expect(NIL)
        }
    }
}

/// The values `transformClassDeclarationWithClassDecorators` computes before
/// it pushes the class alias.
struct DecoratedClass {
    name: Option<NodeId>,
    is_export: bool,
    is_default: bool,
    modifiers: Option<NodeListId>,
    location: TextRange,
    class_alias: Option<NodeId>,
}
