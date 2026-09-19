use crate::tokens::token_is_identifier_or_keyword;
use crate::{Parser, ParserFactory};
use ts_ast::{modifier_flags, SyntaxKind as K};

impl<F: ParserFactory> Parser<'_, F> {
    /// port: tsc/internal/parser/parser.go:Parser.scanTypeMemberStart
    pub(crate) fn scan_type_member_start(&mut self) -> bool {
        if matches!(
            self.token,
            K::OpenParenToken | K::LessThanToken | K::GetKeyword | K::SetKeyword
        ) {
            return true;
        }
        let mut identifier = false;
        while ts_ast::is_modifier_kind(self.token.into()) {
            identifier = true;
            self.next_token();
        }
        if self.token == K::OpenBracketToken {
            return true;
        }
        if self.is_literal_property_name() {
            identifier = true;
            self.next_token();
        }
        identifier
            && (matches!(
                self.token,
                K::OpenParenToken
                    | K::LessThanToken
                    | K::QuestionToken
                    | K::ColonToken
                    | K::CommaToken
            ) || self.can_parse_semicolon())
    }
    /// port: tsc/internal/parser/parser.go:Parser.scanClassMemberStart
    pub(crate) fn scan_class_member_start(&mut self) -> bool {
        let mut identifier = K::Unknown;
        if self.token == K::AtToken {
            return true;
        }
        while ts_ast::is_modifier_kind(self.token.into()) {
            identifier = self.token;
            if is_class_member_modifier(identifier) {
                return true;
            }
            self.next_token();
        }
        if self.token == K::AsteriskToken {
            return true;
        }
        if self.is_literal_property_name() {
            identifier = self.token;
            self.next_token();
        }
        if self.token == K::OpenBracketToken {
            return true;
        }
        if identifier != K::Unknown {
            if !ts_ast::is_keyword_kind(identifier.into())
                || matches!(identifier, K::SetKeyword | K::GetKeyword)
            {
                return true;
            }
            return matches!(
                self.token,
                K::OpenParenToken
                    | K::LessThanToken
                    | K::ExclamationToken
                    | K::ColonToken
                    | K::EqualsToken
                    | K::QuestionToken
            ) || self.can_parse_semicolon();
        }
        false
    }
    /// port: tsc/internal/parser/parser.go:Parser.isStartOfStatement
    pub(crate) fn is_start_of_statement(&mut self) -> bool {
        match self.token {
            K::AtToken
            | K::SemicolonToken
            | K::OpenBraceToken
            | K::VarKeyword
            | K::LetKeyword
            | K::UsingKeyword
            | K::FunctionKeyword
            | K::ClassKeyword
            | K::EnumKeyword
            | K::IfKeyword
            | K::DoKeyword
            | K::WhileKeyword
            | K::ForKeyword
            | K::ContinueKeyword
            | K::BreakKeyword
            | K::ReturnKeyword
            | K::WithKeyword
            | K::SwitchKeyword
            | K::ThrowKeyword
            | K::TryKeyword
            | K::DebuggerKeyword
            | K::CatchKeyword
            | K::FinallyKeyword
            | K::AsyncKeyword
            | K::DeclareKeyword
            | K::InterfaceKeyword
            | K::ModuleKeyword
            | K::NamespaceKeyword
            | K::TypeKeyword
            | K::GlobalKeyword
            | K::DeferKeyword => true,
            K::ImportKeyword => {
                self.is_start_of_declaration()
                    || self.is_next_token_open_paren_or_less_than_or_dot()
            }
            K::ConstKeyword | K::ExportKeyword => self.is_start_of_declaration(),
            K::AccessorKeyword
            | K::PublicKeyword
            | K::PrivateKeyword
            | K::ProtectedKeyword
            | K::StaticKeyword
            | K::ReadonlyKeyword => {
                self.is_start_of_declaration()
                    || !self.look_ahead(Self::next_token_is_identifier_or_keyword_on_same_line)
            }
            _ => self.is_start_of_expression(),
        }
    }
    /// port: tsc/internal/parser/parser.go:Parser.isStartOfDeclaration
    pub(crate) fn is_start_of_declaration(&mut self) -> bool {
        self.look_ahead(Self::scan_start_of_declaration)
    }
    /// port: tsc/internal/parser/parser.go:Parser.scanStartOfDeclaration
    pub(crate) fn scan_start_of_declaration(&mut self) -> bool {
        loop {
            match self.token {
                K::VarKeyword
                | K::LetKeyword
                | K::ConstKeyword
                | K::FunctionKeyword
                | K::ClassKeyword
                | K::EnumKeyword => return true,
                K::UsingKeyword => return self.is_using_declaration(),
                K::AwaitKeyword => return self.is_await_using_declaration(),
                K::InterfaceKeyword | K::TypeKeyword | K::DeferKeyword => {
                    return self.next_token_is_identifier_on_same_line();
                }
                K::ModuleKeyword | K::NamespaceKeyword => {
                    return self.next_token_is_identifier_or_string_literal_on_same_line();
                }
                K::AbstractKeyword
                | K::AccessorKeyword
                | K::AsyncKeyword
                | K::DeclareKeyword
                | K::PrivateKeyword
                | K::ProtectedKeyword
                | K::PublicKeyword
                | K::ReadonlyKeyword => {
                    let previous = self.token;
                    self.next_token();
                    if self.has_preceding_line_break() {
                        return false;
                    }
                    if previous == K::DeclareKeyword && self.token == K::TypeKeyword {
                        return true;
                    }
                }
                K::GlobalKeyword => {
                    self.next_token();
                    return matches!(
                        self.token,
                        K::OpenBraceToken | K::Identifier | K::ExportKeyword
                    );
                }
                K::ImportKeyword => {
                    self.next_token();
                    return matches!(
                        self.token,
                        K::DeferKeyword | K::StringLiteral | K::AsteriskToken | K::OpenBraceToken
                    ) || token_is_identifier_or_keyword(self.token);
                }
                K::ExportKeyword => {
                    self.next_token();
                    if matches!(
                        self.token,
                        K::EqualsToken
                            | K::AsteriskToken
                            | K::OpenBraceToken
                            | K::DefaultKeyword
                            | K::AsKeyword
                            | K::AtToken
                    ) {
                        return true;
                    }
                    if self.token == K::TypeKeyword {
                        self.next_token();
                        return matches!(self.token, K::AsteriskToken | K::OpenBraceToken)
                            || self.is_identifier() && !self.has_preceding_line_break();
                    }
                }
                K::StaticKeyword => {
                    self.next_token();
                }
                _ => return false,
            }
        }
    }
    /// port: tsc/internal/parser/parser.go:Parser.isStartOfType
    pub(crate) fn is_start_of_type(&mut self, parameter: bool) -> bool {
        crate::recursion::guarded(|| match self.token {
            K::AnyKeyword
            | K::UnknownKeyword
            | K::StringKeyword
            | K::NumberKeyword
            | K::BigIntKeyword
            | K::BooleanKeyword
            | K::ReadonlyKeyword
            | K::SymbolKeyword
            | K::UniqueKeyword
            | K::VoidKeyword
            | K::UndefinedKeyword
            | K::NullKeyword
            | K::ThisKeyword
            | K::TypeOfKeyword
            | K::NeverKeyword
            | K::OpenBraceToken
            | K::OpenBracketToken
            | K::LessThanToken
            | K::BarToken
            | K::AmpersandToken
            | K::NewKeyword
            | K::StringLiteral
            | K::NumericLiteral
            | K::BigIntLiteral
            | K::TrueKeyword
            | K::FalseKeyword
            | K::ObjectKeyword
            | K::AsteriskToken
            | K::QuestionToken
            | K::ExclamationToken
            | K::DotDotDotToken
            | K::InferKeyword
            | K::ImportKeyword
            | K::AssertsKeyword
            | K::NoSubstitutionTemplateLiteral
            | K::TemplateHead => true,
            K::FunctionKeyword => !parameter,
            K::MinusToken => {
                !parameter && self.look_ahead(Self::next_token_is_numeric_or_big_int_literal)
            }
            K::OpenParenToken => {
                !parameter && self.look_ahead(Self::next_is_parenthesized_or_function_type)
            }
            _ => self.is_identifier(),
        })
    }
    /// port: tsc/internal/parser/parser.go:Parser.nextTokenIsNumericOrBigIntLiteral
    pub(crate) fn next_token_is_numeric_or_big_int_literal(&mut self) -> bool {
        matches!(self.next_token(), K::NumericLiteral | K::BigIntLiteral)
    }
    /// port: tsc/internal/parser/parser.go:Parser.nextIsParenthesizedOrFunctionType
    pub(crate) fn next_is_parenthesized_or_function_type(&mut self) -> bool {
        self.next_token();
        self.token == K::CloseParenToken
            || self.is_start_of_parameter(false)
            || self.is_start_of_type(false)
    }
    /// port: tsc/internal/parser/parser.go:Parser.isStartOfParameter
    pub(crate) fn is_start_of_parameter(&mut self, jsdoc: bool) -> bool {
        self.token == K::DotDotDotToken
            || self.is_binding_identifier_or_private_identifier_or_pattern()
            || ts_ast::is_modifier_kind(self.token.into())
            || self.token == K::AtToken
            || self.is_start_of_type(!jsdoc)
    }
    /// port: tsc/internal/parser/parser.go:Parser.isBindingIdentifierOrPrivateIdentifierOrPattern
    pub(crate) fn is_binding_identifier_or_private_identifier_or_pattern(&self) -> bool {
        matches!(
            self.token,
            K::OpenBraceToken | K::OpenBracketToken | K::PrivateIdentifier
        ) || self.is_binding_identifier()
    }
    /// port: tsc/internal/parser/parser.go:Parser.isNextTokenOpenParenOrLessThanOrDot
    pub(crate) fn is_next_token_open_paren_or_less_than_or_dot(&mut self) -> bool {
        self.look_ahead(Self::next_token_is_open_paren_or_less_than_or_dot)
    }
    /// port: tsc/internal/parser/parser.go:Parser.nextTokenIsOpenParenOrLessThanOrDot
    pub(crate) fn next_token_is_open_paren_or_less_than_or_dot(&mut self) -> bool {
        matches!(
            self.next_token(),
            K::OpenParenToken | K::LessThanToken | K::DotToken
        )
    }
    /// port: tsc/internal/parser/parser.go:Parser.nextTokenIsIdentifierOnSameLine
    pub(crate) fn next_token_is_identifier_on_same_line(&mut self) -> bool {
        self.next_token();
        self.is_identifier() && !self.has_preceding_line_break()
    }
    /// port: tsc/internal/parser/parser.go:Parser.nextTokenIsIdentifierOrStringLiteralOnSameLine
    pub(crate) fn next_token_is_identifier_or_string_literal_on_same_line(&mut self) -> bool {
        self.next_token();
        (self.is_identifier() || self.token == K::StringLiteral) && !self.has_preceding_line_break()
    }
    /// port: tsc/internal/parser/parser.go:Parser.nextTokenIsIdentifierOrKeyword
    pub(crate) fn next_token_is_identifier_or_keyword(&mut self) -> bool {
        token_is_identifier_or_keyword(self.next_token())
    }
    /// port: tsc/internal/parser/parser.go:Parser.nextTokenIsIdentifierOrKeywordOnSameLine
    pub(crate) fn next_token_is_identifier_or_keyword_on_same_line(&mut self) -> bool {
        self.next_token_is_identifier_or_keyword() && !self.has_preceding_line_break()
    }
    /// port: tsc/internal/parser/parser.go:Parser.isValidHeritageClauseObjectLiteral
    pub(crate) fn is_valid_heritage_clause_object_literal(&mut self) -> bool {
        self.look_ahead(Self::next_is_valid_heritage_clause_object_literal)
    }
    /// port: tsc/internal/parser/parser.go:Parser.nextIsValidHeritageClauseObjectLiteral
    pub(crate) fn next_is_valid_heritage_clause_object_literal(&mut self) -> bool {
        if self.next_token() == K::CloseBraceToken {
            return matches!(
                self.next_token(),
                K::CommaToken | K::OpenBraceToken | K::ExtendsKeyword | K::ImplementsKeyword
            );
        }
        true
    }
    /// port: tsc/internal/parser/parser.go:Parser.isHeritageClause
    pub(crate) fn is_heritage_clause(&self) -> bool {
        matches!(self.token, K::ExtendsKeyword | K::ImplementsKeyword)
    }
    /// port: tsc/internal/parser/parser.go:Parser.isHeritageClauseExtendsOrImplementsKeyword
    pub(crate) fn is_heritage_clause_extends_or_implements_keyword(&mut self) -> bool {
        self.is_heritage_clause() && self.look_ahead(Self::next_is_start_of_expression)
    }
    /// port: tsc/internal/parser/parser.go:Parser.nextIsStartOfExpression
    pub(crate) fn next_is_start_of_expression(&mut self) -> bool {
        self.next_token();
        self.is_start_of_expression()
    }
    /// port: tsc/internal/parser/parser.go:Parser.isUsingDeclaration
    pub(crate) fn is_using_declaration(&mut self) -> bool {
        self.look_ahead(|p| {
            p.next_token_is_binding_identifier_or_start_of_destructuring_on_same_line(false)
        })
    }
    /// port: tsc/internal/parser/parser.go:Parser.nextTokenIsEqualsOrSemicolonOrColonToken
    pub(crate) fn next_token_is_equals_or_semicolon_or_colon_token(&mut self) -> bool {
        matches!(
            self.next_token(),
            K::EqualsToken | K::SemicolonToken | K::ColonToken
        )
    }
    /// port: tsc/internal/parser/parser.go:Parser.nextTokenIsBindingIdentifierOrStartOfDestructuringOnSameLine
    pub(crate) fn next_token_is_binding_identifier_or_start_of_destructuring_on_same_line(
        &mut self,
        disallow_of: bool,
    ) -> bool {
        self.next_token();
        if disallow_of && self.token == K::OfKeyword {
            return self.look_ahead(Self::next_token_is_equals_or_semicolon_or_colon_token);
        }
        (self.is_binding_identifier() || self.token == K::OpenBraceToken)
            && !self.has_preceding_line_break()
    }
    /// port: tsc/internal/parser/parser.go:Parser.nextTokenIsBindingIdentifierOrStartOfDestructuringOnSameLineDisallowOf
    pub(crate) fn next_token_is_binding_identifier_or_start_of_destructuring_on_same_line_disallow_of(
        &mut self,
    ) -> bool {
        self.next_token_is_binding_identifier_or_start_of_destructuring_on_same_line(true)
    }
    /// port: tsc/internal/parser/parser.go:Parser.isAwaitUsingDeclaration
    pub(crate) fn is_await_using_declaration(&mut self) -> bool {
        self.look_ahead(Self::next_is_using_keyword_then_binding_identifier_or_start_of_object_destructuring_on_same_line)
    }
    /// port: tsc/internal/parser/parser.go:Parser.nextIsUsingKeywordThenBindingIdentifierOrStartOfObjectDestructuringOnSameLine
    pub(crate) fn next_is_using_keyword_then_binding_identifier_or_start_of_object_destructuring_on_same_line(
        &mut self,
    ) -> bool {
        self.next_token() == K::UsingKeyword
            && self.next_token_is_binding_identifier_or_start_of_destructuring_on_same_line(false)
    }
    /// port: tsc/internal/parser/parser.go:Parser.nextTokenIsTokenStringLiteral
    pub(crate) fn next_token_is_token_string_literal(&mut self) -> bool {
        self.next_token() == K::StringLiteral
    }
}

/// port: tsc/internal/ast/utilities.go:IsClassMemberModifier
pub(crate) fn is_class_member_modifier(kind: K) -> bool {
    crate::state::modifier_to_flag(kind.into()) & modifier_flags::PARAMETER_PROPERTY_MODIFIER != 0
        || matches!(
            kind,
            K::StaticKeyword | K::OverrideKeyword | K::AccessorKeyword
        )
}
