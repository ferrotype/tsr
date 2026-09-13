//! Flow queries over the immutable binder graph. Temporary assumptions belong
//! to one query; only completed reachability results enter the checker cache.

use crate::{type_facts as facts, type_flags as tf, CheckerState, Error, TypeId};
use ts_arena::NodeId;
use ts_ast::{flow_flags as ff, FlowData, FlowId, FlowListId, FlowNode, SyntaxKind as K};

#[derive(Default)]
pub(crate) struct FlowAnalysis {
    pub(crate) evolving: crate::types::Map<TypeId, TypeId>,
    pub(crate) switches: crate::flow_switch::FlowSwitches,
    pub(crate) effects: crate::flow_effects::FlowEffects,
    pub(crate) assignments: crate::flow_assignments::FlowAssignments,
    pub(crate) symbol_narrowing_parents: crate::types::Set<NodeId>,
    pub(crate) inline_level: usize,
    pub(crate) disabled: bool,
    pub(crate) invocation_count: u64,
    pub(crate) reachable: crate::types::Map<FlowId, bool>,
    post_super: crate::types::Map<FlowId, bool>,
    pub(crate) synthetic: crate::types::Map<NodeId, (NodeId, FlowId)>,
    pub(crate) loop_cache: crate::types::Map<FlowLoopKey, TypeId>,
    pub(crate) loop_stack: Vec<FlowLoopInfo>,
    pub(crate) expression_cache: crate::types::Map<NodeId, TypeId>,
    pub(crate) narrowed: crate::types::Map<(TypeId, TypeId, bool, bool), TypeId>,
    pub(crate) assignment_reduced: crate::types::Map<(TypeId, TypeId), TypeId>,
}

#[derive(Clone, Copy)]
struct FlowType {
    ty: TypeId,
    incomplete: bool,
}
impl FlowType {
    fn complete(ty: TypeId) -> Self {
        Self {
            ty,
            incomplete: false,
        }
    }
}
#[derive(Clone, Eq, PartialEq, Hash)]
pub(crate) struct FlowLoopKey {
    flow: FlowId,
    reference: crate::CacheKey,
    declared: TypeId,
    initial: TypeId,
}
pub(crate) struct FlowLoopInfo {
    key: FlowLoopKey,
    types: Vec<TypeId>,
}
struct FlowQuery {
    reference: NodeId,
    flow_owner: NodeId,
    declared: TypeId,
    initial: TypeId,
    container: Option<NodeId>,
    depth: usize,
    shared: Vec<(FlowId, FlowType)>,
    reduce_labels: Vec<ts_ast::FlowReduceLabelData>,
}

fn required<T>(value: Option<T>, context: &'static str) -> Result<T, Error> {
    value.ok_or(Error::MissingLink(context))
}

impl CheckerState {
    fn flow_node(&self, owner: NodeId, flow: FlowId) -> Result<FlowNode, Error> {
        let owner = self
            .flow
            .synthetic
            .get(&owner)
            .map_or(owner, |&(source, _)| source);
        Ok(self
            .program()?
            .bound(owner)?
            .result()
            .flows()
            .get(flow)?
            .to_owned())
    }

    pub(crate) fn node_flow(&self, node: NodeId) -> Result<Option<FlowId>, Error> {
        if let Some(&(_, flow)) = self.flow.synthetic.get(&node) {
            return Ok(Some(flow));
        }
        Ok(self
            .program()?
            .bound(node)?
            .node_binding(node)?
            .and_then(|b| b.flow_node))
    }

    fn flow_antecedents(
        &self,
        owner: NodeId,
        mut list: Option<FlowListId>,
    ) -> Result<Vec<FlowId>, Error> {
        let owner = self
            .flow
            .synthetic
            .get(&owner)
            .map_or(owner, |&(source, _)| source);
        let lists = self.program()?.bound(owner)?.result().flow_lists();
        let mut result = Vec::new();
        while let Some(id) = list {
            let item = lists.get(id)?;
            result.push(required(item.flow(), "flow list element")?);
            list = item.next();
        }
        Ok(result)
    }

    // port: tsc/internal/checker/checker.go:Checker.functionHasImplicitReturn
    pub(crate) fn function_has_implicit_return(&mut self, function: NodeId) -> Result<bool, Error> {
        let flow = self
            .program()?
            .bound(function)?
            .node_binding(function)?
            .and_then(|b| b.end_flow_node);
        match flow {
            Some(flow) => self.reachable_flow(function, flow),
            None => Ok(false),
        }
    }

    // port: tsc/internal/checker/flow.go:Checker.isReachableFlowNode
    pub(crate) fn reachable_flow(&mut self, owner: NodeId, flow: FlowId) -> Result<bool, Error> {
        self.reachable_flow_worker(owner, flow, &mut Vec::new())
    }

    // port: tsc/internal/checker/flow.go:Checker.isReachableFlowNodeWorker
    fn reachable_flow_worker(
        &mut self,
        owner: NodeId,
        mut flow: FlowId,
        reduced: &mut Vec<ts_ast::FlowReduceLabelData>,
    ) -> Result<bool, Error> {
        stacker::maybe_grow(128 * 1024, 2 * 1024 * 1024, || {
            let mut shared = Vec::new();
            let result = loop {
                let node = self.flow_node(owner, flow)?;
                if node.flags & ff::SHARED != 0 && reduced.is_empty() {
                    if let Some(&result) = self.flow.reachable.get(&flow) {
                        break result;
                    }
                    shared.push(flow);
                }
                if node.flags & (ff::ASSIGNMENT | ff::CONDITION | ff::ARRAY_MUTATION) != 0 {
                    flow = required(node.antecedent, "reachable antecedent")?;
                } else if node.flags & ff::BRANCH_LABEL != 0 {
                    let list = reduced
                        .iter()
                        .rev()
                        .find(|r| r.target == Some(flow))
                        .map_or(node.antecedents, |r| r.antecedents);
                    let mut reachable = false;
                    for branch in self.flow_antecedents(owner, list)? {
                        if self.reachable_flow_worker(owner, branch, reduced)? {
                            reachable = true;
                            break;
                        }
                    }
                    break reachable;
                } else if node.flags & ff::LOOP_LABEL != 0 {
                    let branches = self.flow_antecedents(owner, node.antecedents)?;
                    let Some(&first) = branches.first() else {
                        break false;
                    };
                    flow = first;
                } else if node.flags & ff::REDUCE_LABEL != 0 {
                    let Some(FlowData::ReduceLabel(data)) = node.node else {
                        return Err(ts_arena::Error::InvalidGraph.into());
                    };
                    reduced.push(data);
                    let result = self.reachable_flow_worker(
                        owner,
                        required(node.antecedent, "reduce antecedent")?,
                        reduced,
                    );
                    reduced.pop();
                    break result?;
                } else if node.flags & ff::CALL != 0 {
                    let Some(FlowData::Ast(call)) = node.node else {
                        return Err(ts_arena::Error::InvalidGraph.into());
                    };
                    if self.flow_call_is_never(call)? {
                        break false;
                    }
                    flow = required(node.antecedent, "reachable call antecedent")?;
                } else if node.flags & ff::SWITCH_CLAUSE != 0 {
                    let Some(FlowData::SwitchClause(data)) = node.node else {
                        return Err(ts_arena::Error::InvalidGraph.into());
                    };
                    if data.is_empty()
                        && self.flow_switch_exhaustive(required(
                            data.switch_statement,
                            "reachable switch statement",
                        )?)?
                    {
                        break false;
                    }
                    flow = required(node.antecedent, "reachable switch antecedent")?;
                } else {
                    break node.flags & ff::UNREACHABLE == 0;
                }
            };
            for flow in shared {
                self.flow.reachable.insert(flow, result);
            }
            Ok(result)
        })
    }

    // port: tsc/internal/checker/flow.go:Checker.isPostSuperFlowNode
    pub(crate) fn is_post_super_flow_node(
        &mut self,
        owner: NodeId,
        flow: FlowId,
    ) -> Result<bool, Error> {
        self.post_super_flow_worker(owner, flow, false, &mut Vec::new())
    }

    // port: tsc/internal/checker/flow.go:Checker.isPostSuperFlowNodeWorker
    fn post_super_flow_worker(
        &mut self,
        owner: NodeId,
        mut flow: FlowId,
        mut no_cache_check: bool,
        reduced: &mut Vec<ts_ast::FlowReduceLabelData>,
    ) -> Result<bool, Error> {
        stacker::maybe_grow(128 * 1024, 2 * 1024 * 1024, || loop {
            let node = self.flow_node(owner, flow)?;
            if node.flags & ff::SHARED != 0 {
                if !no_cache_check {
                    if let Some(&result) = self.flow.post_super.get(&flow) {
                        return Ok(result);
                    }
                    let result = self.post_super_flow_worker(owner, flow, true, reduced)?;
                    self.flow.post_super.insert(flow, result);
                }
                no_cache_check = false;
            }
            if node.flags
                & (ff::ASSIGNMENT | ff::CONDITION | ff::ARRAY_MUTATION | ff::SWITCH_CLAUSE)
                != 0
            {
                flow = required(node.antecedent, "post-super antecedent")?;
            } else if node.flags & ff::CALL != 0 {
                let Some(FlowData::Ast(call)) = node.node else {
                    return Err(ts_arena::Error::InvalidGraph.into());
                };
                let expression = required(
                    self.ast(call)?.node(call)?.expression(),
                    "post-super call expression",
                )?;
                if self.ast(expression)?.node(expression)?.kind() == K::SuperKeyword {
                    return Ok(true);
                }
                flow = required(node.antecedent, "post-super call antecedent")?;
            } else if node.flags & ff::BRANCH_LABEL != 0 {
                let list = reduced
                    .iter()
                    .rev()
                    .find(|label| label.target == Some(flow))
                    .map_or(node.antecedents, |label| label.antecedents);
                for branch in self.flow_antecedents(owner, list)? {
                    if !self.post_super_flow_worker(owner, branch, false, reduced)? {
                        return Ok(false);
                    }
                }
                return Ok(true);
            } else if node.flags & ff::LOOP_LABEL != 0 {
                flow = *self
                    .flow_antecedents(owner, node.antecedents)?
                    .first()
                    .ok_or(ts_arena::Error::InvalidGraph)?;
            } else if node.flags & ff::REDUCE_LABEL != 0 {
                let Some(FlowData::ReduceLabel(data)) = node.node else {
                    return Err(ts_arena::Error::InvalidGraph.into());
                };
                reduced.push(data);
                let result = self.post_super_flow_worker(
                    owner,
                    required(node.antecedent, "post-super reduce antecedent")?,
                    false,
                    reduced,
                );
                reduced.pop();
                return result;
            } else {
                return Ok(node.flags & ff::UNREACHABLE != 0);
            }
        })
    }

    // port: tsc/internal/checker/flow.go:Checker.getFlowTypeOfReferenceEx
    pub(crate) fn flow_type_of_reference(
        &mut self,
        reference: NodeId,
        declared: TypeId,
        initial: TypeId,
        container: NodeId,
    ) -> Result<TypeId, Error> {
        self.flow_type_of_reference_with_container(reference, declared, initial, Some(container))
    }

    pub(crate) fn flow_type_of_this_reference(
        &mut self,
        reference: NodeId,
        declared: TypeId,
    ) -> Result<TypeId, Error> {
        self.flow_type_of_reference_with_container(reference, declared, declared, None)
    }

    pub(crate) fn flow_type_of_reference_with_container(
        &mut self,
        reference: NodeId,
        declared: TypeId,
        initial: TypeId,
        container: Option<NodeId>,
    ) -> Result<TypeId, Error> {
        self.flow_type_of_reference_at_flow(
            reference, declared, initial, container, reference, None,
        )
    }

    pub(crate) fn flow_type_of_reference_at_flow(
        &mut self,
        reference: NodeId,
        declared: TypeId,
        initial: TypeId,
        container: Option<NodeId>,
        flow_owner: NodeId,
        flow: Option<FlowId>,
    ) -> Result<TypeId, Error> {
        if self.flow.disabled {
            return Ok(self.builtins.error_type);
        }
        let (flow_owner, flow) = if let Some(flow) = flow {
            (flow_owner, flow)
        } else if let Some(flow) = self.node_flow(reference)? {
            (reference, flow)
        } else {
            return Ok(declared);
        };
        self.flow.invocation_count += 1;
        let mut query = FlowQuery {
            reference,
            flow_owner,
            declared,
            initial,
            container,
            depth: 0,
            shared: Vec::new(),
            reduce_labels: Vec::new(),
        };
        let evolved = self.type_at_flow(&mut query, flow)?.ty;
        let result = if self.types.object_flags(evolved)? & crate::object_flags::EVOLVING_ARRAY != 0
            && self.evolving_array_operation_target(reference)?
        {
            self.auto_array_type()?
        } else {
            self.finalize_evolving_array(evolved)?
        };
        if result == self.builtins.unreachable_never_type {
            return Ok(declared);
        }
        if let Some(parent) = self.ast(reference)?.node(reference)?.parent() {
            if self.ast(parent)?.node(parent)?.kind() == K::NonNullExpression
                && self.types.flags(result)? & tf::NEVER == 0
            {
                let non_null = self.type_with_facts(result, facts::NE_UNDEFINED_OR_NULL)?;
                if self.types.flags(non_null)? & tf::NEVER != 0 {
                    return Ok(declared);
                }
            }
        }
        Ok(result)
    }

    // port: tsc/internal/checker/checker.go:Checker.checkIfExpressionRefinesParameter
    pub(crate) fn infer_predicate_flow_type(
        &mut self,
        reference: NodeId,
        declared: TypeId,
        initial: TypeId,
        function: NodeId,
        expression: NodeId,
        assume_true: bool,
    ) -> Result<TypeId, Error> {
        let mut antecedent = self.node_flow(expression)?;
        if antecedent.is_none() {
            if let Some(parent) = self.ast(expression)?.node(expression)?.parent() {
                if self.ast(parent)?.node(parent)?.kind() == K::ReturnStatement {
                    antecedent = self.node_flow(parent)?;
                }
            }
        }
        self.flow.invocation_count += 1;
        let mut query = FlowQuery {
            reference,
            flow_owner: expression,
            declared,
            initial,
            container: Some(function),
            depth: 0,
            shared: Vec::new(),
            reduce_labels: Vec::new(),
        };
        let ty = match antecedent {
            Some(flow) => self.type_at_flow(&mut query, flow)?.ty,
            None => initial,
        };
        if self.types.flags(ty)? & tf::NEVER != 0 {
            return Ok(ty);
        }
        self.narrow_reference_type(reference, declared, ty, expression, assume_true)
    }

    // port: tsc/internal/checker/flow.go:Checker.newFlowType
    fn new_flow_type(&self, mut ty: TypeId, incomplete: bool) -> Result<FlowType, Error> {
        if incomplete && self.types.flags(ty)? & tf::NEVER != 0 {
            ty = self.builtins.silent_never_type;
        }
        Ok(FlowType { ty, incomplete })
    }

    // port: tsc/internal/checker/flow.go:Checker.getTypeAtFlowNode
    fn type_at_flow(&mut self, query: &mut FlowQuery, mut flow: FlowId) -> Result<FlowType, Error> {
        if query.depth == 2000 {
            self.flow.disabled = true;
            self.report_flow_control_error(query.reference)?;
            return Ok(FlowType {
                ty: self.builtins.error_type,
                incomplete: false,
            });
        }
        query.depth += 1;
        let result = stacker::maybe_grow(128 * 1024, 2 * 1024 * 1024, || {
            let mut shared = None;
            loop {
                let node = self.flow_node(query.flow_owner, flow)?;
                if node.flags & ff::SHARED != 0 {
                    if let Some((_, ty)) = query.shared.iter().find(|(id, _)| *id == flow) {
                        return Ok(*ty);
                    }
                    shared = Some(flow);
                }
                let result = if node.flags & ff::ASSIGNMENT != 0 {
                    let Some(FlowData::Ast(target)) = node.node else {
                        return Err(ts_arena::Error::InvalidGraph.into());
                    };
                    if self.matching_reference(query.reference, target)? {
                        if !self.reachable_flow(query.flow_owner, flow)? {
                            FlowType::complete(self.builtins.unreachable_never_type)
                        } else if self.assignment_target_kind(target)?
                            == crate::flow_assignments::AssignmentKind::Compound
                        {
                            let previous = self.type_at_flow(
                                query,
                                required(node.antecedent, "compound assignment antecedent")?,
                            )?;
                            let ty = self.base_literal_type(previous.ty)?;
                            self.new_flow_type(ty, previous.incomplete)?
                        } else if self.is_auto_flow_type(query.declared) {
                            let ty = if self.empty_array_assignment(target)? {
                                self.evolving_array_type(self.builtins.never_type)?
                            } else {
                                let assigned =
                                    self.initial_or_assigned_type(target, query.reference)?;
                                let assigned = self.widen_literal_type(assigned)?;
                                if self.is_type_related_to(
                                    assigned,
                                    query.declared,
                                    crate::RelationKind::Assignable,
                                )? {
                                    assigned
                                } else {
                                    self.any_array_type()?
                                }
                            };
                            FlowType::complete(ty)
                        } else {
                            let mut declared = query.declared;
                            if self.compound_like_assignment(target)? {
                                declared = self.base_literal_type(declared)?;
                            }
                            let ty = if self.types.flags(declared)? & tf::UNION != 0 {
                                let assigned =
                                    self.initial_or_assigned_type(target, query.reference)?;
                                self.assignment_reduced_type(declared, assigned)?
                            } else {
                                declared
                            };
                            FlowType::complete(ty)
                        }
                    } else if self.contains_flow_reference(query.reference, target)? {
                        if !self.reachable_flow(query.flow_owner, flow)? {
                            return Ok(FlowType::complete(self.builtins.unreachable_never_type));
                        }
                        let read = self.ast(target)?.node(target)?;
                        if read.kind() == K::VariableDeclaration
                            && (ts_ast::utilities::is_in_js_file(Some(&read))
                                || ts_ast::utilities::is_var_const_like(self.ast(target)?, target)?)
                        {
                            if let Some(initializer) = read.initializer() {
                                if matches!(
                                    self.ast(initializer)?.node(initializer)?.kind().known(),
                                    Some(K::FunctionExpression | K::ArrowFunction)
                                ) {
                                    flow =
                                        required(node.antecedent, "expando assignment antecedent")?;
                                    continue;
                                }
                            }
                        }
                        FlowType::complete(query.declared)
                    } else {
                        if self.ast(target)?.node(target)?.kind() == K::VariableDeclaration {
                            let declaration = self.ast(target)?.node(target)?;
                            if let Some(list) = declaration.parent() {
                                if let Some(parent) = self.ast(list)?.node(list)?.parent() {
                                    let statement = self.ast(parent)?.node(parent)?;
                                    if statement.kind() == K::ForInStatement {
                                        let expr =
                                            required(statement.expression(), "for-in expression")?;
                                        if self.matching_reference(query.reference, expr)?
                                            || self.optional_chain_contains_reference(
                                                expr,
                                                query.reference,
                                            )?
                                        {
                                            let previous = self.type_at_flow(
                                                query,
                                                required(node.antecedent, "for-in antecedent")?,
                                            )?;
                                            let non_evolving =
                                                self.finalize_evolving_array(previous.ty)?;
                                            return Ok(FlowType::complete(
                                                if self.options.strict_null_checks {
                                                    self.adjusted_type_with_facts(
                                                        non_evolving,
                                                        facts::NE_UNDEFINED_OR_NULL,
                                                    )?
                                                } else {
                                                    non_evolving
                                                },
                                            ));
                                        }
                                    }
                                }
                            }
                        }
                        flow = required(node.antecedent, "assignment antecedent")?;
                        continue;
                    }
                } else if node.flags & ff::CONDITION != 0 {
                    let previous = self
                        .type_at_flow(query, required(node.antecedent, "condition antecedent")?)?;
                    if self.types.flags(previous.ty)? & tf::NEVER != 0 {
                        previous
                    } else {
                        let Some(FlowData::Ast(condition)) = node.node else {
                            return Err(ts_arena::Error::InvalidGraph.into());
                        };
                        let non_evolving = self.finalize_evolving_array(previous.ty)?;
                        let ty = self.narrow_reference_type(
                            query.reference,
                            query.declared,
                            non_evolving,
                            condition,
                            node.flags & ff::TRUE_CONDITION != 0,
                        )?;
                        if ty == non_evolving {
                            previous
                        } else {
                            self.new_flow_type(ty, previous.incomplete)?
                        }
                    }
                } else if node.flags & ff::BRANCH_LABEL != 0 {
                    let list = query
                        .reduce_labels
                        .iter()
                        .rev()
                        .find(|r| r.target == Some(flow))
                        .map_or(node.antecedents, |r| r.antecedents);
                    let branches = self.flow_antecedents(query.flow_owner, list)?;
                    if branches.len() == 1 {
                        flow = branches[0];
                        continue;
                    }
                    let mut types = Vec::new();
                    let mut subtype = false;
                    let mut incomplete = false;
                    let mut bypass = None;
                    for branch in branches {
                        let branch_node = self.flow_node(query.flow_owner, branch)?;
                        if bypass.is_none() && branch_node.flags & ff::SWITCH_CLAUSE != 0 {
                            let Some(FlowData::SwitchClause(data)) = branch_node.node else {
                                return Err(ts_arena::Error::InvalidGraph.into());
                            };
                            if data.is_empty() {
                                bypass = Some((branch, data));
                                continue;
                            }
                        }
                        let previous = self.type_at_flow(query, branch)?;
                        if previous.ty == query.declared && query.declared == query.initial {
                            return Ok(FlowType::complete(previous.ty));
                        }
                        if !types.contains(&previous.ty) {
                            types.push(previous.ty);
                        }
                        subtype |= !self.flow_type_subset(previous.ty, query.initial)?;
                        incomplete |= previous.incomplete;
                    }
                    if let Some((branch, data)) = bypass {
                        let previous = self.type_at_flow(query, branch)?;
                        if self.types.flags(previous.ty)? & tf::NEVER == 0
                            && !types.contains(&previous.ty)
                            && !self.flow_switch_exhaustive(required(
                                data.switch_statement,
                                "bypass switch statement",
                            )?)?
                        {
                            if previous.ty == query.declared && query.declared == query.initial {
                                return Ok(FlowType::complete(previous.ty));
                            }
                            types.push(previous.ty);
                            subtype |= !self.flow_type_subset(previous.ty, query.initial)?;
                            incomplete |= previous.incomplete;
                        }
                    }
                    let ty = self.flow_union_type(query, &types, subtype)?;
                    self.new_flow_type(ty, incomplete)?
                } else if node.flags & ff::LOOP_LABEL != 0 {
                    let branches = self.flow_antecedents(query.flow_owner, node.antecedents)?;
                    if branches.len() == 1 {
                        flow = branches[0];
                        continue;
                    }
                    self.type_at_flow_loop(query, flow, &branches)?
                } else if node.flags & ff::REDUCE_LABEL != 0 {
                    let Some(FlowData::ReduceLabel(data)) = node.node else {
                        return Err(ts_arena::Error::InvalidGraph.into());
                    };
                    query.reduce_labels.push(data);
                    let result =
                        self.type_at_flow(query, required(node.antecedent, "reduce antecedent")?);
                    query.reduce_labels.pop();
                    result?
                } else if node.flags & ff::START != 0 {
                    if let Some(FlowData::Ast(container)) = node.node {
                        let kind = self.ast(query.reference)?.node(query.reference)?.kind();
                        if Some(container) != query.container
                            && !matches!(
                                kind.known(),
                                Some(K::PropertyAccessExpression | K::ElementAccessExpression)
                            )
                            && !(kind == K::ThisKeyword
                                && self.ast(container)?.node(container)?.kind() != K::ArrowFunction)
                        {
                            flow = required(self.node_flow(container)?, "outer container flow")?;
                            continue;
                        }
                    }
                    FlowType::complete(query.initial)
                } else if node.flags & ff::ARRAY_MUTATION != 0 {
                    let Some(FlowData::Ast(mutation)) = node.node else {
                        return Err(ts_arena::Error::InvalidGraph.into());
                    };
                    if self.is_auto_flow_type(query.declared)
                        && self.evolving_mutation_target(query.reference, mutation)?
                    {
                        let previous = self.type_at_flow(
                            query,
                            required(node.antecedent, "array mutation antecedent")?,
                        )?;
                        let evolved = self.evolve_array_mutation(previous.ty, mutation)?;
                        self.new_flow_type(evolved, previous.incomplete)?
                    } else {
                        flow = required(node.antecedent, "array mutation antecedent")?;
                        continue;
                    }
                } else if node.flags & ff::CALL != 0 {
                    let Some(FlowData::Ast(call)) = node.node else {
                        return Err(ts_arena::Error::InvalidGraph.into());
                    };
                    if let Some(signature) = self.effects_signature(call)? {
                        if let Some(predicate) = self.type_predicate_of_signature(signature)? {
                            let predicate = self.signatures.predicate(predicate)?;
                            if matches!(
                                predicate.kind,
                                crate::TypePredicateKind::AssertsThis
                                    | crate::TypePredicateKind::AssertsIdentifier
                            ) {
                                let predicate_id = self
                                    .type_predicate_of_signature(signature)?
                                    .ok_or(Error::MissingLink("assertion predicate"))?;
                                let predicate = self.signatures.predicate(predicate_id)?.clone();
                                let antecedent = self.type_at_flow(
                                    query,
                                    required(node.antecedent, "assertion call antecedent")?,
                                )?;
                                let finalized = self.finalize_evolving_array(antecedent.ty)?;
                                let narrowed = if predicate.t.is_some() {
                                    self.narrow_flow_predicate(
                                        query.reference,
                                        query.declared,
                                        finalized,
                                        predicate_id,
                                        call,
                                        true,
                                    )?
                                } else if predicate.kind
                                    == crate::TypePredicateKind::AssertsIdentifier
                                {
                                    let args = self.source_list(
                                        call,
                                        self.ast(call)?.node(call)?.argument_list(),
                                    )?;
                                    match usize::try_from(predicate.parameter_index)
                                        .ok()
                                        .and_then(|index| args.get(index).copied())
                                    {
                                        Some(argument) => self.narrow_flow_assertion(
                                            query.reference,
                                            query.declared,
                                            finalized,
                                            argument,
                                        )?,
                                        None => finalized,
                                    }
                                } else {
                                    finalized
                                };
                                return Ok(if narrowed == finalized {
                                    antecedent
                                } else {
                                    self.new_flow_type(narrowed, antecedent.incomplete)?
                                });
                            }
                        }
                        let ty = self.return_type_of_signature(signature)?;
                        if self.types.flags(ty)? & tf::NEVER != 0 {
                            return Ok(FlowType::complete(self.builtins.unreachable_never_type));
                        }
                    }
                    flow = required(node.antecedent, "call antecedent")?;
                    continue;
                } else if node.flags & ff::SWITCH_CLAUSE != 0 {
                    let Some(FlowData::SwitchClause(data)) = node.node else {
                        return Err(ts_arena::Error::InvalidGraph.into());
                    };
                    let previous =
                        self.type_at_flow(query, required(node.antecedent, "switch antecedent")?)?;
                    let narrowed = self.narrow_flow_switch(
                        query.reference,
                        query.declared,
                        previous.ty,
                        data,
                    )?;
                    self.new_flow_type(narrowed, previous.incomplete)?
                } else {
                    FlowType::complete(self.convert_auto_flow_type(query.declared))
                };
                if let Some(flow) = shared {
                    query.shared.push((flow, result));
                }
                return Ok(result);
            }
        });
        query.depth -= 1;
        result
    }

    fn is_auto_flow_type(&self, ty: TypeId) -> bool {
        ty == self.builtins.auto_type || self.query.global_types.get("autoArrayType") == Some(&ty)
    }
    fn convert_auto_flow_type(&self, ty: TypeId) -> TypeId {
        if ty == self.builtins.auto_type {
            self.builtins.any_type
        } else if self.query.global_types.get("autoArrayType") == Some(&ty) {
            self.query
                .global_types
                .get("anyArrayType")
                .copied()
                .unwrap_or(ty)
        } else {
            ty
        }
    }

    // port: tsc/internal/checker/flow.go:Checker.getUnionOrEvolvingArrayType
    fn flow_union_type(
        &mut self,
        query: &FlowQuery,
        types: &[TypeId],
        subtype: bool,
    ) -> Result<TypeId, Error> {
        let mut all_evolving = false;
        for &ty in types {
            if self.types.flags(ty)? & tf::NEVER == 0 {
                if self.types.object_flags(ty)? & crate::object_flags::EVOLVING_ARRAY == 0 {
                    all_evolving = false;
                    break;
                }
                all_evolving = true;
            }
        }
        if all_evolving {
            let mut elements = Vec::with_capacity(types.len());
            for &ty in types {
                elements.push(
                    if self.types.object_flags(ty)? & crate::object_flags::EVOLVING_ARRAY != 0 {
                        self.types.evolving_array(ty)?.element_type
                    } else {
                        self.builtins.never_type
                    },
                );
            }
            let element = self.get_union_type(&elements)?;
            return self.evolving_array_type(element);
        }
        let mut finalized = Vec::with_capacity(types.len());
        for &ty in types {
            finalized.push(self.finalize_evolving_array(ty)?);
        }
        let mut ty = self.get_union_type_ex(
            &finalized,
            if subtype {
                crate::UnionReduction::Subtype
            } else {
                crate::UnionReduction::Literal
            },
            None,
            None,
        )?;
        if ty == self.builtins.unknown_union_type {
            ty = self.builtins.unknown_type;
        }
        if ty != query.declared
            && self.types.flags(ty)? & self.types.flags(query.declared)? & tf::UNION != 0
            && self.types.compound_types(ty)? == self.types.compound_types(query.declared)?
        {
            ty = query.declared;
        }
        Ok(ty)
    }

    // port: tsc/internal/checker/flow.go:Checker.getTypeAtFlowLoopLabel
    fn type_at_flow_loop(
        &mut self,
        query: &mut FlowQuery,
        flow: FlowId,
        branches: &[FlowId],
    ) -> Result<FlowType, Error> {
        let Some(reference) = self.flow_reference_key(query)? else {
            return Ok(FlowType::complete(query.declared));
        };
        let key = FlowLoopKey {
            flow,
            reference,
            declared: query.declared,
            initial: query.initial,
        };
        if let Some(&ty) = self.flow.loop_cache.get(&key) {
            return Ok(FlowType::complete(ty));
        }
        if let Some(info) = self
            .flow
            .loop_stack
            .iter()
            .find(|i| i.key == key && !i.types.is_empty())
        {
            let types = info.types.clone();
            let ty = self.flow_union_type(query, &types, false)?;
            return self.new_flow_type(ty, true);
        }
        let mut types = Vec::with_capacity(4);
        let mut first = None;
        let mut subtype = false;
        for &branch in branches {
            let previous = if first.is_none() {
                let previous = self.type_at_flow(query, branch)?;
                first = Some(previous);
                previous
            } else {
                self.flow.loop_stack.push(FlowLoopInfo {
                    key: key.clone(),
                    types: types.clone(),
                });
                let saved = std::mem::take(&mut self.flow.expression_cache);
                let previous = self.type_at_flow(query, branch);
                self.flow.expression_cache = saved;
                self.flow.loop_stack.pop();
                let previous = previous?;
                if let Some(&ty) = self.flow.loop_cache.get(&key) {
                    return Ok(FlowType::complete(ty));
                }
                previous
            };
            if !types.contains(&previous.ty) {
                types.push(previous.ty);
            }
            subtype |= !self.flow_type_subset(previous.ty, query.initial)?;
            if previous.ty == query.declared {
                break;
            }
        }
        let first = first.ok_or(ts_arena::Error::InvalidGraph)?;
        let ty = self.flow_union_type(query, &types, subtype)?;
        if first.incomplete {
            self.new_flow_type(ty, true)
        } else {
            self.flow.loop_cache.insert(key, ty);
            Ok(FlowType::complete(ty))
        }
    }

    // port: tsc/internal/checker/flow.go:Checker.getFlowReferenceKey
    fn flow_reference_key(&mut self, query: &FlowQuery) -> Result<Option<crate::CacheKey>, Error> {
        let mut builder = crate::key::KeyBuilder::new();
        if self.write_flow_reference_key(&mut builder, query.reference, query)? {
            Ok(Some(builder.finish()))
        } else {
            Ok(None)
        }
    }

    // port: tsc/internal/checker/flow.go:Checker.writeFlowCacheKey
    fn write_flow_reference_key(
        &mut self,
        builder: &mut crate::key::KeyBuilder,
        reference: NodeId,
        query: &FlowQuery,
    ) -> Result<bool, Error> {
        stacker::maybe_grow(128 * 1024, 2 * 1024 * 1024, || {
            let read = self.ast(reference)?.node(reference)?;
            match read.kind().known() {
                Some(K::NonNullExpression | K::ParenthesizedExpression) => self
                    .write_flow_reference_key(
                        builder,
                        required(read.expression(), "flow key operand")?,
                        query,
                    ),
                Some(K::Identifier | K::ThisKeyword) => {
                    if read.kind() == K::Identifier && !self.flow_this_type_query(reference)? {
                        let symbol = self.resolved_value_symbol(reference)?;
                        if symbol == self.builtins.unknown_symbol {
                            return Ok(false);
                        }
                        builder.write_symbol(self.symbol_runtime_id(symbol)?);
                    }
                    builder.write_byte(b':');
                    builder.write_type(query.declared);
                    if query.initial != query.declared {
                        builder.write_byte(b'=');
                        builder.write_type(query.initial);
                    }
                    if let Some(container) = query.container {
                        builder.write_byte(b'@');
                        builder.write_node_id(container.bits());
                    }
                    Ok(true)
                }
                Some(K::QualifiedName) => {
                    let data = read
                        .data_source()
                        .as_qualified_name()
                        .ok_or(ts_arena::Error::InvalidGraph)?;
                    let left = required(data.left(), "flow key qualified left")?;
                    let right = required(data.right(), "flow key qualified right")?;
                    if !self.write_flow_reference_key(builder, left, query)? {
                        return Ok(false);
                    }
                    builder.write_byte(b'.');
                    builder.write_string(self.ast(right)?.node_text(right)?.as_bytes());
                    Ok(true)
                }
                Some(K::PropertyAccessExpression | K::ElementAccessExpression) => {
                    let expression = required(read.expression(), "flow key access expression")?;
                    let argument = read
                        .data_source()
                        .as_element_access_expression()
                        .and_then(|d| d.argument_expression());
                    if let Some(name) = self.flow_property_name(reference)? {
                        if !self.write_flow_reference_key(builder, expression, query)? {
                            return Ok(false);
                        }
                        builder.write_byte(b'.');
                        builder.write_string(name.as_bytes());
                        return Ok(true);
                    }
                    if let Some(argument) = argument {
                        if self.ast(argument)?.node(argument)?.kind() == K::Identifier {
                            let symbol = self.resolved_value_symbol(argument)?;
                            if self.is_constant_flow_variable(symbol)?
                                || self.is_parameter_or_mutable_local_variable(symbol)?
                                    && !self.is_symbol_assigned(symbol)?
                            {
                                if !self.write_flow_reference_key(builder, expression, query)? {
                                    return Ok(false);
                                }
                                builder.write_string(b".@");
                                builder.write_symbol(self.symbol_runtime_id(symbol)?);
                                return Ok(true);
                            }
                        }
                    }
                    Ok(false)
                }
                Some(
                    K::ObjectBindingPattern
                    | K::ArrayBindingPattern
                    | K::FunctionDeclaration
                    | K::FunctionExpression
                    | K::ArrowFunction
                    | K::MethodDeclaration,
                ) => {
                    builder.write_node_id(reference.bits());
                    builder.write_byte(b'#');
                    builder.write_type(query.declared);
                    Ok(true)
                }
                _ => Ok(false),
            }
        })
    }

    // port: tsc/internal/checker/utilities.go:isInCompoundLikeAssignment
    pub(crate) fn compound_like_assignment(&self, node: NodeId) -> Result<bool, Error> {
        let view = self.ast(node)?;
        let Some(target) = ts_ast::get_assignment_target(view, node)? else {
            return Ok(false);
        };
        if !ts_ast::is_assignment_expression(view, target, true)? {
            return Ok(false);
        }
        let read = view.node(target)?;
        let binary = read
            .data_source()
            .as_binary_expression()
            .ok_or(ts_arena::Error::InvalidGraph)?;
        let right = ts_ast::skip_parentheses(view, required(binary.right(), "compound right")?)?;
        let read = view.node(right)?;
        let Some(binary) = read.data_source().as_binary_expression() else {
            return Ok(false);
        };
        Ok(ts_ast::is_shift_operator_or_higher(
            view.node(required(binary.operator_token(), "compound operator")?)?
                .kind(),
        ))
    }

    // port: tsc/internal/checker/relater.go:Checker.isTypeSubsetOf
    // port: tsc/internal/checker/relater.go:Checker.isTypeSubsetOfUnion
    pub(crate) fn flow_type_subset(
        &mut self,
        source: TypeId,
        target: TypeId,
    ) -> Result<bool, Error> {
        if source == target || self.types.flags(source)? & tf::NEVER != 0 {
            return Ok(true);
        }
        if self.types.flags(target)? & tf::UNION == 0 {
            return Ok(false);
        }
        if self.types.flags(source)? & tf::UNION != 0 {
            for &part in self.types.compound_types(source)?.iter() {
                if !self.contains_type(self.types.types_of(target)?, part)? {
                    return Ok(false);
                }
            }
            return Ok(true);
        }
        if self.types.flags(source)? & tf::ENUM_LIKE != 0
            && self.base_type_of_enum_like(source)? == target
        {
            return Ok(true);
        }
        self.contains_type(self.types.types_of(target)?, source)
    }

    // port: tsc/internal/checker/flow.go:Checker.getAssignmentReducedType
    fn assignment_reduced_type(
        &mut self,
        declared: TypeId,
        assigned: TypeId,
    ) -> Result<TypeId, Error> {
        if declared == assigned {
            return Ok(declared);
        }
        if self.types.flags(assigned)? & tf::NEVER != 0 {
            return Ok(assigned);
        }
        if let Some(&cached) = self.flow.assignment_reduced.get(&(declared, assigned)) {
            return Ok(cached);
        }
        let filtered = self.filter_type(declared, &mut |state, ty| {
            if state.types.flags(assigned)? & tf::UNION == 0 {
                return state.is_type_related_to(assigned, ty, crate::RelationKind::Assignable);
            }
            let parts = state.types.compound_types(assigned)?.to_vec();
            if state.contains_type(&parts, ty)? {
                return Ok(true);
            }
            for part in parts {
                if state.is_type_related_to(part, ty, crate::RelationKind::Assignable)? {
                    return Ok(true);
                }
            }
            Ok(false)
        })?;
        let reduced = if self.is_fresh_literal_type(assigned)?
            && self.types.flags(assigned)? & tf::BOOLEAN_LITERAL != 0
        {
            self.map_type(filtered, &mut |state, ty| {
                state.get_fresh_type_of_literal_type(ty).map(Some)
            })?
            .ok_or(Error::MissingLink("assignment reduction"))?
        } else {
            filtered
        };
        let result =
            if self.is_type_related_to(assigned, reduced, crate::RelationKind::Assignable)? {
                reduced
            } else {
                declared
            };
        self.flow
            .assignment_reduced
            .insert((declared, assigned), result);
        Ok(result)
    }
}

#[cfg(any(test, feature = "storage-pilot"))]
impl FlowAnalysis {
    pub(crate) fn census(&self, charge: &mut impl FnMut(&str, usize, usize, usize)) {
        charge(
            "evolvingArrayTypes",
            self.evolving.len(),
            self.evolving.allocation_size(),
            0,
        );
        charge(
            "switchClauseTypes",
            self.switches.types.len(),
            self.switches.types.allocation_size(),
            self.switches
                .types
                .values()
                .map(|types| size_of_val(types.as_ref()))
                .sum(),
        );
        charge(
            "switchClauseWitnesses",
            self.switches.witnesses.len(),
            self.switches.witnesses.allocation_size(),
            self.switches
                .witnesses
                .values()
                .flatten()
                .map(|values| {
                    size_of_val(values.as_ref())
                        + values.iter().map(|value| value.len()).sum::<usize>()
                })
                .sum(),
        );
        charge(
            "switchExhaustive",
            self.switches.exhaustive.len(),
            self.switches.exhaustive.allocation_size(),
            0,
        );
        charge(
            "flowEffectsSignatures",
            self.effects.signatures.len(),
            self.effects.signatures.allocation_size(),
            0,
        );
        charge(
            "flowEffectsResolving",
            self.effects.resolving.len(),
            self.effects.resolving.allocation_size(),
            0,
        );
        charge(
            "explicitSymbolResolving",
            self.effects.explicit_symbols.len(),
            self.effects.explicit_symbols.allocation_size(),
            0,
        );
        charge(
            "symbolNarrowingParents",
            self.symbol_narrowing_parents.len(),
            self.symbol_narrowing_parents.allocation_size(),
            0,
        );
        charge(
            "syntheticFlowReferences",
            self.synthetic.len(),
            self.synthetic.allocation_size(),
            0,
        );
        charge(
            "flowPostSuper",
            self.post_super.len(),
            self.post_super.allocation_size(),
            0,
        );
        charge(
            "flowReachable",
            self.reachable.len(),
            self.reachable.allocation_size(),
            0,
        );
        charge(
            "markedAssignmentSymbolLinks",
            self.assignments.symbols.len(),
            self.assignments.symbols.structural_bytes(),
            0,
        );
        charge(
            "assignmentMarkedRoots",
            self.assignments.roots.len(),
            self.assignments.roots.allocation_size(),
            0,
        );
        charge(
            "flowLoopCache",
            self.loop_cache.len(),
            self.loop_cache.allocation_size(),
            self.loop_cache.keys().map(|key| key.reference.len()).sum(),
        );
        charge(
            "flowLoopStack",
            self.loop_stack.len(),
            self.loop_stack.capacity() * size_of::<FlowLoopInfo>(),
            self.loop_stack
                .iter()
                .map(|info| info.key.reference.len() + info.types.capacity() * size_of::<TypeId>())
                .sum(),
        );
        charge(
            "flowTypeCache",
            self.expression_cache.len(),
            self.expression_cache.allocation_size(),
            0,
        );
        charge(
            "narrowedTypes",
            self.narrowed.len(),
            self.narrowed.allocation_size(),
            0,
        );
        charge(
            "assignmentReducedTypes",
            self.assignment_reduced.len(),
            self.assignment_reduced.allocation_size(),
            0,
        );
    }
    pub(crate) fn signature_roots(&self) -> Vec<crate::SignatureId> {
        self.effects
            .signatures
            .values()
            .filter_map(|result| result.as_ref().ok().copied().flatten())
            .collect()
    }
    /// Observer-only root projection. Every retained type in a key, temporary
    /// frame or completed cache is a root; arena vectors are not roots.
    pub(crate) fn type_roots(&self) -> Vec<TypeId> {
        let mut types = Vec::new();
        for (&element, &evolving) in &self.evolving {
            types.extend([element, evolving]);
        }
        for values in self.switches.types.values() {
            types.extend(values.iter());
        }
        for (key, &value) in &self.loop_cache {
            types.extend([key.declared, key.initial, value]);
        }
        for info in &self.loop_stack {
            types.extend([info.key.declared, info.key.initial]);
            types.extend(&info.types);
        }
        types.extend(self.expression_cache.values());
        for (&(ty, candidate, _, _), &result) in &self.narrowed {
            types.extend([ty, candidate, result]);
        }
        for (&(declared, assigned), &reduced) in &self.assignment_reduced {
            types.extend([declared, assigned, reduced]);
        }
        types
    }
}

impl CheckerState {
    // port: tsc/internal/checker/flow.go:Checker.reportFlowControlError
    fn report_flow_control_error(&mut self, node: NodeId) -> Result<(), Error> {
        let view = self.ast(node)?;
        let block = ts_ast::utilities::find_ancestor(view, Some(node), |candidate| {
            ts_ast::utilities::is_function_or_module_block(view, candidate.id()).unwrap_or(false)
        })?
        .ok_or(Error::MissingLink("flow control block"))?;
        let source = ts_ast::utilities::get_source_file_of_node(view, Some(node))?
            .ok_or(Error::MissingLink("flow control source"))?;
        let statements = view
            .node(block)?
            .statement_list()
            .ok_or(Error::MissingLink("flow control statements"))?;
        let position = view.list(statements)?.loc().pos();
        let range = ts_scanner::get_range_of_token_at_position(view, source, position)?;
        self.add_diagnostic(ts_ast::Diagnostic::new(
            Some(source),
            range,
            ts_diagnostics::The_containing_function_or_module_body_is_too_large_for_control_flow_analysis,
            vec![],
        ))?;
        Ok(())
    }
}
