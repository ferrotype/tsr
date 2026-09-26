use crate::{mapper::Mapper, CheckerState, Error, TypeId};
use tsr_ast::SyntaxKind as K;

impl CheckerState {
    // port: tsc/internal/checker/checker.go:Checker.getInferredTypeParameterConstraint
    pub(crate) fn inferred_parameter_constraint(
        &mut self,
        ty: TypeId,
        omit_references: bool,
    ) -> Result<Option<TypeId>, Error> {
        let Some(symbol) = self.types.get(ty)?.symbol else {
            return Ok(None);
        };
        let mut inferences = Vec::new();
        for declaration in self
            .symbol_declarations(symbol)?
            .to_vec()
            .into_iter()
            .flatten()
        {
            let Some(infer) = self.node(declaration)?.parent() else {
                continue;
            };
            if self.node(infer)?.kind() != K::InferType {
                continue;
            }
            let mut child = infer;
            let mut parent = self.node(child)?.parent();
            while let Some(node) = parent {
                if self.node(node)?.kind() != K::ParenthesizedType {
                    break;
                }
                child = node;
                parent = self.node(node)?.parent();
            }
            let Some(parent) = parent else { continue };
            let read = self.node(parent)?;
            let kind = read.kind();
            if kind == K::TypeReference && !omit_references {
                let referenced = self.get_type_from_type_node(parent)?;
                if self.is_error_type(referenced)? {
                    continue;
                }
                let Some(symbol) = self.type_reference_symbol(parent, true)? else {
                    continue;
                };
                let parameters = self.type_parameters_for_type_and_symbol(referenced, symbol)?;
                let arguments =
                    self.source_list(parent, self.node(parent)?.type_argument_list())?;
                if let Some(index) = arguments
                    .iter()
                    .position(|&node| node == child)
                    .filter(|&index| index < parameters.len())
                {
                    if let Some(constraint) =
                        self.constraint_of_type_parameter(parameters[index])?
                    {
                        let mapper = self.alloc_mapper(Mapper::DeferredArguments {
                            node: parent,
                            sources: parameters,
                        })?;
                        let constraint = self.instantiate_type(constraint, Some(mapper))?;
                        if constraint != ty {
                            inferences.push(constraint);
                        }
                    }
                }
            } else if kind == K::RestType
                || kind == K::Parameter
                    && read
                        .data_source()
                        .as_parameter_declaration()
                        .is_some_and(|p| p.dot_dot_dot_token().is_some())
                || kind == K::NamedTupleMember
                    && read
                        .data_source()
                        .as_named_tuple_member()
                        .is_some_and(|p| p.dot_dot_dot_token().is_some())
            {
                inferences.push(self.create_array_type(self.builtins.unknown_type, false)?);
            } else if kind == K::TemplateLiteralTypeSpan {
                inferences.push(self.builtins.string_type);
            } else if kind == K::TypeParameter {
                if let Some(node) = read.parent() {
                    if self.node(node)?.kind() == K::MappedType {
                        inferences.push(self.builtins.string_number_symbol_type);
                    }
                }
            } else if kind == K::MappedType {
                let Some(mut template) = read.type_node() else {
                    continue;
                };
                while self.node(template)?.kind() == K::ParenthesizedType {
                    template = self
                        .ast(template)?
                        .node(template)?
                        .type_node()
                        .ok_or(Error::MissingLink("parenthesized infer type"))?;
                }
                if template != infer {
                    continue;
                }
                let Some(conditional) = read.parent() else {
                    continue;
                };
                let conditional_read = self.node(conditional)?;
                let Some(data) = conditional_read.data_source().as_conditional_type_node() else {
                    continue;
                };
                if data.extends_type() != Some(parent) {
                    continue;
                }
                let check = data
                    .check_type()
                    .ok_or(Error::MissingLink("conditional check"))?;
                if self.node(check)?.kind() != K::MappedType {
                    continue;
                }
                let Some(template) = self.node(check)?.type_node() else {
                    continue;
                };
                let template = self.get_type_from_type_node(template)?;
                let mapped = self.source_mapped_type(check)?;
                let parameter = self.mapped_parameter(mapped)?;
                let constraint = self.mapped_constraint(mapped)?;
                let mapper = self.new_type_mapper(&[parameter], &[constraint])?;
                inferences.push(self.instantiate_type(template, Some(mapper))?);
            }
        }
        if inferences.is_empty() {
            Ok(None)
        } else {
            self.get_intersection_type(&inferences).map(Some)
        }
    }
}
