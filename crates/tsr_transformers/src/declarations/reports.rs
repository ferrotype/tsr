use super::{
    diagnostics::{diagnostic_for_node, isolated_declaration_error},
    transform::Transformer,
};
use ts_ast::{NodeId, SymbolId, SyntaxKind as K};
use ts_printer::emit_resolver::DeclarationEmitResolver;
impl<R: DeclarationEmitResolver> Transformer<'_, R> {
    fn source_of(&self, node: NodeId) -> Result<NodeId, R::Error> {
        Ok(
            ts_ast::utilities::get_source_file_of_node(self.resolver.ast(node)?, Some(node))?
                .ok_or(ts_arena::Error::InvalidGraph)?,
        )
    }
    // port: tsc/internal/transformers/declarations/tracker.go:SymbolTrackerImpl.ReportInferenceFallback
    pub fn inference_fallback(&mut self, node: NodeId) -> Result<(), R::Error> {
        if !self.options.isolated_declarations || self.source_of(node)? != self.source {
            return Ok(());
        }
        if self.resolver.expando_function_declaration_unsafe(node)? {
            self.expando_errors(node)?;
        }
        let mut ancestor = Some(node);
        while let Some(current) = ancestor {
            if matches!(
                self.node(current).kind().known(),
                Some(K::SourceFile | K::Block)
            ) {
                break;
            }
            if self.bound_expando(current)? {
                return Ok(());
            }
            ancestor = self.node(current).parent();
        }
        let diagnostic = isolated_declaration_error(self.resolver, node)?;
        self.diagnostics.push(diagnostic);
        Ok(())
    }
    // port: tsc/internal/transformers/declarations/tracker.go:SymbolTrackerImpl.isBoundExpando
    fn bound_expando(&mut self, node: NodeId) -> Result<bool, R::Error> {
        if self.node(node).kind() != K::BinaryExpression
            || !ts_ast::utilities_tail::is_expando_property_declaration(Some(&self.node(node)))
        {
            return Ok(false);
        }
        let left = Self::required(self.node(node).as_binary_expression().unwrap().left())?;
        if self.node(left).kind() != K::PropertyAccessExpression {
            return Ok(false);
        }
        let leftmost = super::util::leftmost_expression(self.output.view(), left, true)?;
        let Some(declaration) = self
            .resolver
            .referenced_value_declaration_unsafe(leftmost)?
        else {
            return Ok(false);
        };
        self.resolver
            .expando_function_declaration_unsafe(declaration)
    }
    pub fn expando_errors(&mut self, node: NodeId) -> Result<(), R::Error> {
        if !self.options.isolated_declarations {
            return Ok(());
        }
        for property in self.resolver.properties_of_container_function(node)? {
            if let Some(declaration) = self.resolver.symbol_value_declaration(property)? {
                let view = self.resolver.ast(declaration)?;
                if ts_ast::utilities_tail::is_expando_property_declaration(Some(
                    &view.node(declaration)?,
                )) {
                    let target = if view.node(declaration)?.kind() == K::BinaryExpression {
                        view.node(declaration)?
                            .as_binary_expression()
                            .unwrap()
                            .left()
                            .ok_or(ts_arena::Error::InvalidGraph)?
                    } else {
                        declaration
                    };
                    self.diagnostic(target, ts_diagnostics::Assigning_properties_to_functions_without_declaring_them_is_not_supported_with_isolatedDeclarations_Add_an_explicit_declaration_for_the_properties_assigned_to_this_function, vec![])?;
                }
            }
        }
        Ok(())
    }
    // port: tsc/internal/transformers/declarations/tracker.go:SymbolTrackerImpl.ReportNonlocalAugmentation
    pub fn nonlocal_augmentation(
        &mut self,
        containing: NodeId,
        parent: SymbolId,
        augmenting: SymbolId,
    ) -> Result<(), R::Error> {
        let mut primary = None;
        for declaration in self.resolver.symbol_declarations(parent)? {
            if self.source_of(declaration)? == containing {
                primary = Some(declaration);
                break;
            }
        }
        if let Some(primary) = primary {
            for declaration in self.resolver.symbol_declarations(augmenting)? {
                if self.source_of(declaration)? == containing {
                    continue;
                }
                let mut diagnostic = diagnostic_for_node(self.resolver.ast(declaration)?, Some(declaration), ts_diagnostics::Declaration_augments_declaration_in_another_file_This_cannot_be_serialized, vec![])?;
                let related = diagnostic_for_node(self.resolver.ast(primary)?, Some(primary), ts_diagnostics::This_is_the_declaration_being_augmented_Consider_moving_the_augmenting_declaration_into_the_same_file, vec![])?;
                diagnostic
                    .related_information
                    .push(std::sync::Arc::new(related));
                self.diagnostics.push(diagnostic);
            }
        }
        Ok(())
    }
}
