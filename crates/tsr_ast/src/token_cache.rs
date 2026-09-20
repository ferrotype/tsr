//! The file's token cache: nodes for tokens the parsed tree does not store.
//!
//! Navigation and formatting ask for the token at a position. Punctuation and
//! keywords are not nodes of the parsed tree, so the token is built from the
//! scanner's result and cached per `(parent, range)`, so asking twice gives the
//! same node. The cache, its lock, the kind-mismatch check and the rejection of
//! reparsed parents live in the lazy arena (ownership note, section 2.4); this
//! module supplies the node.

use crate::{
    storage::AstTransaction, token_flags as tf, AstView, BigIntLiteralData, IdentifierData,
    JsString, JsxTextData, NoSubstitutionTemplateLiteralData, Node, NodeData, NodeId, NodeRead,
    NumericLiteralData, PrivateIdentifierData, RegularExpressionLiteralData, StringLiteralData,
    SyntaxKind, TemplateHeadData, TemplateMiddleData, TemplateTailData, TokenData, TokenFlags,
};
use tsr_arena::{Error, TokenKey};

impl<'a> AstView<'a> {
    /// Gets a token from the file's token cache, or creates it if it does not
    /// exist yet. It is for tokens that are in the file's text; a synthetic
    /// token has no place in this cache.
    // port: tsc/internal/ast/ast.go:SourceFile.GetOrCreateToken
    pub fn get_or_create_token(
        self,
        kind: SyntaxKind,
        pos: i32,
        end: i32,
        parent: NodeId,
        flags: TokenFlags,
    ) -> Result<NodeRead<'a>, Error> {
        let (Ok(start), Ok(finish)) = (usize::try_from(pos), usize::try_from(end)) else {
            return Err(Error::InvalidTokenRange);
        };
        let key = TokenKey {
            parent,
            start,
            end: finish,
        };
        let selected = self.0.for_node_owner(parent)?;
        let source = selected.physical_owner().source();
        let id =
            selected.try_token_prepared_id(key, u32::from(kind as u16), |storage, parent| {
                let text = source.get(start..finish).ok_or(Error::InvalidTokenRange)?;
                let mut node = create_token(kind, text, pos, end, flags)?;
                node.set_parent(Some(parent));
                Ok(AstTransaction::stage_token(storage, node))
            })?;
        self.node(id)
    }
}

/// `kind` should be a token kind. The payloads go through the same flag masks
/// as the node factory's constructors, which is what upstream calls here.
// port: tsc/internal/ast/ast.go:createToken
fn create_token(
    kind: SyntaxKind,
    text: &[u8],
    pos: i32,
    end: i32,
    flags: TokenFlags,
) -> Result<Node, Error> {
    use SyntaxKind as K;
    let text = || JsString::from_bytes(text);
    let data = match kind {
        K::NumericLiteral => NodeData::NumericLiteral(Box::new(NumericLiteralData {
            text: text(),
            token_flags: flags & tf::NUMERIC_LITERAL_FLAGS,
        })),
        K::BigIntLiteral => NodeData::BigIntLiteral(Box::new(BigIntLiteralData {
            text: text(),
            token_flags: flags & tf::NUMERIC_LITERAL_FLAGS,
        })),
        K::StringLiteral => NodeData::StringLiteral(Box::new(StringLiteralData {
            text: text(),
            token_flags: flags & tf::STRING_LITERAL_FLAGS,
        })),
        K::JsxText | K::JsxTextAllWhiteSpaces => NodeData::JsxText(Box::new(JsxTextData {
            text: text(),
            token_flags: 0,
            contains_only_trivia_white_spaces: kind == K::JsxTextAllWhiteSpaces,
        })),
        K::RegularExpressionLiteral => {
            NodeData::RegularExpressionLiteral(Box::new(RegularExpressionLiteralData {
                text: text(),
                token_flags: flags & tf::REGULAR_EXPRESSION_LITERAL_FLAGS,
            }))
        }
        K::NoSubstitutionTemplateLiteral => {
            NodeData::NoSubstitutionTemplateLiteral(Box::new(NoSubstitutionTemplateLiteralData {
                text: text(),
                token_flags: 0,
                raw_text: JsString::default(),
                template_flags: flags & tf::TEMPLATE_LITERAL_LIKE_FLAGS,
            }))
        }
        K::TemplateHead => NodeData::TemplateHead(Box::new(TemplateHeadData {
            text: text(),
            token_flags: 0,
            raw_text: JsString::default(),
            template_flags: flags & tf::TEMPLATE_LITERAL_LIKE_FLAGS,
        })),
        K::TemplateMiddle => NodeData::TemplateMiddle(Box::new(TemplateMiddleData {
            text: text(),
            token_flags: 0,
            raw_text: JsString::default(),
            template_flags: flags & tf::TEMPLATE_LITERAL_LIKE_FLAGS,
        })),
        K::TemplateTail => NodeData::TemplateTail(Box::new(TemplateTailData {
            text: text(),
            token_flags: 0,
            raw_text: JsString::default(),
            template_flags: flags & tf::TEMPLATE_LITERAL_LIKE_FLAGS,
        })),
        K::Identifier => NodeData::Identifier(IdentifierData { text: text() }),
        K::PrivateIdentifier => NodeData::PrivateIdentifier(PrivateIdentifierData { text: text() }),
        // Punctuation and keywords.
        _ => NodeData::Token(TokenData {}),
    };
    Node::new(kind, pos, end, data).map_err(|_| Error::InvalidGraph)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{AstBuilder, FactoryMethods};
    use tsr_arena::Counters;
    use tsr_jsstring::SourceText;

    fn published(text: &'static [u8]) -> (crate::AstFile, NodeId) {
        let mut build = AstBuilder::new(SourceText::from_loaded_bytes(text), &Counters::new());
        let root = build.new_token(SyntaxKind::EndOfFile.into());
        (build.complete(root).unwrap().publish_unbound(), root)
    }

    #[test]
    fn a_token_is_created_once_per_parent_and_range_with_its_parent_and_range() {
        let (file, parent) = published(b"a + 12");
        let view = file.view();
        let plus = view
            .get_or_create_token(SyntaxKind::PlusToken, 1, 3, parent, 0)
            .unwrap();
        assert_eq!(plus.kind().known(), Some(SyntaxKind::PlusToken));
        assert_eq!((plus.pos(), plus.end()), (1, 3));
        assert_eq!(plus.parent(), Some(parent));
        assert_ne!(
            plus.id().arena(),
            parent.arena(),
            "a node of the lazy arena"
        );
        let again = view
            .get_or_create_token(SyntaxKind::PlusToken, 1, 3, parent, 0)
            .unwrap();
        assert_eq!(again.id(), plus.id());
        let elsewhere = view
            .get_or_create_token(SyntaxKind::PlusToken, 2, 3, parent, 0)
            .unwrap();
        assert_ne!(elsewhere.id(), plus.id(), "the range is part of the key");
    }

    #[test]
    fn a_literal_token_carries_its_source_text_and_masked_flags() {
        let (file, parent) = published(b"a + 12");
        let view = file.view();
        let flags = tf::SCIENTIFIC | tf::PRECEDING_LINE_BREAK;
        let number = view
            .get_or_create_token(SyntaxKind::NumericLiteral, 3, 6, parent, flags)
            .unwrap();
        let data = number.as_numeric_literal().unwrap();
        assert_eq!(
            data.text(),
            b" 12",
            "the full-start range, as upstream slices it"
        );
        assert_eq!(data.token_flags(), tf::SCIENTIFIC);
        let name = view
            .get_or_create_token(SyntaxKind::Identifier, 0, 1, parent, 0)
            .unwrap();
        assert_eq!(name.as_identifier().unwrap().text(), b"a");
    }

    #[test]
    fn a_cached_token_of_another_kind_and_a_range_outside_the_text_are_rejected() {
        let (file, parent) = published(b"a + 12");
        let view = file.view();
        view.get_or_create_token(SyntaxKind::PlusToken, 1, 3, parent, 0)
            .unwrap();
        assert!(matches!(
            view.get_or_create_token(SyntaxKind::MinusToken, 1, 3, parent, 0),
            Err(Error::TokenKindMismatch { .. })
        ));
        for (pos, end) in [(4, 3), (0, 7), (-1, 2)] {
            assert_eq!(
                view.get_or_create_token(SyntaxKind::PlusToken, pos, end, parent, 0)
                    .err(),
                Some(Error::InvalidTokenRange),
                "{pos}..{end}"
            );
        }
    }
}
