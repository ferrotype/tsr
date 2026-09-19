use crate::{
    modifier_flags, AstBuilder, AstTransaction, ChildRole, Factory, FactoryMethods, NodeData,
    NodeId, NodeKind, NodeList, NodeListId, NodeListRead, NodeRead, NodeSlice, NodeSliceRead,
    SyntaxKind, VisitContext, VisitorMethods,
};
use ts_core::TextRange;

/// Exclusive list and source-file operations needed by transformations. Source
/// metadata creation belongs to a complete builder, not a lazy JSDoc transaction.
pub trait RuntimeFactory: Factory {
    fn read_list(&self, id: NodeListId) -> NodeListRead<'_>;
    fn read_nodes(&self, nodes: NodeSlice) -> NodeSliceRead<'_>;
    fn alloc_nodes(&mut self, nodes: Vec<Option<NodeId>>) -> NodeSlice;
    fn alloc_list(&mut self, loc: TextRange, nodes: NodeSlice) -> NodeListId;
    fn mutable_list(&mut self, id: NodeListId) -> &mut NodeList;
    /// Read all immediate children before changing their parents. Implementors
    /// may specialize validated exclusive storage; the default preserves custom
    /// read/write dispatch and leaves parents untouched if enumeration fails.
    fn override_parent_in_immediate_children(&mut self, node: NodeId, scratch: &mut Vec<NodeId>) {
        override_parent_with_factory(self, node, scratch);
    }
    fn set_list_location(&mut self, id: NodeListId, loc: TextRange) {
        self.mutable_list(id).set_loc(loc);
    }
    fn set_list_modifier_flags(&mut self, id: NodeListId, flags: u32) {
        self.mutable_list(id).set_modifier_flags(flags);
    }
    fn clone_source(&mut self, original: NodeId) -> NodeId;
    fn update_source(
        &mut self,
        original: NodeId,
        statements: Option<NodeListId>,
        eof: Option<NodeId>,
    ) -> NodeId;

    // port: tsc/internal/ast/ast.go:NodeList.Clone
    fn clone_list_header(&mut self, original: NodeListId) -> NodeListId {
        let original = self.read_list(original);
        let (loc, nodes) = (original.loc(), original.nodes());
        self.alloc_list(loc, nodes)
    }
    // port: tsc/internal/ast/ast.go:ModifierList.Clone
    fn clone_modifier_list_header(&mut self, original: NodeListId) -> NodeListId {
        let original = self.read_list(original).to_owned();
        let new = self.alloc_list(original.loc(), original.nodes());
        *self.mutable_list(new) = original;
        new
    }
    fn list_has_trailing_comma(&self, list: NodeListId) -> bool {
        let list = self.read_list(list);
        let nodes = self.read_nodes(list.nodes());
        let Some(last) = nodes.last() else {
            return false;
        };
        i64::from(
            self.node(last.expect("nil last node in NodeList.HasTrailingComma"))
                .end(),
        ) < list.loc().end()
    }
    // port: tsc/internal/ast/ast.go:NodeFactory.NewModifierList
    fn new_modifier_list(&mut self, nodes: NodeSlice) -> NodeListId {
        let list = self.alloc_list(TextRange::new(-1, -1), nodes);
        let flags = self.modifiers_to_flags(nodes);
        self.set_list_modifier_flags(list, flags);
        list
    }
    // port: tsc/internal/ast/utilities.go:ModifiersToFlags
    fn modifiers_to_flags(&self, nodes: NodeSlice) -> u32 {
        let mut flags = 0;
        for node in self.read_nodes(nodes).iter() {
            flags |= modifier_to_flag(self.node(node.expect("nil modifier")).kind());
        }
        flags
    }
}

fn override_parent_with_factory<F: RuntimeFactory + ?Sized>(
    factory: &mut F,
    parent: NodeId,
    scratch: &mut Vec<NodeId>,
) {
    struct Children<'a, F: ?Sized> {
        factory: &'a F,
        nodes: &'a mut Vec<NodeId>,
    }
    impl<F: RuntimeFactory + ?Sized> crate::ChildVisitor for Children<'_, F> {
        fn visit_node(&mut self, node: NodeId) -> std::ops::ControlFlow<()> {
            self.nodes.push(node);
            std::ops::ControlFlow::Continue(())
        }
        fn visit_list(&mut self, list: NodeListId) -> std::ops::ControlFlow<()> {
            self.visit_node_slice(self.factory.read_list(list).nodes())
        }
        fn visit_node_slice(&mut self, nodes: NodeSlice) -> std::ops::ControlFlow<()> {
            self.nodes
                .extend(self.factory.read_nodes(nodes).iter().flatten());
            std::ops::ControlFlow::Continue(())
        }
    }
    let mut visitor = Children {
        factory,
        nodes: scratch,
    };
    let _ = visitor.factory.node(parent).for_each_child(&mut visitor);
    for child in scratch.drain(..) {
        factory.set_node_parent(child, Some(parent));
    }
}

impl RuntimeFactory for AstBuilder {
    fn override_parent_in_immediate_children(&mut self, node: NodeId, scratch: &mut Vec<NodeId>) {
        if !scratch.is_empty() || !self.override_core_parents(node) {
            override_parent_with_factory(self, node, scratch);
        }
    }
    fn read_list(&self, id: NodeListId) -> NodeListRead<'_> {
        self.view().list(id).expect("factory list")
    }
    fn read_nodes(&self, nodes: NodeSlice) -> NodeSliceRead<'_> {
        self.view().node_slice(nodes).expect("factory slice")
    }
    fn alloc_nodes(&mut self, nodes: Vec<Option<NodeId>>) -> NodeSlice {
        self.node_slice(nodes).expect("factory slice edges")
    }
    fn alloc_list(&mut self, loc: TextRange, nodes: NodeSlice) -> NodeListId {
        self.new_list(loc, nodes).expect("factory list edges")
    }
    fn mutable_list(&mut self, id: NodeListId) -> &mut NodeList {
        self.list_mut(id).expect("factory owns list")
    }
    fn set_list_location(&mut self, id: NodeListId, loc: TextRange) {
        AstBuilder::set_list_location(self, id, loc).expect("factory owns list");
    }
    fn set_list_modifier_flags(&mut self, id: NodeListId, flags: u32) {
        AstBuilder::set_list_modifier_flags(self, id, flags).expect("factory owns list");
    }
    fn clone_source(&mut self, original: NodeId) -> NodeId {
        self.clone_source_file(original)
    }
    fn update_source(
        &mut self,
        original: NodeId,
        statements: Option<NodeListId>,
        eof: Option<NodeId>,
    ) -> NodeId {
        self.update_source_file(original, statements, eof)
    }
}
impl RuntimeFactory for AstTransaction<'_, '_> {
    fn read_list(&self, id: NodeListId) -> NodeListRead<'_> {
        self.list(id).expect("transaction list")
    }
    fn read_nodes(&self, nodes: NodeSlice) -> NodeSliceRead<'_> {
        self.node_slice_read(nodes).expect("transaction slice")
    }
    fn alloc_nodes(&mut self, nodes: Vec<Option<NodeId>>) -> NodeSlice {
        self.node_slice(nodes).expect("transaction slice edges")
    }
    fn alloc_list(&mut self, loc: TextRange, nodes: NodeSlice) -> NodeListId {
        self.new_list(loc, nodes).expect("transaction list edges")
    }
    fn mutable_list(&mut self, id: NodeListId) -> &mut NodeList {
        self.list_mut(id).expect("transaction owns staged list")
    }
    fn clone_source(&mut self, _: NodeId) -> NodeId {
        panic!("lazy AST transaction cannot create source-file owner metadata")
    }
    fn update_source(
        &mut self,
        original: NodeId,
        statements: Option<NodeListId>,
        eof: Option<NodeId>,
    ) -> NodeId {
        let node = Factory::node(self, original);
        let data = node
            .data_source()
            .as_source_file()
            .expect("SourceFile payload");
        if data.statements() == statements && data.end_of_file_token() == eof {
            return original;
        }
        panic!("lazy AST transaction cannot create source-file owner metadata")
    }
}

// port: tsc/internal/ast/utilities.go:ModifierToFlag
pub fn modifier_to_flag(kind: NodeKind) -> u32 {
    use modifier_flags as f;
    use SyntaxKind as K;
    match kind.known() {
        Some(K::StaticKeyword) => f::STATIC,
        Some(K::PublicKeyword) => f::PUBLIC,
        Some(K::ProtectedKeyword) => f::PROTECTED,
        Some(K::PrivateKeyword) => f::PRIVATE,
        Some(K::AbstractKeyword) => f::ABSTRACT,
        Some(K::AccessorKeyword) => f::ACCESSOR,
        Some(K::ExportKeyword) => f::EXPORT,
        Some(K::DeclareKeyword) => f::AMBIENT,
        Some(K::ConstKeyword) => f::CONST,
        Some(K::DefaultKeyword) => f::DEFAULT,
        Some(K::AsyncKeyword) => f::ASYNC,
        Some(K::ReadonlyKeyword) => f::READONLY,
        Some(K::OverrideKeyword) => f::OVERRIDE,
        Some(K::InKeyword) => f::IN,
        Some(K::OutKeyword) => f::OUT,
        Some(K::Decorator) => f::DECORATOR,
        _ => f::NONE,
    }
}

pub type NodeVisit<'a> =
    dyn for<'v> Fn(&mut NodeVisitor<'v>, Option<NodeId>) -> Option<NodeId> + 'a;
pub type ListVisit<'a> =
    dyn for<'v> Fn(&mut NodeVisitor<'v>, Option<NodeListId>) -> Option<NodeListId> + 'a;
#[derive(Clone, Copy, Default)]
pub struct NodeVisitorHooks<'a> {
    pub visit_node: Option<&'a NodeVisit<'a>>,
    pub visit_token: Option<&'a NodeVisit<'a>>,
    pub visit_nodes: Option<&'a ListVisit<'a>>,
    pub visit_modifiers: Option<&'a ListVisit<'a>>,
    pub visit_embedded_statement: Option<&'a NodeVisit<'a>>,
    pub visit_iteration_body: Option<&'a NodeVisit<'a>>,
    pub visit_parameters: Option<&'a ListVisit<'a>>,
    pub visit_function_body: Option<&'a NodeVisit<'a>>,
    pub visit_top_level_statements: Option<&'a ListVisit<'a>>,
}
enum VisitorFactory<'a> {
    Borrowed(&'a mut dyn RuntimeFactory),
    Owned(Box<AstBuilder>),
}
/// Callbacks borrow their captures and receive only IDs plus the exclusive visitor.
/// They may replace/clear `visit` or recurse without a node borrow or callback
/// reference-count operation. An omitted factory creates an owned empty builder;
/// foreign nodes still require explicit import into that builder before use.
pub struct NodeVisitor<'a> {
    factory: VisitorFactory<'a>,
    pub visit: Option<&'a NodeVisit<'a>>,
    pub hooks: NodeVisitorHooks<'a>,
}
impl<'a> NodeVisitor<'a> {
    // port: tsc/internal/ast/visitor.go:NewNodeVisitor
    pub fn new(
        visit: Option<&'a NodeVisit<'a>>,
        factory: Option<&'a mut dyn RuntimeFactory>,
        hooks: NodeVisitorHooks<'a>,
    ) -> Self {
        Self {
            factory: factory.map_or_else(
                || {
                    VisitorFactory::Owned(Box::new(AstBuilder::new(
                        ts_jsstring::SourceText::from_loaded_bytes(&b""[..]),
                        &ts_arena::Counters::new(),
                    )))
                },
                VisitorFactory::Borrowed,
            ),
            visit,
            hooks,
        }
    }
    pub fn factory(&self) -> &dyn RuntimeFactory {
        match &self.factory {
            VisitorFactory::Borrowed(f) => &**f,
            VisitorFactory::Owned(f) => &**f,
        }
    }
    pub fn factory_mut(&mut self) -> &mut dyn RuntimeFactory {
        match &mut self.factory {
            VisitorFactory::Borrowed(f) => &mut **f,
            VisitorFactory::Owned(f) => &mut **f,
        }
    }
    pub fn owned_factory_mut(&mut self) -> Option<&mut AstBuilder> {
        match &mut self.factory {
            VisitorFactory::Owned(f) => Some(f),
            VisitorFactory::Borrowed(_) => None,
        }
    }
    pub fn into_owned_factory(self) -> Option<AstBuilder> {
        match self.factory {
            VisitorFactory::Owned(f) => Some(*f),
            VisitorFactory::Borrowed(_) => None,
        }
    }
    // port: tsc/internal/ast/visitor.go:NodeVisitor.VisitSourceFile
    pub fn visit_source_file(&mut self, id: NodeId) -> NodeId {
        let id = self.visit_node(Some(id)).expect("nil visited source file");
        assert!(
            matches!(self.node(id).data(), crate::NodeDataRead::SourceFile(_)),
            "SourceFile payload"
        );
        id
    }
    // port: tsc/internal/ast/visitor.go:NodeVisitor.VisitNode
    pub fn visit_node(&mut self, node: Option<NodeId>) -> Option<NodeId> {
        if node.is_none() {
            return node;
        }
        let Some(visit) = self.visit else { return node };
        stacker::maybe_grow(64 * 1024, 1024 * 1024, || {
            let mut visited = visit(self, node);
            if let Some(id) = visited {
                if self.node(id).kind() == SyntaxKind::SyntaxList {
                    let children = self.syntax_children(id);
                    let children = self.factory().read_nodes(children);
                    assert!(
                        children.len() == 1,
                        "Expected only a single node to be written to output"
                    );
                    visited = children.at(0);
                    if let Some(id) = visited {
                        assert!(
                            self.node(id).kind() != SyntaxKind::SyntaxList,
                            "The result of visiting and lifting a Node may not be SyntaxList"
                        );
                    }
                }
            }
            visited
        })
    }
    // port: tsc/internal/ast/visitor.go:NodeVisitor.VisitEmbeddedStatement
    pub fn visit_embedded_statement(&mut self, node: Option<NodeId>) -> Option<NodeId> {
        if node.is_none() {
            return node;
        }
        let Some(visit) = self.visit else { return node };
        let visited = visit(self, node);
        visited.map(|id| self.lift_to_block(Some(id)))
    }
    // port: tsc/internal/ast/visitor.go:NodeVisitor.VisitNodes
    pub fn visit_nodes(&mut self, list: Option<NodeListId>) -> Option<NodeListId> {
        self.visit_list_public(list, false)
    }
    // port: tsc/internal/ast/visitor.go:NodeVisitor.VisitModifiers
    pub fn visit_modifiers(&mut self, list: Option<NodeListId>) -> Option<NodeListId> {
        self.visit_list_public(list, true)
    }
    fn visit_list_public(
        &mut self,
        list: Option<NodeListId>,
        modifiers: bool,
    ) -> Option<NodeListId> {
        let id = list?;
        if self.visit.is_none() {
            return list;
        }
        let nodes = self.factory().read_list(id).nodes();
        let (nodes, changed) = self.visit_slice(nodes);
        if !changed {
            return list;
        }
        let created = if modifiers {
            self.factory_mut().new_modifier_list(nodes)
        } else {
            self.factory_mut().alloc_list(TextRange::new(-1, -1), nodes)
        };
        // Read the original location after callbacks, as upstream does.
        let loc = self.factory().read_list(id).loc();
        self.factory_mut().mutable_list(created).set_loc(loc);
        Some(created)
    }
    // port: tsc/internal/ast/visitor.go:NodeVisitor.VisitSlice
    pub fn visit_slice(&mut self, nodes: NodeSlice) -> (NodeSlice, bool) {
        if nodes.is_nil() || self.visit.is_none() {
            return (nodes, false);
        }
        for index in 0..nodes.len() {
            let node = self.factory().read_nodes(nodes).at(index);
            let Some(visit) = self.visit else { break };
            let mut visited = visit(self, node);
            if visited.is_none() || visited != node {
                let mut updated: Vec<_> = self
                    .factory()
                    .read_nodes(nodes)
                    .iter()
                    .take(index)
                    .collect();
                let mut index = index;
                loop {
                    if let Some(id) = visited {
                        if self.node(id).kind() == SyntaxKind::SyntaxList {
                            updated
                                .extend(self.factory().read_nodes(self.syntax_children(id)).iter());
                        } else {
                            updated.push(Some(id));
                        }
                    }
                    index += 1;
                    if index >= nodes.len() {
                        break;
                    }
                    if let Some(visit) = self.visit {
                        let node = self.factory().read_nodes(nodes).at(index);
                        visited = visit(self, node);
                    } else {
                        updated.extend(self.factory().read_nodes(nodes).iter().skip(index));
                        break;
                    }
                }
                return (self.factory_mut().alloc_nodes(updated), true);
            }
        }
        (nodes, false)
    }
    // port: tsc/internal/ast/visitor.go:NodeVisitor.VisitEachChild
    pub fn visit_each_child(&mut self, node: Option<NodeId>) -> Option<NodeId> {
        if self.visit.is_none() {
            return node;
        }
        node.map(|id| {
            stacker::maybe_grow(64 * 1024, 1024 * 1024, || {
                self.visit_each_child_generated(id)
            })
        })
    }
    fn syntax_children(&self, id: NodeId) -> NodeSlice {
        self.node(id)
            .data_source()
            .as_syntax_list()
            .expect("SyntaxList payload")
            .children()
    }
    // port: tsc/internal/ast/visitor.go:NodeVisitor.liftToBlock
    fn lift_to_block(&mut self, node: Option<NodeId>) -> NodeId {
        let nodes = if let Some(id) = node {
            if self.node(id).kind() == SyntaxKind::SyntaxList {
                self.syntax_children(id)
            } else {
                return id;
            }
        } else {
            NodeSlice::empty()
        };
        let id = if nodes.len() == 1 {
            self.factory()
                .read_nodes(nodes)
                .at(0)
                .expect("nil lifted statement")
        } else {
            let list = self.factory_mut().alloc_list(TextRange::new(-1, -1), nodes);
            self.new_block(Some(list), true)
        };
        assert!(
            self.node(id).kind() != SyntaxKind::SyntaxList,
            "The result of visiting and lifting a Node may not be SyntaxList"
        );
        id
    }
    // port: tsc/internal/ast/visitor.go:NodeVisitor.visitNode
    fn role_node(&mut self, node: Option<NodeId>) -> Option<NodeId> {
        if let Some(hook) = self.hooks.visit_node {
            hook(self, node)
        } else {
            self.visit_node(node)
        }
    }
    // port: tsc/internal/ast/visitor.go:NodeVisitor.visitToken
    fn role_token(&mut self, node: Option<NodeId>) -> Option<NodeId> {
        if let Some(hook) = self.hooks.visit_token {
            hook(self, node)
        } else {
            self.visit_node(node)
        }
    }
    // port: tsc/internal/ast/visitor.go:NodeVisitor.visitEmbeddedStatement
    fn role_embedded(&mut self, node: Option<NodeId>) -> Option<NodeId> {
        if let Some(hook) = self.hooks.visit_embedded_statement {
            hook(self, node)
        } else if let Some(hook) = self.hooks.visit_node {
            let visited = hook(self, node);
            Some(self.lift_to_block(visited))
        } else {
            self.visit_embedded_statement(node)
        }
    }
    // port: tsc/internal/ast/visitor.go:NodeVisitor.visitIterationBody
    fn role_iteration(&mut self, node: Option<NodeId>) -> Option<NodeId> {
        if let Some(hook) = self.hooks.visit_iteration_body {
            hook(self, node)
        } else {
            self.role_embedded(node)
        }
    }
    // port: tsc/internal/ast/visitor.go:NodeVisitor.visitFunctionBody
    fn role_function(&mut self, node: Option<NodeId>) -> Option<NodeId> {
        if let Some(hook) = self.hooks.visit_function_body {
            hook(self, node)
        } else {
            self.role_node(node)
        }
    }
    // port: tsc/internal/ast/visitor.go:NodeVisitor.visitNodes
    fn role_nodes(&mut self, list: Option<NodeListId>) -> Option<NodeListId> {
        if let Some(hook) = self.hooks.visit_nodes {
            hook(self, list)
        } else {
            self.visit_nodes(list)
        }
    }
    // port: tsc/internal/ast/visitor.go:NodeVisitor.visitModifiers
    fn role_modifiers(&mut self, list: Option<NodeListId>) -> Option<NodeListId> {
        if let Some(hook) = self.hooks.visit_modifiers {
            hook(self, list)
        } else {
            self.visit_modifiers(list)
        }
    }
    // port: tsc/internal/ast/visitor.go:NodeVisitor.visitParameters
    fn role_parameters(&mut self, list: Option<NodeListId>) -> Option<NodeListId> {
        if let Some(hook) = self.hooks.visit_parameters {
            hook(self, list)
        } else {
            self.role_nodes(list)
        }
    }
    // port: tsc/internal/ast/visitor.go:NodeVisitor.visitTopLevelStatements
    fn role_top_level(&mut self, list: Option<NodeListId>) -> Option<NodeListId> {
        if let Some(hook) = self.hooks.visit_top_level_statements {
            hook(self, list)
        } else {
            self.role_nodes(list)
        }
    }
}
impl Factory for NodeVisitor<'_> {
    fn node(&self, id: NodeId) -> NodeRead<'_> {
        self.factory().node(id)
    }
    fn node_mut(&mut self, id: NodeId) -> crate::NodeMut<'_> {
        self.factory_mut().node_mut(id)
    }
    fn node_count(&self) -> i64 {
        self.factory().node_count()
    }
    fn text_count(&self) -> i64 {
        self.factory().text_count()
    }
    fn new_node(&mut self, kind: NodeKind, data: NodeData) -> NodeId {
        self.factory_mut().new_node(kind, data)
    }
    fn increment_text_count(&mut self) {
        self.factory_mut().increment_text_count();
    }
    fn set_node_flags(&mut self, id: NodeId, flags: u32) {
        self.factory_mut().set_node_flags(id, flags);
    }
    fn finish_update(&mut self, updated: NodeId, original: NodeId) -> NodeId {
        self.factory_mut().finish_update(updated, original)
    }
    fn finish_clone(&mut self, updated: NodeId, original: NodeId) -> NodeId {
        self.factory_mut().finish_clone(updated, original)
    }
}
impl VisitContext for NodeVisitor<'_> {
    fn visit_node(&mut self, node: Option<NodeId>, role: ChildRole) -> Option<NodeId> {
        match role {
            ChildRole::Token => self.role_token(node),
            ChildRole::EmbeddedStatement => self.role_embedded(node),
            ChildRole::IterationBody => self.role_iteration(node),
            ChildRole::FunctionBody => self.role_function(node),
            _ => self.role_node(node),
        }
    }
    fn visit_list(&mut self, list: Option<NodeListId>, role: ChildRole) -> Option<NodeListId> {
        match role {
            ChildRole::Modifiers => self.role_modifiers(list),
            ChildRole::Parameters => self.role_parameters(list),
            ChildRole::TopLevelStatements => self.role_top_level(list),
            _ => self.role_nodes(list),
        }
    }
    fn map_raw_nodes(&mut self, nodes: NodeSlice) -> NodeSlice {
        let mut updated: Option<Vec<Option<NodeId>>> = None;
        for index in 0..nodes.len() {
            let original = self.factory().read_nodes(nodes).at(index);
            let visited = self.role_node(original);
            if let Some(updated) = &mut updated {
                updated.push(visited);
            } else if visited != original {
                let mut prefix: Vec<_> = self
                    .factory()
                    .read_nodes(nodes)
                    .iter()
                    .take(index)
                    .collect();
                prefix.push(visited);
                updated = Some(prefix);
            }
        }
        updated.map_or(nodes, |nodes| self.factory_mut().alloc_nodes(nodes))
    }
    // port: tsc/internal/ast/ast.go:SourceFile.VisitEachChild
    fn visit_each_child_source_file(&mut self, id: NodeId) -> NodeId {
        let (statements, eof) = {
            let node = self.node(id);
            let data = node
                .data_source()
                .as_source_file()
                .expect("SourceFile payload");
            (data.statements(), data.end_of_file_token())
        };
        let statements = self.role_top_level(statements);
        let eof = self.role_token(eof);
        self.factory_mut().update_source(id, statements, eof)
    }
}

impl<T: RuntimeFactory + ?Sized> RuntimeFactory for crate::BorrowedFactory<'_, T> {
    fn override_parent_in_immediate_children(&mut self, node: NodeId, scratch: &mut Vec<NodeId>) {
        self.0.override_parent_in_immediate_children(node, scratch);
    }
    fn read_list(&self, id: NodeListId) -> NodeListRead<'_> {
        self.0.read_list(id)
    }
    fn read_nodes(&self, nodes: NodeSlice) -> NodeSliceRead<'_> {
        self.0.read_nodes(nodes)
    }
    fn alloc_nodes(&mut self, nodes: Vec<Option<NodeId>>) -> NodeSlice {
        self.0.alloc_nodes(nodes)
    }
    fn alloc_list(&mut self, loc: TextRange, nodes: NodeSlice) -> NodeListId {
        self.0.alloc_list(loc, nodes)
    }
    fn mutable_list(&mut self, id: NodeListId) -> &mut NodeList {
        self.0.mutable_list(id)
    }
    fn set_list_location(&mut self, id: NodeListId, loc: TextRange) {
        self.0.set_list_location(id, loc);
    }
    fn set_list_modifier_flags(&mut self, id: NodeListId, flags: u32) {
        self.0.set_list_modifier_flags(id, flags);
    }
    fn clone_source(&mut self, original: NodeId) -> NodeId {
        self.0.clone_source(original)
    }
    fn update_source(
        &mut self,
        original: NodeId,
        statements: Option<NodeListId>,
        eof: Option<NodeId>,
    ) -> NodeId {
        self.0.update_source(original, statements, eof)
    }
    fn clone_list_header(&mut self, original: NodeListId) -> NodeListId {
        self.0.clone_list_header(original)
    }
    fn clone_modifier_list_header(&mut self, original: NodeListId) -> NodeListId {
        self.0.clone_modifier_list_header(original)
    }
    fn list_has_trailing_comma(&self, list: NodeListId) -> bool {
        self.0.list_has_trailing_comma(list)
    }
    fn new_modifier_list(&mut self, nodes: NodeSlice) -> NodeListId {
        self.0.new_modifier_list(nodes)
    }
    fn modifiers_to_flags(&self, nodes: NodeSlice) -> u32 {
        self.0.modifiers_to_flags(nodes)
    }
}
