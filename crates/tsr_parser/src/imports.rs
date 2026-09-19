use crate::tokens::token_is_identifier_or_keyword;
use crate::{Parser, ParserFactory, ParsingContext};
use ts_ast::{
    node_flags, FactoryMethods, JsString, NodeDataRead, NodeId, NodeListId, SyntaxKind as K,
};
use ts_diagnostics as diag;

impl<F: ParserFactory> Parser<'_, F> {
    fn import_identifier_has_text(&self, node: NodeId, text: &[u8]) -> bool {
        let node = self.factory.node(node);
        let NodeDataRead::Identifier(data) = node.data() else {
            unreachable!("import identifier")
        };
        data.text() == text
    }
    /// port: tsc/internal/parser/parser.go:Parser.parseImportDeclarationOrImportEqualsDeclaration
    pub(crate) fn parse_import_declaration_or_import_equals_declaration(
        &mut self,
        pos: i64,
        jsdoc: u8,
        modifiers: Option<NodeListId>,
    ) -> NodeId {
        self.parse_expected(K::ImportKeyword);
        let after_import_pos = self.node_pos();
        let saved_await = self.statement_has_await_identifier;
        let mut identifier = self.is_identifier().then(|| self.parse_identifier());
        let mut phase = K::Unknown;
        if identifier.is_some_and(|id| self.import_identifier_has_text(id, b"type"))
            && (self.token != K::FromKeyword
                || self.is_identifier()
                    && self.look_ahead(Self::next_token_is_from_keyword_or_equals_token))
            && (self.is_identifier()
                || self.token_after_import_definitely_produces_import_declaration())
        {
            phase = K::TypeKeyword;
            identifier = self.is_identifier().then(|| self.parse_identifier());
        } else if identifier.is_some_and(|id| self.import_identifier_has_text(id, b"defer")) {
            let parse_as_defer = if self.token == K::FromKeyword {
                !self.look_ahead(Self::next_token_is_token_string_literal)
            } else {
                !matches!(self.token, K::CommaToken | K::EqualsToken)
            };
            if parse_as_defer {
                phase = K::DeferKeyword;
                identifier = self.is_identifier().then(|| self.parse_identifier());
            }
        }
        if let Some(identifier) = identifier {
            if !self.token_after_imported_identifier_definitely_produces_import_declaration()
                && phase != K::DeferKeyword
            {
                let node = self.parse_import_equals_declaration(
                    pos,
                    jsdoc,
                    modifiers,
                    identifier,
                    phase == K::TypeKeyword,
                );
                self.check_js_syntax(node);
                self.statement_has_await_identifier = saved_await;
                return node;
            }
        }
        let clause = self.try_parse_import_clause(identifier, after_import_pos, phase, false);
        self.statement_has_await_identifier = saved_await;
        let specifier = self.parse_module_specifier();
        let attributes = self.try_parse_import_attributes();
        self.parse_semicolon();
        let node =
            self.factory
                .new_import_declaration(modifiers, clause, Some(specifier), attributes);
        self.finish_node(node, pos);
        self.with_js_doc(node, jsdoc);
        self.check_js_syntax(node)
    }
    /// port: tsc/internal/parser/parser.go:Parser.nextTokenIsFromKeywordOrEqualsToken
    pub(crate) fn next_token_is_from_keyword_or_equals_token(&mut self) -> bool {
        self.next_token();
        matches!(self.token, K::FromKeyword | K::EqualsToken)
    }
    /// port: tsc/internal/parser/parser.go:Parser.tokenAfterImportDefinitelyProducesImportDeclaration
    pub(crate) fn token_after_import_definitely_produces_import_declaration(&self) -> bool {
        matches!(self.token, K::AsteriskToken | K::OpenBraceToken)
    }
    /// port: tsc/internal/parser/parser.go:Parser.tokenAfterImportedIdentifierDefinitelyProducesImportDeclaration
    pub(crate) fn token_after_imported_identifier_definitely_produces_import_declaration(
        &self,
    ) -> bool {
        matches!(self.token, K::CommaToken | K::FromKeyword)
    }
    /// port: tsc/internal/parser/parser.go:Parser.parseImportEqualsDeclaration
    pub(crate) fn parse_import_equals_declaration(
        &mut self,
        pos: i64,
        jsdoc: u8,
        modifiers: Option<NodeListId>,
        identifier: NodeId,
        type_only: bool,
    ) -> NodeId {
        self.parse_expected(K::EqualsToken);
        let reference = self.parse_module_reference();
        self.parse_semicolon();
        let node = self.factory.new_import_equals_declaration(
            modifiers,
            type_only,
            Some(identifier),
            Some(reference),
        );
        self.finish_node(node, pos);
        self.with_js_doc(node, jsdoc);
        node
    }
    /// port: tsc/internal/parser/parser.go:Parser.parseModuleReference
    pub(crate) fn parse_module_reference(&mut self) -> NodeId {
        if self.token == K::RequireKeyword && self.look_ahead(Self::next_token_is_open_paren) {
            self.parse_external_module_reference()
        } else {
            self.parse_entity_name(false, false, None)
        }
    }
    /// port: tsc/internal/parser/parser.go:Parser.parseExternalModuleReference
    pub(crate) fn parse_external_module_reference(&mut self) -> NodeId {
        let saved_await = self.statement_has_await_identifier;
        let pos = self.node_pos();
        self.parse_expected(K::RequireKeyword);
        self.parse_expected(K::OpenParenToken);
        let expression = self.parse_module_specifier();
        self.parse_expected(K::CloseParenToken);
        let node = self.factory.new_external_module_reference(Some(expression));
        self.finish_node(node, pos);
        self.statement_has_await_identifier = saved_await;
        node
    }
    /// port: tsc/internal/parser/parser.go:Parser.parseModuleSpecifier
    pub(crate) fn parse_module_specifier(&mut self) -> NodeId {
        if self.token == K::StringLiteral {
            self.parse_literal_expression()
        } else {
            self.parse_expression()
        }
    }
    /// port: tsc/internal/parser/parser.go:Parser.tryParseImportClause
    pub(crate) fn try_parse_import_clause(
        &mut self,
        identifier: Option<NodeId>,
        pos: i64,
        phase: K,
        skip_asterisks: bool,
    ) -> Option<NodeId> {
        if identifier.is_some() || matches!(self.token, K::AsteriskToken | K::OpenBraceToken) {
            let node = self.parse_import_clause(identifier, pos, phase, skip_asterisks);
            self.parse_expected(K::FromKeyword);
            Some(node)
        } else {
            None
        }
    }
    /// port: tsc/internal/parser/parser.go:Parser.parseImportClause
    pub(crate) fn parse_import_clause(
        &mut self,
        identifier: Option<NodeId>,
        pos: i64,
        phase: K,
        skip_asterisks: bool,
    ) -> NodeId {
        let saved_await = self.statement_has_await_identifier;
        let mut bindings = None;
        if identifier.is_none() || self.parse_optional(K::CommaToken) {
            if skip_asterisks {
                self.scanner.set_skip_jsdoc_leading_asterisks(true);
            }
            bindings = Some(if self.token == K::AsteriskToken {
                self.parse_namespace_import()
            } else {
                self.parse_named_imports()
            });
            if skip_asterisks {
                self.scanner.set_skip_jsdoc_leading_asterisks(false);
            }
        }
        let node = self
            .factory
            .new_import_clause(phase.into(), identifier, bindings);
        self.finish_node(node, pos);
        self.statement_has_await_identifier = saved_await;
        node
    }
    /// port: tsc/internal/parser/parser.go:Parser.parseNamespaceImport
    pub(crate) fn parse_namespace_import(&mut self) -> NodeId {
        let pos = self.node_pos();
        self.parse_expected(K::AsteriskToken);
        self.parse_expected(K::AsKeyword);
        let name = self.parse_identifier();
        let node = self.factory.new_namespace_import(Some(name));
        self.finish_node(node, pos)
    }
    /// port: tsc/internal/parser/parser.go:Parser.parseNamedImports
    pub(crate) fn parse_named_imports(&mut self) -> NodeId {
        let pos = self.node_pos();
        let imports = self.parse_bracketed_list(
            ParsingContext::ImportOrExportSpecifiers,
            Self::parse_import_specifier,
            K::OpenBraceToken,
            K::CloseBraceToken,
        );
        let node = self.factory.new_named_imports(imports);
        self.finish_node(node, pos)
    }
    /// port: tsc/internal/parser/parser.go:Parser.parseImportSpecifier
    pub(crate) fn parse_import_specifier(&mut self) -> NodeId {
        let pos = self.node_pos();
        let (type_only, property, name) = self.parse_import_or_export_specifier(K::ImportSpecifier);
        let identifier = if self.factory.node(name).kind() == K::Identifier {
            name
        } else {
            let range = self.factory.node(name).range();
            let range_without_trivia = self.skip_range_trivia(range);
            self.parse_error_at_range(range_without_trivia, diag::Identifier_expected, Vec::new());
            let identifier = self.new_identifier(JsString::default());
            self.finish_node(identifier, range.pos());
            identifier
        };
        let node = self
            .factory
            .new_import_specifier(type_only, property, Some(identifier));
        self.finish_node(node, pos);
        self.check_js_syntax(node)
    }
    /// port: tsc/internal/parser/parser.go:Parser.parseImportOrExportSpecifier
    pub(crate) fn parse_import_or_export_specifier(
        &mut self,
        kind: K,
    ) -> (bool, Option<NodeId>, NodeId) {
        let disallow_keywords = kind == K::ImportSpecifier;
        let mut can_parse_as = true;
        let mut type_only = false;
        let mut property = None;
        let (mut name, mut name_ok) = self.parse_module_export_name(disallow_keywords);
        if self.factory.node(name).kind() == K::Identifier
            && self.import_identifier_has_text(name, b"type")
        {
            if self.token == K::AsKeyword {
                let first_as = self.parse_identifier_name();
                if self.token == K::AsKeyword {
                    let second_as = self.parse_identifier_name();
                    if self.can_parse_module_export_name() {
                        type_only = true;
                        property = Some(first_as);
                        (name, name_ok) = self.parse_module_export_name(disallow_keywords);
                        can_parse_as = false;
                    } else {
                        property = Some(name);
                        name = second_as;
                        can_parse_as = false;
                    }
                } else if self.can_parse_module_export_name() {
                    property = Some(name);
                    can_parse_as = false;
                    (name, name_ok) = self.parse_module_export_name(disallow_keywords);
                } else {
                    type_only = true;
                    name = first_as;
                }
            } else if self.can_parse_module_export_name() {
                type_only = true;
                (name, name_ok) = self.parse_module_export_name(disallow_keywords);
            }
        }
        if can_parse_as && self.token == K::AsKeyword {
            property = Some(name);
            self.parse_expected(K::AsKeyword);
            (name, name_ok) = self.parse_module_export_name(disallow_keywords);
        }
        if !name_ok {
            let range = self.skip_range_trivia(self.factory.node(name).range());
            self.parse_error_at_range(range, diag::Identifier_expected, Vec::new());
        }
        (type_only, property, name)
    }
    /// port: tsc/internal/parser/parser.go:Parser.canParseModuleExportName
    pub(crate) fn can_parse_module_export_name(&self) -> bool {
        token_is_identifier_or_keyword(self.token) || self.token == K::StringLiteral
    }
    /// port: tsc/internal/parser/parser.go:Parser.parseModuleExportName
    pub(crate) fn parse_module_export_name(&mut self, disallow_keywords: bool) -> (NodeId, bool) {
        if self.token == K::StringLiteral {
            return (self.parse_literal_expression(), true);
        }
        let ok = !(disallow_keywords
            && ts_ast::is_keyword_kind(self.token.into())
            && !self.is_identifier());
        (self.parse_identifier_name(), ok)
    }
    /// port: tsc/internal/parser/parser.go:Parser.tryParseImportAttributes
    pub(crate) fn try_parse_import_attributes(&mut self) -> Option<NodeId> {
        if self.token == K::WithKeyword
            || self.token == K::AssertKeyword && !self.has_preceding_line_break()
        {
            if self.token == K::AssertKeyword {
                self.parse_error_at_current_token(diag::Import_assertions_have_been_replaced_by_import_attributes_Use_with_instead_of_assert, Vec::new());
            }
            Some(self.parse_import_attributes(self.token, false))
        } else {
            None
        }
    }
    /// port: tsc/internal/parser/parser.go:Parser.parseExportAssignment
    pub(crate) fn parse_export_assignment(
        &mut self,
        pos: i64,
        jsdoc: u8,
        modifiers: Option<NodeListId>,
    ) -> NodeId {
        let saved_context = self.context_flags;
        let saved_await = self.statement_has_await_identifier;
        self.set_context_flags(node_flags::AWAIT_CONTEXT, true);
        let export_equals = self.parse_optional(K::EqualsToken);
        if !export_equals {
            self.parse_expected(K::DefaultKeyword);
        }
        let expression = self.parse_assignment_expression_or_higher();
        self.parse_semicolon();
        self.context_flags = saved_context;
        self.statement_has_await_identifier = saved_await;
        let node =
            self.factory
                .new_export_assignment(modifiers, export_equals, None, Some(expression));
        self.finish_node(node, pos);
        self.with_js_doc(node, jsdoc);
        self.check_js_syntax(node)
    }
    /// port: tsc/internal/parser/parser.go:Parser.parseNamespaceExportDeclaration
    pub(crate) fn parse_namespace_export_declaration(
        &mut self,
        pos: i64,
        jsdoc: u8,
        modifiers: Option<NodeListId>,
    ) -> NodeId {
        self.parse_expected(K::AsKeyword);
        self.parse_expected(K::NamespaceKeyword);
        let saved_await = self.statement_has_await_identifier;
        let name = self.parse_identifier();
        self.statement_has_await_identifier = saved_await;
        self.parse_semicolon();
        let node = self
            .factory
            .new_namespace_export_declaration(modifiers, Some(name));
        self.finish_node(node, pos);
        self.with_js_doc(node, jsdoc);
        node
    }
    /// port: tsc/internal/parser/parser.go:Parser.parseExportDeclaration
    pub(crate) fn parse_export_declaration(
        &mut self,
        pos: i64,
        jsdoc: u8,
        modifiers: Option<NodeListId>,
    ) -> NodeId {
        let saved_context = self.context_flags;
        let saved_await = self.statement_has_await_identifier;
        self.set_context_flags(node_flags::AWAIT_CONTEXT, true);
        let type_only = self.parse_optional(K::TypeKeyword);
        let namespace_pos = self.node_pos();
        let mut clause = None;
        let mut specifier = None;
        let mut attributes = None;
        if self.parse_optional(K::AsteriskToken) {
            if self.parse_optional(K::AsKeyword) {
                clause = Some(self.parse_namespace_export(namespace_pos));
            }
            self.parse_expected(K::FromKeyword);
            specifier = Some(self.parse_module_specifier());
        } else {
            clause = Some(self.parse_named_exports());
            if self.token == K::FromKeyword
                || self.token == K::StringLiteral && !self.has_preceding_line_break()
            {
                self.parse_expected(K::FromKeyword);
                specifier = Some(self.parse_module_specifier());
            }
        }
        if specifier.is_some()
            && matches!(self.token, K::WithKeyword | K::AssertKeyword)
            && !self.has_preceding_line_break()
        {
            if self.token == K::AssertKeyword {
                self.parse_error_at_current_token(diag::Import_assertions_have_been_replaced_by_import_attributes_Use_with_instead_of_assert, Vec::new());
            }
            attributes = Some(self.parse_import_attributes(self.token, false));
        }
        self.parse_semicolon();
        self.context_flags = saved_context;
        self.statement_has_await_identifier = saved_await;
        let node = self
            .factory
            .new_export_declaration(modifiers, type_only, clause, specifier, attributes);
        self.finish_node(node, pos);
        self.with_js_doc(node, jsdoc);
        self.check_js_syntax(node)
    }
    /// port: tsc/internal/parser/parser.go:Parser.parseNamespaceExport
    pub(crate) fn parse_namespace_export(&mut self, pos: i64) -> NodeId {
        let (name, _) = self.parse_module_export_name(false);
        let node = self.factory.new_namespace_export(Some(name));
        self.finish_node(node, pos)
    }
    /// port: tsc/internal/parser/parser.go:Parser.parseNamedExports
    pub(crate) fn parse_named_exports(&mut self) -> NodeId {
        let pos = self.node_pos();
        let exports = self.parse_bracketed_list(
            ParsingContext::ImportOrExportSpecifiers,
            Self::parse_export_specifier,
            K::OpenBraceToken,
            K::CloseBraceToken,
        );
        let node = self.factory.new_named_exports(exports);
        self.finish_node(node, pos)
    }
    /// port: tsc/internal/parser/parser.go:Parser.parseExportSpecifier
    pub(crate) fn parse_export_specifier(&mut self) -> NodeId {
        let pos = self.node_pos();
        let jsdoc = self.jsdoc_scanner_info();
        let (type_only, property, name) = self.parse_import_or_export_specifier(K::ExportSpecifier);
        let node = self
            .factory
            .new_export_specifier(type_only, property, Some(name));
        self.finish_node(node, pos);
        self.with_js_doc(node, jsdoc);
        self.check_js_syntax(node)
    }
}
