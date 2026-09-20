use crate::{
    element_flags as ef, object_flags as of, type_flags as tf, CheckerState, Error, RelationKind,
    TypeId,
};

impl CheckerState {
    // port: tsc/internal/checker/inference.go:Checker.createEmptyObjectTypeFromStringLiteral
    pub(crate) fn empty_object_from_literal(&mut self, ty: TypeId) -> Result<TypeId, Error> {
        let mut members = tsr_ast::SymbolTable::default();
        for part in self.distributed_types(ty)? {
            if self.types.flags(part)? & tf::STRING_LITERAL == 0 {
                continue;
            }
            let crate::LiteralValue::String(name) = self.types.literal(part)?.value.clone() else {
                return Err(Error::MissingLink("literal property name"));
            };
            let property = self.new_symbol(tsr_ast::symbol_flags::PROPERTY, name.clone())?;
            self.value_symbol_links
                .get_or_default(property)
                .resolved_type = Some(self.builtins.any_type);
            if let Some(symbol) = self.types.get(part)?.symbol {
                let declarations = self.symbol(symbol)?.declarations();
                let value = self.symbol(symbol)?.value_declaration();
                let prop = self.symbol_mut(property)?;
                prop.declarations = declarations;
                prop.value_declaration = value;
            }
            members.insert(name, Some(property));
        }
        let indexes = if self.types.flags(ty)? & tf::STRING != 0 {
            vec![self.signatures.new_index_info(
                self.builtins.string_type,
                self.builtins.empty_object_type,
                false,
                None,
                None,
            )?]
        } else {
            Vec::new()
        };
        let table = self.alloc_symbol_table(members);
        self.new_anonymous_type(None, Some(table), &[], &[], &indexes)
    }

    // port: tsc/internal/checker/checker.go:Checker.isConstTypeVariable
    pub(crate) fn is_const_type_variable(
        &mut self,
        ty: TypeId,
        depth: usize,
    ) -> Result<bool, Error> {
        if depth >= 5 {
            return Ok(false);
        }
        let record = *self.types.get(ty)?;
        if record.flags & tf::TYPE_PARAMETER != 0 {
            if let Some(symbol) = record.symbol {
                for node in self.symbol_declarations(symbol)?.iter().flatten() {
                    if self
                        .ast(node)?
                        .node(node)?
                        .modifier_flags(self.ast(node)?)?
                        & tsr_ast::modifier_flags::CONST
                        != 0
                    {
                        return Ok(true);
                    }
                }
            }
        } else if record.flags & tf::UNION_OR_INTERSECTION != 0 {
            for part in self.types.types_of(ty)?.to_vec() {
                if self.is_const_type_variable(part, depth)? {
                    return Ok(true);
                }
            }
        } else if record.flags & tf::INDEXED_ACCESS != 0 {
            return self
                .is_const_type_variable(self.types.indexed_access(ty)?.object_type, depth + 1);
        } else if record.flags & tf::CONDITIONAL != 0 {
            let constraint = self.constraint_from_conditional(ty)?;
            return self.is_const_type_variable(constraint, depth + 1);
        } else if record.flags & tf::SUBSTITUTION != 0 {
            return self.is_const_type_variable(self.types.substitution(ty)?.base, depth);
        } else if record.object_flags & of::MAPPED != 0 {
            if let Some(variable) = self.homomorphic_type_variable(ty)? {
                return self.is_const_type_variable(variable, depth);
            }
        } else if self.is_generic_tuple_type(ty)? {
            let elements = self.element_types(ty)?;
            let infos = self
                .types
                .tuple(self.types.target(ty)?)?
                .element_infos
                .clone();
            for (&element, info) in elements.iter().zip(infos.iter()) {
                if info.flags & ef::VARIADIC != 0 && self.is_const_type_variable(element, depth)? {
                    return Ok(true);
                }
            }
        }
        Ok(false)
    }

    // port: tsc/internal/checker/checker.go:Checker.isMutableArrayLikeType
    pub(crate) fn some_mutable_array_like(&mut self, ty: TypeId) -> Result<bool, Error> {
        for part in self.distributed_types(ty)? {
            if (self.is_array_type(part)? || self.is_tuple_type(part)?)
                && !self.readonly_array_or_tuple(part)?
            {
                return Ok(true);
            }
            if self.types.flags(part)? & (tf::ANY | tf::NULLABLE) == 0 {
                let array = *self
                    .query
                    .global_types
                    .get("anyArrayType")
                    .ok_or(Error::MissingLink("any array global"))?;
                if self.is_type_related_to(part, array, RelationKind::Assignable)? {
                    return Ok(true);
                }
            }
        }
        Ok(false)
    }
}
