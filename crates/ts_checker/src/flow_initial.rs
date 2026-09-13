//! Initial and assigned types follow the destructuring shape without creating
//! synthetic syntax. Only initializer types already cached by checking are reused.
use crate::{CheckerState, Error, TypeId};
use ts_arena::NodeId;
use ts_ast::{JsString, SyntaxKind as K};

fn required<T>(value: Option<T>, name: &'static str) -> Result<T, Error> {
    value.ok_or(Error::MissingLink(name))
}
impl CheckerState {
    // port: tsc/internal/checker/flow.go:Checker.getInitialOrAssignedType
    pub(crate) fn initial_or_assigned_type(
        &mut self,
        target: NodeId,
        reference: NodeId,
    ) -> Result<TypeId, Error> {
        let ty = if matches!(
            self.ast(target)?.node(target)?.kind().known(),
            Some(K::VariableDeclaration | K::BindingElement)
        ) {
            self.flow_initial_type(target)?
        } else {
            self.flow_assigned_type(target)?
        };
        self.narrowable_reference_type(ty, reference, 0)
    }
    // port: tsc/internal/checker/flow.go:Checker.getInitialType
    // port: tsc/internal/checker/flow.go:Checker.getInitialTypeOfVariableDeclaration
    // port: tsc/internal/checker/flow.go:Checker.getInitialTypeOfBindingElement
    fn flow_initial_type(&mut self, node: NodeId) -> Result<TypeId, Error> {
        stacker::maybe_grow(128 * 1024, 2 * 1024 * 1024, || {
            let read = self.ast(node)?.node(node)?;
            match read.kind().known() {
                Some(K::VariableDeclaration) => {
                    if let Some(initializer) = read.initializer() {
                        // port: tsc/internal/checker/flow.go:Checker.getTypeOfInitializer
                        if let Some(Some(ty)) = self.query.type_nodes.try_get(initializer) {
                            return Ok(*ty);
                        }
                        return self.get_type_of_expression(initializer);
                    }
                    let list = required(read.parent(), "flow initial declaration list")?;
                    let parent = required(
                        self.ast(list)?.node(list)?.parent(),
                        "flow initial statement",
                    )?;
                    match self.ast(parent)?.node(parent)?.kind().known() {
                        Some(K::ForInStatement) => Ok(self.builtins.string_type),
                        Some(K::ForOfStatement) => self.check_right_hand_side_of_for_of(parent),
                        _ => Ok(self.builtins.error_type),
                    }
                }
                Some(K::BindingElement) => {
                    let pattern = required(read.parent(), "initial binding pattern")?;
                    let initializer = read.initializer();
                    let name =
                        required(read.property_name().or(read.name()), "initial binding name")?;
                    let parent = required(
                        self.ast(pattern)?.node(pattern)?.parent(),
                        "initial binding parent",
                    )?;
                    let ty = self.flow_initial_type(parent)?;
                    let ty = if self.ast(pattern)?.node(pattern)?.kind() == K::ObjectBindingPattern
                    {
                        self.flow_destructured_property_type(ty, name)?
                    } else if !self.binding_is_rest(node)? {
                        let elements = self.source_list(
                            pattern,
                            self.ast(pattern)?.node(pattern)?.element_list(),
                        )?;
                        let index = elements
                            .iter()
                            .position(|&element| element == node)
                            .map(|index| index as isize)
                            .unwrap_or(-1);
                        self.flow_destructured_array_element(ty, index)?
                    } else {
                        self.flow_destructured_spread_type(ty)?
                    };
                    self.flow_type_with_default(ty, initializer)
                }
                _ => Err(ts_arena::Error::InvalidGraph.into()),
            }
        })
    }
    // port: tsc/internal/checker/flow.go:Checker.getAssignedType
    fn flow_assigned_type(&mut self, node: NodeId) -> Result<TypeId, Error> {
        stacker::maybe_grow(128 * 1024, 2 * 1024 * 1024, || {
            let parent = required(
                self.ast(node)?.node(node)?.parent(),
                "flow assignment parent",
            )?;
            let read = self.ast(parent)?.node(parent)?;
            match read.kind().known() {
                Some(K::ForInStatement) => Ok(self.builtins.string_type),
                Some(K::ForOfStatement) => self.check_right_hand_side_of_for_of(parent),
                Some(K::BinaryExpression) => {
                    let right = required(
                        read.data_source()
                            .as_binary_expression()
                            .ok_or(ts_arena::Error::InvalidGraph)?
                            .right(),
                        "assigned binary right",
                    )?;
                    let outer = required(read.parent(), "assigned binary parent")?;
                    let kind = self.ast(outer)?.node(outer)?.kind();
                    let default = kind == K::ArrayLiteralExpression
                        && self.reference_destructuring_target(outer)?
                        || kind == K::PropertyAssignment
                            && self.reference_destructuring_target(required(
                                self.ast(outer)?.node(outer)?.parent(),
                                "default object literal",
                            )?)?;
                    if default {
                        let assigned = self.flow_assigned_type(parent)?;
                        self.flow_type_with_default(assigned, Some(right))
                    } else {
                        self.get_type_of_expression(right)
                    }
                }
                Some(K::DeleteExpression) => Ok(self.builtins.undefined_type),
                Some(K::ArrayLiteralExpression) => {
                    let elements = self.source_list(parent, read.element_list())?;
                    let index = elements
                        .iter()
                        .position(|&element| element == node)
                        .map(|index| index as isize)
                        .unwrap_or(-1);
                    let assigned = self.flow_assigned_type(parent)?;
                    self.flow_destructured_array_element(assigned, index)
                }
                Some(K::SpreadElement) => {
                    let array = required(read.parent(), "assigned spread array")?;
                    let assigned = self.flow_assigned_type(array)?;
                    self.flow_destructured_spread_type(assigned)
                }
                Some(K::PropertyAssignment | K::ShorthandPropertyAssignment) => {
                    let name = required(read.name(), "assigned property name")?;
                    let object = required(read.parent(), "assigned property object")?;
                    let default = read
                        .data_source()
                        .as_shorthand_property_assignment()
                        .and_then(|data| data.object_assignment_initializer());
                    let assigned = self.flow_assigned_type(object)?;
                    let ty = self.flow_destructured_property_type(assigned, name)?;
                    self.flow_type_with_default(ty, default)
                }
                _ => Ok(self.builtins.error_type),
            }
        })
    }
    // port: tsc/internal/checker/flow.go:Checker.getTypeOfDestructuredProperty
    fn flow_destructured_property_type(
        &mut self,
        ty: TypeId,
        name: NodeId,
    ) -> Result<TypeId, Error> {
        let key = self.literal_type_from_property_name(name)?;
        let Some(name) = self.index_property_name(key)? else {
            return Ok(self.builtins.error_type);
        };
        if let Some(property) = self.property_type(ty, name.as_bytes())? {
            return Ok(property);
        }
        let key = self.get_string_literal_type(name)?;
        if let Some(info) = self.applicable_index_info(ty, key)? {
            return self.flow_include_index_undefined(self.signatures.index_info(info)?.value_type);
        }
        Ok(self.builtins.error_type)
    }
    // port: tsc/internal/checker/flow.go:Checker.getTypeOfDestructuredArrayElement
    fn flow_destructured_array_element(
        &mut self,
        ty: TypeId,
        index: isize,
    ) -> Result<TypeId, Error> {
        let mut tuples = true;
        for part in self.distributed_types(ty)? {
            tuples &= self.tuple_like_type(part)?;
        }
        if tuples {
            if let Some(element) = self.flow_tuple_element_type(ty, index)? {
                return Ok(element);
            }
        }
        let element = self.check_iterated_type_or_element_type(
            crate::iteration::ALLOW_SYNC | crate::iteration::DESTRUCTURING_FLAG,
            ty,
            self.builtins.undefined_type,
            None,
        )?;
        self.flow_include_index_undefined(element)
    }
    // port: tsc/internal/checker/checker.go:Checker.getTupleElementType
    fn flow_tuple_element_type(
        &mut self,
        ty: TypeId,
        index: isize,
    ) -> Result<Option<TypeId>, Error> {
        let name = JsString::from_bytes(index.to_string().as_bytes());
        if let Some(property) = self.property_type(ty, name.as_bytes())? {
            return Ok(Some(property));
        }
        for part in self.distributed_types(ty)? {
            if !self.is_tuple_type(part)? {
                return Ok(None);
            }
        }
        self.map_type(ty, &mut |checker, part| {
            let start = checker
                .types
                .tuple(checker.types.target(part)?)?
                .fixed_length as usize;
            let Some(rest) = checker.tuple_slice_element_type(part, start, 0, false)? else {
                return Ok(Some(checker.builtins.undefined_type));
            };
            if checker
                .program()?
                .host
                .options()
                .no_unchecked_indexed_access
                .is_true()
                && index >= checker.total_fixed_elements(part)? as isize
            {
                return checker
                    .get_union_type(&[rest, checker.builtins.undefined_type])
                    .map(Some);
            }
            Ok(Some(rest))
        })
    }
    // port: tsc/internal/checker/flow.go:Checker.getTypeOfDestructuredSpreadExpression
    fn flow_destructured_spread_type(&mut self, ty: TypeId) -> Result<TypeId, Error> {
        let element = self.check_iterated_type_or_element_type(
            crate::iteration::ALLOW_SYNC | crate::iteration::DESTRUCTURING_FLAG,
            ty,
            self.builtins.undefined_type,
            None,
        )?;
        self.create_array_type(element, false)
    }
    // port: tsc/internal/checker/flow.go:Checker.includeUndefinedInIndexSignature
    fn flow_include_index_undefined(&mut self, ty: TypeId) -> Result<TypeId, Error> {
        if self
            .program()?
            .host
            .options()
            .no_unchecked_indexed_access
            .is_true()
        {
            self.get_union_type(&[ty, self.builtins.missing_type])
        } else {
            Ok(ty)
        }
    }
    // port: tsc/internal/checker/flow.go:Checker.getTypeWithDefault
    fn flow_type_with_default(
        &mut self,
        ty: TypeId,
        default: Option<NodeId>,
    ) -> Result<TypeId, Error> {
        let Some(default) = default else {
            return Ok(ty);
        };
        let non_undefined = self.non_undefined_binding_type(ty)?;
        let default = self.get_type_of_expression(default)?;
        self.get_union_type(&[non_undefined, default])
    }
}
