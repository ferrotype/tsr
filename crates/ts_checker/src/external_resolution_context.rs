//! Resolve a generated display specifier using the original declaration's
//! location and attributes, without creating a synthetic import or diagnostics.
use super::{is_import_call, ExternalModuleReference};
use crate::{CheckerState, Error, TypeId};
use ts_ast::{JsString, NodeId, SymbolId, SyntaxKind as K};

impl CheckerState {
    // port: tsc/internal/checker/checker.go:Checker.resolveExternalModule
    pub(crate) fn resolve_module_for_display(
        &mut self,
        location: NodeId,
        module_reference: JsString,
        attributes: TypeId,
    ) -> Result<Option<SymbolId>, Error> {
        let context = self.display_resolution_context(location)?;
        let (_, file) = self.module_source(location)?;
        let host = &self.program()?.host;
        let mode = match context {
            Some(node)
                if matches!(
                    self.ast(node)?.node(node)?.kind().known(),
                    Some(K::StringLiteral | K::NoSubstitutionTemplateLiteral)
                ) =>
            {
                host.get_mode_for_usage_location(file.as_bytes(), node)?
            }
            _ => host.get_default_resolution_mode_for_file(file.as_bytes())?,
        };
        self.resolve_external_module_reference(ExternalModuleReference {
            location,
            module_reference,
            error_node: None,
            message: None,
            augmentation: false,
            mode,
            attributes: Some(attributes),
        })
    }

    fn display_resolution_context(&self, location: NodeId) -> Result<Option<NodeId>, Error> {
        let view = self.ast(location)?;
        let node = view.node(location)?;
        if matches!(
            node.kind().known(),
            Some(K::StringLiteral | K::NoSubstitutionTemplateLiteral)
        ) || node
            .parent()
            .map(|parent| {
                let parent = view.node(parent)?;
                Ok::<_, Error>(
                    parent.kind() == K::ModuleDeclaration && parent.name() == Some(location),
                )
            })
            .transpose()?
            .unwrap_or(false)
        {
            return Ok(Some(location));
        }
        if node.kind() == K::ModuleDeclaration {
            return Ok(node.name());
        }
        if let Some(data) = node.data_source().as_import_type_node() {
            if let Some(argument) = data.argument() {
                if let Some(literal) = view.node(argument)?.data_source().as_literal_type_node() {
                    return Ok(literal.literal());
                }
            }
        }
        if ts_ast::is_variable_declaration_initialized_to_bare_or_accessed_require(view, location)?
        {
            let mut current = node.initializer();
            while let Some(id) = current {
                let read = view.node(id)?;
                if ts_ast::utilities_middle::is_require_call(view, &read, true)? {
                    return Ok(self.source_list(id, read.argument_list())?.first().copied());
                }
                current = read.expression();
            }
        }
        // Native FindAncestor searches these families in priority order, not
        // just whichever enclosing import/export happens to be nearest.
        for family in 0..4 {
            let mut current = Some(location);
            while let Some(id) = current {
                let read = view.node(id)?;
                let matches = match family {
                    0 => is_import_call(view, &read)?,
                    1 => matches!(
                        read.kind().known(),
                        Some(K::ImportDeclaration | K::JSImportDeclaration)
                    ),
                    2 => read.kind() == K::ExportDeclaration,
                    _ => read.kind() == K::ImportEqualsDeclaration,
                };
                if matches {
                    return match family {
                        0 => Ok(self.source_list(id, read.argument_list())?.first().copied()),
                        3 => self.module_specifier(id),
                        _ => Ok(read.module_specifier()),
                    };
                }
                current = read.parent();
            }
        }
        Ok(None)
    }
}
