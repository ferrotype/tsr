//! Class instance/constructor identities and lazy heritage resolution. Base
//! constructors live in the same interface payload and resolution stack as Go.

use crate::{
    object_flags as of, signature_flags as sg, type_flags as tf, CheckerState, Error, SignatureId,
    TypeId, TypeList, TypeSystemEntity, TypeSystemPropertyName,
};
use ts_arena::{NodeId, SymbolId};
use ts_ast::{modifier_flags as mf, node_flags as nf, symbol_flags as sf, SyntaxKind as K};
use ts_diagnostics as messages;

impl CheckerState {
    pub(crate) fn class_declaration(&self, symbol: SymbolId) -> Result<Option<NodeId>, Error> {
        for node in self.symbol_declarations(symbol)?.iter().flatten() {
            if matches!(
                self.node(node)?.kind().known(),
                Some(K::ClassDeclaration | K::ClassExpression)
            ) {
                return Ok(Some(node));
            }
        }
        Ok(None)
    }

    pub(crate) fn class_heritage_clauses(&self, node: NodeId) -> Result<Vec<NodeId>, Error> {
        let read = self.node(node)?;
        let clauses = if let Some(data) = read.data_source().as_class_declaration() {
            data.heritage_clauses()
        } else if let Some(data) = read.data_source().as_class_expression() {
            data.heritage_clauses()
        } else {
            return Err(Error::MissingLink("class declaration payload"));
        };
        self.source_list(node, clauses)
    }

    pub(crate) fn class_heritage_nodes(
        &self,
        node: NodeId,
        token: K,
    ) -> Result<Vec<NodeId>, Error> {
        for clause in self.class_heritage_clauses(node)? {
            let read = self.node(clause)?;
            let data = read
                .data_source()
                .as_heritage_clause()
                .ok_or(Error::MissingLink("class heritage clause"))?;
            if data.token() == token {
                return self.source_list(clause, data.types());
            }
        }
        Ok(Vec::new())
    }

    // port: tsc/internal/checker/checker.go:getBaseTypeNodeOfClass
    pub(crate) fn class_base_type_node(&self, ty: TypeId) -> Result<Option<NodeId>, Error> {
        let symbol = self
            .types
            .get(ty)?
            .symbol
            .ok_or(Error::MissingLink("class type symbol"))?;
        let Some(declaration) = self.class_declaration(symbol)? else {
            return Ok(None);
        };
        Ok(self
            .class_heritage_nodes(declaration, K::ExtendsKeyword)?
            .first()
            .copied())
    }

    pub(crate) fn class_local_type_parameters(&self, ty: TypeId) -> Result<TypeList, Error> {
        let data = self.types.interface(ty)?;
        Ok(data.type_parameters()[data.outer_type_parameter_count as usize..].into())
    }

    // port: tsc/internal/checker/checker.go:Checker.getTypeOfFuncClassEnumModuleWorker
    pub(crate) fn type_of_class(&mut self, symbol: SymbolId) -> Result<TypeId, Error> {
        if let Some(ty) = self
            .value_symbol_links
            .try_get(symbol)
            .and_then(|links| links.resolved_type)
        {
            return Ok(ty);
        }
        let ty = self.new_object_type(of::ANONYMOUS, Some(symbol))?;
        let variable = self.class_base_type_variable(symbol)?;
        let result = if let Some(variable) = variable {
            self.get_intersection_type(&[ty, variable])?
        } else {
            ty
        };
        self.value_symbol_links.get_or_default(symbol).resolved_type = Some(result);
        Ok(result)
    }

    // port: tsc/internal/checker/checker.go:Checker.getBaseTypeVariableOfClass
    pub(crate) fn class_base_type_variable(
        &mut self,
        symbol: SymbolId,
    ) -> Result<Option<TypeId>, Error> {
        let class = self.declared_interface_type(symbol)?;
        let base = self.class_base_constructor_type(class)?;
        Ok(if self.types.flags(base)? & tf::TYPE_VARIABLE != 0 {
            Some(base)
        } else if self.types.flags(base)? & tf::INTERSECTION != 0 {
            let mut variable = None;
            for &part in self.types.compound_types(base)?.iter() {
                if self.types.flags(part)? & tf::TYPE_VARIABLE != 0 {
                    variable = Some(part);
                    break;
                }
            }
            variable
        } else {
            None
        })
    }

    // port: tsc/internal/checker/checker.go:Checker.getTypeOfPrototypeProperty
    pub(crate) fn type_of_prototype(&mut self, symbol: SymbolId) -> Result<TypeId, Error> {
        let parent = self
            .parent_of_symbol(symbol)?
            .ok_or(Error::MissingLink("prototype class parent"))?;
        let class = self.get_declared_type_of_symbol(parent)?;
        let count = self.types.interface(class)?.type_parameters().len();
        if count == 0 {
            return Ok(class);
        }
        self.create_type_reference(class, &vec![self.builtins.any_type; count])
    }

    // port: tsc/internal/checker/checker.go:Checker.getBaseConstructorTypeOfClass
    pub(crate) fn class_base_constructor_type(&mut self, ty: TypeId) -> Result<TypeId, Error> {
        if let Some(base) = self.types.interface(ty)?.resolved_base_constructor_type {
            return Ok(base);
        }
        let Some(base_node) = self.class_base_type_node(ty)? else {
            self.types.interface_mut(ty)?.resolved_base_constructor_type =
                Some(self.builtins.undefined_type);
            return Ok(self.builtins.undefined_type);
        };
        let types = &self.types;
        if !self.resolution.push(
            TypeSystemEntity::Type(ty),
            TypeSystemPropertyName::ResolvedBaseConstructorType,
            |entry| {
                let TypeSystemEntity::Type(ty) = entry.target else {
                    return false;
                };
                entry.property_name == TypeSystemPropertyName::ResolvedBaseConstructorType
                    && types
                        .interface(ty)
                        .is_ok_and(|data| data.resolved_base_constructor_type.is_some())
            },
        ) {
            return Ok(self.builtins.error_type);
        }
        let result: Result<TypeId, Error> = (|| {
            let expression = self
                .ast(base_node)?
                .node(base_node)?
                .expression()
                .ok_or(Error::MissingLink("class extends expression"))?;
            let base = self.check_expression(expression)?;
            if self.types.flags(base)? & (tf::OBJECT | tf::INTERSECTION) != 0 {
                self.resolve_type_members(base)?;
            }
            Ok(base)
        })();
        let complete = self.resolution.pop();
        let base = result?;
        if !complete {
            let symbol = self
                .types
                .get(ty)?
                .symbol
                .ok_or(Error::MissingLink("class base symbol"))?;
            let name = self.symbol_to_string(symbol)?;
            self.error_at(
                self.symbol(symbol)?.value_declaration(),
                messages::X_0_is_referenced_directly_or_indirectly_in_its_own_base_expression,
                vec![name],
            )?;
            let error_type = self.builtins.error_type;
            return Ok(*self
                .types
                .interface_mut(ty)?
                .resolved_base_constructor_type
                .get_or_insert(error_type));
        }
        if self.types.flags(base)? & tf::ANY == 0
            && base != self.builtins.null_widening_type
            && !self.is_constructor_type(base)?
        {
            let expression = self
                .ast(base_node)?
                .node(base_node)?
                .expression()
                .ok_or(Error::MissingLink("class extends expression"))?;
            let text = self.type_to_string(base, crate::type_display::DEFAULT_FLAGS)?;
            let mut diagnostic = self.diagnostic_for_node(
                Some(expression),
                messages::Type_0_is_not_a_constructor_function_type,
                vec![text],
            )?;
            if self.types.flags(base)? & tf::TYPE_PARAMETER != 0 {
                let constraint = self.constraint_of_type_parameter(base)?;
                let mut result = self.builtins.unknown_type;
                if let Some(constraint) = constraint {
                    if let Some(&signature) = self.signatures_of_type(constraint, true)?.first() {
                        result = self.return_type_of_signature(signature)?;
                    }
                }
                if let Some(symbol) = self.types.get(base)?.symbol {
                    if let Some(declaration) = self.symbol_declarations(symbol)?.first().flatten() {
                        let name = self.symbol_to_string(symbol)?;
                        let result =
                            self.type_to_string(result, crate::type_display::DEFAULT_FLAGS)?;
                        diagnostic.related_information.push(std::sync::Arc::new(self.diagnostic_for_node(Some(declaration), messages::Did_you_mean_for_0_to_be_constrained_to_type_new_args_Colon_any_1, vec![name, result])?));
                    }
                }
            }
            self.add_diagnostic(diagnostic)?;
            let error_type = self.builtins.error_type;
            return Ok(*self
                .types
                .interface_mut(ty)?
                .resolved_base_constructor_type
                .get_or_insert(error_type));
        }
        Ok(*self
            .types
            .interface_mut(ty)?
            .resolved_base_constructor_type
            .get_or_insert(base))
    }

    // port: tsc/internal/checker/checker.go:Checker.isConstructorType
    pub(crate) fn is_constructor_type(&mut self, ty: TypeId) -> Result<bool, Error> {
        if !self.signatures_of_type(ty, true)?.is_empty() {
            return Ok(true);
        }
        if self.types.flags(ty)? & tf::TYPE_VARIABLE != 0 {
            if let Some(constraint) = self.base_constraint_of_type(ty)? {
                return self.is_mixin_constructor_type(constraint);
            }
        }
        Ok(false)
    }

    // port: tsc/internal/checker/checker.go:Checker.isMixinConstructorType
    pub(crate) fn is_mixin_constructor_type(&mut self, ty: TypeId) -> Result<bool, Error> {
        let signatures = self.signatures_of_type(ty, true)?;
        if signatures.len() != 1 {
            return Ok(false);
        }
        let signature = self.signatures.get(signatures[0])?;
        if signature
            .type_parameters
            .as_ref()
            .is_some_and(|types| !types.is_empty())
            || signature
                .parameters
                .as_ref()
                .is_none_or(|parameters| parameters.len() != 1)
            || signature.flags & sg::HAS_REST_PARAMETER == 0
        {
            return Ok(false);
        }
        let symbol = signature
            .parameters
            .as_ref()
            .expect("single parameter checked")[0];
        let parameter = self.get_type_of_symbol(symbol)?;
        if self.types.flags(parameter)? & tf::ANY != 0 {
            return Ok(true);
        }
        if !self.is_array_type(parameter)? {
            return Ok(false);
        }
        Ok(self.get_type_arguments(parameter)?.first() == Some(&self.builtins.any_type))
    }

    // port: tsc/internal/checker/checker.go:Checker.areAllOuterTypeParametersApplied
    fn all_outer_type_parameters_applied(&mut self, ty: TypeId) -> Result<bool, Error> {
        let data = self.types.interface(ty)?;
        let count = data.outer_type_parameter_count as usize;
        if count == 0 {
            return Ok(true);
        }
        let parameter = data.type_parameters()[count - 1];
        let arguments = self.get_type_arguments(ty)?;
        let argument = arguments
            .get(count - 1)
            .ok_or(Error::MissingLink("outer class type argument"))?;
        Ok(self.types.get(parameter)?.symbol != self.types.get(*argument)?.symbol)
    }

    // port: tsc/internal/checker/checker.go:Checker.resolveBaseTypesOfClass
    pub(crate) fn resolve_class_base_types(&mut self, ty: TypeId) -> Result<(), Error> {
        let constructor = self.class_base_constructor_type(ty)?;
        let constructor = self.apparent_type(constructor)?;
        if self.types.flags(constructor)? & (tf::OBJECT | tf::INTERSECTION | tf::ANY) == 0 {
            return Ok(());
        }
        let base_node = self
            .class_base_type_node(ty)?
            .ok_or(Error::MissingLink("class base type node"))?;
        let symbol = self.types.get(constructor)?.symbol;
        let original = symbol
            .map(|symbol| self.get_declared_type_of_symbol(symbol))
            .transpose()?;
        let base = if let (Some(symbol), Some(original)) = (symbol, original) {
            if self.symbol(symbol)?.flags() & sf::CLASS != 0
                && self.all_outer_type_parameters_applied(original)?
            {
                self.type_reference_from_symbol(base_node, symbol)?
            } else {
                self.base_constructor_instance(constructor, base_node)?
            }
        } else if self.types.flags(constructor)? & tf::ANY != 0 {
            constructor
        } else {
            self.base_constructor_instance(constructor, base_node)?
        };
        if self.is_error_type(base)? {
            return Ok(());
        }
        let reduced = self.get_reduced_type(base)?;
        if !self.is_valid_base_type(reduced)? {
            let expression = self
                .ast(base_node)?
                .node(base_node)?
                .expression()
                .ok_or(Error::MissingLink("base type expression"))?;
            let chain = self.elaborate_never_intersection(None, expression, base)?;
            let text = self.type_to_string(reduced, crate::type_display::DEFAULT_FLAGS)?;
            let message = messages::Base_constructor_return_type_0_is_not_an_object_type_or_intersection_of_object_types_with_statically_known_members;
            let diagnostic = if chain.is_some() {
                ts_ast::Diagnostic::chain(chain, message, vec![text])
            } else {
                self.diagnostic_for_node(Some(expression), message, vec![text])?
            };
            self.add_diagnostic(diagnostic)?;
        } else if ty == reduced || self.has_base_type(reduced, ty)? {
            let symbol = self
                .types
                .get(ty)?
                .symbol
                .ok_or(Error::MissingLink("class symbol"))?;
            let declaration = self
                .symbol(symbol)?
                .value_declaration()
                .ok_or(Error::MissingLink("class declaration"))?;
            self.report_circular_base_type(declaration, ty)?;
        } else {
            self.types.interface_mut(ty)?.resolved_base_types = Some(vec![reduced].into());
        }
        Ok(())
    }

    fn base_constructor_instance(
        &mut self,
        constructor: TypeId,
        base_node: NodeId,
    ) -> Result<TypeId, Error> {
        let constructors = self.instantiated_constructors_for_arguments(constructor, base_node)?;
        if let Some(&constructor) = constructors.first() {
            return self.return_type_of_signature(constructor);
        }
        let expression = self.node(base_node)?.expression();
        self.error_at(
            expression,
            messages::No_base_constructor_has_the_specified_number_of_type_arguments,
            vec![],
        )?;
        Ok(self.builtins.error_type)
    }

    // port: tsc/internal/checker/checker.go:Checker.getConstructorsForTypeArguments
    pub(crate) fn constructors_for_arguments(
        &mut self,
        ty: TypeId,
        node: NodeId,
    ) -> Result<Vec<SignatureId>, Error> {
        let nodes = self.source_list(node, self.node(node)?.type_argument_list())?;
        let mut signatures = Vec::new();
        for signature in self.signatures_of_type(ty, true)? {
            let parameters = self
                .signatures
                .get(signature)?
                .type_parameters
                .as_deref()
                .unwrap_or_default();
            if nodes.len() >= self.min_type_argument_count(parameters)?
                && nodes.len() <= parameters.len()
            {
                signatures.push(signature);
            }
        }
        Ok(signatures)
    }

    // port: tsc/internal/checker/checker.go:Checker.getInstantiatedConstructorsForTypeArguments
    pub(crate) fn instantiated_constructors_for_arguments(
        &mut self,
        ty: TypeId,
        node: NodeId,
    ) -> Result<Vec<SignatureId>, Error> {
        let nodes = self.source_list(node, self.node(node)?.type_argument_list())?;
        let mut signatures = self.constructors_for_arguments(ty, node)?;
        let mut arguments = Vec::with_capacity(nodes.len());
        for node in nodes {
            arguments.push(self.get_type_from_type_node(node)?);
        }
        let javascript = self.node(node)?.flags() & nf::JAVA_SCRIPT_FILE != 0;
        for signature in &mut signatures {
            if self
                .signatures
                .get(*signature)?
                .type_parameters
                .as_ref()
                .is_some_and(|parameters| !parameters.is_empty())
            {
                *signature = self.signature_instantiation(*signature, &arguments, javascript)?;
            }
        }
        Ok(signatures)
    }

    // port: tsc/internal/checker/checker.go:Checker.getDefaultConstructSignatures
    pub(crate) fn default_construct_signatures(
        &mut self,
        class: TypeId,
    ) -> Result<Vec<SignatureId>, Error> {
        let base = self.class_base_constructor_type(class)?;
        let signatures = self.signatures_of_type(base, true)?;
        let symbol = self
            .types
            .get(class)?
            .symbol
            .ok_or(Error::MissingLink("class symbol"))?;
        let declaration = self.class_declaration(symbol)?;
        let abstract_class = declaration
            .map(|node| {
                self.node(node)?
                    .modifier_flags(self.ast(node)?)
                    .map_err(Error::from)
            })
            .transpose()?
            .unwrap_or(0)
            & mf::ABSTRACT
            != 0;
        let parameters = self.class_local_type_parameters(class)?;
        if signatures.is_empty() {
            return Ok(vec![self.signatures.new_signature(
                sg::CONSTRUCT | if abstract_class { sg::ABSTRACT } else { 0 },
                None,
                (!parameters.is_empty()).then_some(parameters),
                None,
                None,
                Some(class),
                None,
                0,
            )?]);
        }
        let base_node = self
            .class_base_type_node(class)?
            .ok_or(Error::MissingLink("derived constructor base node"))?;
        let mut arguments = Vec::new();
        for node in self.source_list(base_node, self.node(base_node)?.type_argument_list())? {
            arguments.push(self.get_type_from_type_node(node)?);
        }
        let javascript = match declaration {
            Some(node) => self.node(node)?.flags() & nf::JAVA_SCRIPT_FILE != 0,
            None => false,
        };
        let mut result = Vec::new();
        for signature in signatures {
            let base_parameters = self
                .signatures
                .get(signature)?
                .type_parameters
                .clone()
                .unwrap_or_default();
            if !javascript
                && (arguments.len() < self.min_type_argument_count(&base_parameters)?
                    || arguments.len() > base_parameters.len())
            {
                continue;
            }
            let fresh = if base_parameters.is_empty() {
                self.clone_signature(signature)?
            } else {
                let arguments =
                    self.fill_missing_type_arguments(&arguments, &base_parameters, javascript)?;
                // This creates a fresh signature: unlike getSignatureInstantiation,
                // the following class-specific mutations must not modify a cached one.
                let mapper = self.new_type_mapper(&base_parameters, &arguments)?;
                self.instantiate_signature_ex(signature, mapper, true)?
            };
            let sig = self.signatures.get_mut(fresh)?;
            sig.type_parameters = (!parameters.is_empty()).then(|| parameters.clone());
            sig.resolved_return_type = Some(class);
            sig.flags = (sig.flags & !sg::ABSTRACT) | if abstract_class { sg::ABSTRACT } else { 0 };
            result.push(fresh);
        }
        Ok(result)
    }
}
