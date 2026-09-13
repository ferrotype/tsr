//! Switch narrowing and exhaustiveness retain native source order and circularity.
use crate::{type_facts as f, type_flags as tf, types::Map, CheckerState, Error, TypeId};
use std::sync::Arc;
use ts_arena::NodeId;
use ts_ast::{FlowSwitchClauseData, JsString, SyntaxKind as K};
#[derive(Default)]
pub(crate) struct FlowSwitches {
    pub(crate) types: Map<NodeId, Arc<[TypeId]>>,
    pub(crate) witnesses: Map<NodeId, Option<Arc<[JsString]>>>,
    // None means computing. A recursive query resolves it permanently to false.
    pub(crate) exhaustive: Map<NodeId, Option<bool>>,
}
fn required<T>(v: Option<T>, name: &'static str) -> Result<T, Error> {
    v.ok_or(Error::MissingLink(name))
}
impl CheckerState {
    fn flow_switch_clauses(&self, node: NodeId) -> Result<Vec<NodeId>, Error> {
        let read = self.ast(node)?.node(node)?;
        let block = required(
            read.data_source()
                .as_switch_statement()
                .ok_or(ts_arena::Error::InvalidGraph)?
                .case_block(),
            "switch case block",
        )?;
        let read = self.ast(block)?.node(block)?;
        self.source_list(
            block,
            read.data_source()
                .as_case_block()
                .ok_or(ts_arena::Error::InvalidGraph)?
                .clauses(),
        )
    }
    // port: tsc/internal/checker/flow.go:Checker.getSwitchClauseTypes
    // port: tsc/internal/checker/flow.go:Checker.getTypeOfSwitchClause
    pub(crate) fn flow_switch_types(&mut self, node: NodeId) -> Result<Arc<[TypeId]>, Error> {
        if let Some(types) = self.flow.switches.types.get(&node) {
            return Ok(types.clone());
        }
        let mut types = Vec::new();
        for clause in self.flow_switch_clauses(node)? {
            let read = self.ast(clause)?.node(clause)?;
            let ty = if read.kind() == K::CaseClause {
                let expression = required(read.expression(), "case expression")?;
                let ty = self.get_type_of_expression(expression)?;
                self.get_regular_type_of_literal_type(ty)?
            } else {
                self.builtins.never_type
            };
            types.push(ty);
        }
        let types: Arc<[TypeId]> = types.into();
        self.flow.switches.types.insert(node, types.clone());
        Ok(types)
    }
    // port: tsc/internal/checker/flow.go:Checker.getSwitchClauseTypeOfWitnesses
    fn flow_switch_witnesses(&mut self, node: NodeId) -> Result<Option<Arc<[JsString]>>, Error> {
        if let Some(value) = self.flow.switches.witnesses.get(&node) {
            return Ok(value.clone());
        }
        let clauses = self.flow_switch_clauses(node)?;
        let mut witnesses = vec![JsString::default(); clauses.len()];
        for (index, clause) in clauses.into_iter().enumerate() {
            let read = self.ast(clause)?.node(clause)?;
            if read.kind() == K::CaseClause {
                let expression = required(read.expression(), "case witness")?;
                if !matches!(
                    self.ast(expression)?.node(expression)?.kind().known(),
                    Some(K::StringLiteral | K::NoSubstitutionTemplateLiteral)
                ) {
                    self.flow.switches.witnesses.insert(node, None);
                    return Ok(None);
                }
                let text = self
                    .ast(expression)?
                    .node_text(expression)?
                    .into_js_string();
                if !witnesses.contains(&text) {
                    witnesses[index] = text;
                }
            }
        }
        let witnesses: Arc<[JsString]> = witnesses.into();
        self.flow
            .switches
            .witnesses
            .insert(node, Some(witnesses.clone()));
        Ok(Some(witnesses))
    }
    // port: tsc/internal/checker/flow.go:Checker.isExhaustiveSwitchStatement
    pub(crate) fn flow_switch_exhaustive(&mut self, node: NodeId) -> Result<bool, Error> {
        if let Some(result) = self.flow.switches.exhaustive.get(&node).copied() {
            if result.is_none() {
                self.flow.switches.exhaustive.insert(node, Some(false));
            }
            return Ok(result.unwrap_or(false));
        }
        self.flow.switches.exhaustive.insert(node, None);
        match self.compute_flow_switch_exhaustive(node) {
            Ok(result) => {
                let cached = self
                    .flow
                    .switches
                    .exhaustive
                    .get_mut(&node)
                    .ok_or(Error::MissingLink("switch exhaustive state"))?;
                if cached.is_none() {
                    *cached = Some(result);
                }
                Ok(*cached == Some(true))
            }
            Err(error) => {
                self.flow.switches.exhaustive.remove(&node);
                Err(error)
            }
        }
    }
    // port: tsc/internal/checker/flow.go:Checker.computeExhaustiveSwitchStatement
    fn compute_flow_switch_exhaustive(&mut self, node: NodeId) -> Result<bool, Error> {
        let expression = required(
            self.ast(node)?.node(node)?.expression(),
            "switch expression",
        )?;
        let read = self.ast(expression)?.node(expression)?;
        if read.kind() == K::TypeOfExpression {
            let inner = required(read.expression(), "typeof switch operand")?;
            let Some(witnesses) = self.flow_switch_witnesses(node)? else {
                return Ok(false);
            };
            let checked = self.check_expression_cached(inner)?;
            let constraint = self.base_constraint_of_type(checked)?.unwrap_or(checked);
            let facts = not_equal_facts(0, 0, &witnesses);
            if self.types.flags(constraint)? & tf::ANY_OR_UNKNOWN != 0 {
                return Ok(facts & f::ALL_TYPEOF_NE == f::ALL_TYPEOF_NE);
            }
            let parts = if self.types.flags(constraint)? & tf::UNION != 0 {
                self.types.types_of(constraint)?.to_vec()
            } else {
                vec![constraint]
            };
            for part in parts {
                if self.type_facts(part, facts)? == facts {
                    return Ok(false);
                }
            }
            return Ok(true);
        }
        let checked = self.check_expression_cached(expression)?;
        let ty = self.base_constraint_of_type(checked)?.unwrap_or(checked);
        if !self.is_literal_type(ty)? {
            return Ok(false);
        }
        let cases = self.flow_switch_types(node)?;
        if cases.is_empty() {
            return Ok(false);
        }
        for &case in cases.iter() {
            if self.types.flags(case)? & (tf::UNIT | tf::NEVER) == 0 {
                return Ok(false);
            }
        }
        let regular = self.get_regular_type_of_literal_type(ty)?;
        let parts = if self.types.flags(regular)? & tf::UNION != 0 {
            self.types.types_of(regular)?
        } else {
            std::slice::from_ref(&regular)
        };
        Ok(parts.iter().all(|part| cases.contains(part)))
    }
    // port: tsc/internal/checker/flow.go:Checker.getTypeAtSwitchClause
    pub(crate) fn narrow_flow_switch(
        &mut self,
        reference: NodeId,
        declared: TypeId,
        mut ty: TypeId,
        data: FlowSwitchClauseData,
    ) -> Result<TypeId, Error> {
        let statement = required(data.switch_statement, "flow switch statement")?;
        let expression = required(
            self.ast(statement)?.node(statement)?.expression(),
            "switch discriminant",
        )?;
        let expression = ts_ast::skip_parentheses(self.ast(expression)?, expression)?;
        let kind = self.ast(expression)?.node(expression)?.kind();
        if self.matching_reference(reference, expression)? {
            return self.narrow_switch_discriminant(ty, data);
        }
        if kind == K::TypeOfExpression {
            let inner = required(
                self.ast(expression)?.node(expression)?.expression(),
                "typeof switch expression",
            )?;
            if self.matching_reference(reference, inner)? {
                return self.narrow_switch_typeof(ty, data);
            }
        }
        if kind == K::TrueKeyword {
            return self.narrow_switch_true(reference, declared, ty, data);
        }
        if self.options.strict_null_checks {
            let optional_typeof = if kind == K::TypeOfExpression {
                let inner = required(
                    self.ast(expression)?.node(expression)?.expression(),
                    "typeof chain expression",
                )?;
                self.optional_chain_contains_reference(inner, reference)?
            } else {
                false
            };
            if self.optional_chain_contains_reference(expression, reference)? || optional_typeof {
                let cases = self.flow_switch_types(statement)?;
                let range = clause_range(data, cases.len())?;
                let mut every = !range.is_empty();
                for &case in &cases[range] {
                    let flags = self.types.flags(case)?;
                    every &= if optional_typeof {
                        flags & tf::NEVER == 0
                            && !(flags & tf::STRING_LITERAL != 0
                                && matches!(&self.types.literal(case)?.value, crate::types::LiteralValue::String(value) if value.as_bytes() == b"undefined"))
                    } else {
                        flags & (tf::UNDEFINED | tf::NEVER) == 0
                    };
                }
                if every {
                    ty = self.type_with_facts(ty, f::NE_UNDEFINED_OR_NULL)?;
                }
            }
        }
        if let Some(access) = self.flow_discriminant_access(reference, declared, expression, ty)? {
            if data.clause_start < data.clause_end && self.types.flags(ty)? & tf::UNION != 0 {
                if let Some(name) = self.flow_property_name(access)? {
                    if !name.is_empty() && self.flow_key_property_name(ty)? == name {
                        let cases = self.flow_switch_types(statement)?;
                        let mut candidates = Vec::new();
                        for &case in &cases[clause_range(data, cases.len())?] {
                            candidates.push(
                                self.flow_constituent_for_key(ty, case)?
                                    .unwrap_or(self.builtins.unknown_type),
                            );
                        }
                        let candidate = self.get_union_type(&candidates)?;
                        if candidate != self.builtins.unknown_type {
                            return Ok(candidate);
                        }
                    }
                }
            }
            return self.narrow_discriminant(ty, access, &mut |state, part| {
                state.narrow_switch_discriminant(part, data)
            });
        }
        Ok(ty)
    }
    // port: tsc/internal/checker/flow.go:Checker.narrowTypeBySwitchOnDiscriminant
    fn narrow_switch_discriminant(
        &mut self,
        ty: TypeId,
        data: FlowSwitchClauseData,
    ) -> Result<TypeId, Error> {
        let cases = self.flow_switch_types(required(data.switch_statement, "switch statement")?)?;
        if cases.is_empty() {
            return Ok(ty);
        }
        let clause_types = &cases[clause_range(data, cases.len())?];
        let default = data.is_empty() || clause_types.contains(&self.builtins.never_type);
        if self.types.flags(ty)? & tf::UNKNOWN != 0 && !default {
            let mut ground = Vec::with_capacity(clause_types.len());
            for &part in clause_types {
                let flags = self.types.flags(part)?;
                if flags & (tf::PRIMITIVE | tf::NON_PRIMITIVE) != 0 {
                    ground.push(part);
                } else if flags & tf::OBJECT != 0 {
                    ground.push(self.builtins.non_primitive_type);
                } else {
                    return Ok(ty);
                }
            }
            return self.get_union_type(&ground);
        }
        let discriminant = self.get_union_type(clause_types)?;
        let case = if self.types.flags(discriminant)? & tf::NEVER != 0 {
            self.builtins.never_type
        } else {
            let regular = self.get_regular_type_of_literal_type(discriminant)?;
            if self.types.flags(discriminant)? & tf::PRIMITIVE != 0
                && self.uniform_union_type(ty)?
                && self.flow_union_contains(ty, regular)?
            {
                regular
            } else {
                let filtered = self.filter_type(ty, &mut |state, part| {
                    state.types_comparable(discriminant, part)
                })?;
                self.replace_primitives_with_literals(filtered, discriminant)?
            }
        };
        if !default {
            return Ok(case);
        }
        let default_type = self.filter_type(ty, &mut |state, part| {
            let base = state.base_constraint_of_type(part)?.unwrap_or(part);
            let unit = if state.types.flags(base)? & tf::INTERSECTION != 0 {
                let mut found = None;
                for &part in state.types.types_of(base)? {
                    if state.types.flags(part)? & tf::UNIT != 0 {
                        found = Some(part);
                        break;
                    }
                }
                found
            } else {
                (state.types.flags(base)? & tf::UNIT != 0).then_some(base)
            };
            let Some(unit) = unit else {
                return Ok(true);
            };
            let unit = if state.types.flags(part)? & tf::UNDEFINED != 0 {
                state.builtins.undefined_type
            } else {
                state.get_regular_type_of_literal_type(unit)?
            };
            for &case in cases.iter() {
                if state.types.flags(case)? & tf::UNIT != 0 && state.types_comparable(case, unit)? {
                    return Ok(false);
                }
            }
            Ok(true)
        })?;
        if self.types.flags(case)? & tf::NEVER != 0 {
            Ok(default_type)
        } else {
            self.get_union_type(&[case, default_type])
        }
    }
    // port: tsc/internal/checker/flow.go:Checker.narrowTypeBySwitchOnTypeOf
    fn narrow_switch_typeof(
        &mut self,
        ty: TypeId,
        data: FlowSwitchClauseData,
    ) -> Result<TypeId, Error> {
        let statement = required(data.switch_statement, "typeof switch statement")?;
        let Some(witnesses) = self.flow_switch_witnesses(statement)? else {
            return Ok(ty);
        };
        let clauses = self.flow_switch_clauses(statement)?;
        let range = clause_range(data, clauses.len())?;
        let mut default = range.is_empty();
        for &clause in &clauses[range.clone()] {
            default |= self.ast(clause)?.node(clause)?.kind() == K::DefaultClause;
        }
        if default {
            let facts = not_equal_facts(range.start, range.end, &witnesses);
            return self.filter_type(ty, &mut |state, part| {
                Ok(state.type_facts(part, facts)? == facts)
            });
        }
        let mut parts = Vec::new();
        for witness in &witnesses[range] {
            parts.push(if witness.is_empty() {
                self.builtins.never_type
            } else {
                self.narrow_typeof(ty, witness.as_bytes(), true)?
            });
        }
        self.get_union_type(&parts)
    }
    // port: tsc/internal/checker/flow.go:Checker.narrowTypeBySwitchOnTrue
    fn narrow_switch_true(
        &mut self,
        reference: NodeId,
        declared: TypeId,
        mut ty: TypeId,
        data: FlowSwitchClauseData,
    ) -> Result<TypeId, Error> {
        let clauses =
            self.flow_switch_clauses(required(data.switch_statement, "true switch statement")?)?;
        let range = clause_range(data, clauses.len())?;
        let mut default = range.is_empty();
        for &clause in &clauses[range.clone()] {
            default |= self.ast(clause)?.node(clause)?.kind() == K::DefaultClause;
        }
        for &clause in &clauses[..range.start] {
            let read = self.ast(clause)?.node(clause)?;
            if read.kind() == K::CaseClause {
                ty = self.narrow_reference_type(
                    reference,
                    declared,
                    ty,
                    required(read.expression(), "true switch case")?,
                    false,
                )?;
            }
        }
        if default {
            for &clause in &clauses[range.end..] {
                let read = self.ast(clause)?.node(clause)?;
                if read.kind() == K::CaseClause {
                    ty = self.narrow_reference_type(
                        reference,
                        declared,
                        ty,
                        required(read.expression(), "true switch case")?,
                        false,
                    )?;
                }
            }
            return Ok(ty);
        }
        let mut parts = Vec::new();
        for &clause in &clauses[range] {
            let read = self.ast(clause)?.node(clause)?;
            parts.push(if read.kind() == K::CaseClause {
                self.narrow_reference_type(
                    reference,
                    declared,
                    ty,
                    required(read.expression(), "true switch case")?,
                    true,
                )?
            } else {
                self.builtins.never_type
            });
        }
        self.get_union_type(&parts)
    }
}
fn clause_range(data: FlowSwitchClauseData, len: usize) -> Result<std::ops::Range<usize>, Error> {
    let start = usize::try_from(data.clause_start).map_err(|_| ts_arena::Error::InvalidGraph)?;
    let end = usize::try_from(data.clause_end).map_err(|_| ts_arena::Error::InvalidGraph)?;
    if start > end || end > len {
        return Err(ts_arena::Error::InvalidGraph.into());
    }
    Ok(start..end)
}
// port: tsc/internal/checker/flow.go:Checker.getNotEqualFactsFromTypeofSwitch
fn not_equal_facts(start: usize, end: usize, witnesses: &[JsString]) -> u32 {
    witnesses
        .iter()
        .enumerate()
        .filter(|(index, witness)| (*index < start || *index >= end) && !witness.is_empty())
        .fold(0, |facts, (_, witness)| {
            facts
                | match witness.as_bytes() {
                    b"string" => f::TYPEOF_NE_STRING,
                    b"number" => f::TYPEOF_NE_NUMBER,
                    b"bigint" => f::TYPEOF_NE_BIG_INT,
                    b"boolean" => f::TYPEOF_NE_BOOLEAN,
                    b"symbol" => f::TYPEOF_NE_SYMBOL,
                    b"undefined" => f::NE_UNDEFINED,
                    b"object" => f::TYPEOF_NE_OBJECT,
                    b"function" => f::TYPEOF_NE_FUNCTION,
                    _ => f::TYPEOF_NE_HOST_OBJECT,
                }
        })
}
