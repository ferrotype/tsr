//! Source signatures and their lazy instantiations. Instantiating a signature
//! leaves its return type and predicate unresolved, because reading either can
//! fix type inferences. This ordering is observable (`checker.go`).

use crate::{
    signature_flags as sg, CheckerState, Error, MapperId, SignatureId, TypeId, TypeSystemEntity,
    TypeSystemPropertyName,
};
use ts_arena::{NodeId, SymbolId};
use ts_ast::{modifier_flags as mf, node_flags as nf, SyntaxKind as K};

impl CheckerState {
    // port: tsc/internal/checker/checker.go:Checker.getSingleCallSignature
    pub(crate) fn call_single_signature(
        &mut self,
        ty: TypeId,
    ) -> Result<Option<SignatureId>, Error> {
        if self.types.flags(ty)? & crate::type_flags::OBJECT == 0 {
            return Ok(None);
        }
        self.resolve_type_members(ty)?;
        let data = self.types.structured(ty)?;
        let signatures = data.signatures.as_deref().unwrap_or_default();
        Ok((data
            .properties
            .as_ref()
            .is_none_or(|properties| properties.is_empty())
            && data
                .index_infos
                .as_ref()
                .is_none_or(|indexes| indexes.is_empty())
            && data.call_signature_count == 1
            && signatures.len() == 1)
            .then(|| signatures[0]))
    }

    // port: tsc/internal/checker/checker.go:Checker.getOrCreateTypeFromSignature
    pub(crate) fn isolated_signature_type(
        &mut self,
        signature: SignatureId,
    ) -> Result<TypeId, Error> {
        if let Some(ty) = self.signatures.get(signature)?.isolated_signature_type {
            return Ok(ty);
        }
        let declaration = self.signatures.get(signature)?.declaration;
        let (constructor, symbol) = if let Some(declaration) = declaration {
            let kind = self.ast(declaration)?.node(declaration)?.kind();
            (
                matches!(
                    kind.known(),
                    Some(K::Constructor | K::ConstructSignature | K::ConstructorType)
                ),
                self.get_symbol_of_declaration(declaration)?,
            )
        } else {
            (true, None)
        };
        let ty = self.new_object_type(
            crate::object_flags::ANONYMOUS | crate::object_flags::SINGLE_SIGNATURE_TYPE,
            symbol,
        )?;
        if constructor {
            self.set_structured_type_members(ty, None, &[], &[signature], &[])?;
        } else {
            self.set_structured_type_members(ty, None, &[signature], &[], &[])?;
        }
        self.signatures.get_mut(signature)?.isolated_signature_type = Some(ty);
        Ok(ty)
    }

    // port: tsc/internal/checker/relater.go:Checker.getTypePredicateOfSignature
    pub(crate) fn type_predicate_of_signature(
        &mut self,
        signature: SignatureId,
    ) -> Result<Option<crate::TypePredicateId>, Error> {
        let sig = self.signatures.get(signature)?;
        if let Some(predicate) = sig.resolved_type_predicate {
            return Ok((predicate != self.builtins.no_type_predicate).then_some(predicate));
        }
        let target = sig.target;
        let composite = sig.composite.clone();
        let mapper = sig.mapper;
        let declaration = sig.declaration;
        let predicate = if let Some(target) = target {
            match self.type_predicate_of_signature(target)? {
                Some(predicate) => {
                    let mut data = self.signatures.predicate(predicate)?.clone();
                    let ty = data
                        .t
                        .map(|ty| self.instantiate_type(ty, mapper))
                        .transpose()?;
                    Some(if ty == data.t {
                        predicate
                    } else {
                        data.t = ty;
                        self.signatures.new_type_predicate(data)?
                    })
                }
                None => None,
            }
        } else if let Some(composite) = composite {
            self.compound_type_predicate(&composite.signatures, composite.is_union)?
        } else if let Some(declaration) = declaration {
            let read = self.ast(declaration)?.node(declaration)?;
            match read.type_node() {
                Some(node) if self.ast(node)?.node(node)?.kind() == K::TypePredicate => {
                    Some(self.predicate_from_node(node, signature)?)
                }
                None if read.body().is_some() => {
                    let sig = self.signatures.get(signature)?;
                    let eligible_return = match sig.resolved_return_type {
                        Some(ty) => self.types.flags(ty)? & crate::type_flags::BOOLEAN != 0,
                        None => true,
                    };
                    if eligible_return && self.parameter_count(signature)? > 0 {
                        // A predicate can consult this same signature through a
                        // recursive call. The sentinel is provisional until the
                        // body finishes; a failed typed operation must be retryable.
                        self.signatures.get_mut(signature)?.resolved_type_predicate =
                            Some(self.builtins.no_type_predicate);
                        match self.type_predicate_from_body(declaration) {
                            Ok(predicate) => predicate,
                            Err(error) => {
                                self.signatures.get_mut(signature)?.resolved_type_predicate = None;
                                return Err(error);
                            }
                        }
                    } else {
                        None
                    }
                }
                Some(_) | None => None,
            }
        } else {
            None
        };
        self.signatures.get_mut(signature)?.resolved_type_predicate =
            Some(predicate.unwrap_or(self.builtins.no_type_predicate));
        Ok(predicate)
    }

    // port: tsc/internal/checker/relater.go:Checker.createTypePredicateFromTypePredicateNode
    fn predicate_from_node(
        &mut self,
        node: NodeId,
        signature: SignatureId,
    ) -> Result<crate::TypePredicateId, Error> {
        use crate::TypePredicateKind as Kind;
        let read = self.ast(node)?.node(node)?;
        let data = read
            .data_source()
            .as_type_predicate_node()
            .ok_or(Error::MissingLink("type predicate"))?;
        let name = data
            .parameter_name()
            .ok_or(Error::MissingLink("predicate name"))?;
        let this = self.ast(name)?.node(name)?.kind() == K::ThisType;
        let asserts = data.asserts_modifier().is_some();
        let annotation = data.r#type();
        let text = if this {
            ts_ast::JsString::default()
        } else {
            self.ast(name)?.node_text(name)?.into_js_string()
        };
        let mut parameter_index = if this { 0 } else { -1 };
        if !this {
            for (index, &parameter) in self
                .signatures
                .get(signature)?
                .parameters
                .as_deref()
                .unwrap_or_default()
                .iter()
                .enumerate()
            {
                if self.symbol(parameter)?.name_bytes() == text.as_bytes() {
                    parameter_index = index as i32;
                    break;
                }
            }
        }
        let kind = match (this, asserts) {
            (true, true) => Kind::AssertsThis,
            (true, false) => Kind::This,
            (false, true) => Kind::AssertsIdentifier,
            (false, false) => Kind::Identifier,
        };
        let ty = annotation
            .map(|node| self.get_type_from_type_node(node))
            .transpose()?;
        self.signatures
            .new_type_predicate(crate::signatures::TypePredicate {
                kind,
                parameter_index,
                parameter_name: text,
                t: ty,
            })
    }

    // port: tsc/internal/checker/checker.go:Checker.getSignaturesOfSymbol
    pub(crate) fn signatures_of_symbol(
        &mut self,
        symbol: Option<SymbolId>,
    ) -> Result<Vec<SignatureId>, Error> {
        let Some(symbol) = symbol else {
            return Ok(Vec::new());
        };
        let declarations = self.symbol_declarations(symbol)?.to_vec();
        let mut signatures = Vec::new();
        for (index, &declaration) in declarations.iter().enumerate() {
            let Some(declaration) = declaration else {
                continue;
            };
            let read = self.ast(declaration)?.node(declaration)?;
            if !ts_ast::utilities::is_function_like(Some(&read)) {
                continue;
            }
            if index > 0 && read.body().is_some() {
                if let Some(previous) = declarations[index - 1] {
                    let previous = self.ast(previous)?.node(previous)?;
                    if read.parent() == previous.parent()
                        && read.kind() == previous.kind()
                        && (read.pos() == previous.end() || previous.flags() & nf::REPARSED != 0)
                    {
                        continue;
                    }
                }
            }
            let signature = match self.signature_of_full_signature(declaration)? {
                Some(signature) => signature,
                None => self.signature_from_declaration(declaration)?,
            };
            signatures.push(signature);
        }
        Ok(signatures)
    }

    // port: tsc/internal/checker/checker.go:Checker.getSignatureFromDeclaration
    pub(crate) fn signature_from_declaration(
        &mut self,
        node: NodeId,
    ) -> Result<SignatureId, Error> {
        if let Some(Some(signature)) = self.query.source_signatures.try_get(node) {
            return Ok(*signature);
        }
        let read = self.ast(node)?.node(node)?;
        let construct = matches!(
            read.kind().known(),
            Some(K::Constructor | K::ConstructorType | K::ConstructSignature)
        );
        let mut flags = if construct { sg::CONSTRUCT } else { 0 };
        let abstract_node = if read.kind() == K::Constructor {
            read.parent()
        } else if read.kind() == K::ConstructorType {
            Some(node)
        } else {
            None
        };
        if abstract_node
            .map(|node| {
                self.ast(node)?
                    .node(node)?
                    .modifier_flags(self.ast(node)?)
                    .map_err(Error::from)
            })
            .transpose()?
            .is_some_and(|flags| flags & mf::ABSTRACT != 0)
        {
            flags |= sg::ABSTRACT;
        }
        let nodes = self.source_list(node, read.parameter_list())?;
        let java_script = read.flags() & nf::JAVA_SCRIPT_FILE != 0
            && matches!(
                read.kind().known(),
                Some(
                    K::FunctionDeclaration
                        | K::FunctionExpression
                        | K::ArrowFunction
                        | K::MethodDeclaration
                        | K::GetAccessor
                        | K::SetAccessor
                        | K::Constructor
                )
            );
        if java_script
            && ts_ast::get_immediately_invoked_function_expression(self.ast(node)?, node)?.is_none()
        {
            let mut untyped = true;
            for &parameter in &nodes {
                if self.ast(parameter)?.node(parameter)?.type_node().is_some() {
                    untyped = false;
                    break;
                }
            }
            if untyped && self.contextual_expression_type_ex(node, 1)?.is_none() {
                flags |= sg::IS_UNTYPED_SIGNATURE_IN_JS_FILE;
            }
        }
        let mut parameters = Vec::new();
        let mut this_parameter = None;
        let mut minimum = 0;
        for (index, parameter) in nodes.iter().copied().enumerate() {
            let mut symbol = self
                .get_symbol_of_declaration(parameter)?
                .ok_or(Error::MissingLink("parameter symbol"))?;
            if self.symbol(symbol)?.flags() & ts_ast::symbol_flags::PROPERTY != 0 {
                let name = self
                    .ast(parameter)?
                    .node(parameter)?
                    .name()
                    .ok_or(Error::MissingLink("parameter property name"))?;
                if self.ast(name)?.node(name)?.kind() == K::Identifier {
                    let text = ts_ast::JsString::from_bytes(self.symbol(symbol)?.name_bytes());
                    symbol = self
                        .resolve_name(
                            Some(parameter),
                            text.as_bytes(),
                            ts_ast::symbol_flags::VALUE,
                            None,
                            false,
                        )?
                        .ok_or(Error::MissingLink("parameter property local symbol"))?;
                }
            }
            let read = self.ast(parameter)?.node(parameter)?;
            if index == 0 && self.symbol(symbol)?.name_bytes() == b"this" {
                this_parameter = Some(symbol);
            } else {
                parameters.push(symbol);
            }
            if let Some(annotation) = read.type_node() {
                if self.ast(annotation)?.node(annotation)?.kind() == K::LiteralType {
                    flags |= sg::HAS_LITERAL_TYPES;
                }
            }
            let rest = read
                .data_source()
                .as_parameter_declaration()
                .ok_or(Error::MissingLink("parameter declaration"))?
                .dot_dot_dot_token()
                .is_some();
            if index + 1 == nodes.len() && rest {
                flags |= sg::HAS_REST_PARAMETER;
            }
            if read.question_token(self.ast(parameter)?)?.is_none()
                && read.initializer().is_none()
                && !rest
                && !self.is_optional_source_parameter(parameter)?
            {
                minimum = parameters.len();
            }
        }
        let kind = self.ast(node)?.node(node)?.kind();
        if this_parameter.is_none() && matches!(kind.known(), Some(K::GetAccessor | K::SetAccessor))
        {
            let bindable = if !ts_ast::has_dynamic_name(self.ast(node)?, Some(node))? {
                true
            } else if let Some(name) = self.late_name(node)? {
                let ty = self.late_name_type(name)?;
                self.types.flags(ty)?
                    & (crate::type_flags::STRING_OR_NUMBER_LITERAL
                        | crate::type_flags::UNIQUE_ES_SYMBOL)
                    != 0
            } else {
                false
            };
            if bindable {
                let symbol = self
                    .get_symbol_of_declaration(node)?
                    .ok_or(Error::MissingLink("accessor signature symbol"))?;
                let opposite = if kind == K::GetAccessor {
                    K::SetAccessor
                } else {
                    K::GetAccessor
                };
                if let Some(other) = self.declaration_of_kind(symbol, opposite)? {
                    if let Some(parameter) = self.accessor_this_parameter(other)? {
                        this_parameter = self.get_symbol_of_declaration(parameter)?;
                    }
                }
            }
        }
        let type_parameters = if self.ast(node)?.node(node)?.kind() == K::Constructor {
            let class = self.constructor_class_type(node)?;
            let data = self.types.interface(class)?;
            data.type_parameters()[data.outer_type_parameter_count as usize..]
                .to_vec()
                .into()
        } else {
            self.type_parameters_from_declaration(node)?
        };
        let signature = self.signatures.new_signature(
            flags,
            Some(node),
            (!type_parameters.is_empty()).then_some(type_parameters),
            this_parameter,
            (!parameters.is_empty()).then(|| parameters.into()),
            None,
            None,
            minimum as i32,
        )?;
        *self.query.source_signatures.get_or_default(node) = Some(signature);
        Ok(signature)
    }

    // port: tsc/internal/checker/checker.go:Checker.getReturnTypeOfSignature
    pub(crate) fn return_type_of_signature(
        &mut self,
        signature: SignatureId,
    ) -> Result<TypeId, Error> {
        let sig = self.signatures.get(signature)?;
        if let Some(ty) = sig.resolved_return_type {
            return Ok(ty);
        }
        let declaration = sig.declaration;
        let flags = sig.flags;
        let target = sig.target;
        let composite = sig.composite.clone();
        let mapper = sig.mapper;
        let signatures = &self.signatures;
        if !self.resolution.push(
            TypeSystemEntity::Signature(signature),
            TypeSystemPropertyName::ResolvedReturnType,
            |entry| {
                let TypeSystemEntity::Signature(signature) = entry.target else {
                    return false;
                };
                entry.property_name == TypeSystemPropertyName::ResolvedReturnType
                    && signatures
                        .get(signature)
                        .is_ok_and(|signature| signature.resolved_return_type.is_some())
            },
        ) {
            return Ok(self.builtins.error_type);
        }
        let result = (|| {
            if let Some(target) = target {
                let ty = self.return_type_of_signature(target)?;
                return self.instantiate_type(ty, mapper);
            }
            if let Some(composite) = composite {
                let mut returns = Vec::with_capacity(composite.signatures.len());
                for &signature in composite.signatures.iter() {
                    returns.push(self.return_type_of_signature(signature)?);
                }
                let ty = if composite.is_union {
                    self.get_union_type_ex(&returns, crate::UnionReduction::Subtype, None, None)?
                } else {
                    self.get_intersection_type(&returns)?
                };
                return self.instantiate_type(ty, mapper);
            }
            let node = declaration.ok_or(Error::MissingLink("signature declaration"))?;
            if let Some(ty) = self.return_type_from_annotation(node)? {
                return Ok(ty);
            }
            let read = self.ast(node)?.node(node)?;
            if let Some(body) = read.body() {
                if !ts_ast::node_is_missing(Some(&self.ast(body)?.node(body)?)) {
                    return self.return_type_from_body(node);
                }
            }
            Ok(self.builtins.any_type)
        })();
        let result = result.and_then(|ty| {
            if flags & sg::IS_INNER_CALL_CHAIN != 0 {
                self.add_optional_type_marker(ty)
            } else if flags & sg::IS_OUTER_CALL_CHAIN != 0 {
                self.add_type_optionality(ty, false, true)
            } else {
                Ok(ty)
            }
        });
        let complete = self.resolution.pop();
        let mut ty = result?;
        if !complete {
            if let Some(node) = declaration {
                let annotation = self.ast(node)?.node(node)?.type_node();
                let options = self.program()?.host.options();
                let no_implicit_any = options.strict_option_value(options.no_implicit_any);
                if let Some(annotation) = annotation {
                    self.error_at(
                        Some(annotation),
                        ts_diagnostics::Return_type_annotation_circularly_references_itself,
                        vec![],
                    )?;
                } else if no_implicit_any {
                    let name = ts_ast::get_name_of_declaration(self.ast(node)?, Some(node))?;
                    if let Some(name) = name {
                        let text =
                            ts_scanner::declaration_name_to_string(self.ast(name)?, Some(name))?;
                        self.error_at(Some(name), ts_diagnostics::X_0_implicitly_has_return_type_any_because_it_does_not_have_a_return_type_annotation_and_is_referenced_directly_or_indirectly_in_one_of_its_return_expressions, vec![text])?;
                    } else {
                        self.error_at(Some(node), ts_diagnostics::Function_implicitly_has_return_type_any_because_it_does_not_have_a_return_type_annotation_and_is_referenced_directly_or_indirectly_in_one_of_its_return_expressions, vec![])?;
                    }
                }
            }
            ty = self.builtins.any_type;
        }
        Ok(*self
            .signatures
            .get_mut(signature)?
            .resolved_return_type
            .get_or_insert(ty))
    }

    fn constructor_class_type(&mut self, node: NodeId) -> Result<TypeId, Error> {
        let parent = self
            .ast(node)?
            .node(node)?
            .parent()
            .ok_or(Error::MissingLink("constructor class"))?;
        let symbol = self
            .get_symbol_of_declaration(parent)?
            .ok_or(Error::MissingLink("constructor class symbol"))?;
        self.declared_interface_type(symbol)
    }

    // port: tsc/internal/checker/checker.go:Checker.getReturnTypeFromAnnotation
    pub(crate) fn return_type_from_annotation(
        &mut self,
        node: NodeId,
    ) -> Result<Option<TypeId>, Error> {
        let read = self.ast(node)?.node(node)?;
        if read.kind() == K::Constructor {
            return self.constructor_class_type(node).map(Some);
        }
        if let Some(annotation) = read.type_node() {
            return self.get_type_from_type_node(annotation).map(Some);
        }
        if read.kind() == K::GetAccessor {
            let symbol = self
                .get_symbol_of_declaration(node)?
                .ok_or(Error::MissingLink("getter declaration symbol"))?;
            let setter = self.declaration_of_kind(symbol, K::SetAccessor)?;
            return self.annotated_accessor_type(setter);
        }
        self.signature_of_full_signature(node)?
            .map(|signature| self.return_type_of_signature(signature))
            .transpose()
    }

    // port: tsc/internal/checker/checker.go:Checker.instantiateSignature
    pub(crate) fn instantiate_signature(
        &mut self,
        signature: SignatureId,
        mapper: MapperId,
    ) -> Result<SignatureId, Error> {
        self.instantiate_signature_ex(signature, mapper, false)
    }

    // port: tsc/internal/checker/checker.go:Checker.instantiateSignatureEx
    pub(crate) fn instantiate_signature_ex(
        &mut self,
        signature: SignatureId,
        mapper: MapperId,
        erase: bool,
    ) -> Result<SignatureId, Error> {
        let sig = self.signatures.get(signature)?.clone();
        let mut mapper = mapper;
        let parameters = if erase {
            &[]
        } else {
            sig.type_parameters.as_deref().unwrap_or_default()
        };
        let mut fresh = Vec::with_capacity(parameters.len());
        for &parameter in parameters {
            let ty = self.new_type_parameter(self.types.get(parameter)?.symbol)?;
            self.types.type_parameter_mut(ty)?.target = Some(parameter);
            fresh.push(ty);
        }
        if !fresh.is_empty() {
            let fresh_mapper = self.new_type_mapper(parameters, &fresh)?;
            mapper = self.combine_type_mappers(Some(fresh_mapper), mapper)?;
            for &parameter in &fresh {
                self.types.type_parameter_mut(parameter)?.mapper = Some(mapper);
            }
        }
        let this_parameter = sig
            .this_parameter
            .map(|symbol| self.instantiate_symbol(symbol, mapper))
            .transpose()?;
        let mut parameters = Vec::new();
        for &parameter in sig.parameters.as_deref().unwrap_or_default() {
            parameters.push(self.instantiate_symbol(parameter, mapper)?);
        }
        let result = self.signatures.new_signature(
            sig.flags & sg::PROPAGATING_FLAGS,
            sig.declaration,
            (!fresh.is_empty()).then(|| fresh.into()),
            this_parameter,
            (!parameters.is_empty()).then(|| parameters.into()),
            None,
            None,
            sig.min_argument_count,
        )?;
        let instantiated = self.signatures.get_mut(result)?;
        instantiated.target = Some(signature);
        instantiated.mapper = Some(mapper);
        Ok(result)
    }

    // port: tsc/internal/checker/checker.go:Checker.getTypeOfInstantiatedSymbol
    pub(crate) fn get_type_of_instantiated_symbol(
        &mut self,
        symbol: SymbolId,
    ) -> Result<TypeId, Error> {
        let links = self
            .value_symbol_links
            .try_get(symbol)
            .copied()
            .ok_or(Error::MissingLink("instantiated symbol links"))?;
        if let Some(ty) = links.resolved_type {
            return Ok(ty);
        }
        let target = links
            .target
            .ok_or(Error::MissingLink("instantiated symbol target"))?;
        let ty = self.get_type_of_symbol(target)?;
        let ty = self.instantiate_type(ty, links.mapper)?;
        self.value_symbol_links.get_or_default(symbol).resolved_type = Some(ty);
        Ok(ty)
    }
}

impl CheckerState {
    // port: tsc/internal/checker/checker.go:Checker.getSignatureOfFullSignatureType
    pub(crate) fn full_signature_type_node(&self, node: NodeId) -> Result<Option<NodeId>, Error> {
        let read = self.ast(node)?.node(node)?;
        if read.flags() & nf::JAVA_SCRIPT_FILE == 0 {
            return Ok(None);
        }
        let data = read.data_source();
        Ok(match read.kind().known() {
            Some(K::FunctionDeclaration) => data
                .as_function_declaration()
                .and_then(|data| data.full_signature()),
            Some(K::FunctionExpression) => data
                .as_function_expression()
                .and_then(|data| data.full_signature()),
            Some(K::ArrowFunction) => data
                .as_arrow_function()
                .and_then(|data| data.full_signature()),
            Some(K::MethodDeclaration) => data
                .as_method_declaration()
                .and_then(|data| data.full_signature()),
            _ => None,
        })
    }

    pub(crate) fn signature_of_full_signature(
        &mut self,
        node: NodeId,
    ) -> Result<Option<SignatureId>, Error> {
        let Some(annotation) = self.full_signature_type_node(node)? else {
            return Ok(None);
        };
        let ty = self.get_type_from_type_node(annotation)?;
        self.call_single_signature(ty)
    }

    // port: tsc/internal/checker/checker.go:Checker.getParameterTypeOfFullSignature
    pub(crate) fn parameter_type_of_full_signature(
        &mut self,
        function: NodeId,
        parameter: NodeId,
    ) -> Result<Option<TypeId>, Error> {
        let Some(signature) = self.signature_of_full_signature(function)? else {
            return Ok(None);
        };
        let parameters = self.source_list(
            function,
            self.ast(function)?.node(function)?.parameter_list(),
        )?;
        let index = parameters
            .iter()
            .position(|&node| node == parameter)
            .ok_or(Error::MissingLink("full-signature parameter position"))?;
        let rest = self
            .ast(parameter)?
            .node(parameter)?
            .data_source()
            .as_parameter_declaration()
            .ok_or(Error::MissingLink("full-signature parameter"))?
            .dot_dot_dot_token()
            .is_some();
        if rest {
            self.rest_type_at(signature, index).map(Some)
        } else {
            Ok(Some(
                self.parameter_type_at(signature, index)?
                    .unwrap_or(self.builtins.any_type),
            ))
        }
    }

    // port: tsc/internal/checker/checker.go:Checker.checkFunctionOrMethodDeclaration
    // port: tsc/internal/checker/checker.go:Checker.checkFunctionExpressionOrObjectLiteralMethod
    pub(crate) fn check_full_signature_arity(&mut self, function: NodeId) -> Result<(), Error> {
        if let Some(annotation) = self.full_signature_type_node(function)? {
            let ty = self.get_type_from_type_node(annotation)?;
            if self.contextual_signature_for_type(function, ty)?.is_none() {
                self.error_at(Some(annotation),ts_diagnostics::A_JSDoc_type_tag_on_a_function_must_have_a_signature_with_the_correct_number_of_arguments,vec![])?;
            }
        }
        Ok(())
    }
}

impl CheckerState {
    // port: tsc/internal/checker/checker.go:Checker.getAccessorThisParameter
    pub(crate) fn accessor_this_parameter(&self, node: NodeId) -> Result<Option<NodeId>, Error> {
        let read = self.ast(node)?.node(node)?;
        let count = if read.kind() == K::GetAccessor { 1 } else { 2 };
        let parameters = self.source_list(node, read.parameter_list())?;
        if parameters.len() != count {
            return Ok(None);
        }
        let parameter = parameters[0];
        let Some(name) = self.ast(parameter)?.node(parameter)?.name() else {
            return Ok(None);
        };
        Ok((self.ast(name)?.node(name)?.kind() == K::Identifier
            && self.ast(name)?.node_text(name)?.as_bytes() == b"this")
            .then_some(parameter))
    }
}
