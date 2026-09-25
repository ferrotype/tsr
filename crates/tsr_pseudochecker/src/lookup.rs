use crate::{
    types::{
        bigint, bigint_literal, boolean, direct, false_type, inferred, inferred_with_errors,
        maybe_const_location, no_result, null, number, numeric_literal, object_literal,
        single_call_signature, string, string_literal, true_type, tuple, undefined, union,
        PseudoObjectElement, PseudoObjectElementData, PseudoParameter, PseudoSignature, PseudoType,
        PseudoTypeData, PseudoTypeKind,
    },
    Error, Host, PseudoChecker,
};
use tsr_arena::NodeId;
use tsr_ast::{
    modifier_flags as mf, node_flags as nf, AstView, NodeListId, NodeRead, SyntaxKind as K,
};

type R<T, H> = Result<T, <H as Host>::Error>;
struct Lookup<'a, H: ?Sized> {
    checker: PseudoChecker,
    host: &'a H,
}
#[derive(Clone, Copy)]
struct Accessors {
    first: NodeId,
    second: Option<NodeId>,
    get: Option<NodeId>,
    set: Option<NodeId>,
}

impl PseudoChecker {
    // port: tsc/internal/pseudochecker/lookup.go:PseudoChecker.GetReturnTypeOfSignature
    pub fn get_return_type_of_signature<H: Host + ?Sized>(
        &self,
        host: &H,
        node: NodeId,
    ) -> R<PseudoType, H> {
        let lookup = Lookup {
            checker: *self,
            host,
        };
        match lookup.node(node)?.kind().known() {
            Some(K::GetAccessor) => lookup.type_from_accessor(node),
            Some(
                K::MethodDeclaration
                | K::FunctionDeclaration
                | K::Constructor
                | K::MethodSignature
                | K::CallSignature
                | K::ConstructSignature
                | K::SetAccessor
                | K::IndexSignature
                | K::FunctionType
                | K::ConstructorType
                | K::FunctionExpression
                | K::ArrowFunction
                | K::JSDocSignature,
            ) => lookup.create_return_from_signature(node),
            _ => Err(lookup.bad_kind(node, "GetReturnTypeOfSignature")?),
        }
    }
    // port: tsc/internal/pseudochecker/lookup.go:PseudoChecker.GetTypeOfAccessor
    pub fn get_type_of_accessor<H: Host + ?Sized>(
        &self,
        host: &H,
        node: NodeId,
    ) -> R<PseudoType, H> {
        Lookup {
            checker: *self,
            host,
        }
        .type_from_accessor(node)
    }
    // port: tsc/internal/pseudochecker/lookup.go:PseudoChecker.GetTypeOfExpression
    pub fn get_type_of_expression<H: Host + ?Sized>(
        &self,
        host: &H,
        node: NodeId,
    ) -> R<PseudoType, H> {
        Lookup {
            checker: *self,
            host,
        }
        .type_from_expression(node)
    }
    // port: tsc/internal/pseudochecker/lookup.go:PseudoChecker.GetTypeOfDeclaration
    pub fn get_type_of_declaration<H: Host + ?Sized>(
        &self,
        host: &H,
        node: NodeId,
    ) -> R<PseudoType, H> {
        Lookup {
            checker: *self,
            host,
        }
        .type_from_declaration(node)
    }
}

// port: tsc/internal/pseudochecker/lookup.go:IsInConstContext
pub fn is_in_const_context<H: Host + ?Sized>(host: &H, node: NodeId) -> R<bool, H> {
    Lookup {
        checker: PseudoChecker::new(false, false),
        host,
    }
    .is_in_const_context(node)
}
// port: tsc/internal/pseudochecker/lookup.go:CouldAlreadyReferToUndefinedType
pub fn could_already_refer_to_undefined_type<H: Host + ?Sized>(
    host: &H,
    ty: &PseudoType,
) -> R<bool, H> {
    Lookup {
        checker: PseudoChecker::new(false, false),
        host,
    }
    .could_refer_to_undefined(ty)
}

impl<H: Host + ?Sized> Lookup<'_, H> {
    fn ast(&self, node: NodeId) -> R<AstView<'_>, H> {
        self.host.ast(node)
    }
    fn node(&self, node: NodeId) -> R<NodeRead<'_>, H> {
        Ok(self.ast(node)?.node(node)?)
    }
    fn required<T>(value: Option<T>, context: &'static str) -> R<T, H> {
        value.ok_or_else(|| Error::MissingLink(context).into())
    }
    fn bad_kind(&self, node: NodeId, operation: &'static str) -> R<H::Error, H> {
        Ok(Error::InvalidKind {
            node,
            kind: self.node(node)?.kind(),
            operation,
        }
        .into())
    }
    fn list(&self, node: NodeId, list: Option<NodeListId>) -> R<Vec<NodeId>, H> {
        let Some(list) = list else {
            return Ok(Vec::new());
        };
        let view = self.ast(node)?;
        Ok(view
            .node_slice(view.list(list)?.nodes())?
            .iter()
            .flatten()
            .collect())
    }
    fn parameters(&self, node: NodeId) -> R<Vec<NodeId>, H> {
        self.list(node, self.node(node)?.parameter_list())
    }
    fn type_parameters(&self, node: NodeId) -> R<Vec<NodeId>, H> {
        self.list(node, self.node(node)?.type_parameter_list())
    }
    fn name(&self, node: NodeId) -> R<NodeId, H> {
        Self::required(self.node(node)?.name(), "pseudo declaration name")
    }
    fn expression(&self, node: NodeId) -> R<NodeId, H> {
        Self::required(self.node(node)?.expression(), "pseudo expression")
    }
    fn is_const_assertion(&self, node: NodeId) -> R<bool, H> {
        let read = self.node(node)?;
        if !matches!(
            read.kind().known(),
            Some(K::AsExpression | K::TypeAssertionExpression)
        ) {
            return Ok(false);
        }
        let annotation = Self::required(read.type_node(), "pseudo assertion type")?;
        self.is_const_type_reference(annotation)
    }
    fn is_const_type_reference(&self, node: NodeId) -> R<bool, H> {
        Ok(tsr_ast::utilities_middle::is_const_type_reference(
            self.ast(node)?,
            &self.node(node)?,
        )?)
    }
    fn full_signature(&self, node: NodeId) -> R<Option<NodeId>, H> {
        let read = self.node(node)?;
        let data = read.data_source();
        Ok(match read.kind().known() {
            Some(K::FunctionDeclaration) => data
                .as_function_declaration()
                .and_then(|d| d.full_signature()),
            Some(K::FunctionExpression) => data
                .as_function_expression()
                .and_then(|d| d.full_signature()),
            Some(K::ArrowFunction) => data.as_arrow_function().and_then(|d| d.full_signature()),
            Some(K::MethodDeclaration) => data
                .as_method_declaration()
                .and_then(|d| d.full_signature()),
            _ => None,
        })
    }
    fn type_from_declaration(&self, node: NodeId) -> R<PseudoType, H> {
        let read = self.node(node)?;
        match read.kind().known() {
            Some(K::Parameter) => self.type_from_parameter(node),
            Some(K::VariableDeclaration) => self.type_from_variable(node),
            Some(K::PropertySignature | K::PropertyDeclaration | K::JSDocPropertyTag) => {
                self.type_from_property(node)
            }
            Some(K::BindingElement | K::CallExpression) => Ok(no_result(node)),
            Some(K::ExportAssignment) => self.type_from_expression(self.expression(node)?),
            Some(
                K::PropertyAccessExpression | K::ElementAccessExpression | K::BinaryExpression,
            ) => Ok(read.type_node().map_or_else(|| no_result(node), direct)),
            Some(K::PropertyAssignment | K::ShorthandPropertyAssignment) => {
                self.type_from_property_assignment(node)
            }
            _ => Err(self.bad_kind(node, "GetTypeOfDeclaration")?),
        }
    }
    // port: tsc/internal/pseudochecker/lookup.go:PseudoChecker.typeFromPropertyAssignment
    fn type_from_property_assignment(&self, node: NodeId) -> R<PseudoType, H> {
        let read = self.node(node)?;
        if let Some(ty) = read.type_node() {
            return Ok(direct(ty));
        }
        if read.kind() == K::PropertyAssignment {
            if let Some(init) = read.initializer() {
                let ty = self.type_from_expression(init)?;
                if usable(&ty) {
                    return Ok(ty);
                }
            }
        }
        Ok(no_result(node))
    }
    // port: tsc/internal/pseudochecker/lookup.go:PseudoChecker.typeFromProperty
    fn type_from_property(&self, node: NodeId) -> R<PseudoType, H> {
        let read = self.node(node)?;
        if let Some(ty) = read.type_node() {
            return Ok(direct(ty));
        }
        if read.kind() == K::PropertyDeclaration {
            if let Some(init) = read.initializer() {
                if !self.contextually_typed(node)? {
                    if read.modifier_flags(self.ast(node)?)? & mf::READONLY != 0
                        && self.node(init)?.kind() == K::TemplateExpression
                    {
                        return Ok(no_result(node));
                    }
                    let ty = self.type_from_expression(init)?;
                    if usable(&ty) {
                        if ty.kind() != PseudoTypeKind::Direct
                            && read.question_token(self.ast(node)?)?.is_some()
                        {
                            return self.add_undefined(ty);
                        }
                        return Ok(ty);
                    }
                }
            }
        }
        Ok(no_result(node))
    }
    // port: tsc/internal/pseudochecker/lookup.go:PseudoChecker.typeFromVariable
    fn type_from_variable(&self, node: NodeId) -> R<PseudoType, H> {
        let read = self.node(node)?;
        if let Some(ty) = read.type_node() {
            return Ok(direct(ty));
        }
        if let Some(init) = read.initializer() {
            if let Some(declarations) = self.host.raw_symbol_declarations(node)? {
                let mut count = 0;
                if declarations.len() != 1 {
                    for decl in &declarations {
                        count += usize::from(self.node(*decl)?.kind() == K::VariableDeclaration);
                    }
                }
                if (declarations.len() == 1 || count == 1) && !self.contextually_typed(node)? {
                    if tsr_ast::utilities::is_var_const(self.ast(node)?, node)?
                        && self.node(init)?.kind() == K::TemplateExpression
                    {
                        return Ok(no_result(node));
                    }
                    let ty = self.type_from_expression(init)?;
                    if usable(&ty) {
                        return Ok(ty);
                    }
                }
            }
        }
        Ok(no_result(node))
    }
    // port: tsc/internal/pseudochecker/lookup.go:PseudoChecker.typeFromAccessor
    fn type_from_accessor(&self, node: NodeId) -> R<PseudoType, H> {
        let accessors = self.accessors(node)?;
        let mut annotation = self.accessor_annotation(Some(node))?;
        if annotation.is_none() && node != accessors.first {
            annotation = self.accessor_annotation(Some(accessors.first))?;
        }
        if annotation.is_none() && accessors.second.is_some_and(|second| second != node) {
            annotation = self.accessor_annotation(accessors.second)?;
        }
        if let Some(annotation) = annotation {
            if self.node(annotation)?.kind() != K::TypePredicate {
                return Ok(direct(annotation));
            }
        }
        if let Some(get) = accessors.get {
            let ty = self.create_return_from_signature(get)?;
            if let PseudoTypeData::Inferred {
                expression,
                error_nodes,
                is_signature_return,
            } = ty.as_ref()
            {
                if error_nodes.is_empty() {
                    let mut errors = vec![get];
                    errors.extend(accessors.set);
                    return Ok(inferred_with_errors(
                        *expression,
                        *is_signature_return,
                        errors,
                    ));
                }
            }
            return Ok(ty);
        }
        Ok(no_result(node))
    }
    fn accessor_annotation(&self, node: Option<NodeId>) -> R<Option<NodeId>, H> {
        let Some(node) = node else {
            return Ok(None);
        };
        let read = self.node(node)?;
        if read.kind() == K::GetAccessor {
            return Ok(read.type_node());
        }
        let Some(&parameter) = self.parameters(node)?.first() else {
            return Ok(None);
        };
        let parameter = self.node(parameter)?;
        Ok(if parameter.kind() == K::Parameter {
            parameter.type_node()
        } else {
            None
        })
    }
    // Inline copy of `GetAllAccessorDeclarationsForDeclaration`; its Phase 1 home is in tsr_ast (table group class).
    fn accessors(&self, node: NodeId) -> R<Accessors, H> {
        let read = self.node(node)?;
        let setter = read.kind() == K::SetAccessor;
        if !setter && read.kind() != K::GetAccessor {
            return Err(self.bad_kind(node, "accessor pairing")?);
        }
        let declarations = Self::required(
            self.host.raw_symbol_declarations(node)?,
            "pseudo accessor symbol",
        )?;
        let other_kind = if setter {
            K::GetAccessor
        } else {
            K::SetAccessor
        };
        let mut other = None;
        for declaration in declarations {
            if self.node(declaration)?.kind() == other_kind {
                other = Some(declaration);
                break;
            }
        }
        let before = match other {
            Some(other) => self.node(other)?.pos() < read.pos(),
            None => false,
        };
        Ok(Accessors {
            first: if before {
                other.expect("other exists when before")
            } else {
                node
            },
            second: if before { Some(node) } else { other },
            get: if setter { other } else { Some(node) },
            set: if setter { Some(node) } else { other },
        })
    }
    // port: tsc/internal/pseudochecker/lookup.go:PseudoChecker.createReturnFromSignature
    fn create_return_from_signature(&self, node: NodeId) -> R<PseudoType, H> {
        let read = self.node(node)?;
        if tsr_ast::utilities::is_function_like(Some(&read)) {
            if let Some(annotation) = read.type_node() {
                return Ok(direct(annotation));
            }
        }
        if matches!(
            read.kind().known(),
            Some(
                K::FunctionExpression
                    | K::ArrowFunction
                    | K::MethodDeclaration
                    | K::GetAccessor
                    | K::SetAccessor
                    | K::FunctionDeclaration
                    | K::Constructor
            )
        ) {
            return self.type_from_single_return(node);
        }
        Ok(no_result(node))
    }
    // port: tsc/internal/pseudochecker/lookup.go:PseudoChecker.typeFromSingleReturnExpression
    fn type_from_single_return(&self, function: NodeId) -> R<PseudoType, H> {
        let mut candidate = None;
        if let Some(body) = self.node(function)?.body() {
            if !tsr_ast::node_is_missing(Some(&self.node(body)?)) {
                if self.async_or_generator(function)? {
                    return Ok(inferred(function, true));
                }
                if self.node(body)?.kind() == K::Block {
                    let mut stack = vec![body];
                    while let Some(node) = stack.pop() {
                        match self.node(node)?.kind().known() {
                            Some(K::ReturnStatement) => {
                                if self.node(node)?.parent() != Some(body) || candidate.is_some() {
                                    candidate = None;
                                    break;
                                }
                                candidate = self.node(node)?.expression();
                            }
                            Some(
                                K::CaseBlock
                                | K::Block
                                | K::IfStatement
                                | K::DoStatement
                                | K::WhileStatement
                                | K::ForStatement
                                | K::ForInStatement
                                | K::ForOfStatement
                                | K::WithStatement
                                | K::SwitchStatement
                                | K::CaseClause
                                | K::DefaultClause
                                | K::LabeledStatement
                                | K::TryStatement
                                | K::CatchClause,
                            ) => stack.extend(self.children(node)?.into_iter().rev()),
                            _ => {}
                        }
                    }
                } else {
                    candidate = Some(body);
                }
            }
        }
        if let Some(candidate) = candidate {
            if self.contextually_typed(candidate)? {
                let read = self.node(candidate)?;
                if matches!(
                    read.kind().known(),
                    Some(K::TypeAssertionExpression | K::AsExpression)
                ) {
                    if let Some(annotation) = read.type_node() {
                        if !self.is_const_type_reference(annotation)? {
                            return Ok(direct(annotation));
                        }
                    }
                }
            } else {
                return self.type_from_expression(candidate);
            }
        }
        Ok(inferred(function, true))
    }
    fn async_or_generator(&self, node: NodeId) -> R<bool, H> {
        let read = self.node(node)?;
        let data = read.data_source();
        let generator = match read.kind().known() {
            Some(K::FunctionDeclaration) => data
                .as_function_declaration()
                .is_some_and(|d| d.asterisk_token().is_some()),
            Some(K::FunctionExpression) => data
                .as_function_expression()
                .is_some_and(|d| d.asterisk_token().is_some()),
            Some(K::MethodDeclaration) => data
                .as_method_declaration()
                .is_some_and(|d| d.asterisk_token().is_some()),
            _ => false,
        };
        Ok(generator
            || matches!(
                read.kind().known(),
                Some(
                    K::FunctionDeclaration
                        | K::FunctionExpression
                        | K::MethodDeclaration
                        | K::ArrowFunction
                )
            ) && read.modifier_flags(self.ast(node)?)? & mf::ASYNC != 0)
    }
    // port: tsc/internal/pseudochecker/lookup.go:PseudoChecker.typeFromExpression
    fn type_from_expression(&self, node: NodeId) -> R<PseudoType, H> {
        stacker::maybe_grow(128 * 1024, 2 * 1024 * 1024, || {
            self.type_from_expression_worker(node)
        })
    }
    fn type_from_expression_worker(&self, node: NodeId) -> R<PseudoType, H> {
        let read = self.node(node)?;
        match read.kind().known() {
            Some(K::OmittedExpression) => return Ok(undefined()),
            Some(K::ParenthesizedExpression) => {
                return self.type_from_expression(self.expression(node)?)
            }
            Some(K::Identifier) => {
                if self.ast(node)?.node_text(node)?.as_bytes() == b"undefined" {
                    return Ok(undefined());
                }
            }
            Some(K::NullKeyword) => return Ok(null()),
            Some(K::ArrowFunction | K::FunctionExpression) => {
                return self.type_from_function_expression(node)
            }
            Some(K::TypeAssertionExpression | K::AsExpression) => {
                let annotation = Self::required(read.type_node(), "pseudo assertion annotation")?;
                return if self.is_const_type_reference(annotation)? {
                    self.type_from_expression(self.expression(node)?)
                } else {
                    Ok(direct(annotation))
                };
            }
            Some(K::PrefixUnaryExpression) => {
                if tsr_ast::utilities_tail::is_primitive_literal_value(
                    self.ast(node)?,
                    &read,
                    true,
                )? {
                    return self.type_from_primitive_prefix(node);
                }
            }
            Some(K::ArrayLiteralExpression) => return self.type_from_array(node),
            Some(K::ObjectLiteralExpression) => return self.type_from_object(node),
            Some(K::ClassExpression) => return Ok(inferred_with_errors(node, false, vec![node])),
            Some(K::TemplateExpression) => {
                return Ok(if self.is_in_const_context(node)? {
                    inferred(node, false)
                } else {
                    maybe_const_location(node, inferred(node, false), string())
                })
            }
            Some(K::NumericLiteral) => {
                return Ok(maybe_const_location(node, numeric_literal(node), number()))
            }
            Some(K::StringLiteral | K::NoSubstitutionTemplateLiteral) => {
                return Ok(maybe_const_location(node, string_literal(node), string()))
            }
            Some(K::BigIntLiteral) => {
                return Ok(maybe_const_location(node, bigint_literal(node), bigint()))
            }
            Some(K::TrueKeyword) => return Ok(maybe_const_location(node, true_type(), boolean())),
            Some(K::FalseKeyword) => {
                return Ok(maybe_const_location(node, false_type(), boolean()))
            }
            _ => {}
        }
        Ok(inferred(node, false))
    }
    fn type_from_primitive_prefix(&self, node: NodeId) -> R<PseudoType, H> {
        let read = self.node(node)?;
        let data = Self::required(
            read.data_source().as_prefix_unary_expression(),
            "pseudo prefix",
        )?;
        let operand = Self::required(data.operand(), "pseudo prefix operand")?;
        let literal = if data.operator() == K::PlusToken {
            operand
        } else {
            node
        };
        let (const_type, regular) = match self.node(operand)?.kind().known() {
            Some(K::NumericLiteral) => (numeric_literal(literal), number()),
            Some(K::BigIntLiteral) => (bigint_literal(literal), bigint()),
            _ => return Err(self.bad_kind(operand, "primitive literal prefix")?),
        };
        Ok(maybe_const_location(node, const_type, regular))
    }
    fn type_from_function_expression(&self, node: NodeId) -> R<PseudoType, H> {
        if let Some(annotation) = self.full_signature(node)? {
            return Ok(direct(annotation));
        }
        let return_type = self.create_return_from_signature(node)?;
        let type_parameters = self.type_parameters(node)?;
        let parameters = self.clone_parameters(node)?;
        Ok(single_call_signature(PseudoSignature {
            signature: node,
            parameters,
            type_parameters,
            return_type,
        }))
    }
    // port: tsc/internal/pseudochecker/lookup.go:PseudoChecker.typeFromObjectLiteral
    fn type_from_object(&self, node: NodeId) -> R<PseudoType, H> {
        let properties = self.list(node, self.node(node)?.property_list())?;
        let errors = self.object_errors(&properties)?;
        if !errors.is_empty() {
            return Ok(inferred_with_errors(node, false, errors));
        }
        let mut elements = Vec::with_capacity(properties.len());
        for property in properties {
            let read = self.node(property)?;
            let name = self.name(property)?;
            let optional = read.question_token(self.ast(property)?)?.is_some();
            match read.kind().known() {
                Some(K::MethodDeclaration) => {
                    if let Some(full) = self.full_signature(property)? {
                        elements.push(PseudoObjectElement::property(
                            false,
                            name,
                            optional,
                            direct(full),
                        ));
                    } else {
                        let type_parameters = self.type_parameters(property)?;
                        let parameters = self.clone_parameters(property)?;
                        let return_type = self.create_return_from_signature(property)?;
                        elements.push(PseudoObjectElement {
                            name,
                            optional,
                            data: PseudoObjectElementData::Method(PseudoSignature {
                                signature: property,
                                parameters,
                                type_parameters,
                                return_type,
                            }),
                        });
                    }
                }
                Some(K::PropertyAssignment) => {
                    let initializer =
                        Self::required(read.initializer(), "pseudo property initializer")?;
                    elements.push(PseudoObjectElement::property(
                        false,
                        name,
                        optional,
                        self.type_from_expression(initializer)?,
                    ));
                }
                Some(K::GetAccessor | K::SetAccessor) => {
                    if let Some(element) = self.accessor_member(property, name)? {
                        elements.push(element);
                    }
                }
                _ => {}
            }
        }
        Ok(object_literal(elements))
    }
    // port: tsc/internal/pseudochecker/lookup.go:PseudoChecker.getAccessorMember
    fn accessor_member(&self, node: NodeId, name: NodeId) -> R<Option<PseudoObjectElement>, H> {
        let all = self.accessors(node)?;
        let both_annotated = if let (Some(get), Some(set)) = (all.get, all.set) {
            self.node(get)?.type_node().is_some()
                && self
                    .parameters(set)?
                    .first()
                    .map(|&p| self.node(p).map(|p| p.type_node().is_some()))
                    .transpose()?
                    .unwrap_or(false)
        } else {
            false
        };
        if both_annotated {
            let data = if self.node(node)?.kind() == K::GetAccessor {
                PseudoObjectElementData::GetAccessor {
                    signature: node,
                    ty: self.type_from_accessor(node)?,
                }
            } else {
                let parameter = self
                    .clone_parameters(node)?
                    .into_iter()
                    .next()
                    .ok_or_else(|| H::Error::from(Error::MissingLink("pseudo setter parameter")))?;
                PseudoObjectElementData::SetAccessor {
                    signature: node,
                    parameter,
                }
            };
            return Ok(Some(PseudoObjectElement {
                name,
                optional: false,
                data,
            }));
        }
        if node == all.first {
            let ty = self.type_from_accessor(node)?;
            let readonly = self.node(node)?.kind() == K::GetAccessor && all.second.is_none();
            return Ok(Some(PseudoObjectElement::property(
                readonly, name, false, ty,
            )));
        }
        Ok(None)
    }
    // port: tsc/internal/pseudochecker/lookup.go:PseudoChecker.canGetTypeFromObjectLiteral
    fn object_errors(&self, properties: &[NodeId]) -> R<Vec<NodeId>, H> {
        let mut errors = Vec::new();
        for &property in properties {
            let read = self.node(property)?;
            if read.flags() & nf::THIS_NODE_HAS_ERROR != 0
                || matches!(
                    read.kind().known(),
                    Some(K::ShorthandPropertyAssignment | K::SpreadAssignment)
                )
            {
                errors.push(property);
                continue;
            }
            let name = self.name(property)?;
            let read = self.node(name)?;
            if read.flags() & nf::THIS_NODE_HAS_ERROR != 0 {
                errors.push(name);
                continue;
            }
            if read.kind() == K::PrivateIdentifier {
                errors.push(property);
                continue;
            }
            if read.kind() == K::ComputedPropertyName {
                let expression = self.expression(name)?;
                if !tsr_ast::utilities_tail::is_primitive_literal_value(
                    self.ast(expression)?,
                    &self.node(expression)?,
                    false,
                )? {
                    errors.push(name);
                }
            }
        }
        Ok(errors)
    }
    // port: tsc/internal/pseudochecker/lookup.go:PseudoChecker.typeFromArrayLiteral
    fn type_from_array(&self, node: NodeId) -> R<PseudoType, H> {
        if !self.is_in_const_context(node)? {
            return Ok(inferred_with_errors(node, false, vec![node]));
        }
        let elements = self.list(node, self.node(node)?.element_list())?;
        for &element in &elements {
            if self.node(element)?.kind() == K::SpreadElement {
                return Ok(inferred_with_errors(node, false, vec![element]));
            }
        }
        if self.contextually_typed(node)? {
            return Ok(inferred(node, false));
        }
        let mut result = Vec::with_capacity(elements.len());
        for element in elements {
            result.push(self.type_from_expression(element)?);
        }
        Ok(tuple(result))
    }
    fn is_in_const_context(&self, node: NodeId) -> R<bool, H> {
        let mut parent = self.node(node)?.parent();
        while let Some(node) = parent {
            let read = self.node(node)?;
            let assertion = matches!(
                read.kind().known(),
                Some(K::AsExpression | K::TypeAssertionExpression)
            );
            if assertion
                || !matches!(
                    read.kind().known(),
                    Some(
                        K::ArrayLiteralExpression
                            | K::ObjectLiteralExpression
                            | K::ParenthesizedExpression
                            | K::SpreadElement
                            | K::PropertyAssignment
                            | K::ShorthandPropertyAssignment
                            | K::TemplateSpan
                            | K::PrefixUnaryExpression
                    )
                )
            {
                return self.is_const_assertion(node);
            }
            parent = read.parent();
        }
        Ok(false)
    }
    // port: tsc/internal/pseudochecker/lookup.go:typeNodeCouldReferToUndefined
    fn type_node_could_refer_to_undefined(&self, node: NodeId) -> R<bool, H> {
        stacker::maybe_grow(128 * 1024, 2 * 1024 * 1024, || {
            self.type_node_could_refer_to_undefined_worker(node)
        })
    }
    fn type_node_could_refer_to_undefined_worker(&self, mut node: NodeId) -> R<bool, H> {
        while self.node(node)?.kind() == K::ParenthesizedType {
            node = Self::required(self.node(node)?.type_node(), "parenthesized pseudo type")?;
        }
        let read = self.node(node)?;
        match read.kind().known() {
            Some(
                K::TypeReference
                | K::IndexedAccessType
                | K::TypeQuery
                | K::OptionalType
                | K::RestType
                | K::ImportType
                | K::ConditionalType
                | K::TypeOperator
                | K::TypePredicate
                | K::UndefinedKeyword,
            ) => Ok(true),
            Some(K::IntersectionType | K::UnionType) => {
                // The pin uses `some`, including for intersections. Keep its
                // conservative syntactic rule instead of resolving semantics.
                let list = if read.kind() == K::UnionType {
                    read.data_source()
                        .as_union_type_node()
                        .ok_or(tsr_arena::Error::InvalidGraph)?
                        .types()
                } else {
                    read.data_source()
                        .as_intersection_type_node()
                        .ok_or(tsr_arena::Error::InvalidGraph)?
                        .types()
                };
                for child in self.list(node, list)? {
                    if self.type_node_could_refer_to_undefined(child)? {
                        return Ok(true);
                    }
                }
                Ok(false)
            }
            _ => Ok(false),
        }
    }
    fn could_refer_to_undefined(&self, ty: &PseudoType) -> R<bool, H> {
        let mut pending = vec![ty];
        while let Some(ty) = pending.pop() {
            if matches!(
                ty.kind(),
                PseudoTypeKind::NoResult | PseudoTypeKind::Inferred
            ) || is_undefined(ty)
            {
                return Ok(true);
            }
            match ty.as_ref() {
                PseudoTypeData::MaybeConstLocation { regular_type, .. } => {
                    pending.push(regular_type);
                }
                PseudoTypeData::Direct { type_node } => {
                    if self.type_node_could_refer_to_undefined(*type_node)? {
                        return Ok(true);
                    }
                }
                PseudoTypeData::Union { types } => pending.extend(types.iter().rev()),
                _ => {}
            }
        }
        Ok(false)
    }
    fn add_undefined(&self, ty: PseudoType) -> R<PseudoType, H> {
        Ok(if self.could_refer_to_undefined(&ty)? {
            ty
        } else {
            union(vec![ty, undefined()])
        })
    }
    fn parameter_rest(&self, node: NodeId) -> R<bool, H> {
        Ok(Self::required(
            self.node(node)?.data_source().as_parameter_declaration(),
            "pseudo parameter",
        )?
        .dot_dot_dot_token()
        .is_some())
    }
    fn last_required_parameter(&self, parameters: &[NodeId]) -> R<usize, H> {
        for (index, &parameter) in parameters.iter().enumerate().rev() {
            let read = self.node(parameter)?;
            if read.initializer().is_none()
                && read.question_token(self.ast(parameter)?)?.is_none()
                && !self.parameter_rest(parameter)?
            {
                return Ok(index + 1);
            }
        }
        Ok(0)
    }
    // port: tsc/internal/pseudochecker/lookup.go:PseudoChecker.typeFromParameter
    fn type_from_parameter(&self, node: NodeId) -> R<PseudoType, H> {
        let read = self.node(node)?;
        let parent = Self::required(read.parent(), "pseudo parameter parent")?;
        if self.node(parent)?.kind() == K::SetAccessor {
            return self.type_from_accessor(parent);
        }
        if read.initializer().is_none() {
            return Ok(read.type_node().map_or_else(|| no_result(node), direct));
        }
        let parameters = self.parameters(parent)?;
        let index = parameters
            .iter()
            .position(|&p| p == node)
            .map_or(-1, |i| i as isize);
        self.type_from_parameter_worker(node, index, self.last_required_parameter(&parameters)?)
    }
    // port: tsc/internal/pseudochecker/lookup.go:PseudoChecker.typeFromParameterWorker
    fn type_from_parameter_worker(
        &self,
        node: NodeId,
        index: isize,
        last_required: usize,
    ) -> R<PseudoType, H> {
        let read = self.node(node)?;
        let parent = Self::required(read.parent(), "pseudo parameter parent")?;
        if self.node(parent)?.kind() == K::SetAccessor {
            return self.type_from_accessor(parent);
        }
        let required_after = index < (last_required as isize) - 1;
        if let Some(annotation) = read.type_node() {
            let ty = direct(annotation);
            return if self.checker.strict_null_checks
                && read.initializer().is_some()
                && required_after
            {
                self.add_undefined(ty)
            } else {
                Ok(ty)
            };
        }
        if let Some(initializer) = read.initializer() {
            if self.node(self.name(node)?)?.kind() == K::Identifier
                && !self.contextually_typed(node)?
            {
                let mut ty = self.type_from_expression(initializer)?;
                if let PseudoTypeData::Inferred {
                    expression,
                    error_nodes,
                    ..
                } = ty.as_ref()
                {
                    if error_nodes.is_empty() {
                        ty = inferred_with_errors(*expression, false, vec![node]);
                    }
                }
                return if self.checker.strict_null_checks && required_after {
                    self.add_undefined(ty)
                } else {
                    Ok(ty)
                };
            }
        }
        Ok(no_result(node))
    }
    fn clone_parameters(&self, node: NodeId) -> R<Vec<PseudoParameter>, H> {
        let parameters = self.parameters(node)?;
        let last_required = self.last_required_parameter(&parameters)?;
        let mut result = Vec::with_capacity(parameters.len());
        for (index, parameter) in parameters.into_iter().enumerate() {
            let read = self.node(parameter)?;
            let optional = read.question_token(self.ast(parameter)?)?.is_some()
                || read.initializer().is_some() && (index as isize) >= (last_required as isize) - 1;
            result.push(PseudoParameter::new(
                self.parameter_rest(parameter)?,
                self.name(parameter)?,
                optional,
                self.type_from_parameter_worker(parameter, index as isize, last_required)?,
            ));
        }
        Ok(result)
    }
    // port: tsc/internal/pseudochecker/lookup.go:isContextuallyTyped
    fn contextually_typed(&self, node: NodeId) -> R<bool, H> {
        let mut parent = self.node(node)?.parent();
        while let Some(node) = parent {
            let read = self.node(node)?;
            if matches!(
                read.kind().known(),
                Some(K::CallExpression | K::SatisfiesExpression | K::JsxElement | K::JsxExpression)
            ) {
                return Ok(true);
            }
            if matches!(
                read.kind().known(),
                Some(
                    K::VariableDeclaration
                        | K::Parameter
                        | K::PropertyDeclaration
                        | K::PropertySignature
                        | K::AsExpression
                        | K::TypeAssertionExpression
                )
            ) && read.type_node().is_some()
                && !self.is_const_assertion(node)?
            {
                return Ok(true);
            }
            parent = read.parent();
        }
        Ok(false)
    }
    fn children(&self, node: NodeId) -> R<Vec<NodeId>, H> {
        use std::ops::ControlFlow;
        struct Collector<'a> {
            view: AstView<'a>,
            nodes: Vec<NodeId>,
            error: Option<tsr_arena::Error>,
        }
        impl tsr_ast::ChildVisitor for Collector<'_> {
            fn visit_node(&mut self, node: NodeId) -> ControlFlow<()> {
                self.nodes.push(node);
                ControlFlow::Continue(())
            }
            fn visit_list(&mut self, list: NodeListId) -> ControlFlow<()> {
                match self.view.list(list) {
                    Ok(list) => self.visit_node_slice(list.nodes()),
                    Err(error) => {
                        self.error = Some(error);
                        ControlFlow::Break(())
                    }
                }
            }
            fn visit_node_slice(&mut self, nodes: tsr_ast::NodeSlice) -> ControlFlow<()> {
                match self.view.node_slice(nodes) {
                    Ok(nodes) => {
                        self.nodes.extend(nodes.iter().flatten());
                        ControlFlow::Continue(())
                    }
                    Err(error) => {
                        self.error = Some(error);
                        ControlFlow::Break(())
                    }
                }
            }
        }
        let view = self.ast(node)?;
        let mut collector = Collector {
            view,
            nodes: Vec::new(),
            error: None,
        };
        let _ = view.node(node)?.for_each_child(&mut collector);
        if let Some(error) = collector.error {
            return Err(error.into());
        }
        Ok(collector.nodes)
    }
}
fn usable(ty: &PseudoType) -> bool {
    !matches!(ty.as_ref(),PseudoTypeData::Inferred{error_nodes,..} if error_nodes.is_empty())
}
fn is_undefined(mut ty: &PseudoType) -> bool {
    loop {
        match ty.as_ref() {
            PseudoTypeData::Undefined => return true,
            PseudoTypeData::MaybeConstLocation { const_type, .. } => ty = const_type,
            _ => return false,
        }
    }
}
