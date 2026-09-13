//! Eager documentation references participate in name usage without forcing lazy comments.
use crate::{CheckerState, Error};
use ts_arena::{NodeId, SymbolId};
use ts_ast::{symbol_flags as sf, SyntaxKind as K};

impl CheckerState {
    // port: tsc/internal/checker/checker.go:Checker.checkSourceElementWorker
    pub(crate) fn check_eager_jsdoc(&mut self, node: NodeId) -> Result<(), Error> {
        let Some(docs) = self.eager_jsdoc_for_node(node)? else {
            return Ok(());
        };
        for &doc in docs.iter() {
            self.check_jsdoc_comments(doc)?;
            let tags = self
                .ast(doc)?
                .node(doc)?
                .data_source()
                .as_js_doc()
                .ok_or(ts_arena::Error::InvalidGraph)?
                .tags();
            for tag in self.source_list(doc, tags)? {
                self.check_jsdoc_comments(tag)?;
            }
        }
        Ok(())
    }

    // port: tsc/internal/checker/checker.go:Checker.checkJSDocComments
    fn check_jsdoc_comments(&mut self, node: NodeId) -> Result<(), Error> {
        let view = self.ast(node)?;
        let comments: Vec<_> = view
            .node_slice(view.node(node)?.comments(view)?)?
            .iter()
            .flatten()
            .collect();
        for comment in comments {
            let read = self.ast(comment)?.node(comment)?;
            if matches!(
                read.kind().known(),
                Some(K::JSDocLink | K::JSDocLinkCode | K::JSDocLinkPlain)
            ) {
                self.resolve_jsdoc_member_name(read.name())?;
            }
        }
        Ok(())
    }

    // port: tsc/internal/ast/utilities.go:GetHostSignatureFromJSDoc
    pub(crate) fn jsdoc_host_signature(&self, name: NodeId) -> Result<Option<NodeId>, Error> {
        let Some(host) = ts_ast::utilities_tail::get_js_doc_host(self.ast(name)?, name)? else {
            return Ok(None);
        };
        let read = self.ast(host)?.node(host)?;
        if read.kind() == K::PropertySignature {
            if let Some(ty) = read.type_node() {
                if ts_ast::utilities::is_function_like(Some(&self.ast(ty)?.node(ty)?)) {
                    return Ok(Some(ty));
                }
            }
        }
        Ok(ts_ast::utilities::is_function_like(Some(&read)).then_some(host))
    }

    // port: tsc/internal/checker/checker.go:Checker.resolveJSDocMemberName
    pub(crate) fn resolve_jsdoc_member_name(
        &mut self,
        name: Option<NodeId>,
    ) -> Result<Option<SymbolId>, Error> {
        let Some(name) = name else {
            return Ok(None);
        };
        let read = self.ast(name)?.node(name)?;
        if !matches!(read.kind().known(), Some(K::Identifier | K::QualifiedName)) {
            return Ok(None);
        }
        let location = self.jsdoc_host_signature(name)?;
        if let Some(symbol) = self.resolve_entity_name_at(
            name,
            sf::TYPE | sf::NAMESPACE | sf::VALUE,
            true,
            true,
            location,
        )? {
            return Ok(Some(symbol));
        }
        let read = self.ast(name)?.node(name)?;
        let Some(data) = read.data_source().as_qualified_name() else {
            return Ok(None);
        };
        let left = data.left();
        let right = data
            .right()
            .ok_or(Error::MissingLink("documentation member name"))?;
        let Some(symbol) = self.resolve_jsdoc_member_name(left)? else {
            return Ok(None);
        };
        let mut prototype = None;
        if self.symbol(symbol)?.flags() & sf::VALUE != 0 {
            let ty = self.get_type_of_symbol(symbol)?;
            if let Some(property) = self.constituent_property(ty, b"prototype", false)? {
                prototype = Some(self.get_type_of_symbol(property)?);
            }
        }
        let ty = match prototype {
            Some(ty) => ty,
            None => self.get_declared_type_of_symbol(symbol)?,
        };
        let text = self.ast(right)?.node_text(right)?.into_js_string();
        self.constituent_property(ty, text.as_bytes(), false)
    }

    pub(crate) fn jsdoc_identifier_symbol(
        &mut self,
        name: NodeId,
    ) -> Result<Option<SymbolId>, Error> {
        let meaning = sf::VALUE | sf::TYPE | sf::NAMESPACE;
        let location = self.jsdoc_host_signature(name)?;
        if let Some(symbol) = self.resolve_entity_name_at(name, meaning, true, true, location)? {
            return Ok(Some(symbol));
        }
        let text = self.ast(name)?.node_text(name)?.into_js_string();
        let mut parent = self.ast(name)?.node(name)?.parent();
        while let Some(container) = parent {
            let read = self.ast(container)?.node(container)?;
            if matches!(
                read.kind().known(),
                Some(K::ClassDeclaration | K::ClassExpression | K::InterfaceDeclaration)
            ) {
                let Some(symbol) = self.get_symbol_of_declaration(container)? else {
                    return Ok(None);
                };
                let exports = self.module_exports_of_symbol(symbol)?;
                if let Some(property) = self.lookup_symbol(exports, text.as_bytes(), meaning)? {
                    return Ok(Some(self.get_merged_symbol(property)));
                }
                let ty = self.get_declared_type_of_symbol(symbol)?;
                return self.constituent_property(ty, text.as_bytes(), false);
            }
            parent = read.parent();
        }
        Ok(None)
    }
}
