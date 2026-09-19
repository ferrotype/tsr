//! Local syntax identities stay branded through dispatch and recursive walks.
//! Unmigrated helpers cross the explicit raw-ID boundary below.
use crate::{backend::Backend, Binder};
use ts_ast::{
    local_bind::{BindEdges, BindList, BindNode},
    NodeId, NodeKind, NodeListId, NodeSlice,
};

#[derive(Clone, Copy)]
pub(crate) enum BindingNode<'scope> {
    Local(BindNode<'scope>),
    Checked(NodeId),
}

impl<'scope> Binder<'_, 'scope, '_> {
    pub(crate) fn binding_node(&self, id: NodeId) -> BindingNode<'scope> {
        if let Backend::Local(local) = &self.builder {
            if let Ok(node) = local.import_node(id) {
                return BindingNode::Local(node);
            }
        }
        BindingNode::Checked(id)
    }

    /// An explicit compatibility boundary; local dispatch does not re-import it.
    pub(crate) fn node_id(&self, node: BindingNode<'scope>) -> NodeId {
        match node {
            BindingNode::Local(node) => {
                let Backend::Local(local) = &self.builder else {
                    unreachable!("local binder scope");
                };
                local.node_id(node)
            }
            BindingNode::Checked(id) => id,
        }
    }

    pub(crate) fn node_kind(&self, node: BindingNode<'scope>) -> NodeKind {
        match node {
            BindingNode::Local(node) => {
                let Backend::Local(local) = &self.builder else {
                    unreachable!("local binder scope");
                };
                local.node(node).kind()
            }
            BindingNode::Checked(id) => self.n(id).kind(),
        }
    }

    pub(crate) fn node_flags(&self, node: BindingNode<'scope>) -> u32 {
        match node {
            BindingNode::Local(node) => {
                let Backend::Local(local) = &self.builder else {
                    unreachable!("local binder scope");
                };
                local.node(node).flags()
            }
            BindingNode::Checked(id) => self.n(id).flags(),
        }
    }

    pub(crate) fn set_binding_flags(&mut self, node: BindingNode<'scope>, flags: u32) {
        match node {
            BindingNode::Local(node) => {
                let Backend::Local(local) = &mut self.builder else {
                    unreachable!("local binder scope");
                };
                if local.node(node).flags() != flags {
                    local.set_flags(node, flags);
                }
            }
            BindingNode::Checked(id) => self.set_flags(id, flags),
        }
    }

    pub(crate) fn container_flags(&self, node: BindingNode<'scope>) -> crate::ContainerFlags {
        match node {
            BindingNode::Local(node) => {
                let Backend::Local(local) = &self.builder else {
                    unreachable!("local binder scope");
                };
                crate::container_classification::local_container_flags(local, node)
            }
            BindingNode::Checked(id) => crate::checked(crate::get_container_flags(self.view(), id)),
        }
    }
}

/// A list identity stays local until an explicit compatibility boundary.
#[derive(Clone, Copy)]
pub(crate) enum BindingList<'scope> {
    Local(BindList<'scope>),
    Checked(NodeListId),
}
#[derive(Clone, Copy)]
pub(crate) enum BindingEdges<'scope> {
    Local(BindEdges<'scope>),
    Checked(NodeSlice),
}
impl BindingEdges<'_> {
    pub(crate) fn len(self) -> usize {
        match self {
            Self::Local(edges) => edges.len(),
            Self::Checked(edges) => edges.len(),
        }
    }
    pub(crate) fn is_empty(self) -> bool {
        self.len() == 0
    }
}

// Both adapters select the same typed accessor and snapshot only the fields
// needed across mutation. Local edges remain branded; no owned payload is built.
macro_rules! target_payload {
    ($binder:expr, $node:expr, $accessor:ident; $($kind:ident: $field:ident),+ $(,)?) => {
        target_payload!($binder, $node, $accessor, "binder syntax payload"; $($kind: $field),+)
    };
    ($binder:expr, $node:expr, $accessor:ident, $message:literal; $($kind:ident: $field:ident),+ $(,)?) => {{
        let binder = &$binder;
        match $node {
            crate::target::BindingNode::Local(node) => {
                let crate::backend::Backend::Local(local) = &binder.builder else {
                    unreachable!("local binder scope")
                };
                let read = local.node(node);
                let data = read.$accessor().expect($message);
                ($(target_payload!(@local $kind data.$field()),)+)
            }
            crate::target::BindingNode::Checked(node) => {
                let read = binder.n(node);
                let data = read.$accessor().expect($message);
                ($(target_payload!(@checked $kind data.$field()),)+)
            }
        }
    }};
    (@local node $value:expr) => { $value.map(crate::target::BindingNode::Local) };
    (@checked node $value:expr) => { $value.map(crate::target::BindingNode::Checked) };
    (@local list $value:expr) => { $value.map(crate::target::BindingList::Local) };
    (@checked list $value:expr) => { $value.map(crate::target::BindingList::Checked) };
    (@local scalar $value:expr) => { $value };
    (@checked scalar $value:expr) => { $value };
}
pub(crate) use target_payload;

macro_rules! node_links {
    ($($name:ident => $getter:ident),+ $(,)?) => {$ (
        pub(crate) fn $name(&self, node: BindingNode<'scope>) -> Option<BindingNode<'scope>> {
            match node {
                BindingNode::Local(node) => {
                    let Backend::Local(local) = &self.builder else { unreachable!("local binder scope") };
                    local.node(node).$getter().map(BindingNode::Local)
                }
                BindingNode::Checked(node) => self.n(node).$getter().map(BindingNode::Checked),
            }
        }
    )+};
}
macro_rules! list_links {
    ($($name:ident => $getter:ident),+ $(,)?) => {$ (
        pub(crate) fn $name(&self, node: BindingNode<'scope>) -> Option<BindingList<'scope>> {
            match node {
                BindingNode::Local(node) => {
                    let Backend::Local(local) = &self.builder else { unreachable!("local binder scope") };
                    local.node(node).$getter().map(BindingList::Local)
                }
                BindingNode::Checked(node) => self.n(node).$getter().map(BindingList::Checked),
            }
        }
    )+};
}
impl<'scope> Binder<'_, 'scope, '_> {
    node_links! { node_parent => parent, node_name => name, node_expression => expression,
    node_initializer => initializer, node_label => label, node_question_dot_token => question_dot_token,
    node_type => type_node, node_postfix_token => postfix_token }
    list_links! { node_element_list => element_list, node_property_list => property_list,
    node_statement_list => statement_list,
    node_parameter_list => parameter_list }

    pub(crate) fn same_node(
        &self,
        left: Option<BindingNode<'scope>>,
        right: Option<BindingNode<'scope>>,
    ) -> bool {
        match (left, right) {
            (None, None) => true,
            (Some(BindingNode::Local(left)), Some(BindingNode::Local(right))) => left == right,
            (Some(left), Some(right)) => self.node_id(left) == self.node_id(right),
            _ => false,
        }
    }
    pub(crate) fn target_list_edges(
        &self,
        list: Option<BindingList<'scope>>,
    ) -> BindingEdges<'scope> {
        match list {
            Some(BindingList::Local(list)) => {
                let Backend::Local(local) = &self.builder else {
                    unreachable!("local binder scope")
                };
                BindingEdges::Local(local.edges(local.list(list)))
            }
            Some(BindingList::Checked(list)) => {
                BindingEdges::Checked(self.syntax_slice(Some(list)))
            }
            None => BindingEdges::Checked(NodeSlice::empty()),
        }
    }
    pub(crate) fn edge(
        &self,
        edges: BindingEdges<'scope>,
        index: usize,
    ) -> Option<BindingNode<'scope>> {
        match edges {
            BindingEdges::Local(edges) => {
                let Backend::Local(local) = &self.builder else {
                    unreachable!("local binder scope")
                };
                local.edge(edges, index).map(BindingNode::Local)
            }
            BindingEdges::Checked(edges) => {
                self.syntax_node(edges, index).map(BindingNode::Checked)
            }
        }
    }
    pub(crate) fn bind_optional_target(&mut self, node: Option<BindingNode<'scope>>) -> bool {
        node.is_some_and(|node| self.bind_target(node))
    }
    // port: tsc/internal/binder/binder.go:Binder.bindEach
    // port: tsc/internal/binder/binder.go:Binder.bindNodeList
    // port: tsc/internal/binder/binder.go:Binder.bindModifiers
    pub(crate) fn bind_target_list(&mut self, list: Option<BindingList<'scope>>) {
        let edges = self.target_list_edges(list);
        for index in 0..edges.len() {
            self.bind_optional_target(self.edge(edges, index));
        }
    }
    pub(crate) fn target_name_of_declaration(
        &self,
        node: Option<BindingNode<'scope>>,
    ) -> Option<BindingNode<'scope>> {
        let node = node?;
        match node {
            BindingNode::Local(node) => {
                let Backend::Local(local) = &self.builder else {
                    unreachable!("local binder scope")
                };
                crate::checked(local.get_name_of_declaration(Some(node))).map(BindingNode::Local)
            }
            BindingNode::Checked(node) => {
                crate::checked(ts_ast::get_name_of_declaration(self.view(), Some(node)))
                    .map(|name| self.binding_node(name))
            }
        }
    }
    pub(crate) fn target_has_dynamic_name(&self, node: Option<BindingNode<'scope>>) -> bool {
        match node {
            Some(BindingNode::Local(node)) => {
                let Backend::Local(local) = &self.builder else {
                    unreachable!("local binder scope")
                };
                crate::checked(local.has_dynamic_name(Some(node)))
            }
            node => crate::checked(ts_ast::has_dynamic_name(
                self.view(),
                node.map(|node| self.node_id(node)),
            )),
        }
    }
    pub(crate) fn target_is_ambient_module(&self, node: BindingNode<'scope>) -> bool {
        match node {
            BindingNode::Local(node) => {
                let Backend::Local(local) = &self.builder else {
                    unreachable!("local binder scope")
                };
                local.is_ambient_module(node)
            }
            BindingNode::Checked(node) => {
                crate::checked(ts_ast::is_ambient_module(self.view(), node))
            }
        }
    }
    pub(crate) fn target_has_syntactic_modifier(
        &self,
        node: BindingNode<'scope>,
        flags: u32,
    ) -> bool {
        match node {
            BindingNode::Local(node) => {
                let Backend::Local(local) = &self.builder else {
                    unreachable!("local binder scope")
                };
                local.has_syntactic_modifier(node, flags)
            }
            BindingNode::Checked(node) => crate::checked(
                ts_ast::utilities::has_syntactic_modifier(self.view(), node, flags),
            ),
        }
    }
    pub(crate) fn target_combined_modifier_flags(&self, node: BindingNode<'scope>) -> u32 {
        match node {
            BindingNode::Local(node) => {
                let Backend::Local(local) = &self.builder else {
                    unreachable!("local binder scope")
                };
                local.get_combined_modifier_flags(node)
            }
            BindingNode::Checked(node) => crate::checked(
                ts_ast::utilities::get_combined_modifier_flags(self.view(), node),
            ),
        }
    }
    pub(crate) fn target_text(&self, node: BindingNode<'scope>) -> ts_ast::JsString {
        match node {
            BindingNode::Local(node) => {
                let Backend::Local(local) = &self.builder else {
                    unreachable!("local binder scope")
                };
                // Label/name paths require identifier text. Other text shapes keep
                // the public text helper's checked shape and failure contract.
                let read = local.node(node);
                if read.kind() == ts_ast::SyntaxKind::Identifier {
                    if let Some(identifier) = read.as_identifier() {
                        return identifier.text_owned();
                    }
                }
                self.text(local.node_id(node))
            }
            BindingNode::Checked(node) => self.text(node),
        }
    }
}

macro_rules! target_predicates {
    ($($name:ident => $getter:ident in $module:path),+ $(,)?) => {$ (
        pub(crate) fn $name(&self, node: BindingNode<'scope>) -> bool {
            match node {
                BindingNode::Local(node) => {
                    let Backend::Local(local) = &self.builder else { unreachable!("local binder scope") };
                    local.$getter(node)
                }
                BindingNode::Checked(node) => {
                    use $module as helpers;
                    helpers::$getter(self.view(), node).expect("retained binder helper input")
                }
            }
        }
    )+};
}
impl<'scope> Binder<'_, 'scope, '_> {
    target_predicates! {
        target_is_dotted_name => is_dotted_name in ts_ast,
        target_is_entity_name_expression => is_entity_name_expression in ts_ast,
        target_is_left_hand_side_expression => is_left_hand_side_expression in ts_ast,
        target_is_push_or_unshift_identifier => is_push_or_unshift_identifier in ts_ast,
        target_is_outermost_optional_chain => is_outermost_optional_chain in ts_ast::utilities,
        target_is_expression_of_optional_chain_root => is_expression_of_optional_chain_root in ts_ast::utilities,
        target_is_logical_expression => is_logical_expression in ts_ast::utilities,
        target_is_logical_or_coalescing_assignment_expression => is_logical_or_coalescing_assignment_expression in ts_ast::utilities,
        target_is_nullish_coalesce => is_nullish_coalesce in ts_ast::utilities,
    }
    pub(crate) fn target_skip_parentheses(&self, node: BindingNode<'scope>) -> BindingNode<'scope> {
        match node {
            BindingNode::Local(node) => {
                let Backend::Local(local) = &self.builder else {
                    unreachable!("local binder scope")
                };
                BindingNode::Local(local.skip_parentheses(node))
            }
            BindingNode::Checked(node) => BindingNode::Checked(
                ts_ast::skip_parentheses(self.view(), node)
                    .expect("retained parenthesized expression"),
            ),
        }
    }
    pub(crate) fn target_is_optional_chain(&self, node: BindingNode<'scope>) -> bool {
        match node {
            BindingNode::Local(node) => {
                let Backend::Local(local) = &self.builder else {
                    unreachable!("local binder scope")
                };
                local.is_optional_chain(node)
            }
            BindingNode::Checked(node) => ts_ast::utilities::is_optional_chain(&self.n(node)),
        }
    }
    pub(crate) fn target_is_optional_chain_root(&self, node: BindingNode<'scope>) -> bool {
        match node {
            BindingNode::Local(node) => {
                let Backend::Local(local) = &self.builder else {
                    unreachable!("local binder scope")
                };
                local.is_optional_chain_root(node)
            }
            BindingNode::Checked(node) => ts_ast::utilities::is_optional_chain_root(&self.n(node)),
        }
    }
}
