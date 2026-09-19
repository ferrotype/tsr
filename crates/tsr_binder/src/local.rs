//! Binder helpers whose child links stay in the exclusive owner's namespace.
use crate::need;
use crate::{backend::Backend, Binder};
use std::ops::ControlFlow;
use ts_ast::local_bind::{BindEdges, BindList, BindSlice, LocalChildVisitor};
use ts_ast::{local_bind::BindNode, SyntaxKind as K};

#[derive(Clone, Copy)]
enum Child<'scope> {
    Node(BindNode<'scope>),
    List(BindList<'scope>),
    Slice(BindSlice<'scope>),
}
struct Children<'scope> {
    entries: [Option<Child<'scope>>; 9],
    len: usize,
}
impl<'scope> Children<'scope> {
    fn push(&mut self, child: Child<'scope>) -> ControlFlow<()> {
        self.entries[self.len] = Some(child);
        self.len += 1;
        ControlFlow::Continue(())
    }
}
impl<'scope> LocalChildVisitor<'scope> for Children<'scope> {
    fn visit_node(&mut self, node: BindNode<'scope>) -> ControlFlow<()> {
        self.push(Child::Node(node))
    }
    fn visit_list(&mut self, list: BindList<'scope>) -> ControlFlow<()> {
        self.push(Child::List(list))
    }
    fn visit_node_slice(&mut self, slice: BindSlice<'scope>) -> ControlFlow<()> {
        self.push(Child::Slice(slice))
    }
}

impl<'scope> Binder<'_, 'scope, '_> {
    pub(crate) fn bind_local_entry(&mut self, node: BindNode<'scope>) -> bool {
        crate::recursion::guarded(|| {
            let Backend::Local(local) = &self.builder else {
                unreachable!("local binder scope");
            };
            let read = local.node(node);
            let flags = read.flags();
            if read.kind() == K::Identifier && read.as_identifier().is_some() {
                if !self
                    .try_set_target_flow(crate::target::BindingNode::Local(node), self.current_flow)
                {
                    let flow = self.current_flow.map(|flow| self.flow_id(flow));
                    let Backend::Local(local) = &mut self.builder else {
                        unreachable!("local binder scope");
                    };
                    let id = local.node_id(node);
                    local
                        .general_set_node_flow(id, flow)
                        .expect("binder flow and target belong to result");
                }
                self.check_local_contextual_identifier(node);
                if flags & ts_ast::node_flags::THIS_NODE_HAS_ERROR != 0 {
                    let Backend::Local(local) = &mut self.builder else {
                        unreachable!("local binder scope");
                    };
                    local.set_flags(
                        node,
                        flags | ts_ast::node_flags::THIS_NODE_OR_ANY_SUB_NODES_HAS_ERROR,
                    );
                    self.seen_parse_error = true;
                }
                return false;
            }
            self.bind_worker_target(crate::target::BindingNode::Local(node))
        })
    }

    pub(crate) fn bind_local_children(&mut self, node: BindNode<'scope>) {
        let Backend::Local(local) = &self.builder else {
            unreachable!("local binder scope");
        };
        let mut children = Children {
            entries: [None; 9],
            len: 0,
        };
        let _ = local.node(node).for_each_child(&mut children);
        for child in children.entries[..children.len].iter().flatten() {
            if let Child::Node(node) = *child {
                if self.bind_local_entry(node) {
                    break;
                }
                continue;
            }
            let Backend::Local(local) = &self.builder else {
                unreachable!("local binder scope");
            };
            let slice = match *child {
                Child::List(list) => local.list(list),
                Child::Slice(slice) => slice,
                Child::Node(_) => unreachable!(),
            };
            let edges = local.edges(slice);
            if self.bind_local_edges(edges) {
                return;
            }
        }
    }

    pub(crate) fn bind_local_edges(&mut self, edges: BindEdges<'scope>) -> bool {
        for index in 0..edges.len() {
            let Backend::Local(local) = &self.builder else {
                unreachable!("local binder scope");
            };
            if let Some(node) = local.edge(edges, index) {
                if self.bind_local_entry(node) {
                    return true;
                }
            }
        }
        false
    }

    pub(crate) fn bind_local_source_children(&mut self, node: BindNode<'scope>) {
        let Backend::Local(local) = &self.builder else {
            unreachable!("local binder scope");
        };
        let source = local
            .node(node)
            .as_source_file()
            .expect("SourceFile payload");
        let statements = source.statements();
        let eof = source.end_of_file_token();
        self.bind_local_statements_functions_first(need(statements));
        if let Some(eof) = eof {
            self.bind_local_entry(eof);
        }
    }

    pub(crate) fn bind_local_block_children(&mut self, node: BindNode<'scope>) {
        let Backend::Local(local) = &self.builder else {
            unreachable!("local binder scope");
        };
        let read = local.node(node);
        let statements = match read.kind().known() {
            Some(K::Block) => read.as_block().map(|data| data.statements()),
            Some(K::ModuleBlock) => read.as_module_block().map(|data| data.statements()),
            _ => unreachable!("block child dispatch"),
        }
        .unwrap_or_else(|| {
            // Constructed kind/shape mismatches preserve the checked accessor's
            // interface-conversion failure rather than a new local panic.
            local
                .general_node(local.node_id(node))
                .expect("binder node is retained")
                .statement_list()
                .map(|list| local.import_list(list).expect("validated block list"))
        });
        self.bind_local_statements_functions_first(need(statements));
    }

    pub(crate) fn bind_local_statements_functions_first(&mut self, statements: BindList<'scope>) {
        let Backend::Local(local) = &self.builder else {
            unreachable!("local binder scope");
        };
        let edges = local.edges(local.list(statements));
        // Both passes borrow the same resolved, immutable edge range. A narrow
        // binding write can change flags, but cannot alter statement membership.
        for functions in [true, false] {
            for index in 0..edges.len() {
                let Backend::Local(local) = &self.builder else {
                    unreachable!("local binder scope");
                };
                let node = need(local.edge(edges, index));
                if (local.node(node).kind() == K::FunctionDeclaration) == functions {
                    self.bind_local_entry(node);
                }
            }
        }
    }
}
