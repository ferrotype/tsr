use crate::{Parser, ParserFactory};
use ts_ast::{
    token_flags, AstBuilder, Diagnostic, Factory, FactoryMethods, NodeId, RuntimeFactory,
    SyntaxKind as K,
};
use ts_core::TextRange;
use ts_diagnostics as diagnostics;

impl Parser<'_, AstBuilder> {
    /// port: tsc/internal/parser/parser.go:Parser.parseJSONText
    pub(crate) fn parse_json_text(&mut self) -> NodeId {
        let pos = self.node_pos();
        let (statements, eof) = if self.token == K::EndOfFile {
            let list = self.new_node_list(TextRange::new(pos, self.node_pos()), vec![]);
            (list, self.parse_token_node())
        } else {
            let mut expressions = vec![];
            while self.token != K::EndOfFile {
                let token = self.token;
                let expression = match token {
                    K::OpenBracketToken => self.parse_array_literal_expression(),
                    K::TrueKeyword | K::FalseKeyword | K::NullKeyword => self.parse_token_node(),
                    K::MinusToken => {
                        if self.look_ahead(|p| {
                            p.next_token() == K::NumericLiteral && p.next_token() != K::ColonToken
                        }) {
                            self.parse_prefix_unary_expression()
                        } else {
                            self.parse_object_literal_expression()
                        }
                    }
                    K::NumericLiteral | K::StringLiteral => {
                        if self.look_ahead(|p| p.next_token() != K::ColonToken) {
                            self.parse_literal_expression()
                        } else {
                            self.parse_object_literal_expression()
                        }
                    }
                    _ => self.parse_object_literal_expression(),
                };
                expressions.push(expression);
                if expressions.len() == 1 && self.token != K::EndOfFile {
                    self.parse_error_at_current_token(diagnostics::Unexpected_token, vec![]);
                }
            }
            let expression = if expressions.len() == 1 {
                expressions[0]
            } else {
                let list = self.new_node_list(TextRange::new(pos, self.node_pos()), expressions);
                let node = self.factory.new_array_literal_expression(Some(list), false);
                self.finish_node(node, pos)
            };
            let statement = self.factory.new_expression_statement(Some(expression));
            self.finish_node(statement, pos);
            let list = self.new_node_list(TextRange::new(pos, self.node_pos()), vec![statement]);
            (list, self.parse_expected_token(K::EndOfFile))
        };
        let root = self.factory.new_source_file(
            self.opts.clone(),
            self.source_owner.clone(),
            Some(statements),
            Some(eof),
        );
        self.finish_node(root, pos);
        let nodes = self.factory.read_list(statements).nodes();
        if !nodes.is_empty() {
            let statement = self
                .factory
                .read_nodes(nodes)
                .at(0)
                .expect("JSON statement");
            let expression = self
                .factory
                .node(statement)
                .data_source()
                .as_expression_statement()
                .expect("JSON expression statement")
                .expression();
            self.validate_json_value(root, expression);
        }
        self.finish_source_file(root, false);
        root
    }
}

impl<F: ParserFactory> Parser<'_, F> {
    /// port: tsc/internal/parser/parser.go:getErrorSpanForNode
    pub(crate) fn get_error_span_for_node(&self, node: NodeId) -> TextRange {
        let loc = self.factory.node(node).range();
        let pos = if self.node_is_missing(Some(node)) {
            loc.pos()
        } else {
            ts_scanner::skip_trivia(self.source_text, loc.pos())
        };
        TextRange::new(pos, loc.end())
    }
    /// port: tsc/internal/parser/parser.go:Parser.validateJsonValue
    pub(crate) fn validate_json_value(&mut self, source: NodeId, value: Option<NodeId>) {
        let Some(value) = value else {
            return;
        };
        crate::recursion::guarded(|| {
            let kind = self.factory.node(value).kind().known();
            match kind {
                Some(K::TrueKeyword | K::FalseKeyword | K::NullKeyword | K::NumericLiteral) => {
                    return;
                }
                Some(K::StringLiteral) => {
                    if !self.is_double_quoted_string(value) {
                        self.diagnostics.push(Diagnostic::new(
                            Some(source),
                            self.get_error_span_for_node(value),
                            diagnostics::String_literal_with_double_quotes_expected,
                            vec![],
                        ));
                    }
                    return;
                }
                Some(K::PrefixUnaryExpression) => {
                    let data = self.factory.node(value);
                    let unary = data
                        .data_source()
                        .as_prefix_unary_expression()
                        .expect("prefix unary payload");
                    if unary.operator() == K::MinusToken
                        && self
                            .factory
                            .node(unary.operand().expect("parsed unary operand"))
                            .kind()
                            == K::NumericLiteral
                    {
                        return;
                    }
                }
                Some(K::ObjectLiteralExpression) => {
                    self.validate_json_object_literal(source, value);
                    return;
                }
                Some(K::ArrayLiteralExpression) => {
                    let elements = self
                        .factory
                        .node(value)
                        .data_source()
                        .as_array_literal_expression()
                        .expect("array payload")
                        .elements();
                    if let Some(list) = elements {
                        let nodes = self.factory.read_list(list).nodes();
                        for i in 0..nodes.len() {
                            let element = self.factory.read_nodes(nodes).at(i);
                            self.validate_json_value(source, element);
                        }
                    }
                    return;
                }
                _ => {}
            }
            self.diagnostics.push(Diagnostic::new(Some(source), self.get_error_span_for_node(value), diagnostics::Property_value_can_only_be_string_literal_numeric_literal_true_false_null_object_literal_or_array_literal, vec![]));
        });
    }
    /// port: tsc/internal/parser/parser.go:isDoubleQuotedString
    pub(crate) fn is_double_quoted_string(&self, node: NodeId) -> bool {
        let node = self.factory.node(node);
        node.kind() == K::StringLiteral
            && node
                .data_source()
                .as_string_literal()
                .expect("string payload")
                .token_flags()
                & token_flags::SINGLE_QUOTE
                == 0
    }
    /// port: tsc/internal/parser/parser.go:Parser.validateJsonObjectLiteral
    pub(crate) fn validate_json_object_literal(&mut self, source: NodeId, node: NodeId) {
        let properties = self
            .factory
            .node(node)
            .data_source()
            .as_object_literal_expression()
            .expect("object payload")
            .properties()
            .expect("parsed object properties");
        let nodes = self.factory.read_list(properties).nodes();
        for i in 0..nodes.len() {
            let element = self
                .factory
                .read_nodes(nodes)
                .at(i)
                .expect("parsed property");
            if self.factory.node(element).kind() != K::PropertyAssignment {
                self.diagnostics.push(Diagnostic::new(
                    Some(source),
                    self.get_error_span_for_node(element),
                    diagnostics::Property_assignment_expected,
                    vec![],
                ));
                continue;
            }
            let (name, initializer) = {
                let data = self.factory.node(element);
                let property = data
                    .data_source()
                    .as_property_assignment()
                    .expect("property payload");
                (property.name(), property.initializer())
            };
            if let Some(name) = name.filter(|&name| !self.is_double_quoted_string(name)) {
                self.diagnostics.push(Diagnostic::new(
                    Some(source),
                    self.get_error_span_for_node(name),
                    diagnostics::String_literal_with_double_quotes_expected,
                    vec![],
                ));
            }
            self.validate_json_value(source, initializer);
        }
    }
}
