//! Binding-pattern inference. Implied pattern types retain their source pattern
//! only for contextual typing; direct destructuring reads keep the source order
//! and the native checked indexed-access flags.
use crate::{
    access_flags as af, element_flags as ef, object_flags as of, type_facts as facts,
    type_flags as tf, types::Map, CheckerState, Error, TypeId, UnionReduction,
};
use tsr_arena::NodeId;
use tsr_ast::{node_flags as nf, symbol_flags as sf, SymbolTable, SyntaxKind as K};

#[derive(Default)]
pub(crate) struct BindingState {
    pub pattern_for_type: Map<TypeId, NodeId>,
    pub contextual_patterns: Vec<NodeId>,
    #[allow(
        clippy::option_option,
        reason = "Distinguish an unqueried symbol from a cached absent symbol"
    )]
    pub omit_symbol: Option<Option<tsr_arena::SymbolId>>,
    pub spread_links: Map<tsr_arena::SymbolId, (tsr_arena::SymbolId, tsr_arena::SymbolId)>,
    pub discriminated_contexts: Map<(NodeId, TypeId), TypeId>,
}

impl CheckerState {
    // port: tsc/internal/checker/checker.go:Checker.getContextualTypeForBindingElement
    pub(crate) fn contextual_binding_element_type(
        &mut self,
        declaration: NodeId,
    ) -> Result<Option<TypeId>, Error> {
        let read = self.node(declaration)?;
        let name = read
            .property_name()
            .or(read.name())
            .ok_or(Error::MissingLink("binding context name"))?;
        if self.is_binding_pattern(name)? {
            return Ok(None);
        }
        if self.node(name)?.kind() == K::ComputedPropertyName {
            let expression = self
                .ast(name)?
                .node(name)?
                .expression()
                .ok_or(Error::MissingLink("computed binding name"))?;
            if !matches!(
                self.node(expression)?.kind().known(),
                Some(K::StringLiteral | K::NumericLiteral | K::NoSubstitutionTemplateLiteral)
            ) {
                return Ok(None);
            }
        }
        let pattern = read
            .parent()
            .ok_or(Error::MissingLink("binding context pattern"))?;
        let parent = self
            .ast(pattern)?
            .node(pattern)?
            .parent()
            .ok_or(Error::MissingLink("binding context parent"))?;
        let read = self.node(parent)?;
        let mut ty = if let Some(annotation) = read.type_node() {
            Some(self.get_type_from_type_node(annotation)?)
        } else if read.kind() == K::Parameter {
            self.contextually_typed_parameter_type(parent)?
        } else if read.kind() == K::BindingElement {
            self.contextual_binding_element_type(parent)?
        } else {
            None
        };
        if ty.is_none()
            && self.node(parent)?.kind() != K::BindingElement
            && self.node(parent)?.initializer().is_some()
        {
            let mode = if self.binding_is_rest(declaration)? {
                32
            } else {
                0
            };
            ty = Some(self.check_declaration_initializer(parent, mode, None)?);
        }
        let Some(ty) = ty else {
            return Ok(None);
        };
        if self.node(pattern)?.kind() == K::ArrayBindingPattern {
            let elements = self.source_list(pattern, self.node(pattern)?.element_list())?;
            let Some(index) = elements.iter().position(|&node| node == declaration) else {
                return Ok(None);
            };
            return self.map_type_ex(
                ty,
                &mut |checker, ty| checker.contextual_element_type(ty, index, None, None, None),
                true,
            );
        }
        let literal = self.literal_type_from_property_name(name)?;
        if let Some(name) = self.index_property_name(literal)? {
            return self.property_type(ty, name.as_bytes());
        }
        Ok(None)
    }

    // port: tsc/internal/checker/checker.go:Checker.padObjectLiteralType
    pub(crate) fn pad_binding_object_literal_type(
        &mut self,
        ty: TypeId,
        pattern: NodeId,
    ) -> Result<TypeId, Error> {
        let mut missing = Vec::new();
        for element in self.source_list(pattern, self.node(pattern)?.element_list())? {
            let read = self.node(element)?;
            if read.initializer().is_some() {
                let name = read
                    .property_name()
                    .or(read.name())
                    .ok_or(Error::MissingLink("padded property name"))?;
                let literal = self.literal_type_from_property_name(name)?;
                if let Some(name) = self.index_property_name(literal)? {
                    if self
                        .constituent_property(ty, name.as_bytes(), false)?
                        .is_none()
                    {
                        missing.push(element);
                    }
                }
            }
        }
        if missing.is_empty() {
            return Ok(ty);
        }
        let mut members = SymbolTable::default();
        for property in self.get_properties_of_type(ty)? {
            members.insert(self.symbol(property)?.name_to_owned(), Some(property));
        }
        for element in missing {
            let read = self.node(element)?;
            let name = read
                .property_name()
                .or(read.name())
                .ok_or(Error::MissingLink("padded property name"))?;
            let literal = self.literal_type_from_property_name(name)?;
            let name = self
                .index_property_name(literal)?
                .ok_or(Error::MissingLink("padded property text"))?;
            let symbol = self.new_symbol(sf::PROPERTY | sf::OPTIONAL, name.clone())?;
            let ty = self.type_from_binding_element(element, false, false)?;
            self.value_symbol_links.get_or_default(symbol).resolved_type = Some(ty);
            members.insert(name, Some(symbol));
        }
        let record = *self.types.get(ty)?;
        let indices = self.index_infos_of_type(ty)?;
        let members = self.alloc_symbol_table(members);
        let result = self.new_anonymous_type(record.symbol, Some(members), &[], &[], &indices)?;
        self.types.get_mut(result)?.object_flags = record.object_flags;
        Ok(result)
    }

    // port: tsc/internal/checker/checker.go:Checker.padTupleType
    pub(crate) fn pad_binding_tuple_type(
        &mut self,
        ty: TypeId,
        pattern: NodeId,
    ) -> Result<TypeId, Error> {
        let elements = self.source_list(pattern, self.node(pattern)?.element_list())?;
        let tuple = self.types.tuple(self.types.target(ty)?)?;
        let readonly = tuple.readonly;
        let mut infos = tuple.element_infos.to_vec();
        if tuple.combined_flags & ef::VARIABLE != 0 {
            return Ok(ty);
        }
        let mut types = self.element_types(ty)?.to_vec();
        if types.len() >= elements.len() {
            return Ok(ty);
        }
        for (index, &element) in elements.iter().enumerate().skip(types.len()) {
            if index + 1 == elements.len() && self.binding_is_rest(element)? {
                continue;
            }
            let read = self.node(element)?;
            let omitted = read.kind() == K::OmittedExpression;
            let default = read.initializer().is_some();
            types.push(if !omitted && default {
                self.type_from_binding_element(element, false, false)?
            } else {
                self.builtins.any_type
            });
            infos.push(crate::TupleElementInfo {
                flags: ef::OPTIONAL,
                labeled_declaration: None,
            });
            if !omitted && !default {
                self.report_implicit_any(element, self.builtins.any_type)?;
            }
        }
        self.create_tuple_type_ex(&types, &infos, readonly)
    }

    pub(crate) fn is_binding_pattern(&self, node: NodeId) -> Result<bool, Error> {
        Ok(matches!(
            self.node(node)?.kind().known(),
            Some(K::ObjectBindingPattern | K::ArrayBindingPattern)
        ))
    }

    pub(crate) fn binding_is_rest(&self, node: NodeId) -> Result<bool, Error> {
        let read = self.node(node)?;
        Ok(
            if let Some(data) = read.data_source().as_binding_element() {
                data.dot_dot_dot_token().is_some()
            } else if let Some(data) = read.data_source().as_parameter_declaration() {
                data.dot_dot_dot_token().is_some()
            } else {
                false
            },
        )
    }

    // port: tsc/internal/checker/checker.go:Checker.getLiteralTypeFromPropertyName
    pub(crate) fn literal_type_from_property_name(
        &mut self,
        node: NodeId,
    ) -> Result<TypeId, Error> {
        match self.node(node)?.kind().known() {
            Some(K::PrivateIdentifier) => Ok(self.builtins.never_type),
            Some(K::NumericLiteral) => {
                let ty = self.check_expression(node)?;
                self.get_regular_type_of_literal_type(ty)
            }
            Some(K::ComputedPropertyName) => {
                let ty = self.check_computed_property_name(node)?;
                self.get_regular_type_of_literal_type(ty)
            }
            Some(K::Identifier | K::StringLiteral | K::NoSubstitutionTemplateLiteral) => {
                let text = self.node_text(node)?.into_js_string();
                self.get_string_literal_type(text)
            }
            _ => {
                if tsr_ast::utilities::is_expression_kind(self.node(node)?.kind()) {
                    let ty = self.check_expression(node)?;
                    self.get_regular_type_of_literal_type(ty)
                } else {
                    Ok(self.builtins.never_type)
                }
            }
        }
    }

    // port: tsc/internal/checker/checker.go:Checker.getTypeForBindingElement
    pub(crate) fn type_for_binding_element(
        &mut self,
        declaration: NodeId,
    ) -> Result<Option<TypeId>, Error> {
        let pattern = self
            .ast(declaration)?
            .node(declaration)?
            .parent()
            .ok_or(Error::MissingLink("binding pattern"))?;
        let parent = self
            .ast(pattern)?
            .node(pattern)?
            .parent()
            .ok_or(Error::MissingLink("binding declaration"))?;
        let mode = if self.binding_is_rest(declaration)? {
            32
        } else {
            0
        };
        let Some(parent_type) = self.type_for_binding_element_parent(parent, mode)? else {
            return Ok(None);
        };
        self.binding_element_type_from_parent(declaration, parent_type, false)
            .map(Some)
    }

    // port: tsc/internal/checker/checker.go:Checker.getTypeForBindingElementParent
    pub(crate) fn type_for_binding_element_parent(
        &mut self,
        declaration: NodeId,
        mode: u32,
    ) -> Result<Option<TypeId>, Error> {
        if mode == 0 {
            if let Some(symbol) = self.get_symbol_of_declaration(declaration)? {
                if let Some(ty) = self
                    .value_symbol_links
                    .try_get(symbol)
                    .and_then(|links| links.resolved_type)
                {
                    if !(self.options.strict_null_checks
                        && self
                            .ast(declaration)?
                            .node(declaration)?
                            .question_token(self.ast(declaration)?)?
                            .is_some())
                    {
                        return Ok(Some(ty));
                    }
                }
            }
        }
        self.type_for_variable_like_raw(declaration, false, mode)
    }

    // port: tsc/internal/checker/checker.go:Checker.getBindingElementTypeFromParentType
    pub(crate) fn binding_element_type_from_parent(
        &mut self,
        declaration: NodeId,
        mut parent_type: TypeId,
        no_tuple_bounds_check: bool,
    ) -> Result<TypeId, Error> {
        if self.types.flags(parent_type)? & tf::ANY != 0 {
            return Ok(parent_type);
        }
        let read = self.node(declaration)?;
        let pattern = read.parent().ok_or(Error::MissingLink("binding pattern"))?;
        let parent = self
            .ast(pattern)?
            .node(pattern)?
            .parent()
            .ok_or(Error::MissingLink("binding declaration"))?;
        let ambient = read.flags() & nf::AMBIENT != 0;
        let initializer = read.initializer();
        let rest = self.binding_is_rest(declaration)?;
        let parent_initializer = self.node(parent)?.initializer();
        let root = self.root_binding_declaration(declaration)?;
        let parameter = self.node(root)?.kind() == K::Parameter;
        if self.options.strict_null_checks && ambient && parameter {
            parent_type = self.non_nullable_type(parent_type)?;
        } else if self.options.strict_null_checks {
            if let Some(initializer) = parent_initializer {
                let initialized = self.type_of_initializer(initializer)?;
                if self.type_facts(initialized, facts::EQ_UNDEFINED)? == 0 {
                    parent_type = self.type_with_facts(parent_type, facts::NE_UNDEFINED)?;
                }
            }
        }
        let access = af::EXPRESSION_POSITION
            | if no_tuple_bounds_check || initializer.is_some() {
                af::ALLOW_MISSING
            } else {
                0
            };
        let mut ty = match self.node(pattern)?.kind().known() {
            Some(K::ObjectBindingPattern) => {
                if rest {
                    parent_type = self.get_reduced_type(parent_type)?;
                    if self.types.flags(parent_type)? & tf::UNKNOWN != 0
                        || !self.valid_spread_type(parent_type)?
                    {
                        self.error_at(
                            Some(declaration),
                            tsr_diagnostics::Rest_types_may_only_be_created_from_object_types,
                            vec![],
                        )?;
                        return Ok(self.builtins.error_type);
                    }
                    let mut properties = Vec::new();
                    for element in self.source_list(pattern, self.node(pattern)?.element_list())? {
                        if !self.binding_is_rest(element)? {
                            let read = self.node(element)?;
                            properties.push(
                                read.property_name()
                                    .or(read.name())
                                    .ok_or(Error::MissingLink("rest excluded property"))?,
                            );
                        }
                    }
                    let symbol = self.get_symbol_of_declaration(declaration)?;
                    self.object_rest_type(parent_type, &properties, symbol)?
                } else {
                    let name = self
                        .ast(declaration)?
                        .node(declaration)?
                        .property_name()
                        .or(self.node(declaration)?.name())
                        .ok_or(Error::MissingLink("binding property name"))?;
                    let index = self.literal_type_from_property_name(name)?;
                    let declared =
                        self.get_indexed_access_type(parent_type, index, access, Some(name), None)?;
                    self.flow_type_of_destructuring(declaration, declared)?
                }
            }
            Some(K::ArrayBindingPattern) => {
                let use_ = crate::iteration::ALLOW_SYNC
                    | crate::iteration::DESTRUCTURING_FLAG
                    | if rest {
                        0
                    } else {
                        crate::iteration::POSSIBLY_OUT_OF_BOUNDS
                    };
                let element = self.check_iterated_type_or_element_type(
                    use_,
                    parent_type,
                    self.builtins.undefined_type,
                    Some(pattern),
                )?;
                let elements = self.source_list(pattern, self.node(pattern)?.element_list())?;
                let index = elements
                    .iter()
                    .position(|&node| node == declaration)
                    .ok_or(Error::MissingLink("array binding index"))?;
                if rest {
                    let base = self
                        .map_type(parent_type, &mut |checker, ty| {
                            if checker.types.flags(ty)? & tf::INSTANTIABLE_NON_PRIMITIVE != 0 {
                                Ok(Some(checker.base_constraint_of_type(ty)?.unwrap_or(ty)))
                            } else {
                                Ok(Some(ty))
                            }
                        })?
                        .ok_or(Error::MissingLink("binding base constraint"))?;
                    let parts = if self.types.flags(base)? & tf::UNION != 0 {
                        self.types.compound_types(base)?.to_vec()
                    } else {
                        vec![base]
                    };
                    let mut tuple = true;
                    for &part in &parts {
                        tuple &= self.is_tuple_type(part)?;
                    }
                    if tuple {
                        self.map_type(base, &mut |checker, ty| {
                            checker.slice_tuple(ty, index, 0).map(Some)
                        })?
                        .ok_or(Error::MissingLink("binding tuple slice"))?
                    } else {
                        self.create_array_type(element, false)?
                    }
                } else if self.is_array_like_type(parent_type)? {
                    let index =
                        self.get_number_literal_type(tsr_jsnum::Number::new(index as f64))?;
                    let name = self.node(declaration)?.name();
                    let declared = self
                        .indexed_access_or_undefined(parent_type, index, access, name, None)?
                        .unwrap_or(self.builtins.error_type);
                    self.flow_type_of_destructuring(declaration, declared)?
                } else {
                    element
                }
            }
            _ => return Err(Error::MissingLink("binding pattern kind")),
        };
        if initializer.is_none() {
            return Ok(ty);
        }
        if self.node(root)?.type_node().is_some() {
            if self.options.strict_null_checks {
                let initial = self.check_declaration_initializer(declaration, 0, None)?;
                if self.type_facts(initial, facts::IS_UNDEFINED)? == 0 {
                    ty = self.non_undefined_binding_type(ty)?;
                }
            }
            return Ok(ty);
        }
        let defined = self.non_undefined_binding_type(ty)?;
        let initial = self.check_declaration_initializer(declaration, 0, None)?;
        let combined =
            self.get_union_type_ex(&[defined, initial], UnionReduction::Subtype, None, None)?;
        self.widen_type_inferred_from_initializer(declaration, combined)
    }

    // port: tsc/internal/ast/utilities.go:WalkUpBindingElementsAndPatterns
    pub(crate) fn root_binding_declaration(
        &self,
        mut declaration: NodeId,
    ) -> Result<NodeId, Error> {
        while self.node(declaration)?.kind() == K::BindingElement
            || self.is_binding_pattern(declaration)?
        {
            declaration = self
                .ast(declaration)?
                .node(declaration)?
                .parent()
                .ok_or(Error::MissingLink("binding root"))?;
        }
        Ok(declaration)
    }

    // port: tsc/internal/checker/checker.go:Checker.getNonUndefinedType
    pub(crate) fn non_undefined_binding_type(&mut self, ty: TypeId) -> Result<TypeId, Error> {
        let parts = if self.types.flags(ty)? & tf::UNION != 0 {
            self.types.compound_types(ty)?.to_vec()
        } else {
            vec![ty]
        };
        let mut generic = false;
        for &part in &parts {
            if self.types.flags(part)? & tf::INSTANTIABLE != 0 {
                if let Some(constraint) = self.base_constraint_of_type(part)? {
                    if self.maybe_type_of_kind(constraint, tf::UNDEFINED)? {
                        generic = true;
                        break;
                    }
                }
            }
        }
        let ty = if generic {
            self.map_type(ty, &mut |checker, ty| {
                Ok(Some(if checker.types.flags(ty)? & tf::INSTANTIABLE != 0 {
                    checker.base_constraint_of_type(ty)?.unwrap_or(ty)
                } else {
                    ty
                }))
            })?
            .ok_or(Error::MissingLink("binding undefined constraint"))?
        } else {
            ty
        };
        self.type_with_facts(ty, facts::NE_UNDEFINED)
    }

    /// The cached type of an initializer when the variable's type was inferred
    /// from it; otherwise computed now without caching, so transient types are
    /// reflected. Re-checking the initializer here once per binding element
    /// cloned the pattern's contextual type each time.
    // port: tsc/internal/checker/flow.go:Checker.getTypeOfInitializer
    pub(crate) fn type_of_initializer(&mut self, node: NodeId) -> Result<TypeId, Error> {
        if let Some(Some(ty)) = self.query.type_nodes.try_get(node) {
            return Ok(*ty);
        }
        self.get_type_of_expression(node)
    }

    // port: tsc/internal/checker/checker.go:Checker.getTypeFromBindingPattern
    pub(crate) fn type_from_binding_pattern(
        &mut self,
        pattern: NodeId,
        include_pattern: bool,
        report: bool,
    ) -> Result<TypeId, Error> {
        if include_pattern {
            self.bindings.contextual_patterns.push(pattern);
        }
        let result = if self.node(pattern)?.kind() == K::ObjectBindingPattern {
            self.type_from_object_binding_pattern(pattern, include_pattern, report)
        } else {
            self.type_from_array_binding_pattern(pattern, include_pattern, report)
        };
        if include_pattern {
            self.bindings.contextual_patterns.pop();
        }
        result
    }

    // port: tsc/internal/checker/checker.go:Checker.getTypeFromObjectBindingPattern
    fn type_from_object_binding_pattern(
        &mut self,
        pattern: NodeId,
        include_pattern: bool,
        report: bool,
    ) -> Result<TypeId, Error> {
        let mut members = SymbolTable::default();
        let mut string_index = None;
        let mut flags = of::OBJECT_LITERAL | of::CONTAINS_OBJECT_OR_ARRAY_LITERAL;
        for element in self.source_list(pattern, self.node(pattern)?.element_list())? {
            if self.binding_is_rest(element)? {
                string_index = Some(self.signatures.new_index_info(
                    self.builtins.string_type,
                    self.builtins.any_type,
                    false,
                    None,
                    None,
                )?);
                continue;
            }
            let read = self.node(element)?;
            let optional = read.initializer().is_some();
            let name = read
                .property_name()
                .or(read.name())
                .ok_or(Error::MissingLink("implied binding property"))?;
            let literal = self.literal_type_from_property_name(name)?;
            let Some(name) = self.index_property_name(literal)? else {
                flags |= of::OBJECT_LITERAL_PATTERN_WITH_COMPUTED_PROPERTIES;
                continue;
            };
            let symbol = self.new_symbol(
                sf::PROPERTY | if optional { sf::OPTIONAL } else { 0 },
                name.clone(),
            )?;
            let ty = self.type_from_binding_element(element, include_pattern, report)?;
            self.value_symbol_links.get_or_default(symbol).resolved_type = Some(ty);
            members.insert(name, Some(symbol));
        }
        let members = self.alloc_symbol_table(members);
        let indices = string_index.into_iter().collect::<Vec<_>>();
        let ty = self.new_anonymous_type(None, Some(members), &[], &[], &indices)?;
        self.types.get_mut(ty)?.object_flags |= flags;
        if include_pattern {
            self.bindings.pattern_for_type.insert(ty, pattern);
        }
        Ok(ty)
    }

    // port: tsc/internal/checker/checker.go:Checker.getTypeFromArrayBindingPattern
    fn type_from_array_binding_pattern(
        &mut self,
        pattern: NodeId,
        include_pattern: bool,
        report: bool,
    ) -> Result<TypeId, Error> {
        let elements = self.source_list(pattern, self.node(pattern)?.element_list())?;
        let rest = match elements.last().copied() {
            Some(last) if self.binding_is_rest(last)? => Some(last),
            _ => None,
        };
        if elements.is_empty() || elements.len() == 1 && rest.is_some() {
            if self.program()?.host.options().emit_script_target() >= tsr_core::ScriptTarget::ES2015
            {
                return self.create_iterable_type(self.builtins.any_type);
            }
            return Ok(*self
                .query
                .global_types
                .get("anyArrayType")
                .ok_or(Error::MissingLink("any array type"))?);
        }
        let mut minimum = 0;
        for (i, &element) in elements.iter().enumerate() {
            let read = self.node(element)?;
            if Some(element) != rest && read.name().is_some() && read.initializer().is_none() {
                minimum = i + 1;
            }
        }
        let mut types = Vec::with_capacity(elements.len());
        let mut infos = Vec::with_capacity(elements.len());
        for (i, &element) in elements.iter().enumerate() {
            types.push(if self.node(element)?.name().is_none() {
                self.builtins.any_type
            } else {
                self.type_from_binding_element(element, include_pattern, report)?
            });
            infos.push(crate::TupleElementInfo {
                flags: if Some(element) == rest {
                    ef::REST
                } else if i >= minimum {
                    ef::OPTIONAL
                } else {
                    ef::REQUIRED
                },
                labeled_declaration: None,
            });
        }
        let mut ty = self.create_tuple_type_ex(&types, &infos, false)?;
        if include_pattern {
            ty = self.clone_binding_type_reference(ty)?;
            self.bindings.pattern_for_type.insert(ty, pattern);
            self.types.get_mut(ty)?.object_flags |= of::CONTAINS_OBJECT_OR_ARRAY_LITERAL;
        }
        Ok(ty)
    }

    // port: tsc/internal/checker/checker.go:Checker.cloneTypeReference
    pub(crate) fn clone_binding_type_reference(&mut self, source: TypeId) -> Result<TypeId, Error> {
        let record = *self.types.get(source)?;
        let data = self.types.type_reference(source)?;
        let target = data.object.target;
        let arguments = data.resolved_type_arguments.clone();
        let ty = self.new_object_type(of::REFERENCE, record.symbol)?;
        self.types.get_mut(ty)?.object_flags = record.object_flags & !of::MEMBERS_RESOLVED;
        let data = self.types.type_reference_mut(ty)?;
        data.object.target = target;
        data.resolved_type_arguments = arguments;
        Ok(ty)
    }

    // port: tsc/internal/checker/checker.go:Checker.getTypeFromBindingElement
    pub(crate) fn type_from_binding_element(
        &mut self,
        element: NodeId,
        include_pattern: bool,
        report: bool,
    ) -> Result<TypeId, Error> {
        let read = self.node(element)?;
        let initializer = read.initializer();
        let name = read
            .name()
            .ok_or(Error::MissingLink("implied binding name"))?;
        let binding = self.is_binding_pattern(name)?;
        if initializer.is_some() {
            let context = if binding {
                self.type_from_binding_pattern(name, true, false)?
            } else {
                self.builtins.unknown_type
            };
            let ty = self.check_declaration_initializer(element, 0, Some(context))?;
            let ty = self.widen_type_inferred_from_initializer(element, ty)?;
            return self.add_type_optionality(ty, false, true);
        }
        if binding {
            return self.type_from_binding_pattern(name, include_pattern, report);
        }
        if report && !self.binding_private_ambient(element)? {
            self.report_implicit_any(element, self.builtins.any_type)?;
        }
        Ok(if include_pattern {
            self.builtins.non_inferrable_any_type
        } else {
            self.builtins.any_type
        })
    }

    // port: tsc/internal/checker/checker.go:Checker.declarationBelongsToPrivateAmbientMember
    pub(crate) fn binding_private_ambient(&self, declaration: NodeId) -> Result<bool, Error> {
        let mut root = self.root_binding_declaration(declaration)?;
        if self.node(root)?.kind() == K::Parameter {
            root = self
                .ast(root)?
                .node(root)?
                .parent()
                .ok_or(Error::MissingLink("private ambient function"))?;
        }
        let read = self.node(root)?;
        Ok(read.flags() & nf::AMBIENT != 0
            && (read.modifier_flags(self.ast(root)?)? & tsr_ast::modifier_flags::PRIVATE != 0
                || tsr_ast::utilities::is_private_identifier_class_element_declaration(
                    self.ast(root)?,
                    root,
                )?))
    }

    // port: tsc/internal/checker/checker.go:Checker.assignBindingElementTypes
    pub(crate) fn assign_binding_element_types(
        &mut self,
        pattern: NodeId,
        parent_type: TypeId,
    ) -> Result<(), Error> {
        for element in self.source_list(pattern, self.node(pattern)?.element_list())? {
            if let Some(name) = self.node(element)?.name() {
                let ty = self.binding_element_type_from_parent(element, parent_type, false)?;
                if self.node(name)?.kind() == K::Identifier {
                    let symbol = self
                        .get_symbol_of_declaration(element)?
                        .ok_or(Error::MissingLink("assigned binding symbol"))?;
                    self.value_symbol_links.get_or_default(symbol).resolved_type = Some(ty);
                } else {
                    self.assign_binding_element_types(name, ty)?;
                }
            }
        }
        Ok(())
    }
}
