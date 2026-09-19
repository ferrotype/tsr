//! Enum display emits ordinary synthetic type syntax, preserving duplicate
//! value aliases and quoted member names through the same printer as other types.

use super::NodeBuilder;
use crate::{type_flags as tf, Error, TypeId};
use ts_ast::{symbol_flags as sf, AstView, FactoryMethods, NodeId, SyntaxKind as K};

impl NodeBuilder<'_> {
    /// Moves a reference's entity names onto an import type qualifier or a type name.
    // port: tsc/internal/checker/nodebuilderimpl.go:NodeBuilderImpl.appendReferenceToType
    pub(super) fn append_reference_to_type(
        &mut self,
        root: NodeId,
        reference: NodeId,
    ) -> Result<NodeId, Error> {
        let view = self.ast.view();
        let ids = access_stack(view, reference)?;
        let arguments = view
            .node(reference)?
            .data_source()
            .as_type_reference_node()
            .ok_or(ts_arena::Error::InvalidGraph)?
            .type_arguments();
        let read = view.node(root)?;
        let kind = read.kind();
        let import = read.data_source().as_import_type_node().map(|data| {
            (
                data.is_type_of(),
                data.argument(),
                data.attributes(),
                data.qualifier(),
            )
        });
        let reference_root = read
            .data_source()
            .as_type_reference_node()
            .map(|data| (data.type_name(), data.type_arguments()));
        if kind == K::ImportType {
            let (is_type_of, argument, attributes, mut qualifier) =
                import.ok_or(ts_arena::Error::InvalidGraph)?;
            for id in ids {
                qualifier = Some(match qualifier {
                    Some(left) => self.ast.new_qualified_name(Some(left), Some(id)),
                    None => id,
                });
            }
            return Ok(self.ast.update_import_type_node(
                root, is_type_of, argument, attributes, qualifier, arguments,
            ));
        }
        if kind == K::TypeReference {
            let (type_name, root_arguments) =
                reference_root.ok_or(ts_arena::Error::InvalidGraph)?;
            if self.flags & ts_nodebuilder::flags::USE_INSTANTIATION_EXPRESSIONS != 0 {
                if let Some(list) = root_arguments {
                    if !self.ast.view().list(list)?.nodes().is_empty() {
                        return Err(Error::Unsupported(
                            "appendReferenceToType instantiation expression",
                        ));
                    }
                }
            }
            let mut type_name = type_name.ok_or(Error::MissingLink("appended reference name"))?;
            for id in ids {
                type_name = self.ast.new_qualified_name(Some(type_name), Some(id));
            }
            return Ok(self
                .ast
                .update_type_reference_node(root, Some(type_name), arguments));
        }
        Err(Error::Unsupported(
            "appendReferenceToType access expression",
        ))
    }

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
            let parent_name = self.type_reference(parent, &[])?;
            if self.checker.get_declared_type_of_symbol(parent)? == ty {
                return Ok(Some(parent_name));
            }
            let name = self.checker.symbol(symbol)?.name_to_owned();
            if ts_scanner::is_identifier_text(name.as_bytes(), ts_core::LanguageVariant::STANDARD) {
                let member = self.ast.new_identifier(name);
                let reference = self.ast.new_type_reference_node(Some(member), None);
                return self
                    .append_reference_to_type(parent_name, reference)
                    .map(Some);
            }
            let (kind, import, type_name) = {
                let read = self.ast.view().node(parent_name)?;
                let import = read.data_source().as_import_type_node().map(|data| {
                    (
                        data.argument(),
                        data.attributes(),
                        data.qualifier(),
                        data.type_arguments(),
                    )
                });
                let type_name = read
                    .data_source()
                    .as_type_reference_node()
                    .and_then(|data| data.type_name());
                (read.kind(), import, type_name)
            };
            let object = if kind == K::ImportType {
                let (argument, attributes, qualifier, arguments) =
                    import.ok_or(ts_arena::Error::InvalidGraph)?;
                // Native code sets IsTypeOf on the import type it just built.
                self.ast
                    .new_import_type_node(true, argument, attributes, qualifier, arguments)
            } else if kind == K::TypeReference {
                self.ast.new_type_query_node(type_name, None)
            } else {
                return Err(Error::Unsupported(
                    "enum member parent type node is neither an import type nor a reference",
                ));
            };
            let name = self.string_literal(name);
            let index = self.ast.new_literal_type_node(Some(name));
            return Ok(Some(
                self.ast
                    .new_indexed_access_type_node(Some(object), Some(index)),
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

// port: tsc/internal/checker/nodebuilderimpl.go:getAccessStack
fn access_stack(view: AstView<'_>, reference: NodeId) -> Result<Vec<NodeId>, Error> {
    let mut state = view
        .node(reference)?
        .data_source()
        .as_type_reference_node()
        .and_then(|data| data.type_name())
        .ok_or(Error::MissingLink("access stack type name"))?;
    let mut ids = Vec::new();
    while view.node(state)?.kind() != K::Identifier {
        let node = view.node(state)?;
        let name = node
            .data_source()
            .as_qualified_name()
            .ok_or(ts_arena::Error::InvalidGraph)?;
        ids.push(
            name.right()
                .ok_or(Error::MissingLink("access stack right"))?,
        );
        state = name.left().ok_or(Error::MissingLink("access stack left"))?;
    }
    ids.push(state);
    ids.reverse();
    Ok(ids)
}
