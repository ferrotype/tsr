use crate::tokens::token_is_identifier_or_keyword;
use crate::{Parser, ParserFactory, ParsingContext};
use ts_ast::{
    node_flags, FactoryMethods, JsString, NodeDataRead, NodeId, NodeListId, SyntaxKind as K,
};
use ts_core::TextRange;
use ts_diagnostics as diag;

impl<F: ParserFactory> Parser<'_, F> {
    /// port: tsc/internal/parser/parser.go:Parser.parseType
    pub(crate) fn parse_type(&mut self) -> NodeId {
        crate::recursion::guarded(|| self.parse_type_worker())
    }

    fn parse_type_worker(&mut self) -> NodeId {
        let saved = self.context_flags;
        self.set_context_flags(node_flags::TYPE_EXCLUDES_FLAGS, false);
        let mut node;
        if self.is_start_of_function_type_or_constructor_type() {
            node = self.parse_function_or_constructor_type();
        } else {
            let pos = self.node_pos();
            node = self.parse_union_type_or_higher();
            if !self.in_disallow_conditional_types_context()
                && !self.has_preceding_line_break()
                && self.parse_optional(K::ExtendsKeyword)
            {
                let extends = self.do_in_context(
                    node_flags::DISALLOW_CONDITIONAL_TYPES_CONTEXT,
                    true,
                    Self::parse_type,
                );
                self.parse_expected(K::QuestionToken);
                let true_type = self.do_in_context(
                    node_flags::DISALLOW_CONDITIONAL_TYPES_CONTEXT,
                    false,
                    Self::parse_type,
                );
                self.parse_expected(K::ColonToken);
                let false_type = self.do_in_context(
                    node_flags::DISALLOW_CONDITIONAL_TYPES_CONTEXT,
                    false,
                    Self::parse_type,
                );
                node = self.factory.new_conditional_type_node(
                    Some(node),
                    Some(extends),
                    Some(true_type),
                    Some(false_type),
                );
                self.finish_node(node, pos);
            }
        }
        self.context_flags = saved;
        node
    }

    /// port: tsc/internal/parser/parser.go:Parser.parseUnionTypeOrHigher
    pub(crate) fn parse_union_type_or_higher(&mut self) -> NodeId {
        self.parse_union_or_intersection_type(K::BarToken, Self::parse_intersection_type_or_higher)
    }
    /// port: tsc/internal/parser/parser.go:Parser.parseIntersectionTypeOrHigher
    pub(crate) fn parse_intersection_type_or_higher(&mut self) -> NodeId {
        self.parse_union_or_intersection_type(
            K::AmpersandToken,
            Self::parse_type_operator_or_higher,
        )
    }
    /// port: tsc/internal/parser/parser.go:Parser.parseUnionOrIntersectionType
    pub(crate) fn parse_union_or_intersection_type(
        &mut self,
        operator: K,
        parse: fn(&mut Self) -> NodeId,
    ) -> NodeId {
        let pos = self.node_pos();
        let union = operator == K::BarToken;
        let leading = self.parse_optional(operator);
        let mut node = if leading {
            self.parse_function_or_constructor_type_to_error(union, parse)
        } else {
            parse(self)
        };
        if self.token == operator || leading {
            let mut types = Vec::with_capacity(8);
            types.push(node);
            while self.parse_optional(operator) {
                types.push(self.parse_function_or_constructor_type_to_error(union, parse));
            }
            let list = self.new_node_list(TextRange::new(pos, self.node_pos()), types);
            node = self.create_union_or_intersection_type_node(operator, list);
            self.finish_node(node, pos);
        }
        node
    }
    /// port: tsc/internal/parser/parser.go:Parser.createUnionOrIntersectionTypeNode
    pub(crate) fn create_union_or_intersection_type_node(
        &mut self,
        operator: K,
        types: NodeListId,
    ) -> NodeId {
        match operator {
            K::BarToken => self.factory.new_union_type_node(Some(types)),
            K::AmpersandToken => self.factory.new_intersection_type_node(Some(types)),
            _ => panic!("Unhandled case in createUnionOrIntersectionType"),
        }
    }
    /// port: tsc/internal/parser/parser.go:Parser.parseTypeOperatorOrHigher
    pub(crate) fn parse_type_operator_or_higher(&mut self) -> NodeId {
        crate::recursion::guarded(|| match self.token {
            K::KeyOfKeyword | K::UniqueKeyword | K::ReadonlyKeyword => {
                self.parse_type_operator(self.token)
            }
            K::InferKeyword => self.parse_infer_type(),
            _ => self.do_in_context(
                node_flags::DISALLOW_CONDITIONAL_TYPES_CONTEXT,
                false,
                Self::parse_postfix_type_or_higher,
            ),
        })
    }
    /// port: tsc/internal/parser/parser.go:Parser.parseTypeOperator
    pub(crate) fn parse_type_operator(&mut self, operator: K) -> NodeId {
        let pos = self.node_pos();
        self.parse_expected(operator);
        let operand = self.parse_type_operator_or_higher();
        let node = self
            .factory
            .new_type_operator_node(operator.into(), Some(operand));
        self.finish_node(node, pos)
    }
    /// port: tsc/internal/parser/parser.go:Parser.parseInferType
    pub(crate) fn parse_infer_type(&mut self) -> NodeId {
        let pos = self.node_pos();
        self.parse_expected(K::InferKeyword);
        let parameter = self.parse_type_parameter_of_infer_type();
        let node = self.factory.new_infer_type_node(Some(parameter));
        self.finish_node(node, pos)
    }
    /// port: tsc/internal/parser/parser.go:Parser.parseTypeParameterOfInferType
    pub(crate) fn parse_type_parameter_of_infer_type(&mut self) -> NodeId {
        let pos = self.node_pos();
        let name = self.parse_identifier();
        let constraint = self.try_parse_constraint_of_infer_type();
        let node =
            self.factory
                .new_type_parameter_declaration(None, Some(name), constraint, None, None);
        self.finish_node(node, pos)
    }
    /// port: tsc/internal/parser/parser.go:Parser.tryParseConstraintOfInferType
    pub(crate) fn try_parse_constraint_of_infer_type(&mut self) -> Option<NodeId> {
        let state = self.mark();
        if self.parse_optional(K::ExtendsKeyword) {
            let constraint = self.do_in_context(
                node_flags::DISALLOW_CONDITIONAL_TYPES_CONTEXT,
                true,
                Self::parse_type,
            );
            if self.in_disallow_conditional_types_context() || self.token != K::QuestionToken {
                self.commit(state);
                return Some(constraint);
            }
        }
        self.rewind(state);
        None
    }
    /// port: tsc/internal/parser/parser.go:Parser.parsePostfixTypeOrHigher
    pub(crate) fn parse_postfix_type_or_higher(&mut self) -> NodeId {
        let pos = self.node_pos();
        let mut node = self.parse_non_array_type();
        while !self.has_preceding_line_break() {
            node = match self.token {
                K::ExclamationToken => {
                    self.next_token();
                    self.factory.new_js_doc_non_nullable_type(Some(node))
                }
                K::QuestionToken => {
                    if self.look_ahead(Self::next_is_start_of_type) {
                        return node;
                    }
                    self.next_token();
                    self.factory.new_js_doc_nullable_type(Some(node))
                }
                K::OpenBracketToken => {
                    self.parse_expected(K::OpenBracketToken);
                    if self.is_start_of_type(false) {
                        let index_type = self.parse_type();
                        self.parse_expected(K::CloseBracketToken);
                        self.factory
                            .new_indexed_access_type_node(Some(node), Some(index_type))
                    } else {
                        self.parse_expected(K::CloseBracketToken);
                        self.factory.new_array_type_node(Some(node))
                    }
                }
                _ => return node,
            };
            self.finish_node(node, pos);
        }
        node
    }
    /// port: tsc/internal/parser/parser.go:Parser.nextIsStartOfType
    pub(crate) fn next_is_start_of_type(&mut self) -> bool {
        self.next_token();
        self.is_start_of_type(false)
    }

    /// port: tsc/internal/parser/parser.go:Parser.parseNonArrayType
    pub(crate) fn parse_non_array_type(&mut self) -> NodeId {
        match self.token {
            K::AnyKeyword
            | K::UnknownKeyword
            | K::StringKeyword
            | K::NumberKeyword
            | K::BigIntKeyword
            | K::SymbolKeyword
            | K::BooleanKeyword
            | K::UndefinedKeyword
            | K::NeverKeyword
            | K::ObjectKeyword => {
                let state = self.mark();
                let node = self.parse_keyword_type_node();
                if self.token != K::DotToken {
                    self.commit(state);
                    return node;
                }
                self.rewind(state);
                self.parse_type_reference()
            }
            K::AsteriskEqualsToken => {
                self.scan_operation(ts_scanner::Scanner::rescan_asterisk_equals_token);
                self.parse_js_doc_all_type()
            }
            K::AsteriskToken => self.parse_js_doc_all_type(),
            K::QuestionQuestionToken => {
                self.scan_operation(ts_scanner::Scanner::rescan_question_token);
                self.parse_js_doc_nullable_type()
            }
            K::QuestionToken => self.parse_js_doc_nullable_type(),
            K::ExclamationToken => self.parse_js_doc_non_nullable_type(),
            K::NoSubstitutionTemplateLiteral
            | K::StringLiteral
            | K::NumericLiteral
            | K::BigIntLiteral
            | K::TrueKeyword
            | K::FalseKeyword
            | K::NullKeyword => self.parse_literal_type_node(false),
            K::MinusToken => {
                if self.look_ahead(Self::next_token_is_numeric_or_big_int_literal) {
                    self.parse_literal_type_node(true)
                } else {
                    self.parse_type_reference()
                }
            }
            K::VoidKeyword => self.parse_keyword_type_node(),
            K::ThisKeyword => {
                let this = self.parse_this_type_node();
                if self.token == K::IsKeyword && !self.has_preceding_line_break() {
                    self.parse_this_type_predicate(this)
                } else {
                    this
                }
            }
            K::TypeOfKeyword => {
                if self.look_ahead(Self::next_is_start_of_type_of_import_type) {
                    self.parse_import_type()
                } else {
                    self.parse_type_query()
                }
            }
            K::OpenBraceToken => {
                if self.look_ahead(Self::next_is_start_of_mapped_type) {
                    self.parse_mapped_type()
                } else {
                    self.parse_type_literal()
                }
            }
            K::OpenBracketToken => self.parse_tuple_type(),
            K::OpenParenToken => self.parse_parenthesized_type(),
            K::ImportKeyword => self.parse_import_type(),
            K::AssertsKeyword => {
                if self.look_ahead(Self::next_token_is_identifier_or_keyword_on_same_line) {
                    self.parse_asserts_type_predicate()
                } else {
                    self.parse_type_reference()
                }
            }
            K::TemplateHead => self.parse_template_type(),
            _ => self.parse_type_reference(),
        }
    }
    /// port: tsc/internal/parser/parser.go:Parser.parseKeywordTypeNode
    pub(crate) fn parse_keyword_type_node(&mut self) -> NodeId {
        let pos = self.node_pos();
        let node = self.factory.new_keyword_type_node(self.token.into());
        self.next_token();
        self.finish_node(node, pos)
    }
    /// port: tsc/internal/parser/parser.go:Parser.parseThisTypeNode
    pub(crate) fn parse_this_type_node(&mut self) -> NodeId {
        let pos = self.node_pos();
        self.next_token();
        let node = self.factory.new_this_type_node();
        self.finish_node(node, pos)
    }
    /// port: tsc/internal/parser/parser.go:Parser.parseThisTypePredicate
    pub(crate) fn parse_this_type_predicate(&mut self, lhs: NodeId) -> NodeId {
        self.next_token();
        let ty = self.parse_type();
        let node = self
            .factory
            .new_type_predicate_node(None, Some(lhs), Some(ty));
        let pos = i64::from(self.factory.node(lhs).pos());
        self.finish_node(node, pos)
    }
    /// port: tsc/internal/parser/parser.go:Parser.parseJSDocAllType
    pub(crate) fn parse_js_doc_all_type(&mut self) -> NodeId {
        let pos = self.node_pos();
        self.next_token();
        let node = self.factory.new_js_doc_all_type();
        self.finish_node(node, pos)
    }
    /// port: tsc/internal/parser/parser.go:Parser.parseJSDocNonNullableType
    pub(crate) fn parse_js_doc_non_nullable_type(&mut self) -> NodeId {
        let pos = self.node_pos();
        self.next_token();
        let ty = self.parse_type_operator_or_higher();
        let node = self.factory.new_js_doc_non_nullable_type(Some(ty));
        self.finish_node(node, pos)
    }
    /// port: tsc/internal/parser/parser.go:Parser.parseJSDocNullableType
    pub(crate) fn parse_js_doc_nullable_type(&mut self) -> NodeId {
        let pos = self.node_pos();
        self.next_token();
        let ty = self.parse_type_operator_or_higher();
        let node = self.factory.new_js_doc_nullable_type(Some(ty));
        self.finish_node(node, pos)
    }
    /// port: tsc/internal/parser/parser.go:Parser.parseJSDocType
    pub(crate) fn parse_js_doc_type(&mut self) -> NodeId {
        self.scanner.set_skip_jsdoc_leading_asterisks(true);
        let pos = self.node_pos();
        let rest = self.parse_optional(K::DotDotDotToken);
        let mut node = self.parse_type_or_type_predicate();
        self.scanner.set_skip_jsdoc_leading_asterisks(false);
        if rest {
            node = self.factory.new_js_doc_variadic_type(Some(node));
            self.finish_node(node, pos);
        }
        if self.token == K::EqualsToken {
            self.next_token();
            node = self.factory.new_js_doc_optional_type(Some(node));
            self.finish_node(node, pos);
        }
        node
    }
    /// port: tsc/internal/parser/parser.go:Parser.parseLiteralTypeNode
    pub(crate) fn parse_literal_type_node(&mut self, negative: bool) -> NodeId {
        let pos = self.node_pos();
        if negative {
            self.next_token();
        }
        let mut expression = if matches!(
            self.token,
            K::TrueKeyword | K::FalseKeyword | K::NullKeyword
        ) {
            self.parse_keyword_expression()
        } else {
            self.parse_literal_expression()
        };
        if negative {
            expression = self
                .factory
                .new_prefix_unary_expression(K::MinusToken.into(), Some(expression));
            self.finish_node(expression, pos);
        }
        let node = self.factory.new_literal_type_node(Some(expression));
        self.finish_node(node, pos)
    }
    /// port: tsc/internal/parser/parser.go:Parser.parseTypeReference
    pub(crate) fn parse_type_reference(&mut self) -> NodeId {
        let pos = self.node_pos();
        let name = self.parse_entity_name_of_type_reference();
        let arguments = self.parse_type_arguments_of_type_reference();
        let node = self.factory.new_type_reference_node(Some(name), arguments);
        self.finish_node(node, pos)
    }
    /// port: tsc/internal/parser/parser.go:Parser.parseEntityNameOfTypeReference
    pub(crate) fn parse_entity_name_of_type_reference(&mut self) -> NodeId {
        self.parse_entity_name(true, false, Some(diag::Type_expected))
    }
    /// port: tsc/internal/parser/parser.go:Parser.parseEntityName
    pub(crate) fn parse_entity_name(
        &mut self,
        allow_reserved: bool,
        allow_private: bool,
        message: Option<&'static diag::Message>,
    ) -> NodeId {
        let pos = self.node_pos();
        let mut entity = if allow_reserved {
            self.parse_identifier_name_with_diagnostic(message)
        } else {
            self.parse_identifier_with_diagnostic(message, None)
        };
        while self.parse_optional(K::DotToken) {
            if self.token == K::LessThanToken {
                break;
            }
            let right = self.parse_right_side_of_dot(allow_reserved, allow_private, true);
            entity = self.factory.new_qualified_name(Some(entity), Some(right));
            self.finish_node(entity, pos);
        }
        entity
    }
    /// port: tsc/internal/parser/parser.go:Parser.parseRightSideOfDot
    pub(crate) fn parse_right_side_of_dot(
        &mut self,
        allow_names: bool,
        allow_private: bool,
        allow_escape: bool,
    ) -> NodeId {
        if self.has_preceding_line_break()
            && token_is_identifier_or_keyword(self.token)
            && self.look_ahead(Self::next_token_is_identifier_or_keyword_on_same_line)
        {
            self.parse_error_at(
                self.node_pos(),
                self.node_pos(),
                diag::Identifier_expected,
                Vec::new(),
            );
            return self.create_missing_identifier();
        }
        if self.token == K::PrivateIdentifier {
            let node = self.parse_private_identifier();
            if allow_private {
                return node;
            }
            self.parse_error_at(
                self.node_pos(),
                self.node_pos(),
                diag::Identifier_expected,
                Vec::new(),
            );
            return self.create_missing_identifier();
        }
        if allow_names {
            return if allow_escape {
                self.parse_identifier_name()
            } else {
                self.parse_identifier_name_error_on_unicode_escape_sequence()
            };
        }
        let saved = self.statement_has_await_identifier;
        let id = self.parse_identifier();
        self.statement_has_await_identifier = saved;
        id
    }
    /// port: tsc/internal/parser/parser.go:Parser.parseTypeArgumentsOfTypeReference
    pub(crate) fn parse_type_arguments_of_type_reference(&mut self) -> Option<NodeListId> {
        if !self.has_preceding_line_break() && self.re_scan_less_than_token() == K::LessThanToken {
            self.parse_type_arguments()
        } else {
            None
        }
    }
    /// port: tsc/internal/parser/parser.go:Parser.parseTypeArguments
    pub(crate) fn parse_type_arguments(&mut self) -> Option<NodeListId> {
        if self.token == K::LessThanToken {
            self.parse_bracketed_list(
                ParsingContext::TypeArguments,
                Self::parse_type,
                K::LessThanToken,
                K::GreaterThanToken,
            )
        } else {
            None
        }
    }
    /// port: tsc/internal/parser/parser.go:Parser.nextIsStartOfTypeOfImportType
    pub(crate) fn next_is_start_of_type_of_import_type(&mut self) -> bool {
        self.next_token();
        self.token == K::ImportKeyword
    }

    /// port: tsc/internal/parser/parser.go:Parser.parseTypeQuery
    pub(crate) fn parse_type_query(&mut self) -> NodeId {
        let pos = self.node_pos();
        self.parse_expected(K::TypeOfKeyword);
        let name = self.parse_entity_name(true, true, None);
        let arguments = if self.has_preceding_line_break() {
            None
        } else {
            self.parse_type_arguments()
        };
        let node = self.factory.new_type_query_node(Some(name), arguments);
        self.finish_node(node, pos)
    }
    /// port: tsc/internal/parser/parser.go:Parser.nextIsStartOfMappedType
    pub(crate) fn next_is_start_of_mapped_type(&mut self) -> bool {
        self.next_token();
        if matches!(self.token, K::PlusToken | K::MinusToken) {
            return self.next_token() == K::ReadonlyKeyword;
        }
        if self.token == K::ReadonlyKeyword {
            self.next_token();
        }
        self.token == K::OpenBracketToken
            && self.next_token_is_identifier()
            && self.next_token() == K::InKeyword
    }
    /// port: tsc/internal/parser/parser.go:Parser.parseMappedType
    pub(crate) fn parse_mapped_type(&mut self) -> NodeId {
        let pos = self.node_pos();
        self.parse_expected(K::OpenBraceToken);
        let readonly = if matches!(
            self.token,
            K::ReadonlyKeyword | K::PlusToken | K::MinusToken
        ) {
            let token = self.parse_token_node();
            if self.factory.node(token).kind() != K::ReadonlyKeyword {
                self.parse_expected(K::ReadonlyKeyword);
            }
            Some(token)
        } else {
            None
        };
        self.parse_expected(K::OpenBracketToken);
        let parameter = self.parse_mapped_type_parameter();
        let name = if self.parse_optional(K::AsKeyword) {
            Some(self.parse_type())
        } else {
            None
        };
        self.parse_expected(K::CloseBracketToken);
        let question = if matches!(self.token, K::QuestionToken | K::PlusToken | K::MinusToken) {
            let token = self.parse_token_node();
            if self.factory.node(token).kind() != K::QuestionToken {
                self.parse_expected(K::QuestionToken);
            }
            Some(token)
        } else {
            None
        };
        let ty = self.parse_type_annotation();
        self.parse_semicolon();
        let members = self.parse_list(ParsingContext::TypeMembers, Self::parse_type_member);
        self.parse_expected(K::CloseBraceToken);
        let node = self.factory.new_mapped_type_node(
            readonly,
            Some(parameter),
            name,
            question,
            ty,
            Some(members),
        );
        self.finish_node(node, pos)
    }
    /// port: tsc/internal/parser/parser.go:Parser.parseMappedTypeParameter
    pub(crate) fn parse_mapped_type_parameter(&mut self) -> NodeId {
        let pos = self.node_pos();
        let name = self.parse_identifier_name();
        self.parse_expected(K::InKeyword);
        let ty = self.parse_type();
        let node =
            self.factory
                .new_type_parameter_declaration(None, Some(name), Some(ty), None, None);
        self.finish_node(node, pos)
    }
    /// port: tsc/internal/parser/parser.go:Parser.parseTypeParameters
    pub(crate) fn parse_type_parameters(&mut self) -> Option<NodeListId> {
        if self.token == K::LessThanToken {
            self.parse_bracketed_list(
                ParsingContext::TypeParameters,
                Self::parse_type_parameter,
                K::LessThanToken,
                K::GreaterThanToken,
            )
        } else {
            None
        }
    }
    /// port: tsc/internal/parser/parser.go:Parser.parseTypeParameter
    pub(crate) fn parse_type_parameter(&mut self) -> NodeId {
        let pos = self.node_pos();
        let modifiers = self.parse_modifiers_ex(false, true, false);
        let name = self.parse_identifier();
        let mut constraint = None;
        let mut expression = None;
        if self.parse_optional(K::ExtendsKeyword) {
            if self.is_start_of_type(false) || !self.is_start_of_expression() {
                constraint = Some(self.parse_type());
            } else {
                expression = Some(self.parse_unary_expression_or_higher());
            }
        }
        let default_type = if self.parse_optional(K::EqualsToken) {
            Some(self.parse_type())
        } else {
            None
        };
        let node = self.factory.new_type_parameter_declaration(
            modifiers,
            Some(name),
            constraint,
            expression,
            default_type,
        );
        self.finish_node(node, pos)
    }

    /// port: tsc/internal/parser/parser.go:Parser.parseImportType
    pub(crate) fn parse_import_type(&mut self) -> NodeId {
        self.source_flags |= node_flags::POSSIBLY_CONTAINS_DYNAMIC_IMPORT;
        let pos = self.node_pos();
        let is_type_of = self.parse_optional(K::TypeOfKeyword);
        self.parse_expected(K::ImportKeyword);
        self.parse_expected(K::OpenParenToken);
        let ty = self.parse_type();
        let mut attributes = None;
        if self.parse_optional(K::CommaToken) {
            let open = self.scanner.token_start();
            self.parse_expected(K::OpenBraceToken);
            let token = self.token;
            if matches!(token, K::WithKeyword | K::AssertKeyword) {
                if token == K::AssertKeyword {
                    self.parse_error_at_current_token(diag::Import_assertions_have_been_replaced_by_import_attributes_Use_with_instead_of_assert, Vec::new());
                }
                self.next_token();
            } else {
                self.parse_error_at_current_token(
                    diag::X_0_expected,
                    vec![JsString::from_bytes(&b"with"[..])],
                );
            }
            self.parse_expected(K::ColonToken);
            attributes = Some(self.parse_import_attributes(token, true));
            self.parse_optional(K::CommaToken);
            if !self.parse_expected(K::CloseBraceToken) {
                self.add_missing_brace_related(open);
            }
        }
        self.parse_expected(K::CloseParenToken);
        let qualifier = if self.parse_optional(K::DotToken) {
            Some(self.parse_entity_name_of_type_reference())
        } else {
            None
        };
        let arguments = self.parse_type_arguments_of_type_reference();
        let node = self.factory.new_import_type_node(
            is_type_of,
            Some(ty),
            attributes,
            qualifier,
            arguments,
        );
        self.finish_node(node, pos)
    }
    /// port: tsc/internal/parser/parser.go:Parser.parseImportAttribute
    pub(crate) fn parse_import_attribute(&mut self) -> NodeId {
        let pos = self.node_pos();
        let name = if token_is_identifier_or_keyword(self.token) {
            Some(self.parse_identifier_name())
        } else if self.token == K::StringLiteral {
            Some(self.parse_literal_expression())
        } else {
            None
        };
        if name.is_some() {
            self.parse_expected(K::ColonToken);
        } else {
            self.parse_error_at_current_token(
                diag::Identifier_or_string_literal_expected,
                Vec::new(),
            );
        }
        let value = self.parse_assignment_expression_or_higher();
        let node = self.factory.new_import_attribute(name, Some(value));
        self.finish_node(node, pos)
    }
    /// port: tsc/internal/parser/parser.go:Parser.parseImportAttributes
    pub(crate) fn parse_import_attributes(&mut self, token: K, skip_keyword: bool) -> NodeId {
        let pos = self.node_pos();
        if !skip_keyword {
            self.parse_expected(token);
        }
        let open = self.scanner.token_start();
        let mut multi_line = false;
        let elements;
        if self.parse_expected(K::OpenBraceToken) {
            multi_line = self.has_preceding_line_break();
            elements = self.parse_delimited_list(
                ParsingContext::ImportAttributes,
                Self::parse_import_attribute,
            );
            if !self.parse_expected(K::CloseBraceToken) {
                self.add_missing_brace_related(open);
            }
        } else {
            elements = Some(self.parse_empty_node_list());
        }
        let node = self
            .factory
            .new_import_attributes(token.into(), elements, multi_line);
        self.finish_node(node, pos)
    }
    fn add_missing_brace_related(&mut self, open: i64) {
        if let Some(last) = self.diagnostics.last_mut() {
            if last.code == diag::X_0_expected.code {
                last.related_information
                    .push(std::sync::Arc::new(ts_ast::Diagnostic::new(
                        None,
                        TextRange::new(open, open),
                        diag::The_parser_expected_to_find_a_1_to_match_the_0_token_here,
                        vec![
                            JsString::from_bytes(&b"{"[..]),
                            JsString::from_bytes(&b"}"[..]),
                        ],
                    )));
            }
        }
    }

    /// port: tsc/internal/parser/parser.go:Parser.parseTemplateType
    pub(crate) fn parse_template_type(&mut self) -> NodeId {
        let pos = self.node_pos();
        let head = self.parse_template_head(false);
        let spans = self.parse_template_type_spans();
        let node = self
            .factory
            .new_template_literal_type_node(Some(head), Some(spans));
        self.finish_node(node, pos)
    }
    /// port: tsc/internal/parser/parser.go:Parser.parseTemplateHead
    pub(crate) fn parse_template_head(&mut self, tagged: bool) -> NodeId {
        if !tagged && self.scanner.token_flags() & ts_ast::token_flags::IS_INVALID != 0 {
            self.re_scan_template_token(false);
        }
        let pos = self.node_pos();
        let text = self.token_value();
        let raw = self.get_template_literal_raw_text(2);
        let node = self
            .factory
            .new_template_head(text, raw, self.scanner.token_flags());
        self.next_token();
        self.finish_node(node, pos)
    }
    /// port: tsc/internal/parser/parser.go:Parser.getTemplateLiteralRawText
    pub(crate) fn get_template_literal_raw_text(&self, mut end_length: usize) -> JsString {
        let token = self.scanner.token_text();
        if self.scanner.token_flags() & ts_ast::token_flags::UNTERMINATED != 0 {
            end_length = 0;
        }
        let start = self.scanner.token_start() as usize + 1;
        // The scanner's checked token view establishes source provenance. Go's
        // [1:len-endLength] bounds remain observable on malformed scanner state.
        let bytes = &token[1..token.len() - end_length];
        self.source_owner
            .slice(start..start + bytes.len())
            .expect("template raw text is a checked source slice")
    }
    /// port: tsc/internal/parser/parser.go:Parser.parseTemplateTypeSpans
    pub(crate) fn parse_template_type_spans(&mut self) -> NodeListId {
        let pos = self.node_pos();
        let mut list = Vec::new();
        loop {
            let span = self.parse_template_type_span();
            list.push(span);
            let literal = match self.factory.node(span).data() {
                NodeDataRead::TemplateLiteralTypeSpan(data) => {
                    data.literal().expect("template span has literal")
                }
                _ => unreachable!("new template span payload"),
            };
            if self.factory.node(literal).kind() != K::TemplateMiddle {
                break;
            }
        }
        self.new_node_list(TextRange::new(pos, self.node_pos()), list)
    }
    /// port: tsc/internal/parser/parser.go:Parser.parseTemplateTypeSpan
    pub(crate) fn parse_template_type_span(&mut self) -> NodeId {
        let pos = self.node_pos();
        let ty = self.parse_type();
        let literal = self.parse_literal_of_template_span(false);
        let node = self
            .factory
            .new_template_literal_type_span(Some(ty), Some(literal));
        self.finish_node(node, pos)
    }
    /// port: tsc/internal/parser/parser.go:Parser.parseLiteralOfTemplateSpan
    pub(crate) fn parse_literal_of_template_span(&mut self, tagged: bool) -> NodeId {
        if self.token == K::CloseBraceToken {
            self.re_scan_template_token(tagged);
            return self.parse_template_middle_or_tail();
        }
        self.parse_error_at_current_token(
            diag::X_0_expected,
            vec![JsString::from_bytes(&b"}"[..])],
        );
        let node = self.factory.new_template_tail(
            JsString::default(),
            JsString::default(),
            ts_ast::token_flags::NONE,
        );
        self.finish_node(node, self.node_pos())
    }
    /// port: tsc/internal/parser/parser.go:Parser.parseTemplateMiddleOrTail
    pub(crate) fn parse_template_middle_or_tail(&mut self) -> NodeId {
        let pos = self.node_pos();
        let text = self.token_value();
        let node = if self.token == K::TemplateMiddle {
            let raw = self.get_template_literal_raw_text(2);
            self.factory
                .new_template_middle(text, raw, self.scanner.token_flags())
        } else {
            let raw = self.get_template_literal_raw_text(1);
            self.factory
                .new_template_tail(text, raw, self.scanner.token_flags())
        };
        self.next_token();
        self.finish_node(node, pos)
    }
}
