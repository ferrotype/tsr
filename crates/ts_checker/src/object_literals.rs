//! Object literals are checked in source order. Spread chunks allocate their
//! native intermediate objects, and deferred members keep the source schedule.
use crate::{object_flags as of, type_flags as tf, CheckerState, Error, TypeId};
use ts_arena::{NodeId, SymbolId};
use ts_ast::{
    check_flags as cf, node_flags as nf, symbol_flags as sf, SymbolTable, SyntaxKind as K,
};
use ts_diagnostics as d;

#[derive(Default)]
struct ObjectChunk {
    members: SymbolTable,
    properties: Vec<SymbolId>,
    offset: usize,
    computed: [bool; 3],
    pattern_computed: bool,
    flags: u32,
}
impl CheckerState {
    // port: tsc/internal/checker/utilities.go:Checker.isJSLiteralType
    pub(crate) fn is_js_literal_type(&mut self, ty: TypeId) -> Result<bool, Error> {
        if self.program.as_ref().is_some_and(|program| {
            let options = program.host.options();
            options.strict_option_value(options.no_implicit_any)
        }) {
            return Ok(false);
        }
        let record = self.types.get(ty)?;
        if record.object_flags & of::JS_LITERAL != 0 {
            return Ok(true);
        }
        let flags = record.flags;
        if flags & tf::UNION != 0 {
            for index in 0..self.types.compound_types(ty)?.len() {
                let part = self.types.compound_types(ty)?[index];
                if !self.is_js_literal_type(part)? {
                    return Ok(false);
                }
            }
            return Ok(true);
        }
        if flags & tf::INTERSECTION != 0 {
            for index in 0..self.types.compound_types(ty)?.len() {
                let part = self.types.compound_types(ty)?[index];
                if self.is_js_literal_type(part)? {
                    return Ok(true);
                }
            }
            return Ok(false);
        }
        if flags & tf::INSTANTIABLE != 0 {
            let constraint = self.resolved_base_constraint(ty, &mut Vec::new())?;
            return Ok(constraint != ty && self.is_js_literal_type(constraint)?);
        }
        Ok(false)
    }

    // port: tsc/internal/checker/checker.go:Checker.checkObjectLiteral
    pub(crate) fn check_object_literal(&mut self, node: NodeId) -> Result<TypeId, Error> {
        let properties = self.source_list(node, self.ast(node)?.node(node)?.property_list())?;
        let symbol = self.get_symbol_of_declaration(node)?;
        if properties.is_empty() {
            if let Some(symbol) = symbol {
                if let Some(exports) = self.symbol(symbol)?.exports() {
                    if !self.table(exports)?.is_empty() {
                        let result =
                            self.new_anonymous_type(Some(symbol), Some(exports), &[], &[], &[])?;
                        let flags = self.ast(node)?.node(node)?.flags();
                        if flags & nf::JAVA_SCRIPT_FILE != 0 && flags & nf::JSON_FILE == 0 {
                            self.types.get_mut(result)?.object_flags |= of::JS_LITERAL;
                        }
                        return Ok(result);
                    }
                }
            }
        }
        self.defer_checker_node(node)?;
        let destructuring = ts_ast::is_assignment_target(self.ast(node)?, node)?;
        self.check_object_literal_grammar(node, destructuring)?;
        let context = self.contextual_expression_type(node)?;
        let inference = self.call_inference_at_node(node)?;
        if let Some(ty) = context {
            self.calls.contexts.push(crate::calls::ArgumentContext {
                node,
                ty,
                inference,
            });
        }
        let result = self.check_object_literal_members(node, &properties, symbol, destructuring);
        if context.is_some() {
            self.calls.contexts.pop();
        }
        result
    }
    fn check_object_literal_members(
        &mut self,
        node: NodeId,
        properties: &[NodeId],
        symbol: Option<SymbolId>,
        destructuring: bool,
    ) -> Result<TypeId, Error> {
        let context = self.apparent_contextual_expression_type(node)?;
        let context_pattern = context
            .and_then(|ty| self.bindings.pattern_for_type.get(&ty).copied())
            .map(|pattern| {
                self.ast(pattern).and_then(|view| {
                    view.node(pattern)
                        .map(|read| {
                            matches!(
                                read.kind().known(),
                                Some(K::ObjectBindingPattern | K::ObjectLiteralExpression)
                            )
                        })
                        .map_err(Error::from)
                })
            })
            .transpose()?
            .unwrap_or(false);
        let readonly = self.is_const_context(node)?;
        let checks = if readonly { cf::READONLY } else { 0 };
        let mut chunk = ObjectChunk {
            flags: of::FRESH_LITERAL,
            ..Default::default()
        };
        let mut all = SymbolTable::default();
        let mut spread = self.builtins.empty_object_type;
        for &declaration in properties {
            if let Some(name) = self.ast(declaration)?.node(declaration)?.name() {
                if self.ast(name)?.node(name)?.kind() == K::ComputedPropertyName {
                    self.check_computed_property_name(name)?;
                }
            }
        }
        for &declaration in properties {
            let read = self.ast(declaration)?.node(declaration)?;
            let kind = read.kind();
            let name = read.name();
            let mut member = self.get_symbol_of_declaration(declaration)?;
            let computed = match name {
                Some(name) if self.ast(name)?.node(name)?.kind() == K::ComputedPropertyName => {
                    Some(self.check_computed_property_name(name)?)
                }
                _ => None,
            };
            if matches!(
                kind.known(),
                Some(K::PropertyAssignment | K::ShorthandPropertyAssignment | K::MethodDeclaration)
            ) {
                let ty = match kind.known() {
                    Some(K::PropertyAssignment) => {
                        self.check_object_property_assignment(declaration, self.expression_mode)?
                    }
                    Some(K::ShorthandPropertyAssignment) => self
                        .check_shorthand_property_assignment(
                            declaration,
                            destructuring,
                            self.expression_mode,
                        )?,
                    _ => self.check_object_literal_method(declaration)?,
                };
                chunk.flags |= self.types.get(ty)?.object_flags & of::PROPAGATING_FLAGS;
                let original = member.ok_or(Error::MissingLink("object property symbol"))?;
                let name_type = computed.filter(|&ty| {
                    self.types
                        .flags(ty)
                        .is_ok_and(|flags| flags & tf::STRING_OR_NUMBER_LITERAL_OR_UNIQUE != 0)
                });
                let text = match name_type {
                    Some(ty) => self
                        .index_property_name(ty)?
                        .ok_or(Error::MissingLink("computed property name"))?,
                    None => self.symbol(original)?.name_to_owned(),
                };
                let prop = self.new_symbol_ex(
                    sf::PROPERTY | self.symbol(original)?.flags(),
                    text.clone(),
                    checks | if name_type.is_some() { cf::LATE } else { 0 },
                )?;
                self.value_symbol_links.get_or_default(prop).name_type = name_type;
                if destructuring && self.object_member_has_default(declaration)? {
                    self.symbol_mut(prop)?.flags |= sf::OPTIONAL;
                } else if context_pattern {
                    let context = context.ok_or(Error::MissingLink("object pattern context"))?;
                    if self.types.get(context)?.object_flags
                        & of::OBJECT_LITERAL_PATTERN_WITH_COMPUTED_PROPERTIES
                        == 0
                    {
                        let original_name = self.symbol(original)?.name_to_owned();
                        if let Some(implied) =
                            self.constituent_property(context, original_name.as_bytes(), false)?
                        {
                            self.symbol_mut(prop)?.flags |=
                                self.symbol(implied)?.flags() & sf::OPTIONAL;
                        } else if self
                            .index_info_of_type(context, self.builtins.string_type)?
                            .is_none()
                        {
                            let display =
                                self.type_to_string(context, crate::type_display::DEFAULT_FLAGS)?;
                            let display_name = self.symbol_to_string(original)?;
                            self.error_at(name.or(Some(declaration)),d::Object_literal_may_only_specify_known_properties_and_0_does_not_exist_in_type_1,vec![display_name,display])?;
                        }
                    }
                }
                let original_read = self.symbol(original)?;
                let declarations = original_read.declarations();
                let parent = original_read.parent();
                let value = original_read.value_declaration();
                let stored = self.symbol_mut(prop)?;
                stored.declarations = declarations;
                stored.parent = parent;
                stored.value_declaration = value;
                let links = self.value_symbol_links.get_or_default(prop);
                links.resolved_type = Some(ty);
                links.target = Some(original);
                member = Some(prop);
                if self.options.strict_null_checks {
                    all.insert(text, Some(prop));
                }
                if context.is_some()
                    && self.expression_mode & 2 != 0
                    && self.expression_mode & 4 == 0
                    && matches!(
                        kind.known(),
                        Some(K::PropertyAssignment | K::MethodDeclaration)
                    )
                    && self.expression_is_context_sensitive(declaration)?
                {
                    let inference = self
                        .call_inference_at_node(node)?
                        .ok_or(Error::MissingLink("object inference context"))?;
                    let site = if kind == K::PropertyAssignment {
                        self.ast(declaration)?
                            .node(declaration)?
                            .initializer()
                            .ok_or(Error::MissingLink("property initializer"))?
                    } else {
                        declaration
                    };
                    self.add_intra_expression_inference_site(inference, site, ty)?;
                }
            } else if kind == K::SpreadAssignment {
                if !chunk.properties.is_empty() {
                    let object = self.create_object_literal_chunk(
                        node,
                        symbol,
                        context,
                        destructuring,
                        &mut chunk,
                    )?;
                    spread =
                        self.object_spread_type(spread, object, symbol, chunk.flags, readonly)?;
                    chunk.properties.clear();
                    chunk.members = SymbolTable::default();
                    chunk.computed = [false; 3];
                }
                let expression = self
                    .ast(declaration)?
                    .node(declaration)?
                    .expression()
                    .ok_or(Error::MissingLink("object spread expression"))?;
                let ty = self.check_expression_ex(expression, self.expression_mode & 2)?;
                let ty = self.get_reduced_type(ty)?;
                if self.valid_spread_type(ty)? {
                    let ty = self.merge_spread_union(ty, readonly)?;
                    if self.options.strict_null_checks {
                        self.check_spread_property_overrides(ty, &all, declaration)?;
                    }
                    chunk.offset = chunk.properties.len();
                    if !self.is_error_type(spread)? {
                        spread =
                            self.object_spread_type(spread, ty, symbol, chunk.flags, readonly)?;
                    }
                } else {
                    self.error_at(
                        Some(declaration),
                        d::Spread_types_may_only_be_created_from_object_types,
                        vec![],
                    )?;
                    spread = self.builtins.error_type;
                }
                continue;
            } else {
                self.defer_checker_node(declaration)?;
            }
            let member = member.ok_or(Error::MissingLink("object member symbol"))?;
            if let Some(computed) = computed.filter(|&ty| {
                self.types
                    .flags(ty)
                    .is_ok_and(|flags| flags & tf::STRING_OR_NUMBER_LITERAL_OR_UNIQUE == 0)
            }) {
                if self.is_type_related_to(
                    computed,
                    self.builtins.string_number_symbol_type,
                    crate::RelationKind::Assignable,
                )? {
                    let index = if self.is_type_related_to(
                        computed,
                        self.builtins.number_type,
                        crate::RelationKind::Assignable,
                    )? {
                        1
                    } else if self.is_type_related_to(
                        computed,
                        self.builtins.es_symbol_type,
                        crate::RelationKind::Assignable,
                    )? {
                        2
                    } else {
                        0
                    };
                    chunk.computed[index] = true;
                    if destructuring {
                        chunk.pattern_computed = true;
                    }
                }
            } else {
                chunk
                    .members
                    .insert(self.symbol(member)?.name_to_owned(), Some(member));
            }
            chunk.properties.push(member);
        }
        if self.is_error_type(spread)? {
            return Ok(self.builtins.error_type);
        }
        if spread != self.builtins.empty_object_type {
            if !chunk.properties.is_empty() {
                let object = self.create_object_literal_chunk(
                    node,
                    symbol,
                    context,
                    destructuring,
                    &mut chunk,
                )?;
                spread = self.object_spread_type(spread, object, symbol, chunk.flags, readonly)?;
                chunk.properties.clear();
                chunk.members = SymbolTable::default();
                chunk.computed[0] = false;
                chunk.computed[1] = false;
            }
            return self
                .map_type(spread, &mut |checker, ty| {
                    if ty == checker.builtins.empty_object_type {
                        checker
                            .create_object_literal_chunk(
                                node,
                                symbol,
                                context,
                                destructuring,
                                &mut chunk,
                            )
                            .map(Some)
                    } else {
                        Ok(Some(ty))
                    }
                })?
                .ok_or(Error::MissingLink("spread literal result"));
        }
        self.create_object_literal_chunk(node, symbol, context, destructuring, &mut chunk)
    }
    fn create_object_literal_chunk(
        &mut self,
        node: NodeId,
        symbol: Option<SymbolId>,
        context: Option<TypeId>,
        destructuring: bool,
        chunk: &mut ObjectChunk,
    ) -> Result<TypeId, Error> {
        let readonly = self.is_const_context(node)?;
        let mut indices = Vec::new();
        for (active, key) in chunk.computed.iter().zip([
            self.builtins.string_type,
            self.builtins.number_type,
            self.builtins.es_symbol_type,
        ]) {
            if *active {
                indices.push(self.object_literal_index_info(
                    readonly,
                    &chunk.properties[chunk.offset..],
                    key,
                )?);
            }
        }
        let members = self.alloc_symbol_table(std::mem::take(&mut chunk.members));
        let result = self.new_anonymous_type(symbol, Some(members), &[], &[], &indices)?;
        let flags = self.ast(node)?.node(node)?.flags();
        self.types.get_mut(result)?.object_flags |= chunk.flags
            | of::OBJECT_LITERAL
            | of::CONTAINS_OBJECT_OR_ARRAY_LITERAL
            | if context.is_none()
                && flags & nf::JAVA_SCRIPT_FILE != 0
                && flags & nf::JSON_FILE == 0
            {
                of::JS_LITERAL
            } else {
                0
            }
            | if chunk.pattern_computed {
                of::OBJECT_LITERAL_PATTERN_WITH_COMPUTED_PROPERTIES
            } else {
                0
            };
        if destructuring {
            self.bindings.pattern_for_type.insert(result, node);
        }
        Ok(result)
    }
    // port: tsc/internal/checker/checker.go:Checker.hasDefaultValue
    fn object_member_has_default(&self, node: NodeId) -> Result<bool, Error> {
        let read = self.ast(node)?.node(node)?;
        match read.kind().known() {
            Some(K::BindingElement) => Ok(read.initializer().is_some()),
            Some(K::PropertyAssignment) => self.object_member_has_default(
                read.initializer()
                    .ok_or(Error::MissingLink("property initializer"))?,
            ),
            Some(K::ShorthandPropertyAssignment) => Ok(read
                .data_source()
                .as_shorthand_property_assignment()
                .ok_or(Error::MissingLink("shorthand property"))?
                .object_assignment_initializer()
                .is_some()),
            Some(K::BinaryExpression) => Ok(self
                .ast(
                    read.data_source()
                        .as_binary_expression()
                        .ok_or(Error::MissingLink("binary default"))?
                        .operator_token()
                        .ok_or(Error::MissingLink("default operator"))?,
                )?
                .node(
                    read.data_source()
                        .as_binary_expression()
                        .ok_or(Error::MissingLink("binary default"))?
                        .operator_token()
                        .ok_or(Error::MissingLink("default operator"))?,
                )?
                .kind()
                == K::EqualsToken),
            _ => Ok(false),
        }
    }
    fn check_object_mutable_location_ex(
        &mut self,
        node: NodeId,
        mode: u32,
    ) -> Result<TypeId, Error> {
        let saved = std::mem::replace(&mut self.expression_mode, mode);
        let result = self.check_expression_for_mutable_location(node);
        self.expression_mode = saved;
        result
    }
    // port: tsc/internal/checker/checker.go:Checker.checkPropertyAssignment
    pub(crate) fn check_object_property_assignment(
        &mut self,
        node: NodeId,
        mode: u32,
    ) -> Result<TypeId, Error> {
        let read = self.ast(node)?.node(node)?;
        let name = read.name();
        let initializer = read
            .initializer()
            .ok_or(Error::MissingLink("property initializer"))?;
        let annotation = read.type_node();
        if let Some(name) = name {
            if self.ast(name)?.node(name)?.kind() == K::ComputedPropertyName {
                self.check_computed_property_name(name)?;
            }
        }
        let ty = self.check_object_mutable_location_ex(initializer, mode)?;
        if let Some(annotation) = annotation {
            let target = self.get_type_from_type_node(annotation)?;
            self.check_expression_related_with_elaboration(
                ty,
                target,
                crate::RelationKind::Assignable,
                Some(node),
                Some(initializer),
                None,
            )?;
            return Ok(target);
        }
        Ok(ty)
    }
    // port: tsc/internal/checker/checker.go:Checker.checkShorthandPropertyAssignment
    pub(crate) fn check_shorthand_property_assignment(
        &mut self,
        node: NodeId,
        destructuring: bool,
        mode: u32,
    ) -> Result<TypeId, Error> {
        let read = self.ast(node)?.node(node)?;
        let annotation = read.type_node();
        let initializer = read
            .data_source()
            .as_shorthand_property_assignment()
            .ok_or(Error::MissingLink("shorthand property"))?
            .object_assignment_initializer();
        let expression = if destructuring { None } else { initializer }
            .or(read.name())
            .ok_or(Error::MissingLink("shorthand expression"))?;
        let ty = self.check_object_mutable_location_ex(expression, mode)?;
        if let Some(annotation) = annotation {
            let target = self.get_type_from_type_node(annotation)?;
            self.check_expression_related_with_elaboration(
                ty,
                target,
                crate::RelationKind::Assignable,
                Some(node),
                Some(expression),
                None,
            )?;
            return Ok(target);
        }
        Ok(ty)
    }
    // port: tsc/internal/checker/checker.go:Checker.checkObjectLiteralMethod
    pub(crate) fn check_object_literal_method(&mut self, node: NodeId) -> Result<TypeId, Error> {
        self.check_object_method_grammar(node)?;
        if let Some(name) = self.ast(node)?.node(node)?.name() {
            if self.ast(name)?.node(name)?.kind() == K::ComputedPropertyName {
                self.check_computed_property_name(name)?;
            }
        }
        let ty = self.check_function_expression(node)?;
        self.instantiate_single_generic_function(node, ty, self.expression_mode)
    }

    // port: tsc/internal/checker/checker.go:Checker.checkSpreadPropOverrides
    fn check_spread_property_overrides(
        &mut self,
        ty: TypeId,
        properties: &SymbolTable,
        spread: NodeId,
    ) -> Result<(), Error> {
        for right in self.get_properties_of_type(ty)? {
            let read = self.symbol(right)?;
            if read.flags() & sf::OPTIONAL == 0 && read.check_flags() & cf::PARTIAL == 0 {
                let name = read.name_to_owned();
                if let Some(left) = properties.get(name.as_bytes()).copied().flatten() {
                    let declaration = self.symbol(left)?.value_declaration();
                    let mut diagnostic = self.diagnostic_for_node(
                        declaration,
                        d::X_0_is_specified_more_than_once_so_this_usage_will_be_overwritten,
                        vec![name],
                    )?;
                    diagnostic.related_information.push(std::sync::Arc::new(
                        self.diagnostic_for_node(
                            Some(spread),
                            d::This_spread_always_overwrites_this_property,
                            vec![],
                        )?,
                    ));
                    self.add_diagnostic(diagnostic)?;
                }
            }
        }
        Ok(())
    }
    // port: tsc/internal/checker/checker.go:Checker.checkContextualDeprecations
    pub(crate) fn check_object_contextual_deprecations(
        &mut self,
        node: NodeId,
    ) -> Result<(), Error> {
        let Some(ty) = self.apparent_contextual_expression_type(node)? else {
            return Ok(());
        };
        for property in self.source_list(node, self.ast(node)?.node(node)?.property_list())? {
            if let Some(name) = self.ast(property)?.node(property)?.name() {
                if self.ast(name)?.node(name)?.kind() != K::ComputedPropertyName {
                    let text = self.ast(name)?.node_text(name)?.into_js_string();
                    // port: tsc/internal/checker/checker.go:Checker.checkDeprecatedProperty
                    if let Some(symbol) = self.constituent_property(ty, text.as_bytes(), false)? {
                        if !self.symbol_declarations(symbol)?.is_empty()
                            && self.is_deprecated_symbol(symbol)?
                        {
                            let declarations = self.symbol_declarations(symbol)?.to_vec();
                            self.add_deprecated_suggestion(name, &declarations, text)?;
                        }
                    }
                }
            }
        }
        Ok(())
    }
}
