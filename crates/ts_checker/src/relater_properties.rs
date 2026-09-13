//! Structural property and index-signature comparisons.
use crate::{
    object_flags as of,
    relater::{Relater, RelationKind, BOTH, SOURCE},
    ternary as tr, type_flags as tf, CheckerState, Error, IndexInfoId, Ternary, TypeId,
};
use ts_arena::SymbolId;
use ts_ast::{check_flags as cf, modifier_flags as mf, symbol_flags as sf};

impl CheckerState {
    // port: tsc/internal/checker/checker.go:Checker.isValidOverrideOf
    fn valid_protected_override(
        &mut self,
        source: SymbolId,
        target: SymbolId,
    ) -> Result<bool, Error> {
        for target in self.underlying_access_properties(target)? {
            if self.property_modifiers(target)? & mf::PROTECTED == 0 {
                continue;
            }
            let base = self.property_declaring_class(target)?;
            let mut derives = false;
            for source in self.underlying_access_properties(source)? {
                if let (Some(class), Some(base)) = (self.property_declaring_class(source)?, base) {
                    if self.has_base_type(class, base)? {
                        derives = true;
                        break;
                    }
                }
            }
            if !derives {
                return Ok(false);
            }
        }
        Ok(true)
    }

    // port: tsc/internal/checker/relater.go:Checker.isWeakType
    pub(crate) fn is_weak_type(&mut self, ty: TypeId) -> Result<bool, Error> {
        let flags = self.types.flags(ty)?;
        if flags & tf::OBJECT != 0 {
            self.resolve_type_members(ty)?;
            let members = self.types.structured(ty)?;
            if members
                .signatures
                .as_ref()
                .is_some_and(|types| !types.is_empty())
                || members
                    .index_infos
                    .as_ref()
                    .is_some_and(|types| !types.is_empty())
            {
                return Ok(false);
            }
            let properties = members.properties.as_deref().unwrap_or_default();
            if properties.is_empty() {
                return Ok(false);
            }
            for &property in properties {
                if self.symbol(property)?.flags() & sf::OPTIONAL == 0 {
                    return Ok(false);
                }
            }
            return Ok(true);
        }
        if flags & tf::INTERSECTION != 0 {
            let parts = self.types.compound_types(ty)?.clone();
            for &part in parts.iter() {
                if !self.is_weak_type(part)? {
                    return Ok(false);
                }
            }
            return Ok(true);
        }
        if flags & tf::SUBSTITUTION != 0 {
            return self.is_weak_type(self.types.substitution(ty)?.base);
        }
        Ok(false)
    }

    // port: tsc/internal/checker/checker.go:Checker.isEmptyObjectType
    pub(crate) fn empty_object_type(&mut self, ty: TypeId) -> Result<bool, Error> {
        let flags = self.types.flags(ty)?;
        if flags & tf::OBJECT != 0 {
            self.resolve_type_members(ty)?;
            if self.is_generic_mapped_type(ty)? {
                return Ok(false);
            }
            let data = self.types.structured(ty)?;
            return Ok(data
                .properties
                .as_ref()
                .is_none_or(|types| types.is_empty())
                && data
                    .signatures
                    .as_ref()
                    .is_none_or(|types| types.is_empty())
                && data
                    .index_infos
                    .as_ref()
                    .is_none_or(|types| types.is_empty()));
        }
        if flags & tf::NON_PRIMITIVE != 0 {
            return Ok(true);
        }
        if flags & tf::UNION_OR_INTERSECTION != 0 {
            let union = flags & tf::UNION != 0;
            let parts = self.types.compound_types(ty)?.clone();
            for &part in parts.iter() {
                if self.empty_object_type(part)? == union {
                    return Ok(union);
                }
            }
            return Ok(!union);
        }
        Ok(false)
    }

    // port: tsc/internal/checker/relater.go:Checker.isObjectTypeWithInferableIndex
    pub(crate) fn inferable_index(&mut self, ty: TypeId) -> Result<bool, Error> {
        let record = *self.types.get(ty)?;
        if record.flags & tf::INTERSECTION != 0 {
            let parts = self.types.compound_types(ty)?.clone();
            for &part in parts.iter() {
                if !self.inferable_index(part)? {
                    return Ok(false);
                }
            }
            return Ok(true);
        }
        if let Some(symbol) = record.symbol {
            let flags = self.symbol(symbol)?.flags();
            if flags & (sf::OBJECT_LITERAL | sf::TYPE_LITERAL | sf::ENUM | sf::VALUE_MODULE) != 0
                && flags & sf::CLASS == 0
                && self.signatures_of_type(ty, false)?.is_empty()
                && self.signatures_of_type(ty, true)?.is_empty()
            {
                return Ok(true);
            }
        }
        if record.object_flags & (of::JS_LITERAL | of::OBJECT_REST_TYPE) != 0 {
            return Ok(true);
        }
        if record.object_flags & of::REVERSE_MAPPED != 0 {
            return self.inferable_index(self.reverse_mapped_parts(ty)?.0);
        }
        Ok(false)
    }
}

impl Relater<'_> {
    // port: tsc/internal/checker/relater.go:Relater.propertiesRelatedTo
    pub(crate) fn properties_related(
        &mut self,
        source: TypeId,
        target: TypeId,
        optional_only: bool,
        intersection: u32,
    ) -> Result<Ternary, Error> {
        self.properties_related_except(source, target, optional_only, intersection, &[])
    }

    pub(crate) fn properties_related_except(
        &mut self,
        source: TypeId,
        target: TypeId,
        optional_only: bool,
        intersection: u32,
        excluded: &[ts_ast::JsString],
    ) -> Result<Ternary, Error> {
        if self.kind == RelationKind::Identity {
            return self.properties_identical(source, target);
        }
        if self.checker.is_tuple_type(target)? {
            if self.checker.is_tuple_type(source)? || self.checker.is_array_type(source)? {
                return self.tuple_properties_related(source, target, intersection, excluded);
            }
            if self
                .checker
                .types
                .tuple(self.checker.types.target(target)?)?
                .combined_flags
                & crate::element_flags::VARIABLE
                != 0
            {
                return Ok(tr::FALSE);
            }
        }
        let source_record = *self.checker.types.get(source)?;
        let require_optional = matches!(
            self.kind,
            RelationKind::Subtype | RelationKind::StrictSubtype
        ) && source_record.object_flags & of::OBJECT_LITERAL == 0
            && !self.checker.is_tuple_type(source)?;
        let targets = self.checker.get_properties_of_type(target)?;
        // The missing-property pass precedes type comparisons in Go. Keep it
        // separate: resolving a property's type may allocate or cache relations.
        for &property in &targets {
            let read = self.checker.symbol(property)?;
            if read.flags() & sf::PROTOTYPE != 0 {
                continue;
            }
            if require_optional
                || read.flags() & sf::OPTIONAL == 0 && read.check_flags() & cf::PARTIAL == 0
            {
                let name = read.name_to_owned();
                if self
                    .checker
                    .constituent_property(source, name.as_bytes(), false)?
                    .is_none()
                {
                    if self.report_errors {
                        self.report_unmatched_property(source, target, property, require_optional)?;
                    }
                    return Ok(tr::FALSE);
                }
            }
        }
        if self.checker.types.get(target)?.object_flags & of::OBJECT_LITERAL != 0 {
            for property in self.checker.get_properties_of_type(source)? {
                let name = self.checker.symbol(property)?.name_to_owned();
                if excluded.contains(&name) {
                    continue;
                }
                if self
                    .checker
                    .constituent_property(target, name.as_bytes(), true)?
                    .is_none()
                {
                    return Ok(tr::FALSE);
                }
            }
        }
        let mut result = tr::TRUE;
        for property in targets {
            let read = self.checker.symbol(property)?;
            if read.flags() & sf::PROTOTYPE != 0
                || optional_only && read.flags() & sf::OPTIONAL == 0
            {
                continue;
            }
            let name = read.name_to_owned();
            if excluded.contains(&name) {
                continue;
            }
            if let Some(source_property) =
                self.checker
                    .constituent_property(source, name.as_bytes(), false)?
            {
                if source_property != property {
                    result &= self.property_related(
                        source,
                        target,
                        source_property,
                        property,
                        intersection,
                    )?;
                    if result == tr::FALSE {
                        return Ok(result);
                    }
                }
            }
        }
        Ok(result)
    }

    // port: tsc/internal/checker/relater.go:Relater.propertyRelatedTo
    fn property_related(
        &mut self,
        source_object: TypeId,
        target_object: TypeId,
        source: SymbolId,
        target: SymbolId,
        intersection: u32,
    ) -> Result<Ternary, Error> {
        self.property_related_as(
            source_object,
            target_object,
            source,
            target,
            intersection,
            None,
            self.kind == RelationKind::Comparable,
        )
    }

    #[allow(
        clippy::too_many_arguments,
        reason = "Pinned propertyRelatedTo carries both owner types, both symbols, recursion state, discriminant override and optionality independently"
    )]
    pub(crate) fn property_related_as(
        &mut self,
        source_object: TypeId,
        target_object: TypeId,
        source: SymbolId,
        target: SymbolId,
        intersection: u32,
        source_type: Option<TypeId>,
        skip_optional: bool,
    ) -> Result<Ternary, Error> {
        let s = self.checker.property_modifiers(source)?;
        let t = self.checker.property_modifiers(target)?;
        if (s | t) & mf::PRIVATE != 0 {
            if self.checker.symbol(source)?.value_declaration()
                != self.checker.symbol(target)?.value_declaration()
            {
                if self.report_errors {
                    let name = self.checker.symbol_to_string(target)?;
                    if s & t & mf::PRIVATE != 0 {
                        self.report_error(ts_diagnostics::Types_have_separate_declarations_of_a_private_property_0, vec![name]);
                    } else {
                        let (private, public) = if s & mf::PRIVATE != 0 {
                            (source_object, target_object)
                        } else {
                            (target_object, source_object)
                        };
                        let private = self
                            .checker
                            .type_to_string(private, crate::type_display::DEFAULT_FLAGS)?;
                        let public = self
                            .checker
                            .type_to_string(public, crate::type_display::DEFAULT_FLAGS)?;
                        self.report_error(
                            ts_diagnostics::Property_0_is_private_in_type_1_but_not_in_type_2,
                            vec![name, private, public],
                        );
                    }
                }
                return Ok(tr::FALSE);
            }
        } else if t & mf::PROTECTED != 0 {
            if !self.checker.valid_protected_override(source, target)? {
                if self.report_errors {
                    let name = self.checker.symbol_to_string(target)?;
                    let source = self
                        .checker
                        .property_declaring_class(source)?
                        .unwrap_or(source_object);
                    let target = self
                        .checker
                        .property_declaring_class(target)?
                        .unwrap_or(target_object);
                    let source = self
                        .checker
                        .type_to_string(source, crate::type_display::DEFAULT_FLAGS)?;
                    let target = self
                        .checker
                        .type_to_string(target, crate::type_display::DEFAULT_FLAGS)?;
                    self.report_error(ts_diagnostics::Property_0_is_protected_but_type_1_is_not_a_class_derived_from_2, vec![name, source, target]);
                }
                return Ok(tr::FALSE);
            }
        } else if s & mf::PROTECTED != 0 {
            if self.report_errors {
                let name = self.checker.symbol_to_string(target)?;
                let source = self
                    .checker
                    .type_to_string(source_object, crate::type_display::DEFAULT_FLAGS)?;
                let target = self
                    .checker
                    .type_to_string(target_object, crate::type_display::DEFAULT_FLAGS)?;
                self.report_error(
                    ts_diagnostics::Property_0_is_protected_in_type_1_but_public_in_type_2,
                    vec![name, source, target],
                );
            }
            return Ok(tr::FALSE);
        }
        if self.kind == RelationKind::StrictSubtype
            && self.checker.is_readonly_symbol(source)?
            && !self.checker.is_readonly_symbol(target)?
        {
            return Ok(tr::FALSE);
        }
        let target_optional = self.checker.options.strict_null_checks
            && self.checker.symbol(target)?.check_flags() & cf::PARTIAL != 0;
        let target_type = self.checker.non_missing_symbol_type(target)?;
        let target_type = self
            .checker
            .add_type_optionality(target_type, false, target_optional)?;
        let mask = if self.kind == RelationKind::StrictSubtype {
            tf::ANY
        } else {
            tf::ANY_OR_UNKNOWN
        };
        let related = if self.checker.types.flags(target_type)? & mask != 0 {
            tr::TRUE
        } else {
            let source_type = match source_type {
                Some(ty) => ty,
                None => self.checker.non_missing_symbol_type(source)?,
            };
            self.related(source_type, target_type, BOTH, intersection)?
        };
        if related == tr::FALSE {
            if self.report_errors {
                let name = self.checker.symbol_to_string(target)?;
                self.report_error(
                    ts_diagnostics::Types_of_property_0_are_incompatible,
                    vec![name],
                );
            }
            return Ok(related);
        }
        if !skip_optional
            && self.checker.symbol(source)?.flags() & sf::OPTIONAL != 0
            && self.checker.symbol(target)?.flags() & sf::CLASS_MEMBER != 0
            && self.checker.symbol(target)?.flags() & sf::OPTIONAL == 0
        {
            if self.report_errors {
                let name = self.checker.symbol_to_string(target)?;
                let source = self
                    .checker
                    .type_to_string(source_object, crate::type_display::DEFAULT_FLAGS)?;
                let target = self
                    .checker
                    .type_to_string(target_object, crate::type_display::DEFAULT_FLAGS)?;
                self.report_error(
                    ts_diagnostics::Property_0_is_optional_in_type_1_but_required_in_type_2,
                    vec![name, source, target],
                );
            }
            return Ok(tr::FALSE);
        }
        Ok(related)
    }

    // port: tsc/internal/checker/relater.go:Relater.propertiesIdenticalTo
    fn properties_identical(&mut self, source: TypeId, target: TypeId) -> Result<Ternary, Error> {
        if self.checker.types.flags(source)? & self.checker.types.flags(target)? & tf::OBJECT == 0 {
            return Ok(tr::FALSE);
        }
        let sources = self.checker.get_properties_of_type(source)?;
        if sources.len() != self.checker.get_properties_of_type(target)?.len() {
            return Ok(tr::FALSE);
        }
        let mut result = tr::TRUE;
        for property in sources {
            let name = self.checker.symbol(property)?.name_to_owned();
            let Some(other) = self
                .checker
                .constituent_property(target, name.as_bytes(), true)?
            else {
                return Ok(tr::FALSE);
            };
            let frame = self.frame;
            result &= self.checker.compare_properties(
                property,
                other,
                &mut |state, source, target| state.compare_in_relation_frame(frame, source, target),
            )?;
            if result == tr::FALSE {
                break;
            }
        }
        Ok(result)
    }

    // port: tsc/internal/checker/relater.go:Relater.indexSignaturesRelatedTo
    pub(crate) fn indexes_related(
        &mut self,
        source: TypeId,
        target: TypeId,
        primitive: bool,
        intersection: u32,
    ) -> Result<Ternary, Error> {
        let indexes = self.checker.index_infos_of_type(target)?;
        if self.kind == RelationKind::Identity {
            if indexes.len() != self.checker.index_infos_of_type(source)?.len() {
                return Ok(tr::FALSE);
            }
            for index in indexes {
                let target = self.checker.signatures.index_info(index)?.clone();
                let Some(source) = self.checker.index_info_of_type(source, target.key_type)? else {
                    return Ok(tr::FALSE);
                };
                let source = self.checker.signatures.index_info(source)?.clone();
                if source.is_readonly != target.is_readonly
                    || self.related(source.value_type, target.value_type, BOTH, 0)? == tr::FALSE
                {
                    return Ok(tr::FALSE);
                }
            }
            return Ok(tr::TRUE);
        }
        let mut has_string = false;
        for &index in &indexes {
            has_string |= self.checker.signatures.index_info(index)?.key_type
                == self.checker.builtins.string_type;
        }
        let mut result = tr::TRUE;
        for index in indexes {
            let target_info = self.checker.signatures.index_info(index)?.clone();
            let related = if self.kind != RelationKind::StrictSubtype
                && !primitive
                && has_string
                && self.checker.types.flags(target_info.value_type)? & tf::ANY != 0
            {
                tr::TRUE
            } else if self.checker.is_generic_mapped_type(source)? && has_string {
                let template = self.checker.mapped_template(source)?;
                self.related(template, target_info.value_type, BOTH, 0)?
            } else if let Some(info) = self
                .checker
                .applicable_index_info(source, target_info.key_type)?
            {
                self.index_info_related(info, index, intersection)?
            } else if intersection & SOURCE == 0
                && (self.kind != RelationKind::StrictSubtype
                    || self.checker.types.get(source)?.object_flags & of::FRESH_LITERAL != 0)
                && self.checker.inferable_index(source)?
            {
                self.members_related_to_index(source, index, intersection)?
            } else {
                if self.report_errors {
                    let key = self
                        .checker
                        .type_to_string(target_info.key_type, crate::type_display::DEFAULT_FLAGS)?;
                    let source = self
                        .checker
                        .type_to_string(source, crate::type_display::DEFAULT_FLAGS)?;
                    self.report_error(
                        ts_diagnostics::Index_signature_for_type_0_is_missing_in_type_1,
                        vec![key, source],
                    );
                }
                tr::FALSE
            };
            result &= related;
            if result == tr::FALSE {
                break;
            }
        }
        Ok(result)
    }

    // port: tsc/internal/checker/relater.go:Relater.membersRelatedToIndexInfo
    fn members_related_to_index(
        &mut self,
        source: TypeId,
        target: IndexInfoId,
        intersection: u32,
    ) -> Result<Ternary, Error> {
        let info = self.checker.signatures.index_info(target)?.clone();
        let mut result = tr::TRUE;
        for property in self.checker.get_properties_of_type(source)? {
            let key = self
                .checker
                .literal_type_from_property(property, tf::STRING_OR_NUMBER_LITERAL_OR_UNIQUE)?;
            if self.checker.applicable_index_type(key, info.key_type)? {
                let mut value = self.checker.non_missing_symbol_type(property)?;
                if !self.checker.options.exact_optional_property_types
                    && self.checker.types.flags(value)? & tf::UNDEFINED == 0
                    && info.key_type != self.checker.builtins.number_type
                    && self.checker.symbol(property)?.flags() & sf::OPTIONAL != 0
                {
                    value = self.checker.filter_type_flags(value, !tf::UNDEFINED)?;
                }
                let related = self.related(value, info.value_type, BOTH, intersection)?;
                if related == tr::FALSE {
                    if self.report_errors {
                        let name = self.checker.symbol_to_string(property)?;
                        self.report_error(
                            ts_diagnostics::Property_0_is_incompatible_with_index_signature,
                            vec![name],
                        );
                    }
                    return Ok(tr::FALSE);
                }
                result &= related;
            }
        }
        for source_index in self.checker.index_infos_of_type(source)? {
            let source_info = self.checker.signatures.index_info(source_index)?.clone();
            if self
                .checker
                .applicable_index_type(source_info.key_type, info.key_type)?
            {
                let related = self.index_info_related(source_index, target, intersection)?;
                if related == tr::FALSE {
                    return Ok(tr::FALSE);
                }
                result &= related;
            }
        }
        Ok(result)
    }

    // port: tsc/internal/checker/relater.go:Relater.indexInfoRelatedTo
    fn index_info_related(
        &mut self,
        source: IndexInfoId,
        target: IndexInfoId,
        intersection: u32,
    ) -> Result<Ternary, Error> {
        let source_info = self.checker.signatures.index_info(source)?.clone();
        let target_info = self.checker.signatures.index_info(target)?.clone();
        let related = self.related(
            source_info.value_type,
            target_info.value_type,
            BOTH,
            intersection,
        )?;
        if related == tr::FALSE && self.report_errors {
            let source_name = self
                .checker
                .type_to_string(source_info.key_type, crate::type_display::DEFAULT_FLAGS)?;
            if source_info.key_type == target_info.key_type {
                self.report_error(
                    ts_diagnostics::X_0_index_signatures_are_incompatible,
                    vec![source_name],
                );
            } else {
                let target_name = self
                    .checker
                    .type_to_string(target_info.key_type, crate::type_display::DEFAULT_FLAGS)?;
                self.report_error(
                    ts_diagnostics::X_0_and_1_index_signatures_are_incompatible,
                    vec![source_name, target_name],
                );
            }
        }
        Ok(related)
    }
}
