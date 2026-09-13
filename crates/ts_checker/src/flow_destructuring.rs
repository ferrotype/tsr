//! Synthetic element accesses let destructuring use the ordinary flow matcher.
//! Their AST owner retains completed sources; flow links name the original
//! binder owner because the checker factory does not own binder arenas.
use crate::{type_flags as tf, CheckerState, Error, TypeId};
use ts_arena::NodeId;
use ts_ast::{Factory, FactoryMethods, JsString, SyntaxKind as K};

impl CheckerState {
    pub(crate) fn retain_flow_source(&mut self, owner: NodeId) -> Result<(), Error> {
        let source_owner = self
            .flow
            .synthetic
            .get(&owner)
            .map_or(owner, |&(source, _)| source);
        let bound = self.program()?.bound(source_owner)?;
        let source = bound.result().source();
        let index = self.program()?.file_index(Some(source));
        let completed = self.program()?.host.source_file(index).clone();
        completed
            .view()
            .ast()
            .for_node_owner(source_owner)?
            .node(source_owner)?;
        self.factory.retain_completed(&completed);
        Ok(())
    }

    // port: tsc/internal/checker/checker.go:Checker.getFlowTypeOfDestructuring
    pub(crate) fn flow_type_of_destructuring(
        &mut self,
        node: NodeId,
        declared: TypeId,
    ) -> Result<TypeId, Error> {
        match self.synthetic_element_access(node)? {
            Some(reference) => {
                self.flow_type_of_reference_with_container(reference, declared, declared, None)
            }
            None => Ok(declared),
        }
    }
    // port: tsc/internal/checker/checker.go:Checker.getSyntheticElementAccess
    fn synthetic_element_access(&mut self, node: NodeId) -> Result<Option<NodeId>, Error> {
        let Some(parent_access) = self.parent_element_access(node)? else {
            return Ok(None);
        };
        let Some(flow) = self.node_flow(parent_access)? else {
            return Ok(None);
        };
        let Some(name) = self.destructuring_property_name(node)? else {
            return Ok(None);
        };
        let source_owner = self
            .flow
            .synthetic
            .get(&parent_access)
            .map_or(parent_access, |&(owner, _)| owner);
        self.retain_flow_source(source_owner)?;
        let range = self.ast(node)?.node(node)?.range();
        let literal = self.factory.new_string_literal(name, 0);
        self.factory.set_node_range(literal, range);
        let lhs = if ts_ast::is_left_hand_side_expression(self.ast(parent_access)?, parent_access)?
        {
            parent_access
        } else {
            let lhs = self
                .factory
                .new_parenthesized_expression(Some(parent_access));
            self.factory.set_node_range(lhs, range);
            lhs
        };
        let result = self
            .factory
            .new_element_access_expression(Some(lhs), None, Some(literal), 0);
        self.factory.set_node_range(result, range);
        self.factory.set_node_parent(literal, Some(result));
        self.factory.set_node_parent(result, Some(node));
        if lhs != parent_access {
            self.factory.set_node_parent(lhs, Some(result));
        }
        self.flow.synthetic.insert(result, (source_owner, flow));
        Ok(Some(result))
    }
    // port: tsc/internal/checker/checker.go:Checker.getParentElementAccess
    fn parent_element_access(&mut self, node: NodeId) -> Result<Option<NodeId>, Error> {
        let parent = self
            .ast(node)?
            .node(node)?
            .parent()
            .ok_or(Error::MissingLink("destructuring parent"))?;
        let ancestor = self
            .ast(parent)?
            .node(parent)?
            .parent()
            .ok_or(Error::MissingLink("destructuring ancestor"))?;
        let read = self.ast(ancestor)?.node(ancestor)?;
        match read.kind().known() {
            Some(K::BindingElement | K::PropertyAssignment) => {
                self.synthetic_element_access(ancestor)
            }
            Some(K::ArrayLiteralExpression) => self.synthetic_element_access(parent),
            Some(K::VariableDeclaration) => Ok(read.initializer()),
            Some(K::BinaryExpression) => Ok(read
                .data_source()
                .as_binary_expression()
                .ok_or(ts_arena::Error::InvalidGraph)?
                .right()),
            _ => Ok(None),
        }
    }
    // port: tsc/internal/checker/flow.go:Checker.getDestructuringPropertyName
    pub(crate) fn destructuring_property_name(
        &mut self,
        node: NodeId,
    ) -> Result<Option<JsString>, Error> {
        let read = self.ast(node)?.node(node)?;
        let parent = read
            .parent()
            .ok_or(Error::MissingLink("destructuring property parent"))?;
        let parent_kind = self.ast(parent)?.node(parent)?.kind();
        let name = if read.kind() == K::BindingElement && parent_kind == K::ObjectBindingPattern {
            let data = read
                .data_source()
                .as_binding_element()
                .ok_or(ts_arena::Error::InvalidGraph)?;
            data.property_name().or(data.name())
        } else if matches!(
            read.kind().known(),
            Some(K::PropertyAssignment | K::ShorthandPropertyAssignment)
        ) {
            read.name()
        } else {
            None
        };
        if let Some(name) = name {
            let ty = self.literal_type_from_property_name(name)?;
            return if self.types.flags(ty)? & tf::STRING_OR_NUMBER_LITERAL != 0 {
                self.index_property_name(ty)
            } else {
                Ok(None)
            };
        }
        if matches!(
            parent_kind.known(),
            Some(K::ArrayLiteralExpression | K::ArrayBindingPattern)
        ) {
            let elements =
                self.source_list(parent, self.ast(parent)?.node(parent)?.element_list())?;
            // Native slices.Index returns -1, including on malformed input.
            let index = match elements.iter().position(|&element| element == node) {
                Some(index) => isize::try_from(index).map_err(|_| Error::IdExhausted)?,
                None => -1,
            };
            return Ok(Some(JsString::from_bytes(index.to_string().into_bytes())));
        }
        Ok(None)
    }
}
