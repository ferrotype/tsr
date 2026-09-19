use crate::{
    element_flags as ef,
    relater::{Relater, RelationKind, BOTH},
    ternary as tr, type_flags as tf, Error, Ternary, TypeId,
};
use ts_ast::JsString;
use ts_diagnostics as messages;

impl Relater<'_> {
    // port: tsc/internal/checker/relater.go:Relater.propertiesRelatedTo
    pub(crate) fn tuple_properties_related(
        &mut self,
        source: TypeId,
        target: TypeId,
        intersection: u32,
        excluded: &[ts_ast::JsString],
    ) -> Result<Ternary, Error> {
        let source_tuple = self.checker.is_tuple_type(source)?;
        let target_data = self
            .checker
            .types
            .tuple(self.checker.types.target(target)?)?;
        let target_infos = target_data.element_infos.clone();
        let target_min = target_data.min_length as usize;
        let target_flags = target_data.combined_flags;
        let target_readonly = target_data.readonly;
        let source_infos = if source_tuple {
            Some(
                self.checker
                    .types
                    .tuple(self.checker.types.target(source)?)?
                    .element_infos
                    .clone(),
            )
        } else {
            None
        };
        let source_readonly = self.checker.readonly_array_or_tuple(source)?;
        if !target_readonly && source_readonly {
            return Ok(tr::FALSE);
        }
        let source_arguments = self.checker.get_type_arguments(source)?;
        let target_arguments = self.checker.get_type_arguments(target)?;
        let source_arity = source_arguments.len();
        let target_arity = target_arguments.len();
        let source_rest = if source_tuple {
            self.checker
                .types
                .tuple(self.checker.types.target(source)?)?
                .combined_flags
                & ef::REST
                != 0
        } else {
            true
        };
        let source_min = if source_tuple {
            self.checker
                .types
                .tuple(self.checker.types.target(source)?)?
                .min_length as usize
        } else {
            0
        };
        let target_rest = target_flags & ef::REST != 0;
        let target_variable = target_flags & ef::VARIABLE != 0;
        if !source_rest && source_arity < target_min {
            if self.report_errors {
                self.report_error(
                    messages::Source_has_0_element_s_but_target_requires_1,
                    vec![number_text(source_arity), number_text(target_min)],
                );
            }
            return Ok(tr::FALSE);
        }
        if !target_variable && target_arity < source_min {
            if self.report_errors {
                self.report_error(
                    messages::Source_has_0_element_s_but_target_allows_only_1,
                    vec![number_text(source_min), number_text(target_arity)],
                );
            }
            return Ok(tr::FALSE);
        }
        if !target_variable && (source_rest || target_arity < source_arity) {
            if self.report_errors {
                let (message, count) = if source_min < target_min {
                    (
                        messages::Target_requires_0_element_s_but_source_may_have_fewer,
                        target_min,
                    )
                } else {
                    (
                        messages::Target_allows_only_0_element_s_but_source_may_have_more,
                        target_arity,
                    )
                };
                self.report_error(message, vec![number_text(count)]);
            }
            return Ok(tr::FALSE);
        }
        let target_start = target_infos
            .iter()
            .take_while(|info| info.flags & ef::NON_REST != 0)
            .count();
        let target_end = target_infos
            .iter()
            .rev()
            .take_while(|info| info.flags & ef::NON_REST != 0)
            .count();
        let mut result = tr::TRUE;
        let mut can_exclude = !excluded.is_empty();
        for (position, &source_type) in source_arguments.iter().enumerate() {
            let source_flags = source_infos
                .as_ref()
                .map_or(ef::REST, |infos| infos[position].flags);
            let from_end = source_arity - 1 - position;
            let target_position = if target_rest && position >= target_start {
                target_arity - 1 - from_end.min(target_end)
            } else if position >= target_arity {
                if self.report_errors {
                    self.report_error(
                        messages::Target_allows_only_0_element_s_but_source_may_have_more,
                        vec![number_text(target_arity)],
                    );
                }
                return Ok(tr::FALSE);
            } else {
                position
            };
            let flags = target_infos[target_position].flags;
            let mismatch = if flags & ef::VARIADIC != 0 && source_flags & ef::VARIADIC == 0 {
                Some((
                    messages::Source_provides_no_match_for_variadic_element_at_position_0_in_target,
                    target_position,
                    None,
                ))
            } else if source_flags & ef::VARIADIC != 0 && flags & ef::VARIABLE == 0 {
                Some((messages::Variadic_element_at_position_0_in_source_does_not_match_element_at_position_1_in_target, position, Some(target_position)))
            } else if flags & ef::REQUIRED != 0 && source_flags & ef::REQUIRED == 0 {
                Some((
                    messages::Source_provides_no_match_for_required_element_at_position_0_in_target,
                    target_position,
                    None,
                ))
            } else {
                None
            };
            if let Some((message, first, second)) = mismatch {
                if self.report_errors {
                    let mut args = vec![number_text(first)];
                    if let Some(second) = second {
                        args.push(number_text(second));
                    }
                    self.report_error(message, args);
                }
                return Ok(tr::FALSE);
            }
            if can_exclude {
                if (source_flags | flags) & ef::VARIABLE != 0 {
                    can_exclude = false;
                }
                if can_exclude
                    && excluded
                        .iter()
                        .any(|name| name.as_bytes() == position.to_string().as_bytes())
                {
                    continue;
                }
            }
            let source_type = self
                .checker
                .remove_missing_type(source_type, source_flags & flags & ef::OPTIONAL != 0)?;
            let target_type = target_arguments[target_position];
            let target_type = if source_flags & ef::VARIADIC != 0 && flags & ef::REST != 0 {
                self.checker.create_array_type(target_type, false)?
            } else {
                self.checker
                    .remove_missing_type(target_type, flags & ef::OPTIONAL != 0)?
            };
            result &= self.related(source_type, target_type, BOTH, intersection)?;
            if result == tr::FALSE {
                if self.report_errors && (target_arity > 1 || source_arity > 1) {
                    if target_rest
                        && position >= target_start
                        && from_end >= target_end
                        && target_start != source_arity - target_end - 1
                    {
                        self.report_error(messages::Type_at_positions_0_through_1_in_source_is_not_compatible_with_type_at_position_2_in_target,
                            vec![number_text(target_start), number_text(source_arity - target_end - 1), number_text(target_position)]);
                    } else {
                        self.report_error(messages::Type_at_position_0_in_source_is_not_compatible_with_type_at_position_1_in_target,
                            vec![number_text(position), number_text(target_position)]);
                    }
                }
                return Ok(tr::FALSE);
            }
        }
        Ok(result)
    }

    pub(crate) fn array_relation(
        &mut self,
        source: TypeId,
        target: TypeId,
    ) -> Result<Option<Ternary>, Error> {
        if !self.checker.is_array_type(target)? {
            return Ok(None);
        }
        let readonly = self.checker.readonly_array_or_tuple(target)?;
        let parts = if self.checker.types.flags(source)? & tf::UNION != 0 {
            self.checker.types.compound_types(source)?.clone()
        } else {
            vec![source].into()
        };
        for &part in parts.iter() {
            if readonly {
                if !self.checker.is_tuple_type(part)? && !self.checker.is_array_type(part)? {
                    return Ok(None);
                }
            } else if !self.checker.is_tuple_type(part)?
                || self.checker.readonly_array_or_tuple(part)?
            {
                return Ok(None);
            }
        }
        if self.kind == RelationKind::Identity {
            return Ok(Some(tr::FALSE));
        }
        let number = self.checker.builtins.number_type;
        let source_info = self.checker.index_info_of_type(source, number)?;
        let target_info = self.checker.index_info_of_type(target, number)?;
        let source_type = match source_info {
            Some(info) => self.checker.signatures.index_info(info)?.value_type,
            None => self.checker.builtins.any_type,
        };
        let target_type = match target_info {
            Some(info) => self.checker.signatures.index_info(info)?.value_type,
            None => self.checker.builtins.any_type,
        };
        self.related(source_type, target_type, BOTH, 0).map(Some)
    }
}

impl crate::CheckerState {
    pub(crate) fn readonly_array_or_tuple(&self, ty: TypeId) -> Result<bool, Error> {
        if self.is_tuple_type(ty)? {
            return Ok(self.types.tuple(self.types.target(ty)?)?.readonly);
        }
        Ok(self.is_array_type(ty)?
            && self.query.global_types.get("ReadonlyArray") == Some(&self.types.target(ty)?))
    }
}

fn number_text(value: usize) -> JsString {
    JsString::from_bytes(value.to_string().into_bytes())
}
