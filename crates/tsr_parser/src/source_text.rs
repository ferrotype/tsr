use crate::{Parser, ParserFactory};
use std::borrow::Cow;
use ts_ast::{node_flags, token_flags, JsString, NodeId, SyntaxKind};

impl<F: ParserFactory> Parser<'_, F> {
    /// port: tsc/internal/scanner/utilities.go:GetTextOfNodeFromSourceText
    pub(crate) fn get_text_of_node_from_source_text(
        &self,
        id: NodeId,
        include_trivia: bool,
    ) -> JsString {
        if self.node_is_missing(Some(id)) {
            return JsString::default();
        }
        let node = self.factory.node(id);
        let loc = node.range();
        let pos = if include_trivia {
            loc.pos()
        } else {
            ts_scanner::skip_trivia(self.source_text, loc.pos())
        };
        let range = usize::try_from(pos).expect("source node start is nonnegative")
            ..usize::try_from(loc.end()).expect("source node end is nonnegative");
        let text = &self.source_text[range.clone()];
        let text = if self.is_jsdoc_type_expression_or_child(id) {
            ts_scanner::normalize_jsdoc_type_source_text(text)
        } else {
            Cow::Borrowed(text)
        };
        if node.flags() & node_flags::REPARSER_TRANSFORMED_LITERAL != 0 {
            if node.kind() == SyntaxKind::StringLiteral {
                let quote = if node
                    .data_source()
                    .as_string_literal()
                    .expect("string literal payload")
                    .token_flags()
                    & token_flags::SINGLE_QUOTE
                    != 0
                {
                    b'\''
                } else {
                    b'"'
                };
                let mut quoted = Vec::with_capacity(text.len() + 2);
                quoted.push(quote);
                quoted.extend_from_slice(&text);
                quoted.push(quote);
                return JsString::from_bytes(quoted);
            }
            if node.kind() == SyntaxKind::Identifier {
                return node
                    .data_source()
                    .as_identifier()
                    .expect("identifier payload")
                    .text_owned();
            }
            panic!(
                "Debug failure. Unexpected reparser-transformed node kind\nNode {} was unexpected.",
                node.kind()
            );
        }
        match text {
            Cow::Borrowed(_) => self
                .source_owner
                .slice(range)
                .expect("node range belongs to parser source"),
            Cow::Owned(text) => JsString::from_bytes(text),
        }
    }
    /// port: tsc/internal/scanner/utilities.go:isJSDocTypeExpressionOrChild
    fn is_jsdoc_type_expression_or_child(&self, id: NodeId) -> bool {
        let node = self.factory.node(id);
        if node.kind() == SyntaxKind::JSDocTypeExpression {
            return true;
        }
        if node.flags() & (node_flags::JS_DOC | node_flags::REPARSED) == 0 {
            return false;
        }
        let mut current = Some(id);
        while let Some(id) = current {
            let node = self.factory.node(id);
            if is_type_node_kind(node.kind()) {
                return true;
            }
            current = node.parent();
        }
        false
    }
}

/// port: tsc/internal/ast/utilities.go:IsTypeNodeKind
pub(crate) fn is_type_node_kind(kind: ts_ast::NodeKind) -> bool {
    matches!(
        kind.known(),
        Some(
            SyntaxKind::AnyKeyword
                | SyntaxKind::UnknownKeyword
                | SyntaxKind::NumberKeyword
                | SyntaxKind::BigIntKeyword
                | SyntaxKind::ObjectKeyword
                | SyntaxKind::BooleanKeyword
                | SyntaxKind::StringKeyword
                | SyntaxKind::SymbolKeyword
                | SyntaxKind::VoidKeyword
                | SyntaxKind::UndefinedKeyword
                | SyntaxKind::NeverKeyword
                | SyntaxKind::IntrinsicKeyword
                | SyntaxKind::ExpressionWithTypeArguments
                | SyntaxKind::JSDocAllType
                | SyntaxKind::JSDocNullableType
                | SyntaxKind::JSDocNonNullableType
                | SyntaxKind::JSDocOptionalType
                | SyntaxKind::JSDocVariadicType
        )
    ) || (SyntaxKind::FirstTypeNode as i16..=SyntaxKind::LastTypeNode as i16).contains(&kind.raw())
}
