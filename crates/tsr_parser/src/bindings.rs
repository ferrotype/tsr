use crate::{Parser, ParserFactory, ParsingContext};
use ts_ast::{node_flags, FactoryMethods, NodeId, NodeListId, SyntaxKind as K};
use ts_diagnostics as diag;

impl<F: ParserFactory> Parser<'_, F> {
    /// port: tsc/internal/parser/parser.go:Parser.parseVariableStatement
    pub(crate) fn parse_variable_statement(
        &mut self,
        pos: i64,
        jsdoc: u8,
        modifiers: Option<NodeListId>,
    ) -> NodeId {
        let declarations = self.parse_variable_declaration_list(false);
        self.parse_semicolon();
        let node = self
            .factory
            .new_variable_statement(modifiers, Some(declarations));
        self.finish_node(node, pos);
        self.with_js_doc(node, jsdoc);
        self.check_js_syntax(node);
        node
    }
    /// port: tsc/internal/parser/parser.go:Parser.parseVariableDeclarationList
    pub(crate) fn parse_variable_declaration_list(&mut self, in_for: bool) -> NodeId {
        let pos = self.node_pos();
        let flags = match self.token {
            K::VarKeyword => 0,
            K::LetKeyword => node_flags::LET,
            K::ConstKeyword => node_flags::CONST,
            K::UsingKeyword => node_flags::USING,
            K::AwaitKeyword => {
                if self.is_await_using_declaration() {
                    self.next_token();
                    node_flags::AWAIT_USING
                } else {
                    0
                }
            }
            _ => panic!("Unhandled case in parseVariableDeclarationList"),
        };
        self.next_token();
        let declarations = if self.token == K::OfKeyword
            && self.look_ahead(Self::next_is_identifier_and_close_paren)
        {
            Some(self.create_missing_list())
        } else {
            let saved = self.context_flags;
            self.set_context_flags(node_flags::DISALLOW_IN_CONTEXT, in_for);
            let list = if in_for {
                self.parse_delimited_list(
                    ParsingContext::VariableDeclarations,
                    Self::parse_variable_declaration,
                )
            } else {
                self.parse_delimited_list(
                    ParsingContext::VariableDeclarations,
                    Self::parse_variable_declaration_allow_exclamation,
                )
            };
            self.context_flags = saved;
            list
        };
        let node = self
            .factory
            .new_variable_declaration_list(declarations, flags);
        self.finish_node(node, pos)
    }
    /// port: tsc/internal/parser/parser.go:Parser.nextIsIdentifierAndCloseParen
    pub(crate) fn next_is_identifier_and_close_paren(&mut self) -> bool {
        self.next_token_is_identifier() && self.next_token() == K::CloseParenToken
    }
    /// port: tsc/internal/parser/parser.go:Parser.nextTokenIsIdentifier
    pub(crate) fn next_token_is_identifier(&mut self) -> bool {
        self.next_token();
        self.is_identifier()
    }
    /// port: tsc/internal/parser/parser.go:Parser.parseVariableDeclaration
    pub(crate) fn parse_variable_declaration(&mut self) -> NodeId {
        self.parse_variable_declaration_worker(false)
    }
    /// port: tsc/internal/parser/parser.go:Parser.parseVariableDeclarationAllowExclamation
    pub(crate) fn parse_variable_declaration_allow_exclamation(&mut self) -> NodeId {
        self.parse_variable_declaration_worker(true)
    }
    /// port: tsc/internal/parser/parser.go:Parser.parseVariableDeclarationWorker
    pub(crate) fn parse_variable_declaration_worker(&mut self, allow_exclamation: bool) -> NodeId {
        let pos = self.node_pos();
        let jsdoc = self.jsdoc_scanner_info();
        let name = self.parse_identifier_or_pattern_with_diagnostic(Some(
            diag::Private_identifiers_are_not_allowed_in_variable_declarations,
        ));
        let exclamation = if allow_exclamation
            && self.factory.node(name).kind() == K::Identifier
            && self.token == K::ExclamationToken
            && !self.has_preceding_line_break()
        {
            Some(self.parse_token_node())
        } else {
            None
        };
        let ty = self.parse_type_annotation();
        let initializer = if matches!(self.token, K::InKeyword | K::OfKeyword) {
            None
        } else {
            self.parse_initializer()
        };
        let node = self
            .factory
            .new_variable_declaration(Some(name), exclamation, ty, initializer);
        self.finish_node(node, pos);
        self.with_js_doc(node, jsdoc);
        self.check_js_syntax(node);
        node
    }
    /// port: tsc/internal/parser/parser.go:Parser.parseIdentifierOrPattern
    pub(crate) fn parse_identifier_or_pattern(&mut self) -> NodeId {
        self.parse_identifier_or_pattern_with_diagnostic(None)
    }
    /// port: tsc/internal/parser/parser.go:Parser.parseIdentifierOrPatternWithDiagnostic
    pub(crate) fn parse_identifier_or_pattern_with_diagnostic(
        &mut self,
        message: Option<&'static diag::Message>,
    ) -> NodeId {
        crate::recursion::guarded(|| match self.token {
            K::OpenBracketToken => self.parse_array_binding_pattern(),
            K::OpenBraceToken => self.parse_object_binding_pattern(),
            _ => self.parse_binding_identifier_with_diagnostic(message),
        })
    }
    /// port: tsc/internal/parser/parser.go:Parser.parseArrayBindingPattern
    pub(crate) fn parse_array_binding_pattern(&mut self) -> NodeId {
        let pos = self.node_pos();
        self.parse_expected(K::OpenBracketToken);
        let saved = self.context_flags;
        self.set_context_flags(node_flags::DISALLOW_IN_CONTEXT, false);
        let elements = self.parse_delimited_list(
            ParsingContext::ArrayBindingElements,
            Self::parse_array_binding_element,
        );
        self.context_flags = saved;
        self.parse_expected(K::CloseBracketToken);
        let node = self
            .factory
            .new_binding_pattern(K::ArrayBindingPattern.into(), elements);
        self.finish_node(node, pos)
    }
    /// port: tsc/internal/parser/parser.go:Parser.parseArrayBindingElement
    pub(crate) fn parse_array_binding_element(&mut self) -> NodeId {
        let pos = self.node_pos();
        let (rest, name, initializer) = if self.token == K::CommaToken {
            (None, None, None)
        } else {
            let rest = self.parse_optional_token(K::DotDotDotToken);
            let name = self.parse_identifier_or_pattern();
            let initializer = self.parse_initializer();
            (rest, Some(name), initializer)
        };
        let node = self
            .factory
            .new_binding_element(rest, None, name, initializer);
        self.finish_node(node, pos)
    }
    /// port: tsc/internal/parser/parser.go:Parser.parseObjectBindingPattern
    pub(crate) fn parse_object_binding_pattern(&mut self) -> NodeId {
        let pos = self.node_pos();
        self.parse_expected(K::OpenBraceToken);
        let saved = self.context_flags;
        self.set_context_flags(node_flags::DISALLOW_IN_CONTEXT, false);
        let elements = self.parse_delimited_list(
            ParsingContext::ObjectBindingElements,
            Self::parse_object_binding_element,
        );
        self.context_flags = saved;
        self.parse_expected(K::CloseBraceToken);
        let node = self
            .factory
            .new_binding_pattern(K::ObjectBindingPattern.into(), elements);
        self.finish_node(node, pos)
    }
    /// port: tsc/internal/parser/parser.go:Parser.parseObjectBindingElement
    pub(crate) fn parse_object_binding_element(&mut self) -> NodeId {
        let pos = self.node_pos();
        let rest = self.parse_optional_token(K::DotDotDotToken);
        let identifier = self.is_binding_identifier();
        let property_name = self.parse_property_name();
        let (property_name, name) = if identifier && self.token != K::ColonToken {
            (None, property_name)
        } else {
            self.parse_expected(K::ColonToken);
            (Some(property_name), self.parse_identifier_or_pattern())
        };
        let initializer = self.parse_initializer();
        let node = self
            .factory
            .new_binding_element(rest, property_name, Some(name), initializer);
        self.finish_node(node, pos)
    }
    /// port: tsc/internal/parser/parser.go:Parser.parseInitializer
    pub(crate) fn parse_initializer(&mut self) -> Option<NodeId> {
        if self.parse_optional(K::EqualsToken) {
            Some(self.parse_assignment_expression_or_higher())
        } else {
            None
        }
    }
    /// port: tsc/internal/parser/parser.go:Parser.parseTypeAnnotation
    pub(crate) fn parse_type_annotation(&mut self) -> Option<NodeId> {
        if self.parse_optional(K::ColonToken) {
            Some(self.parse_type())
        } else {
            None
        }
    }
    /// port: tsc/internal/parser/parser.go:Parser.isLetDeclaration
    pub(crate) fn is_let_declaration(&mut self) -> bool {
        self.look_ahead(Self::next_token_is_binding_identifier_or_start_of_destructuring)
    }
    /// port: tsc/internal/parser/parser.go:Parser.nextTokenIsBindingIdentifierOrStartOfDestructuring
    pub(crate) fn next_token_is_binding_identifier_or_start_of_destructuring(&mut self) -> bool {
        self.next_token();
        self.is_binding_identifier()
            || matches!(self.token, K::OpenBraceToken | K::OpenBracketToken)
    }
}
