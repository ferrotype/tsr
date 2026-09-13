//! Literal import types use the same retained module resolution as source
//! imports. Qualifier links preserve each immediate alias identity.
use crate::{CheckerState, Error, TypeId};
use ts_arena::{NodeId, SymbolId};
use ts_ast::{symbol_flags as sf, SyntaxKind as K};
use ts_diagnostics as d;
impl CheckerState {
    // port: tsc/internal/checker/checker.go:Checker.getTypeFromImportTypeNode
    pub(crate) fn type_from_import_node(&mut self, node: NodeId) -> Result<TypeId, Error> {
        if let Some(Some(ty)) = self.query.type_nodes.try_get(node) {
            return Ok(*ty);
        }
        let read = self.ast(node)?.node(node)?;
        let data = read
            .data_source()
            .as_import_type_node()
            .ok_or(ts_arena::Error::InvalidGraph)?;
        let argument = data
            .argument()
            .ok_or(Error::MissingLink("import type argument"))?;
        let qualifier = data.qualifier();
        let type_of = data.is_type_of();
        let argument_read = self.ast(argument)?.node(argument)?;
        let literal = argument_read
            .data_source()
            .as_literal_type_node()
            .and_then(|d| d.literal());
        let literal = match literal {
            Some(literal) if self.ast(literal)?.node(literal)?.kind() == K::StringLiteral => {
                literal
            }
            _ => {
                self.error_at(Some(argument), d::String_literal_expected, vec![])?;
                *self.query.resolved_symbols.get_or_default(node) =
                    Some(self.builtins.unknown_symbol);
                *self.query.type_nodes.get_or_default(node) = Some(self.builtins.error_type);
                return Ok(self.builtins.error_type);
            }
        };
        let meaning = if type_of { sf::VALUE } else { sf::TYPE };
        let module = self.resolve_external_module_name(node, literal, false)?;
        let Some(inner) = module else {
            *self.query.resolved_symbols.get_or_default(node) = Some(self.builtins.unknown_symbol);
            *self.query.type_nodes.get_or_default(node) = Some(self.builtins.error_type);
            return Ok(self.builtins.error_type);
        };
        let module = self
            .resolve_external_module_symbol(Some(inner), false)?
            .ok_or(Error::MissingLink("import type module"))?;
        let qualifier = match qualifier {
            Some(qualifier)
                if self.ast(qualifier)?.node(qualifier)?.pos()
                    != self.ast(qualifier)?.node(qualifier)?.end() =>
            {
                Some(qualifier)
            }
            _ => None,
        };
        let ty = if let Some(qualifier) = qualifier {
            let mut chain = Vec::new();
            let mut part = qualifier;
            loop {
                let read = self.ast(part)?.node(part)?;
                if read.kind() == K::Identifier {
                    chain.push(part);
                    break;
                }
                let qualified = read
                    .data_source()
                    .as_qualified_name()
                    .ok_or(Error::MissingLink("import qualifier"))?;
                chain.push(
                    qualified
                        .right()
                        .ok_or(Error::MissingLink("import qualifier right"))?,
                );
                part = qualified
                    .left()
                    .ok_or(Error::MissingLink("import qualifier left"))?;
            }
            chain.reverse();
            let mut namespace = module;
            for (index, current) in chain.iter().copied().enumerate() {
                let lookup_meaning = if index + 1 == chain.len() {
                    meaning
                } else {
                    sf::NAMESPACE
                };
                let text = self.ast(current)?.node_text(current)?.into_js_string();
                let resolved = self
                    .resolve_module_symbol(Some(namespace), false)?
                    .ok_or(Error::MissingLink("import qualifier namespace"))?;
                let resolved = self.get_merged_symbol(resolved);
                let next = if type_of {
                    let ty = self.get_type_of_symbol(resolved)?;
                    self.constituent_property_ex(ty, text.as_bytes(), false, true)?
                } else {
                    let exports = self.module_exports_of_symbol(resolved)?;
                    self.lookup_symbol_resolving(exports, text.as_bytes(), lookup_meaning)?
                };
                let Some(next) = next else {
                    if !type_of {
                        let immediate = self
                            .resolve_external_module_symbol(Some(inner), true)?
                            .ok_or(Error::MissingLink("immediate import namespace"))?;
                        for declaration in self.symbol_declarations(immediate)?.iter().flatten() {
                            if self.ast(declaration)?.node(declaration)?.flags()
                                & ts_ast::node_flags::JAVA_SCRIPT_FILE
                                != 0
                            {
                                return Err(Error::Unsupported(
                                    "getTypeFromImportTypeNode: CommonJS typedef export lookup",
                                ));
                            }
                        }
                    }
                    let name = self.fully_qualified_name(namespace, None)?;
                    let member =
                        ts_scanner::declaration_name_to_string(self.ast(current)?, Some(current))?;
                    self.error_at(
                        Some(current),
                        d::Namespace_0_has_no_exported_member_1,
                        vec![name, member],
                    )?;
                    *self.query.type_nodes.get_or_default(node) = Some(self.builtins.error_type);
                    return Ok(self.builtins.error_type);
                };
                *self.query.resolved_symbols.get_or_default(current) = Some(next);
                let parent = self
                    .ast(current)?
                    .node(current)?
                    .parent()
                    .ok_or(Error::MissingLink("import qualifier parent"))?;
                *self.query.resolved_symbols.get_or_default(parent) = Some(next);
                namespace = next;
            }
            self.resolve_import_symbol_type(node, namespace, meaning)?
        } else if self.module_symbol_flags(module, false, false)? & meaning != 0 {
            self.resolve_import_symbol_type(node, module, meaning)?
        } else {
            let text = self.ast(literal)?.node_text(literal)?.into_js_string();
            self.error_at(Some(node),if type_of{d::Module_0_does_not_refer_to_a_value_but_is_used_as_a_value_here}else{d::Module_0_does_not_refer_to_a_type_but_is_used_as_a_type_here_Did_you_mean_typeof_import_0},vec![text])?;
            *self.query.resolved_symbols.get_or_default(node) = Some(self.builtins.unknown_symbol);
            self.builtins.error_type
        };
        *self.query.type_nodes.get_or_default(node) = Some(ty);
        Ok(ty)
    }
    // port: tsc/internal/checker/checker.go:Checker.resolveImportSymbolType
    fn resolve_import_symbol_type(
        &mut self,
        node: NodeId,
        symbol: SymbolId,
        meaning: u32,
    ) -> Result<TypeId, Error> {
        let resolved = self
            .resolve_module_symbol(Some(symbol), false)?
            .ok_or(Error::MissingLink("import symbol"))?;
        *self.query.resolved_symbols.get_or_default(node) = Some(resolved);
        if meaning == sf::VALUE {
            let ty = self.get_type_of_symbol(symbol)?;
            self.instantiation_expression_type(ty, node)
        } else {
            self.type_reference_from_symbol(node, resolved)
        }
    }
    // port: tsc/internal/checker/checker.go:Checker.checkImportType
    pub(crate) fn check_import_type_node(&mut self, node: NodeId) -> Result<(), Error> {
        let read = self.ast(node)?.node(node)?;
        let data = read
            .data_source()
            .as_import_type_node()
            .ok_or(ts_arena::Error::InvalidGraph)?;
        let argument = data
            .argument()
            .ok_or(Error::MissingLink("import type argument"))?;
        let attributes = data.attributes();
        self.check_source_element(argument)?;
        if let Some(attributes) = attributes {
            self.check_grammar_import_attribute_values(attributes)?;
            self.import_resolution_mode_override(attributes, true)?;
        }
        let ty = self.get_type_from_type_node(node)?;
        if ty != self.builtins.error_type
            && !self
                .source_list(node, self.ast(node)?.node(node)?.type_argument_list())?
                .is_empty()
        {
            if let Some(symbol) = self.query.resolved_symbols.try_get(node).copied().flatten() {
                let parameters = self.get_local_type_parameters(symbol)?;
                if !parameters.is_empty() {
                    self.check_type_argument_constraints(node, &parameters)?;
                }
            }
        }
        self.check_import_attributes(node)
    }
}
