//! Enum display emits ordinary synthetic type syntax, preserving duplicate
//! value aliases and quoted member names through the same printer as other types.

use super::NodeBuilder;
use crate::{type_flags as tf, Error, TypeId};
use ts_ast::{symbol_flags as sf, FactoryMethods, NodeId};

impl NodeBuilder<'_> {
    // port: tsc/internal/checker/nodebuilderimpl.go:NodeBuilderImpl.typeToTypeNode
    pub(super) fn enum_type_node(
        &mut self,
        ty: TypeId,
        expanding: bool,
    ) -> Result<Option<NodeId>, Error> {
        let record = *self.checker.types.get(ty)?;
        let symbol = record
            .symbol
            .ok_or(Error::MissingLink("enum display symbol"))?;
        if self.checker.symbol(symbol)?.flags() & sf::ENUM_MEMBER != 0 {
            let parent = self
                .checker
                .parent_of_symbol(symbol)?
                .ok_or(Error::MissingLink("enum display parent"))?;
            let parent_node = self.type_reference(parent, &[])?;
            if self.checker.get_declared_type_of_symbol(parent)? == ty {
                return Ok(Some(parent_node));
            }
            let name = self.checker.symbol(symbol)?.name_to_owned();
            let parent_name = self
                .ast
                .view()
                .node(parent_node)?
                .data_source()
                .as_type_reference_node()
                .ok_or(Error::MissingLink("enum parent reference"))?
                .type_name()
                .ok_or(Error::MissingLink("enum parent reference name"))?;
            if ts_scanner::is_identifier_text(name.as_bytes(), ts_core::LanguageVariant::STANDARD) {
                self.approximate_length += name.len() + 1;
                let member = self.ast.new_identifier(name);
                let qualified = self.ast.new_qualified_name(Some(parent_name), Some(member));
                return Ok(Some(
                    self.ast.new_type_reference_node(Some(qualified), None),
                ));
            }
            let query = self.ast.new_type_query_node(Some(parent_name), None);
            let name = self.string_literal(name);
            let index = self.ast.new_literal_type_node(Some(name));
            return Ok(Some(
                self.ast
                    .new_indexed_access_type_node(Some(query), Some(index)),
            ));
        }
        if record.flags & tf::UNION == 0 || !expanding {
            return self.type_reference(symbol, &[]).map(Some);
        }
        Ok(None)
    }

    // port: tsc/internal/checker/printer.go:Checker.formatUnionTypes
    pub(super) fn format_union_with_enums(
        &mut self,
        types: &[TypeId],
        expanding_enum: bool,
    ) -> Result<Vec<TypeId>, Error> {
        let mut result = Vec::new();
        let mut flags = 0;
        let mut index = 0;
        while index < types.len() {
            let ty = types[index];
            let part_flags = self.checker.types.flags(ty)?;
            flags |= part_flags;
            if part_flags & tf::NULLABLE == 0 {
                if part_flags & tf::BOOLEAN_LITERAL != 0
                    || !expanding_enum && part_flags & tf::ENUM_LIKE != 0
                {
                    let base = if part_flags & tf::BOOLEAN_LITERAL != 0 {
                        self.checker.builtins.boolean_type
                    } else {
                        self.checker.base_type_of_enum_like(ty)?
                    };
                    if self.checker.types.flags(base)? & tf::UNION != 0 {
                        let parts = self.checker.types.union(base)?.types.clone();
                        let count = parts.len();
                        if index + count <= types.len()
                            && self
                                .checker
                                .get_regular_type_of_literal_type(types[index + count - 1])?
                                == self
                                    .checker
                                    .get_regular_type_of_literal_type(parts[count - 1])?
                        {
                            result.push(base);
                            index += count;
                            continue;
                        }
                    }
                }
                result.push(ty);
            }
            index += 1;
        }
        if flags & tf::NULL != 0 {
            result.push(self.checker.builtins.null_type);
        }
        if flags & tf::UNDEFINED != 0 {
            result.push(self.checker.builtins.undefined_type);
        }
        Ok(result)
    }
}
