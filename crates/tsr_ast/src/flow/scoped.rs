//! Immutable local graph traversal, available only after binding publication.
//!
//! External identities are checked once on import. Graph edges were validated
//! by publication and retain safe slot bounds reads without reconstructing owner
//! identities. The callback borrows the completed graph; handles cannot escape
//! or be used with another graph's scope.
//!
//! ```compile_fail
//! fn escape(view: tsr_ast::BoundView<'_>, id: tsr_ast::FlowId) {
//!     let _ = view.with_flow_scope(|scope| scope.import_flow(id).unwrap());
//! }
//! ```
//!
//! ```compile_fail
//! fn cross_graph(a: tsr_ast::BoundView<'_>, b: tsr_ast::BoundView<'_>, id: tsr_ast::FlowId) {
//!     a.with_flow_scope(|a| {
//!         let flow = a.import_flow(id).unwrap();
//!         b.with_flow_scope(|b| { b.node(flow); });
//!     });
//! }
//! ```

use super::{FlowData, FlowFlags, FlowId, FlowListId, FlowListRead, FlowNodeRead};
use crate::{BindResult, BoundView};
use std::{marker::PhantomData, num::NonZeroU32};
use tsr_arena::Error;

type Brand<'scope> = PhantomData<fn(&'scope ()) -> &'scope ()>;

macro_rules! local_id {
    ($name:ident) => {
        /// A non-owning graph identity that belongs to one completed-flow scope.
        #[derive(Clone, Copy, Debug, PartialEq, Eq)]
        pub struct $name<'scope> {
            slot: NonZeroU32,
            brand: Brand<'scope>,
        }
        impl $name<'_> {
            fn from_slot(slot: u32) -> Option<Self> {
                NonZeroU32::new(slot).map(|slot| Self {
                    slot,
                    brand: PhantomData,
                })
            }
        }
    };
}
local_id!(LocalFlow);
local_id!(LocalFlowList);

/// Borrowed completed storage; creating a scope does not scan or allocate.
pub struct FlowScope<'scope, 'owner> {
    result: &'owner BindResult,
    brand: Brand<'scope>,
}

impl<'owner> BoundView<'owner> {
    pub fn with_flow_scope<R>(
        self,
        operation: impl for<'scope> FnOnce(FlowScope<'scope, 'owner>) -> R,
    ) -> R {
        operation(FlowScope {
            result: self.result(),
            brand: PhantomData,
        })
    }
}

impl<'scope, 'owner> FlowScope<'scope, 'owner> {
    pub fn import_flow(&self, id: FlowId) -> Result<LocalFlow<'scope>, Error> {
        self.result.flows().get(id)?;
        Ok(LocalFlow::from_slot(id.slot()).expect("checked flow slot is nonzero"))
    }

    pub fn import_list(&self, id: FlowListId) -> Result<LocalFlowList<'scope>, Error> {
        self.result.flow_lists().get(id)?;
        Ok(LocalFlowList::from_slot(id.slot()).expect("checked flow-list slot is nonzero"))
    }

    pub fn flow_id(&self, id: LocalFlow<'scope>) -> FlowId {
        FlowId::from_parts(self.result.flows().id(), id.slot.get())
            .expect("local flow slot is nonzero")
    }

    pub fn node(&self, id: LocalFlow<'scope>) -> LocalFlowNodeRead<'scope, 'owner> {
        LocalFlowNodeRead {
            read: self
                .result
                .flows()
                .get_slot(id.slot.get())
                .expect("completed flow edge or checked import is published"),
            brand: PhantomData,
        }
    }

    pub fn list(&self, id: LocalFlowList<'scope>) -> LocalFlowListRead<'scope, 'owner> {
        LocalFlowListRead {
            read: self
                .result
                .flow_lists()
                .get_slot(id.slot.get())
                .expect("completed flow-list edge or checked import is published"),
            brand: PhantomData,
        }
    }
}

#[derive(Clone, Copy)]
pub struct LocalFlowNodeRead<'scope, 'owner> {
    read: FlowNodeRead<'owner>,
    brand: Brand<'scope>,
}

impl<'scope> LocalFlowNodeRead<'scope, '_> {
    pub fn flags(self) -> FlowFlags {
        self.read.flags()
    }
    pub fn node(self) -> Option<FlowData> {
        self.read.node()
    }
    pub fn antecedent(self) -> Option<LocalFlow<'scope>> {
        LocalFlow::from_slot(self.read.local_antecedent_slot())
    }
    pub fn antecedents(self) -> Option<LocalFlowList<'scope>> {
        LocalFlowList::from_slot(self.read.local_antecedents_slot())
    }
}

#[derive(Clone, Copy)]
pub struct LocalFlowListRead<'scope, 'owner> {
    read: FlowListRead<'owner>,
    brand: Brand<'scope>,
}

impl<'scope> LocalFlowListRead<'scope, '_> {
    pub fn flow(self) -> Option<LocalFlow<'scope>> {
        LocalFlow::from_slot(self.read.local_flow_slot())
    }
    pub fn next(self) -> Option<LocalFlowList<'scope>> {
        LocalFlowList::from_slot(self.read.local_next_slot())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        flow_flags, AstBuilder, CompletedFile, FlowList, FlowNode, SourceFileParseOptions,
    };
    use tsr_arena::Counters;
    use tsr_jsstring::SourceText;

    fn completed(counters: &Counters) -> (CompletedFile, FlowId, FlowListId) {
        let text = SourceText::default();
        let mut builder = AstBuilder::new(text.clone(), counters);
        let source = builder.new_source_file(
            SourceFileParseOptions {
                file_name: crate::JsString::from_bytes(b"/scoped-flow.ts".as_slice()),
                ..SourceFileParseOptions::default()
            },
            text,
            None,
            None,
        );
        let mut ids = None;
        let completed = builder
            .complete(source)
            .unwrap()
            .bind_and_publish(|binding| {
                let start = binding.flows_mut().push(FlowNode::new(flow_flags::START));
                let list = binding.flow_lists_mut().push(FlowList {
                    flow: Some(start),
                    next: None,
                });
                let branch = binding.flows_mut().push(FlowNode {
                    flags: flow_flags::BRANCH_LABEL,
                    antecedents: Some(list),
                    ..FlowNode::default()
                });
                ids = Some((branch, list));
                Ok(())
            })
            .unwrap();
        let (flow, list) = ids.unwrap();
        (completed, flow, list)
    }

    #[test]
    fn completed_scope_preserves_links_and_rejects_foreign_or_unpublished_imports() {
        let counters = Counters::new();
        let (file, flow, list) = completed(&counters);
        let (_, foreign_flow, foreign_list) = completed(&counters);
        file.view().with_flow_scope(|scope| {
            assert_eq!(scope.import_flow(foreign_flow), Err(Error::WrongOwner));
            assert_eq!(scope.import_list(foreign_list), Err(Error::WrongOwner));
            assert!(scope
                .import_flow(FlowId::from_parts(flow.arena(), 999).unwrap())
                .is_err());
            assert!(scope
                .import_list(FlowListId::from_parts(list.arena(), 999).unwrap())
                .is_err());
            let local = scope.import_flow(flow).unwrap();
            assert_eq!(scope.flow_id(local), flow);
            let branch = scope.node(local);
            assert_eq!(branch.flags(), flow_flags::BRANCH_LABEL);
            let head = scope.list(branch.antecedents().unwrap());
            assert!(head.next().is_none());
            assert_eq!(scope.node(head.flow().unwrap()).flags(), flow_flags::START);
        });
    }

    #[test]
    fn publication_rejects_foreign_reduction_targets_before_local_scope_exists() {
        let counters = Counters::new();
        let (_, foreign_flow, _) = completed(&counters);
        let text = SourceText::default();
        let mut builder = AstBuilder::new(text.clone(), &counters);
        let source = builder.new_source_file(
            SourceFileParseOptions {
                file_name: crate::JsString::from_bytes(b"/scoped-flow.ts".as_slice()),
                ..SourceFileParseOptions::default()
            },
            text,
            None,
            None,
        );
        let result = builder
            .complete(source)
            .unwrap()
            .bind_and_publish(|binding| {
                binding.flows_mut().push(FlowNode::new_ex(
                    flow_flags::REDUCE_LABEL,
                    Some(FlowData::ReduceLabel(crate::FlowReduceLabelData::new(
                        Some(foreign_flow),
                        None,
                    ))),
                    None,
                ));
                Ok(())
            });
        assert!(result.is_err());
    }
}
