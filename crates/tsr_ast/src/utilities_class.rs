//! AST utilities: classes, heritage, decorators and modifiers.
//!
//! Ports of `tsc/internal/ast/utilities.go`, witnessed by the `class` group of the Phase 1
//! operation tables (`docs/PHASE1-mutation-witnesses.md`, section 9).
use crate::bind_result::BoundView;
use crate::{modifier_flags, AstView, NodeAccess, NodeId, NodeListId, SymbolId, SyntaxKind as K};
use tsr_arena::Error;

const NIL: &str = "runtime error: invalid memory address or nil pointer dereference";

fn nodes(view: AstView<'_>, slice: crate::NodeSlice) -> Result<Vec<NodeId>, Error> {
    Ok(view.node_slice(slice)?.iter().flatten().collect())
}

/// Go's `Node.Members()`, which panics outside the member-list kinds.
fn members(view: AstView<'_>, node: NodeId) -> Result<Vec<NodeId>, Error> {
    let read = view.node(node)?;
    match read.kind().known() {
        Some(
            K::ClassDeclaration
            | K::ClassExpression
            | K::InterfaceDeclaration
            | K::EnumDeclaration
            | K::TypeLiteral
            | K::MappedType,
        ) => nodes(view, read.members(view)?),
        _ => panic!("Unhandled case in Node.MemberList: {}", read.kind()),
    }
}

/// port: tsc/internal/ast/utilities.go:ChildIsDecorated
pub fn child_is_decorated(
    view: AstView<'_>,
    use_legacy_decorators: bool,
    node: NodeId,
    parent: Option<NodeId>,
) -> Result<bool, Error> {
    let read = view.node(node)?;
    match read.kind().known() {
        Some(K::ClassDeclaration | K::ClassExpression) => {
            for member in members(view, node)? {
                if node_or_child_is_decorated(
                    view,
                    use_legacy_decorators,
                    member,
                    Some(node),
                    parent,
                )? {
                    return Ok(true);
                }
            }
            Ok(false)
        }
        Some(K::MethodDeclaration | K::SetAccessor | K::Constructor) => {
            for parameter in nodes(view, read.parameters(view)?)? {
                if node_is_decorated(view, use_legacy_decorators, parameter, Some(node), parent)? {
                    return Ok(true);
                }
            }
            Ok(false)
        }
        _ => Ok(false),
    }
}

/// port: tsc/internal/ast/utilities.go:ClassElementOrClassElementParameterIsDecorated
pub fn class_element_or_class_element_parameter_is_decorated(
    view: AstView<'_>,
    use_legacy_decorators: bool,
    node: NodeId,
    parent: Option<NodeId>,
) -> Result<bool, Error> {
    let read = view.node(node)?;
    let mut parameters = None;
    if crate::utilities::is_accessor(&read) {
        let declarations =
            get_all_accessor_declarations(view, &members(view, parent.expect(NIL))?, node)?;
        let first = declarations.first_accessor.expect(NIL);
        let first_accessor_with_decorators =
            if crate::utilities_middle::has_decorators(view, &view.node(first)?)? {
                Some(first)
            } else {
                match declarations.second_accessor {
                    Some(second)
                        if crate::utilities_middle::has_decorators(view, &view.node(second)?)? =>
                    {
                        Some(second)
                    }
                    _ => None,
                }
            };
        if first_accessor_with_decorators != Some(node) {
            return Ok(false);
        }
        if let Some(set_accessor) = declarations.set_accessor {
            parameters = view.node(set_accessor)?.parameter_list();
        }
    } else if read.kind() == K::MethodDeclaration {
        parameters = read.parameter_list();
    }
    if node_is_decorated(view, use_legacy_decorators, node, parent, None)? {
        return Ok(true);
    }
    if let Some(parameters) = parameters {
        for parameter in nodes(view, view.list(parameters)?.nodes())? {
            if is_this_parameter(view, parameter)? {
                continue;
            }
            if node_is_decorated(view, use_legacy_decorators, parameter, Some(node), parent)? {
                return Ok(true);
            }
        }
    }
    Ok(false)
}

/// port: tsc/internal/ast/utilities.go:ClassOrConstructorParameterIsDecorated
pub fn class_or_constructor_parameter_is_decorated(
    view: AstView<'_>,
    use_legacy_decorators: bool,
    node: NodeId,
) -> Result<bool, Error> {
    if node_is_decorated(view, use_legacy_decorators, node, None, None)? {
        return Ok(true);
    }
    match get_first_constructor_with_body(view, node)? {
        Some(constructor) => {
            child_is_decorated(view, use_legacy_decorators, constructor, Some(node))
        }
        None => Ok(false),
    }
}

/// Go's `AllAccessorDeclarations`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct AllAccessorDeclarations {
    pub first_accessor: Option<NodeId>,
    pub second_accessor: Option<NodeId>,
    pub set_accessor: Option<NodeId>,
    pub get_accessor: Option<NodeId>,
}

/// Go's `GetAllAccessorDeclarations`. The port marker is on the dynamic-name
/// test, a site the mutation splicer can negate (a struct result has no sound
/// replacement value).
pub fn get_all_accessor_declarations(
    view: AstView<'_>,
    parent_declarations: &[NodeId],
    accessor: NodeId,
) -> Result<AllAccessorDeclarations, Error> {
    // port: tsc/internal/ast/utilities.go:GetAllAccessorDeclarations
    if crate::binder_helpers::has_dynamic_name(view, Some(accessor))? {
        return get_all_accessor_declarations_for_declaration(view, accessor, &[accessor]);
    }
    let name = |node: NodeId| -> Result<Vec<u8>, Error> {
        crate::utilities_targets::get_property_name_for_property_name_node(
            view,
            view.node(node)?.name().expect(NIL),
        )
    };
    let accessor_name = name(accessor)?;
    let accessor_static = crate::utilities::is_static(view, accessor)?;
    let mut matches = Vec::new();
    for &member in parent_declarations {
        if !crate::utilities::is_accessor(&view.node(member)?)
            || crate::utilities::is_static(view, member)? != accessor_static
        {
            continue;
        }
        if name(member)? == accessor_name {
            matches.push(member);
        }
    }
    get_all_accessor_declarations_for_declaration(view, accessor, &matches)
}

/// Go's `GetAllAccessorDeclarationsForDeclaration`. The port marker is on the
/// order test, a site the mutation splicer can negate (a struct result has no
/// sound replacement value).
pub fn get_all_accessor_declarations_for_declaration(
    view: AstView<'_>,
    accessor: NodeId,
    declarations_of_symbol: &[NodeId],
) -> Result<AllAccessorDeclarations, Error> {
    let read = view.node(accessor)?;
    let other_kind = match read.kind().known() {
        Some(K::SetAccessor) => K::GetAccessor,
        Some(K::GetAccessor) => K::SetAccessor,
        _ => panic!("Unexpected node kind {:?}", read.kind()),
    };
    let mut other_accessor = None;
    for &declaration in declarations_of_symbol {
        if view.node(declaration)?.kind() == other_kind {
            other_accessor = Some(declaration);
            break;
        }
    }
    let (mut first_accessor, mut second_accessor) = (Some(accessor), other_accessor);
    let other_first = match other_accessor {
        Some(other) => view.node(other)?.pos() < read.pos(),
        None => false,
    };
    // port: tsc/internal/ast/utilities.go:GetAllAccessorDeclarationsForDeclaration
    if other_first {
        (first_accessor, second_accessor) = (other_accessor, Some(accessor));
    }
    let (set_accessor, get_accessor) = if read.kind() == K::SetAccessor {
        (Some(accessor), other_accessor)
    } else {
        (other_accessor, Some(accessor))
    };
    Ok(AllAccessorDeclarations {
        first_accessor,
        second_accessor,
        set_accessor,
        get_accessor,
    })
}

/// port: tsc/internal/ast/utilities.go:GetClassExtendsHeritageElement
pub fn get_class_extends_heritage_element(
    view: AstView<'_>,
    node: NodeId,
) -> Result<Option<NodeId>, Error> {
    Ok(get_heritage_elements(view, node, K::ExtendsKeyword)?
        .first()
        .copied())
}

/// port: tsc/internal/ast/utilities.go:GetClassLikeDeclarationOfSymbol
pub fn get_class_like_declaration_of_symbol(
    view: AstView<'_>,
    bound: BoundView<'_>,
    symbol: SymbolId,
) -> Result<Option<NodeId>, Error> {
    let symbol = bound.symbol(symbol)?;
    for declaration in bound
        .result()
        .declarations()
        .get(symbol.declarations())?
        .iter()
        .flatten()
    {
        if crate::utilities::is_class_like(&view.node(declaration)?) {
            return Ok(Some(declaration));
        }
    }
    Ok(None)
}

/// port: tsc/internal/ast/utilities.go:GetExtendsHeritageClauseElements
pub fn get_extends_heritage_clause_elements(
    view: AstView<'_>,
    node: NodeId,
) -> Result<Vec<NodeId>, Error> {
    get_heritage_elements(view, node, K::ExtendsKeyword)
}

/// port: tsc/internal/ast/utilities.go:GetFirstConstructorWithBody
pub fn get_first_constructor_with_body(
    view: AstView<'_>,
    node: NodeId,
) -> Result<Option<NodeId>, Error> {
    for member in members(view, node)? {
        let read = view.node(member)?;
        if read.kind() == K::Constructor {
            let body = read.body().map(|body| view.node(body)).transpose()?;
            if crate::binder_helpers::node_is_present(body.as_ref()) {
                return Ok(Some(member));
            }
        }
    }
    Ok(None)
}

/// port: tsc/internal/ast/utilities.go:GetHeritageClause
pub fn get_heritage_clause(
    view: AstView<'_>,
    node: NodeId,
    kind: K,
) -> Result<Option<NodeId>, Error> {
    if let Some(clauses) = get_heritage_clauses(view, node)? {
        for clause in nodes(view, view.list(clauses)?.nodes())? {
            let token = view
                .node(clause)?
                .data_source()
                .as_heritage_clause()
                .ok_or(Error::InvalidGraph)?
                .token();
            if token == kind {
                return Ok(Some(clause));
            }
        }
    }
    Ok(None)
}

/// port: tsc/internal/ast/utilities.go:GetHeritageClauseElementName
pub fn get_heritage_clause_element_name(
    view: AstView<'_>,
    node: NodeId,
) -> Result<Option<NodeId>, Error> {
    let read = view.node(node)?;
    if read.kind() == K::TypeReference {
        return Ok(read
            .data_source()
            .as_type_reference_node()
            .ok_or(Error::InvalidGraph)?
            .type_name());
    }
    Ok(read
        .data_source()
        .as_expression_with_type_arguments()
        .expect("interface conversion: the node is not an ExpressionWithTypeArguments")
        .expression())
}

/// port: tsc/internal/ast/utilities.go:GetHeritageElements
pub fn get_heritage_elements(
    view: AstView<'_>,
    node: NodeId,
    kind: K,
) -> Result<Vec<NodeId>, Error> {
    let Some(clause) = get_heritage_clause(view, node, kind)? else {
        return Ok(Vec::new());
    };
    let types = view
        .node(clause)?
        .data_source()
        .as_heritage_clause()
        .ok_or(Error::InvalidGraph)?
        .types()
        .expect(NIL);
    nodes(view, view.list(types)?.nodes())
}

/// port: tsc/internal/ast/utilities.go:GetImplementsHeritageClauseElements
pub fn get_implements_heritage_clause_elements(
    view: AstView<'_>,
    node: NodeId,
) -> Result<Vec<NodeId>, Error> {
    get_heritage_elements(view, node, K::ImplementsKeyword)
}

/// port: tsc/internal/ast/utilities.go:GetThisParameter
pub fn get_this_parameter(view: AstView<'_>, signature: NodeId) -> Result<Option<NodeId>, Error> {
    let parameters = nodes(view, view.node(signature)?.parameters(view)?)?;
    if let Some(&first) = parameters.first() {
        if is_this_parameter(view, first)? {
            return Ok(Some(first));
        }
    }
    Ok(None)
}

/// port: tsc/internal/ast/utilities.go:HasAbstractModifier
pub fn has_abstract_modifier(view: AstView<'_>, node: NodeId) -> Result<bool, Error> {
    crate::utilities::has_syntactic_modifier(view, node, modifier_flags::ABSTRACT)
}

/// port: tsc/internal/ast/utilities.go:HasAmbientModifier
pub fn has_ambient_modifier(view: AstView<'_>, node: NodeId) -> Result<bool, Error> {
    crate::utilities::has_syntactic_modifier(view, node, modifier_flags::AMBIENT)
}

/// port: tsc/internal/ast/utilities.go:IsClassOrTypeElement
pub fn is_class_or_type_element(node: &(impl NodeAccess + ?Sized)) -> bool {
    crate::utilities::is_class_element(node) || crate::utilities::is_type_element(node)
}

/// port: tsc/internal/ast/utilities.go:IsExpressionWithTypeArgumentsInClassExtendsClause
pub fn is_expression_with_type_arguments_in_class_extends_clause(
    view: AstView<'_>,
    node: NodeId,
) -> Result<bool, Error> {
    Ok(try_get_class_extending_expression_with_type_arguments(view, node)?.is_some())
}

/// Go reads `node.Parent` unconditionally, so a parentless node panics.
/// port: tsc/internal/ast/utilities.go:IsNameOfHeritageClauseTypeReference
pub fn is_name_of_heritage_clause_type_reference(
    view: AstView<'_>,
    node: NodeId,
) -> Result<bool, Error> {
    let mut current = node;
    loop {
        let parent = view.node(current)?.parent().expect(NIL);
        if !crate::is_qualified_name(&view.node(parent)?) {
            break;
        }
        current = parent;
    }
    let parent_id = view.node(current)?.parent().expect(NIL);
    let parent = view.node(parent_id)?;
    if parent.kind() != K::TypeReference
        || parent
            .data_source()
            .as_type_reference_node()
            .ok_or(Error::InvalidGraph)?
            .type_name()
            != Some(current)
    {
        return Ok(false);
    }
    Ok(crate::is_heritage_clause(
        &view.node(parent.parent().expect(NIL))?,
    ))
}

/// port: tsc/internal/ast/utilities.go:IsThisParameter
pub fn is_this_parameter(view: AstView<'_>, node: NodeId) -> Result<bool, Error> {
    let read = view.node(node)?;
    Ok(read.kind() == K::Parameter
        && read
            .name()
            .is_some_and(|name| view.is_this_identifier(name)))
}

/// port: tsc/internal/ast/utilities.go:NodeCanBeDecorated
pub fn node_can_be_decorated(
    view: AstView<'_>,
    use_legacy_decorators: bool,
    node: NodeId,
    parent: Option<NodeId>,
    grandparent: Option<NodeId>,
) -> Result<bool, Error> {
    let read = view.node(node)?;
    if use_legacy_decorators {
        if let Some(name) = read.name() {
            if view.node(name)?.kind() == K::PrivateIdentifier {
                return Ok(false);
            }
        }
    }
    let parent_read = parent.map(|parent| view.node(parent)).transpose()?;
    Ok(match read.kind().known() {
        Some(K::ClassDeclaration) => true,
        Some(K::ClassExpression) => !use_legacy_decorators,
        Some(K::PropertyDeclaration) => match &parent_read {
            None => false,
            Some(parent) => {
                use_legacy_decorators && parent.kind() == K::ClassDeclaration
                    || !use_legacy_decorators
                        && crate::utilities::is_class_like(parent)
                        && !has_abstract_modifier(view, node)?
                        && !has_ambient_modifier(view, node)?
            }
        },
        Some(K::GetAccessor | K::SetAccessor | K::MethodDeclaration) => match &parent_read {
            None => false,
            Some(parent) => {
                read.body().is_some()
                    && (use_legacy_decorators && parent.kind() == K::ClassDeclaration
                        || !use_legacy_decorators && crate::utilities::is_class_like(parent))
            }
        },
        Some(K::Parameter) => {
            if !use_legacy_decorators {
                return Ok(false);
            }
            match (parent, &parent_read) {
                (Some(parent), Some(parent_read)) => {
                    parent_read.body().is_some()
                        && matches!(
                            parent_read.kind().known(),
                            Some(K::Constructor | K::MethodDeclaration | K::SetAccessor)
                        )
                        && get_this_parameter(view, parent)? != Some(node)
                        && match grandparent {
                            Some(grandparent) => {
                                view.node(grandparent)?.kind() == K::ClassDeclaration
                            }
                            None => false,
                        }
                }
                _ => false,
            }
        }
        _ => false,
    })
}

/// port: tsc/internal/ast/utilities.go:NodeIsDecorated
pub fn node_is_decorated(
    view: AstView<'_>,
    use_legacy_decorators: bool,
    node: NodeId,
    parent: Option<NodeId>,
    grandparent: Option<NodeId>,
) -> Result<bool, Error> {
    Ok(
        crate::utilities_middle::has_decorators(view, &view.node(node)?)?
            && node_can_be_decorated(view, use_legacy_decorators, node, parent, grandparent)?,
    )
}

/// port: tsc/internal/ast/utilities.go:NodeOrChildIsDecorated
pub fn node_or_child_is_decorated(
    view: AstView<'_>,
    use_legacy_decorators: bool,
    node: NodeId,
    parent: Option<NodeId>,
    grandparent: Option<NodeId>,
) -> Result<bool, Error> {
    Ok(
        node_is_decorated(view, use_legacy_decorators, node, parent, grandparent)?
            || child_is_decorated(view, use_legacy_decorators, node, parent)?,
    )
}

/// port: tsc/internal/ast/utilities.go:TryGetClassExtendingExpressionWithTypeArguments
pub fn try_get_class_extending_expression_with_type_arguments(
    view: AstView<'_>,
    node: NodeId,
) -> Result<Option<NodeId>, Error> {
    if view.node(node)?.kind() != K::ExpressionWithTypeArguments {
        return Ok(None);
    }
    Ok(
        match try_get_class_implementing_or_extending_heritage_clause_element(view, node)? {
            (Some(class), false) => Some(class),
            _ => None,
        },
    )
}

/// port: tsc/internal/ast/utilities.go:TryGetClassImplementingOrExtendingHeritageClauseElement
pub fn try_get_class_implementing_or_extending_heritage_clause_element(
    view: AstView<'_>,
    node: NodeId,
) -> Result<(Option<NodeId>, bool), Error> {
    let read = view.node(node)?;
    if matches!(
        read.kind().known(),
        Some(K::ExpressionWithTypeArguments | K::TypeReference)
    ) {
        let parent_id = read.parent().expect(NIL);
        let parent = view.node(parent_id)?;
        if crate::is_heritage_clause(&parent) {
            let grandparent = parent.parent().expect(NIL);
            if crate::utilities::is_class_like(&view.node(grandparent)?) {
                let token = parent
                    .data_source()
                    .as_heritage_clause()
                    .ok_or(Error::InvalidGraph)?
                    .token();
                return Ok((Some(grandparent), token == K::ImplementsKeyword));
            }
        }
    }
    Ok((None, false))
}

/// port: tsc/internal/ast/utilities.go:getHeritageClauses
fn get_heritage_clauses(view: AstView<'_>, node: NodeId) -> Result<Option<NodeListId>, Error> {
    let read = view.node(node)?;
    let data = read.data_source();
    Ok(match read.kind().known() {
        Some(K::ClassDeclaration) => data
            .as_class_declaration()
            .ok_or(Error::InvalidGraph)?
            .heritage_clauses(),
        Some(K::ClassExpression) => data
            .as_class_expression()
            .ok_or(Error::InvalidGraph)?
            .heritage_clauses(),
        Some(K::InterfaceDeclaration) => data
            .as_interface_declaration()
            .ok_or(Error::InvalidGraph)?
            .heritage_clauses(),
        _ => None,
    })
}

/// port: tsc/internal/ast/utilities.go:ReplaceModifiers
pub fn replace_modifiers<F: crate::Factory + ?Sized>(
    factory: &mut F,
    node: NodeId,
    modifier_array: Option<NodeListId>,
) -> NodeId {
    use crate::FactoryMethods;
    const PAYLOAD: &str = "ReplaceModifiers reads the payload of its node's kind";
    let read = factory.node(node);
    let kind = read.kind();
    let data = read.data_source();
    // Each arm reads only its kind's fields, as Go does: several accessors
    // panic on kinds without the field.
    macro_rules! read {
        ($($field:ident),*) => { $(let $field = read!(@ $field);)* };
        (@ name) => { read.name() };
        (@ type_node) => { read.type_node() };
        (@ body) => { read.body() };
        (@ initializer) => { read.initializer() };
        (@ postfix) => { read.postfix_token() };
        (@ type_parameters) => { read.type_parameter_list() };
        (@ parameters) => { read.parameter_list() };
        (@ member_list) => { read.member_list() };
    }
    match kind.known() {
        Some(K::TypeParameter) => {
            read!(name);
            let payload = data.as_type_parameter_declaration().expect(PAYLOAD);
            let (constraint, expression, default_type) = (
                payload.constraint(),
                payload.expression(),
                payload.default_type(),
            );
            drop(read);
            factory.update_type_parameter_declaration(
                node,
                modifier_array,
                name,
                constraint,
                expression,
                default_type,
            )
        }
        Some(K::Parameter) => {
            read!(name, type_node, initializer);
            let payload = data.as_parameter_declaration().expect(PAYLOAD);
            let (dot_dot_dot, question) = (payload.dot_dot_dot_token(), payload.question_token());
            drop(read);
            factory.update_parameter_declaration(
                node,
                modifier_array,
                dot_dot_dot,
                name,
                question,
                type_node,
                initializer,
            )
        }
        Some(K::ConstructorType) => {
            read!(type_node, type_parameters, parameters);
            drop(read);
            factory.update_constructor_type_node(
                node,
                modifier_array,
                type_parameters,
                parameters,
                type_node,
            )
        }
        Some(K::PropertySignature) => {
            read!(name, type_node, initializer, postfix);
            drop(read);
            factory.update_property_signature_declaration(
                node,
                modifier_array,
                name,
                postfix,
                type_node,
                initializer,
            )
        }
        Some(K::PropertyDeclaration) => {
            read!(name, type_node, initializer, postfix);
            drop(read);
            factory.update_property_declaration(
                node,
                modifier_array,
                name,
                postfix,
                type_node,
                initializer,
            )
        }
        Some(K::MethodSignature) => {
            read!(name, type_node, postfix, type_parameters, parameters);
            drop(read);
            factory.update_method_signature_declaration(
                node,
                modifier_array,
                name,
                postfix,
                type_parameters,
                parameters,
                type_node,
            )
        }
        Some(K::MethodDeclaration) => {
            read!(name, type_node, body, postfix, type_parameters, parameters);
            let payload = data.as_method_declaration().expect(PAYLOAD);
            let (asterisk, full_signature) = (payload.asterisk_token(), payload.full_signature());
            drop(read);
            factory.update_method_declaration(
                node,
                modifier_array,
                asterisk,
                name,
                postfix,
                type_parameters,
                parameters,
                type_node,
                full_signature,
                body,
            )
        }
        Some(K::Constructor) => {
            read!(type_node, body, type_parameters, parameters);
            let full_signature = data
                .as_constructor_declaration()
                .expect(PAYLOAD)
                .full_signature();
            drop(read);
            factory.update_constructor_declaration(
                node,
                modifier_array,
                type_parameters,
                parameters,
                type_node,
                full_signature,
                body,
            )
        }
        Some(K::GetAccessor) => {
            read!(name, type_node, body, type_parameters, parameters);
            let full_signature = data
                .as_get_accessor_declaration()
                .expect(PAYLOAD)
                .full_signature();
            drop(read);
            factory.update_get_accessor_declaration(
                node,
                modifier_array,
                name,
                type_parameters,
                parameters,
                type_node,
                full_signature,
                body,
            )
        }
        Some(K::SetAccessor) => {
            read!(name, type_node, body, type_parameters, parameters);
            let full_signature = data
                .as_set_accessor_declaration()
                .expect(PAYLOAD)
                .full_signature();
            drop(read);
            factory.update_set_accessor_declaration(
                node,
                modifier_array,
                name,
                type_parameters,
                parameters,
                type_node,
                full_signature,
                body,
            )
        }
        Some(K::IndexSignature) => {
            read!(type_node, parameters);
            drop(read);
            factory.update_index_signature_declaration(node, modifier_array, parameters, type_node)
        }
        Some(K::FunctionExpression) => {
            read!(name, type_node, body, type_parameters, parameters);
            let payload = data.as_function_expression().expect(PAYLOAD);
            let (asterisk, full_signature) = (payload.asterisk_token(), payload.full_signature());
            drop(read);
            factory.update_function_expression(
                node,
                modifier_array,
                asterisk,
                name,
                type_parameters,
                parameters,
                type_node,
                full_signature,
                body,
            )
        }
        Some(K::ArrowFunction) => {
            read!(type_node, body, type_parameters, parameters);
            let payload = data.as_arrow_function().expect(PAYLOAD);
            let (full_signature, arrow) = (
                payload.full_signature(),
                payload.equals_greater_than_token(),
            );
            drop(read);
            factory.update_arrow_function(
                node,
                modifier_array,
                type_parameters,
                parameters,
                type_node,
                full_signature,
                arrow,
                body,
            )
        }
        Some(K::ClassExpression) => {
            read!(name, type_parameters, member_list);
            let heritage = data
                .as_class_expression()
                .expect(PAYLOAD)
                .heritage_clauses();
            drop(read);
            factory.update_class_expression(
                node,
                modifier_array,
                name,
                type_parameters,
                heritage,
                member_list,
            )
        }
        Some(K::VariableStatement) => {
            let list = data
                .as_variable_statement()
                .expect(PAYLOAD)
                .declaration_list();
            drop(read);
            factory.update_variable_statement(node, modifier_array, list)
        }
        Some(K::FunctionDeclaration) => {
            read!(name, type_node, body, type_parameters, parameters);
            let payload = data.as_function_declaration().expect(PAYLOAD);
            let (asterisk, full_signature) = (payload.asterisk_token(), payload.full_signature());
            drop(read);
            factory.update_function_declaration(
                node,
                modifier_array,
                asterisk,
                name,
                type_parameters,
                parameters,
                type_node,
                full_signature,
                body,
            )
        }
        Some(K::ClassDeclaration) => {
            read!(name, type_parameters, member_list);
            let heritage = data
                .as_class_declaration()
                .expect(PAYLOAD)
                .heritage_clauses();
            drop(read);
            factory.update_class_declaration(
                node,
                modifier_array,
                name,
                type_parameters,
                heritage,
                member_list,
            )
        }
        Some(K::InterfaceDeclaration) => {
            read!(name, type_parameters, member_list);
            let heritage = data
                .as_interface_declaration()
                .expect(PAYLOAD)
                .heritage_clauses();
            drop(read);
            factory.update_interface_declaration(
                node,
                modifier_array,
                name,
                type_parameters,
                heritage,
                member_list,
            )
        }
        Some(K::TypeAliasDeclaration) => {
            read!(name, type_node, type_parameters);
            drop(read);
            factory.update_type_alias_declaration(
                node,
                modifier_array,
                name,
                type_parameters,
                type_node,
            )
        }
        Some(K::EnumDeclaration) => {
            read!(name, member_list);
            drop(read);
            factory.update_enum_declaration(node, modifier_array, name, member_list)
        }
        Some(K::ModuleDeclaration) => {
            read!(name, body);
            let keyword = data.as_module_declaration().expect(PAYLOAD).keyword();
            let attributes = read.attributes();
            drop(read);
            factory.update_module_declaration(node, modifier_array, keyword, name, attributes, body)
        }
        Some(K::ImportEqualsDeclaration) => {
            read!(name);
            let reference = data
                .as_import_equals_declaration()
                .expect(PAYLOAD)
                .module_reference();
            let is_type_only = read.is_type_only();
            drop(read);
            factory.update_import_equals_declaration(
                node,
                modifier_array,
                is_type_only,
                name,
                reference,
            )
        }
        Some(K::ImportDeclaration) => {
            let payload = data.as_import_declaration().expect(PAYLOAD);
            let attributes = payload.attributes();
            let (clause, specifier) = (read.import_clause(), read.module_specifier());
            drop(read);
            factory.update_import_declaration(node, modifier_array, clause, specifier, attributes)
        }
        Some(K::ExportAssignment) => {
            read!(type_node);
            let is_export_equals = data
                .as_export_assignment()
                .expect(PAYLOAD)
                .is_export_equals();
            let expression = read.expression();
            drop(read);
            factory.update_export_assignment(
                node,
                modifier_array,
                is_export_equals,
                type_node,
                expression,
            )
        }
        Some(K::ExportDeclaration) => {
            let payload = data.as_export_declaration().expect(PAYLOAD);
            let (clause, attributes) = (payload.export_clause(), payload.attributes());
            let (is_type_only, specifier) = (read.is_type_only(), read.module_specifier());
            drop(read);
            factory.update_export_declaration(
                node,
                modifier_array,
                is_type_only,
                clause,
                specifier,
                attributes,
            )
        }
        _ => panic!(
            "Node that does not have modifiers tried to have modifier replaced: {}",
            kind.raw()
        ),
    }
}

/// port: tsc/internal/ast/ast.go:Node.Decorators
pub fn decorators(view: AstView<'_>, node: NodeId) -> Result<Vec<NodeId>, Error> {
    let Some(modifiers) = view.node(node)?.modifiers() else {
        return Ok(Vec::new());
    };
    let mut out = Vec::new();
    for modifier in nodes(view, view.list(modifiers)?.nodes())? {
        if crate::is_decorator(&view.node(modifier)?) {
            out.push(modifier);
        }
    }
    Ok(out)
}
