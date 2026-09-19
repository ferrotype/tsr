use crate::tokens::token_is_identifier_or_keyword;
use crate::{parse_flags, Parser, ParserFactory, ParsingContext};
use ts_ast::{
    node_flags, FactoryMethods, JsString, NodeDataRead, NodeId, NodeListId, SyntaxKind as K,
};
use ts_diagnostics as diag;

impl<F: ParserFactory> Parser<'_, F> {
    /// port: tsc/internal/parser/parser.go:Parser.parseParameters
    pub(crate) fn parse_parameters(&mut self, flags: u32) -> Option<NodeListId> {
        if self.parse_expected(K::OpenParenToken) {
            let parameters = self.parse_parameters_worker(flags, true);
            self.parse_expected(K::CloseParenToken);
            parameters
        } else {
            Some(self.create_missing_list())
        }
    }
    /// port: tsc/internal/parser/parser.go:Parser.parseParametersWorker
    pub(crate) fn parse_parameters_worker(
        &mut self,
        flags: u32,
        allow_ambiguity: bool,
    ) -> Option<NodeListId> {
        let outer_await = self.in_await_context();
        let saved = self.context_flags;
        self.set_context_flags(node_flags::YIELD_CONTEXT, flags & parse_flags::YIELD != 0);
        self.set_context_flags(node_flags::AWAIT_CONTEXT, flags & parse_flags::AWAIT != 0);
        let parameters = self.parse_delimited_list(ParsingContext::Parameters, |parser| {
            let parameter = parser.parse_parameter_ex(outer_await, allow_ambiguity);
            if flags & parse_flags::TYPE == 0 {
                if let Some(parameter) = parameter {
                    parser.check_js_syntax(parameter);
                }
            }
            parameter
        });
        self.context_flags = saved;
        parameters
    }
    /// port: tsc/internal/parser/parser.go:Parser.parseParameter
    pub(crate) fn parse_parameter(&mut self) -> Option<NodeId> {
        self.parse_parameter_ex(false, true)
    }
    /// port: tsc/internal/parser/parser.go:Parser.parseParameterEx
    pub(crate) fn parse_parameter_ex(
        &mut self,
        outer_await: bool,
        allow_ambiguity: bool,
    ) -> Option<NodeId> {
        let pos = self.node_pos();
        let jsdoc = self.jsdoc_scanner_info();
        let saved = self.context_flags;
        self.set_context_flags(node_flags::AWAIT_CONTEXT, outer_await);
        let modifiers = self.parse_modifiers_ex(true, false, false);
        self.context_flags = saved;
        if self.token == K::ThisKeyword {
            let name = self.create_identifier(true);
            let ty = self.parse_type_annotation();
            let node =
                self.factory
                    .new_parameter_declaration(modifiers, None, Some(name), None, ty, None);
            if let Some(modifiers) = modifiers {
                let nodes = self.factory.read_list(modifiers).nodes();
                let first = self
                    .factory
                    .read_nodes(nodes)
                    .at(0)
                    .expect("modifier list contains nodes");
                let range = self.factory.node(first).range();
                self.parse_error_at_range(
                    range,
                    diag::Neither_decorators_nor_modifiers_may_be_applied_to_this_parameters,
                    Vec::new(),
                );
            }
            self.finish_node(node, pos);
            self.with_js_doc(node, jsdoc);
            return Some(node);
        }
        let rest = self.parse_optional_token(K::DotDotDotToken);
        if !allow_ambiguity && !self.is_parameter_name_start() {
            return None;
        }
        let name = self.parse_name_of_parameter(modifiers);
        let question = self.parse_optional_token(K::QuestionToken);
        let ty = self.parse_type_annotation();
        let initializer = self.parse_initializer();
        let node = self.factory.new_parameter_declaration(
            modifiers,
            rest,
            Some(name),
            question,
            ty,
            initializer,
        );
        self.finish_node(node, pos);
        self.with_js_doc(node, jsdoc);
        Some(node)
    }
    /// port: tsc/internal/parser/parser.go:Parser.isParameterNameStart
    pub(crate) fn is_parameter_name_start(&self) -> bool {
        self.is_binding_identifier()
            || matches!(self.token, K::OpenBracketToken | K::OpenBraceToken)
    }
    /// port: tsc/internal/parser/parser.go:Parser.parseNameOfParameter
    pub(crate) fn parse_name_of_parameter(&mut self, modifiers: Option<NodeListId>) -> NodeId {
        let name = self.parse_identifier_or_pattern_with_diagnostic(Some(
            diag::Private_identifiers_cannot_be_used_as_parameters,
        ));
        if self.factory.node(name).range().is_empty()
            && modifiers.is_none()
            && ts_ast::is_modifier_kind(self.token.into())
        {
            self.next_token();
        }
        name
    }
    /// port: tsc/internal/parser/parser.go:Parser.parseReturnType
    pub(crate) fn parse_return_type(&mut self, return_token: K, is_type: bool) -> Option<NodeId> {
        if self.should_parse_return_type(return_token, is_type) {
            Some(self.do_in_context(
                node_flags::DISALLOW_CONDITIONAL_TYPES_CONTEXT,
                false,
                Self::parse_type_or_type_predicate,
            ))
        } else {
            None
        }
    }
    /// port: tsc/internal/parser/parser.go:Parser.shouldParseReturnType
    pub(crate) fn should_parse_return_type(&mut self, return_token: K, is_type: bool) -> bool {
        if return_token == K::EqualsGreaterThanToken {
            self.parse_expected(return_token);
            return true;
        }
        if self.parse_optional(K::ColonToken) {
            return true;
        }
        if is_type && self.token == K::EqualsGreaterThanToken {
            self.parse_error_at_current_token(
                diag::X_0_expected,
                vec![JsString::from_bytes(&b":"[..])],
            );
            self.next_token();
            return true;
        }
        false
    }
    /// port: tsc/internal/parser/parser.go:Parser.parseTypeOrTypePredicate
    pub(crate) fn parse_type_or_type_predicate(&mut self) -> NodeId {
        if self.is_identifier() {
            let state = self.mark();
            let pos = self.node_pos();
            let id = self.parse_identifier();
            if self.token == K::IsKeyword && !self.has_preceding_line_break() {
                self.next_token();
                let ty = self.parse_type();
                let node = self
                    .factory
                    .new_type_predicate_node(None, Some(id), Some(ty));
                self.finish_node(node, pos);
                self.commit(state);
                return node;
            }
            self.rewind(state);
        }
        self.parse_type()
    }
    /// port: tsc/internal/parser/parser.go:Parser.parseTypeMemberSemicolon
    pub(crate) fn parse_type_member_semicolon(&mut self) {
        if !self.parse_optional(K::CommaToken) {
            self.parse_semicolon();
        }
    }
    /// port: tsc/internal/parser/parser.go:Parser.parseAccessorDeclaration
    pub(crate) fn parse_accessor_declaration(
        &mut self,
        pos: i64,
        jsdoc: u8,
        modifiers: Option<NodeListId>,
        kind: K,
        flags: u32,
    ) -> NodeId {
        let name = self.parse_property_name();
        let type_parameters = self.parse_type_parameters();
        let parameters = self.parse_parameters(parse_flags::NONE);
        let ty = self.parse_return_type(K::ColonToken, false);
        let body = self.parse_function_block_or_semicolon(flags, None);
        let node = if kind == K::GetAccessor {
            self.factory.new_get_accessor_declaration(
                modifiers,
                Some(name),
                type_parameters,
                parameters,
                ty,
                None,
                body,
            )
        } else {
            self.factory.new_set_accessor_declaration(
                modifiers,
                Some(name),
                type_parameters,
                parameters,
                ty,
                None,
                body,
            )
        };
        self.finish_node(node, pos);
        self.with_js_doc(node, jsdoc);
        if flags & parse_flags::TYPE == 0 {
            self.check_js_syntax(node);
        }
        node
    }
    /// port: tsc/internal/parser/parser.go:Parser.parsePropertyName
    pub(crate) fn parse_property_name(&mut self) -> NodeId {
        let saved = self.statement_has_await_identifier;
        let node = self.parse_property_name_worker(true);
        self.statement_has_await_identifier = saved;
        node
    }
    /// port: tsc/internal/parser/parser.go:Parser.parsePropertyNameWorker
    pub(crate) fn parse_property_name_worker(&mut self, allow_computed: bool) -> NodeId {
        if matches!(
            self.token,
            K::StringLiteral | K::NumericLiteral | K::BigIntLiteral
        ) {
            return self.parse_literal_expression();
        }
        if allow_computed && self.token == K::OpenBracketToken {
            return self.parse_computed_property_name();
        }
        if self.token == K::PrivateIdentifier {
            return self.parse_private_identifier();
        }
        self.parse_identifier_name()
    }
    /// port: tsc/internal/parser/parser.go:Parser.parseComputedPropertyName
    pub(crate) fn parse_computed_property_name(&mut self) -> NodeId {
        let pos = self.node_pos();
        self.parse_expected(K::OpenBracketToken);
        let expression = self.parse_expression_allow_in();
        self.parse_expected(K::CloseBracketToken);
        let node = self.factory.new_computed_property_name(Some(expression));
        self.finish_node(node, pos)
    }
    /// port: tsc/internal/parser/parser.go:Parser.parseFunctionBlockOrSemicolon
    pub(crate) fn parse_function_block_or_semicolon(
        &mut self,
        flags: u32,
        message: Option<&'static diag::Message>,
    ) -> Option<NodeId> {
        if self.token != K::OpenBraceToken {
            if flags & parse_flags::TYPE != 0 {
                self.parse_type_member_semicolon();
                return None;
            }
            if self.can_parse_semicolon() {
                self.parse_semicolon();
                return None;
            }
        }
        Some(self.parse_function_block(flags, message))
    }
    /// port: tsc/internal/parser/parser.go:Parser.parseFunctionBlock
    pub(crate) fn parse_function_block(
        &mut self,
        flags: u32,
        message: Option<&'static diag::Message>,
    ) -> NodeId {
        let saved = self.context_flags;
        let saved_await = self.statement_has_await_identifier;
        self.set_context_flags(node_flags::YIELD_CONTEXT, flags & parse_flags::YIELD != 0);
        self.set_context_flags(node_flags::AWAIT_CONTEXT, flags & parse_flags::AWAIT != 0);
        self.set_context_flags(node_flags::DECORATOR_CONTEXT, false);
        let block = self.parse_block(flags & parse_flags::IGNORE_MISSING_OPEN_BRACE != 0, message);
        self.context_flags = saved;
        self.statement_has_await_identifier = saved_await;
        block
    }
    /// port: tsc/internal/parser/parser.go:Parser.isIndexSignature
    pub(crate) fn is_index_signature(&mut self) -> bool {
        self.token == K::OpenBracketToken
            && self.look_ahead(Self::next_is_unambiguously_index_signature)
    }
    /// port: tsc/internal/parser/parser.go:Parser.nextIsUnambiguouslyIndexSignature
    pub(crate) fn next_is_unambiguously_index_signature(&mut self) -> bool {
        self.next_token();
        if matches!(self.token, K::DotDotDotToken | K::CloseBracketToken) {
            return true;
        }
        if ts_ast::is_modifier_kind(self.token.into()) {
            self.next_token();
            if self.is_identifier() {
                return true;
            }
        } else if !self.is_identifier() {
            return false;
        } else {
            self.next_token();
        }
        if matches!(self.token, K::ColonToken | K::CommaToken) {
            return true;
        }
        if self.token != K::QuestionToken {
            return false;
        }
        self.next_token();
        matches!(
            self.token,
            K::ColonToken | K::CommaToken | K::CloseBracketToken
        )
    }
    /// port: tsc/internal/parser/parser.go:Parser.parseIndexSignatureDeclaration
    pub(crate) fn parse_index_signature_declaration(
        &mut self,
        pos: i64,
        jsdoc: u8,
        modifiers: Option<NodeListId>,
    ) -> NodeId {
        let parameters = self.parse_bracketed_list(
            ParsingContext::Parameters,
            Self::parse_parameter,
            K::OpenBracketToken,
            K::CloseBracketToken,
        );
        let ty = self.parse_type_annotation();
        self.parse_type_member_semicolon();
        let node = self
            .factory
            .new_index_signature_declaration(modifiers, parameters, ty);
        self.finish_node(node, pos);
        self.with_js_doc(node, jsdoc);
        node
    }
    /// port: tsc/internal/parser/parser.go:Parser.parsePropertyOrMethodSignature
    pub(crate) fn parse_property_or_method_signature(
        &mut self,
        pos: i64,
        jsdoc: u8,
        modifiers: Option<NodeListId>,
    ) -> NodeId {
        let name = self.parse_property_name();
        let question = self.parse_optional_token(K::QuestionToken);
        let node = if matches!(self.token, K::OpenParenToken | K::LessThanToken) {
            let type_parameters = self.parse_type_parameters();
            let parameters = self.parse_parameters(parse_flags::TYPE);
            let ty = self.parse_return_type(K::ColonToken, true);
            self.factory.new_method_signature_declaration(
                modifiers,
                Some(name),
                question,
                type_parameters,
                parameters,
                ty,
            )
        } else {
            let ty = self.parse_type_annotation();
            let initializer = if self.token == K::EqualsToken {
                self.parse_initializer()
            } else {
                None
            };
            self.factory.new_property_signature_declaration(
                modifiers,
                Some(name),
                question,
                ty,
                initializer,
            )
        };
        self.parse_type_member_semicolon();
        self.finish_node(node, pos);
        self.with_js_doc(node, jsdoc);
        node
    }
    /// port: tsc/internal/parser/parser.go:Parser.parseTypeLiteral
    pub(crate) fn parse_type_literal(&mut self) -> NodeId {
        let pos = self.node_pos();
        let members = self.parse_object_type_members();
        let node = self.factory.new_type_literal_node(Some(members));
        self.finish_node(node, pos)
    }
    /// port: tsc/internal/parser/parser.go:Parser.parseObjectTypeMembers
    pub(crate) fn parse_object_type_members(&mut self) -> NodeListId {
        if self.parse_expected(K::OpenBraceToken) {
            let members = self.parse_list(ParsingContext::TypeMembers, Self::parse_type_member);
            self.parse_expected(K::CloseBraceToken);
            members
        } else {
            self.create_missing_list()
        }
    }
    /// port: tsc/internal/parser/parser.go:Parser.parseTypeMember
    pub(crate) fn parse_type_member(&mut self) -> NodeId {
        if matches!(self.token, K::OpenParenToken | K::LessThanToken) {
            return self.parse_signature_member(K::CallSignature);
        }
        if self.token == K::NewKeyword
            && self.look_ahead(Self::next_token_is_open_paren_or_less_than)
        {
            return self.parse_signature_member(K::ConstructSignature);
        }
        let pos = self.node_pos();
        let jsdoc = self.jsdoc_scanner_info();
        let modifiers = self.parse_modifiers();
        if self.parse_contextual_modifier(K::GetKeyword) {
            return self.parse_accessor_declaration(
                pos,
                jsdoc,
                modifiers,
                K::GetAccessor,
                parse_flags::TYPE,
            );
        }
        if self.parse_contextual_modifier(K::SetKeyword) {
            return self.parse_accessor_declaration(
                pos,
                jsdoc,
                modifiers,
                K::SetAccessor,
                parse_flags::TYPE,
            );
        }
        if self.is_index_signature() {
            return self.parse_index_signature_declaration(pos, jsdoc, modifiers);
        }
        self.parse_property_or_method_signature(pos, jsdoc, modifiers)
    }
    /// port: tsc/internal/parser/parser.go:Parser.nextTokenIsOpenParenOrLessThan
    pub(crate) fn next_token_is_open_paren_or_less_than(&mut self) -> bool {
        self.next_token();
        matches!(self.token, K::OpenParenToken | K::LessThanToken)
    }
    /// port: tsc/internal/parser/parser.go:Parser.parseSignatureMember
    pub(crate) fn parse_signature_member(&mut self, kind: K) -> NodeId {
        let pos = self.node_pos();
        let jsdoc = self.jsdoc_scanner_info();
        if kind == K::ConstructSignature {
            self.parse_expected(K::NewKeyword);
        }
        let type_parameters = self.parse_type_parameters();
        let parameters = self.parse_parameters(parse_flags::TYPE);
        let ty = self.parse_return_type(K::ColonToken, true);
        self.parse_type_member_semicolon();
        let node = if kind == K::CallSignature {
            self.factory
                .new_call_signature_declaration(type_parameters, parameters, ty)
        } else {
            self.factory
                .new_construct_signature_declaration(type_parameters, parameters, ty)
        };
        self.finish_node(node, pos);
        self.with_js_doc(node, jsdoc);
        node
    }

    /// port: tsc/internal/parser/parser.go:Parser.parseTupleType
    pub(crate) fn parse_tuple_type(&mut self) -> NodeId {
        let pos = self.node_pos();
        let elements = self.parse_bracketed_list(
            ParsingContext::TupleElementTypes,
            Self::parse_tuple_element_name_or_tuple_element_type,
            K::OpenBracketToken,
            K::CloseBracketToken,
        );
        let node = self.factory.new_tuple_type_node(elements);
        self.finish_node(node, pos)
    }
    /// port: tsc/internal/parser/parser.go:Parser.parseTupleElementNameOrTupleElementType
    pub(crate) fn parse_tuple_element_name_or_tuple_element_type(&mut self) -> NodeId {
        if self.look_ahead(Self::scan_start_of_named_tuple_element) {
            let pos = self.node_pos();
            let jsdoc = self.jsdoc_scanner_info();
            let rest = self.parse_optional_token(K::DotDotDotToken);
            let name = self.parse_identifier_name();
            let question = self.parse_optional_token(K::QuestionToken);
            self.parse_expected(K::ColonToken);
            let ty = self.parse_tuple_element_type();
            let node = self
                .factory
                .new_named_tuple_member(rest, Some(name), question, Some(ty));
            self.finish_node(node, pos);
            self.with_js_doc(node, jsdoc);
            return node;
        }
        self.parse_tuple_element_type()
    }
    /// port: tsc/internal/parser/parser.go:Parser.scanStartOfNamedTupleElement
    pub(crate) fn scan_start_of_named_tuple_element(&mut self) -> bool {
        if self.token == K::DotDotDotToken {
            return token_is_identifier_or_keyword(self.next_token())
                && self.next_token_is_colon_or_question_colon();
        }
        token_is_identifier_or_keyword(self.token) && self.next_token_is_colon_or_question_colon()
    }
    /// port: tsc/internal/parser/parser.go:Parser.nextTokenIsColonOrQuestionColon
    pub(crate) fn next_token_is_colon_or_question_colon(&mut self) -> bool {
        self.next_token() == K::ColonToken
            || self.token == K::QuestionToken && self.next_token() == K::ColonToken
    }
    /// port: tsc/internal/parser/parser.go:Parser.parseTupleElementType
    pub(crate) fn parse_tuple_element_type(&mut self) -> NodeId {
        let pos = self.node_pos();
        if self.parse_optional(K::DotDotDotToken) {
            let ty = self.parse_type();
            let node = self.factory.new_rest_type_node(Some(ty));
            return self.finish_node(node, pos);
        }
        let ty = self.parse_type();
        let optional = {
            let node = self.factory.node(ty);
            if node.kind() == K::JSDocNullableType {
                let NodeDataRead::JSDocNullableType(data) = node.data() else {
                    panic!("JSDoc nullable payload required");
                };
                let inner = data.r#type().expect("JSDoc nullable type has inner type");
                (node.pos() == self.factory.node(inner).pos()).then_some((
                    inner,
                    node.flags(),
                    node.range(),
                ))
            } else {
                None
            }
        };
        if let Some((inner, flags, range)) = optional {
            let node = self.factory.new_optional_type_node(Some(inner));
            self.factory.set_node_flags(node, flags);
            self.factory.set_node_range(node, range);
            self.factory.set_node_parent(inner, Some(node));
            return node;
        }
        ty
    }
    /// port: tsc/internal/parser/parser.go:Parser.parseParenthesizedType
    pub(crate) fn parse_parenthesized_type(&mut self) -> NodeId {
        let pos = self.node_pos();
        self.parse_expected(K::OpenParenToken);
        let ty = self.parse_type();
        self.parse_expected(K::CloseParenToken);
        let node = self.factory.new_parenthesized_type_node(Some(ty));
        self.finish_node(node, pos)
    }
    /// port: tsc/internal/parser/parser.go:Parser.parseAssertsTypePredicate
    pub(crate) fn parse_asserts_type_predicate(&mut self) -> NodeId {
        let pos = self.node_pos();
        let asserts = self.parse_expected_token(K::AssertsKeyword);
        let name = if self.token == K::ThisKeyword {
            self.parse_this_type_node()
        } else {
            self.parse_identifier()
        };
        let ty = if self.parse_optional(K::IsKeyword) {
            Some(self.parse_type())
        } else {
            None
        };
        let node = self
            .factory
            .new_type_predicate_node(Some(asserts), Some(name), ty);
        self.finish_node(node, pos)
    }
    /// port: tsc/internal/parser/parser.go:Parser.parseFunctionOrConstructorTypeToError
    pub(crate) fn parse_function_or_constructor_type_to_error(
        &mut self,
        union: bool,
        parse: fn(&mut Self) -> NodeId,
    ) -> NodeId {
        if self.is_start_of_function_type_or_constructor_type() {
            let ty = self.parse_function_or_constructor_type();
            let node = self.factory.node(ty);
            let message = if node.kind() == K::FunctionType {
                if union {
                    diag::Function_type_notation_must_be_parenthesized_when_used_in_a_union_type
                } else {
                    diag::Function_type_notation_must_be_parenthesized_when_used_in_an_intersection_type
                }
            } else if union {
                diag::Constructor_type_notation_must_be_parenthesized_when_used_in_a_union_type
            } else {
                diag::Constructor_type_notation_must_be_parenthesized_when_used_in_an_intersection_type
            };
            let range = node.range();
            drop(node);
            self.parse_error_at_range(range, message, Vec::new());
            return ty;
        }
        parse(self)
    }
    /// port: tsc/internal/parser/parser.go:Parser.isStartOfFunctionTypeOrConstructorType
    pub(crate) fn is_start_of_function_type_or_constructor_type(&mut self) -> bool {
        self.token == K::LessThanToken
            || self.token == K::OpenParenToken
                && self.look_ahead(Self::next_is_unambiguously_start_of_function_type)
            || self.token == K::NewKeyword
            || self.token == K::AbstractKeyword && self.look_ahead(Self::next_token_is_new_keyword)
    }
    /// port: tsc/internal/parser/parser.go:Parser.parseFunctionOrConstructorType
    pub(crate) fn parse_function_or_constructor_type(&mut self) -> NodeId {
        let pos = self.node_pos();
        let jsdoc = self.jsdoc_scanner_info();
        let modifiers = self.parse_modifiers_for_constructor_type();
        let constructor = self.parse_optional(K::NewKeyword);
        assert!(
            modifiers.is_none() || constructor,
            "Debug failure. False expression: Per isStartOfFunctionOrConstructorType, a function type cannot have modifiers."
        );
        let type_parameters = self.parse_type_parameters();
        let parameters = self.parse_parameters(parse_flags::TYPE);
        let ty = self.parse_return_type(K::EqualsGreaterThanToken, false);
        let node = if constructor {
            self.factory
                .new_constructor_type_node(modifiers, type_parameters, parameters, ty)
        } else {
            self.factory
                .new_function_type_node(type_parameters, parameters, ty)
        };
        self.finish_node(node, pos);
        self.with_js_doc(node, jsdoc);
        node
    }
    /// port: tsc/internal/parser/parser.go:Parser.parseModifiersForConstructorType
    pub(crate) fn parse_modifiers_for_constructor_type(&mut self) -> Option<NodeListId> {
        if self.token == K::AbstractKeyword {
            let pos = self.node_pos();
            let modifier = self.factory.new_modifier(self.token.into());
            self.next_token();
            self.finish_node(modifier, pos);
            let range = self.factory.node(modifier).range();
            return Some(self.new_modifier_list(range, vec![modifier]));
        }
        None
    }
    /// port: tsc/internal/parser/parser.go:Parser.nextTokenIsNewKeyword
    pub(crate) fn next_token_is_new_keyword(&mut self) -> bool {
        self.next_token() == K::NewKeyword
    }
    /// port: tsc/internal/parser/parser.go:Parser.nextIsUnambiguouslyStartOfFunctionType
    pub(crate) fn next_is_unambiguously_start_of_function_type(&mut self) -> bool {
        self.next_token();
        if matches!(self.token, K::CloseParenToken | K::DotDotDotToken) {
            return true;
        }
        if self.skip_parameter_start() {
            if matches!(
                self.token,
                K::ColonToken | K::CommaToken | K::QuestionToken | K::EqualsToken
            ) {
                return true;
            }
            if self.token == K::CloseParenToken && self.next_token() == K::EqualsGreaterThanToken {
                return true;
            }
        }
        false
    }
    /// port: tsc/internal/parser/parser.go:Parser.skipParameterStart
    pub(crate) fn skip_parameter_start(&mut self) -> bool {
        if ts_ast::is_modifier_kind(self.token.into()) {
            self.parse_modifiers();
        }
        self.parse_optional(K::DotDotDotToken);
        if self.is_identifier() || self.token == K::ThisKeyword {
            self.next_token();
            return true;
        }
        if matches!(self.token, K::OpenBracketToken | K::OpenBraceToken) {
            let previous = self.diagnostics.len();
            self.parse_identifier_or_pattern();
            return previous == self.diagnostics.len();
        }
        false
    }
}
