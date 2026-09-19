use crate::{Parser, ParserFactory, ParsingContext};
use ts_ast::{node_flags, FactoryMethods, JsString, NodeId, NodeListId, SyntaxKind};
use ts_core::TextRange;
use ts_diagnostics as diagnostics;

impl<F: ParserFactory> Parser<'_, F> {
    /// port: tsc/internal/parser/parser.go:Parser.parseJsxElementOrSelfClosingElementOrFragment
    pub(crate) fn parse_jsx_element_or_self_closing_element_or_fragment(
        &mut self,
        expression_context: bool,
        top_invalid_pos: i64,
        opening_tag: Option<NodeId>,
        must_be_unary: bool,
    ) -> NodeId {
        crate::recursion::guarded(|| {
            let pos = self.node_pos();
            let opening = self
                .parse_jsx_opening_or_self_closing_element_or_opening_fragment(expression_context);
            let opening_kind = self
                .factory
                .node(opening)
                .kind()
                .known()
                .expect("parsed JSX has known kind");
            let mut result = match opening_kind {
                SyntaxKind::JsxOpeningElement => {
                    let mut children = self.parse_jsx_children(opening);
                    let last = {
                        let nodes = self.factory.read_list(children).nodes();
                        self.factory.read_nodes(nodes).last().flatten()
                    };
                    let closing = if let Some(last) = last
                        .filter(|&last| self.jsx_child_consumed_parent_closing_tag(opening, last))
                    {
                        let (last_opening, last_children, last_closing) =
                            self.jsx_element_parts(last);
                        let end = self.factory.read_list(last_children).loc().end();
                        let missing = self.new_identifier(JsString::default());
                        self.finish_node_with_end(missing, end, end);
                        let new_closing = self.factory.new_jsx_closing_element(Some(missing));
                        self.finish_node_with_end(new_closing, end, end);
                        let new_last = self.factory.new_jsx_element(
                            Some(last_opening),
                            Some(last_children),
                            Some(new_closing),
                        );
                        let last_start = self.factory.node(last_opening).range().pos();
                        self.finish_node_with_end(new_last, last_start, end);
                        self.factory.set_node_parent(last_opening, Some(new_last));
                        let nodes = self.factory.read_list(last_children).nodes();
                        for i in 0..nodes.len() {
                            let child = self
                                .factory
                                .read_nodes(nodes)
                                .at(i)
                                .expect("parsed JSX child");
                            self.factory.set_node_parent(child, Some(new_last));
                        }
                        self.factory.set_node_parent(new_closing, Some(new_last));
                        let loc = self.factory.read_list(children).loc();
                        let nodes = self.factory.read_list(children).nodes();
                        let mut replacement: Vec<_> = self
                            .factory
                            .read_nodes(nodes)
                            .iter()
                            .map(|id| id.expect("parsed JSX child"))
                            .collect();
                        *replacement.last_mut().expect("last child was observed") = new_last;
                        children = self.new_node_list(TextRange::new(loc.pos(), end), replacement);
                        last_closing
                    } else {
                        let closing = self.parse_jsx_closing_element(opening, expression_context);
                        let open_name = self.jsx_tag_name(opening);
                        let close_name = self.jsx_tag_name(closing);
                        if !self.tag_names_are_equivalent(open_name, close_name) {
                            let text = self.get_text_of_node_from_source_text(open_name, false);
                            if opening_tag.is_some_and(|tag| {
                                self.factory.node(tag).kind() == SyntaxKind::JsxOpeningElement
                                    && self.tag_names_are_equivalent(
                                        close_name,
                                        self.jsx_tag_name(tag),
                                    )
                            }) {
                                let loc = self.factory.node(open_name).range();
                                self.parse_error_at_range(
                                    loc,
                                    diagnostics::JSX_element_0_has_no_corresponding_closing_tag,
                                    vec![text],
                                );
                            } else {
                                let loc = self.factory.node(close_name).range();
                                self.parse_error_at_range(
                                    loc,
                                    diagnostics::Expected_corresponding_JSX_closing_tag_for_0,
                                    vec![text],
                                );
                            }
                        }
                        closing
                    };
                    let node =
                        self.factory
                            .new_jsx_element(Some(opening), Some(children), Some(closing));
                    self.finish_node(node, pos);
                    self.factory.set_node_parent(closing, Some(node));
                    node
                }
                SyntaxKind::JsxOpeningFragment => {
                    let children = self.parse_jsx_children(opening);
                    let closing = self.parse_jsx_closing_fragment(expression_context);
                    let node =
                        self.factory
                            .new_jsx_fragment(Some(opening), Some(children), Some(closing));
                    self.finish_node(node, pos)
                }
                SyntaxKind::JsxSelfClosingElement => opening,
                _ => panic!("Unhandled case in parseJsxElementOrSelfClosingElementOrFragment"),
            };
            if !must_be_unary && expression_context && self.token == SyntaxKind::LessThanToken {
                let bad_pos = if top_invalid_pos < 0 {
                    self.factory.node(result).range().pos()
                } else {
                    top_invalid_pos
                };
                let invalid = self.parse_jsx_element_or_self_closing_element_or_fragment(
                    true, bad_pos, None, false,
                );
                let operator = self.factory.new_token(SyntaxKind::CommaToken.into());
                let loc = self.factory.node(invalid).range();
                self.factory
                    .set_node_range(operator, TextRange::new(loc.pos(), loc.pos()));
                self.parse_error_at(
                    ts_scanner::skip_trivia(self.source_text, bad_pos),
                    loc.end(),
                    diagnostics::JSX_expressions_must_have_one_parent_element,
                    vec![],
                );
                let node = self.factory.new_binary_expression(
                    None,
                    Some(result),
                    None,
                    Some(operator),
                    Some(invalid),
                );
                result = self.finish_node(node, pos);
            }
            result
        })
    }
    fn jsx_element_parts(&self, node: NodeId) -> (NodeId, NodeListId, NodeId) {
        let node = self.factory.node(node);
        let data = node
            .data_source()
            .as_jsx_element()
            .expect("JSX element payload");
        (
            data.opening_element().expect("parsed opening"),
            data.children().expect("parsed children"),
            data.closing_element().expect("parsed closing"),
        )
    }
    fn jsx_tag_name(&self, node: NodeId) -> NodeId {
        let node = self.factory.node(node);
        match node.kind().known() {
            Some(SyntaxKind::JsxOpeningElement) => node
                .data_source()
                .as_jsx_opening_element()
                .expect("opening payload")
                .tag_name(),
            Some(SyntaxKind::JsxClosingElement) => node
                .data_source()
                .as_jsx_closing_element()
                .expect("closing payload")
                .tag_name(),
            Some(SyntaxKind::JsxSelfClosingElement) => node
                .data_source()
                .as_jsx_self_closing_element()
                .expect("self-closing payload")
                .tag_name(),
            _ => panic!("JSX tag-name access requires a named tag"),
        }
        .expect("parsed JSX tag name")
    }
    fn jsx_child_consumed_parent_closing_tag(&self, opening: NodeId, child: NodeId) -> bool {
        if self.factory.node(opening).kind() != SyntaxKind::JsxOpeningElement
            || self.factory.node(child).kind() != SyntaxKind::JsxElement
        {
            return false;
        }
        let (child_opening, _, child_closing) = self.jsx_element_parts(child);
        !self.tag_names_are_equivalent(
            self.jsx_tag_name(child_opening),
            self.jsx_tag_name(child_closing),
        ) && self
            .tag_names_are_equivalent(self.jsx_tag_name(opening), self.jsx_tag_name(child_closing))
    }
    /// port: tsc/internal/parser/parser.go:Parser.parseJsxChildren
    pub(crate) fn parse_jsx_children(&mut self, opening: NodeId) -> NodeListId {
        let pos = self.node_pos();
        let saved = self.parsing_contexts;
        self.parsing_contexts |= 1 << ParsingContext::JsxChildren as u8;
        let mut list = vec![];
        loop {
            // The Go call intentionally leaves Parser.token unchanged here.
            let token = self.scan_operation(|scanner| scanner.rescan_jsx_token(true));
            let Some(child) = self.parse_jsx_child(opening, token) else {
                break;
            };
            list.push(child);
            if self.jsx_child_consumed_parent_closing_tag(opening, child) {
                break;
            }
        }
        self.parsing_contexts = saved;
        self.new_node_list(TextRange::new(pos, self.node_pos()), list)
    }
    /// port: tsc/internal/parser/parser.go:Parser.parseJsxChild
    pub(crate) fn parse_jsx_child(&mut self, opening: NodeId, token: SyntaxKind) -> Option<NodeId> {
        match token {
            SyntaxKind::EndOfFile => {
                if self.factory.node(opening).kind() == SyntaxKind::JsxOpeningFragment {
                    let loc = self.factory.node(opening).range();
                    self.parse_error_at_range(
                        loc,
                        diagnostics::JSX_fragment_has_no_corresponding_closing_tag,
                        vec![],
                    );
                } else {
                    let tag = self.jsx_tag_name(opening);
                    let loc = self.factory.node(tag).range();
                    let start = ts_scanner::skip_trivia(self.source_text, loc.pos()).min(loc.end());
                    let text = self.get_text_of_node_from_source_text(tag, false);
                    self.parse_error_at(
                        start,
                        loc.end(),
                        diagnostics::JSX_element_0_has_no_corresponding_closing_tag,
                        vec![text],
                    );
                }
                None
            }
            SyntaxKind::LessThanSlashToken | SyntaxKind::ConflictMarkerTrivia => None,
            SyntaxKind::JsxText | SyntaxKind::JsxTextAllWhiteSpaces => Some(self.parse_jsx_text()),
            SyntaxKind::OpenBraceToken => self.parse_jsx_expression(false),
            SyntaxKind::LessThanToken => {
                Some(self.parse_jsx_element_or_self_closing_element_or_fragment(
                    false,
                    -1,
                    Some(opening),
                    false,
                ))
            }
            _ => panic!("Unhandled case in parseJsxChild"),
        }
    }
    /// port: tsc/internal/parser/parser.go:Parser.parseJsxText
    pub(crate) fn parse_jsx_text(&mut self) -> NodeId {
        let pos = self.node_pos();
        let text = self.token_value();
        let node = self
            .factory
            .new_jsx_text(text, self.token == SyntaxKind::JsxTextAllWhiteSpaces);
        self.scan_jsx_text();
        self.finish_node(node, pos)
    }
    /// port: tsc/internal/parser/parser.go:Parser.parseJsxExpression
    pub(crate) fn parse_jsx_expression(&mut self, expression_context: bool) -> Option<NodeId> {
        let pos = self.node_pos();
        if !self.parse_expected(SyntaxKind::OpenBraceToken) {
            return None;
        }
        let mut dots = None;
        let mut expression = None;
        if self.token != SyntaxKind::CloseBraceToken {
            if !expression_context {
                dots = self.parse_optional_token(SyntaxKind::DotDotDotToken);
            }
            expression = Some(self.parse_expression());
        }
        if expression_context {
            self.parse_expected(SyntaxKind::CloseBraceToken);
        } else if self.parse_expected_without_advancing(SyntaxKind::CloseBraceToken) {
            self.scan_jsx_text();
        }
        let node = self.factory.new_jsx_expression(dots, expression);
        Some(self.finish_node(node, pos))
    }
    /// port: tsc/internal/parser/parser.go:Parser.scanJsxText
    pub(crate) fn scan_jsx_text(&mut self) -> SyntaxKind {
        self.token = self.scan_operation(ts_scanner::Scanner::scan_jsx_token);
        self.token
    }
    /// port: tsc/internal/parser/parser.go:Parser.scanJsxIdentifier
    pub(crate) fn scan_jsx_identifier(&mut self) -> SyntaxKind {
        self.token = self.scan_operation(ts_scanner::Scanner::scan_jsx_identifier);
        self.token
    }
    /// port: tsc/internal/parser/parser.go:Parser.scanJsxAttributeValue
    pub(crate) fn scan_jsx_attribute_value(&mut self) -> SyntaxKind {
        self.token = self.scan_operation(ts_scanner::Scanner::scan_jsx_attribute_value);
        self.token
    }
    /// port: tsc/internal/parser/parser.go:Parser.parseJsxClosingElement
    pub(crate) fn parse_jsx_closing_element(
        &mut self,
        opening: NodeId,
        expression_context: bool,
    ) -> NodeId {
        let pos = self.node_pos();
        self.parse_expected(SyntaxKind::LessThanSlashToken);
        let name = self.parse_jsx_element_name();
        if self.parse_expected_with_diagnostic(SyntaxKind::GreaterThanToken, None, false) {
            if expression_context
                || !self.tag_names_are_equivalent(self.jsx_tag_name(opening), name)
            {
                self.next_token();
            } else {
                self.scan_jsx_text();
            }
        }
        let node = self.factory.new_jsx_closing_element(Some(name));
        self.finish_node(node, pos)
    }
    /// port: tsc/internal/parser/parser.go:Parser.parseJsxOpeningOrSelfClosingElementOrOpeningFragment
    pub(crate) fn parse_jsx_opening_or_self_closing_element_or_opening_fragment(
        &mut self,
        expression_context: bool,
    ) -> NodeId {
        let pos = self.node_pos();
        self.parse_expected(SyntaxKind::LessThanToken);
        if self.token == SyntaxKind::GreaterThanToken {
            self.scan_jsx_text();
            let node = self.factory.new_jsx_opening_fragment();
            return self.finish_node(node, pos);
        }
        let name = self.parse_jsx_element_name();
        let types = if self.context_flags & node_flags::JAVA_SCRIPT_FILE == 0 {
            self.parse_type_arguments()
        } else {
            None
        };
        let attributes = self.parse_jsx_attributes();
        let node = if self.token == SyntaxKind::GreaterThanToken {
            self.scan_jsx_text();
            self.factory
                .new_jsx_opening_element(Some(name), types, Some(attributes))
        } else {
            self.parse_expected(SyntaxKind::SlashToken);
            if self.parse_expected_without_advancing(SyntaxKind::GreaterThanToken) {
                if expression_context {
                    self.next_token();
                } else {
                    self.scan_jsx_text();
                }
            }
            self.factory
                .new_jsx_self_closing_element(Some(name), types, Some(attributes))
        };
        self.finish_node(node, pos)
    }
    /// port: tsc/internal/parser/parser.go:Parser.parseJsxElementName
    pub(crate) fn parse_jsx_element_name(&mut self) -> NodeId {
        let pos = self.node_pos();
        let mut expression = self.parse_jsx_tag_name();
        if self.factory.node(expression).kind() == SyntaxKind::JsxNamespacedName {
            return expression;
        }
        while self.parse_optional(SyntaxKind::DotToken) {
            let name = self.parse_right_side_of_dot(true, false, false);
            let node =
                self.factory
                    .new_property_access_expression(Some(expression), None, Some(name), 0);
            expression = self.finish_node(node, pos);
        }
        expression
    }
    /// port: tsc/internal/parser/parser.go:Parser.parseJsxTagName
    pub(crate) fn parse_jsx_tag_name(&mut self) -> NodeId {
        let pos = self.node_pos();
        self.scan_jsx_identifier();
        let is_this = self.token == SyntaxKind::ThisKeyword;
        let name = self.parse_identifier_name_error_on_unicode_escape_sequence();
        if self.parse_optional(SyntaxKind::ColonToken) {
            self.scan_jsx_identifier();
            let right = self.parse_identifier_name_error_on_unicode_escape_sequence();
            let node = self
                .factory
                .new_jsx_namespaced_name(Some(name), Some(right));
            return self.finish_node(node, pos);
        }
        if is_this {
            let node = self
                .factory
                .new_keyword_expression(SyntaxKind::ThisKeyword.into());
            return self.finish_node(node, pos);
        }
        name
    }
    /// port: tsc/internal/parser/parser.go:Parser.parseJsxAttributes
    pub(crate) fn parse_jsx_attributes(&mut self) -> NodeId {
        let pos = self.node_pos();
        let attributes = self.parse_list(ParsingContext::JsxAttributes, Self::parse_jsx_attribute);
        let node = self.factory.new_jsx_attributes(Some(attributes));
        self.finish_node(node, pos)
    }
    /// port: tsc/internal/parser/parser.go:Parser.parseJsxAttribute
    pub(crate) fn parse_jsx_attribute(&mut self) -> NodeId {
        if self.token == SyntaxKind::OpenBraceToken {
            return self.parse_jsx_spread_attribute();
        }
        let pos = self.node_pos();
        let name = self.parse_jsx_attribute_name();
        let value = self.parse_jsx_attribute_value();
        let node = self.factory.new_jsx_attribute(Some(name), value);
        self.finish_node(node, pos)
    }
    /// port: tsc/internal/parser/parser.go:Parser.parseJsxSpreadAttribute
    pub(crate) fn parse_jsx_spread_attribute(&mut self) -> NodeId {
        let pos = self.node_pos();
        self.parse_expected(SyntaxKind::OpenBraceToken);
        self.parse_expected(SyntaxKind::DotDotDotToken);
        let expression = self.parse_expression();
        self.parse_expected(SyntaxKind::CloseBraceToken);
        let node = self.factory.new_jsx_spread_attribute(Some(expression));
        self.finish_node(node, pos)
    }
    /// port: tsc/internal/parser/parser.go:Parser.parseJsxAttributeName
    pub(crate) fn parse_jsx_attribute_name(&mut self) -> NodeId {
        let pos = self.node_pos();
        self.scan_jsx_identifier();
        let name = self.parse_identifier_name_error_on_unicode_escape_sequence();
        if self.parse_optional(SyntaxKind::ColonToken) {
            self.scan_jsx_identifier();
            let right = self.parse_identifier_name_error_on_unicode_escape_sequence();
            let node = self
                .factory
                .new_jsx_namespaced_name(Some(name), Some(right));
            return self.finish_node(node, pos);
        }
        name
    }
    /// port: tsc/internal/parser/parser.go:Parser.parseJsxAttributeValue
    pub(crate) fn parse_jsx_attribute_value(&mut self) -> Option<NodeId> {
        if self.token == SyntaxKind::EqualsToken {
            if self.scan_jsx_attribute_value() == SyntaxKind::StringLiteral {
                return Some(self.parse_literal_expression());
            }
            if self.token == SyntaxKind::OpenBraceToken {
                return self.parse_jsx_expression(true);
            }
            if self.token == SyntaxKind::LessThanToken {
                return Some(
                    self.parse_jsx_element_or_self_closing_element_or_fragment(
                        true, -1, None, false,
                    ),
                );
            }
            self.parse_error_at_current_token(diagnostics::X_or_JSX_element_expected, vec![]);
        }
        None
    }
    /// port: tsc/internal/parser/parser.go:Parser.parseJsxClosingFragment
    pub(crate) fn parse_jsx_closing_fragment(&mut self, expression_context: bool) -> NodeId {
        let pos = self.node_pos();
        self.parse_expected(SyntaxKind::LessThanSlashToken);
        if self.parse_expected_with_diagnostic(
            SyntaxKind::GreaterThanToken,
            Some(diagnostics::Expected_corresponding_closing_tag_for_JSX_fragment),
            false,
        ) {
            if expression_context {
                self.next_token();
            } else {
                self.scan_jsx_text();
            }
        }
        let node = self.factory.new_jsx_closing_fragment();
        self.finish_node(node, pos)
    }
    /// port: tsc/internal/ast/utilities.go:TagNamesAreEquivalent
    pub(crate) fn tag_names_are_equivalent(&self, mut left: NodeId, mut right: NodeId) -> bool {
        loop {
            let lhs = self.factory.node(left);
            let rhs = self.factory.node(right);
            if lhs.kind() != rhs.kind() {
                return false;
            }
            match lhs.kind().known() {
                Some(SyntaxKind::Identifier) => {
                    return lhs
                        .data_source()
                        .as_identifier()
                        .expect("identifier payload")
                        .text()
                        == rhs
                            .data_source()
                            .as_identifier()
                            .expect("identifier payload")
                            .text();
                }
                Some(SyntaxKind::ThisKeyword) => return true,
                Some(SyntaxKind::JsxNamespacedName) => {
                    let l = lhs
                        .data_source()
                        .as_jsx_namespaced_name()
                        .expect("namespaced payload");
                    let r = rhs
                        .data_source()
                        .as_jsx_namespaced_name()
                        .expect("namespaced payload");
                    return self.identifier_texts_equal(l.namespace(), r.namespace())
                        && self.identifier_texts_equal(l.name(), r.name());
                }
                Some(SyntaxKind::PropertyAccessExpression) => {
                    let l = lhs
                        .data_source()
                        .as_property_access_expression()
                        .expect("property access payload");
                    let r = rhs
                        .data_source()
                        .as_property_access_expression()
                        .expect("property access payload");
                    if !self.identifier_texts_equal(l.name(), r.name()) {
                        return false;
                    }
                    left = l.expression().expect("parsed property receiver");
                    right = r.expression().expect("parsed property receiver");
                }
                _ => panic!("Unhandled case in TagNamesAreEquivalent"),
            }
        }
    }
    fn identifier_texts_equal(&self, left: Option<NodeId>, right: Option<NodeId>) -> bool {
        self.factory
            .node(left.expect("parsed identifier"))
            .data_source()
            .as_identifier()
            .expect("identifier payload")
            .text()
            == self
                .factory
                .node(right.expect("parsed identifier"))
                .data_source()
                .as_identifier()
                .expect("identifier payload")
                .text()
    }
}
