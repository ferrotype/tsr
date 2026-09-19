//! Shared logical reads for owned factory nodes and contextual compact reads.
use crate::{
    AstView, Node, NodeDataRead, NodeDataSource, NodeId, NodeKind, NodeListId, NodeSlice,
    SyntaxKind as K,
};
use ts_arena::Error;
use ts_core::TextRange;

macro_rules! field {
    ($node:expr, $variant:ident, $accessor:ident, $field:ident) => {
        match $node.data_source().$accessor() {
            Some(data) => data.$field(),
            None => panic!(
                "interface conversion: ast.nodeData is *ast.{}, not *ast.{}",
                $node.data_source().name(),
                stringify!($variant)
            ),
        }
    };
}

macro_rules! shape {
    ($node:expr; $($variant:ident $accessor:ident => |$data:ident| $value:expr,)*) => {
        match $node.data() {
            $(NodeDataRead::$variant($data) => $value,)*
            _ => None,
        }
    };
}

mod sealed {
    pub trait Sealed {}
    impl Sealed for crate::Node {}
    impl Sealed for crate::NodeRead<'_> {}
}

/// A borrowed semantic node interface; physical payload storage stays private.
/// Implementations are limited to owned construction nodes and checked reads.
pub trait NodeAccess: sealed::Sealed {
    fn kind(&self) -> NodeKind;
    fn parent(&self) -> Option<NodeId>;
    fn flags(&self) -> u32;
    fn pos(&self) -> i32;
    fn end(&self) -> i32;
    fn range(&self) -> TextRange;
    fn data(&self) -> NodeDataRead<'_>;
    fn data_source(&self) -> NodeDataSource<'_>;
    fn cached_subtree_facts(&self) -> u32;
    fn store_subtree_facts(&self, facts: u32);
    fn existing_runtime_id(&self) -> u64;
    fn runtime_id(&self) -> u64;

    /// Visit children in the pinned node-kind order.
    fn for_each_child(&self, visitor: &mut impl crate::ChildVisitor) -> std::ops::ControlFlow<()>
    where
        Self: Sized,
    {
        crate::runtime_generated::for_each_child_generated(self, visitor)
    }

    crate::node_semantics::reads!(NodeId, NodeListId, field;,);
    crate::node_semantics::shape_reads!(NodeId, NodeListId, shape;,);

    /// port: tsc/internal/ast/ast.go:Node.Name
    fn name(&self) -> Option<NodeId> {
        self.data().declaration_name_generated()
    }

    /// port: tsc/internal/ast/ast.go:Node.ModifierFlags
    fn modifier_flags(&self, view: AstView<'_>) -> Result<u32, Error> {
        self.modifiers()
            .map_or(Ok(0), |id| Ok(view.list(id)?.modifier_flags()))
    }
    /// port: tsc/internal/ast/ast.go:Node.QuestionToken
    fn question_token(&self, view: AstView<'_>) -> Result<Option<NodeId>, Error> {
        match self.kind().known() {
            Some(K::Parameter) => {
                return Ok(field!(
                    self,
                    ParameterDeclaration,
                    as_parameter_declaration,
                    question_token
                ))
            }
            Some(K::ConditionalExpression) => {
                return Ok(field!(
                    self,
                    ConditionalExpression,
                    as_conditional_expression,
                    question_token
                ));
            }
            Some(K::MappedType) => {
                return Ok(field!(
                    self,
                    MappedTypeNode,
                    as_mapped_type_node,
                    question_token
                ))
            }
            Some(K::NamedTupleMember) => {
                return Ok(field!(
                    self,
                    NamedTupleMember,
                    as_named_tuple_member,
                    question_token
                ))
            }
            _ => {}
        }
        let token = self.postfix_token();
        Ok(match token {
            Some(token) if view.node(token)?.kind() == K::QuestionToken => Some(token),
            _ => None,
        })
    }
    /// port: tsc/internal/ast/ast.go:Node.Parameters
    fn parameters(&self, view: AstView<'_>) -> Result<NodeSlice, Error> {
        Ok(view
            .list(
                self.parameter_list()
                    .expect("runtime error: invalid memory address or nil pointer dereference"),
            )?
            .nodes())
    }
    /// port: tsc/internal/ast/ast.go:Node.Arguments
    fn arguments(&self, view: AstView<'_>) -> Result<NodeSlice, Error> {
        self.argument_list()
            .map_or(Ok(NodeSlice::empty()), |id| Ok(view.list(id)?.nodes()))
    }
    /// port: tsc/internal/ast/ast.go:Node.TypeArguments
    fn type_arguments(&self, view: AstView<'_>) -> Result<NodeSlice, Error> {
        self.type_argument_list()
            .map_or(Ok(NodeSlice::empty()), |id| Ok(view.list(id)?.nodes()))
    }
    /// port: tsc/internal/ast/ast.go:Node.TypeParameters
    fn type_parameters(&self, view: AstView<'_>) -> Result<NodeSlice, Error> {
        self.type_parameter_list()
            .map_or(Ok(NodeSlice::empty()), |id| Ok(view.list(id)?.nodes()))
    }
    /// port: tsc/internal/ast/ast.go:Node.Members
    fn members(&self, view: AstView<'_>) -> Result<NodeSlice, Error> {
        self.member_list()
            .map_or(Ok(NodeSlice::empty()), |id| Ok(view.list(id)?.nodes()))
    }
    /// port: tsc/internal/ast/ast.go:Node.Statements
    fn statements(&self, view: AstView<'_>) -> Result<NodeSlice, Error> {
        self.statement_list()
            .map_or(Ok(NodeSlice::empty()), |id| Ok(view.list(id)?.nodes()))
    }
    /// port: tsc/internal/ast/ast.go:Node.ModifierNodes
    fn modifier_nodes(&self, view: AstView<'_>) -> Result<NodeSlice, Error> {
        self.modifiers()
            .map_or(Ok(NodeSlice::empty()), |id| Ok(view.list(id)?.nodes()))
    }
    /// port: tsc/internal/ast/ast.go:Node.Comments
    fn comments(&self, view: AstView<'_>) -> Result<NodeSlice, Error> {
        self.comment_list()
            .map_or(Ok(NodeSlice::empty()), |id| Ok(view.list(id)?.nodes()))
    }
    /// port: tsc/internal/ast/ast.go:Node.Properties
    fn properties(&self, view: AstView<'_>) -> Result<NodeSlice, Error> {
        self.property_list()
            .map_or(Ok(NodeSlice::empty()), |id| Ok(view.list(id)?.nodes()))
    }
    /// port: tsc/internal/ast/ast.go:Node.Elements
    fn elements(&self, view: AstView<'_>) -> Result<NodeSlice, Error> {
        self.element_list()
            .map_or(Ok(NodeSlice::empty()), |id| Ok(view.list(id)?.nodes()))
    }
}

impl Node {
    pub fn data_source(&self) -> NodeDataSource<'_> {
        NodeDataSource::from_owned(self.data())
    }
}

impl NodeAccess for Node {
    fn kind(&self) -> NodeKind {
        Node::kind(self)
    }
    fn parent(&self) -> Option<NodeId> {
        Node::parent(self)
    }
    fn flags(&self) -> u32 {
        Node::flags(self)
    }
    fn pos(&self) -> i32 {
        Node::pos(self)
    }
    fn end(&self) -> i32 {
        Node::end(self)
    }
    fn range(&self) -> TextRange {
        Node::range(self)
    }
    fn data(&self) -> NodeDataRead<'_> {
        NodeDataRead::from_owned(Node::data(self))
    }
    fn data_source(&self) -> NodeDataSource<'_> {
        NodeDataSource::from_owned(Node::data(self))
    }
    fn cached_subtree_facts(&self) -> u32 {
        Node::cached_subtree_facts(self)
    }
    fn store_subtree_facts(&self, facts: u32) {
        Node::store_subtree_facts(self, facts);
    }
    fn existing_runtime_id(&self) -> u64 {
        crate::runtime_id::owned_existing_runtime_node_id(self)
    }
    fn runtime_id(&self) -> u64 {
        crate::runtime_id::owned_runtime_node_id(self)
    }
}
