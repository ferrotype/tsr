use crate::tokens::token_is_identifier_or_keyword;
use crate::{Parser, ParserFactory, ParsingContext};
use ts_ast::{node_flags, FactoryMethods, NodeId, NodeListId, SyntaxKind};
use ts_core::TextRange;
use ts_diagnostics as diagnostics;

impl<F: ParserFactory> Parser<'_, F> {
    /// port: tsc/internal/parser/parser.go:Parser.parseLeftHandSideExpressionOrHigher
    pub(crate) fn parse_left_hand_side_expression_or_higher(&mut self) -> NodeId {
        let pos = self.node_pos();
        let expression = if self.token == SyntaxKind::ImportKeyword {
            if self.look_ahead(|p| {
                matches!(
                    p.next_token(),
                    SyntaxKind::OpenParenToken | SyntaxKind::LessThanToken
                )
            }) {
                self.source_flags |= node_flags::POSSIBLY_CONTAINS_DYNAMIC_IMPORT;
                self.parse_keyword_expression()
            } else if self.look_ahead(|p| p.next_token() == SyntaxKind::DotToken) {
                self.next_token();
                self.next_token();
                let name = self.parse_identifier_name();
                let node = self
                    .factory
                    .new_meta_property(SyntaxKind::ImportKeyword.into(), Some(name));
                self.finish_node(node, pos);
                if self
                    .factory
                    .node(name)
                    .data_source()
                    .as_identifier()
                    .expect("meta property identifier")
                    .text()
                    == b"defer"
                {
                    if matches!(
                        self.token,
                        SyntaxKind::OpenParenToken | SyntaxKind::LessThanToken
                    ) {
                        self.source_flags |= node_flags::POSSIBLY_CONTAINS_DYNAMIC_IMPORT;
                    }
                } else {
                    self.source_flags |= node_flags::POSSIBLY_CONTAINS_IMPORT_META;
                }
                node
            } else {
                self.parse_member_expression_or_higher()
            }
        } else if self.token == SyntaxKind::SuperKeyword {
            self.parse_super_expression()
        } else {
            self.parse_member_expression_or_higher()
        };
        self.parse_call_expression_rest(pos, expression)
    }
    /// port: tsc/internal/parser/parser.go:Parser.parseSuperExpression
    pub(crate) fn parse_super_expression(&mut self) -> NodeId {
        let pos = self.node_pos();
        let mut expression = self.parse_keyword_expression();
        if self.token == SyntaxKind::LessThanToken {
            let start = self.node_pos();
            if let Some(types) = self.try_parse_type_arguments_in_expression() {
                self.parse_error_at(
                    start,
                    self.node_pos(),
                    diagnostics::X_super_may_not_use_type_arguments,
                    vec![],
                );
                if !self.is_template_start_of_tagged_template() {
                    let node = self
                        .factory
                        .new_expression_with_type_arguments(Some(expression), Some(types));
                    expression = self.finish_node(node, pos);
                }
            }
        }
        if matches!(
            self.token,
            SyntaxKind::OpenParenToken | SyntaxKind::DotToken | SyntaxKind::OpenBracketToken
        ) {
            return expression;
        }
        self.parse_error_at_current_token(
            diagnostics::X_super_must_be_followed_by_an_argument_list_or_member_access,
            vec![],
        );
        let name = self.parse_right_side_of_dot(true, true, true);
        let node =
            self.factory
                .new_property_access_expression(Some(expression), None, Some(name), 0);
        self.finish_node(node, pos)
    }
    /// port: tsc/internal/parser/parser.go:Parser.isTemplateStartOfTaggedTemplate
    pub(crate) fn is_template_start_of_tagged_template(&self) -> bool {
        matches!(
            self.token,
            SyntaxKind::NoSubstitutionTemplateLiteral | SyntaxKind::TemplateHead
        )
    }
    /// port: tsc/internal/parser/parser.go:Parser.tryParseTypeArgumentsInExpression
    pub(crate) fn try_parse_type_arguments_in_expression(&mut self) -> Option<NodeListId> {
        if self.context_flags & node_flags::JAVA_SCRIPT_FILE != 0
            || !matches!(
                self.token,
                SyntaxKind::LessThanToken | SyntaxKind::LessThanLessThanToken
            )
        {
            return None;
        }
        let state = self.mark();
        if self.re_scan_less_than_token() == SyntaxKind::LessThanToken {
            self.next_token();
            let types = self.parse_delimited_list(ParsingContext::TypeArguments, Self::parse_type);
            if self.re_scan_greater_than_token() == SyntaxKind::GreaterThanToken {
                self.next_token();
                if self.can_follow_type_arguments_in_expression() {
                    self.commit(state);
                    return types;
                }
            }
        }
        self.rewind(state);
        None
    }
    /// port: tsc/internal/parser/parser.go:Parser.canFollowTypeArgumentsInExpression
    pub(crate) fn can_follow_type_arguments_in_expression(&mut self) -> bool {
        match self.token {
            SyntaxKind::OpenParenToken
            | SyntaxKind::NoSubstitutionTemplateLiteral
            | SyntaxKind::TemplateHead => true,
            SyntaxKind::LessThanToken
            | SyntaxKind::GreaterThanToken
            | SyntaxKind::PlusToken
            | SyntaxKind::MinusToken => false,
            _ => {
                self.has_preceding_line_break()
                    || self.is_binary_operator()
                    || !self.is_start_of_expression()
            }
        }
    }
    /// port: tsc/internal/parser/parser.go:Parser.parseMemberExpressionOrHigher
    pub(crate) fn parse_member_expression_or_higher(&mut self) -> NodeId {
        crate::recursion::guarded(|| {
            let pos = self.node_pos();
            let expression = self.parse_primary_expression();
            self.parse_member_expression_rest(pos, expression, true)
        })
    }
    /// port: tsc/internal/parser/parser.go:Parser.parseMemberExpressionRest
    pub(crate) fn parse_member_expression_rest(
        &mut self,
        pos: i64,
        mut expression: NodeId,
        allow_optional_chain: bool,
    ) -> NodeId {
        loop {
            let mut question = None;
            let property = if allow_optional_chain
                && self.is_start_of_optional_property_or_element_access_chain()
            {
                question = Some(self.parse_expected_token(SyntaxKind::QuestionDotToken));
                token_is_identifier_or_keyword(self.token)
            } else {
                self.parse_optional(SyntaxKind::DotToken)
            };
            if property {
                expression = self.parse_property_access_expression_rest(pos, expression, question);
                continue;
            }
            if (question.is_some() || !self.in_decorator_context())
                && self.parse_optional(SyntaxKind::OpenBracketToken)
            {
                expression = self.parse_element_access_expression_rest(pos, expression, question);
                continue;
            }
            if self.is_template_start_of_tagged_template() {
                if question.is_none()
                    && self.factory.node(expression).kind()
                        == SyntaxKind::ExpressionWithTypeArguments
                {
                    let (inner, types) = self.expression_with_type_arguments(expression);
                    expression = self.parse_tagged_template_rest(pos, inner, question, types);
                    self.unparse_expression_with_type_arguments(Some(inner), types, expression);
                } else {
                    expression = self.parse_tagged_template_rest(pos, expression, question, None);
                }
                continue;
            }
            if question.is_none() {
                if self.token == SyntaxKind::ExclamationToken && !self.has_preceding_line_break() {
                    self.next_token();
                    let node = self.factory.new_non_null_expression(Some(expression), 0);
                    self.finish_node(node, pos);
                    expression = self.check_js_syntax(node);
                    continue;
                }
                if let Some(types) = self.try_parse_type_arguments_in_expression() {
                    let node = self
                        .factory
                        .new_expression_with_type_arguments(Some(expression), Some(types));
                    expression = self.finish_node(node, pos);
                    continue;
                }
            }
            return expression;
        }
    }
    fn expression_with_type_arguments(&self, node: NodeId) -> (NodeId, Option<NodeListId>) {
        let node = self.factory.node(node);
        let data = node
            .data_source()
            .as_expression_with_type_arguments()
            .expect("instantiation has expression/type arguments payload");
        (
            data.expression()
                .expect("parsed instantiation has expression"),
            data.type_arguments(),
        )
    }
    /// port: tsc/internal/parser/parser.go:Parser.isStartOfOptionalPropertyOrElementAccessChain
    pub(crate) fn is_start_of_optional_property_or_element_access_chain(&mut self) -> bool {
        self.token == SyntaxKind::QuestionDotToken
            && self
                .look_ahead(Self::next_token_is_identifier_or_keyword_or_open_bracket_or_template)
    }
    /// port: tsc/internal/parser/parser.go:Parser.nextTokenIsIdentifierOrKeywordOrOpenBracketOrTemplate
    pub(crate) fn next_token_is_identifier_or_keyword_or_open_bracket_or_template(
        &mut self,
    ) -> bool {
        self.next_token();
        token_is_identifier_or_keyword(self.token)
            || self.token == SyntaxKind::OpenBracketToken
            || self.is_template_start_of_tagged_template()
    }
    /// port: tsc/internal/parser/parser.go:Parser.parsePropertyAccessExpressionRest
    pub(crate) fn parse_property_access_expression_rest(
        &mut self,
        pos: i64,
        expression: NodeId,
        question: Option<NodeId>,
    ) -> NodeId {
        let name = self.parse_right_side_of_dot(true, true, true);
        let optional = question.is_some() || self.try_reparse_optional_chain(expression);
        let node = self.factory.new_property_access_expression(
            Some(expression),
            question,
            Some(name),
            if optional {
                node_flags::OPTIONAL_CHAIN
            } else {
                0
            },
        );
        if optional && self.factory.node(name).kind() == SyntaxKind::PrivateIdentifier {
            let range = self.skip_range_trivia(self.factory.node(name).range());
            self.parse_error_at_range(
                range,
                diagnostics::An_optional_chain_cannot_contain_private_identifiers,
                vec![],
            );
        }
        if self.factory.node(expression).kind() == SyntaxKind::ExpressionWithTypeArguments {
            if let Some(types) = self.expression_with_type_arguments(expression).1 {
                let loc = self.factory.read_list(types).loc();
                let loc = TextRange::new(
                    loc.pos() - 1,
                    ts_scanner::skip_trivia(self.source_text, loc.end()) + 1,
                );
                self.parse_error_at_range(loc, diagnostics::An_instantiation_expression_cannot_be_followed_by_a_property_access, vec![]);
            }
        }
        self.finish_node(node, pos)
    }
    /// port: tsc/internal/parser/parser.go:Parser.tryReparseOptionalChain
    pub(crate) fn try_reparse_optional_chain(&mut self, mut node: NodeId) -> bool {
        if self.factory.node(node).flags() & node_flags::OPTIONAL_CHAIN != 0 {
            return true;
        }
        if self.factory.node(node).kind() == SyntaxKind::NonNullExpression {
            let mut expression = self.non_null_expression(node);
            while self.factory.node(expression).kind() == SyntaxKind::NonNullExpression
                && self.factory.node(expression).flags() & node_flags::OPTIONAL_CHAIN == 0
            {
                expression = self.non_null_expression(expression);
            }
            if self.factory.node(expression).flags() & node_flags::OPTIONAL_CHAIN != 0 {
                while self.factory.node(node).kind() == SyntaxKind::NonNullExpression {
                    let next = self.non_null_expression(node);
                    self.factory
                        .add_node_flags(node, node_flags::OPTIONAL_CHAIN);
                    node = next;
                }
                return true;
            }
        }
        false
    }
    fn non_null_expression(&self, node: NodeId) -> NodeId {
        self.factory
            .node(node)
            .data_source()
            .as_non_null_expression()
            .expect("non-null payload")
            .expression()
            .expect("parsed non-null expression")
    }
    /// port: tsc/internal/parser/parser.go:Parser.parseElementAccessExpressionRest
    pub(crate) fn parse_element_access_expression_rest(
        &mut self,
        pos: i64,
        expression: NodeId,
        question: Option<NodeId>,
    ) -> NodeId {
        // The source allocates this missing identifier even when a real argument follows.
        let mut argument = self.create_missing_identifier();
        if self.token == SyntaxKind::CloseBracketToken {
            self.parse_error_at(
                self.node_pos(),
                self.node_pos(),
                diagnostics::An_element_access_expression_should_take_an_argument,
                vec![],
            );
        } else {
            argument = self.parse_expression_allow_in();
        }
        self.parse_expected(SyntaxKind::CloseBracketToken);
        let optional = question.is_some() || self.try_reparse_optional_chain(expression);
        let node = self.factory.new_element_access_expression(
            Some(expression),
            question,
            Some(argument),
            if optional {
                node_flags::OPTIONAL_CHAIN
            } else {
                0
            },
        );
        self.finish_node(node, pos)
    }
    /// port: tsc/internal/parser/parser.go:Parser.parseCallExpressionRest
    pub(crate) fn parse_call_expression_rest(
        &mut self,
        pos: i64,
        mut expression: NodeId,
    ) -> NodeId {
        loop {
            expression = self.parse_member_expression_rest(pos, expression, true);
            let mut types = None;
            let question = self.parse_optional_token(SyntaxKind::QuestionDotToken);
            if question.is_some() {
                types = self.try_parse_type_arguments_in_expression();
                if self.is_template_start_of_tagged_template() {
                    expression = self.parse_tagged_template_rest(pos, expression, question, types);
                    continue;
                }
            }
            if types.is_some() || self.token == SyntaxKind::OpenParenToken {
                if question.is_none()
                    && self.factory.node(expression).kind()
                        == SyntaxKind::ExpressionWithTypeArguments
                {
                    (expression, types) = self.expression_with_type_arguments(expression);
                }
                let inner = expression;
                let arguments = self.parse_argument_list();
                let optional = question.is_some() || self.try_reparse_optional_chain(expression);
                let node = self.factory.new_call_expression(
                    Some(expression),
                    question,
                    types,
                    arguments,
                    if optional {
                        node_flags::OPTIONAL_CHAIN
                    } else {
                        0
                    },
                );
                self.finish_node(node, pos);
                expression = self.check_js_syntax(node);
                self.unparse_expression_with_type_arguments(Some(inner), types, expression);
                continue;
            }
            if question.is_some() {
                self.parse_error_at_current_token(diagnostics::Identifier_expected, vec![]);
                let name = self.create_missing_identifier();
                let node = self.factory.new_property_access_expression(
                    Some(expression),
                    question,
                    Some(name),
                    node_flags::OPTIONAL_CHAIN,
                );
                expression = self.finish_node(node, pos);
            }
            return expression;
        }
    }
    /// port: tsc/internal/parser/parser.go:Parser.parseArgumentList
    pub(crate) fn parse_argument_list(&mut self) -> Option<NodeListId> {
        self.parse_expected(SyntaxKind::OpenParenToken);
        let result = self.parse_delimited_list(
            ParsingContext::ArgumentExpressions,
            Self::parse_argument_expression,
        );
        self.parse_expected(SyntaxKind::CloseParenToken);
        result
    }
    /// port: tsc/internal/parser/parser.go:Parser.parseArgumentExpression
    pub(crate) fn parse_argument_expression(&mut self) -> NodeId {
        self.do_in_context(
            node_flags::DISALLOW_IN_CONTEXT | node_flags::DECORATOR_CONTEXT,
            false,
            Self::parse_argument_or_array_literal_element,
        )
    }
    /// port: tsc/internal/parser/parser.go:Parser.parseArgumentOrArrayLiteralElement
    pub(crate) fn parse_argument_or_array_literal_element(&mut self) -> NodeId {
        if self.token == SyntaxKind::DotDotDotToken {
            return self.parse_spread_element();
        }
        if self.token == SyntaxKind::CommaToken {
            let node = self.factory.new_omitted_expression();
            return self.finish_node(node, self.node_pos());
        }
        self.parse_assignment_expression_or_higher()
    }
    /// port: tsc/internal/parser/parser.go:Parser.parseSpreadElement
    pub(crate) fn parse_spread_element(&mut self) -> NodeId {
        let pos = self.node_pos();
        self.parse_expected(SyntaxKind::DotDotDotToken);
        let expression = self.parse_assignment_expression_or_higher();
        let node = self.factory.new_spread_element(Some(expression));
        self.finish_node(node, pos)
    }
    /// port: tsc/internal/parser/parser.go:Parser.parseTaggedTemplateRest
    pub(crate) fn parse_tagged_template_rest(
        &mut self,
        pos: i64,
        tag: NodeId,
        question: Option<NodeId>,
        types: Option<NodeListId>,
    ) -> NodeId {
        let template = if self.token == SyntaxKind::NoSubstitutionTemplateLiteral {
            self.re_scan_template_token(true);
            self.parse_literal_expression()
        } else {
            self.parse_template_expression(true)
        };
        let optional =
            question.is_some() || self.factory.node(tag).flags() & node_flags::OPTIONAL_CHAIN != 0;
        let node = self.factory.new_tagged_template_expression(
            Some(tag),
            question,
            types,
            Some(template),
            if optional {
                node_flags::OPTIONAL_CHAIN
            } else {
                0
            },
        );
        self.finish_node(node, pos);
        self.check_js_syntax(node)
    }
    /// port: tsc/internal/parser/parser.go:Parser.unparseExpressionWithTypeArguments
    pub(crate) fn unparse_expression_with_type_arguments(
        &mut self,
        expression: Option<NodeId>,
        types: Option<NodeListId>,
        result: NodeId,
    ) {
        if let Some(expression) = expression {
            self.factory.set_node_parent(expression, Some(result));
        }
        if let Some(types) = types {
            let nodes = self.factory.read_list(types).nodes();
            for i in 0..nodes.len() {
                let node = self
                    .factory
                    .read_nodes(nodes)
                    .at(i)
                    .expect("parsed type argument exists");
                self.factory.set_node_parent(node, Some(result));
            }
        }
    }
    /// port: tsc/internal/parser/parser.go:Parser.parseNewExpressionOrNewDotTarget
    pub(crate) fn parse_new_expression_or_new_dot_target(&mut self) -> NodeId {
        crate::recursion::guarded(|| {
            let pos = self.node_pos();
            self.parse_expected(SyntaxKind::NewKeyword);
            if self.parse_optional(SyntaxKind::DotToken) {
                let name = self.parse_identifier_name();
                let node = self
                    .factory
                    .new_meta_property(SyntaxKind::NewKeyword.into(), Some(name));
                return self.finish_node(node, pos);
            }
            let expression_pos = self.node_pos();
            let primary = self.parse_primary_expression();
            let mut expression = self.parse_member_expression_rest(expression_pos, primary, false);
            let mut types = None;
            if self.factory.node(expression).kind() == SyntaxKind::ExpressionWithTypeArguments {
                (expression, types) = self.expression_with_type_arguments(expression);
            }
            if self.token == SyntaxKind::QuestionDotToken {
                let text = self.get_text_of_node_from_source_text(expression, false);
                self.parse_error_at_current_token(
                    diagnostics::Invalid_optional_chain_from_new_expression_Did_you_mean_to_call_0,
                    vec![text],
                );
            }
            let arguments = if self.token == SyntaxKind::OpenParenToken {
                self.parse_argument_list()
            } else {
                None
            };
            let node = self
                .factory
                .new_new_expression(Some(expression), types, arguments);
            self.finish_node(node, pos);
            let result = self.check_js_syntax(node);
            self.unparse_expression_with_type_arguments(Some(expression), types, result);
            result
        })
    }
}
