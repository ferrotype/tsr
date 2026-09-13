//! Discriminant narrowing shares P3's property and constituent caches.
use crate::{type_facts as f, type_flags as tf, CheckerState, Error, TypeId};
use ts_arena::NodeId;
use ts_ast::{node_flags as nf, SyntaxKind as K};
fn required<T>(value: Option<T>, context: &'static str) -> Result<T, Error> {
    value.ok_or(Error::MissingLink(context))
}
impl CheckerState {
    // port: tsc/internal/checker/flow.go:Checker.getDiscriminantPropertyAccess
    pub(crate) fn flow_discriminant_access(
        &mut self,
        reference: NodeId,
        declared: TypeId,
        expr: NodeId,
        computed: TypeId,
    ) -> Result<Option<NodeId>, Error> {
        if self.types.flags(declared)? & tf::UNION == 0
            && self.types.flags(computed)? & tf::UNION == 0
        {
            return Ok(None);
        }
        let access = self.candidate_discriminant_access(reference, expr)?;
        let Some(access) = access else {
            return Ok(None);
        };
        let Some(name) = self.flow_property_name(access)? else {
            return Ok(None);
        };
        let ty = if self.types.flags(declared)? & tf::UNION != 0
            && self.flow_type_subset(computed, declared)?
        {
            declared
        } else {
            computed
        };
        Ok(self
            .discriminant_property(ty, name.as_bytes())?
            .then_some(access))
    }

    // port: tsc/internal/checker/flow.go:Checker.getCandidateDiscriminantPropertyAccess
    fn candidate_discriminant_access(
        &mut self,
        reference: NodeId,
        expr: NodeId,
    ) -> Result<Option<NodeId>, Error> {
        let kind = self.ast(reference)?.node(reference)?.kind();
        let pseudo = matches!(
            kind.known(),
            Some(
                K::ObjectBindingPattern
                    | K::ArrayBindingPattern
                    | K::FunctionExpression
                    | K::ArrowFunction
            )
        ) || ts_ast::utilities::is_object_literal_method(
            self.ast(reference)?,
            Some(reference),
        )?;
        let read = self.ast(expr)?.node(expr)?;
        if pseudo {
            if read.kind() == K::Identifier {
                let mut symbol = self.resolved_value_symbol(expr)?;
                if self.symbol(symbol)?.flags() & ts_ast::symbol_flags::EXPORT_VALUE != 0 {
                    symbol = self.symbol(symbol)?.export_symbol().unwrap_or(symbol);
                }
                if let Some(declaration) = self.symbol(symbol)?.value_declaration() {
                    let read = self.ast(declaration)?.node(declaration)?;
                    let rest = match read.kind().known() {
                        Some(K::BindingElement) => read
                            .data_source()
                            .as_binding_element()
                            .ok_or(ts_arena::Error::InvalidGraph)?
                            .dot_dot_dot_token()
                            .is_some(),
                        Some(K::Parameter) => read
                            .data_source()
                            .as_parameter_declaration()
                            .ok_or(ts_arena::Error::InvalidGraph)?
                            .dot_dot_dot_token()
                            .is_some(),
                        _ => return Ok(None),
                    };
                    if read.parent() == Some(reference) && read.initializer().is_none() && !rest {
                        return Ok(Some(declaration));
                    }
                }
            }
        } else if matches!(
            read.kind().known(),
            Some(K::PropertyAccessExpression | K::ElementAccessExpression)
        ) {
            let expression = required(read.expression(), "discriminant object")?;
            if self.matching_reference(reference, expression)? {
                return Ok(Some(expr));
            }
        } else if read.kind() == K::Identifier {
            let symbol = self.resolved_value_symbol(expr)?;
            if self.is_constant_flow_variable(symbol)? {
                if let Some(declaration) = self.symbol(symbol)?.value_declaration() {
                    if let Some(initializer) =
                        self.candidate_discriminant_initializer(declaration)?
                    {
                        let read = self.ast(initializer)?.node(initializer)?;
                        if matches!(
                            read.kind().known(),
                            Some(K::PropertyAccessExpression | K::ElementAccessExpression)
                        ) {
                            let expression =
                                required(read.expression(), "discriminant initializer object")?;
                            if self.matching_reference(reference, expression)? {
                                return Ok(Some(initializer));
                            }
                        }
                    }
                    let read = self.ast(declaration)?.node(declaration)?;
                    if read.kind() == K::BindingElement && read.initializer().is_none() {
                        let pattern = required(read.parent(), "discriminant binding pattern")?;
                        let variable = required(
                            self.ast(pattern)?.node(pattern)?.parent(),
                            "discriminant binding variable",
                        )?;
                        if let Some(initializer) =
                            self.candidate_discriminant_initializer(variable)?
                        {
                            if matches!(
                                self.ast(initializer)?.node(initializer)?.kind().known(),
                                Some(
                                    K::Identifier
                                        | K::PropertyAccessExpression
                                        | K::ElementAccessExpression
                                )
                            ) && self.matching_reference(reference, initializer)?
                            {
                                return Ok(Some(declaration));
                            }
                        }
                    }
                }
            }
        }
        Ok(None)
    }
    // port: tsc/internal/checker/flow.go:getCandidateVariableDeclarationInitializer
    fn candidate_discriminant_initializer(&self, node: NodeId) -> Result<Option<NodeId>, Error> {
        let read = self.ast(node)?.node(node)?;
        if read.kind() == K::VariableDeclaration && read.type_node().is_none() {
            if let Some(initializer) = read.initializer() {
                return Ok(Some(ts_ast::skip_parentheses(
                    self.ast(initializer)?,
                    initializer,
                )?));
            }
        }
        Ok(None)
    }

    // port: tsc/internal/checker/flow.go:Checker.narrowTypeByDiscriminant
    pub(crate) fn narrow_discriminant(
        &mut self,
        ty: TypeId,
        access: NodeId,
        narrow: &mut dyn FnMut(&mut Self, TypeId) -> Result<TypeId, Error>,
    ) -> Result<TypeId, Error> {
        let Some(name) = self.flow_property_name(access)? else {
            return Ok(ty);
        };
        let read = self.ast(access)?.node(access)?;
        let optional_chain = read.flags() & nf::OPTIONAL_CHAIN != 0;
        let nonnull = if matches!(
            read.kind().known(),
            Some(K::PropertyAccessExpression | K::ElementAccessExpression)
        ) {
            match read.expression() {
                Some(expr) => self.ast(expr)?.node(expr)?.kind() == K::NonNullExpression,
                None => false,
            }
        } else {
            false
        };
        let remove_nullable = (nonnull || optional_chain)
            && self.options.strict_null_checks
            && self.maybe_type_of_kind(ty, tf::NULLABLE)?;
        let nonnull_ty = if remove_nullable {
            self.type_with_facts(ty, f::NE_UNDEFINED_OR_NULL)?
        } else {
            ty
        };
        let Some(property) = self.constituent_property(nonnull_ty, name.as_bytes(), false)? else {
            return Ok(ty);
        };
        let mut property_type = self.get_type_of_symbol(property)?;
        if remove_nullable && optional_chain {
            property_type = self.add_type_optionality(property_type, false, true)?;
        }
        let narrowed = narrow(self, property_type)?;
        self.filter_type(ty, &mut |state, part| {
            let discriminant = state
                .property_or_index_type(part, name.as_bytes())?
                .unwrap_or(state.builtins.unknown_type);
            Ok(state.types.flags(discriminant)? & tf::NEVER == 0
                && state.types.flags(narrowed)? & tf::NEVER == 0
                && state.types_comparable(narrowed, discriminant)?)
        })
    }

    // port: tsc/internal/checker/flow.go:Checker.narrowTypeByDiscriminantProperty
    pub(crate) fn narrow_discriminant_property(
        &mut self,
        ty: TypeId,
        access: NodeId,
        operator: ts_ast::NodeKind,
        value: NodeId,
        assume: bool,
    ) -> Result<TypeId, Error> {
        if matches!(
            operator.known(),
            Some(K::EqualsEqualsEqualsToken | K::ExclamationEqualsEqualsToken)
        ) && self.types.flags(ty)? & tf::UNION != 0
        {
            let name = self.flow_key_property_name(ty)?;
            if !name.is_empty() && self.flow_property_name(access)?.as_ref() == Some(&name) {
                let value_type = self.get_type_of_expression(value)?;
                if let Some(candidate) = self.flow_constituent_for_key(ty, value_type)? {
                    if assume == (operator == K::EqualsEqualsEqualsToken) {
                        return Ok(candidate);
                    }
                    if let Some(property) =
                        self.constituent_property(candidate, name.as_bytes(), false)?
                    {
                        let property_type = self.get_type_of_symbol(property)?;
                        if self.types.flags(property_type)? & tf::UNIT != 0 {
                            return self.flow_remove_type(ty, candidate);
                        }
                    }
                    return Ok(ty);
                }
            }
        }
        self.narrow_discriminant(ty, access, &mut |state, part| {
            state.narrow_equality(part, operator, value, assume)
        })
    }

    // port: tsc/internal/checker/relater.go:Checker.getKeyPropertyName
    pub(crate) fn flow_key_property_name(&mut self, ty: TypeId) -> Result<ts_ast::JsString, Error> {
        if self.types.union(ty)?.key_property_name.is_none() {
            self.compute_key_property_name(ty)?;
        }
        Ok(self
            .types
            .union(ty)?
            .key_property_name
            .clone()
            .unwrap_or_default())
    }
    // port: tsc/internal/checker/relater.go:Checker.getConstituentTypeForKeyType
    pub(crate) fn flow_constituent_for_key(
        &mut self,
        ty: TypeId,
        key: TypeId,
    ) -> Result<Option<TypeId>, Error> {
        let key = self.get_regular_type_of_literal_type(key)?;
        Ok(self
            .types
            .union(ty)?
            .constituent_map
            .as_ref()
            .and_then(|map| map.get(&key))
            .copied()
            .filter(|&part| part != self.builtins.unknown_type))
    }
}
