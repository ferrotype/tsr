//! Qualified-name errors distinguish missing exports from value/type misuse.
use crate::{CheckerState, Error};
use ts_arena::{NodeId, SymbolId};
use ts_ast::{symbol_flags as sf, SyntaxKind as K};
use ts_diagnostics as d;

impl CheckerState {
    // port: tsc/internal/checker/checker.go:Checker.getSuggestedSymbolForNonexistentModule
    pub(crate) fn suggested_module_member(
        &mut self,
        name: NodeId,
        module: SymbolId,
    ) -> Result<Option<SymbolId>, Error> {
        let name = self.ast(name)?.node_text(name)?.into_js_string();
        let table = self.module_exports(module)?;
        let mut candidates = Vec::new();
        for (_, candidate) in self.module_table_entries(table)? {
            let Some(candidate) = candidate else { continue };
            let read = self.symbol(candidate)?;
            let text = read.name_to_owned();
            if text.is_empty() || matches!(text.as_bytes()[0], b'"' | 0xfe) {
                continue;
            }
            let mut flags = read.flags();
            if flags & sf::MODULE_MEMBER == 0 && flags & sf::ALIAS != 0 {
                flags = self.module_symbol_flags(candidate, false, false)?;
            }
            if flags & sf::MODULE_MEMBER != 0 {
                candidates.push((text, candidate));
            }
        }
        let failure = std::cell::Cell::new(None);
        let result = ts_scanner::get_spelling_suggestion(
            name.as_bytes(),
            candidates.iter(),
            |entry| entry.0.as_bytes(),
            |a, b| match self.compare_symbols(Some(a.1), Some(b.1)) {
                Ok(order) => order,
                Err(error) => {
                    failure.set(Some(error));
                    std::cmp::Ordering::Equal
                }
            },
            0,
        )
        .map(|entry| entry.1);
        if let Some(error) = failure.get() {
            return Err(error);
        }
        Ok(result)
    }
    // port: tsc/internal/checker/checker.go:Checker.resolveQualifiedName
    pub(crate) fn report_missing_qualified_name(
        &mut self,
        name: NodeId,
        right: NodeId,
        namespace: SymbolId,
        meaning: u32,
    ) -> Result<(), Error> {
        let namespace_name = self.fully_qualified_name(namespace, None)?;
        let declaration_name =
            ts_scanner::declaration_name_to_string(self.ast(right)?, Some(right))?;
        if let Some(suggestion) = self.suggested_module_member(right, namespace)? {
            let suggestion = self.symbol_to_string(suggestion)?;
            self.error_at(
                Some(right),
                d::X_0_has_no_exported_member_named_1_Did_you_mean_2,
                vec![namespace_name, declaration_name, suggestion],
            )?;
            return Ok(());
        }
        if self.query.global_types.contains_key("Object")
            && meaning & sf::TYPE != 0
            && self.ast(name)?.node(name)?.kind() == K::QualifiedName
        {
            let mut containing = name;
            while let Some(parent) = self.ast(containing)?.node(containing)?.parent() {
                if self.ast(parent)?.node(parent)?.kind() != K::QualifiedName {
                    break;
                }
                containing = parent;
            }
            let parent = self.ast(containing)?.node(containing)?.parent();
            let typeof_parent = match parent {
                Some(parent) => self.ast(parent)?.node(parent)?.kind() == K::TypeOfExpression,
                None => false,
            };
            if !typeof_parent && self.qualified_name_as_value(containing)?.is_some() {
                let text = self.entity_name_text(containing)?;
                self.error_at(
                    Some(containing),
                    d::X_0_refers_to_a_value_but_is_being_used_as_a_type_here_Did_you_mean_typeof_0,
                    vec![text],
                )?;
                return Ok(());
            }
        }
        if meaning & sf::NAMESPACE != 0 {
            if let Some(parent) = self.ast(name)?.node(name)?.parent() {
                if self.ast(parent)?.node(parent)?.kind() == K::QualifiedName {
                    let text = self.ast(right)?.node_text(right)?.into_js_string();
                    let exports = self.module_exports_of_symbol(namespace)?;
                    if let Some(exported) =
                        self.lookup_symbol_resolving(exports, text.as_bytes(), sf::TYPE)?
                    {
                        let member = self
                            .ast(parent)?
                            .node(parent)?
                            .data_source()
                            .as_qualified_name()
                            .and_then(|data| data.right())
                            .ok_or(Error::MissingLink("qualified name member"))?;
                        let text = self.symbol_to_string(self.get_merged_symbol(exported))?;
                        let member_text = self.ast(member)?.node_text(member)?.into_js_string();
                        self.error_at(Some(member), d::Cannot_access_0_1_because_0_is_a_type_but_not_a_namespace_Did_you_mean_to_retrieve_the_type_of_the_property_1_in_0_with_0_1, vec![text, member_text])?;
                        return Ok(());
                    }
                }
            }
        }
        self.error_at(
            Some(right),
            d::Namespace_0_has_no_exported_member_1,
            vec![namespace_name, declaration_name],
        )?;
        Ok(())
    }
    // port: tsc/internal/checker/checker.go:Checker.tryGetQualifiedNameAsValue
    fn qualified_name_as_value(&mut self, mut node: NodeId) -> Result<Option<SymbolId>, Error> {
        while self.ast(node)?.node(node)?.kind() == K::QualifiedName {
            node = self
                .ast(node)?
                .node(node)?
                .data_source()
                .as_qualified_name()
                .and_then(|data| data.left())
                .ok_or(Error::MissingLink("qualified name left"))?;
        }
        let text = self.ast(node)?.node_text(node)?.into_js_string();
        let Some(mut symbol) =
            self.resolve_name(Some(node), text.as_bytes(), sf::VALUE, None, true)?
        else {
            return Ok(None);
        };
        while let Some(parent) = self.ast(node)?.node(node)?.parent() {
            if self.ast(parent)?.node(parent)?.kind() != K::QualifiedName {
                break;
            }
            let right = self
                .ast(parent)?
                .node(parent)?
                .data_source()
                .as_qualified_name()
                .and_then(|data| data.right())
                .ok_or(Error::MissingLink("qualified name right"))?;
            let text = self.ast(right)?.node_text(right)?.into_js_string();
            let ty = self.get_type_of_symbol(symbol)?;
            let Some(property) = self.constituent_property(ty, text.as_bytes(), false)? else {
                return Ok(None);
            };
            symbol = property;
            node = parent;
        }
        Ok(Some(symbol))
    }
}
