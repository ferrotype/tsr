//! CommonJS assignments share alias targets with explicit exports; arbitrary
//! expressions yield no alias rather than inventing a named declaration.
use crate::{CheckerState, Error};
use ts_arena::{NodeId, SymbolId};
use ts_ast::{internal_symbol_names as names, symbol_flags as sf, SyntaxKind as K};
use ts_core::ModuleKind;
impl CheckerState {
    // port: tsc/internal/checker/checker.go:Checker.isCommonJSRequire
    pub(crate) fn is_common_js_require(&mut self, node: NodeId) -> Result<bool, Error> {
        if !ts_ast::utilities_middle::is_require_call(
            self.ast(node)?,
            &self.ast(node)?.node(node)?,
            true,
        )? {
            return Ok(false);
        }
        let expression = self
            .ast(node)?
            .node(node)?
            .expression()
            .ok_or(Error::MissingLink("require expression"))?;
        let text = self
            .ast(expression)?
            .node_text(expression)?
            .into_js_string();
        let Some(symbol) =
            self.resolve_name(Some(expression), text.as_bytes(), sf::VALUE, None, true)?
        else {
            return Ok(false);
        };
        if symbol == self.builtins.require_symbol {
            return Ok(true);
        }
        let flags = self.symbol(symbol)?.flags();
        if flags & sf::ALIAS != 0 {
            return Ok(false);
        }
        let kind = if flags & sf::FUNCTION != 0 {
            K::FunctionDeclaration
        } else if flags & sf::VARIABLE != 0 {
            K::VariableDeclaration
        } else {
            return Ok(false);
        };
        for declaration in self.symbol_declarations(symbol)?.iter().flatten() {
            let read = self.ast(declaration)?.node(declaration)?;
            if read.kind() == kind {
                return Ok(read.flags() & ts_ast::node_flags::AMBIENT != 0);
            }
        }
        Ok(false)
    }
    // port: tsc/internal/checker/checker.go:Checker.resolveQualifiedName
    pub(crate) fn resolve_common_js_namespace(
        &mut self,
        namespace: SymbolId,
    ) -> Result<SymbolId, Error> {
        let Some(declaration) = self.symbol(namespace)?.value_declaration() else {
            return Ok(namespace);
        };
        let read = self.ast(declaration)?.node(declaration)?;
        if read.flags() & ts_ast::node_flags::JAVA_SCRIPT_FILE == 0
            || read.kind() != K::VariableDeclaration
            || self.program()?.host.options().module_resolution_kind()
                == ts_core::ModuleResolutionKind::BUNDLER
        {
            return Ok(namespace);
        }
        if let Some(initializer) = read.initializer() {
            if self.is_common_js_require(initializer)? {
                let arguments = self.source_list(
                    initializer,
                    self.ast(initializer)?.node(initializer)?.argument_list(),
                )?;
                let name = *arguments
                    .first()
                    .ok_or(Error::MissingLink("require module argument"))?;
                let module = self.resolve_external_module_name(name, name, false)?;
                if let Some(module) = self.resolve_external_module_symbol(module, false)? {
                    return Ok(module);
                }
            }
        }
        Ok(namespace)
    }
    // port: tsc/internal/checker/checker.go:Checker.getTargetOfAliasLikeExpression
    pub(crate) fn target_of_alias_like_expression(
        &mut self,
        expression: NodeId,
    ) -> Result<Option<SymbolId>, Error> {
        let read = self.ast(expression)?.node(expression)?;
        if read.kind() == K::ClassExpression {
            let ty = self.check_expression_cached(expression)?;
            return Ok(self.types.get(ty)?.symbol);
        }
        if !ts_ast::utilities::is_entity_name(&read)
            && !ts_ast::is_entity_name_expression(self.ast(expression)?, expression)?
        {
            return Ok(None);
        }
        if let Some(symbol) = self.resolve_entity_name_ex(
            expression,
            sf::VALUE | sf::TYPE | sf::NAMESPACE,
            true,
            true,
        )? {
            return Ok(Some(symbol));
        }
        self.check_expression_cached(expression)?;
        Ok(self
            .query
            .resolved_symbols
            .try_get(expression)
            .copied()
            .flatten())
    }
    // port: tsc/internal/checker/checker.go:Checker.getTargetOfAccessExpression
    pub(crate) fn target_of_alias_access(
        &mut self,
        node: NodeId,
    ) -> Result<Option<SymbolId>, Error> {
        let Some(parent) = self.ast(node)?.node(node)?.parent() else {
            return Ok(None);
        };
        let read = self.ast(parent)?.node(parent)?;
        if let Some(binary) = read.data_source().as_binary_expression() {
            if binary.left() == Some(node)
                && self
                    .ast(parent)?
                    .node(
                        binary
                            .operator_token()
                            .ok_or(Error::MissingLink("alias assignment operator"))?,
                    )?
                    .kind()
                    == K::EqualsToken
            {
                let right = binary
                    .right()
                    .ok_or(Error::MissingLink("alias assignment right"))?;
                return self.target_of_alias_like_expression(right);
            }
        }
        Ok(None)
    }
    // port: tsc/internal/checker/checker.go:Checker.getTargetOfNamespaceExportDeclaration
    pub(crate) fn target_of_namespace_export_declaration(
        &mut self,
        node: NodeId,
    ) -> Result<Option<SymbolId>, Error> {
        let parent = self
            .ast(node)?
            .node(node)?
            .parent()
            .ok_or(Error::MissingLink("namespace export parent"))?;
        if !ts_ast::utilities::can_have_symbol(&self.ast(parent)?.node(parent)?) {
            return Ok(None);
        }
        let raw = self
            .program()?
            .bound(parent)?
            .node_binding(parent)?
            .and_then(|binding| binding.symbol);
        let result = self.resolve_external_module_symbol(raw, true)?;
        self.mark_module_alias_type_only(node, None)?;
        Ok(result)
    }
    // port: tsc/internal/checker/checker.go:Checker.getTargetOfImportEqualsDeclaration
    pub(crate) fn target_of_require_variable(
        &mut self,
        node: NodeId,
    ) -> Result<Option<SymbolId>, Error> {
        let initializer = self
            .ast(node)?
            .node(node)?
            .initializer()
            .ok_or(Error::MissingLink("require declaration initializer"))?;
        if !ts_ast::utilities_middle::is_require_call(
            self.ast(initializer)?,
            &self.ast(initializer)?.node(initializer)?,
            true,
        )? {
            return Err(Error::Unsupported(
                "getTargetOfImportEqualsDeclaration: variable not initialized to require",
            ));
        }
        let arguments = self.source_list(
            initializer,
            self.ast(initializer)?.node(initializer)?.argument_list(),
        )?;
        let specifier = *arguments
            .first()
            .ok_or(Error::MissingLink("require module argument"))?;
        let immediate = self.resolve_external_module_name(node, specifier, false)?;
        let resolved = self.resolve_external_module_symbol(immediate, true)?;
        if let Some(resolved) = resolved {
            if (ModuleKind::NODE20..=ModuleKind::NODE_NEXT)
                .contains(&self.program()?.host.options().emit_module_kind())
            {
                if let Some(export) =
                    self.module_export_member(resolved, names::MODULE_EXPORTS, node, true)?
                {
                    return Ok(Some(export));
                }
            }
        }
        self.mark_module_alias_type_only(node, None)?;
        Ok(resolved)
    }
}
