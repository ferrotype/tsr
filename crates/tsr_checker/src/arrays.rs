//! Array references and tuple normalization. Array identities come from the
//! loaded program's global declarations; an absent global uses Go's sentinel.

use crate::{
    element_flags as ef, object_flags as of, type_flags as tf, CheckerState, Error, ObjectFlags,
    TupleElementInfo, TypeId, TypeList,
};
use tsr_arena::NodeId;
use tsr_ast::SyntaxKind as K;

#[derive(Default)]
struct TupleNormalizer {
    types: Vec<TypeId>,
    infos: Vec<TupleElementInfo>,
    last_required: Option<usize>,
    first_rest: Option<usize>,
    last_optional_or_rest: Option<usize>,
}

impl TupleNormalizer {
    // port: tsc/internal/checker/checker.go:TupleNormalizer.add
    fn add(
        &mut self,
        checker: &mut CheckerState,
        ty: TypeId,
        info: TupleElementInfo,
    ) -> Result<(), Error> {
        let index = self.types.len();
        if info.flags & ef::REQUIRED != 0 {
            self.last_required = Some(index);
        }
        if info.flags & ef::REST != 0 {
            self.first_rest.get_or_insert(index);
        }
        if info.flags & (ef::OPTIONAL | ef::REST) != 0 {
            self.last_optional_or_rest = Some(index);
        }
        self.types
            .push(checker.add_type_optionality(ty, true, info.flags & ef::OPTIONAL != 0)?);
        self.infos.push(info);
        Ok(())
    }
}

impl CheckerState {
    // port: tsc/internal/checker/relater.go:Checker.getKnownKeysOfTupleType
    pub(crate) fn known_tuple_keys(&mut self, ty: TypeId) -> Result<TypeId, Error> {
        let tuple = self.types.tuple(self.types.target(ty)?)?;
        let length = tuple.fixed_length;
        let readonly = tuple.readonly;
        let mut keys = Vec::with_capacity(length as usize + 1);
        for i in 0..length {
            keys.push(self.get_string_literal_type(tsr_ast::JsString::from_bytes(
                i.to_string().into_bytes(),
            ))?);
        }
        keys.push(self.get_index_type(self.array_target(readonly)?, 0)?);
        self.get_union_type(&keys)
    }

    pub(crate) fn array_target(&self, readonly: bool) -> Result<TypeId, Error> {
        self.query
            .global_types
            .get(if readonly { "ReadonlyArray" } else { "Array" })
            .copied()
            .ok_or(Error::MissingLink("program array global"))
    }

    // port: tsc/internal/checker/checker.go:Checker.createArrayTypeEx
    pub(crate) fn create_array_type(
        &mut self,
        element: TypeId,
        readonly: bool,
    ) -> Result<TypeId, Error> {
        self.type_from_generic_global(self.array_target(readonly)?, element)
    }

    // port: tsc/internal/checker/checker.go:Checker.isArrayType
    pub(crate) fn is_array_type(&self, ty: TypeId) -> Result<bool, Error> {
        if self.types.get(ty)?.object_flags & of::REFERENCE == 0 {
            return Ok(false);
        }
        let target = self.types.target(ty)?;
        Ok(self.query.global_types.get("Array") == Some(&target)
            || self.query.global_types.get("ReadonlyArray") == Some(&target))
    }

    // port: tsc/internal/checker/checker.go:Checker.isArrayLikeType
    pub(crate) fn is_array_like_type(&mut self, ty: TypeId) -> Result<bool, Error> {
        if self.is_array_type(ty)? {
            return Ok(true);
        }
        if self.types.flags(ty)? & tf::NULLABLE != 0 {
            return Ok(false);
        }
        let target = self
            .query
            .global_types
            .get("anyReadonlyArrayType")
            .copied()
            .ok_or(Error::MissingLink("readonly array global"))?;
        self.source_type_assignable(ty, target, &mut Vec::new())
    }

    // port: tsc/internal/checker/checker.go:Checker.addOptionalityEx
    pub(crate) fn add_type_optionality(
        &mut self,
        ty: TypeId,
        property: bool,
        optional: bool,
    ) -> Result<TypeId, Error> {
        if !optional || !self.options.strict_null_checks {
            return Ok(ty);
        }
        let undefined = if property {
            self.builtins.undefined_or_missing_type
        } else {
            self.builtins.undefined_type
        };
        self.get_union_type(&[ty, undefined])
    }

    // port: tsc/internal/checker/checker.go:Checker.getArrayElementTypeNode
    pub(crate) fn array_element_type_node(&self, node: NodeId) -> Result<Option<NodeId>, Error> {
        let read = self.node(node)?;
        match read.kind().known() {
            Some(K::ParenthesizedType) => self.array_element_type_node(
                read.type_node()
                    .ok_or(Error::MissingLink("parenthesized type"))?,
            ),
            Some(K::ArrayType) => Ok(read
                .data_source()
                .as_array_type_node()
                .ok_or(Error::MissingLink("array type"))?
                .element_type()),
            Some(K::TupleType) => {
                let elements = self.source_list(node, read.element_list())?;
                if elements.len() == 1 {
                    let read = self.ast(elements[0])?.node(elements[0])?;
                    if read.kind() == K::RestType
                        || read
                            .data_source()
                            .as_named_tuple_member()
                            .is_some_and(|data| data.dot_dot_dot_token().is_some())
                    {
                        return self.array_element_type_node(
                            read.type_node()
                                .ok_or(Error::MissingLink("tuple rest type"))?,
                        );
                    }
                }
                Ok(None)
            }
            _ => Ok(None),
        }
    }

    // port: tsc/internal/checker/checker.go:Checker.getTupleElementInfo
    // port: tsc/internal/checker/checker.go:Checker.getTupleElementFlags
    pub(crate) fn tuple_element_info(&self, node: NodeId) -> Result<TupleElementInfo, Error> {
        let read = self.node(node)?;
        let named = read.kind() == K::NamedTupleMember;
        let flags = if read.kind() == K::OptionalType
            || named && read.question_token(self.ast(node)?)?.is_some()
        {
            ef::OPTIONAL
        } else if read.kind() == K::RestType
            || read
                .data_source()
                .as_named_tuple_member()
                .is_some_and(|data| data.dot_dot_dot_token().is_some())
        {
            if self
                .array_element_type_node(read.type_node().ok_or(Error::MissingLink("rest type"))?)?
                .is_some()
            {
                ef::REST
            } else {
                ef::VARIADIC
            }
        } else {
            ef::REQUIRED
        };
        Ok(TupleElementInfo {
            flags,
            labeled_declaration: (named || read.kind() == K::Parameter).then_some(node),
        })
    }

    // port: tsc/internal/checker/checker.go:Checker.getTypeFromArrayOrTupleTypeNode
    pub(crate) fn source_array_or_tuple_type(&mut self, node: NodeId) -> Result<TypeId, Error> {
        let read = self.node(node)?;
        let readonly = match read.parent() {
            Some(parent) => self
                .ast(parent)?
                .node(parent)?
                .data_source()
                .as_type_operator_node()
                .is_some_and(|data| data.operator() == K::ReadonlyKeyword),
            None => false,
        };
        let array = read.kind() == K::ArrayType;
        let elements = if array {
            Vec::new()
        } else {
            self.source_list(node, read.element_list())?
        };
        let mut infos = Vec::new();
        for &element in &elements {
            infos.push(self.tuple_element_info(element)?);
        }
        let target = if self.array_element_type_node(node)?.is_some() {
            self.array_target(readonly)?
        } else {
            self.get_tuple_target_type(&infos, readonly)?
        };
        if target == self.builtins.empty_generic_type {
            return Ok(self.builtins.empty_object_type);
        }
        if !infos.iter().any(|info| info.flags & ef::VARIADIC != 0)
            && self.is_deferred_type_reference_node(node, false)?
        {
            return if !array && elements.is_empty() {
                Ok(target)
            } else {
                self.create_deferred_type_reference(target, node, None, None)
            };
        }
        let mut types = Vec::new();
        if array {
            let element = self
                .array_element_type_node(node)?
                .ok_or(Error::MissingLink("array element"))?;
            types.push(self.get_type_from_type_node(element)?);
        } else {
            for element in elements {
                types.push(self.get_type_from_type_node(element)?);
            }
        }
        if self.types.get(target)?.object_flags & of::TUPLE != 0 {
            self.create_normalized_tuple_type_ex(target, &types, of::FROM_TYPE_NODE)
        } else {
            self.create_type_reference_ex(target, &types, of::FROM_TYPE_NODE)
        }
    }

    // port: tsc/internal/checker/checker.go:Checker.getTypeFromNamedTupleTypeNode
    // port: tsc/internal/checker/checker.go:Checker.getTypeFromRestTypeNode
    // port: tsc/internal/checker/checker.go:Checker.getTypeFromOptionalTypeNode
    pub(crate) fn source_tuple_element_type(&mut self, node: NodeId) -> Result<TypeId, Error> {
        let read = self.node(node)?;
        let mut annotation = read
            .type_node()
            .ok_or(Error::MissingLink("tuple element type"))?;
        let info = self.tuple_element_info(node)?;
        if info.flags & ef::VARIABLE != 0 {
            annotation = self
                .array_element_type_node(annotation)?
                .unwrap_or(annotation);
        }
        let ty = self.get_type_from_type_node(annotation)?;
        self.add_type_optionality(ty, true, info.flags & ef::OPTIONAL != 0)
    }

    /// The target's type parameter count, without its `this` type. A reference
    /// resolved through a `this` argument carries one more type argument than
    /// the target has elements, so tuple arities never come from the argument
    /// list.
    // port: tsc/internal/checker/checker.go:Checker.getTypeReferenceArity
    pub(crate) fn get_type_reference_arity(&self, ty: TypeId) -> Result<usize, Error> {
        Ok(self
            .types
            .interface(self.types.target(ty)?)?
            .type_parameters()
            .len())
    }

    // port: tsc/internal/checker/checker.go:Checker.getElementTypes
    pub(crate) fn element_types(&mut self, ty: TypeId) -> Result<TypeList, Error> {
        let arguments = self.get_type_arguments(ty)?;
        let arity = self.get_type_reference_arity(ty)?;
        Ok(if arguments.len() == arity {
            arguments
        } else {
            arguments[..arity].into()
        })
    }

    // port: tsc/internal/checker/checker.go:Checker.getTupleBaseType
    pub(crate) fn tuple_base_type(&mut self, ty: TypeId) -> Result<TypeId, Error> {
        let data = self.types.tuple(ty)?;
        let readonly = data.readonly;
        let mut parameters = data.interface.type_parameters().to_vec();
        let infos = data.element_infos.clone();
        if data.combined_flags & ef::VARIADIC != 0 {
            for (parameter, info) in parameters.iter_mut().zip(infos.iter()) {
                if info.flags & ef::VARIADIC != 0 {
                    *parameter = self.get_indexed_access_type(
                        *parameter,
                        self.builtins.number_type,
                        0,
                        None,
                        None,
                    )?;
                }
            }
        }
        let union = self.get_union_type(&parameters)?;
        self.create_array_type(union, readonly)
    }

    // port: tsc/internal/checker/checker.go:Checker.createNormalizedTupleTypeEx
    // port: tsc/internal/checker/checker.go:TupleNormalizer.normalize
    pub(crate) fn normalize_tuple(
        &mut self,
        target: TypeId,
        types: &[TypeId],
        flags: ObjectFlags,
    ) -> Result<TypeId, Error> {
        let data = self.types.tuple(target)?;
        let readonly = data.readonly;
        let infos = data.element_infos.clone();
        for (index, info) in infos.iter().enumerate() {
            if info.flags & ef::VARIADIC != 0 {
                let ty = types[index];
                let type_flags = self.types.flags(ty)?;
                if type_flags & tf::NEVER != 0 {
                    return Ok(ty);
                }
                if type_flags & tf::UNION != 0 {
                    let check: Vec<_> = types
                        .iter()
                        .enumerate()
                        .map(|(i, &ty)| {
                            if infos
                                .get(i)
                                .is_some_and(|info| info.flags & ef::VARIADIC != 0)
                            {
                                ty
                            } else {
                                self.builtins.unknown_type
                            }
                        })
                        .collect();
                    if self.check_cross_product_union(&check)? {
                        let parts = self.types.union(ty)?.types.clone();
                        let mut result = Vec::new();
                        for &part in parts.iter() {
                            let mut replaced = types.to_vec();
                            replaced[index] = part;
                            result.push(
                                self.create_normalized_tuple_type_ex(target, &replaced, flags)?,
                            );
                        }
                        return self.get_union_type(&result);
                    }
                }
            }
        }
        let mut normal = TupleNormalizer::default();
        for (&ty, &info) in types.iter().zip(infos.iter()) {
            if info.flags & ef::VARIADIC == 0 {
                normal.add(self, ty, info)?;
                continue;
            }
            let type_flags = self.types.flags(ty)?;
            if type_flags & tf::ANY != 0 {
                normal.add(
                    self,
                    ty,
                    TupleElementInfo {
                        flags: ef::REST,
                        ..info
                    },
                )?;
            } else if type_flags & tf::INSTANTIABLE_NON_PRIMITIVE != 0
                || self.is_generic_mapped_type(ty)?
            {
                normal.add(self, ty, info)?;
            } else if self.is_tuple_type(ty)? {
                let elements = self.element_types(ty)?;
                if elements.len() + normal.types.len() >= 10_000 {
                    let type_node = self
                        .current_node
                        .map(|node| self.is_part_of_type_node(node))
                        .transpose()?
                        .unwrap_or(false);
                    let message = if type_node {
                        tsr_diagnostics::Type_produces_a_tuple_type_that_is_too_large_to_represent
                    } else {
                        tsr_diagnostics::Expression_produces_a_tuple_type_that_is_too_large_to_represent
                    };
                    self.error_at(self.current_node, message, vec![])?;
                    return Ok(self.builtins.error_type);
                }
                let spread = self
                    .types
                    .tuple(self.types.target(ty)?)?
                    .element_infos
                    .clone();
                for (&ty, &info) in elements.iter().zip(spread.iter()) {
                    normal.add(self, ty, info)?;
                }
            } else {
                let element = if self.is_array_like_type(ty)? {
                    self.numeric_index_type(ty)?
                        .unwrap_or(self.builtins.error_type)
                } else {
                    self.builtins.error_type
                };
                normal.add(
                    self,
                    element,
                    TupleElementInfo {
                        flags: ef::REST,
                        ..info
                    },
                )?;
            }
        }
        if let Some(last) = normal.last_required {
            for info in &mut normal.infos[..last] {
                if info.flags & ef::OPTIONAL != 0 {
                    info.flags = ef::REQUIRED;
                }
            }
        }
        if let (Some(first), Some(last)) = (normal.first_rest, normal.last_optional_or_rest) {
            if first < last {
                let mut collapsed = Vec::with_capacity(last - first + 1);
                for index in first..=last {
                    let ty = normal.types[index];
                    collapsed.push(if normal.infos[index].flags & ef::VARIADIC != 0 {
                        self.get_indexed_access_type(ty, self.builtins.number_type, 0, None, None)?
                    } else {
                        ty
                    });
                }
                normal.types[first] = self.get_union_type(&collapsed)?;
                normal.types.drain(first + 1..=last);
                normal.infos.drain(first + 1..=last);
            }
        }
        if types.len() > infos.len() {
            normal.types.push(types[infos.len()]);
        }
        let target = self.get_tuple_target_type(&normal.infos, readonly)?;
        if target == self.builtins.empty_generic_type {
            Ok(self.builtins.empty_object_type)
        } else if !normal.types.is_empty() {
            self.create_type_reference_ex(target, &normal.types, flags)
        } else {
            Ok(target)
        }
    }

    pub(crate) fn numeric_index_type(&mut self, ty: TypeId) -> Result<Option<TypeId>, Error> {
        let index = self.index_info_of_type(ty, self.builtins.number_type)?;
        index
            .map(|index| {
                self.signatures
                    .index_info(index)
                    .map(|info| info.value_type)
            })
            .transpose()
    }
}
