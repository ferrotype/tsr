use crate::{parse_flags, Parser, ParserFactory, ParsingContext};
use ts_ast::{modifier_flags, node_flags, FactoryMethods, NodeId, NodeListId, SyntaxKind as K};
use ts_core::TextRange;
use ts_diagnostics as diag;

impl<F: ParserFactory> Parser<'_, F> {
    /// port: tsc/internal/parser/parser.go:Parser.parseDeclaration
    pub(crate) fn parse_declaration(&mut self) -> NodeId {
        let pos = self.node_pos();
        let jsdoc = self.jsdoc_scanner_info();
        let modifiers = self.parse_modifiers_ex(true, false, false);
        let saved = self.context_flags;
        if self.has_modifier_kind(modifiers, K::DeclareKeyword) {
            self.mark_modifiers_ambient(modifiers.expect("declare modifier"));
            self.set_context_flags(node_flags::AMBIENT, true);
        }
        let node = self.parse_declaration_worker(pos, jsdoc, modifiers);
        self.context_flags = saved;
        node
    }
    /// port: tsc/internal/parser/parser.go:Parser.parseDeclarationWorker
    pub(crate) fn parse_declaration_worker(
        &mut self,
        pos: i64,
        jsdoc: u8,
        modifiers: Option<NodeListId>,
    ) -> NodeId {
        let token = self.token;
        match token {
            K::VarKeyword | K::LetKeyword | K::ConstKeyword | K::UsingKeyword => {
                return self.parse_variable_statement(pos, jsdoc, modifiers)
            }
            K::AwaitKeyword if self.is_await_using_declaration() => {
                return self.parse_variable_statement(pos, jsdoc, modifiers)
            }
            K::FunctionKeyword => return self.parse_function_declaration(pos, jsdoc, modifiers),
            K::ClassKeyword => return self.parse_class_declaration(pos, jsdoc, modifiers),
            K::InterfaceKeyword => return self.parse_interface_declaration(pos, jsdoc, modifiers),
            K::TypeKeyword => return self.parse_type_alias_declaration(pos, jsdoc, modifiers),
            K::EnumKeyword => return self.parse_enum_declaration(pos, jsdoc, modifiers),
            K::GlobalKeyword | K::ModuleKeyword | K::NamespaceKeyword => {
                return self.parse_module_declaration(pos, jsdoc, modifiers)
            }
            K::ImportKeyword => {
                return self
                    .parse_import_declaration_or_import_equals_declaration(pos, jsdoc, modifiers)
            }
            K::ExportKeyword => {
                self.next_token();
                return match self.token {
                    K::DefaultKeyword | K::EqualsToken => {
                        self.parse_export_assignment(pos, jsdoc, modifiers)
                    }
                    K::AsKeyword => self.parse_namespace_export_declaration(pos, jsdoc, modifiers),
                    _ => self.parse_export_declaration(pos, jsdoc, modifiers),
                };
            }
            _ => {}
        }
        if modifiers.is_some() {
            self.parse_error_at(
                self.node_pos(),
                self.node_pos(),
                diag::Declaration_expected,
                Vec::new(),
            );
            let node = self.factory.new_missing_declaration(modifiers);
            return self.finish_node(node, pos);
        }
        panic!("Unhandled case in parseDeclarationWorker")
    }
    /// port: tsc/internal/parser/parser.go:Parser.parseFunctionDeclaration
    pub(crate) fn parse_function_declaration(
        &mut self,
        pos: i64,
        jsdoc: u8,
        modifiers: Option<NodeListId>,
    ) -> NodeId {
        self.parse_expected(K::FunctionKeyword);
        let asterisk = self.parse_optional_token(K::AsteriskToken);
        let modifier_flags = modifiers.map_or(0, |id| self.factory.read_list(id).modifier_flags());
        let name = if modifiers.is_none()
            || modifier_flags & modifier_flags::DEFAULT == 0
            || self.is_binding_identifier()
        {
            Some(self.parse_binding_identifier())
        } else {
            None
        };
        let flags = if asterisk.is_some() {
            parse_flags::YIELD
        } else {
            0
        } | if modifier_flags & modifier_flags::ASYNC != 0 {
            parse_flags::AWAIT
        } else {
            0
        };
        let type_parameters = self.parse_type_parameters();
        let saved = self.context_flags;
        if modifier_flags & modifier_flags::EXPORT != 0 {
            self.set_context_flags(node_flags::AWAIT_CONTEXT, true);
        }
        let parameters = self.parse_parameters(flags);
        let ty = self.parse_return_type(K::ColonToken, false);
        let body = self.parse_function_block_or_semicolon(flags, Some(diag::X_or_expected));
        self.context_flags = saved;
        let node = self.factory.new_function_declaration(
            modifiers,
            asterisk,
            name,
            type_parameters,
            parameters,
            ty,
            None,
            body,
        );
        self.finish_node(node, pos);
        self.with_js_doc(node, jsdoc);
        self.check_js_syntax(node)
    }
    /// port: tsc/internal/parser/parser.go:Parser.parseInterfaceDeclaration
    pub(crate) fn parse_interface_declaration(
        &mut self,
        pos: i64,
        jsdoc: u8,
        modifiers: Option<NodeListId>,
    ) -> NodeId {
        self.parse_expected(K::InterfaceKeyword);
        let name = self.parse_identifier();
        let type_parameters = self.parse_type_parameters();
        let heritage = self.parse_heritage_clauses(true);
        let members = self.parse_object_type_members();
        let node = self.factory.new_interface_declaration(
            modifiers,
            Some(name),
            type_parameters,
            heritage,
            Some(members),
        );
        self.finish_node(node, pos);
        self.with_js_doc(node, jsdoc);
        self.check_js_syntax(node)
    }
    /// port: tsc/internal/parser/parser.go:Parser.parseTypeAliasDeclaration
    pub(crate) fn parse_type_alias_declaration(
        &mut self,
        pos: i64,
        jsdoc: u8,
        modifiers: Option<NodeListId>,
    ) -> NodeId {
        self.parse_expected(K::TypeKeyword);
        if self.has_preceding_line_break() {
            self.parse_error_at_current_token(diag::Line_break_not_permitted_here, Vec::new());
        }
        let name = self.parse_identifier();
        let type_parameters = self.parse_type_parameters();
        self.parse_expected(K::EqualsToken);
        let ty = if self.token == K::IntrinsicKeyword && self.look_ahead(Self::next_is_not_dot) {
            self.parse_keyword_type_node()
        } else {
            self.parse_type()
        };
        self.parse_semicolon();
        let node = self.factory.new_type_alias_declaration(
            modifiers,
            Some(name),
            type_parameters,
            Some(ty),
        );
        self.finish_node(node, pos);
        self.with_js_doc(node, jsdoc);
        self.check_js_syntax(node)
    }
    /// port: tsc/internal/parser/parser.go:Parser.nextIsNotDot
    pub(crate) fn next_is_not_dot(&mut self) -> bool {
        self.next_token() != K::DotToken
    }
    /// port: tsc/internal/parser/parser.go:Parser.parseEnumMember
    pub(crate) fn parse_enum_member(&mut self) -> NodeId {
        let pos = self.node_pos();
        let jsdoc = self.jsdoc_scanner_info();
        let name = self.parse_property_name();
        let initializer = self.do_in_context(
            node_flags::DISALLOW_IN_CONTEXT,
            false,
            Self::parse_initializer,
        );
        let node = self.factory.new_enum_member(Some(name), initializer);
        self.finish_node(node, pos);
        self.with_js_doc(node, jsdoc);
        node
    }
    /// port: tsc/internal/parser/parser.go:Parser.parseEnumDeclaration
    pub(crate) fn parse_enum_declaration(
        &mut self,
        pos: i64,
        jsdoc: u8,
        modifiers: Option<NodeListId>,
    ) -> NodeId {
        let saved_await = self.statement_has_await_identifier;
        self.parse_expected(K::EnumKeyword);
        let name = self.parse_identifier();
        let members = if self.parse_expected(K::OpenBraceToken) {
            let members = self.do_in_context(
                node_flags::YIELD_CONTEXT | node_flags::AWAIT_CONTEXT,
                false,
                |p| p.parse_delimited_list(ParsingContext::EnumMembers, Self::parse_enum_member),
            );
            self.parse_expected(K::CloseBraceToken);
            members
        } else {
            Some(self.create_missing_list())
        };
        let node = self
            .factory
            .new_enum_declaration(modifiers, Some(name), members);
        self.finish_node(node, pos);
        self.with_js_doc(node, jsdoc);
        self.check_js_syntax(node);
        self.statement_has_await_identifier = saved_await;
        node
    }
    /// port: tsc/internal/parser/parser.go:Parser.parseModuleDeclaration
    pub(crate) fn parse_module_declaration(
        &mut self,
        pos: i64,
        jsdoc: u8,
        modifiers: Option<NodeListId>,
    ) -> NodeId {
        let mut keyword = K::ModuleKeyword;
        if self.token == K::GlobalKeyword {
            return self.parse_ambient_external_module_declaration(pos, jsdoc, modifiers);
        }
        if self.parse_optional(K::NamespaceKeyword) {
            keyword = K::NamespaceKeyword;
        } else {
            self.parse_expected(K::ModuleKeyword);
            if self.token == K::StringLiteral {
                return self.parse_ambient_external_module_declaration(pos, jsdoc, modifiers);
            }
        }
        self.parse_module_or_namespace_declaration(pos, jsdoc, modifiers, false, keyword)
    }
    /// port: tsc/internal/parser/parser.go:Parser.parseAmbientExternalModuleDeclaration
    pub(crate) fn parse_ambient_external_module_declaration(
        &mut self,
        pos: i64,
        jsdoc: u8,
        modifiers: Option<NodeListId>,
    ) -> NodeId {
        let saved_await = self.statement_has_await_identifier;
        let (keyword, name) = if self.token == K::GlobalKeyword {
            (K::GlobalKeyword, self.parse_identifier())
        } else {
            (K::ModuleKeyword, self.parse_literal_expression())
        };
        let attributes = if keyword == K::ModuleKeyword && self.parse_optional(K::WithKeyword) {
            Some(self.parse_type_literal())
        } else {
            None
        };
        let body = if self.token == K::OpenBraceToken {
            Some(self.parse_module_block())
        } else {
            self.parse_semicolon();
            None
        };
        let node = self.factory.new_module_declaration(
            modifiers,
            keyword.into(),
            Some(name),
            attributes,
            body,
        );
        self.finish_node(node, pos);
        self.with_js_doc(node, jsdoc);
        self.statement_has_await_identifier = saved_await;
        node
    }
    /// port: tsc/internal/parser/parser.go:Parser.parseModuleBlock
    pub(crate) fn parse_module_block(&mut self) -> NodeId {
        let pos = self.node_pos();
        let statements = if self.parse_expected(K::OpenBraceToken) {
            let statements =
                self.parse_list(ParsingContext::BlockStatements, Self::parse_statement);
            self.parse_expected(K::CloseBraceToken);
            statements
        } else {
            self.create_missing_list()
        };
        let node = self.factory.new_module_block(Some(statements));
        self.finish_node(node, pos)
    }
    /// port: tsc/internal/parser/parser.go:Parser.parseModuleOrNamespaceDeclaration
    pub(crate) fn parse_module_or_namespace_declaration(
        &mut self,
        pos: i64,
        jsdoc: u8,
        modifiers: Option<NodeListId>,
        nested: bool,
        keyword: K,
    ) -> NodeId {
        crate::recursion::guarded(|| {
            let saved_await = self.statement_has_await_identifier;
            let name = if nested {
                self.parse_identifier_name()
            } else {
                self.parse_identifier()
            };
            let body = if self.parse_optional(K::DotToken) {
                let export = self.factory.new_modifier(K::ExportKeyword.into());
                let range = TextRange::new(self.node_pos(), self.node_pos());
                self.factory.set_node_range(export, range);
                self.factory.set_node_flags(export, node_flags::REPARSED);
                let modifiers = self.new_modifier_list(range, vec![export]);
                self.parse_module_or_namespace_declaration(
                    self.node_pos(),
                    0,
                    Some(modifiers),
                    true,
                    keyword,
                )
            } else {
                self.parse_module_block()
            };
            let node = self.factory.new_module_declaration(
                modifiers,
                keyword.into(),
                Some(name),
                None,
                Some(body),
            );
            self.finish_node(node, pos);
            self.with_js_doc(node, jsdoc);
            self.check_js_syntax(node);
            self.statement_has_await_identifier = saved_await;
            node
        })
    }
}
