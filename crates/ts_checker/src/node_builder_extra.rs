//! Array, tuple and signature syntax emitted by the type node builder.

use super::{
    check_flags, emit_flags, nf, sf, Error, Factory, FactoryMethods, JsString, NodeBuilder, NodeId,
    SymbolId, TypeId, K,
};
use crate::{element_flags as ef, signature_flags as sg, IndexInfoId, SignatureId};

impl NodeBuilder<'_> {
    // port: tsc/internal/checker/nodebuilderimpl.go:NodeBuilderImpl.typeReferenceToTypeNode
    pub(super) fn reference_type_node(&mut self, ty: TypeId) -> Result<NodeId, Error> {
        if self.checker.is_array_type(ty)? || self.checker.is_tuple_type(ty)? {
            return self.array_or_tuple_node(ty);
        }
        if self.inaccessible_class_reference(ty)? {
            return self.anonymous_type_node(ty);
        }
        let target = self.checker.types.target(ty)?;
        let arguments = self.checker.get_type_arguments(ty)?;
        let interface = self.checker.types.interface(target)?;
        let outer = interface.outer_type_parameter_count as usize;
        if arguments[..outer] != interface.type_parameters()[..outer] {
            return Err(Error::Unsupported(
                "typeReferenceToTypeNode: applied outer arguments",
            ));
        }
        let arity = self.reference_display_arity(ty, &arguments)?;
        self.type_reference(
            self.checker
                .types
                .get(ty)?
                .symbol
                .ok_or(Error::MissingLink("reference symbol"))?,
            &arguments[outer..arity],
        )
    }

    // port: tsc/internal/checker/nodebuilderimpl.go:NodeBuilderImpl.conditionalTypeToTypeNode
    pub(super) fn conditional_type_node(&mut self, ty: TypeId) -> Result<NodeId, Error> {
        if self.check_truncation() {
            return self.elision(b"...");
        }
        let data = *self.checker.types.conditional(ty)?;
        let root = self.checker.conditional_root(data.root)?.clone();
        if self.flags & nf::GENERATE_NAMES_FOR_SHADOWED_TYPE_PARAMS != 0
            && root.distributive
            && self.checker.types.flags(data.check_type)? & crate::type_flags::TYPE_PARAMETER == 0
        {
            return Err(Error::Unsupported(
                "conditionalTypeToTypeNode: shadowed distribution parameter",
            ));
        }
        let check = self.type_node(data.check_type)?;
        self.approximate_length += 15;
        let previous = std::mem::replace(&mut self.infer_parameters, root.infer_parameters);
        let extends = self.type_node(data.extends_type);
        self.infer_parameters = previous;
        let extends = extends?;
        let yes = self.checker.conditional_true_type(ty, false)?;
        let no = self.checker.conditional_false_type(ty)?;
        let yes = self.type_node_or_circularity_elision(yes)?;
        let no = self.type_node_or_circularity_elision(no)?;
        Ok(self
            .ast
            .new_conditional_type_node(Some(check), Some(extends), Some(yes), Some(no)))
    }

    // port: tsc/internal/checker/nodebuilderimpl.go:NodeBuilderImpl.typeReferenceToTypeNode
    pub(super) fn array_or_tuple_node(&mut self, ty: TypeId) -> Result<NodeId, Error> {
        let target = self.checker.types.target(ty)?;
        let arguments = self.checker.element_types(ty)?;
        if self.checker.is_array_type(ty)? {
            let readonly = target == self.checker.array_target(true)?
                && target != self.checker.array_target(false)?;
            let element = self.type_node(arguments[0])?;
            if self.flags & nf::WRITE_ARRAY_AS_GENERIC_TYPE != 0 {
                let name = self.ast.new_identifier(JsString::from_bytes(if readonly {
                    b"ReadonlyArray".as_slice()
                } else {
                    b"Array".as_slice()
                }));
                let arguments = self.list(vec![element])?;
                return Ok(self
                    .ast
                    .new_type_reference_node(Some(name), Some(arguments)));
            }
            let node = self.ast.new_array_type_node(Some(element));
            return Ok(if readonly {
                self.ast
                    .new_type_operator_node(K::ReadonlyKeyword.into(), Some(node))
            } else {
                node
            });
        }
        let data = self.checker.types.tuple(target)?;
        let readonly = data.readonly;
        let infos = data.element_infos.clone();
        let mut element_types = Vec::with_capacity(infos.len());
        for (&argument, info) in arguments.iter().zip(infos.iter()) {
            element_types.push(self.without_missing(argument, info.flags & ef::OPTIONAL != 0)?);
        }
        let element_nodes = self.type_nodes(&element_types)?;
        let mut nodes = Vec::new();
        for (mut node, info) in element_nodes.into_iter().zip(infos.iter()) {
            if info.flags & ef::REST != 0 {
                node = self.ast.new_array_type_node(Some(node));
            }
            let question = (info.flags & ef::OPTIONAL != 0)
                .then(|| self.ast.new_token(K::QuestionToken.into()));
            let rest = (info.flags & ef::VARIABLE != 0)
                .then(|| self.ast.new_token(K::DotDotDotToken.into()));
            if let Some(declaration) = info.labeled_declaration {
                let view = self.checker.ast(declaration)?;
                let name = view
                    .node(declaration)?
                    .name()
                    .ok_or(Error::MissingLink("tuple label"))?;
                if view.node(name)?.kind() != K::Identifier {
                    return Err(Error::Unsupported("getTupleElementLabel: binding pattern"));
                }
                let name = self
                    .ast
                    .new_identifier(view.node_text(name)?.into_js_string());
                node = self
                    .ast
                    .new_named_tuple_member(rest, Some(name), question, Some(node));
            } else if rest.is_some() {
                node = self.ast.new_rest_type_node(Some(node));
            } else if question.is_some() {
                node = self.ast.new_optional_type_node(Some(node));
            }
            nodes.push(node);
        }
        let elements = self.list(nodes)?;
        let tuple = self.ast.new_tuple_type_node(Some(elements));
        self.emit.set_emit_flags(tuple, emit_flags::SINGLE_LINE);
        Ok(if readonly {
            self.ast
                .new_type_operator_node(K::ReadonlyKeyword.into(), Some(tuple))
        } else {
            tuple
        })
    }

    // port: tsc/internal/checker/nodebuilderimpl.go:NodeBuilderImpl.typeReferenceToTypeNode
    pub(super) fn reference_display_arity(
        &mut self,
        ty: TypeId,
        arguments: &[TypeId],
    ) -> Result<usize, Error> {
        let target = self.checker.types.target(ty)?;
        let parameters = self
            .checker
            .types
            .interface(target)?
            .type_parameters()
            .to_vec();
        let mut count = parameters.len().min(arguments.len());
        // The pin elides trailing defaults only for these four global identities,
        // not arbitrary interfaces or a same-spelled declaration in another scope.
        let mut elide = false;
        for name in [
            "Iterable",
            "IterableIterator",
            "AsyncIterable",
            "AsyncIterableIterator",
        ] {
            let global = self.checker.iteration_global(name, 3)?;
            if self.checker.iteration_is_reference(ty, global)? {
                elide = true;
                break;
            }
        }
        if elide {
            let explicit = if let Some(node) = self.checker.types.type_reference(ty)?.node {
                let read = self.checker.ast(node)?.node(node)?;
                read.kind() == K::TypeReference
                    && self
                        .checker
                        .source_list(node, read.type_argument_list())?
                        .len()
                        >= count
            } else {
                false
            };
            if !explicit {
                while count > 0 {
                    let default = self
                        .checker
                        .resolved_type_parameter_default(parameters[count - 1])?;
                    if default == self.checker.builtins.no_constraint_type
                        || default == self.checker.builtins.circular_constraint_type
                        || !self.checker.is_type_related_to(
                            arguments[count - 1],
                            default,
                            crate::RelationKind::Identity,
                        )?
                    {
                        break;
                    }
                    count -= 1;
                }
            }
        }
        Ok(count)
    }

    fn without_missing(&mut self, ty: TypeId, optional: bool) -> Result<TypeId, Error> {
        if !optional || !self.checker.options.exact_optional_property_types {
            return Ok(ty);
        }
        let missing = self.checker.builtins.missing_type;
        Ok(self
            .checker
            .map_type(ty, &mut |_, ty| Ok((ty != missing).then_some(ty)))?
            .unwrap_or(self.checker.builtins.never_type))
    }

    // port: tsc/internal/checker/nodebuilderimpl.go:NodeBuilderImpl.typeParameterToDeclaration
    pub(super) fn type_parameter_node(&mut self, ty: TypeId) -> Result<NodeId, Error> {
        let constraint = if let Some(constraint) = self.checker.constraint_of_type_parameter(ty)? {
            let annotation = self.checker.constraint_declaration(ty)?;
            Some(self.type_node_with_reusable_annotation(constraint, annotation)?)
        } else {
            None
        };
        self.type_parameter_node_with_constraint(ty, constraint)
    }

    // port: tsc/internal/checker/nodebuilderimpl.go:NodeBuilderImpl.typeParameterToDeclarationWithConstraint
    pub(super) fn type_parameter_node_with_constraint(
        &mut self,
        ty: TypeId,
        constraint: Option<NodeId>,
    ) -> Result<NodeId, Error> {
        let flags = self.flags;
        self.flags &= !nf::WRITE_TYPE_PARAMETERS_IN_QUALIFIED_NAME;
        let result = (|| {
            let modifier_flags = self.checker.type_parameter_modifiers(ty)?;
            let modifiers = ts_ast::utilities_middle::create_modifiers_from_modifier_flags(
                modifier_flags,
                |kind| Some(self.ast.new_modifier(kind)),
            )
            .map(|nodes| self.list(nodes.into_iter().flatten().collect()))
            .transpose()?;
            let name = self.type_parameter_name(ty)?;
            let default = self.checker.resolved_type_parameter_default(ty)?;
            let default = if default == self.checker.builtins.no_constraint_type
                || default == self.checker.builtins.circular_constraint_type
            {
                None
            } else {
                Some(self.type_node(default)?)
            };
            Ok(self.ast.new_type_parameter_declaration(
                modifiers,
                Some(name),
                constraint,
                None,
                default,
            ))
        })();
        self.flags = flags;
        result
    }

    // port: tsc/internal/checker/nodebuilderimpl.go:NodeBuilderImpl.symbolToParameterDeclaration
    fn parameter_node(&mut self, symbol: SymbolId) -> Result<NodeId, Error> {
        let value = self.checker.symbol(symbol)?;
        let text = value.name_to_owned();
        let mut source_name = None;
        let mut rest = value.check_flags() & check_flags::REST_PARAMETER != 0;
        let mut optional = value.check_flags() & check_flags::OPTIONAL_PARAMETER != 0;
        let declaration = value.value_declaration();
        if let Some(node) = value.value_declaration() {
            let read = self.checker.ast(node)?.node(node)?;
            if let Some(name) = read.name() {
                source_name = Some(name);
            }
            if let Some(parameter) = read.data_source().as_parameter_declaration() {
                rest |= parameter.dot_dot_dot_token().is_some();
            }
            optional |= self.checker.is_optional_source_parameter(node)?;
        }
        let ty = self.checker.get_type_of_symbol(symbol)?;
        let annotation =
            self.serialize_declaration_type(declaration, Some(ty), Some(symbol), true)?;
        let name = match source_name {
            Some(name) => self.clone_binding_name(name)?,
            None => self.ast.new_identifier(text.clone()),
        };
        let rest = rest.then(|| self.ast.new_token(K::DotDotDotToken.into()));
        let question = optional.then(|| self.ast.new_token(K::QuestionToken.into()));
        self.approximate_length += text.len() + 3;
        Ok(self.ast.new_parameter_declaration(
            None,
            rest,
            Some(name),
            question,
            Some(annotation),
            None,
        ))
    }

    // port: tsc/internal/checker/nodebuilderimpl.go:NodeBuilderImpl.cloneBindingName
    fn clone_binding_name(&mut self, source: NodeId) -> Result<NodeId, Error> {
        self.clone_binding_name_native(source)
    }

    // port: tsc/internal/checker/nodebuilderimpl.go:NodeBuilderImpl.signatureToSignatureDeclarationHelper
    pub(crate) fn signature_node(
        &mut self,
        signature: SignatureId,
        kind: K,
        name: Option<NodeId>,
        question: Option<NodeId>,
    ) -> Result<NodeId, Error> {
        self.with_expanded_signature_scope(signature, |b, expanded| {
            b.signature_node_worker(signature, kind, name, question, expanded)
        })
    }

    fn signature_node_worker(
        &mut self,
        signature: SignatureId,
        kind: K,
        name: Option<NodeId>,
        question: Option<NodeId>,
        expanded: &[SymbolId],
    ) -> Result<NodeId, Error> {
        let sig = self.checker.signatures.get(signature)?.clone();
        self.approximate_length += 3;
        let mut type_parameters = Vec::new();
        for &ty in sig.type_parameters.as_deref().unwrap_or_default() {
            type_parameters.push(self.type_parameter_node(ty)?);
        }
        let type_parameters = if type_parameters.is_empty() {
            None
        } else {
            Some(self.list(type_parameters)?)
        };
        // Parameters do not inherit suppression of the enclosing signature's
        // top-level `any` return type.
        let flags = self.flags;
        self.flags &= !nf::SUPPRESS_ANY_RETURN_TYPE;
        let parameters = (|| {
            let mut parameters = Vec::new();
            if self.flags & nf::OMIT_THIS_PARAMETER == 0 {
                if let Some(this) = sig.this_parameter {
                    parameters.push(self.parameter_node(this)?);
                }
            }
            let mut non_trailing_rest = false;
            for &parameter in expanded {
                if Some(&parameter) != expanded.last()
                    && self.checker.symbol(parameter)?.check_flags()
                        & ts_ast::check_flags::REST_PARAMETER
                        != 0
                {
                    non_trailing_rest = true;
                    break;
                }
            }
            let displayed = if non_trailing_rest {
                sig.parameters.as_deref().unwrap_or_default()
            } else {
                expanded
            };
            for &parameter in displayed {
                parameters.push(self.parameter_node(parameter)?);
            }
            self.list(parameters)
        })();
        self.flags = flags;
        let parameters = parameters?;
        let mut return_type = self.serialize_signature_return(signature, true)?;
        if return_type.is_none() && matches!(kind, K::FunctionType | K::ConstructorType) {
            let empty = self.ast.new_identifier(JsString::default());
            return_type = Some(self.ast.new_type_reference_node(Some(empty), None));
        }
        let modifiers = if kind == K::ConstructorType && sig.flags & sg::ABSTRACT != 0 {
            let abstract_modifier = self.ast.new_modifier(K::AbstractKeyword.into());
            Some(self.list(vec![abstract_modifier])?)
        } else {
            None
        };
        Ok(match kind {
            K::FunctionType => {
                self.ast
                    .new_function_type_node(type_parameters, Some(parameters), return_type)
            }
            K::ConstructorType => self.ast.new_constructor_type_node(
                modifiers,
                type_parameters,
                Some(parameters),
                return_type,
            ),
            K::CallSignature => self.ast.new_call_signature_declaration(
                type_parameters,
                Some(parameters),
                return_type,
            ),
            K::ConstructSignature => self.ast.new_construct_signature_declaration(
                type_parameters,
                Some(parameters),
                return_type,
            ),
            K::GetAccessor => self.ast.new_get_accessor_declaration(
                None,
                name,
                None,
                Some(parameters),
                return_type,
                None,
                None,
            ),
            K::SetAccessor => self.ast.new_set_accessor_declaration(
                None,
                name,
                None,
                Some(parameters),
                None,
                None,
                None,
            ),
            K::MethodSignature => self.ast.new_method_signature_declaration(
                None,
                name,
                question,
                type_parameters,
                Some(parameters),
                return_type,
            ),
            _ => {
                return Err(Error::Unsupported(
                    "signatureToSignatureDeclarationHelper: syntax kind",
                ))
            }
        })
    }

    // port: tsc/internal/checker/nodebuilderimpl.go:NodeBuilderImpl.isTriviallySerializableComputedName
    pub(super) fn serializable_computed_name(
        &mut self,
        declaration: NodeId,
    ) -> Result<bool, Error> {
        let Some(enclosing) = self.enclosing else {
            return Ok(false);
        };
        let Some(name) = self.checker.ast(declaration)?.node(declaration)?.name() else {
            return Ok(false);
        };
        let read = self.checker.ast(name)?.node(name)?;
        if read.kind() != K::ComputedPropertyName {
            return Ok(false);
        }
        let expression = read
            .expression()
            .ok_or(Error::MissingLink("computed index name"))?;
        Ok(
            ts_ast::is_entity_name_expression(self.checker.ast(expression)?, expression)?
                && self
                    .checker
                    .emit_entity_visible_ex(expression, enclosing, false)?
                    .accessibility
                    == ts_printer::emit_resolver::SymbolAccessibility::Accessible,
        )
    }

    // port: tsc/internal/checker/nodebuilderimpl.go:NodeBuilderImpl.indexInfoToObjectComputedNamesOrSignatureDeclaration
    pub(super) fn object_index_nodes(
        &mut self,
        index: IndexInfoId,
        value_node: Option<NodeId>,
    ) -> Result<Vec<NodeId>, Error> {
        let info = self.checker.signatures.index_info(index)?.clone();
        if let Some(components) = info.components.as_ref().filter(|items| !items.is_empty()) {
            let mut serializable = self.enclosing.is_some();
            for &component in components.iter() {
                if !serializable {
                    break;
                }
                serializable = self.serializable_computed_name(component)?;
            }
            if serializable {
                // Native Filter precedes Map: finish all late-name queries before
                // reusing names or serializing component value types.
                let mut selected = Vec::new();
                for &component in components.iter() {
                    let late = if let Some(name) = self.checker.late_name(component)? {
                        let ty = self.checker.late_name_type(name)?;
                        self.checker.types.flags(ty)?
                            & (crate::type_flags::STRING_OR_NUMBER_LITERAL
                                | crate::type_flags::UNIQUE_ES_SYMBOL)
                            != 0
                    } else {
                        false
                    };
                    if !late {
                        selected.push(component);
                    }
                }
                let mut results = Vec::new();
                let mut bailed = false;
                for component in selected {
                    let read = self.checker.ast(component)?.node(component)?;
                    let range = read.range();
                    let name = read
                        .name()
                        .ok_or(Error::MissingLink("computed index name"))?;
                    let postfix = read.postfix_token();
                    if let Some(reused) = self.reuse_node(name)? {
                        let expression = self
                            .checker
                            .ast(name)?
                            .node(name)?
                            .expression()
                            .ok_or(Error::MissingLink("computed index expression"))?;
                        self.reuse_track_computed_name(expression)?;
                        let modifiers = if info.is_readonly {
                            let token = self.ast.new_modifier(K::ReadonlyKeyword.into());
                            Some(self.list(vec![token])?)
                        } else {
                            None
                        };
                        let postfix = postfix.map(|node| ts_ast::clone_node(&mut self.ast, node));
                        let value = if let Some(node) = value_node {
                            ts_ast::deep_clone_node(&mut self.ast, Some(node))
                                .ok_or(Error::MissingLink("index value clone"))?
                        } else {
                            let symbol = self
                                .checker
                                .raw_declaration_symbol(component)?
                                .ok_or(Error::MissingLink("computed index symbol"))?;
                            let ty = self.checker.get_type_of_symbol(symbol)?;
                            self.type_node(ty)?
                        };
                        let node = self.ast.new_property_signature_declaration(
                            modifiers,
                            Some(reused),
                            postfix,
                            Some(value),
                            None,
                        );
                        self.ast.set_node_range(node, range);
                        results.push(node);
                    } else {
                        bailed = true;
                    }
                }
                if !bailed {
                    return Ok(results);
                }
            }
        }
        Ok(vec![self.index_signature_node_with_type(index, value_node)?])
    }

    pub(super) fn index_signature_node(&mut self, index: IndexInfoId) -> Result<NodeId, Error> {
        self.index_signature_node_with_type(index, None)
    }

    pub(super) fn index_signature_node_with_type(
        &mut self,
        index: IndexInfoId,
        value_node: Option<NodeId>,
    ) -> Result<NodeId, Error> {
        let info = self.checker.signatures.index_info(index)?.clone();
        let name = if let Some(declaration) = info.declaration {
            let view = self.checker.ast(declaration)?;
            let parameters = self
                .checker
                .source_list(declaration, view.node(declaration)?.parameter_list())?;
            let name = view
                .node(parameters[0])?
                .name()
                .ok_or(Error::MissingLink("index parameter name"))?;
            view.node_text(name)?.into_js_string()
        } else {
            JsString::from_bytes(b"x".as_slice())
        };
        let name = self.ast.new_identifier(name);
        let key = self.type_node(info.key_type)?;
        let value = match value_node {
            Some(node) => node,
            None => self.type_node(info.value_type)?,
        };
        let parameter =
            self.ast
                .new_parameter_declaration(None, None, Some(name), None, Some(key), None);
        let parameters = self.list(vec![parameter])?;
        let modifiers = if info.is_readonly {
            let token = self.ast.new_modifier(K::ReadonlyKeyword.into());
            Some(self.list(vec![token])?)
        } else {
            None
        };
        Ok(self
            .ast
            .new_index_signature_declaration(modifiers, Some(parameters), Some(value)))
    }

    // port: tsc/internal/checker/nodebuilderimpl.go:NodeBuilderImpl.typePredicateToTypePredicateNodeHelper
    pub(crate) fn predicate_node(
        &mut self,
        predicate: crate::TypePredicateId,
    ) -> Result<NodeId, Error> {
        let data = self.checker.signatures.predicate(predicate)?.clone();
        self.predicate_data_node(data)
    }

    pub(super) fn predicate_data_node(
        &mut self,
        data: crate::TypePredicate,
    ) -> Result<NodeId, Error> {
        use crate::TypePredicateKind as Kind;
        let asserts = matches!(data.kind, Kind::AssertsThis | Kind::AssertsIdentifier)
            .then(|| self.ast.new_token(K::AssertsKeyword.into()));
        let name = if matches!(data.kind, Kind::Identifier | Kind::AssertsIdentifier) {
            let node = self.ast.new_identifier(data.parameter_name);
            self.emit
                .set_emit_flags(node, emit_flags::NO_ASCII_ESCAPING);
            node
        } else {
            self.ast.new_this_type_node()
        };
        let annotation = data.t.map(|ty| self.type_node(ty)).transpose()?;
        Ok(self
            .ast
            .new_type_predicate_node(asserts, Some(name), annotation))
    }

    // port: tsc/internal/checker/nodebuilderimpl.go:NodeBuilderImpl.addPropertyToElementList
    // The property spelling is resolved in its declaration, but accessibility
    // is checked in the declaration being emitted before changing that context.
    fn track_late_property_name(&mut self, symbol: SymbolId) -> Result<(), Error> {
        if !super::names::is_late_bound_name(self.checker.symbol(symbol)?.name_bytes()) {
            return Ok(());
        }
        let declaration = self.checker.symbol_declarations(symbol)?.first().flatten();
        let Some(declaration) = declaration else {
            let name = self.checker.symbol_to_string(symbol)?;
            self.report(
                ts_printer::emit_resolver::DeclarationTrackerEvent::NonSerializableProperty(name),
            );
            return Ok(());
        };
        let Some(name) = self.checker.late_name(declaration)? else {
            return Ok(());
        };
        if !self.reuse_late_bindable_name(name)? {
            return Ok(());
        }
        let read = self.checker.ast(name)?.node(name)?;
        if self.checker.ast(declaration)?.node(declaration)?.kind() == K::BinaryExpression {
            if let Some(access) = read.data_source().as_element_access_expression() {
                if let Some(argument) = access.argument_expression() {
                    if ts_ast::is_property_access_entity_name_expression(
                        self.checker.ast(argument)?,
                        argument,
                        false,
                    )? {
                        self.reuse_track_computed_name(argument)?;
                    }
                }
            }
        } else if let Some(expression) = read.expression() {
            self.reuse_track_computed_name(expression)?;
        }
        Ok(())
    }

    pub(super) fn property_elements(&mut self, symbol: SymbolId) -> Result<Vec<NodeId>, Error> {
        let flags = self.checker.symbol(symbol)?.flags();
        let placeholder = self.reverse_property_placeholder(symbol)?;
        let ty = if placeholder {
            self.checker.builtins.any_type
        } else {
            self.checker.get_type_of_symbol(symbol)?
        };
        let ty = self.without_missing(ty, flags & sf::OPTIONAL != 0)?;
        self.track_late_property_name(symbol)?;
        self.approximate_length += self.checker.symbol(symbol)?.name_bytes().len() + 1;
        if flags & sf::ACCESSOR != 0 {
            let write = self.checker.write_type_of_symbol(symbol)?;
            if ty != self.checker.builtins.error_type && write != self.checker.builtins.error_type {
                let declarations: Vec<_> = self
                    .checker
                    .symbol_declarations(symbol)?
                    .iter()
                    .flatten()
                    .collect();
                let mut property = None;
                for &node in &declarations {
                    if self.checker.ast(node)?.node(node)?.kind() == K::PropertyDeclaration {
                        property = Some(node);
                        break;
                    }
                }
                let class = self
                    .checker
                    .parent_of_symbol(symbol)?
                    .map(|parent| {
                        self.checker
                            .symbol(parent)
                            .map(|read| read.flags() & sf::CLASS != 0)
                    })
                    .transpose()?
                    .unwrap_or(false);
                if ty != write || class && property.is_none() {
                    let mapper = self
                        .checker
                        .value_symbol_links
                        .try_get(symbol)
                        .and_then(|links| links.mapper);
                    let mut nodes = Vec::new();
                    for kind in [K::GetAccessor, K::SetAccessor] {
                        let mut declaration = None;
                        for &node in &declarations {
                            if self.checker.ast(node)?.node(node)?.kind() == kind {
                                declaration = Some(node);
                                break;
                            }
                        }
                        if let Some(declaration) = declaration {
                            let mut signature =
                                self.checker.signature_from_declaration(declaration)?;
                            if let Some(mapper) = mapper {
                                signature =
                                    self.checker.instantiate_signature(signature, mapper)?;
                            }
                            let name = self.property_name_node(symbol)?;
                            nodes.push(self.signature_node(signature, kind, Some(name), None)?);
                        }
                    }
                    return Ok(nodes);
                }
                if class
                    && property
                        .map(|node| {
                            self.checker
                                .ast(node)?
                                .node(node)?
                                .modifier_flags(self.checker.ast(node)?)
                                .map(|flags| flags & ts_ast::modifier_flags::ACCESSOR != 0)
                                .map_err(Error::from)
                        })
                        .transpose()?
                        .unwrap_or(false)
                {
                    let getter = self.checker.signatures.new_signature(
                        0,
                        None,
                        None,
                        None,
                        None,
                        Some(ty),
                        None,
                        0,
                    )?;
                    let parameter = self.checker.new_symbol(
                        sf::FUNCTION_SCOPED_VARIABLE,
                        JsString::from_bytes(b"arg".to_vec()),
                    )?;
                    self.checker
                        .value_symbol_links
                        .get_or_default(parameter)
                        .resolved_type = Some(write);
                    let setter = self.checker.signatures.new_signature(
                        0,
                        None,
                        None,
                        None,
                        Some(vec![parameter].into()),
                        Some(self.checker.builtins.void_type),
                        None,
                        0,
                    )?;
                    let name = self.property_name_node(symbol)?;
                    let getter = self.signature_node(getter, K::GetAccessor, Some(name), None)?;
                    let name = self.property_name_node(symbol)?;
                    let setter = self.signature_node(setter, K::SetAccessor, Some(name), None)?;
                    return Ok(vec![getter, setter]);
                }
            }
        }
        if flags & (sf::METHOD | sf::FUNCTION) != 0
            && (self.checker.types.flags(ty)? & crate::type_flags::OBJECT == 0
                || self.checker.get_properties_of_type(ty)?.is_empty())
            && !self.checker.is_readonly_symbol(symbol)?
        {
            let ty = self
                .checker
                .map_type(ty, &mut |checker, part| {
                    Ok(
                        (checker.types.flags(part)? & crate::type_flags::UNDEFINED == 0)
                            .then_some(part),
                    )
                })?
                .unwrap_or(self.checker.builtins.never_type);
            let signatures = self.checker.signatures_of_type(ty, false)?;
            let mut nodes = Vec::new();
            for signature in signatures {
                let name = self.property_name_node(symbol)?;
                let question = (flags & sf::OPTIONAL != 0)
                    .then(|| self.ast.new_token(K::QuestionToken.into()));
                nodes.push(self.signature_node(
                    signature,
                    K::MethodSignature,
                    Some(name),
                    question,
                )?);
            }
            if !nodes.is_empty() || flags & sf::OPTIONAL == 0 {
                return Ok(nodes);
            }
        }
        self.property(symbol).map(|node| vec![node])
    }
}

impl NodeBuilder<'_> {
    // port: tsc/internal/checker/nodebuilderimpl.go:NodeBuilderImpl.isHomomorphicMappedTypeWithNonHomomorphicInstantiation
    fn non_homomorphic_instantiation(&mut self, ty: TypeId) -> Result<bool, Error> {
        let Some(target) = self.checker.types.object(ty)?.target else {
            return Ok(false);
        };
        Ok(self.checker.homomorphic_type_variable(ty)?.is_none()
            && self.checker.homomorphic_type_variable(target)?.is_some())
    }

    fn mapped_wrapper_variable(&mut self) -> Result<(TypeId, NodeId), Error> {
        let symbol = self
            .checker
            .new_symbol(sf::TYPE_PARAMETER, JsString::from_bytes(b"T".as_slice()))?;
        let parameter = self.checker.new_type_parameter(Some(symbol))?;
        let name = self.type_parameter_name(parameter)?;
        Ok((
            parameter,
            self.ast.new_type_reference_node(Some(name), None),
        ))
    }

    fn mapped_wrapper_infer(
        &mut self,
        variable: NodeId,
        constraint: Option<NodeId>,
    ) -> Result<NodeId, Error> {
        let name = self
            .ast
            .view()
            .node(variable)?
            .data_source()
            .as_type_reference_node()
            .and_then(|data| data.type_name())
            .ok_or(Error::MissingLink("mapped wrapper variable"))?;
        let name = ts_ast::clone_node(&mut self.ast, name);
        let parameter =
            self.ast
                .new_type_parameter_declaration(None, Some(name), constraint, None, None);
        Ok(self.ast.new_infer_type_node(Some(parameter)))
    }

    // port: tsc/internal/checker/nodebuilderimpl.go:NodeBuilderImpl.createMappedTypeNodeFromType
    pub(super) fn mapped_type_node(&mut self, ty: TypeId) -> Result<NodeId, Error> {
        use crate::type_flags as tf;
        let declaration = self.checker.mapped_declaration(ty)?;
        let read = self.checker.ast(declaration)?.node(declaration)?;
        let data = read
            .data_source()
            .as_mapped_type_node()
            .ok_or(Error::MissingLink("mapped display syntax"))?;
        let (readonly, question) = (data.readonly_token(), data.question_token());
        let readonly = readonly
            .map(|node| {
                self.checker
                    .ast(node)?
                    .node(node)
                    .map(|read| read.kind())
                    .map_err(Error::from)
            })
            .transpose()?
            .map(|kind| self.ast.new_token(kind));
        let question = question
            .map(|node| {
                self.checker
                    .ast(node)?
                    .node(node)
                    .map(|read| read.kind())
                    .map_err(Error::from)
            })
            .transpose()?
            .map(|kind| self.ast.new_token(kind));
        let mut template = self.checker.mapped_template(ty)?;
        let parameter = self.checker.mapped_parameter(ty)?;
        let keyof = self.checker.mapped_keyof_constraint(ty)?;
        let generate_names = self.flags & nf::GENERATE_NAMES_FOR_SHADOWED_TYPE_PARAMS != 0;
        // Retain the native short-circuit order: computing a constraint may
        // instantiate types and populate links used by the subsequent display.
        let needs_wrapper = !keyof
            && {
                let modifiers = self.checker.mapped_modifiers_type(ty)?;
                self.checker.types.flags(modifiers)? & tf::UNKNOWN == 0
            }
            && generate_names
            && {
                let constraint = self.checker.mapped_constraint(ty)?;
                !(self.checker.types.flags(constraint)? & tf::TYPE_PARAMETER != 0
                    && self
                        .checker
                        .constraint_of_type_parameter(constraint)?
                        .map(|ty| self.checker.types.flags(ty))
                        .transpose()?
                        .is_some_and(|flags| flags & tf::INDEX != 0))
            };
        let mut variable = None;
        let constraint = if keyof {
            if generate_names && self.non_homomorphic_instantiation(ty)? {
                let (new_parameter, new_variable) = self.mapped_wrapper_variable()?;
                variable = Some(new_variable);
                let target = self
                    .checker
                    .types
                    .object(ty)?
                    .target
                    .ok_or(Error::MissingLink("mapped instantiation target"))?;
                let target_template = self.checker.mapped_template(target)?;
                let target_parameter = self.checker.mapped_parameter(target)?;
                let target_modifiers = self.checker.mapped_modifiers_type(target)?;
                let mapper = self.checker.new_type_mapper(
                    &[target_parameter, target_modifiers],
                    &[parameter, new_parameter],
                )?;
                template = self
                    .checker
                    .instantiate_type(target_template, Some(mapper))?;
            }
            let operand = if let Some(node) = variable {
                node
            } else {
                let modifiers = self.checker.mapped_modifiers_type(ty)?;
                self.type_node(modifiers)?
            };
            self.ast
                .new_type_operator_node(K::KeyOfKeyword.into(), Some(operand))
        } else if needs_wrapper {
            let (_, new_variable) = self.mapped_wrapper_variable()?;
            variable = Some(new_variable);
            new_variable
        } else {
            let constraint = self.checker.mapped_constraint(ty)?;
            self.type_node(constraint)?
        };
        let (parameter_node, name, template_node) = self.with_serialization_scope(
            Some(declaration),
            &[],
            &[parameter],
            &[],
            None,
            |builder| {
                let parameter_node =
                    builder.type_parameter_node_with_constraint(parameter, Some(constraint))?;
                let name = builder
                    .checker
                    .mapped_name(ty)?
                    .map(|ty| builder.type_node(ty))
                    .transpose()?;
                let optional =
                    builder.checker.mapped_modifiers(ty)? & crate::mapped::INCLUDE_OPTIONAL != 0;
                let template = builder.without_missing(template, optional)?;
                Ok((parameter_node, name, builder.type_node(template)?))
            },
        )?;
        let result = self.ast.new_mapped_type_node(
            readonly,
            Some(parameter_node),
            name,
            question,
            Some(template_node),
            None,
        );
        self.approximate_length += 10;
        self.emit.add_emit_flags(result, emit_flags::SINGLE_LINE);
        if generate_names && self.non_homomorphic_instantiation(ty)? {
            let raw = self.checker.mapped_constraint_node(ty)?;
            let raw = match self.reuse_type_from_node(raw, false)? {
                Some(ty) => self.checker.constraint_of_type_parameter(ty)?,
                None => None,
            }
            .unwrap_or(self.checker.builtins.unknown_type);
            let mapper = self.checker.types.object(ty)?.mapper;
            let constraint = self.checker.instantiate_type(raw, mapper)?;
            let constraint = if self.checker.types.flags(constraint)? & tf::UNKNOWN == 0 {
                Some(self.type_node(constraint)?)
            } else {
                None
            };
            let modifiers = self.checker.mapped_modifiers_type(ty)?;
            let check = self.type_node(modifiers)?;
            let infer = self.mapped_wrapper_infer(
                variable.ok_or(Error::MissingLink("homomorphic wrapper variable"))?,
                constraint,
            )?;
            let never = self.ast.new_keyword_type_node(K::NeverKeyword.into());
            return Ok(self.ast.new_conditional_type_node(
                Some(check),
                Some(infer),
                Some(result),
                Some(never),
            ));
        }
        if needs_wrapper {
            let constraint = self.checker.mapped_constraint(ty)?;
            let check = self.type_node(constraint)?;
            let modifiers = self.checker.mapped_modifiers_type(ty)?;
            let modifiers = self.type_node(modifiers)?;
            let constraint = self
                .ast
                .new_type_operator_node(K::KeyOfKeyword.into(), Some(modifiers));
            let infer = self.mapped_wrapper_infer(
                variable.ok_or(Error::MissingLink("modifier wrapper variable"))?,
                Some(constraint),
            )?;
            let never = self.ast.new_keyword_type_node(K::NeverKeyword.into());
            return Ok(self.ast.new_conditional_type_node(
                Some(check),
                Some(infer),
                Some(result),
                Some(never),
            ));
        }
        Ok(result)
    }
}
