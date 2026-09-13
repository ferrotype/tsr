//! Source references, generic interface identities and deferred type arguments.
//! Member resolution is separate: a declared generic type does not eagerly
//! instantiate its properties or signatures.

use crate::{
    object_flags as of, AliasId, CheckerState, Error, MapperId, TypeId, TypeList, TypeSystemEntity,
    TypeSystemPropertyName,
};
use ts_arena::{NodeId, SymbolId};
use ts_ast::{node_flags as nf, symbol_flags as sf, JsString, SyntaxKind as K};

impl CheckerState {
    // port: tsc/internal/checker/checker.go:Checker.getDeclaredTypeOfClassOrInterface
    pub(crate) fn declared_interface_type(&mut self, symbol: SymbolId) -> Result<TypeId, Error> {
        if let Some(Some(ty)) = self.query.declared_types.try_get(symbol) {
            return Ok(*ty);
        }
        let is_class = self.symbol(symbol)?.flags() & sf::CLASS != 0;
        let declaration = if is_class {
            self.symbol(symbol)?.value_declaration()
        } else {
            let declarations = self.symbol_declarations(symbol)?.to_vec();
            let mut declaration = None;
            for node in declarations.into_iter().flatten() {
                if self.ast(node)?.node(node)?.kind() == K::InterfaceDeclaration {
                    declaration = Some(node);
                    break;
                }
            }
            declaration
        };
        let declaration = declaration.ok_or(Error::MissingLink("class/interface declaration"))?;
        let ty = self.new_object_type(
            if is_class { of::CLASS } else { of::INTERFACE },
            Some(symbol),
        )?;
        *self.query.declared_types.get_or_default(symbol) = Some(ty);
        let result = (|| {
            let outer = self.get_outer_type_parameters(declaration, false)?;
            let local = self.get_local_type_parameters(symbol)?;
            let mut parameters = outer.to_vec();
            for &parameter in local.iter() {
                if !parameters.contains(&parameter) {
                    parameters.push(parameter);
                }
            }
            if !parameters.is_empty() || is_class || !self.is_thisless_interface(symbol)? {
                self.types.get_mut(ty)?.object_flags |= of::REFERENCE;
                let this = self.new_type_parameter(Some(symbol))?;
                self.types.type_parameter_mut(this)?.is_this_type = true;
                self.types.type_parameter_mut(this)?.constraint = Some(ty);
                let arguments: TypeList = parameters.clone().into();
                parameters.push(this);
                let mut cache = crate::types::Map::default();
                cache.insert(crate::key::type_list_key(&arguments), ty);
                let interface = self.types.interface_mut(ty)?;
                interface.all_type_parameters = Some(parameters.into());
                interface.outer_type_parameter_count = outer.len() as u32;
                interface.this_type = Some(this);
                interface.reference.object.target = Some(ty);
                interface.reference.object.instantiations = Some(Box::new(cache));
                interface.reference.resolved_type_arguments = Some(arguments);
            }
            Ok(ty)
        })();
        if result.is_err() {
            *self.query.declared_types.get_or_default(symbol) = None;
        }
        result
    }

    // port: tsc/internal/checker/checker.go:Checker.getTypeAliasInstantiation
    pub(crate) fn type_alias_instantiation(
        &mut self,
        symbol: SymbolId,
        ty: TypeId,
        parameters: &[TypeId],
        arguments: &[TypeId],
        alias: Option<AliasId>,
    ) -> Result<TypeId, Error> {
        let mut key = crate::key::KeyBuilder::new();
        key.write_types(arguments);
        let parts = self.alias_key_parts(alias)?;
        key.write_alias(
            parts
                .as_ref()
                .map(|(symbol, arguments)| (*symbol, &arguments[..])),
        );
        let key = key.finish();
        if let Some(&cached) = self
            .query
            .type_aliases
            .get_or_default(symbol)
            .instantiations
            .get(&key)
        {
            return Ok(cached);
        }
        let mapper = self.new_type_mapper(parameters, arguments)?;
        let result = self.instantiate_type_with_alias(ty, Some(mapper), alias)?;
        self.query
            .type_aliases
            .get_or_default(symbol)
            .instantiations
            .insert(key, result);
        Ok(result)
    }

    pub(crate) fn interface_base_nodes(&self, node: NodeId) -> Result<Vec<NodeId>, Error> {
        let read = self.ast(node)?.node(node)?;
        let clauses = read
            .data_source()
            .as_interface_declaration()
            .ok_or(Error::MissingLink("interface payload"))?
            .heritage_clauses();
        let mut result = Vec::new();
        for clause in self.source_list(node, clauses)? {
            let read = self.ast(clause)?.node(clause)?;
            let clause_data = read
                .data_source()
                .as_heritage_clause()
                .ok_or(Error::MissingLink("heritage clause"))?;
            if clause_data.token() == K::ExtendsKeyword {
                result.extend(self.source_list(clause, clause_data.types())?);
            }
        }
        Ok(result)
    }

    // port: tsc/internal/checker/checker.go:Checker.isThislessInterface
    fn is_thisless_interface(&mut self, symbol: SymbolId) -> Result<bool, Error> {
        for node in self
            .symbol_declarations(symbol)?
            .to_vec()
            .into_iter()
            .flatten()
        {
            let read = self.ast(node)?.node(node)?;
            if read.kind() != K::InterfaceDeclaration {
                continue;
            }
            if read.flags() & nf::CONTAINS_THIS != 0 {
                return Ok(false);
            }
            for base in self.interface_base_nodes(node)? {
                let Some(symbol) = self.type_reference_symbol(base, true)? else {
                    return Ok(false);
                };
                if self.symbol(symbol)?.flags() & sf::INTERFACE == 0 {
                    return Ok(false);
                }
                let ty = self.get_declared_type_of_symbol(symbol)?;
                if self.types.interface(ty)?.this_type.is_some() {
                    return Ok(false);
                }
            }
        }
        Ok(true)
    }

    // port: tsc/internal/checker/checker.go:Checker.getSymbolFromTypeReference
    pub(crate) fn type_reference_symbol(
        &mut self,
        node: NodeId,
        ignore_errors: bool,
    ) -> Result<Option<SymbolId>, Error> {
        let read = self.ast(node)?.node(node)?;
        if read.kind() == K::ImportType {
            self.type_from_import_node(node)?;
            return Ok(self.query.resolved_symbols.try_get(node).copied().flatten());
        }
        if !ignore_errors {
            if let Some(Some(symbol)) = self.query.resolved_symbols.try_get(node) {
                return Ok(Some(*symbol));
            }
        }
        let symbol = self.resolve_type_reference_symbol(node, ignore_errors)?;
        if !ignore_errors {
            *self.query.resolved_symbols.get_or_default(node) = Some(symbol);
        }
        Ok(Some(symbol))
    }

    // port: tsc/internal/checker/checker.go:Checker.resolveTypeReferenceName
    fn resolve_type_reference_symbol(
        &mut self,
        node: NodeId,
        ignore_errors: bool,
    ) -> Result<SymbolId, Error> {
        let read = self.ast(node)?.node(node)?;
        let name = match read.kind().known() {
            Some(K::TypeReference) => read
                .data_source()
                .as_type_reference_node()
                .ok_or(Error::MissingLink("type reference payload"))?
                .type_name(),
            Some(K::ExpressionWithTypeArguments) => match read.expression() {
                Some(name) if ts_ast::is_entity_name_expression(self.ast(name)?, name)? => {
                    Some(name)
                }
                _ => None,
            },
            _ => None,
        };
        Ok(match name {
            None => self.builtins.unknown_symbol,
            Some(name) => match self.resolve_entity_name(name, sf::TYPE, ignore_errors)? {
                Some(symbol) if symbol != self.builtins.unknown_symbol => symbol,
                _ if ignore_errors => self.builtins.unknown_symbol,
                _ => self.unresolved_symbol_for_name(name)?,
            },
        })
    }

    // port: tsc/internal/checker/checker.go:Checker.getTypeFromTypeReference
    pub(crate) fn source_type_reference(&mut self, node: NodeId) -> Result<TypeId, Error> {
        let Some(symbol) = self.type_reference_symbol(node, false)? else {
            return Ok(self.builtins.error_type);
        };
        self.type_reference_from_symbol(node, symbol)
    }
    // port: tsc/internal/checker/checker.go:Checker.getTypeReferenceType
    pub(crate) fn type_reference_from_symbol(
        &mut self,
        node: NodeId,
        symbol: SymbolId,
    ) -> Result<TypeId, Error> {
        if symbol == self.builtins.unknown_symbol {
            return Ok(self.builtins.error_type);
        }
        let ty = self.get_declared_type_of_symbol(symbol)?;
        let flags = self.symbol(symbol)?.flags();
        let argument_nodes =
            self.source_list(node, self.ast(node)?.node(node)?.type_argument_list())?;
        if flags & (sf::CLASS | sf::INTERFACE) != 0 {
            let interface = self.types.interface(ty)?;
            let outer_count = interface.outer_type_parameter_count as usize;
            let outer = interface.type_parameters()[..outer_count].to_vec();
            let parameters = interface.type_parameters()[outer_count..].to_vec();
            if !parameters.is_empty() {
                if !self.check_class_reference_arity(node, ty, &argument_nodes, &parameters)? {
                    return Ok(self.builtins.error_type);
                }
                if self.ast(node)?.node(node)?.kind() == K::TypeReference
                    && self.is_deferred_type_reference_node(
                        node,
                        argument_nodes.len() != parameters.len(),
                    )?
                {
                    return self.create_deferred_type_reference(ty, node, None, None);
                }
                let local = self.effective_type_arguments(node, &parameters)?;
                let mut arguments = outer;
                arguments.extend_from_slice(&local);
                return self.create_type_reference_ex(ty, &arguments, of::FROM_TYPE_NODE);
            }
        }
        if flags & sf::TYPE_ALIAS != 0 {
            if let Some(parameters) = self
                .query
                .type_aliases
                .try_get(symbol)
                .and_then(|links| links.parameters.clone())
            {
                if !self.check_reference_arity(node, symbol, &argument_nodes, &parameters)? {
                    return Ok(self.builtins.error_type);
                }
                let arguments = self.effective_type_arguments(node, &parameters)?;
                if ty == self.builtins.intrinsic_marker_type && arguments.len() == 1 {
                    match self.symbol(symbol)?.name_bytes() {
                        b"Uppercase" | b"Lowercase" | b"Capitalize" | b"Uncapitalize" => {
                            return self.get_string_mapping_type(symbol, arguments[0])
                        }
                        b"NoInfer" => return self.no_infer_type(arguments[0]),
                        _ => {}
                    }
                }
                let alias = self
                    .alias_for_type_node(node)?
                    .map(|alias| self.types.push_alias(alias))
                    .transpose()?;
                return self.type_alias_instantiation(symbol, ty, &parameters, &arguments, alias);
            }
        }
        if !argument_nodes.is_empty() {
            let name = self.symbol_to_string(symbol)?;
            self.error_at(
                Some(node),
                ts_diagnostics::Type_0_is_not_generic,
                vec![name],
            )?;
            return Ok(self.builtins.error_type);
        }
        self.get_regular_type_of_literal_type(ty)
    }

    // port: tsc/internal/checker/checker.go:Checker.getTypeFromClassOrInterfaceReference
    fn check_class_reference_arity(
        &mut self,
        node: NodeId,
        ty: TypeId,
        nodes: &[NodeId],
        parameters: &[TypeId],
    ) -> Result<bool, Error> {
        let minimum = self.min_type_argument_count(parameters)?;
        let read = self.ast(node)?.node(node)?;
        let is_js = read.flags() & nf::JAVA_SCRIPT_FILE != 0;
        let options = self.program()?.host.options();
        if is_js && !options.strict_option_value(options.no_implicit_any)
            || nodes.len() >= minimum && nodes.len() <= parameters.len()
        {
            return Ok(true);
        }
        let parent = read.parent();
        let missing_augments = is_js
            && read.kind() == K::ExpressionWithTypeArguments
            && match parent {
                Some(parent) => self.ast(parent)?.node(parent)?.kind() != K::JSDocAugmentsTag,
                None => true,
            };
        let message = if missing_augments {
            if minimum < parameters.len() {
                ts_diagnostics::Expected_0_1_type_arguments_provide_these_with_an_extends_tag
            } else {
                ts_diagnostics::Expected_0_type_arguments_provide_these_with_an_extends_tag
            }
        } else if minimum < parameters.len() {
            ts_diagnostics::Generic_type_0_requires_between_1_and_2_type_arguments
        } else {
            ts_diagnostics::Generic_type_0_requires_1_type_argument_s
        };
        let text =
            self.type_to_string(ty, crate::type_format_flags::WRITE_ARRAY_AS_GENERIC_TYPE)?;
        self.error_at(
            Some(node),
            message,
            vec![
                text,
                JsString::from_bytes(minimum.to_string().into_bytes()),
                JsString::from_bytes(parameters.len().to_string().into_bytes()),
            ],
        )?;
        Ok(is_js)
    }

    fn check_reference_arity(
        &mut self,
        node: NodeId,
        symbol: SymbolId,
        nodes: &[NodeId],
        parameters: &[TypeId],
    ) -> Result<bool, Error> {
        let minimum = self.min_type_argument_count(parameters)?;
        if nodes.len() >= minimum && nodes.len() <= parameters.len() {
            return Ok(true);
        }
        let name = self.symbol_to_string(symbol)?;
        let minimum_text = JsString::from_bytes(minimum.to_string().into_bytes());
        let (message, arguments) = if minimum == parameters.len() {
            (
                ts_diagnostics::Generic_type_0_requires_1_type_argument_s,
                vec![name, minimum_text],
            )
        } else {
            (
                ts_diagnostics::Generic_type_0_requires_between_1_and_2_type_arguments,
                vec![
                    name,
                    minimum_text,
                    JsString::from_bytes(parameters.len().to_string().into_bytes()),
                ],
            )
        };
        self.error_at(Some(node), message, arguments)?;
        Ok(false)
    }

    pub(crate) fn effective_type_arguments(
        &mut self,
        node: NodeId,
        parameters: &[TypeId],
    ) -> Result<TypeList, Error> {
        let read = self.ast(node)?.node(node)?;
        let in_js = read.flags() & nf::JAVA_SCRIPT_FILE != 0;
        let nodes = self.source_list(node, read.type_argument_list())?;
        let mut arguments = Vec::with_capacity(nodes.len());
        for node in nodes {
            arguments.push(self.get_type_from_type_node(node)?);
        }
        self.fill_missing_type_arguments(&arguments, parameters, in_js)
    }

    // port: tsc/internal/checker/checker.go:Checker.isDeferredTypeReferenceNode
    pub(crate) fn is_deferred_type_reference_node(
        &mut self,
        node: NodeId,
        defaults: bool,
    ) -> Result<bool, Error> {
        if self.alias_for_type_node(node)?.is_some() {
            return Ok(true);
        }
        if !self.is_resolved_by_type_alias(node)? {
            return Ok(false);
        }
        if defaults {
            return Ok(true);
        }
        let read = self.ast(node)?.node(node)?;
        let nodes = if read.kind() == K::TypeReference {
            self.source_list(node, read.type_argument_list())?
        } else if read.kind() == K::ArrayType {
            vec![read
                .data_source()
                .as_array_type_node()
                .ok_or(Error::MissingLink("array type"))?
                .element_type()
                .ok_or(Error::MissingLink("array element"))?]
        } else {
            self.source_list(node, read.element_list())?
        };
        for node in nodes {
            if self.may_resolve_type_alias(node)? {
                return Ok(true);
            }
        }
        Ok(false)
    }

    fn is_resolved_by_type_alias(&self, node: NodeId) -> Result<bool, Error> {
        let mut parent = self.ast(node)?.node(node)?.parent();
        while let Some(node) = parent {
            let read = self.ast(node)?.node(node)?;
            match read.kind().known() {
                Some(K::TypeAliasDeclaration | K::JSTypeAliasDeclaration) => return Ok(true),
                Some(
                    K::ParenthesizedType
                    | K::NamedTupleMember
                    | K::TypeReference
                    | K::UnionType
                    | K::IntersectionType
                    | K::IndexedAccessType
                    | K::ConditionalType
                    | K::TypeOperator
                    | K::ArrayType
                    | K::TupleType,
                ) => parent = read.parent(),
                _ => return Ok(false),
            }
        }
        Ok(false)
    }

    fn may_resolve_type_alias(&mut self, node: NodeId) -> Result<bool, Error> {
        let read = self.ast(node)?.node(node)?;
        match read.kind().known() {
            Some(K::TypeReference) => {
                let symbol = self.resolve_type_reference_symbol(node, false)?;
                Ok(self.symbol(symbol)?.flags() & sf::TYPE_ALIAS != 0)
            }
            Some(K::TypeQuery) => Ok(true),
            Some(
                K::ParenthesizedType
                | K::OptionalType
                | K::RestType
                | K::NamedTupleMember
                | K::TypeOperator,
            ) => {
                let node = read.type_node().ok_or(Error::MissingLink("wrapped type"))?;
                self.may_resolve_type_alias(node)
            }
            Some(
                K::UnionType | K::IntersectionType | K::IndexedAccessType | K::ConditionalType,
            ) => {
                for child in self.source_children(node)? {
                    if self.may_resolve_type_alias(child)? {
                        return Ok(true);
                    }
                }
                Ok(false)
            }
            _ => Ok(false),
        }
    }

    // port: tsc/internal/checker/checker.go:Checker.createDeferredTypeReference
    pub(crate) fn create_deferred_type_reference(
        &mut self,
        target: TypeId,
        node: NodeId,
        mapper: Option<MapperId>,
        alias: Option<AliasId>,
    ) -> Result<TypeId, Error> {
        let alias = if let Some(alias) = alias {
            Some(alias)
        } else if let Some(mut alias) = self.alias_for_type_node(node)? {
            alias.type_arguments = self.instantiate_types(&alias.type_arguments, mapper)?;
            Some(self.types.push_alias(alias)?)
        } else {
            None
        };
        let ty = self.new_object_type(of::REFERENCE, self.types.get(target)?.symbol)?;
        self.types.get_mut(ty)?.alias = alias;
        let reference = self.types.type_reference_mut(ty)?;
        reference.object.target = Some(target);
        reference.object.mapper = mapper;
        reference.node = Some(node);
        Ok(ty)
    }

    // port: tsc/internal/checker/checker.go:Checker.getTypeArguments
    pub(crate) fn get_type_arguments(&mut self, ty: TypeId) -> Result<TypeList, Error> {
        let reference = self.types.type_reference(ty)?;
        if let Some(arguments) = &reference.resolved_type_arguments {
            return Ok(arguments.clone());
        }
        let target = reference
            .object
            .target
            .ok_or(Error::MissingLink("reference target"))?;
        let mapper = reference.object.mapper;
        let node = reference.node;
        let interface = self.types.interface(target)?;
        let parameters = interface.type_parameters().to_vec();
        let outer_count = interface.outer_type_parameter_count as usize;
        let types = &self.types;
        if !self.resolution.push(
            TypeSystemEntity::Type(ty),
            TypeSystemPropertyName::ResolvedTypeArguments,
            |resolution| {
                let TypeSystemEntity::Type(ty) = resolution.target else {
                    return false;
                };
                resolution.property_name == TypeSystemPropertyName::ResolvedTypeArguments
                    && types
                        .type_reference(ty)
                        .is_ok_and(|reference| reference.resolved_type_arguments.is_some())
            },
        ) {
            return Ok(vec![self.builtins.error_type; parameters.len()].into());
        }
        let arguments = (|| {
            let Some(node) = node else {
                return Ok([].into());
            };
            let read = self.ast(node)?.node(node)?;
            match read.kind().known() {
                Some(K::TypeReference) => {
                    let local = self.effective_type_arguments(node, &parameters[outer_count..])?;
                    let mut result = parameters[..outer_count].to_vec();
                    result.extend_from_slice(&local);
                    Ok(result.into())
                }
                Some(K::ArrayType) => {
                    let element = read
                        .data_source()
                        .as_array_type_node()
                        .ok_or(Error::MissingLink("array type"))?
                        .element_type()
                        .ok_or(Error::MissingLink("array element"))?;
                    Ok(vec![self.get_type_from_type_node(element)?].into())
                }
                Some(K::TupleType) => {
                    let mut result = Vec::new();
                    for node in self.source_list(node, read.element_list())? {
                        result.push(self.get_type_from_type_node(node)?);
                    }
                    Ok(result.into())
                }
                _ => Err(Error::MissingLink("deferred reference source kind")),
            }
        })();
        let complete = self.resolution.pop();
        let arguments: TypeList = arguments?;
        if !complete {
            self.types
                .type_reference_mut(ty)?
                .resolved_type_arguments
                .get_or_insert_with(|| vec![self.builtins.error_type; parameters.len()].into());
            let (message, args) = if let Some(symbol) = self.types.get(target)?.symbol {
                (
                    ts_diagnostics::Type_arguments_for_0_circularly_reference_themselves,
                    vec![self.symbol_to_string(symbol)?],
                )
            } else {
                (
                    ts_diagnostics::Tuple_type_arguments_circularly_reference_themselves,
                    vec![],
                )
            };
            self.error_at(node.or(self.current_node), message, args)?;
            return self
                .types
                .type_reference(ty)?
                .resolved_type_arguments
                .clone()
                .ok_or(Error::MissingLink("circular arguments"));
        }
        let arguments = self.instantiate_types(&arguments, mapper)?;
        Ok(self
            .types
            .type_reference_mut(ty)?
            .resolved_type_arguments
            .get_or_insert(arguments)
            .clone())
    }
}
