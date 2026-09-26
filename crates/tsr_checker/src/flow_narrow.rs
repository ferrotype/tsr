//! Predicate narrowing. Reference identities come from symbol resolution;
//! spelling equality alone never identifies a variable.

use crate::{type_facts as f, type_flags as tf, CheckerState, Error, RelationKind, TypeId};
use tsr_arena::NodeId;
use tsr_ast::SyntaxKind as K;

impl CheckerState {
    // port: tsc/internal/checker/flow.go:Checker.narrowType
    pub(crate) fn narrow_reference_type(
        &mut self,
        reference: NodeId,
        declared: TypeId,
        ty: TypeId,
        expression: NodeId,
        assume: bool,
    ) -> Result<TypeId, Error> {
        stacker::maybe_grow(128 * 1024, 2 * 1024 * 1024, || {
            self.narrow_reference_worker(reference, declared, ty, expression, assume)
        })
    }

    fn narrow_reference_worker(
        &mut self,
        reference: NodeId,
        declared: TypeId,
        mut ty: TypeId,
        expression: NodeId,
        assume: bool,
    ) -> Result<TypeId, Error> {
        let read = self.node(expression)?;
        let optionality = if let Some(parent) = read.parent() {
            let parent = self.node(parent)?;
            let coalescing = if let Some(binary) = parent.data_source().as_binary_expression() {
                let op = binary
                    .operator_token()
                    .ok_or(Error::MissingLink("coalescing operator"))?;
                binary.left() == Some(expression)
                    && matches!(
                        self.node(op)?.kind().known(),
                        Some(K::QuestionQuestionToken | K::QuestionQuestionEqualsToken)
                    )
            } else {
                false
            };
            coalescing
                || tsr_ast::utilities::is_expression_of_optional_chain_root(
                    self.ast(expression)?,
                    expression,
                )?
        } else {
            false
        };
        // port: tsc/internal/checker/flow.go:Checker.narrowTypeByOptionality
        if optionality {
            let facts = if assume {
                f::NE_UNDEFINED_OR_NULL
            } else {
                f::EQ_UNDEFINED_OR_NULL
            };
            if self.matching_reference(reference, expression)? {
                return self.adjusted_type_with_facts(ty, facts);
            }
            if let Some(access) =
                self.flow_discriminant_access(reference, declared, expression, ty)?
            {
                return self.narrow_discriminant(ty, access, &mut |state, part| {
                    state.type_with_facts(part, facts)
                });
            }
            return Ok(ty);
        }
        match read.kind().known() {
            Some(
                K::Identifier
                | K::ThisKeyword
                | K::SuperKeyword
                | K::PropertyAccessExpression
                | K::ElementAccessExpression,
            ) => {
                if read.kind() == K::Identifier
                    && self.flow.inline_level < 5
                    && !self.matching_reference(reference, expression)?
                {
                    let symbol = self.resolved_value_symbol(expression)?;
                    if self.is_constant_flow_variable(symbol)? {
                        if let Some(declaration) = self.symbol(symbol)?.value_declaration() {
                            let read = self.node(declaration)?;
                            if read.kind() == K::VariableDeclaration && read.type_node().is_none() {
                                if let Some(initializer) = read.initializer() {
                                    if self.constant_flow_reference(reference)? {
                                        self.flow.inline_level += 1;
                                        let result = self.narrow_reference_type(
                                            reference,
                                            declared,
                                            ty,
                                            initializer,
                                            assume,
                                        );
                                        self.flow.inline_level -= 1;
                                        return result;
                                    }
                                }
                            }
                        }
                    }
                }
                return self.narrow_flow_truthiness(reference, declared, ty, expression, assume);
            }
            Some(K::ParenthesizedExpression | K::NonNullExpression | K::SatisfiesExpression) => {
                let operand = read
                    .expression()
                    .ok_or(Error::MissingLink("narrowing operand"))?;
                return self.narrow_reference_type(reference, declared, ty, operand, assume);
            }
            Some(K::PrefixUnaryExpression) => {
                let data = read
                    .data_source()
                    .as_prefix_unary_expression()
                    .ok_or(tsr_arena::Error::InvalidGraph)?;
                if data.operator() == K::ExclamationToken {
                    return self.narrow_reference_type(
                        reference,
                        declared,
                        ty,
                        data.operand()
                            .ok_or(Error::MissingLink("narrowing operand"))?,
                        !assume,
                    );
                }
            }
            // port: tsc/internal/checker/flow.go:Checker.narrowTypeByBinaryExpression
            Some(K::BinaryExpression) => {
                let data = read
                    .data_source()
                    .as_binary_expression()
                    .ok_or(tsr_arena::Error::InvalidGraph)?;
                let left = data.left().ok_or(Error::MissingLink("binary left"))?;
                let right = data.right().ok_or(Error::MissingLink("binary right"))?;
                let op = data
                    .operator_token()
                    .ok_or(Error::MissingLink("binary operator"))?;
                let operator = self.node(op)?.kind();
                match operator.known() {
                    Some(
                        K::EqualsEqualsToken
                        | K::ExclamationEqualsToken
                        | K::EqualsEqualsEqualsToken
                        | K::ExclamationEqualsEqualsToken,
                    ) => {
                        for (candidate, value) in [(left, right), (right, left)] {
                            let candidate = self.reference_candidate(candidate)?;
                            let value = self.reference_candidate(value)?;
                            let read = self.node(candidate)?;
                            // port: tsc/internal/checker/flow.go:Checker.narrowTypeByTypeof
                            if read.kind() == K::TypeOfExpression
                                && matches!(
                                    self.node(value)?.kind().known(),
                                    Some(K::StringLiteral | K::NoSubstitutionTemplateLiteral)
                                )
                            {
                                let inner = self.reference_candidate(
                                    read.expression()
                                        .ok_or(Error::MissingLink("typeof operand"))?,
                                )?;
                                let text = self.node_text(value)?.into_js_string();
                                let equal = assume
                                    != matches!(
                                        operator.known(),
                                        Some(
                                            K::ExclamationEqualsToken
                                                | K::ExclamationEqualsEqualsToken
                                        )
                                    );
                                if self.matching_reference(reference, inner)? {
                                    return self.narrow_typeof(ty, text.as_bytes(), equal);
                                }
                                if self.options.strict_null_checks
                                    && self.optional_chain_contains_reference(inner, reference)?
                                    && equal == (text.as_bytes() != b"undefined")
                                {
                                    ty =
                                        self.adjusted_type_with_facts(ty, f::NE_UNDEFINED_OR_NULL)?;
                                }
                                if let Some(access) =
                                    self.flow_discriminant_access(reference, declared, inner, ty)?
                                {
                                    return self.narrow_discriminant(
                                        ty,
                                        access,
                                        &mut |state, part| {
                                            state.narrow_typeof(part, text.as_bytes(), equal)
                                        },
                                    );
                                }
                                return Ok(ty);
                            }
                            if self.matching_reference(reference, candidate)? {
                                return self.narrow_equality(ty, operator, value, assume);
                            }
                        }
                        let left = self.reference_candidate(left)?;
                        let right = self.reference_candidate(right)?;
                        if self.options.strict_null_checks {
                            if self.optional_chain_contains_reference(left, reference)? {
                                ty = self.narrow_optional_chain_containment(
                                    ty, operator, right, assume,
                                )?;
                            } else if self.optional_chain_contains_reference(right, reference)? {
                                ty = self.narrow_optional_chain_containment(
                                    ty, operator, left, assume,
                                )?;
                            }
                        }
                        for (candidate, value) in [(left, right), (right, left)] {
                            if let Some(access) =
                                self.flow_discriminant_access(reference, declared, candidate, ty)?
                            {
                                return self.narrow_discriminant_property(
                                    ty, access, operator, value, assume,
                                );
                            }
                        }
                        for (expr, value) in [(left, right), (right, left)] {
                            if matches!(
                                self.node(value)?.kind().known(),
                                // port: tsc/internal/checker/flow.go:Checker.narrowTypeByBooleanComparison
                                Some(K::TrueKeyword | K::FalseKeyword)
                            ) && !matches!(
                                self.node(expr)?.kind().known(),
                                Some(K::PropertyAccessExpression | K::ElementAccessExpression)
                            ) {
                                let boolean = self.node(value)?.kind() == K::TrueKeyword;
                                let equal = !matches!(
                                    operator.known(),
                                    Some(
                                        K::ExclamationEqualsToken | K::ExclamationEqualsEqualsToken
                                    )
                                );
                                return self.narrow_reference_type(
                                    reference,
                                    declared,
                                    ty,
                                    expr,
                                    (assume != boolean) != equal,
                                );
                            }
                            if self.node(value)?.kind() == K::Identifier
                                && self.matching_constructor_reference(reference, expr)?
                            {
                                return self.narrow_flow_constructor(ty, operator, value, assume);
                            }
                        }
                    }
                    Some(K::AmpersandAmpersandToken | K::BarBarToken) => {
                        let conjunction = operator == K::AmpersandAmpersandToken;
                        let left_type =
                            self.narrow_reference_type(reference, declared, ty, left, assume)?;
                        if conjunction == assume {
                            return self.narrow_reference_type(
                                reference, declared, left_type, right, assume,
                            );
                        }
                        let right_type =
                            self.narrow_reference_type(reference, declared, ty, right, assume)?;
                        return self.get_union_type(&[left_type, right_type]);
                    }
                    Some(K::CommaToken) => {
                        return self.narrow_reference_type(reference, declared, ty, right, assume)
                    }
                    Some(
                        K::EqualsToken
                        | K::BarBarEqualsToken
                        | K::AmpersandAmpersandEqualsToken
                        | K::QuestionQuestionEqualsToken,
                    ) => {
                        let ty =
                            self.narrow_reference_type(reference, declared, ty, right, assume)?;
                        return self.narrow_flow_truthiness(reference, declared, ty, left, assume);
                    }
                    Some(K::InKeyword) => {
                        if self.node(left)?.kind() == K::PrivateIdentifier {
                            return self.narrow_flow_private_in(reference, ty, left, right, assume);
                        }
                        let target = self.reference_candidate(right)?;
                        if self.type_contains_missing(ty)?
                            && matches!(
                                self.node(reference)?.kind().known(),
                                Some(K::PropertyAccessExpression | K::ElementAccessExpression)
                            )
                        {
                            let expression = self
                                .ast(reference)?
                                .node(reference)?
                                .expression()
                                .ok_or(Error::MissingLink("in reference expression"))?;
                            if self.matching_reference(expression, target)? {
                                let key_type = self.get_type_of_expression(left)?;
                                if let Some(name) = self.index_property_name(key_type)? {
                                    if self.flow_property_name(reference)?.as_ref() == Some(&name) {
                                        return self.type_with_facts(
                                            ty,
                                            if assume {
                                                f::NE_UNDEFINED
                                            } else {
                                                f::EQ_UNDEFINED
                                            },
                                        );
                                    }
                                }
                            }
                        }
                        if self.matching_reference(reference, target)? {
                            let key_type = self.get_type_of_expression(left)?;
                            if self.index_property_name(key_type)?.is_some() {
                                return self.narrow_flow_in(ty, key_type, assume);
                            }
                        }
                    }
                    Some(K::InstanceOfKeyword) => {
                        return self.narrow_flow_instanceof(
                            reference, ty, expression, left, right, assume,
                        );
                    }
                    _ => {}
                }
            }
            Some(K::CallExpression) => {
                return self.narrow_flow_call(reference, declared, ty, expression, assume)
            }
            _ => {}
        }
        Ok(ty)
    }

    // port: tsc/internal/checker/flow.go:Checker.narrowTypeByTruthiness
    fn narrow_flow_truthiness(
        &mut self,
        reference: NodeId,
        declared: TypeId,
        mut ty: TypeId,
        expr: NodeId,
        assume: bool,
    ) -> Result<TypeId, Error> {
        if self.matching_reference(reference, expr)? {
            return self.adjusted_type_with_facts(ty, if assume { f::TRUTHY } else { f::FALSY });
        }
        if self.options.strict_null_checks
            && assume
            && self.optional_chain_contains_reference(expr, reference)?
        {
            ty = self.adjusted_type_with_facts(ty, f::NE_UNDEFINED_OR_NULL)?;
        }
        if let Some(access) = self.flow_discriminant_access(reference, declared, expr, ty)? {
            return self.narrow_discriminant(ty, access, &mut |state, part| {
                state.type_with_facts(part, if assume { f::TRUTHY } else { f::FALSY })
            });
        }
        Ok(ty)
    }
    // port: tsc/internal/checker/flow.go:Checker.narrowTypeByOptionalChainContainment
    fn narrow_optional_chain_containment(
        &mut self,
        ty: TypeId,
        operator: tsr_ast::NodeKind,
        value: NodeId,
        assume: bool,
    ) -> Result<TypeId, Error> {
        let equal = matches!(
            operator.known(),
            Some(K::EqualsEqualsToken | K::EqualsEqualsEqualsToken)
        );
        let nullable = if matches!(
            operator.known(),
            Some(K::EqualsEqualsToken | K::ExclamationEqualsToken)
        ) {
            tf::NULLABLE
        } else {
            tf::UNDEFINED
        };
        let value_type = self.get_type_of_expression(value)?;
        let parts = if self.types.flags(value_type)? & tf::UNION != 0 {
            self.types.types_of(value_type)?
        } else {
            std::slice::from_ref(&value_type)
        };
        let mut remove = true;
        for &part in parts {
            let flags = self.types.flags(part)?;
            remove &= if equal == assume {
                flags & (tf::ANY_OR_UNKNOWN | nullable) == 0
            } else {
                flags & nullable != 0
            };
        }
        if remove {
            self.adjusted_type_with_facts(ty, f::NE_UNDEFINED_OR_NULL)
        } else {
            Ok(ty)
        }
    }

    // port: tsc/internal/checker/flow.go:Checker.narrowTypeByInKeyword
    fn narrow_flow_in(&mut self, ty: TypeId, key: TypeId, assume: bool) -> Result<TypeId, Error> {
        let name = self
            .index_property_name(key)?
            .ok_or(Error::MissingLink("in property name"))?;
        let parts = if self.types.flags(ty)? & tf::UNION != 0 {
            self.types.types_of(ty)?.to_vec()
        } else {
            vec![ty]
        };
        let mut known = false;
        for part in parts {
            if self.flow_type_presence_possible(part, name.as_bytes(), true)? {
                known = true;
                break;
            }
        }
        if known {
            return self.filter_type(ty, &mut |state, part| {
                state.flow_type_presence_possible(part, name.as_bytes(), assume)
            });
        }
        if assume {
            if let Some(symbol) = self.lookup_symbol(
                self.builtins.globals,
                b"Record",
                tsr_ast::symbol_flags::TYPE_ALIAS,
            )? {
                let declared = self.get_declared_type_of_symbol(symbol)?;
                let parameters = self
                    .query
                    .type_aliases
                    .try_get(symbol)
                    .and_then(|links| links.parameters.clone())
                    .unwrap_or_default();
                if parameters.len() == 2 {
                    let record = self.type_alias_instantiation(
                        symbol,
                        declared,
                        &parameters,
                        &[key, self.builtins.unknown_type],
                        None,
                    )?;
                    return self.get_intersection_type(&[ty, record]);
                }
            }
        }
        Ok(ty)
    }
    // port: tsc/internal/checker/flow.go:Checker.isTypePresencePossible
    fn flow_type_presence_possible(
        &mut self,
        ty: TypeId,
        name: &[u8],
        assume: bool,
    ) -> Result<bool, Error> {
        if let Some(property) = self.constituent_property(ty, name, false)? {
            let symbol = self.symbol(property)?;
            return Ok(symbol.flags() & tsr_ast::symbol_flags::OPTIONAL != 0
                || symbol.check_flags() & tsr_ast::check_flags::PARTIAL != 0
                || assume);
        }
        let key = self.get_string_literal_type(tsr_ast::JsString::from_bytes(name))?;
        Ok(self.applicable_index_info(ty, key)?.is_some() || !assume)
    }

    // port: tsc/internal/checker/flow.go:Checker.getReferenceCandidate
    pub(crate) fn reference_candidate(&self, mut node: NodeId) -> Result<NodeId, Error> {
        loop {
            let read = self.node(node)?;
            if read.kind() == K::ParenthesizedExpression {
                node = read
                    .expression()
                    .ok_or(Error::MissingLink("reference operand"))?;
                continue;
            }
            if let Some(binary) = read.data_source().as_binary_expression() {
                let op = binary
                    .operator_token()
                    .ok_or(Error::MissingLink("reference operator"))?;
                let next = match self.node(op)?.kind().known() {
                    Some(
                        K::EqualsToken
                        | K::BarBarEqualsToken
                        | K::AmpersandAmpersandEqualsToken
                        | K::QuestionQuestionEqualsToken,
                    ) => binary.left(),
                    Some(K::CommaToken) => binary.right(),
                    _ => None,
                };
                if let Some(next) = next {
                    node = next;
                    continue;
                }
            }
            return Ok(node);
        }
    }

    // port: tsc/internal/checker/relater.go:Checker.areTypesComparable
    pub(crate) fn types_comparable(&mut self, a: TypeId, b: TypeId) -> Result<bool, Error> {
        Ok(self.is_type_related_to(a, b, RelationKind::Comparable)?
            || self.is_type_related_to(b, a, RelationKind::Comparable)?)
    }

    // port: tsc/internal/checker/flow.go:Checker.narrowTypeByLiteralExpression
    pub(crate) fn narrow_typeof(
        &mut self,
        ty: TypeId,
        name: &[u8],
        assume: bool,
    ) -> Result<TypeId, Error> {
        let (implied, eq, ne) = match name {
            // port: tsc/internal/checker/flow.go:Checker.narrowTypeByTypeName
            b"string" => (
                self.builtins.string_type,
                f::TYPEOF_EQ_STRING,
                f::TYPEOF_NE_STRING,
            ),
            b"number" => (
                self.builtins.number_type,
                f::TYPEOF_EQ_NUMBER,
                f::TYPEOF_NE_NUMBER,
            ),
            b"bigint" => (
                self.builtins.bigint_type,
                f::TYPEOF_EQ_BIG_INT,
                f::TYPEOF_NE_BIG_INT,
            ),
            b"boolean" => (
                self.builtins.boolean_type,
                f::TYPEOF_EQ_BOOLEAN,
                f::TYPEOF_NE_BOOLEAN,
            ),
            b"symbol" => (
                self.builtins.es_symbol_type,
                f::TYPEOF_EQ_SYMBOL,
                f::TYPEOF_NE_SYMBOL,
            ),
            b"undefined" => (
                self.builtins.undefined_type,
                f::EQ_UNDEFINED,
                f::NE_UNDEFINED,
            ),
            b"object" => (
                self.builtins.non_primitive_type,
                f::TYPEOF_EQ_OBJECT,
                f::TYPEOF_NE_OBJECT,
            ),
            b"function" => (
                *self
                    .query
                    .global_types
                    .get("Function")
                    .ok_or(Error::MissingLink("global Function"))?,
                f::TYPEOF_EQ_FUNCTION,
                f::TYPEOF_NE_FUNCTION,
            ),
            _ => (
                self.builtins.non_primitive_type,
                f::TYPEOF_EQ_HOST_OBJECT,
                f::TYPEOF_NE_HOST_OBJECT,
            ),
        };
        if !assume {
            return self.adjusted_type_with_facts(ty, ne);
        }
        if matches!(name, b"object" | b"function") && self.types.flags(ty)? & tf::ANY != 0 {
            return Ok(ty);
        }
        let result = self.narrow_by_type_facts(ty, implied, eq)?;
        if name == b"object" {
            let null = self.narrow_by_type_facts(ty, self.builtins.null_type, f::EQ_NULL)?;
            return self.get_union_type(&[result, null]);
        }
        Ok(result)
    }

    // port: tsc/internal/checker/flow.go:Checker.narrowTypeByTypeFacts
    fn narrow_by_type_facts(
        &mut self,
        ty: TypeId,
        implied: TypeId,
        facts: u32,
    ) -> Result<TypeId, Error> {
        self.map_type(ty, &mut |state, part| {
            Ok(Some(
                if state.is_type_related_to(part, implied, RelationKind::StrictSubtype)? {
                    if state.type_facts(part, facts)? != 0 {
                        part
                    } else {
                        state.builtins.never_type
                    }
                } else if state.is_type_related_to(implied, part, RelationKind::Subtype)? {
                    implied
                } else if state.type_facts(part, facts)? != 0 {
                    state.get_intersection_type(&[part, implied])?
                } else {
                    state.builtins.never_type
                },
            ))
        })?
        .ok_or(Error::MissingLink("narrowed typeof type"))
    }
}
