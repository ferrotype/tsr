//! Call argument expansion preserves tuple labels and the original spread's
//! source range. Synthetic arguments retain their source owner through the
//! checker factory and carry their checked type in the checker-owned map.
use crate::{
    access_flags as af, element_flags as ef, type_flags as tf, CheckerState, Error, InferenceId,
    TupleElementInfo, TypeId,
};
use ts_arena::NodeId;
use ts_ast::{Factory, SyntaxKind as K};

impl CheckerState {
    // port: tsc/internal/checker/checker.go:Checker.checkSpreadExpression
    pub(crate) fn check_spread_expression(
        &mut self,
        node: NodeId,
        mode: u32,
    ) -> Result<TypeId, Error> {
        let operand = self
            .ast(node)?
            .node(node)?
            .expression()
            .ok_or(Error::MissingLink("spread expression operand"))?;
        let ty = self.check_expression_ex(operand, mode)?;
        self.check_iterated_type_or_element_type(
            crate::iteration::SPREAD,
            ty,
            self.builtins.undefined_type,
            Some(operand),
        )
    }

    // port: tsc/internal/checker/checker.go:Checker.checkSyntheticExpression
    pub(crate) fn check_synthetic_expression(&mut self, node: NodeId) -> Result<TypeId, Error> {
        let ty = self.synthetic_argument_type(node)?;
        if self.is_spread_argument(node)? {
            self.get_indexed_access_type(ty, self.builtins.number_type, 0, None, None)
        } else {
            Ok(ty)
        }
    }

    // port: tsc/internal/checker/checker.go:isSpreadArgument
    pub(crate) fn is_spread_argument(&self, node: NodeId) -> Result<bool, Error> {
        let read = self.node(node)?;
        Ok(read.kind() == K::SpreadElement
            || read.kind() == K::SyntheticExpression
                && read
                    .data_source()
                    .as_synthetic_expression()
                    .ok_or(Error::MissingLink("synthetic call argument"))?
                    .is_spread())
    }

    // port: tsc/internal/checker/checker.go:Checker.getSpreadArgumentIndex
    pub(crate) fn spread_argument_index(&self, args: &[NodeId]) -> Result<Option<usize>, Error> {
        for (index, &node) in args.iter().enumerate() {
            if self.is_spread_argument(node)? {
                return Ok(Some(index));
            }
        }
        Ok(None)
    }

    // port: tsc/internal/checker/checker.go:Checker.createSyntheticExpression
    pub(crate) fn synthetic_call_argument(
        &mut self,
        parent: NodeId,
        ty: TypeId,
        spread: bool,
        label: Option<NodeId>,
    ) -> Result<NodeId, Error> {
        self.retain_flow_source(parent)?;
        if let Some(label) = label {
            self.retain_flow_source(label)?;
        }
        let range = self.node(parent)?.range();
        let node = self.new_synthetic_expression(ty, spread, label)?;
        self.factory.set_node_range(node, range);
        self.factory.set_node_parent(node, Some(parent));
        Ok(node)
    }

    // port: tsc/internal/checker/checker.go:Checker.getEffectiveCallArguments
    pub(crate) fn effective_call_arguments(&mut self, node: NodeId) -> Result<Vec<NodeId>, Error> {
        let read = self.node(node)?;
        if read.kind() == K::TaggedTemplateExpression {
            return self.tagged_template_arguments(node);
        }
        if read.kind() == K::BinaryExpression {
            return Ok(vec![read
                .data_source()
                .as_binary_expression()
                .ok_or(Error::MissingLink("instanceof binary expression"))?
                .left()
                .ok_or(Error::MissingLink("instanceof left operand"))?]);
        }
        let args = self.source_list(node, read.argument_list())?;
        let Some(start) = self.spread_argument_index(&args)? else {
            return Ok(args);
        };
        let mut expanded = args[..start].to_vec();
        for &argument in &args[start..] {
            let spread = if self.node(argument)?.kind() == K::SpreadElement {
                let operand = self
                    .ast(argument)?
                    .node(argument)?
                    .expression()
                    .ok_or(Error::MissingLink("spread operand"))?;
                Some(if self.flow.loop_stack.is_empty() {
                    self.check_expression_cached(operand)?
                } else {
                    self.check_expression(operand)?
                })
            } else {
                None
            };
            let tuple = match spread {
                Some(ty) if self.is_tuple_type(ty)? => Some(ty),
                _ => None,
            };
            if let Some(ty) = tuple {
                let elements = self.get_type_arguments(ty)?;
                let infos = self
                    .types
                    .tuple(self.types.target(ty)?)?
                    .element_infos
                    .clone();
                for (&ty, info) in elements.iter().zip(infos.iter()) {
                    let ty = if info.flags & ef::REST != 0 {
                        self.create_array_type(ty, false)?
                    } else {
                        ty
                    };
                    expanded.push(self.synthetic_call_argument(
                        argument,
                        ty,
                        info.flags & ef::VARIABLE != 0,
                        info.labeled_declaration,
                    )?);
                }
            } else {
                expanded.push(argument);
            }
        }
        Ok(expanded)
    }

    // port: tsc/internal/checker/checker.go:Checker.getSpreadArgumentType
    pub(crate) fn spread_argument_type(
        &mut self,
        args: &[NodeId],
        start: usize,
        count: usize,
        rest: TypeId,
        context: Option<InferenceId>,
        mode: u32,
    ) -> Result<TypeId, Error> {
        let in_const = self.is_const_type_variable(rest, 0)?;
        if count > 0 && start >= count - 1 {
            let argument = args[count - 1];
            if self.is_spread_argument(argument)? {
                let synthetic = self.node(argument)?.kind() == K::SyntheticExpression;
                let operand = if synthetic {
                    argument
                } else {
                    self.node(argument)?
                        .expression()
                        .ok_or(Error::MissingLink("last spread operand"))?
                };
                let ty = if synthetic {
                    self.synthetic_argument_type(argument)?
                } else {
                    self.check_call_argument_ex(operand, rest, context, mode)?
                };
                if self.is_array_like_type(ty)? {
                    return self.mutable_array_or_tuple_type(ty);
                }
                let element = self.check_iterated_type_or_element_type(
                    crate::iteration::SPREAD,
                    ty,
                    self.builtins.undefined_type,
                    Some(operand),
                )?;
                return self.create_array_type(element, in_const);
            }
        }
        let mut types = Vec::new();
        let mut infos = Vec::new();
        // Upstream's `for i := index; i < argCount; i++` simply does not run when a
        // rest parameter sits past the argument list, so the window is empty here
        // rather than an out-of-range slice.
        for (offset, &argument) in args
            .get(start..count)
            .unwrap_or_default()
            .iter()
            .enumerate()
        {
            let index = start + offset;
            let synthetic = self.node(argument)?.kind() == K::SyntheticExpression;
            let (ty, flags) = if self.is_spread_argument(argument)? {
                let operand = if synthetic {
                    argument
                } else {
                    self.node(argument)?
                        .expression()
                        .ok_or(Error::MissingLink("spread argument operand"))?
                };
                let ty = if synthetic {
                    self.synthetic_argument_type(argument)?
                } else {
                    self.check_expression(operand)?
                };
                if self.is_array_like_type(ty)? {
                    (ty, ef::VARIADIC)
                } else {
                    (
                        self.check_iterated_type_or_element_type(
                            crate::iteration::SPREAD,
                            ty,
                            self.builtins.undefined_type,
                            Some(operand),
                        )?,
                        ef::REST,
                    )
                }
            } else {
                let contextual = if self.is_tuple_type(rest)? {
                    self.contextual_element_type(
                        rest,
                        index - start,
                        Some(count - start),
                        None,
                        None,
                    )?
                    .unwrap_or(self.builtins.unknown_type)
                } else {
                    let key = self
                        .get_number_literal_type(ts_jsnum::Number::new((index - start) as f64))?;
                    self.get_indexed_access_type(rest, key, af::CONTEXTUAL, None, None)?
                };
                let ty = self.check_call_argument_ex(argument, contextual, context, mode)?;
                let primitive = in_const
                    || self.maybe_type_of_kind(
                        contextual,
                        tf::PRIMITIVE | tf::INDEX | tf::TEMPLATE_LITERAL | tf::STRING_MAPPING,
                    )?;
                (
                    if primitive {
                        self.get_regular_type_of_literal_type(ty)?
                    } else {
                        self.widen_literal_type(ty)?
                    },
                    ef::REQUIRED,
                )
            };
            let label = if synthetic {
                self.node(argument)?
                    .data_source()
                    .as_synthetic_expression()
                    .ok_or(Error::MissingLink("synthetic tuple label"))?
                    .tuple_name_source()
            } else {
                None
            };
            types.push(ty);
            infos.push(TupleElementInfo {
                flags,
                labeled_declaration: label,
            });
        }
        let readonly = in_const && !self.some_mutable_array_like(rest)?;
        self.create_tuple_type_ex(&types, &infos, readonly)
    }

    fn synthetic_argument_type(&self, node: NodeId) -> Result<TypeId, Error> {
        self.synthetic_expression_types
            .get(&node)
            .copied()
            .ok_or(Error::MissingLink("synthetic argument type"))
    }

    // port: tsc/internal/checker/checker.go:Checker.getMutableArrayOrTupleType
    fn mutable_array_or_tuple_type(&mut self, ty: TypeId) -> Result<TypeId, Error> {
        if self.types.flags(ty)? & tf::UNION != 0 {
            return self
                .map_type(ty, &mut |state, part| {
                    state.mutable_array_or_tuple_type(part).map(Some)
                })?
                .ok_or(Error::MissingLink("mutable array union"));
        }
        if self.types.flags(ty)? & tf::ANY != 0 {
            return Ok(ty);
        }
        let constraint = self.base_constraint_of_type(ty)?.unwrap_or(ty);
        if (self.is_array_type(constraint)? || self.is_tuple_type(constraint)?)
            && !self.readonly_array_or_tuple(constraint)?
        {
            return Ok(ty);
        }
        if self.is_tuple_type(ty)? {
            let elements = self.get_type_arguments(ty)?;
            let infos = self
                .types
                .tuple(self.types.target(ty)?)?
                .element_infos
                .clone();
            return self.create_tuple_type_ex(&elements, &infos, false);
        }
        self.create_tuple_type_ex(
            &[ty],
            &[TupleElementInfo {
                flags: ef::VARIADIC,
                labeled_declaration: None,
            }],
            false,
        )
    }
}
