//! Flow identities remain scoped while the private binder creates and links them.
use crate::{
    backend::{Backend, ScopedValue},
    target::BindingNode,
    Binder,
};
use ts_ast::{
    local_bind::{BindFlow, BindFlowList},
    FlowData, FlowId, FlowList, FlowListId, FlowNode, FlowNodeRead,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum BindingFlow<'scope> {
    Local(BindFlow<'scope>),
    Checked(FlowId),
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum BindingFlowList<'scope> {
    Local(BindFlowList<'scope>),
    Checked(FlowListId),
}
pub(crate) struct FlowLinks<'scope> {
    pub flow: Option<BindingFlow<'scope>>,
    pub next: Option<BindingFlowList<'scope>>,
}

impl<'scope> Binder<'_, 'scope, '_> {
    /// A checked reference can become valid after a later arena allocation.
    /// Representation equality is therefore insufficient for mixed aliases.
    pub(crate) fn same_flow(
        &self,
        left: Option<BindingFlow<'scope>>,
        right: Option<BindingFlow<'scope>>,
    ) -> bool {
        match (left, right) {
            (None, None) => true,
            (Some(BindingFlow::Local(left)), Some(BindingFlow::Local(right))) => left == right,
            (Some(left), Some(right)) => self.flow_id(left) == self.flow_id(right),
            _ => false,
        }
    }
    pub(crate) fn binding_flow(&self, id: FlowId) -> BindingFlow<'scope> {
        if let Backend::Local(local) = &self.builder {
            if let Ok(flow) = local.import_flow(id) {
                return BindingFlow::Local(flow);
            }
        }
        // Preserve a reference until the algorithm actually dereferences it.
        // Escaped/invalid links do not fail merely because a sibling was read.
        BindingFlow::Checked(id)
    }
    pub(crate) fn binding_flow_list(&self, id: FlowListId) -> BindingFlowList<'scope> {
        if let Backend::Local(local) = &self.builder {
            if let Ok(list) = local.import_flow_list(id) {
                return BindingFlowList::Local(list);
            }
        }
        BindingFlowList::Checked(id)
    }
    pub(crate) fn flow_id(&self, flow: BindingFlow<'scope>) -> FlowId {
        match flow {
            BindingFlow::Local(flow) => {
                let Backend::Local(local) = &self.builder else {
                    unreachable!("local binder scope");
                };
                local.flow_id(flow)
            }
            BindingFlow::Checked(id) => id,
        }
    }
    pub(crate) fn flow_list_id(&self, list: BindingFlowList<'scope>) -> FlowListId {
        match list {
            BindingFlowList::Local(list) => {
                let Backend::Local(local) = &self.builder else {
                    unreachable!("local binder scope");
                };
                local.flow_list_id(list)
            }
            BindingFlowList::Checked(id) => id,
        }
    }
    pub(crate) fn flow(&self, flow: BindingFlow<'scope>) -> FlowNodeRead<'_> {
        match flow {
            BindingFlow::Local(flow) => {
                let Backend::Local(local) = &self.builder else {
                    unreachable!("local binder scope");
                };
                local.flow(flow)
            }
            BindingFlow::Checked(id) => self
                .builder
                .flows()
                .get(id)
                .expect("binder flow belongs to result"),
        }
    }
    pub(crate) fn flow_antecedents(
        &self,
        flow: BindingFlow<'scope>,
    ) -> Option<BindingFlowList<'scope>> {
        self.flow(flow)
            .antecedents()
            .map(|id| self.binding_flow_list(id))
    }
    #[cfg(test)]
    pub(crate) fn flow_antecedent(&self, flow: BindingFlow<'scope>) -> Option<BindingFlow<'scope>> {
        self.flow(flow).antecedent().map(|id| self.binding_flow(id))
    }
    pub(crate) fn flow_list(&self, list: BindingFlowList<'scope>) -> FlowLinks<'scope> {
        let read = match list {
            BindingFlowList::Local(list) => {
                let Backend::Local(local) = &self.builder else {
                    unreachable!("local binder scope");
                };
                local.flow_list(list)
            }
            BindingFlowList::Checked(id) => self
                .builder
                .flow_lists()
                .get(id)
                .expect("binder flow list belongs to result"),
        };
        FlowLinks {
            flow: read.flow().map(|id| self.binding_flow(id)),
            next: read.next().map(|id| self.binding_flow_list(id)),
        }
    }
    pub(crate) fn flow_flags_mut(&mut self, flow: BindingFlow<'scope>) -> &mut u32 {
        match flow {
            BindingFlow::Local(flow) => {
                let Backend::Local(local) = &mut self.builder else {
                    unreachable!("local binder scope");
                };
                local.local_flow_flags_mut(flow)
            }
            BindingFlow::Checked(id) => self
                .builder
                .flow_flags_mut(id)
                .expect("binder flow belongs to result"),
        }
    }
    pub(crate) fn set_flow_data(&mut self, flow: BindingFlow<'scope>, value: Option<FlowData>) {
        match flow {
            BindingFlow::Local(flow) => {
                let Backend::Local(local) = &mut self.builder else {
                    unreachable!("local binder scope");
                };
                local.local_set_flow_data(flow, value);
            }
            BindingFlow::Checked(id) => self
                .builder
                .set_flow_data(id, value)
                .expect("binder flow belongs to result"),
        }
    }
    pub(crate) fn set_flow_antecedents(
        &mut self,
        flow: BindingFlow<'scope>,
        value: Option<BindingFlowList<'scope>>,
    ) {
        if let BindingFlow::Local(flow) = flow {
            if let ScopedValue::Local(value) = local_list(value) {
                let Backend::Local(local) = &mut self.builder else {
                    unreachable!("local binder scope");
                };
                local.local_set_flow_antecedents(flow, value);
                return;
            }
        }
        let raw_flow = self.flow_id(flow);
        let raw_value = value.map(|value| self.flow_list_id(value));
        self.builder
            .set_flow_antecedents(raw_flow, raw_value)
            .expect("binder flow belongs to result");
    }
    pub(crate) fn set_flow_list_next(
        &mut self,
        list: BindingFlowList<'scope>,
        next: Option<BindingFlowList<'scope>>,
    ) {
        if let BindingFlowList::Local(list) = list {
            if let ScopedValue::Local(next) = local_list(next) {
                let Backend::Local(local) = &mut self.builder else {
                    unreachable!("local binder scope");
                };
                local.local_set_flow_list_next(list, next);
                return;
            }
        }
        let raw_list = self.flow_list_id(list);
        let raw_next = next.map(|next| self.flow_list_id(next));
        self.builder
            .set_flow_list_next(raw_list, raw_next)
            .expect("retained flow list");
    }
    // port: tsc/internal/binder/binder.go:Binder.newFlowNodeEx
    pub(crate) fn new_flow_node_ex(
        &mut self,
        flags: u32,
        data: Option<FlowData>,
        antecedent: Option<BindingFlow<'scope>>,
    ) -> BindingFlow<'scope> {
        if let ScopedValue::Local(antecedent) = local_flow(antecedent) {
            if let Backend::Local(local) = &mut self.builder {
                return BindingFlow::Local(local.new_flow(flags, data, antecedent));
            }
        }
        let raw_antecedent = antecedent.map(|flow| self.flow_id(flow));
        let id = self
            .builder
            .push_flow(FlowNode::new_ex(flags, data, raw_antecedent));
        self.binding_flow(id)
    }
    // port: tsc/internal/binder/binder.go:Binder.newFlowList
    pub(crate) fn new_flow_list(
        &mut self,
        flow: Option<BindingFlow<'scope>>,
        next: Option<BindingFlowList<'scope>>,
    ) -> BindingFlowList<'scope> {
        if let (ScopedValue::Local(flow), ScopedValue::Local(next)) =
            (local_flow(flow), local_list(next))
        {
            if let Backend::Local(local) = &mut self.builder {
                return BindingFlowList::Local(local.new_flow_list(flow, next));
            }
        }
        let flow = flow.map(|flow| self.flow_id(flow));
        let next = next.map(|next| self.flow_list_id(next));
        let id = self.builder.push_flow_list(FlowList { flow, next });
        self.binding_flow_list(id)
    }
    pub(crate) fn try_set_target_flow(
        &mut self,
        node: BindingNode<'scope>,
        flow: Option<BindingFlow<'scope>>,
    ) -> bool {
        let BindingNode::Local(node) = node else {
            return false;
        };
        let Backend::Local(local) = &mut self.builder else {
            unreachable!("local binder scope");
        };
        if !local.node(node).has_flow_node() {
            return true;
        }
        let ScopedValue::Local(flow) = local_flow(flow) else {
            return false;
        };
        assert!(
            local.set_flow(node, flow),
            "generated flow capability and writer agree"
        );
        true
    }
}

fn local_flow(value: Option<BindingFlow<'_>>) -> ScopedValue<BindFlow<'_>> {
    match value {
        None => ScopedValue::Local(None),
        Some(BindingFlow::Local(value)) => ScopedValue::Local(Some(value)),
        Some(BindingFlow::Checked(_)) => ScopedValue::Checked,
    }
}
fn local_list(value: Option<BindingFlowList<'_>>) -> ScopedValue<BindFlowList<'_>> {
    match value {
        None => ScopedValue::Local(None),
        Some(BindingFlowList::Local(value)) => ScopedValue::Local(Some(value)),
        Some(BindingFlowList::Checked(_)) => ScopedValue::Checked,
    }
}

#[cfg(test)]
#[path = "flow_access_tests.rs"]
mod tests;
