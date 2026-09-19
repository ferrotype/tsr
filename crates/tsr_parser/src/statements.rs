use crate::{Parser, ParserFactory, ParsingContext};
use ts_ast::{node_flags, FactoryMethods, NodeId, SyntaxKind as K};
use ts_diagnostics as diag;

impl<F: ParserFactory> Parser<'_, F> {
    fn finish_statement(&mut self, node: NodeId, pos: i64, jsdoc: u8) -> NodeId {
        self.finish_node(node, pos);
        self.with_js_doc(node, jsdoc);
        node
    }
    /// port: tsc/internal/parser/parser.go:Parser.parseStatement
    pub(crate) fn parse_statement(&mut self) -> NodeId {
        crate::recursion::guarded(|| self.parse_statement_worker())
    }
    fn parse_statement_worker(&mut self) -> NodeId {
        let token = self.token;
        match token {
            K::SemicolonToken => return self.parse_empty_statement(),
            K::OpenBraceToken => return self.parse_block(false, None),
            K::VarKeyword => {
                return self.parse_variable_statement(
                    self.node_pos(),
                    self.jsdoc_scanner_info(),
                    None,
                );
            }
            K::LetKeyword => {
                if self.is_let_declaration() {
                    return self.parse_variable_statement(
                        self.node_pos(),
                        self.jsdoc_scanner_info(),
                        None,
                    );
                }
            }
            K::AwaitKeyword => {
                if self.is_await_using_declaration() {
                    return self.parse_variable_statement(
                        self.node_pos(),
                        self.jsdoc_scanner_info(),
                        None,
                    );
                }
            }
            K::UsingKeyword => {
                if self.is_using_declaration() {
                    return self.parse_variable_statement(
                        self.node_pos(),
                        self.jsdoc_scanner_info(),
                        None,
                    );
                }
            }
            K::FunctionKeyword => {
                return self.parse_function_declaration(
                    self.node_pos(),
                    self.jsdoc_scanner_info(),
                    None,
                );
            }
            K::ClassKeyword => {
                return self.parse_class_declaration(
                    self.node_pos(),
                    self.jsdoc_scanner_info(),
                    None,
                );
            }
            K::IfKeyword => return self.parse_if_statement(),
            K::DoKeyword => return self.parse_do_statement(),
            K::WhileKeyword => return self.parse_while_statement(),
            K::ForKeyword => return self.parse_for_or_for_in_or_for_of_statement(),
            K::ContinueKeyword => return self.parse_continue_statement(),
            K::BreakKeyword => return self.parse_break_statement(),
            K::ReturnKeyword => return self.parse_return_statement(),
            K::WithKeyword => return self.parse_with_statement(),
            K::SwitchKeyword => return self.parse_switch_statement(),
            K::ThrowKeyword => return self.parse_throw_statement(),
            K::TryKeyword | K::CatchKeyword | K::FinallyKeyword => {
                return self.parse_try_statement();
            }
            K::DebuggerKeyword => return self.parse_debugger_statement(),
            K::AtToken => return self.parse_declaration(),
            K::AsyncKeyword
            | K::InterfaceKeyword
            | K::TypeKeyword
            | K::ModuleKeyword
            | K::NamespaceKeyword
            | K::DeclareKeyword
            | K::ConstKeyword
            | K::EnumKeyword
            | K::ExportKeyword
            | K::ImportKeyword
            | K::PrivateKeyword
            | K::ProtectedKeyword
            | K::PublicKeyword
            | K::AbstractKeyword
            | K::AccessorKeyword
            | K::StaticKeyword
            | K::ReadonlyKeyword
            | K::GlobalKeyword
                if self.is_start_of_declaration() =>
            {
                return self.parse_declaration()
            }
            _ => {}
        }
        self.parse_expression_or_labeled_statement()
    }
    /// port: tsc/internal/parser/parser.go:Parser.parseBlock
    pub(crate) fn parse_block(
        &mut self,
        ignore_missing_open: bool,
        message: Option<&'static diag::Message>,
    ) -> NodeId {
        let pos = self.node_pos();
        let jsdoc = self.jsdoc_scanner_info();
        let open = self.scanner.token_start();
        let open_parsed = self.parse_expected_with_diagnostic(K::OpenBraceToken, message, true);
        if open_parsed || ignore_missing_open {
            let multiline = self.has_preceding_line_break();
            let statements =
                self.parse_list(ParsingContext::BlockStatements, Self::parse_statement);
            self.parse_expected_matching_brackets(
                K::OpenBraceToken,
                K::CloseBraceToken,
                open_parsed,
                open,
            );
            let node = self.factory.new_block(Some(statements), multiline);
            self.finish_statement(node, pos, jsdoc);
            if self.token == K::EqualsToken {
                self.parse_error_at_current_token(diag::Declaration_or_statement_expected_This_follows_a_block_of_statements_so_if_you_intended_to_write_a_destructuring_assignment_you_might_need_to_wrap_the_whole_assignment_in_parentheses, Vec::new());
                self.next_token();
            }
            node
        } else {
            let missing = self.create_missing_list();
            let node = self.factory.new_block(Some(missing), false);
            self.finish_statement(node, pos, jsdoc)
        }
    }
    /// port: tsc/internal/parser/parser.go:Parser.parseEmptyStatement
    pub(crate) fn parse_empty_statement(&mut self) -> NodeId {
        let pos = self.node_pos();
        let jsdoc = self.jsdoc_scanner_info();
        self.parse_expected(K::SemicolonToken);
        let node = self.factory.new_empty_statement();
        self.finish_statement(node, pos, jsdoc)
    }
    /// port: tsc/internal/parser/parser.go:Parser.parseIfStatement
    pub(crate) fn parse_if_statement(&mut self) -> NodeId {
        let pos = self.node_pos();
        let jsdoc = self.jsdoc_scanner_info();
        self.parse_expected(K::IfKeyword);
        let open = self.scanner.token_start();
        let parsed = self.parse_expected(K::OpenParenToken);
        let expression = self.parse_expression_allow_in();
        self.parse_expected_matching_brackets(K::OpenParenToken, K::CloseParenToken, parsed, open);
        let then_statement = self.parse_statement();
        let else_statement = if self.parse_optional(K::ElseKeyword) {
            Some(self.parse_statement())
        } else {
            None
        };
        let node =
            self.factory
                .new_if_statement(Some(expression), Some(then_statement), else_statement);
        self.finish_statement(node, pos, jsdoc)
    }
    /// port: tsc/internal/parser/parser.go:Parser.parseDoStatement
    pub(crate) fn parse_do_statement(&mut self) -> NodeId {
        let pos = self.node_pos();
        let jsdoc = self.jsdoc_scanner_info();
        self.parse_expected(K::DoKeyword);
        let statement = self.parse_statement();
        self.parse_expected(K::WhileKeyword);
        let open = self.scanner.token_start();
        let parsed = self.parse_expected(K::OpenParenToken);
        let expression = self.parse_expression_allow_in();
        self.parse_expected_matching_brackets(K::OpenParenToken, K::CloseParenToken, parsed, open);
        self.parse_optional(K::SemicolonToken);
        let node = self
            .factory
            .new_do_statement(Some(statement), Some(expression));
        self.finish_statement(node, pos, jsdoc)
    }
    /// port: tsc/internal/parser/parser.go:Parser.parseWhileStatement
    pub(crate) fn parse_while_statement(&mut self) -> NodeId {
        let pos = self.node_pos();
        let jsdoc = self.jsdoc_scanner_info();
        self.parse_expected(K::WhileKeyword);
        let open = self.scanner.token_start();
        let parsed = self.parse_expected(K::OpenParenToken);
        let expression = self.parse_expression_allow_in();
        self.parse_expected_matching_brackets(K::OpenParenToken, K::CloseParenToken, parsed, open);
        let statement = self.parse_statement();
        let node = self
            .factory
            .new_while_statement(Some(expression), Some(statement));
        self.finish_statement(node, pos, jsdoc)
    }
    /// port: tsc/internal/parser/parser.go:Parser.parseForOrForInOrForOfStatement
    pub(crate) fn parse_for_or_for_in_or_for_of_statement(&mut self) -> NodeId {
        let pos = self.node_pos();
        let jsdoc = self.jsdoc_scanner_info();
        self.parse_expected(K::ForKeyword);
        let await_token = self.parse_optional_token(K::AwaitKeyword);
        self.parse_expected(K::OpenParenToken);
        let initializer = if self.token == K::SemicolonToken { None }
            else if matches!(self.token, K::VarKeyword | K::LetKeyword | K::ConstKeyword)
                || self.token == K::UsingKeyword && self.look_ahead(Self::next_token_is_binding_identifier_or_start_of_destructuring_on_same_line_disallow_of)
                || self.token == K::AwaitKeyword && self.look_ahead(Self::next_is_using_keyword_then_binding_identifier_or_start_of_object_destructuring_on_same_line) {
                Some(self.parse_variable_declaration_list(true))
            } else { Some(self.do_in_context(node_flags::DISALLOW_IN_CONTEXT, true, Self::parse_expression)) };
        let node = if await_token.is_some() && self.parse_expected(K::OfKeyword)
            || await_token.is_none() && self.parse_optional(K::OfKeyword)
        {
            let expression = self.do_in_context(
                node_flags::DISALLOW_IN_CONTEXT,
                false,
                Self::parse_assignment_expression_or_higher,
            );
            self.parse_expected(K::CloseParenToken);
            let statement = self.parse_statement();
            self.factory.new_for_in_or_of_statement(
                K::ForOfStatement.into(),
                await_token,
                initializer,
                Some(expression),
                Some(statement),
            )
        } else if self.parse_optional(K::InKeyword) {
            let expression = self.parse_expression_allow_in();
            self.parse_expected(K::CloseParenToken);
            let statement = self.parse_statement();
            self.factory.new_for_in_or_of_statement(
                K::ForInStatement.into(),
                None,
                initializer,
                Some(expression),
                Some(statement),
            )
        } else {
            self.parse_expected(K::SemicolonToken);
            let condition = if matches!(self.token, K::SemicolonToken | K::CloseParenToken) {
                None
            } else {
                Some(self.parse_expression_allow_in())
            };
            self.parse_expected(K::SemicolonToken);
            let incrementor = if self.token == K::CloseParenToken {
                None
            } else {
                Some(self.parse_expression_allow_in())
            };
            self.parse_expected(K::CloseParenToken);
            let statement = self.parse_statement();
            self.factory
                .new_for_statement(initializer, condition, incrementor, Some(statement))
        };
        self.finish_statement(node, pos, jsdoc)
    }
    /// port: tsc/internal/parser/parser.go:Parser.parseBreakStatement
    pub(crate) fn parse_break_statement(&mut self) -> NodeId {
        let pos = self.node_pos();
        let jsdoc = self.jsdoc_scanner_info();
        self.parse_expected(K::BreakKeyword);
        let label = self.parse_identifier_unless_at_semicolon();
        self.parse_semicolon();
        let node = self.factory.new_break_statement(label);
        self.finish_statement(node, pos, jsdoc)
    }
    /// port: tsc/internal/parser/parser.go:Parser.parseContinueStatement
    pub(crate) fn parse_continue_statement(&mut self) -> NodeId {
        let pos = self.node_pos();
        let jsdoc = self.jsdoc_scanner_info();
        self.parse_expected(K::ContinueKeyword);
        let label = self.parse_identifier_unless_at_semicolon();
        self.parse_semicolon();
        let node = self.factory.new_continue_statement(label);
        self.finish_statement(node, pos, jsdoc)
    }
    /// port: tsc/internal/parser/parser.go:Parser.parseIdentifierUnlessAtSemicolon
    pub(crate) fn parse_identifier_unless_at_semicolon(&mut self) -> Option<NodeId> {
        if self.can_parse_semicolon() {
            None
        } else {
            Some(self.parse_identifier())
        }
    }
    /// port: tsc/internal/parser/parser.go:Parser.parseReturnStatement
    pub(crate) fn parse_return_statement(&mut self) -> NodeId {
        let pos = self.node_pos();
        let jsdoc = self.jsdoc_scanner_info();
        self.parse_expected(K::ReturnKeyword);
        let expression = if self.can_parse_semicolon() {
            None
        } else {
            Some(self.parse_expression_allow_in())
        };
        self.parse_semicolon();
        let node = self.factory.new_return_statement(expression);
        self.finish_statement(node, pos, jsdoc)
    }
    /// port: tsc/internal/parser/parser.go:Parser.parseWithStatement
    pub(crate) fn parse_with_statement(&mut self) -> NodeId {
        let pos = self.node_pos();
        let jsdoc = self.jsdoc_scanner_info();
        self.parse_expected(K::WithKeyword);
        let open = self.scanner.token_start();
        let parsed = self.parse_expected(K::OpenParenToken);
        let expression = self.parse_expression_allow_in();
        self.parse_expected_matching_brackets(K::OpenParenToken, K::CloseParenToken, parsed, open);
        let statement =
            self.do_in_context(node_flags::IN_WITH_STATEMENT, true, Self::parse_statement);
        let node = self
            .factory
            .new_with_statement(Some(expression), Some(statement));
        self.finish_statement(node, pos, jsdoc)
    }
    /// port: tsc/internal/parser/parser.go:Parser.parseCaseClause
    pub(crate) fn parse_case_clause(&mut self) -> NodeId {
        let pos = self.node_pos();
        let jsdoc = self.jsdoc_scanner_info();
        self.parse_expected(K::CaseKeyword);
        let expression = self.parse_expression_allow_in();
        self.parse_expected(K::ColonToken);
        let statements = self.parse_list(
            ParsingContext::SwitchClauseStatements,
            Self::parse_statement,
        );
        let node = self.factory.new_case_or_default_clause(
            K::CaseClause.into(),
            Some(expression),
            Some(statements),
        );
        self.finish_statement(node, pos, jsdoc)
    }
    /// port: tsc/internal/parser/parser.go:Parser.parseDefaultClause
    pub(crate) fn parse_default_clause(&mut self) -> NodeId {
        let pos = self.node_pos();
        let jsdoc = self.jsdoc_scanner_info();
        self.parse_expected(K::DefaultKeyword);
        self.parse_expected(K::ColonToken);
        let statements = self.parse_list(
            ParsingContext::SwitchClauseStatements,
            Self::parse_statement,
        );
        let node = self.factory.new_case_or_default_clause(
            K::DefaultClause.into(),
            None,
            Some(statements),
        );
        self.finish_statement(node, pos, jsdoc)
    }
    /// port: tsc/internal/parser/parser.go:Parser.parseCaseOrDefaultClause
    pub(crate) fn parse_case_or_default_clause(&mut self) -> NodeId {
        if self.token == K::CaseKeyword {
            self.parse_case_clause()
        } else {
            self.parse_default_clause()
        }
    }
    /// port: tsc/internal/parser/parser.go:Parser.parseCaseBlock
    pub(crate) fn parse_case_block(&mut self) -> NodeId {
        let pos = self.node_pos();
        let jsdoc = self.jsdoc_scanner_info();
        self.parse_expected(K::OpenBraceToken);
        let clauses = self.parse_list(
            ParsingContext::SwitchClauses,
            Self::parse_case_or_default_clause,
        );
        self.parse_expected(K::CloseBraceToken);
        let node = self.factory.new_case_block(Some(clauses));
        self.finish_statement(node, pos, jsdoc)
    }
    /// port: tsc/internal/parser/parser.go:Parser.parseSwitchStatement
    pub(crate) fn parse_switch_statement(&mut self) -> NodeId {
        let pos = self.node_pos();
        let jsdoc = self.jsdoc_scanner_info();
        self.parse_expected(K::SwitchKeyword);
        self.parse_expected(K::OpenParenToken);
        let expression = self.parse_expression_allow_in();
        self.parse_expected(K::CloseParenToken);
        let block = self.parse_case_block();
        let node = self
            .factory
            .new_switch_statement(Some(expression), Some(block));
        self.finish_statement(node, pos, jsdoc)
    }
    /// port: tsc/internal/parser/parser.go:Parser.parseThrowStatement
    pub(crate) fn parse_throw_statement(&mut self) -> NodeId {
        let pos = self.node_pos();
        let jsdoc = self.jsdoc_scanner_info();
        self.parse_expected(K::ThrowKeyword);
        let expression = if self.has_preceding_line_break() {
            self.create_missing_identifier()
        } else {
            self.parse_expression_allow_in()
        };
        if !self.try_parse_semicolon() {
            self.parse_error_for_missing_semicolon_after(expression);
        }
        let node = self.factory.new_throw_statement(Some(expression));
        self.finish_statement(node, pos, jsdoc)
    }
    /// port: tsc/internal/parser/parser.go:Parser.parseTryStatement
    pub(crate) fn parse_try_statement(&mut self) -> NodeId {
        let pos = self.node_pos();
        let jsdoc = self.jsdoc_scanner_info();
        self.parse_expected(K::TryKeyword);
        let try_block = self.parse_block(false, None);
        let catch = if self.token == K::CatchKeyword {
            Some(self.parse_catch_clause())
        } else {
            None
        };
        let finally = if catch.is_none() || self.token == K::FinallyKeyword {
            self.parse_expected_with_diagnostic(
                K::FinallyKeyword,
                Some(diag::X_catch_or_finally_expected),
                true,
            );
            Some(self.parse_block(false, None))
        } else {
            None
        };
        let node = self
            .factory
            .new_try_statement(Some(try_block), catch, finally);
        self.finish_statement(node, pos, jsdoc)
    }
    /// port: tsc/internal/parser/parser.go:Parser.parseCatchClause
    pub(crate) fn parse_catch_clause(&mut self) -> NodeId {
        let pos = self.node_pos();
        self.parse_expected(K::CatchKeyword);
        let variable = if self.parse_optional(K::OpenParenToken) {
            let variable = self.parse_variable_declaration();
            self.parse_expected(K::CloseParenToken);
            Some(variable)
        } else {
            None
        };
        let block = self.parse_block(false, None);
        let node = self.factory.new_catch_clause(variable, Some(block));
        self.finish_node(node, pos)
    }
    /// port: tsc/internal/parser/parser.go:Parser.parseDebuggerStatement
    pub(crate) fn parse_debugger_statement(&mut self) -> NodeId {
        let pos = self.node_pos();
        let jsdoc = self.jsdoc_scanner_info();
        self.parse_expected(K::DebuggerKeyword);
        self.parse_semicolon();
        let node = self.factory.new_debugger_statement();
        self.finish_statement(node, pos, jsdoc)
    }
    /// port: tsc/internal/parser/parser.go:Parser.parseExpressionOrLabeledStatement
    pub(crate) fn parse_expression_or_labeled_statement(&mut self) -> NodeId {
        let pos = self.node_pos();
        let mut jsdoc = self.jsdoc_scanner_info();
        let paren = self.token == K::OpenParenToken;
        let expression = self.parse_expression();
        if self.factory.node(expression).kind() == K::Identifier
            && self.parse_optional(K::ColonToken)
        {
            let statement = self.parse_statement();
            let node = self
                .factory
                .new_labeled_statement(Some(expression), Some(statement));
            return self.finish_statement(node, pos, jsdoc);
        }
        if !self.try_parse_semicolon() {
            self.parse_error_for_missing_semicolon_after(expression);
        }
        let node = self.factory.new_expression_statement(Some(expression));
        if paren {
            jsdoc &= !1;
        }
        self.finish_statement(node, pos, jsdoc)
    }
}
