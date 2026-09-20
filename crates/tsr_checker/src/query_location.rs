//! Public location queries follow contextual syntax classification. Checking an
//! identifier expression directly is not equivalent to querying a property name.
use crate::{CheckerState, Error, TypeId};
use tsr_arena::NodeId;
use tsr_ast::{node_flags as nf, AstView, SyntaxKind as K};

// port: tsc/internal/ast/utilities.go:IsDeclarationNameOrImportPropertyName
pub(super) fn declaration_or_import_name(view: AstView<'_>, node: NodeId) -> Result<bool, Error> {
    let read = view.node(node)?;
    let Some(parent) = read.parent() else {
        return Ok(false);
    };
    let parent = view.node(parent)?;
    Ok(
        if matches!(
            parent.kind().known(),
            Some(K::ImportSpecifier | K::ExportSpecifier)
        ) {
            matches!(read.kind().known(), Some(K::Identifier | K::StringLiteral))
        } else {
            read.kind() != K::SourceFile
                && !tsr_ast::utilities::is_binding_pattern(&read)
                && tsr_ast::is_declaration(&parent)
                && parent.name() == Some(node)
        },
    )
}

// port: tsc/internal/checker/utilities.go:isInRightSideOfImportOrExportAssignment
fn import_or_export_assignment(view: AstView<'_>, mut node: NodeId) -> Result<bool, Error> {
    while let Some(parent) = view.node(node)?.parent() {
        if view.node(parent)?.kind() != K::QualifiedName {
            break;
        }
        node = parent;
    }
    let Some(parent) = view.node(node)?.parent() else {
        return Ok(false);
    };
    let read = view.node(parent)?;
    Ok(match read.kind().known() {
        Some(K::ImportEqualsDeclaration) => {
            read.data_source()
                .as_import_equals_declaration()
                .and_then(|d| d.module_reference())
                == Some(node)
        }
        Some(K::ExportAssignment) => read.expression() == Some(node),
        _ => false,
    })
}

impl CheckerState {
    /// The literal arms of getSymbolAtLocation. Ordinary call arguments have
    /// no symbol; only module names and indexed property names trigger lookup.
    pub(crate) fn symbol_at_literal_location(
        &mut self,
        node: NodeId,
    ) -> Result<Option<tsr_arena::SymbolId>, Error> {
        let view = self.ast(node)?;
        let read = view.node(node)?;
        let Some(parent) = read.parent() else {
            return Ok(None);
        };
        let parent_read = view.node(parent)?;
        let grandparent = parent_read.parent();
        if read.kind() != K::NumericLiteral {
            let import_equals = if let Some(grand) = grandparent {
                let g = view.node(grand)?;
                g.kind() == K::ImportEqualsDeclaration
                    && parent_read.kind() == K::ExternalModuleReference
                    && parent_read.expression() == Some(node)
            } else {
                false
            };
            let module_declaration = matches!(
                parent_read.kind().known(),
                Some(K::ImportDeclaration | K::JSImportDeclaration | K::ExportDeclaration)
            ) && parent_read.module_specifier() == Some(node);
            let require = grandparent
                .map(|g| tsr_ast::is_variable_declaration_initialized_to_require(view, g))
                .transpose()?
                .unwrap_or(false);
            let import_type = if let Some(grand) = grandparent {
                let g = view.node(grand)?;
                parent_read.kind() == K::LiteralType
                    && g.kind() == K::ImportType
                    && g.data_source()
                        .as_import_type_node()
                        .and_then(|d| d.argument())
                        == Some(parent)
            } else {
                false
            };
            if import_equals
                || module_declaration
                || require
                || import_type
                || crate::external_resolution::is_import_call(view, &parent_read)?
            {
                return self.resolve_external_module_name(node, node, true);
            }
            if parent_read.kind() == K::CallExpression
                && tsr_ast::is_bindable_object_define_property_call(view, parent)?
            {
                let args = parent_read.arguments(view)?;
                if view.node_slice(args)?.get(1) == Some(Some(node)) {
                    return self.get_symbol_of_declaration(parent);
                }
            }
        }
        let object = if parent_read.kind() == K::ElementAccessExpression {
            let data = parent_read
                .data_source()
                .as_element_access_expression()
                .ok_or(Error::MissingLink("element access payload"))?;
            if data.argument_expression() == Some(node) {
                Some(
                    self.get_type_of_expression(
                        data.expression()
                            .ok_or(Error::MissingLink("element access receiver"))?,
                    )?,
                )
            } else {
                None
            }
        } else if parent_read.kind() == K::LiteralType {
            if let Some(grand) = grandparent {
                let read = view.node(grand)?;
                if let Some(data) = read.data_source().as_indexed_access_type_node() {
                    Some(
                        self.get_type_from_type_node(
                            data.object_type()
                                .ok_or(Error::MissingLink("indexed access object type"))?,
                        )?,
                    )
                } else {
                    None
                }
            } else {
                None
            }
        } else {
            None
        };
        match object {
            Some(object) => {
                let name = self.node_text(node)?.into_js_string();
                self.constituent_property(object, name.as_bytes(), false)
            }
            None => Ok(None),
        }
    }

    // port: tsc/internal/ast/utilities.go:TryGetClassImplementingOrExtendingHeritageClauseElement
    fn heritage_class(&self, node: NodeId) -> Result<Option<(NodeId, bool)>, Error> {
        let read = self.node(node)?;
        if !matches!(
            read.kind().known(),
            Some(K::ExpressionWithTypeArguments | K::TypeReference)
        ) {
            return Ok(None);
        }
        let Some(parent) = read.parent() else {
            return Ok(None);
        };
        let parent = self.node(parent)?;
        if parent.kind() != K::HeritageClause {
            return Ok(None);
        }
        let Some(class) = parent.parent() else {
            return Ok(None);
        };
        if !matches!(
            self.node(class)?.kind().known(),
            Some(K::ClassDeclaration | K::ClassExpression)
        ) {
            return Ok(None);
        }
        let heritage = parent
            .data_source()
            .as_heritage_clause()
            .ok_or(Error::MissingLink("heritage payload"))?;
        Ok(Some((class, heritage.token() == K::ImplementsKeyword)))
    }

    // port: tsc/internal/checker/checker.go:Checker.GetTypeAtLocation
    // port: tsc/internal/checker/checker.go:Checker.getTypeOfNode
    pub(crate) fn get_type_at_location(&mut self, node: NodeId) -> Result<TypeId, Error> {
        stacker::maybe_grow(128 * 1024, 2 * 1024 * 1024, || {
            self.type_of_location_worker(node)
        })
    }

    fn type_of_location_worker(&mut self, node: NodeId) -> Result<TypeId, Error> {
        let read = self.node(node)?;
        if read.kind() == K::SourceFile
            && !tsr_ast::utilities::is_external_or_common_js_module(&self.source_file_read(node)?)
            || read.flags() & nf::IN_WITH_STATEMENT != 0
        {
            return Ok(self.builtins.error_type);
        }
        let kind = read.kind();
        let parent = read.parent();
        let class_type = if let Some((class, implements)) = self.heritage_class(node)? {
            let symbol = self
                .get_symbol_of_declaration(class)?
                .ok_or(Error::MissingLink("heritage class symbol"))?;
            Some((self.declared_interface_type(symbol)?, implements))
        } else {
            None
        };
        if self.is_part_of_type_node(node)? {
            let ty = self.get_type_from_type_node(node)?;
            if let Some((class, _)) = class_type {
                let this = self
                    .types
                    .interface(class)?
                    .this_type
                    .ok_or(Error::MissingLink("class this type"))?;
                return self.get_type_with_this_argument(ty, this, false);
            }
            return Ok(ty);
        }
        if self.expression_node(node)? {
            return self.regular_type_of_expression(node);
        }
        if let Some((class, false)) = class_type {
            if let Some(&base) = self.interface_base_types(class)?.first() {
                let this = self
                    .types
                    .interface(class)?
                    .this_type
                    .ok_or(Error::MissingLink("class this type"))?;
                return self.get_type_with_this_argument(base, this, false);
            }
            return Ok(self.builtins.error_type);
        }
        if self.is_type_declaration(node)? {
            let symbol = self
                .get_symbol_of_declaration(node)?
                .ok_or(Error::MissingLink("type declaration symbol"))?;
            return self.get_declared_type_of_symbol(symbol);
        }
        if kind == K::Identifier {
            if let Some(parent) = parent {
                if self.is_type_declaration(parent)? && self.node(parent)?.name() == Some(node) {
                    return match self.get_symbol_at_location(node)? {
                        Some(symbol) => self.get_declared_type_of_symbol(symbol),
                        None => Ok(self.builtins.error_type),
                    };
                }
            }
        }
        if kind == K::BindingElement {
            return Ok(self
                .type_for_variable_like_raw(node, true, 0 /* CheckModeNormal */)?
                .unwrap_or(self.builtins.error_type));
        }
        if tsr_ast::is_declaration(&self.node(node)?) {
            return match self.get_symbol_of_declaration(node)? {
                Some(symbol) => self.get_type_of_symbol(symbol),
                None => Ok(self.builtins.error_type),
            };
        }
        if declaration_or_import_name(self.ast(node)?, node)? {
            return match self.get_symbol_at_location(node)? {
                Some(symbol) => self.get_type_of_symbol(symbol),
                None => Ok(self.builtins.error_type),
            };
        }
        if tsr_ast::utilities::is_binding_pattern(&self.node(node)?) {
            let parent = parent.ok_or(Error::MissingLink("binding pattern parent"))?;
            return Ok(self
                .type_for_variable_like_raw(parent, true, 0 /* CheckModeNormal */)?
                .unwrap_or(self.builtins.error_type));
        }
        if import_or_export_assignment(self.ast(node)?, node)? {
            if let Some(symbol) = self.get_symbol_at_location(node)? {
                let declared = self.get_declared_type_of_symbol(symbol)?;
                if !self.is_error_type(declared)? {
                    return Ok(declared);
                }
                return self.get_type_of_symbol(symbol);
            }
        }
        if let Some(parent) = parent {
            if let Some(meta) = self
                .ast(parent)?
                .node(parent)?
                .data_source()
                .as_meta_property()
            {
                if meta.keyword_token() == kind {
                    return self.check_meta_property(parent);
                }
            }
        }
        if kind == K::ImportAttributes {
            return self.import_attributes_expression_type(node);
        }
        Ok(self.builtins.error_type)
    }
}
