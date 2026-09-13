//! Evolving array elements are temporary flow types; final arrays share the
//! generic Array constructor and the regular-object-literal cache.
use crate::{
    object_flags as of, type_flags as tf,
    types::{EvolvingArrayData, ObjectData, Payload},
    CheckerState, Error, TypeId, UnionReduction,
};
use ts_arena::NodeId;
use ts_ast::SyntaxKind as K;
fn required<T>(value: Option<T>, context: &'static str) -> Result<T, Error> {
    value.ok_or(Error::MissingLink(context))
}
impl CheckerState {
    // port: tsc/internal/checker/flow.go:Checker.getEvolvingArrayType
    pub(crate) fn evolving_array_type(&mut self, element: TypeId) -> Result<TypeId, Error> {
        if let Some(&ty) = self.flow.evolving.get(&element) {
            return Ok(ty);
        }
        let ty = self.types.new_type(
            tf::OBJECT,
            of::EVOLVING_ARRAY,
            Payload::EvolvingArray(EvolvingArrayData {
                object: ObjectData::default(),
                element_type: element,
                final_array_type: None,
            }),
        )?;
        self.flow.evolving.insert(element, ty);
        Ok(ty)
    }
    // port: tsc/internal/checker/flow.go:Checker.finalizeEvolvingArrayType
    // port: tsc/internal/checker/flow.go:Checker.getFinalArrayType
    // port: tsc/internal/checker/flow.go:Checker.createFinalArrayType
    pub(crate) fn finalize_evolving_array(&mut self, ty: TypeId) -> Result<TypeId, Error> {
        if self.types.object_flags(ty)? & of::EVOLVING_ARRAY == 0 {
            return Ok(ty);
        }
        let data = self.types.evolving_array(ty)?;
        if let Some(final_type) = data.final_array_type {
            return Ok(final_type);
        }
        let element = data.element_type;
        let flags = self.types.flags(element)?;
        let result = if flags & tf::NEVER != 0 {
            self.auto_array_type()?
        } else {
            let element = if flags & tf::UNION != 0 {
                let parts = self.types.types_of(element)?.to_vec();
                self.get_union_type_ex(&parts, UnionReduction::Subtype, None, None)?
            } else {
                element
            };
            self.create_array_type(element, false)?
        };
        self.types.evolving_array_mut(ty)?.final_array_type = Some(result);
        Ok(result)
    }
    pub(crate) fn auto_array_type(&self) -> Result<TypeId, Error> {
        self.query
            .global_types
            .get("autoArrayType")
            .copied()
            .ok_or(Error::MissingLink("autoArrayType"))
    }
    pub(crate) fn any_array_type(&self) -> Result<TypeId, Error> {
        self.query
            .global_types
            .get("anyArrayType")
            .copied()
            .ok_or(Error::MissingLink("anyArrayType"))
    }
    // port: tsc/internal/checker/flow.go:Checker.addEvolvingArrayElementType
    fn add_evolving_array_element(
        &mut self,
        evolving: TypeId,
        expression: NodeId,
    ) -> Result<TypeId, Error> {
        let ty = self.get_context_free_type_of_expression(expression)?;
        let ty = self.base_literal_type(ty)?;
        let new_element = self.regular_object_literal_type(ty)?;
        let element = self.types.evolving_array(evolving)?.element_type;
        if self.flow_type_subset(new_element, element)? {
            return Ok(evolving);
        }
        let union = self.get_union_type(&[element, new_element])?;
        self.evolving_array_type(union)
    }
    // port: tsc/internal/checker/flow.go:Checker.getTypeAtFlowArrayMutation
    pub(crate) fn evolving_mutation_target(
        &mut self,
        reference: NodeId,
        mutation: NodeId,
    ) -> Result<bool, Error> {
        let read = self.ast(mutation)?.node(mutation)?;
        let access = if read.kind() == K::CallExpression {
            required(read.expression(), "array mutation callee")?
        } else {
            required(
                read.data_source()
                    .as_binary_expression()
                    .ok_or(ts_arena::Error::InvalidGraph)?
                    .left(),
                "array mutation left",
            )?
        };
        let object = required(
            self.ast(access)?.node(access)?.expression(),
            "array mutation object",
        )?;
        let object = self.reference_candidate(object)?;
        self.matching_reference(reference, object)
    }
    pub(crate) fn evolve_array_mutation(
        &mut self,
        mut evolving: TypeId,
        mutation: NodeId,
    ) -> Result<TypeId, Error> {
        if self.types.object_flags(evolving)? & of::EVOLVING_ARRAY == 0 {
            return Ok(evolving);
        }
        let read = self.ast(mutation)?.node(mutation)?;
        if read.kind() == K::CallExpression {
            for argument in self.source_list(mutation, read.argument_list())? {
                evolving = self.add_evolving_array_element(evolving, argument)?;
            }
        } else {
            let binary = read
                .data_source()
                .as_binary_expression()
                .ok_or(ts_arena::Error::InvalidGraph)?;
            let left = required(binary.left(), "array mutation left")?;
            let right = required(binary.right(), "array mutation right")?;
            let index = required(
                self.ast(left)?
                    .node(left)?
                    .data_source()
                    .as_element_access_expression()
                    .ok_or(ts_arena::Error::InvalidGraph)?
                    .argument_expression(),
                "array mutation index",
            )?;
            let index_type = self.get_context_free_type_of_expression(index)?;
            if self.type_assignable_to_kind(index_type, tf::NUMBER_LIKE)? {
                evolving = self.add_evolving_array_element(evolving, right)?;
            }
        }
        Ok(evolving)
    }
    // port: tsc/internal/checker/flow.go:Checker.isEmptyArrayAssignment
    pub(crate) fn empty_array_assignment(&self, target: NodeId) -> Result<bool, Error> {
        let read = self.ast(target)?.node(target)?;
        let value = if read.kind() == K::VariableDeclaration {
            read.initializer()
        } else if read.kind() != K::BindingElement {
            match read.parent() {
                Some(parent) => self
                    .ast(parent)?
                    .node(parent)?
                    .data_source()
                    .as_binary_expression()
                    .and_then(|data| data.right()),
                None => None,
            }
        } else {
            None
        };
        let Some(value) = value else {
            return Ok(false);
        };
        let read = self.ast(value)?.node(value)?;
        Ok(read.kind() == K::ArrayLiteralExpression
            && self.source_list(value, read.element_list())?.is_empty())
    }
    // port: tsc/internal/checker/flow.go:Checker.getReferenceRoot
    fn flow_reference_root(&self, mut node: NodeId) -> Result<NodeId, Error> {
        loop {
            let Some(parent) = self.ast(node)?.node(node)?.parent() else {
                return Ok(node);
            };
            let read = self.ast(parent)?.node(parent)?;
            let keep = if read.kind() == K::ParenthesizedExpression {
                true
            } else if let Some(binary) = read.data_source().as_binary_expression() {
                let operator = required(binary.operator_token(), "reference root operator")?;
                match self.ast(operator)?.node(operator)?.kind().known() {
                    Some(K::EqualsToken) => binary.left() == Some(node),
                    Some(K::CommaToken) => binary.right() == Some(node),
                    _ => false,
                }
            } else {
                false
            };
            if !keep {
                return Ok(node);
            }
            node = parent;
        }
    }
    // port: tsc/internal/checker/flow.go:Checker.isEvolvingArrayOperationTarget
    pub(crate) fn evolving_array_operation_target(&mut self, node: NodeId) -> Result<bool, Error> {
        let root = self.flow_reference_root(node)?;
        let Some(parent) = self.ast(root)?.node(root)?.parent() else {
            return Ok(false);
        };
        let read = self.ast(parent)?.node(parent)?;
        if read.kind() == K::PropertyAccessExpression {
            let name = required(read.name(), "evolving array property")?;
            let text = self.ast(name)?.node_text(name)?;
            if text.as_bytes() == b"length" {
                return Ok(true);
            }
            if self.ast(name)?.node(name)?.kind() == K::Identifier
                && matches!(text.as_bytes(), b"push" | b"unshift")
            {
                if let Some(call) = read.parent() {
                    return Ok(self.ast(call)?.node(call)?.kind() == K::CallExpression);
                }
            }
        } else if read.kind() == K::ElementAccessExpression && read.expression() == Some(root) {
            let Some(assignment) = read.parent() else {
                return Ok(false);
            };
            let index = required(
                read.data_source()
                    .as_element_access_expression()
                    .ok_or(ts_arena::Error::InvalidGraph)?
                    .argument_expression(),
                "evolving array index",
            )?;
            let read = self.ast(assignment)?.node(assignment)?;
            if let Some(binary) = read.data_source().as_binary_expression() {
                let op = required(binary.operator_token(), "evolving array assignment")?;
                if binary.left() == Some(parent)
                    && self.ast(op)?.node(op)?.kind() == K::EqualsToken
                    && !ts_ast::is_assignment_target(self.ast(assignment)?, assignment)?
                {
                    let ty = self.get_type_of_expression(index)?;
                    return self.type_assignable_to_kind(ty, tf::NUMBER_LIKE);
                }
            }
        }
        Ok(false)
    }
}
