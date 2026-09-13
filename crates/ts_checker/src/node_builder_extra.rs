//! Array, tuple and signature syntax emitted by the type node builder.

use super::{
    check_flags, emit_flags, nf, sf, Error, Factory, FactoryMethods, JsString, NodeBuilder, NodeId,
    SymbolId, TypeId, K,
};
use crate::{element_flags as ef, signature_flags as sg, IndexInfoId, SignatureId};

impl NodeBuilder<'_> {
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
        let mut nodes = Vec::new();
        for (&argument, info) in arguments.iter().zip(infos.iter()) {
            let argument = self.without_missing(argument, info.flags & ef::OPTIONAL != 0)?;
            let mut node = self.type_node(argument)?;
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
        let mut ty = self.checker.get_type_of_symbol(symbol)?;
        if let Some(node) = declaration {
            if self
                .checker
                .parameter_requires_implicit_undefined(node, None)?
            {
                ty = self
                    .checker
                    .get_union_type(&[ty, self.checker.builtins.undefined_type])?;
            }
        }
        let annotation = self.type_node(ty)?;
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
        let mut parameters = Vec::new();
        if self.flags & nf::OMIT_THIS_PARAMETER == 0 {
            if let Some(this) = sig.this_parameter {
                parameters.push(self.parameter_node(this)?);
            }
        }
        for &parameter in expanded {
            parameters.push(self.parameter_node(parameter)?);
        }
        let parameters = self.list(parameters)?;
        let mut return_type = self.checker.return_type_of_signature(signature)?;
        if let Some(declaration) = sig.declaration {
            if self.checker.ast(declaration)?.node(declaration)?.flags()
                & ts_ast::node_flags::SYNTHESIZED
                == 0
            {
                return_type = self.checker.instantiate_type(return_type, self.mapper)?;
            }
        }
        let return_type = match self.checker.type_predicate_of_signature(signature)? {
            Some(predicate) => self.predicate_node(predicate)?,
            None => self.type_node(return_type)?,
        };
        let modifiers = if kind == K::ConstructorType && sig.flags & sg::ABSTRACT != 0 {
            let abstract_modifier = self.ast.new_modifier(K::AbstractKeyword.into());
            Some(self.list(vec![abstract_modifier])?)
        } else {
            None
        };
        Ok(match kind {
            K::FunctionType => self.ast.new_function_type_node(
                type_parameters,
                Some(parameters),
                Some(return_type),
            ),
            K::ConstructorType => self.ast.new_constructor_type_node(
                modifiers,
                type_parameters,
                Some(parameters),
                Some(return_type),
            ),
            K::CallSignature => self.ast.new_call_signature_declaration(
                type_parameters,
                Some(parameters),
                Some(return_type),
            ),
            K::ConstructSignature => self.ast.new_construct_signature_declaration(
                type_parameters,
                Some(parameters),
                Some(return_type),
            ),
            K::GetAccessor => self.ast.new_get_accessor_declaration(
                None,
                name,
                None,
                Some(parameters),
                Some(return_type),
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
                Some(return_type),
            ),
            _ => {
                return Err(Error::Unsupported(
                    "signatureToSignatureDeclarationHelper: syntax kind",
                ))
            }
        })
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
    pub(super) fn property_elements(&mut self, symbol: SymbolId) -> Result<Vec<NodeId>, Error> {
        let flags = self.checker.symbol(symbol)?.flags();
        let placeholder = self.reverse_property_placeholder(symbol)?;
        let ty = if placeholder {
            self.checker.builtins.any_type
        } else {
            self.checker.get_type_of_symbol(symbol)?
        };
        let ty = self.without_missing(ty, flags & sf::OPTIONAL != 0)?;
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
    // port: tsc/internal/checker/nodebuilderimpl.go:NodeBuilderImpl.createMappedTypeNodeFromType
    pub(super) fn mapped_type_node(&mut self, ty: TypeId) -> Result<NodeId, Error> {
        let declaration = self.checker.mapped_declaration(ty)?;
        let read = self.checker.ast(declaration)?.node(declaration)?;
        let data = read
            .data_source()
            .as_mapped_type_node()
            .ok_or(Error::MissingLink("mapped display syntax"))?;
        let readonly = data
            .readonly_token()
            .map(|node| {
                self.checker
                    .ast(node)?
                    .node(node)
                    .map(|read| read.kind())
                    .map_err(Error::from)
            })
            .transpose()?;
        let question = data
            .question_token()
            .map(|node| {
                self.checker
                    .ast(node)?
                    .node(node)
                    .map(|read| read.kind())
                    .map_err(Error::from)
            })
            .transpose()?;
        if self.flags & ts_nodebuilder::flags::GENERATE_NAMES_FOR_SHADOWED_TYPE_PARAMS != 0 {
            return Err(Error::Unsupported(
                "mapped display: modifier preserving wrapper",
            ));
        }
        let template = self.checker.mapped_template(ty)?;
        let parameter = self.checker.mapped_parameter(ty)?;
        let constraint = if self.checker.mapped_keyof_constraint(ty)? {
            let modifiers = self.checker.mapped_modifiers_type(ty)?;
            let operand = self.type_node(modifiers)?;
            self.ast
                .new_type_operator_node(K::KeyOfKeyword.into(), Some(operand))
        } else {
            let constraint = self.checker.mapped_constraint(ty)?;
            self.type_node(constraint)?
        };
        let name = self.symbol_node(
            self.checker
                .types
                .get(parameter)?
                .symbol
                .ok_or(Error::MissingLink("mapped parameter display"))?,
        )?;
        let parameter =
            self.ast
                .new_type_parameter_declaration(None, Some(name), Some(constraint), None, None);
        let name_type = self
            .checker
            .mapped_name(ty)?
            .map(|ty| self.type_node(ty))
            .transpose()?;
        let optional = self.checker.mapped_modifiers(ty)? & crate::mapped::INCLUDE_OPTIONAL != 0;
        let template = self.without_missing(template, optional)?;
        let template = self.type_node(template)?;
        let readonly = readonly.map(|kind| self.ast.new_token(kind));
        let question = question.map(|kind| self.ast.new_token(kind));
        let result = self.ast.new_mapped_type_node(
            readonly,
            Some(parameter),
            name_type,
            question,
            Some(template),
            None,
        );
        self.approximate_length += 10;
        self.emit.add_emit_flags(result, emit_flags::SINGLE_LINE);
        Ok(result)
    }
}
