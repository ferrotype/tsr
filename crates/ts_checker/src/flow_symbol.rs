//! Correlated destructuring and contextual-rest parameters use their shared
//! declaration as a pseudo-reference at the occurrence's flow position.
use crate::{signature_flags as sg, type_flags as tf, CheckerState, Error, TypeId};
use ts_arena::{NodeId, SymbolId};
use ts_ast::{node_flags as nf, SyntaxKind as K};

fn required<T>(value: Option<T>, name: &'static str) -> Result<T, Error> {
    value.ok_or(Error::MissingLink(name))
}

impl CheckerState {
    // port: tsc/internal/checker/checker.go:Checker.getNarrowedTypeOfSymbol
    pub(crate) fn narrowed_type_of_symbol(
        &mut self,
        symbol: SymbolId,
        location: NodeId,
    ) -> Result<TypeId, Error> {
        let mut ty = self.get_type_of_symbol(symbol)?;
        let Some(declaration) = self.symbol(symbol)?.value_declaration() else {
            return Ok(ty);
        };
        let read = self.ast(declaration)?.node(declaration)?;
        let kind = read.kind();
        if !matches!(kind.known(), Some(K::BindingElement | K::Parameter)) {
            return Ok(ty);
        }
        let initializer = read.initializer();
        let annotation = read.type_node();
        let parent = required(read.parent(), "narrowed declaration parent")?;
        if kind == K::BindingElement
            && initializer.is_none()
            && !self.binding_is_rest(declaration)?
            && self
                .source_list(parent, self.ast(parent)?.node(parent)?.element_list())?
                .len()
                >= 2
        {
            let root =
                ts_ast::utilities::get_root_declaration(self.ast(declaration)?, declaration)?;
            if let Some(initializer) = self.ast(root)?.node(root)?.initializer() {
                if ts_ast::utilities::is_node_descendant_of(
                    self.ast(location)?,
                    Some(location),
                    Some(initializer),
                )? && self.control_flow_container(declaration)?
                    == self.control_flow_container(location)?
                {
                    return Ok(ty);
                }
            }
            let read = self.ast(root)?.node(root)?;
            let parameter = read.kind() == K::Parameter;
            let constant = read.kind() == K::VariableDeclaration
                && ts_ast::utilities::get_combined_node_flags(self.ast(root)?, root)?
                    & nf::CONSTANT
                    != 0;
            if constant || parameter {
                let binding_parent = required(
                    self.ast(parent)?.node(parent)?.parent(),
                    "narrowed binding parent",
                )?;
                if self.flow.symbol_narrowing_parents.insert(binding_parent) {
                    // Only parent resolution is guarded: the native flow walk
                    // intentionally permits reentrant correlated narrowing.
                    let parent_type = (|| {
                        let Some(parent_type) =
                            self.type_for_binding_element_parent(binding_parent, 0)?
                        else {
                            return Ok(None);
                        };
                        self.map_type(parent_type, &mut |checker, part| {
                            Ok(Some(checker.base_constraint_of_type(part)?.unwrap_or(part)))
                        })
                    })();
                    self.flow.symbol_narrowing_parents.remove(&binding_parent);
                    if let Some(parent_type) = parent_type? {
                        if self.types.flags(parent_type)? & tf::UNION != 0
                            && !(parameter && self.some_binding_symbol_assigned(root)?)
                        {
                            let flow = self.node_flow(location)?;
                            let narrowed = self.flow_type_of_reference_at_flow(
                                parent,
                                parent_type,
                                parent_type,
                                None,
                                location,
                                flow,
                            )?;
                            ty = if self.types.flags(narrowed)? & tf::NEVER != 0 {
                                self.builtins.never_type
                            } else {
                                self.binding_element_type_from_parent(declaration, narrowed, true)?
                            };
                        }
                    }
                }
            }
        } else if kind == K::Parameter
            && annotation.is_none()
            && initializer.is_none()
            && !self.binding_is_rest(declaration)?
        {
            let parameters =
                self.source_list(parent, self.ast(parent)?.node(parent)?.parameter_list())?;
            let kind = self.ast(parent)?.node(parent)?.kind();
            let object_method = kind == K::MethodDeclaration
                && self
                    .ast(parent)?
                    .node(parent)?
                    .parent()
                    .map(|owner| {
                        self.ast(owner)?
                            .node(owner)
                            .map(|read| read.kind() == K::ObjectLiteralExpression)
                            .map_err(Error::from)
                    })
                    .transpose()?
                    .unwrap_or(false);
            if parameters.len() >= 2
                && (matches!(kind.known(), Some(K::FunctionExpression | K::ArrowFunction))
                    || object_method)
                && self.expression_is_context_sensitive(parent)?
            {
                if let Some(signature) = self.contextual_body_signature(parent)? {
                    let data = self.signatures.get(signature)?;
                    let context_parameters = data.parameters.as_deref().unwrap_or_default();
                    if context_parameters.len() == 1 && data.flags & sg::HAS_REST_PARAMETER != 0 {
                        let rest_symbol = context_parameters[0];
                        let mapper = self
                            .call_inference_at_node(parent)?
                            .map(|context| {
                                self.inference_context(context)
                                    .map(|context| context.non_fixing_mapper)
                            })
                            .transpose()?;
                        let rest = self.get_type_of_symbol(rest_symbol)?;
                        let rest = self.instantiate_type(rest, mapper)?;
                        let rest = self.reduced_apparent_type(rest)?;
                        if self.types.flags(rest)? & tf::UNION != 0 {
                            let mut tuples = true;
                            for part in self.types.compound_types(rest)?.to_vec() {
                                tuples &= self.is_tuple_type(part)?;
                            }
                            if tuples {
                                let mut assigned = false;
                                for &parameter in &parameters {
                                    if self.some_binding_symbol_assigned(parameter)? {
                                        assigned = true;
                                        break;
                                    }
                                }
                                if !assigned {
                                    let flow = self.node_flow(location)?;
                                    let narrowed = self.flow_type_of_reference_at_flow(
                                        parent, rest, rest, None, location, flow,
                                    )?;
                                    let position = parameters
                                        .iter()
                                        .position(|&parameter| parameter == declaration)
                                        .ok_or(ts_arena::Error::InvalidGraph)?;
                                    let this_parameter = match parameters.first().copied() {
                                        Some(parameter) => {
                                            match self.ast(parameter)?.node(parameter)?.name() {
                                                Some(name) => {
                                                    self.ast(name)?.node_text(name)?.as_bytes()
                                                        == b"this"
                                                }
                                                None => false,
                                            }
                                        }
                                        None => false,
                                    };
                                    let index = isize::try_from(position)
                                        .map_err(|_| ts_arena::Error::InvalidGraph)?
                                        - isize::from(this_parameter);
                                    let index = self.get_number_literal_type(
                                        ts_jsnum::Number::new(index as f64),
                                    )?;
                                    ty = self
                                        .get_indexed_access_type(narrowed, index, 0, None, None)?;
                                }
                            }
                        }
                    }
                }
            }
        }
        Ok(ty)
    }
}
