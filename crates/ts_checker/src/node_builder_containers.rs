//! Container alternatives are evaluated in native order: lexical containers,
//! export-equals aliases, visible values with the container's type, and reexports.
use super::{left_meaning, NameQuery};
use crate::node_builder::NodeBuilder;
use crate::{type_flags as tf, Error};
use ts_arena::{NodeId, SymbolId};
use ts_ast::{symbol_flags as sf, SyntaxKind as K};

impl NodeBuilder<'_> {
    // port: tsc/internal/checker/symbolaccessibility.go:Checker.getAliasForSymbolInContainer
    pub(super) fn name_alias_in_container(
        &mut self,
        container: SymbolId,
        symbol: SymbolId,
    ) -> Result<Option<SymbolId>, Error> {
        if Some(container) == self.checker.parent_of_symbol(symbol)? {
            return Ok(Some(symbol));
        }
        if let Some(exports) = self.checker.symbol(container)?.exports() {
            if let Some(export) = self
                .checker
                .table(exports)?
                .get(ts_ast::internal_symbol_names::EXPORT_EQUALS)
                .flatten()
            {
                if self.checker.module_symbols_same_reference(export, symbol)? {
                    return Ok(Some(container));
                }
            }
        }
        let Some(exports) = self.checker.module_exports_of_symbol(container)? else {
            return Ok(None);
        };
        let name = self.checker.symbol(symbol)?.name_to_owned();
        if let Some(quick) = self.checker.table(exports)?.get(name.as_bytes()).flatten() {
            if self.checker.module_symbols_same_reference(quick, symbol)? {
                return Ok(Some(quick));
            }
        }
        let symbols: Vec<_> = self
            .checker
            .table(exports)?
            .iter()
            .filter_map(|(_, s)| s)
            .collect();
        let mut candidates = vec![];
        for candidate in symbols {
            if self
                .checker
                .module_symbols_same_reference(candidate, symbol)?
            {
                candidates.push(candidate);
            }
        }
        self.checker.sort_symbols(&mut candidates)?;
        Ok(candidates.first().copied())
    }

    pub(super) fn external_name_container(
        &mut self,
        declaration: NodeId,
    ) -> Result<Option<SymbolId>, Error> {
        let mut node = Some(declaration);
        while let Some(current) = node {
            let read = self.checker.node(current)?;
            if ts_ast::is_ambient_module(self.checker.ast(current)?, current)?
                || read.kind() == K::SourceFile
                    && ts_ast::utilities::is_external_or_common_js_module(
                        &self.checker.source_file_read(current)?,
                    )
            {
                return self.checker.get_symbol_of_declaration(current);
            }
            node = read.parent();
        }
        Ok(None)
    }

    // port: tsc/internal/checker/symbolaccessibility.go:Checker.getWithAlternativeContainers
    fn name_container_alternatives(
        &mut self,
        container: SymbolId,
        query: NameQuery,
    ) -> Result<Vec<SymbolId>, Error> {
        let declarations: Vec<_> = self
            .checker
            .symbol_declarations(container)?
            .iter()
            .flatten()
            .collect();
        let mut additional = vec![];
        for declaration in declarations {
            if let Some(file) = self.external_name_container(declaration)? {
                if let Some(exports) = self.checker.symbol(file)?.exports() {
                    if let Some(export) = self
                        .checker
                        .table(exports)?
                        .get(ts_ast::internal_symbol_names::EXPORT_EQUALS)
                        .flatten()
                    {
                        if self
                            .checker
                            .module_symbols_same_reference(export, container)?
                        {
                            additional.push(file);
                        }
                    }
                }
            }
        }
        let reexports = self.alternative_name_modules(query)?;
        let object = self.object_literal_name_container(container, query.meaning)?;
        let meaning = left_meaning(query.meaning);
        let flags = self.checker.symbol(container)?.flags();
        if query.enclosing.is_some()
            && flags & meaning != 0
            && !self
                .accessible_name_chain(NameQuery {
                    symbol: container,
                    meaning: sf::NAMESPACE,
                    external_only: false,
                    ..query
                })?
                .is_empty()
        {
            let mut result = vec![container];
            result.extend(additional);
            result.extend(reexports);
            result.extend(object);
            return Ok(result);
        }
        let mut variables = vec![];
        if query.meaning == sf::VALUE && flags & meaning == 0 && flags & sf::TYPE != 0 {
            let declared = self.checker.get_declared_type_of_symbol(container)?;
            if self.checker.types.flags(declared)? & tf::OBJECT != 0 {
                self.some_name_scope(query.enclosing, |this, table, _| {
                    let mut found = false;
                    for symbol in this.name_table_symbols(&table)? {
                        if this.checker.symbol(symbol)?.flags() & meaning != 0
                            && this.checker.get_type_of_symbol(symbol)?
                                == this.checker.get_declared_type_of_symbol(container)?
                        {
                            variables.push(symbol);
                            found = true;
                        }
                    }
                    Ok(found)
                })?;
                self.checker.sort_symbols(&mut variables)?;
            }
        }
        variables.extend(additional);
        variables.push(container);
        variables.extend(object);
        variables.extend(reexports);
        Ok(variables)
    }

    // port: tsc/internal/checker/symbolaccessibility.go:Checker.getAlternativeContainingModules
    fn alternative_name_modules(&mut self, query: NameQuery) -> Result<Vec<SymbolId>, Error> {
        let Some(enclosing) = query.enclosing else {
            return Ok(vec![]);
        };
        let source = ts_ast::utilities::get_source_file_of_node(
            self.checker.ast(enclosing)?,
            Some(enclosing),
        )?
        .ok_or(Error::MissingLink("name module source"))?;
        if let Some(result) = self
            .name_access
            .extended_by_file
            .get(&(query.symbol, source))
        {
            return Ok(result.clone());
        }
        let imports: Vec<_> = self
            .checker
            .ast(source)?
            .source_file(source)?
            .imports()?
            .iter()
            .flatten()
            .copied()
            .collect();
        let mut result = vec![];
        for import in imports {
            if ts_ast::utilities::node_is_synthesized(&self.checker.node(import)?) {
                continue;
            }
            if let Some(module) = self
                .checker
                .resolve_external_module_name(enclosing, import, true)?
            {
                if self
                    .name_alias_in_container(module, query.symbol)?
                    .is_some()
                {
                    result.push(module);
                }
            }
        }
        if !result.is_empty() {
            self.name_access
                .extended_by_file
                .insert((query.symbol, source), result.clone());
            return Ok(result);
        }
        if let Some(result) = self.name_access.extended.get(&query.symbol) {
            return Ok(result.clone());
        }
        let host = &self.checker.program()?.host;
        let files: Vec<_> = (0..host.source_file_count())
            .map(|i| host.source_file(i).source())
            .collect();
        for file in files {
            if self
                .checker
                .ast(file)?
                .source_file(file)?
                .external_module_indicator
                .is_none()
            {
                continue;
            }
            let symbol = self
                .checker
                .get_symbol_of_declaration(file)?
                .ok_or(Error::MissingLink("external file symbol"))?;
            if self
                .name_alias_in_container(symbol, query.symbol)?
                .is_some()
            {
                result.push(symbol);
            }
        }
        self.name_access
            .extended
            .insert(query.symbol, result.clone());
        Ok(result)
    }

    // port: tsc/internal/checker/symbolaccessibility.go:Checker.getVariableDeclarationOfObjectLiteral
    fn object_literal_name_container(
        &mut self,
        symbol: SymbolId,
        meaning: u32,
    ) -> Result<Option<SymbolId>, Error> {
        if meaning & sf::VALUE == 0 {
            return Ok(None);
        }
        let Some(declaration) = self
            .checker
            .symbol_declarations(symbol)?
            .iter()
            .flatten()
            .next()
        else {
            return Ok(None);
        };
        let read = self.checker.node(declaration)?;
        let kind = read.kind();
        let Some(parent) = read.parent() else {
            return Ok(None);
        };
        let read = self.checker.node(parent)?;
        if read.kind() == K::VariableDeclaration
            && (kind == K::ObjectLiteralExpression && read.initializer() == Some(declaration)
                || kind == K::TypeLiteral && read.type_node() == Some(declaration))
        {
            return self.checker.get_symbol_of_declaration(parent);
        }
        Ok(None)
    }

    // port: tsc/internal/checker/symbolaccessibility.go:Checker.getContainersOfSymbol
    pub(super) fn name_containers(&mut self, query: NameQuery) -> Result<Vec<SymbolId>, Error> {
        if self.checker.symbol(query.symbol)?.flags() & sf::TYPE_PARAMETER == 0 {
            if let Some(parent) = self.checker.parent_of_symbol(query.symbol)? {
                return self.name_container_alternatives(parent, query);
            }
        }
        let declarations: Vec<_> = self
            .checker
            .symbol_declarations(query.symbol)?
            .iter()
            .flatten()
            .collect();
        let mut candidates = vec![];
        for declaration in declarations {
            let read = self.checker.node(declaration)?;
            let kind = read.kind();
            let Some(parent) = read.parent() else {
                continue;
            };
            if !ts_ast::is_ambient_module(self.checker.ast(declaration)?, declaration)? {
                let read = self.checker.node(parent)?;
                let external = read.kind() == K::SourceFile
                    && ts_ast::utilities::is_external_or_common_js_module(
                        &self.checker.source_file_read(parent)?,
                    )
                    || read.kind() == K::ModuleDeclaration
                        && read
                            .name()
                            .map(|name| {
                                self.checker
                                    .ast(name)
                                    .and_then(|view| view.node(name).map_err(Error::from))
                                    .map(|read| read.kind() == K::StringLiteral)
                            })
                            .transpose()?
                            .unwrap_or(false);
                if external {
                    if let Some(symbol) = self.checker.get_symbol_of_declaration(parent)? {
                        if !candidates.contains(&symbol) {
                            candidates.push(symbol);
                        }
                    }
                    continue;
                }
                if read.kind() == K::ModuleBlock {
                    if let Some(parent) = read.parent() {
                        let symbol = self.checker.get_symbol_of_declaration(parent)?;
                        if self.checker.resolve_external_module_symbol(symbol, false)?
                            == Some(query.symbol)
                        {
                            if let Some(symbol) = symbol {
                                if !candidates.contains(&symbol) {
                                    candidates.push(symbol);
                                }
                            }
                            continue;
                        }
                    }
                }
            }
            if kind == K::ClassExpression {
                let view = self.checker.ast(parent)?;
                let assignment = view.node(parent)?;
                let Some(data) = assignment.data_source().as_binary_expression() else {
                    continue;
                };
                let operator = data
                    .operator_token()
                    .ok_or(Error::MissingLink("class assignment operator"))?;
                let left = data
                    .left()
                    .ok_or(Error::MissingLink("class assignment left"))?;
                let left_node = view.node(left)?;
                if view.node(operator)?.kind() != K::EqualsToken
                    || !ts_ast::utilities::is_access_expression(&left_node)
                {
                    continue;
                }
                let target = left_node
                    .expression()
                    .ok_or(Error::MissingLink("class assignment target"))?;
                if !ts_ast::is_entity_name_expression(view, target)? {
                    continue;
                }
                if ts_ast::is_module_exports_access_expression(view, left)?
                    || ts_ast::is_exports_identifier(view, target)?
                {
                    let source =
                        ts_ast::utilities::get_source_file_of_node(view, Some(declaration))?;
                    let Some(source) = source else {
                        continue;
                    };
                    if let Some(symbol) = self.checker.get_symbol_of_declaration(source)? {
                        if !candidates.contains(&symbol) {
                            candidates.push(symbol);
                        }
                    }
                    continue;
                }
                self.checker.check_expression_cached(target)?;
                if let Some(symbol) = *self.checker.query.resolved_symbols.get_or_default(target) {
                    if !candidates.contains(&symbol) {
                        candidates.push(symbol);
                    }
                }
            }
        }
        let mut best = vec![];
        let mut alternatives = vec![];
        for candidate in candidates {
            if self
                .name_alias_in_container(candidate, query.symbol)?
                .is_none()
            {
                continue;
            }
            let containers = self.name_container_alternatives(candidate, query)?;
            if let Some((&first, rest)) = containers.split_first() {
                best.push(first);
                alternatives.extend_from_slice(rest);
            }
        }
        best.extend(alternatives);
        Ok(best)
    }
}
