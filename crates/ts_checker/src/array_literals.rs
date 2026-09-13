//! Array construction shares tuple normalization and contextual element types.
use crate::{
    element_flags as ef, object_flags as of, type_flags as tf, CheckerState, Error,
    TupleElementInfo, TypeId, UnionReduction,
};
use ts_arena::NodeId;
use ts_ast::{JsString, SyntaxKind as K};

impl CheckerState {
    // port: tsc/internal/checker/checker.go:Checker.createArrayLiteralType
    fn array_literal_type(&mut self, ty: TypeId) -> Result<TypeId, Error> {
        let record = *self.types.get(ty)?;
        if record.object_flags & of::REFERENCE == 0 {
            return Ok(ty);
        }
        if let Some(&cached) = self.query.array_literal_types.get(&ty) {
            return Ok(cached);
        }
        let target = self.types.target(ty)?;
        let arguments = self
            .types
            .type_reference(ty)?
            .resolved_type_arguments
            .clone();
        let literal = self.new_object_type(of::REFERENCE, record.symbol)?;
        self.types.get_mut(literal)?.object_flags = record.object_flags & !of::MEMBERS_RESOLVED
            | of::ARRAY_LITERAL
            | of::CONTAINS_OBJECT_OR_ARRAY_LITERAL;
        let reference = self.types.type_reference_mut(literal)?;
        reference.object.target = Some(target);
        reference.resolved_type_arguments = arguments;
        self.query.array_literal_types.insert(ty, literal);
        Ok(literal)
    }

    // port: tsc/internal/checker/checker.go:Checker.isTupleLikeType
    pub(crate) fn tuple_like_type(&mut self, ty: TypeId) -> Result<bool, Error> {
        if self.is_tuple_type(ty)? || self.constituent_property(ty, b"0", false)?.is_some() {
            return Ok(true);
        }
        if self.is_array_like_type(ty)? {
            if let Some(length) = self.property_type(ty, b"length")? {
                for part in self.distributed_types(length)? {
                    if self.types.flags(part)? & tf::NUMBER_LITERAL == 0 {
                        return Ok(false);
                    }
                }
                return Ok(true);
            }
        }
        Ok(false)
    }

    // port: tsc/internal/checker/checker.go:Checker.checkArrayLiteral
    pub(crate) fn check_array_literal(&mut self, node: NodeId) -> Result<TypeId, Error> {
        let contextual = self.contextual_expression_type(node)?;
        let length = self.calls.contexts.len();
        if let Some(ty) = contextual {
            let inference = self
                .contextual_call_argument(node)
                .and_then(|context| context.inference);
            self.calls.contexts.push(crate::calls::ArgumentContext {
                node,
                ty,
                inference,
            });
        }
        let result = self.check_array_literal_worker(node);
        self.calls.contexts.truncate(length);
        result
    }

    fn check_array_literal_worker(&mut self, node: NodeId) -> Result<TypeId, Error> {
        let elements = self.source_list(node, self.ast(node)?.node(node)?.element_list())?;
        let destructuring = ts_ast::is_assignment_target(self.ast(node)?, node)?;
        let constant = self.is_const_context(node)?;
        let contextual = self.apparent_contextual_expression_type(node)?;
        let mut parent = self.ast(node)?.node(node)?.parent();
        while let Some(id) = parent {
            if self.ast(id)?.node(id)?.kind() != K::ParenthesizedExpression {
                break;
            }
            parent = self.ast(id)?.node(id)?.parent();
        }
        let mut tuple_context = false;
        if let Some(parent) = parent {
            if self.ast(parent)?.node(parent)?.kind() == K::SpreadElement {
                if let Some(call) = self.ast(parent)?.node(parent)?.parent() {
                    tuple_context = matches!(
                        self.ast(call)?.node(call)?.kind().known(),
                        Some(K::CallExpression | K::NewExpression)
                    );
                }
            }
        }
        if !tuple_context {
            if let Some(contextual) = contextual {
                tuple_context = self.any_type(contextual, &mut |c, t| {
                    if c.tuple_like_type(t)? {
                        return Ok(true);
                    }
                    if c.is_generic_mapped_type(t)? && c.types.mapped(t)?.name_type.is_none() {
                        let target = c.types.mapped(t)?.object.target.unwrap_or(t);
                        return Ok(c.homomorphic_type_variable(target)?.is_some());
                    }
                    Ok(false)
                })?;
            }
        }
        let mut types = Vec::with_capacity(elements.len());
        let mut infos = Vec::with_capacity(elements.len());
        let mut omitted = false;
        for element in elements {
            let read = self.ast(element)?.node(element)?;
            let (ty, flags) = if read.kind() == K::SpreadElement {
                let expression = read
                    .expression()
                    .ok_or(Error::MissingLink("spread expression"))?;
                let spread = self.check_expression_ex(expression, self.expression_mode)?;
                if self.is_array_like_type(spread)? {
                    (spread, ef::VARIADIC)
                } else if destructuring {
                    let element = match self
                        .index_info_of_type(spread, self.builtins.number_type)?
                    {
                        Some(index) => self.signatures.index_info(index)?.value_type,
                        None => self
                            .iterated_type_or_element_type(
                                crate::iteration::ALLOW_SYNC | crate::iteration::DESTRUCTURING_FLAG,
                                spread,
                                self.builtins.undefined_type,
                                None,
                                false,
                            )?
                            .unwrap_or(self.builtins.unknown_type),
                    };
                    (element, ef::REST)
                } else {
                    (
                        self.check_iterated_type_or_element_type(
                            crate::iteration::SPREAD,
                            spread,
                            self.builtins.undefined_type,
                            Some(expression),
                        )?,
                        ef::REST,
                    )
                }
            } else if self.options.exact_optional_property_types
                && read.kind() == K::OmittedExpression
            {
                omitted = true;
                (self.builtins.undefined_or_missing_type, ef::OPTIONAL)
            } else {
                let ty = self.check_expression_for_mutable_location(element)?;
                if tuple_context
                    && self.expression_mode & 2 != 0
                    && self.expression_mode & 4 == 0
                    && self.expression_is_context_sensitive(element)?
                {
                    return Err(Error::Unsupported(
                        "addIntraExpressionInferenceSite: array element",
                    ));
                }
                (
                    self.add_type_optionality(ty, true, omitted)?,
                    if omitted { ef::OPTIONAL } else { ef::REQUIRED },
                )
            };
            types.push(ty);
            infos.push(TupleElementInfo {
                flags,
                labeled_declaration: None,
            });
        }
        if destructuring {
            return self.create_tuple_type_ex(&types, &infos, false);
        }
        if self.expression_mode & 128 != 0 || constant || tuple_context {
            let mutable = if let Some(context) = contextual {
                self.some_mutable_array_like(context)?
            } else {
                false
            };
            let tuple = self.create_tuple_type_ex(&types, &infos, constant && !mutable)?;
            return self.array_literal_type(tuple);
        }
        let element = if types.is_empty() {
            if self.options.strict_null_checks {
                self.builtins.implicit_never_type
            } else {
                self.builtins.undefined_widening_type
            }
        } else {
            for (ty, info) in types.iter_mut().zip(&infos) {
                if info.flags & ef::VARIADIC != 0 {
                    *ty = self
                        .indexed_access_or_undefined(*ty, self.builtins.number_type, 0, None, None)?
                        .unwrap_or(self.builtins.any_type);
                }
            }
            self.get_union_type_ex(&types, UnionReduction::Subtype, None, None)?
        };
        let array = self.create_array_type(element, constant)?;
        self.array_literal_type(array)
    }

    pub(crate) fn contextual_array_element_ex(
        &mut self,
        node: NodeId,
        array: NodeId,
        context_flags: u32,
    ) -> Result<Option<TypeId>, Error> {
        let Some(context) = self.apparent_contextual_expression_type_ex(array, context_flags)?
        else {
            return Ok(None);
        };
        let elements = self.source_list(array, self.ast(array)?.node(array)?.element_list())?;
        let Some(index) = elements.iter().position(|&element| element == node) else {
            return Ok(None);
        };
        let mut spreads = Vec::new();
        for (i, &element) in elements.iter().enumerate() {
            if self.ast(element)?.node(element)?.kind() == K::SpreadElement {
                spreads.push(i);
            }
        }
        self.map_type_ex(
            context,
            &mut |c, t| {
                c.contextual_element_type(
                    t,
                    index,
                    Some(elements.len()),
                    spreads.first().copied(),
                    spreads.last().copied(),
                )
            },
            true,
        )
    }

    // port: tsc/internal/checker/checker.go:Checker.getContextualTypeForElementExpression
    pub(crate) fn contextual_element_type(
        &mut self,
        ty: TypeId,
        index: usize,
        length: Option<usize>,
        first_spread: Option<usize>,
        last_spread: Option<usize>,
    ) -> Result<Option<TypeId>, Error> {
        if self.is_tuple_type(ty)? {
            let tuple = self.types.tuple(self.types.target(ty)?)?;
            let fixed = tuple.fixed_length as usize;
            let infos = tuple.element_infos.clone();
            let combined = tuple.combined_flags;
            if first_spread.is_none_or(|first| index < first) && index < fixed {
                let element = self.element_types(ty)?[index];
                return self
                    .remove_missing_type(element, infos[index].flags & ef::OPTIONAL != 0)
                    .map(Some);
            }
            let offset = if last_spread.is_none_or(|last| index > last) {
                length.map_or(0, |length| length - index)
            } else {
                0
            };
            let fixed_end = if offset > 0 && combined & ef::VARIABLE != 0 {
                infos
                    .iter()
                    .rev()
                    .take_while(|info| info.flags & ef::FIXED != 0)
                    .count()
            } else {
                0
            };
            if offset > 0 && offset <= fixed_end {
                let elements = self.element_types(ty)?;
                return Ok(Some(elements[elements.len() - offset]));
            }
            let start = first_spread.map_or(fixed, |first| fixed.min(first));
            let end_skip = match (length, last_spread) {
                (Some(length), Some(last)) => fixed_end.min(length - last),
                _ => fixed_end,
            };
            return self.tuple_slice_element_type_ex(ty, start, end_skip, false, true);
        }
        if first_spread.is_none_or(|first| index < first) {
            if let Some(property) = self.property_type(
                ty,
                JsString::from_bytes(index.to_string().into_bytes()).as_bytes(),
            )? {
                return Ok(Some(property));
            }
        }
        self.iterated_type_or_element_type(
            crate::iteration::ALLOW_SYNC,
            ty,
            self.builtins.undefined_type,
            None,
            false,
        )
    }
}
