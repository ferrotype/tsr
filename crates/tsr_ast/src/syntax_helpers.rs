//! Shared syntax helper algorithms over checked views and proven local scopes.
//! Only the storage adapters differ; scoped errors are uninhabited and disappear
//! after monomorphization. Selected payload reads retain their public contracts.
use crate::{
    local_bind::{BindNode, BindRead, LocalBind},
    AstView, NodeAccess, NodeId, NodeKind, NodeRead, SyntaxKind as K,
};
use std::convert::Infallible;
use ts_arena::Error;

pub(crate) trait Read {
    type Id: Copy + Eq;
    fn kind(&self) -> NodeKind;
    fn flags(&self) -> u32;
    fn parent(&self) -> Option<Self::Id>;
    fn name(&self) -> Option<Self::Id>;
    fn expression(&self) -> Option<Self::Id>;
    fn question_dot_token(&self) -> Option<Self::Id>;
    fn binary_operator(&self) -> Option<Self::Id>;
    fn prefix_operator(&self) -> NodeKind;
    fn prefix_operand(&self) -> Option<Self::Id>;
    fn element_argument(&self) -> Option<Self::Id>;
    fn declaration_binary_operands(&self) -> (Option<Self::Id>, Option<Self::Id>);
    fn property_assignment_name(&self) -> Option<Self::Id>;
    fn binding_element_name(&self) -> Option<Self::Id>;
    fn module_name(&self) -> Option<Self::Id>;
    fn module_keyword(&self) -> NodeKind;
}

impl<T: NodeAccess + ?Sized> Read for T {
    type Id = NodeId;
    fn kind(&self) -> NodeKind {
        NodeAccess::kind(self)
    }
    fn flags(&self) -> u32 {
        NodeAccess::flags(self)
    }
    fn parent(&self) -> Option<NodeId> {
        NodeAccess::parent(self)
    }
    fn name(&self) -> Option<NodeId> {
        NodeAccess::name(self)
    }
    fn expression(&self) -> Option<NodeId> {
        NodeAccess::expression(self)
    }
    fn question_dot_token(&self) -> Option<NodeId> {
        NodeAccess::question_dot_token(self)
    }
    fn binary_operator(&self) -> Option<NodeId> {
        self.data_source()
            .as_binary_expression()
            .expect("BinaryExpression payload")
            .operator_token()
    }
    fn prefix_operator(&self) -> NodeKind {
        self.data_source()
            .as_prefix_unary_expression()
            .expect("PrefixUnaryExpression payload")
            .operator()
    }
    fn prefix_operand(&self) -> Option<NodeId> {
        self.data_source()
            .as_prefix_unary_expression()
            .expect("PrefixUnaryExpression payload")
            .operand()
    }
    fn element_argument(&self) -> Option<NodeId> {
        self.data_source().as_element_access_expression().unwrap_or_else(|| panic!(
            "interface conversion: ast.nodeData is *ast.{}, not *ast.ElementAccessExpression", self.data_source().name()
        )).argument_expression()
    }
    fn declaration_binary_operands(&self) -> (Option<Self::Id>, Option<Self::Id>) {
        let data = self
            .data_source()
            .as_binary_expression()
            .unwrap_or_else(|| {
                panic!(
                    "interface conversion: ast.nodeData is *ast.{}, not *ast.BinaryExpression",
                    self.data_source().name()
                )
            });
        (data.left(), data.right())
    }

    fn property_assignment_name(&self) -> Option<Self::Id> {
        let data = self
            .data_source()
            .as_property_assignment()
            .unwrap_or_else(|| {
                panic!(
                    "interface conversion: ast.nodeData is *ast.{}, not *ast.PropertyAssignment",
                    self.data_source().name()
                )
            });
        data.name()
    }

    fn binding_element_name(&self) -> Option<Self::Id> {
        let data = self.data_source().as_binding_element().unwrap_or_else(|| {
            panic!(
                "interface conversion: ast.nodeData is *ast.{}, not *ast.BindingElement",
                self.data_source().name()
            )
        });
        data.name()
    }

    fn module_name(&self) -> Option<Self::Id> {
        let data = self
            .data_source()
            .as_module_declaration()
            .unwrap_or_else(|| {
                panic!(
                    "interface conversion: ast.nodeData is *ast.{}, not *ast.ModuleDeclaration",
                    self.data_source().name()
                )
            });
        data.name()
    }

    fn module_keyword(&self) -> NodeKind {
        self.data_source()
            .as_module_declaration()
            .expect("ModuleDeclaration payload")
            .keyword()
    }
}

impl<'scope> Read for BindRead<'scope, '_> {
    type Id = BindNode<'scope>;
    fn kind(&self) -> NodeKind {
        self.kind()
    }
    fn flags(&self) -> u32 {
        self.flags()
    }
    fn parent(&self) -> Option<Self::Id> {
        self.parent()
    }
    fn name(&self) -> Option<Self::Id> {
        self.name()
    }
    fn expression(&self) -> Option<Self::Id> {
        self.expression()
    }
    fn question_dot_token(&self) -> Option<Self::Id> {
        self.question_dot_token()
    }
    fn binary_operator(&self) -> Option<Self::Id> {
        self.as_binary_expression()
            .expect("BinaryExpression payload")
            .operator_token()
    }
    fn prefix_operator(&self) -> NodeKind {
        self.as_prefix_unary_expression()
            .expect("PrefixUnaryExpression payload")
            .operator()
    }
    fn prefix_operand(&self) -> Option<Self::Id> {
        self.as_prefix_unary_expression()
            .expect("PrefixUnaryExpression payload")
            .operand()
    }
    fn element_argument(&self) -> Option<Self::Id> {
        self.as_element_access_expression().unwrap_or_else(|| panic!(
            "interface conversion: ast.nodeData is *ast.{}, not *ast.ElementAccessExpression", self.payload_name()
        )).argument_expression()
    }
    fn declaration_binary_operands(&self) -> (Option<Self::Id>, Option<Self::Id>) {
        let data = self.as_binary_expression().unwrap_or_else(|| {
            panic!(
                "interface conversion: ast.nodeData is *ast.{}, not *ast.BinaryExpression",
                self.payload_name()
            )
        });
        (data.left(), data.right())
    }

    fn property_assignment_name(&self) -> Option<Self::Id> {
        let data = self.as_property_assignment().unwrap_or_else(|| {
            panic!(
                "interface conversion: ast.nodeData is *ast.{}, not *ast.PropertyAssignment",
                self.payload_name()
            )
        });
        data.name()
    }

    fn binding_element_name(&self) -> Option<Self::Id> {
        let data = self.as_binding_element().unwrap_or_else(|| {
            panic!(
                "interface conversion: ast.nodeData is *ast.{}, not *ast.BindingElement",
                self.payload_name()
            )
        });
        data.name()
    }

    fn module_name(&self) -> Option<Self::Id> {
        let data = self.as_module_declaration().unwrap_or_else(|| {
            panic!(
                "interface conversion: ast.nodeData is *ast.{}, not *ast.ModuleDeclaration",
                self.payload_name()
            )
        });
        data.name()
    }

    fn module_keyword(&self) -> NodeKind {
        self.as_module_declaration()
            .expect("ModuleDeclaration payload")
            .keyword()
    }
}

pub(crate) trait View {
    type Id: Copy + Eq;
    type Error;
    type Node<'read>: Read<Id = Self::Id>
    where
        Self: 'read;
    fn read(&self, node: Self::Id) -> Result<Self::Node<'_>, Self::Error>;
    fn modifier_flags<'read>(&'read self, node: &Self::Node<'read>) -> Result<u32, Self::Error>;
}

impl View for AstView<'_> {
    type Id = NodeId;
    type Error = Error;
    type Node<'read>
        = NodeRead<'read>
    where
        Self: 'read;
    fn read(&self, node: NodeId) -> Result<Self::Node<'_>, Error> {
        self.node(node)
    }
    fn modifier_flags<'read>(&'read self, node: &Self::Node<'read>) -> Result<u32, Error> {
        node.modifier_flags(*self)
    }
}

impl<'scope> View for LocalBind<'scope, '_> {
    type Id = BindNode<'scope>;
    type Error = Infallible;
    type Node<'read>
        = BindRead<'scope, 'read>
    where
        Self: 'read;
    fn read(&self, node: Self::Id) -> Result<Self::Node<'_>, Infallible> {
        Ok(self.node(node))
    }
    fn modifier_flags<'read>(&'read self, node: &Self::Node<'read>) -> Result<u32, Infallible> {
        Ok(node
            .modifiers()
            .map_or(0, |list| self.list_modifier_flags(list)))
    }
}

fn required<T>(node: Option<T>) -> T {
    node.expect("nil node in source AST utility")
}

pub(crate) fn skip_parentheses<V: View>(view: &V, mut node: V::Id) -> Result<V::Id, V::Error> {
    while view.read(node)?.kind() == K::ParenthesizedExpression {
        node = view
            .read(node)?
            .expression()
            .expect("nil parenthesized expression");
    }
    Ok(node)
}

pub(crate) fn skip_partially_emitted_expressions<V: View>(
    view: &V,
    mut node: V::Id,
) -> Result<V::Id, V::Error> {
    while view.read(node)?.kind() == K::PartiallyEmittedExpression {
        node = view
            .read(node)?
            .expression()
            .expect("nil partially emitted expression");
    }
    Ok(node)
}

pub(crate) fn is_left_hand_side_expression<V: View>(
    view: &V,
    node: V::Id,
) -> Result<bool, V::Error> {
    Ok(crate::is_left_hand_side_expression_kind(
        view.read(skip_partially_emitted_expressions(view, node)?)?
            .kind(),
    ))
}

pub(crate) fn is_entity_name_expression<V: View>(
    view: &V,
    mut node: V::Id,
    allow_js: bool,
) -> Result<bool, V::Error> {
    loop {
        let read = view.read(node)?;
        match read.kind().known() {
            Some(K::Identifier) => return Ok(true),
            Some(K::PropertyAccessExpression) => {
                if view.read(required(read.name()))?.kind() != K::Identifier {
                    return Ok(false);
                }
            }
            Some(K::ThisKeyword) if allow_js => return Ok(true),
            Some(K::ElementAccessExpression) if allow_js => {
                if !crate::utilities::is_string_or_numeric_literal_like_kind(
                    view.read(required(read.element_argument()))?.kind(),
                ) {
                    return Ok(false);
                }
            }
            _ => return Ok(false),
        }
        node = read.expression().expect("nil entity-name expression");
    }
}

pub(crate) fn is_dotted_name<V: View>(view: &V, mut node: V::Id) -> Result<bool, V::Error> {
    loop {
        let read = view.read(node)?;
        match read.kind().known() {
            Some(K::Identifier | K::ThisKeyword | K::SuperKeyword | K::MetaProperty) => {
                return Ok(true)
            }
            Some(K::PropertyAccessExpression | K::ParenthesizedExpression) => {
                node = read.expression().expect("nil dotted-name expression");
            }
            _ => return Ok(false),
        }
    }
}

pub(crate) fn is_optional_chain<N: Read + ?Sized>(node: &N) -> bool {
    node.flags() & crate::node_flags::OPTIONAL_CHAIN != 0
        && matches!(
            node.kind().known(),
            Some(
                K::PropertyAccessExpression
                    | K::ElementAccessExpression
                    | K::CallExpression
                    | K::NonNullExpression
            )
        )
}

pub(crate) fn is_optional_chain_root<N: Read + ?Sized>(node: &N) -> bool {
    is_optional_chain(node)
        && node.kind() != K::NonNullExpression
        && node.question_dot_token().is_some()
}

pub(crate) fn is_outermost_optional_chain<V: View>(
    view: &V,
    node: V::Id,
) -> Result<bool, V::Error> {
    let parent = view.read(required(view.read(node)?.parent()))?;
    Ok(!is_optional_chain(&parent)
        || is_optional_chain_root(&parent)
        || parent.expression() != Some(node))
}

pub(crate) fn is_expression_of_optional_chain_root<V: View>(
    view: &V,
    node: V::Id,
) -> Result<bool, V::Error> {
    let parent = view.read(required(view.read(node)?.parent()))?;
    Ok(is_optional_chain_root(&parent) && parent.expression() == Some(node))
}

fn binary_kind<V: View>(
    view: &V,
    node: V::Id,
    predicate: impl FnOnce(NodeKind) -> bool,
) -> Result<bool, V::Error> {
    let read = view.read(node)?;
    Ok(read.kind() == K::BinaryExpression
        && predicate(view.read(required(read.binary_operator()))?.kind()))
}

pub(crate) fn is_logical_or_coalescing_binary_expression<V: View>(
    view: &V,
    node: V::Id,
) -> Result<bool, V::Error> {
    binary_kind(
        view,
        node,
        crate::utilities::is_logical_or_coalescing_binary_operator,
    )
}

pub(crate) fn is_logical_or_coalescing_assignment_expression<V: View>(
    view: &V,
    node: V::Id,
) -> Result<bool, V::Error> {
    binary_kind(
        view,
        node,
        crate::is_logical_or_coalescing_assignment_operator,
    )
}

pub(crate) fn is_nullish_coalesce<V: View>(view: &V, node: V::Id) -> Result<bool, V::Error> {
    binary_kind(view, node, |kind| kind == K::QuestionQuestionToken)
}

pub(crate) fn is_logical_expression<V: View>(view: &V, mut node: V::Id) -> Result<bool, V::Error> {
    loop {
        let read = view.read(node)?;
        if read.kind() == K::ParenthesizedExpression {
            node = required(read.expression());
        } else if read.kind() == K::PrefixUnaryExpression
            && read.prefix_operator() == K::ExclamationToken
        {
            node = required(read.prefix_operand());
        } else {
            return is_logical_or_coalescing_binary_expression(view, node);
        }
    }
}
