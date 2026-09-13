//! Symbol queries share ordinary expression resolution, including cached access symbols.
use crate::{CheckerState, Error, TypeId};
use ts_arena::{NodeId, SymbolId};
use ts_ast::{check_flags as cf, symbol_flags as sf, JsString, SyntaxKind as K};
fn required<T>(value: Option<T>, name: &'static str) -> Result<T, Error> {
    value.ok_or(Error::MissingLink(name))
}
impl CheckerState {
    // port: tsc/internal/checker/checker.go:Checker.getSymbolOfNameOrPropertyAccessExpression
    pub(crate) fn symbol_of_expression_name(
        &mut self,
        mut name: NodeId,
    ) -> Result<Option<SymbolId>, Error> {
        let read = self.ast(name)?.node(name)?;
        let parent = required(read.parent(), "name query parent")?;
        if self.ast(parent)?.node(parent)?.name() == Some(name)
            && ts_ast::is_declaration(&self.ast(parent)?.node(parent)?)
        {
            return self.get_symbol_of_declaration(parent);
        }
        if self.ast(parent)?.node(parent)?.kind() == K::ExportAssignment
            && ts_ast::is_entity_name_expression(self.ast(name)?, name)?
        {
            if let Some(symbol) = self.resolve_entity_name(
                name,
                sf::VALUE | sf::TYPE | sf::NAMESPACE | sf::ALIAS,
                true,
            )? {
                if symbol != self.builtins.unknown_symbol {
                    return Ok(Some(symbol));
                }
            }
        } else {
            let mut top = name;
            while let Some(parent) = self.ast(top)?.node(top)?.parent() {
                if self.ast(parent)?.node(parent)?.kind() != K::QualifiedName {
                    break;
                }
                top = parent;
            }
            if let Some(import) = self.ast(top)?.node(top)?.parent() {
                if self.ast(import)?.node(import)?.kind() == K::ImportEqualsDeclaration {
                    if self.ast(name)?.node(name)?.kind()==K::Identifier && ts_ast::utilities_middle::is_right_side_of_qualified_name_or_property_access(self.ast(name)?,name)? {name=required(self.ast(name)?.node(name)?.parent(),"import name parent")?;}
                    let parent =
                        required(self.ast(name)?.node(name)?.parent(), "import name parent")?;
                    let meaning = if self.ast(name)?.node(name)?.kind() == K::Identifier
                        || self.ast(parent)?.node(parent)?.kind() == K::QualifiedName
                    {
                        sf::NAMESPACE
                    } else {
                        sf::VALUE | sf::TYPE | sf::NAMESPACE
                    };
                    return self.resolve_entity_name_ex(name, meaning, false, true);
                }
                if self.ast(import)?.node(import)?.kind() == K::ImportType {
                    self.get_type_from_type_node(import)?;
                    return Ok(self
                        .query
                        .resolved_symbols
                        .try_get(name)
                        .copied()
                        .flatten()
                        .filter(|symbol| *symbol != self.builtins.unknown_symbol));
                }
            }
        }
        while ts_ast::utilities_middle::is_right_side_of_qualified_name_or_property_access(
            self.ast(name)?,
            name,
        )? {
            name = required(self.ast(name)?.node(name)?.parent(), "right name parent")?;
        }
        let mut top = name;
        while let Some(parent) = self.ast(top)?.node(top)?.parent() {
            if !matches!(
                self.ast(parent)?.node(parent)?.kind().known(),
                Some(K::QualifiedName | K::PropertyAccessExpression)
            ) {
                break;
            }
            top = parent;
        }
        if let Some(parent) = self.ast(top)?.node(top)?.parent() {
            let parent_read = self.ast(parent)?.node(parent)?;
            let heritage = parent_read
                .parent()
                .map(|ancestor| {
                    self.ast(ancestor)?
                        .node(ancestor)
                        .map(|n| n.kind() == K::HeritageClause)
                        .map_err(Error::from)
                })
                .transpose()?
                .unwrap_or(false);
            if parent_read.kind() == K::ExpressionWithTypeArguments
                || parent_read.kind() == K::TypeReference && heritage
            {
                let own_parent =
                    required(self.ast(name)?.node(name)?.parent(), "heritage name parent")?;
                let direct = matches!(
                    self.ast(own_parent)?.node(own_parent)?.kind().known(),
                    Some(K::ExpressionWithTypeArguments | K::TypeReference)
                );
                let mut meaning = if direct {
                    if self.is_part_of_type_node(name)? {
                        sf::TYPE
                    } else {
                        sf::VALUE
                    }
                } else {
                    sf::NAMESPACE
                };
                if direct
                    && self.ast(own_parent)?.node(own_parent)?.kind()
                        == K::ExpressionWithTypeArguments
                    && !self.is_type_heritage_expression(own_parent)?
                {
                    meaning |= sf::VALUE;
                }
                if ts_ast::is_entity_name_expression(self.ast(name)?, name)? {
                    if let Some(symbol) =
                        self.resolve_entity_name(name, meaning | sf::ALIAS, true)?
                    {
                        return Ok(Some(symbol));
                    }
                }
            }
        }
        if self.expression_node(name)? {
            let read = self.ast(name)?.node(name)?;
            if ts_ast::node_is_missing(Some(&read)) {
                return Ok(None);
            }
            let jsdoc =
                ts_ast::utilities_tail::is_js_doc_name_reference_context(self.ast(name)?, name)?;
            match read.kind().known() {
                Some(K::Identifier) => {
                    if jsdoc {
                        self.jsdoc_identifier_symbol(name)
                    } else {
                        self.resolve_entity_name_ex(name, sf::VALUE, true, true)
                    }
                }
                Some(K::PrivateIdentifier) => self.private_identifier_expression_symbol(name),
                Some(K::PropertyAccessExpression | K::QualifiedName) => {
                    if let Some(symbol) =
                        self.query.resolved_symbols.try_get(name).copied().flatten()
                    {
                        return Ok(Some(symbol));
                    }
                    let property = read.kind() == K::PropertyAccessExpression;
                    self.check_property_access(name)?;
                    if self
                        .query
                        .resolved_symbols
                        .try_get(name)
                        .copied()
                        .flatten()
                        .is_none()
                        && property
                    {
                        let read = self.ast(name)?.node(name)?;
                        let key_node = required(read.name(), "queried property name")?;
                        let receiver = required(read.expression(), "queried property receiver")?;
                        if self.ast(key_node)?.node(key_node)?.kind() != K::PrivateIdentifier {
                            let ty = self.check_expression_cached(receiver)?;
                            let key = self.literal_type_from_property_name(key_node)?;
                            let symbol = self.applicable_index_symbol(ty, key)?;
                            *self.query.resolved_symbols.get_or_default(name) = symbol;
                        }
                    }
                    let symbol = self.query.resolved_symbols.try_get(name).copied().flatten();
                    if symbol.is_none() && jsdoc && !property {
                        return self.resolve_jsdoc_member_name(Some(name));
                    }
                    Ok(symbol)
                }
                _ => Ok(None),
            }
        } else {
            let mut top = name;
            while let Some(parent) = self.ast(top)?.node(top)?.parent() {
                if self.ast(parent)?.node(parent)?.kind() != K::QualifiedName {
                    break;
                }
                top = parent;
            }
            let parent = required(self.ast(name)?.node(name)?.parent(), "type name parent")?;
            let top_parent = required(self.ast(top)?.node(top)?.parent(), "type name top parent")?;
            if self.ast(top_parent)?.node(top_parent)?.kind() == K::TypeReference {
                let meaning = if self.ast(parent)?.node(parent)?.kind() == K::TypeReference {
                    sf::TYPE
                } else {
                    sf::NAMESPACE
                };
                if let Some(symbol) = self.resolve_entity_name_ex(name, meaning, true, true)? {
                    if symbol != self.builtins.unknown_symbol {
                        return Ok(Some(symbol));
                    }
                }
                if self
                    .ast(top_parent)?
                    .node(top_parent)?
                    .parent()
                    .map(|p| {
                        self.ast(p)?
                            .node(p)
                            .map(|n| n.kind() == K::HeritageClause)
                            .map_err(Error::from)
                    })
                    .transpose()?
                    .unwrap_or(false)
                {
                    return Ok(None);
                }
                return self.unresolved_symbol_for_name(name).map(Some);
            }
            if self.ast(parent)?.node(parent)?.kind() == K::TypePredicate {
                return self.resolve_entity_name(name, sf::FUNCTION_SCOPED_VARIABLE, true);
            }
            Ok(None)
        }
    }
    // port: tsc/internal/checker/checker.go:Checker.getApplicableIndexSymbol
    fn applicable_index_symbol(
        &mut self,
        ty: TypeId,
        key: TypeId,
    ) -> Result<Option<SymbolId>, Error> {
        let Some(info) = self.applicable_index_info(ty, key)? else {
            return Ok(None);
        };
        if info == self.builtins.any_base_type_index_info {
            return Ok(None);
        }
        let data = self.signatures.index_info(info)?.clone();
        if data.index_symbol.is_some() {
            return Ok(data.index_symbol);
        }
        let mut declarations = Vec::new();
        if let Some(declaration) = data.declaration {
            declarations.push(Some(declaration));
        } else {
            for other in self.index_infos_of_type(ty)? {
                let other = self.signatures.index_info(other)?.clone();
                if let Some(declaration) = other.declaration {
                    if self.applicable_index_type(key, other.key_type)? {
                        declarations.push(Some(declaration));
                    }
                }
            }
        }
        if declarations.is_empty() {
            return Ok(None);
        }
        let symbol = self.new_symbol_ex(
            sf::PROPERTY,
            JsString::from_bytes(ts_ast::internal_symbol_names::INDEX),
            cf::INDEX_SYMBOL,
        )?;
        let value_declaration = declarations[0];
        let declarations = self.declarations.alloc(declarations)?;
        let parent = self.types.get(ty)?.symbol;
        let data_symbol = self.symbol_mut(symbol)?;
        data_symbol.declarations = declarations;
        data_symbol.value_declaration = value_declaration;
        data_symbol.parent = parent;
        self.value_symbol_links.get_or_default(symbol).resolved_type = Some(data.value_type);
        self.signatures.index_info_mut(info)?.index_symbol = Some(symbol);
        Ok(Some(symbol))
    }
    // port: tsc/internal/checker/checker.go:Checker.getUnresolvedSymbolForEntityName
    pub(crate) fn unresolved_symbol_for_name(&mut self, name: NodeId) -> Result<SymbolId, Error> {
        let read = self.ast(name)?.node(name)?;
        let (identifier, left) = match read.kind().known() {
            Some(K::QualifiedName) => {
                let data = read
                    .data_source()
                    .as_qualified_name()
                    .ok_or(ts_arena::Error::InvalidGraph)?;
                (required(data.right(), "unresolved right")?, data.left())
            }
            Some(K::PropertyAccessExpression) => {
                (required(read.name(), "unresolved name")?, read.expression())
            }
            _ => (name, None),
        };
        let text = self
            .ast(identifier)?
            .node_text(identifier)?
            .into_js_string();
        if text.is_empty() {
            return Ok(self.builtins.unknown_symbol);
        }
        let parent = left
            .map(|left| self.unresolved_symbol_for_name(left))
            .transpose()?;
        let mut path = Vec::new();
        let mut names = Vec::new();
        let mut current = parent;
        while let Some(symbol) = current {
            let symbol = self.symbol(symbol)?;
            names.push(symbol.name_to_owned());
            current = symbol.parent();
        }
        for name in names.into_iter().rev() {
            path.extend_from_slice(name.as_bytes());
            path.push(b'.');
        }
        path.extend_from_slice(text.as_bytes());
        if let Some(&symbol) = self.query.unresolved_symbols.get(path.as_slice()) {
            return Ok(symbol);
        }
        let symbol = self.new_symbol_ex(sf::TYPE_ALIAS, text, cf::UNRESOLVED)?;
        self.symbol_mut(symbol)?.parent = parent;
        *self.query.declared_types.get_or_default(symbol) = Some(self.builtins.unresolved_type);
        self.query
            .unresolved_symbols
            .insert(JsString::from_bytes(path), symbol);
        Ok(symbol)
    }
}
