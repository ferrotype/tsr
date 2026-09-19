//! Literal text (`getLiteralText` and `canUseOriginalText` in
//! `tsc/internal/printer/utilities.go`): a parsed literal prints its source
//! bytes when they can be reached and are valid; a synthesized one prints its
//! canonical or escaped form. The escape worker itself is `ts_jsstring::escape`.

use crate::{Error, Session};
use ts_ast::{token_flags, NodeId, NodeRead, SyntaxKind as K};
use ts_jsstring::{escape::escape_string_with_flags, LiteralEscapeFlags, QuoteChar};

/// `getLiteralTextFlags`; the bit layout is shared with the escape worker.
pub type LiteralTextFlags = LiteralEscapeFlags;

fn union(flags: LiteralTextFlags, other: LiteralTextFlags) -> LiteralTextFlags {
    LiteralTextFlags::from_bits(flags.bits() | other.bits()).expect("known literal text flags")
}

// port: tsc/internal/printer/utilities.go:canUseOriginalText
pub(crate) fn can_use_original_text(node: &NodeRead<'_>, flags: LiteralTextFlags) -> bool {
    // A synthetic node has no original text, nor does a node without a parent as
    // the containing source file cannot be found. Unterminated literals lose
    // their original text when the caller asked for proper termination.
    if ts_ast::utilities::node_is_synthesized(node)
        || node.parent().is_none()
        || flags.contains(LiteralEscapeFlags::TERMINATE_UNTERMINATED_LITERALS)
            && ts_ast::utilities_middle::is_unterminated_literal(node)
    {
        return false;
    }
    if node.kind() == K::NumericLiteral {
        let token_flags = node
            .data_source()
            .as_numeric_literal()
            .map_or(0, |literal| literal.token_flags());
        // An invalid literal has no reusable original text.
        if token_flags & token_flags::IS_INVALID != 0 {
            return false;
        }
        // Separators are reusable only where the target permits them.
        if token_flags & token_flags::CONTAINS_SEPARATOR != 0 {
            return flags.contains(LiteralEscapeFlags::ALLOW_NUMERIC_SEPARATOR);
        }
    }
    // Upstream never reuses the original text of a BigInt literal.
    node.kind() != K::BigIntLiteral
}

impl Session<'_, '_> {
    /// Text of a literal-like node: the source bytes when reachable, otherwise the
    /// canonical form for numbers and a quoted, escaped form for string-likes.
    // port: tsc/internal/printer/utilities.go:getLiteralText
    pub(crate) fn get_literal_text(
        &self,
        node: NodeId,
        use_source: bool,
        flags: LiteralTextFlags,
    ) -> Result<Vec<u8>, Error> {
        let read = self.node(node)?;
        if use_source && can_use_original_text(&read, flags) {
            if let Some(text) = self.source_text() {
                return Ok(ts_scanner::get_text_of_node_from_source_text(
                    self.view,
                    text,
                    Some(node),
                    false,
                )?
                .as_bytes()
                .to_vec());
            }
        }
        let data = read.data_source();
        match read.kind().known() {
            Some(K::StringLiteral) => {
                let literal = data
                    .as_string_literal()
                    .ok_or(Error::MissingNode("string literal payload"))?;
                let quote = if literal.token_flags() & token_flags::SINGLE_QUOTE != 0 {
                    QuoteChar::Single
                } else {
                    QuoteChar::Double
                };
                let mut out = Vec::with_capacity(literal.text().len() + 2);
                out.push(quote as u8);
                out.extend_from_slice(&escape_string_with_flags(literal.text(), quote, flags));
                out.push(quote as u8);
                Ok(out)
            }
            Some(
                kind @ (K::NoSubstitutionTemplateLiteral
                | K::TemplateHead
                | K::TemplateMiddle
                | K::TemplateTail),
            ) => {
                let (text, raw_text) = match kind {
                    K::NoSubstitutionTemplateLiteral => {
                        let literal = data
                            .as_no_substitution_template_literal()
                            .ok_or(Error::MissingNode("template literal payload"))?;
                        (literal.text(), literal.raw_text())
                    }
                    K::TemplateHead => {
                        let literal = data
                            .as_template_head()
                            .ok_or(Error::MissingNode("template head payload"))?;
                        (literal.text(), literal.raw_text())
                    }
                    K::TemplateMiddle => {
                        let literal = data
                            .as_template_middle()
                            .ok_or(Error::MissingNode("template middle payload"))?;
                        (literal.text(), literal.raw_text())
                    }
                    _ => {
                        let literal = data
                            .as_template_tail()
                            .ok_or(Error::MissingNode("template tail payload"))?;
                        (literal.text(), literal.raw_text())
                    }
                };
                // Raw text, when present, is expected to be valid as written.
                let raw = !raw_text.is_empty() || text.is_empty();
                let mut out = Vec::with_capacity(3 + if raw { raw_text.len() } else { text.len() });
                out.extend_from_slice(match kind {
                    K::NoSubstitutionTemplateLiteral | K::TemplateHead => b"`",
                    _ => b"}",
                });
                if raw {
                    out.extend_from_slice(raw_text);
                } else {
                    out.extend_from_slice(&escape_string_with_flags(
                        text,
                        QuoteChar::Backtick,
                        flags,
                    ));
                }
                out.extend_from_slice(match kind {
                    K::NoSubstitutionTemplateLiteral | K::TemplateTail => b"`",
                    _ => b"${",
                });
                Ok(out)
            }
            Some(K::NumericLiteral) => Ok(data
                .as_numeric_literal()
                .ok_or(Error::MissingNode("numeric literal payload"))?
                .text()
                .to_vec()),
            Some(K::BigIntLiteral) => Ok(data
                .as_big_int_literal()
                .ok_or(Error::MissingNode("bigint literal payload"))?
                .text()
                .to_vec()),
            Some(K::RegularExpressionLiteral) => {
                let literal = data
                    .as_regular_expression_literal()
                    .ok_or(Error::MissingNode("regular expression payload"))?;
                let text = literal.text();
                if flags.contains(LiteralEscapeFlags::TERMINATE_UNTERMINATED_LITERALS)
                    && ts_ast::utilities_middle::is_unterminated_literal(&read)
                {
                    let mut out = text.to_vec();
                    out.extend_from_slice(if text.last() == Some(&b'\\') {
                        b" /"
                    } else {
                        b"/"
                    });
                    return Ok(out);
                }
                Ok(text.to_vec())
            }
            _ => Err(Error::UnexpectedKind {
                context: "getLiteralText",
                kind: read.kind(),
            }),
        }
    }

    /// Literal text with the node's emit flags and the printer's target applied.
    /// Upstream first consults the emit context's string-literal text sources,
    /// which arrive with the transforms; none exist here yet.
    // port: tsc/internal/printer/printer.go:Printer.getLiteralTextOfNode
    pub(crate) fn get_literal_text_of_node(
        &self,
        node: NodeId,
        mut flags: LiteralTextFlags,
    ) -> Result<Vec<u8>, Error> {
        if self.printer.emit_context.emit_flags(node) & crate::emit_flags::NO_ASCII_ESCAPING != 0 {
            flags = union(flags, LiteralEscapeFlags::NEVER_ASCII_ESCAPE);
        }
        if self.printer.options.target >= ts_core::ScriptTarget::ES2021 {
            flags = union(flags, LiteralEscapeFlags::ALLOW_NUMERIC_SEPARATOR);
        }
        self.get_literal_text(node, self.current_source.is_some(), flags)
    }
}

pub(crate) fn with_flag(flags: LiteralTextFlags, other: LiteralTextFlags) -> LiteralTextFlags {
    union(flags, other)
}
