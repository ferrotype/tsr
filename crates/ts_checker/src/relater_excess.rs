use crate::{
    object_flags as of,
    relater::{Relater, RelationKind},
    type_flags as tf, CheckerState, Error, TypeId,
};
use ts_arena::{NodeId, SymbolId};
use ts_ast::{symbol_flags as sf, JsString, SymbolFlags, SyntaxKind as K};
use ts_diagnostics as d;

impl CheckerState {
    // port: tsc/internal/checker/relater.go:isExcessPropertyCheckTarget
    fn excess_check_target(&self, ty: TypeId) -> Result<bool, Error> {
        let record = self.types.get(ty)?;
        if record.flags & tf::OBJECT != 0
            && record.object_flags & of::OBJECT_LITERAL_PATTERN_WITH_COMPUTED_PROPERTIES == 0
            || record.flags & tf::NON_PRIMITIVE != 0
        {
            return Ok(true);
        }
        if record.flags & tf::SUBSTITUTION != 0 {
            return self.excess_check_target(self.types.substitution(ty)?.base);
        }
        if record.flags & tf::UNION_OR_INTERSECTION != 0 {
            let union = record.flags & tf::UNION != 0;
            for &part in self.types.types_of(ty)? {
                if self.excess_check_target(part)? == union {
                    return Ok(union);
                }
            }
            return Ok(!union);
        }
        Ok(false)
    }

    // port: tsc/internal/checker/relater.go:Checker.isKnownProperty
    pub(crate) fn known_property(
        &mut self,
        ty: TypeId,
        name: &[u8],
        jsx: bool,
    ) -> Result<bool, Error> {
        let flags = self.types.flags(ty)?;
        if flags & tf::OBJECT != 0 {
            if self.constituent_property(ty, name, true)?.is_some() {
                return Ok(true);
            }
            let late = name.starts_with(b"\xfe@");
            let key = if late {
                self.builtins.es_symbol_type
            } else {
                self.get_string_literal_type(ts_ast::JsString::from_bytes(name))?
            };
            if self.applicable_index_info(ty, key)?.is_some()
                || late
                    && self
                        .index_info_of_type(ty, self.builtins.string_type)?
                        .is_some()
                || jsx && name.contains(&b'-')
            {
                return Ok(true);
            }
        }
        if flags & tf::SUBSTITUTION != 0 {
            return self.known_property(self.types.substitution(ty)?.base, name, jsx);
        }
        if flags & tf::UNION_OR_INTERSECTION != 0 && self.excess_check_target(ty)? {
            let parts = self.types.compound_types(ty)?.clone();
            for &part in parts.iter() {
                if self.known_property(part, name, jsx)? {
                    return Ok(true);
                }
            }
        }
        Ok(false)
    }
}

impl Relater<'_> {
    // port: tsc/internal/checker/relater.go:Relater.isRelatedToEx
    pub(crate) fn no_common_properties(
        &mut self,
        source: TypeId,
        target: TypeId,
    ) -> Result<bool, Error> {
        let s = self.checker.types.flags(source)?;
        let t = self.checker.types.flags(target)?;
        if self.kind == RelationKind::Comparable && s & tf::UNIT == 0
            || s & (tf::PRIMITIVE | tf::OBJECT | tf::INTERSECTION) == 0
            || self.checker.query.global_types.get("Object") == Some(&source)
            || t & (tf::OBJECT | tf::INTERSECTION) == 0
            || !self.checker.is_weak_type(target)?
        {
            return Ok(false);
        }
        let properties = self.checker.get_properties_of_type(source)?;
        if properties.is_empty()
            && self.checker.signatures_of_type(source, false)?.is_empty()
            && self.checker.signatures_of_type(source, true)?.is_empty()
        {
            return Ok(false);
        }
        let jsx = self.checker.types.get(source)?.object_flags & of::JSX_ATTRIBUTES != 0;
        for property in properties {
            let name = self.checker.symbol(property)?.name_to_owned();
            if self.checker.known_property(target, name.as_bytes(), jsx)? {
                return Ok(false);
            }
        }
        Ok(true)
    }

    // port: tsc/internal/checker/relater.go:Relater.hasExcessProperties
    pub(crate) fn excess_properties(
        &mut self,
        source: TypeId,
        target: TypeId,
        report: bool,
    ) -> Result<bool, Error> {
        let record = *self.checker.types.get(source)?;
        if record.object_flags & (of::OBJECT_LITERAL | of::FRESH_LITERAL)
            != (of::OBJECT_LITERAL | of::FRESH_LITERAL)
            || !self.checker.excess_check_target(target)?
        {
            return Ok(false);
        }
        if self.checker.types.get(target)?.object_flags & of::JS_LITERAL != 0 {
            let options = self.checker.program()?.host.options();
            if !options.strict_option_value(options.no_implicit_any) {
                return Ok(false);
            }
        }
        let jsx = record.object_flags & of::JSX_ATTRIBUTES != 0;
        if matches!(
            self.kind,
            RelationKind::Assignable | RelationKind::Comparable
        ) {
            let global_object = self.checker.query.global_types.get("Object").copied();
            let subset = global_object == Some(target)
                || self.checker.types.flags(target)? & tf::UNION != 0
                    && global_object.is_some_and(|global| {
                        self.checker
                            .types
                            .types_of(target)
                            .is_ok_and(|parts| parts.contains(&global))
                    });
            if subset || !jsx && self.checker.empty_object_type(target)? {
                return Ok(false);
            }
        }
        let mut reduced = target;
        let union = self.checker.types.flags(target)? & tf::UNION != 0;
        if union {
            if let Some(matching) = self.matching_discriminant_type(source, target)? {
                reduced = matching;
            } else {
                let mut non_primitive = false;
                for &part in self.checker.types.types_of(target)? {
                    non_primitive |= self.checker.types.flags(part)?
                        & (tf::INSTANTIABLE_NON_PRIMITIVE | tf::OBJECT | tf::INTERSECTION)
                        != 0;
                }
                if non_primitive {
                    reduced = self.checker.filter_type_flags(target, !tf::PRIMITIVE)?;
                }
            }
        }
        let container = record
            .symbol
            .ok_or(Error::MissingLink("fresh literal symbol"))?;
        let container_declaration = self.checker.symbol(container)?.value_declaration();
        for property in self.checker.get_properties_of_type(source)? {
            let read = self.checker.symbol(property)?;
            let name = read.name_to_owned();
            let Some(declaration) = read.value_declaration() else {
                continue;
            };
            if container_declaration.is_none()
                || self.checker.ast(declaration)?.node(declaration)?.parent()
                    != container_declaration
                || jsx && name.as_bytes().contains(&b'-')
            {
                continue;
            }
            if !self.checker.known_property(reduced, name.as_bytes(), jsx)? {
                if report {
                    self.report_excess_property(property, declaration, container, reduced, jsx)?;
                }
                return Ok(true);
            }
            if union {
                let parts = if self.checker.types.flags(reduced)? & tf::UNION != 0 {
                    self.checker.types.compound_types(reduced)?.clone()
                } else {
                    vec![reduced].into()
                };
                let mut types = Vec::new();
                for &part in parts.iter() {
                    if let Some(property) =
                        self.checker
                            .constituent_property(part, name.as_bytes(), true)?
                    {
                        types.push(self.checker.get_type_of_symbol(property)?);
                    } else {
                        let key = self.checker.get_string_literal_type(name.clone())?;
                        types.push(
                            if let Some(info) = self.checker.applicable_index_info(part, key)? {
                                self.checker.signatures.index_info(info)?.value_type
                            } else {
                                self.checker.builtins.undefined_type
                            },
                        );
                    }
                }
                let target = self.checker.get_union_type(&types)?;
                let source = self.checker.get_type_of_symbol(property)?;
                if self.related_with_errors(source, target, crate::relater::BOTH, 0, report)?
                    == crate::ternary::FALSE
                {
                    if report {
                        let text = self.checker.symbol_to_string(property)?;
                        self.report_error(d::Types_of_property_0_are_incompatible, vec![text]);
                    }
                    return Ok(true);
                }
            }
        }
        Ok(false)
    }

    /// The error branch of `hasExcessProperties`: the diagnostic is reported in
    /// terms of the object constituents of the target, at the offending property
    /// when it is written inside the literal itself.
    fn report_excess_property(
        &mut self,
        property: SymbolId,
        declaration: NodeId,
        container: SymbolId,
        reduced: TypeId,
        jsx: bool,
    ) -> Result<(), Error> {
        // Report error in terms of object types in the target as those are the only ones
        // we check in isKnownProperty.
        let error_target = self.checker.filter_type(reduced, &mut |checker, part| {
            checker.excess_check_target(part)
        })?;
        let error_node = self
            .errors
            .error_node
            .ok_or(Error::MissingLink("No errorNode in hasExcessProperties"))?;
        let error_read = self.checker.ast(error_node)?.node(error_node)?;
        let jsx_error = jsx
            || ts_ast::is_jsx_attributes(&error_read)
            || ts_ast::utilities_middle::is_jsx_opening_like_element(&error_read)
            || error_read
                .parent()
                .map(|parent| {
                    Ok::<_, Error>(ts_ast::utilities_middle::is_jsx_opening_like_element(
                        &self.checker.ast(parent)?.node(parent)?,
                    ))
                })
                .transpose()?
                .unwrap_or(false);
        if jsx_error {
            return Err(Error::Unsupported("hasExcessProperties: JSX attributes"));
        }
        // use the property's value declaration if the property is assigned inside the literal itself
        let object_literal_declaration = self
            .checker
            .symbol_declarations(container)?
            .get(0)
            .flatten();
        let view = self.checker.ast(declaration)?;
        let declaration_read = view.node(declaration)?;
        let mut suggestion = None;
        if let Some(literal) = object_literal_declaration {
            if ts_ast::utilities::is_object_literal_element(&declaration_read)
                && ts_ast::utilities::find_ancestor(view, Some(declaration), |node| {
                    node.id() == literal
                })?
                .is_some()
                && ts_ast::utilities::get_source_file_of_node(view, Some(literal))?
                    == ts_ast::utilities::get_source_file_of_node(
                        self.checker.ast(error_node)?,
                        Some(error_node),
                    )?
            {
                let name = declaration_read
                    .name()
                    .ok_or(Error::MissingLink("object literal element name"))?;
                self.errors.error_node = Some(name);
                if view.node(name)?.kind() == K::Identifier {
                    let text = view.node_text(name)?.into_js_string();
                    suggestion = self
                        .checker
                        .suggestion_for_nonexistent_property(text.as_bytes(), error_target)?;
                }
            }
        }
        let property_name = self.checker.symbol_to_string(property)?;
        let target_name = self
            .checker
            .type_to_string(error_target, crate::type_display::DEFAULT_FLAGS)?;
        if let Some(suggestion) = suggestion {
            self.report_error(
                d::Object_literal_may_only_specify_known_properties_but_0_does_not_exist_in_type_1_Did_you_mean_to_write_2,
                vec![property_name, target_name, suggestion],
            );
        } else {
            self.report_error(
                d::Object_literal_may_only_specify_known_properties_and_0_does_not_exist_in_type_1,
                vec![property_name, target_name],
            );
        }
        Ok(())
    }
}

impl CheckerState {
    // port: tsc/internal/checker/checker.go:Checker.getSuggestionForNonexistentProperty
    pub(crate) fn suggestion_for_nonexistent_property(
        &mut self,
        name: &[u8],
        containing: TypeId,
    ) -> Result<Option<JsString>, Error> {
        let properties = self.get_properties_of_type(containing)?;
        match self.spelling_suggestion_for_name(name, &properties, sf::VALUE)? {
            Some(symbol) => Ok(Some(self.symbol(symbol)?.name_to_owned())),
            None => Ok(None),
        }
    }

    // port: tsc/internal/checker/checker.go:Checker.getSpellingSuggestionForName
    pub(crate) fn spelling_suggestion_for_name(
        &mut self,
        name: &[u8],
        symbols: &[SymbolId],
        meaning: SymbolFlags,
    ) -> Result<Option<SymbolId>, Error> {
        let mut candidates: Vec<(JsString, SymbolId)> = Vec::with_capacity(symbols.len());
        for &candidate in symbols {
            let candidate_name = self.ast_symbol_name(candidate)?;
            let bytes = candidate_name.as_bytes();
            if bytes.is_empty() || bytes[0] == b'"' || bytes[0] == 0xFE {
                continue;
            }
            let flags = self.symbol(candidate)?.flags();
            let matches_meaning = flags & meaning != 0
                || flags & sf::ALIAS != 0 && {
                    let alias = self.resolve_alias(candidate)?;
                    alias != self.builtins.unknown_symbol
                        && self.symbol(alias)?.flags() & meaning != 0
                };
            if matches_meaning {
                candidates.push((candidate_name, candidate));
            }
        }
        let failure = std::cell::Cell::new(None);
        let result = ts_scanner::get_spelling_suggestion(
            name,
            candidates.iter(),
            |entry| entry.0.as_bytes(),
            |a, b| match self.compare_symbols(Some(a.1), Some(b.1)) {
                Ok(order) => order,
                Err(error) => {
                    failure.set(Some(error));
                    std::cmp::Ordering::Equal
                }
            },
            0,
        )
        .map(|entry| entry.1);
        if let Some(error) = failure.into_inner() {
            return Err(error);
        }
        Ok(result)
    }
}
