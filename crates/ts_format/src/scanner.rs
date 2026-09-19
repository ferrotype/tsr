//! The formatting scanner (`format/scanner.go`): tokens with their leading and
//! trailing trivia. The scanner normally returns the smallest token it can, so
//! the node being processed decides when it must be greedier and rescan, as in
//! `>=`, a regular expression, a template part or JSX.

use crate::{debug_assert, Error};
use ts_arena::NodeId;
use ts_ast::{utilities_middle, AstView, NodeRead, SyntaxKind as K};
use ts_core::TextRange;
use ts_scanner::Scanner;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TextRangeWithKind {
    pub loc: TextRange,
    pub kind: K,
}

impl TextRangeWithKind {
    // port: tsc/internal/format/scanner.go:NewTextRangeWithKind
    pub fn new(pos: i64, end: i64, kind: K) -> Self {
        Self {
            loc: TextRange::new(pos, end),
            kind,
        }
    }
}

#[derive(Clone, Debug)]
pub(crate) struct TokenInfo {
    pub(crate) leading_trivia: Vec<TextRangeWithKind>,
    pub(crate) token: TextRangeWithKind,
    pub(crate) trailing_trivia: Vec<TextRangeWithKind>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ScanAction {
    Scan,
    RescanGreaterThanToken,
    RescanSlashToken,
    RescanTemplateToken,
    RescanJsxIdentifier,
    RescanJsxText,
    RescanJsxAttributeValue,
}

pub(crate) struct FormattingScanner<'src> {
    s: Scanner<'src>,
    start_pos: i64,
    end_pos: i64,
    saved_pos: i64,
    last_token_info: Option<TokenInfo>,
    last_scan_action: ScanAction,
    leading_trivia: Vec<TextRangeWithKind>,
    trailing_trivia: Vec<TextRangeWithKind>,
    was_new_line: bool,
}

fn is_trivia(kind: K) -> bool {
    utilities_middle::is_trivia(kind.into())
}

// port: tsc/internal/format/scanner.go:shouldRescanGreaterThanToken
fn should_rescan_greater_than_token(node: &NodeRead<'_>) -> bool {
    matches!(
        node.kind().known(),
        Some(
            K::GreaterThanEqualsToken
                | K::GreaterThanGreaterThanEqualsToken
                | K::GreaterThanGreaterThanGreaterThanEqualsToken
                | K::GreaterThanGreaterThanGreaterThanToken
                | K::GreaterThanGreaterThanToken
        )
    )
}

// port: tsc/internal/format/scanner.go:shouldRescanJsxIdentifier
fn should_rescan_jsx_identifier(view: AstView<'_>, node: &NodeRead<'_>) -> Result<bool, Error> {
    let Some(parent) = node.parent() else {
        return Ok(false);
    };
    let named = ts_ast::is_keyword_kind(node.kind()) || node.kind() == K::Identifier;
    Ok(match view.node(parent)?.kind().known() {
        // An identifier like `module-layout` is scanned as a keyword at first;
        // the whole thing has to be scanned to get the identifier.
        Some(
            K::JsxAttribute
            | K::JsxOpeningElement
            | K::JsxClosingElement
            | K::JsxSelfClosingElement
            | K::JsxNamespacedName,
        ) => named,
        // The leftmost name of a dotted JSX tag name (`a-b` in `<a-b.c>`) may
        // contain hyphens too.
        Some(K::PropertyAccessExpression) => named && is_leftmost_jsx_tag_name(view, node.id())?,
        _ => false,
    })
}

// port: tsc/internal/ast/utilities.go:IsJsxTagName
fn is_jsx_tag_name(view: AstView<'_>, node: &NodeRead<'_>) -> Result<bool, Error> {
    let Some(parent) = node.parent() else {
        return Ok(false);
    };
    let parent = view.node(parent)?;
    Ok(matches!(
        parent.kind().known(),
        Some(K::JsxOpeningElement | K::JsxClosingElement | K::JsxSelfClosingElement)
    ) && parent.tag_name() == Some(node.id()))
}

// port: tsc/internal/format/scanner.go:isLeftmostJsxTagName
fn is_leftmost_jsx_tag_name(view: AstView<'_>, node: NodeId) -> Result<bool, Error> {
    let mut current = node;
    loop {
        let read = view.node(current)?;
        let Some(parent) = read.parent() else {
            return Ok(false);
        };
        if is_jsx_tag_name(view, &read)? {
            return Ok(true);
        }
        let parent_read = view.node(parent)?;
        if parent_read.kind() == K::PropertyAccessExpression
            && parent_read.expression() == Some(current)
        {
            current = parent;
        } else {
            return Ok(false);
        }
    }
}

// port: tsc/internal/format/scanner.go:shouldRescanSlashToken
fn should_rescan_slash_token(container: &NodeRead<'_>) -> bool {
    container.kind() == K::RegularExpressionLiteral
}

// port: tsc/internal/format/scanner.go:shouldRescanTemplateToken
fn should_rescan_template_token(container: &NodeRead<'_>) -> bool {
    matches!(
        container.kind().known(),
        Some(K::TemplateMiddle | K::TemplateTail)
    )
}

// port: tsc/internal/format/scanner.go:shouldRescanJsxAttributeValue
fn should_rescan_jsx_attribute_value(
    view: AstView<'_>,
    node: &NodeRead<'_>,
) -> Result<bool, Error> {
    let Some(parent) = node.parent() else {
        return Ok(false);
    };
    let parent = view.node(parent)?;
    Ok(parent.kind() == K::JsxAttribute && parent.initializer() == Some(node.id()))
}

// port: tsc/internal/format/scanner.go:startsWithSlashToken
fn starts_with_slash_token(token: K) -> bool {
    matches!(token, K::SlashToken | K::SlashEqualsToken)
}

// port: tsc/internal/format/scanner.go:fixTokenKind
fn fix_token_kind(mut info: TokenInfo, container: &NodeRead<'_>) -> TokenInfo {
    if ts_ast::is_token_kind(container.kind()) {
        if let Some(kind) = container.kind().known() {
            if info.token.kind != kind {
                info.token.kind = kind;
            }
        }
    }
    info
}

impl<'src> FormattingScanner<'src> {
    /// The scanner over `[start_pos, end_pos)`. Upstream's constructor also runs
    /// the span worker and resets the scanner; the caller owns that here.
    // port: tsc/internal/format/scanner.go:newFormattingScanner
    pub(crate) fn new(
        text: &'src [u8],
        language_variant: ts_core::LanguageVariant,
        start_pos: i64,
        end_pos: i64,
    ) -> Self {
        let mut s = Scanner::new();
        s.set_skip_trivia(false);
        s.set_language_variant(language_variant);
        s.set_text(text);
        s.reset_token_state(start_pos);
        Self {
            s,
            start_pos,
            end_pos,
            saved_pos: 0,
            last_token_info: None,
            last_scan_action: ScanAction::Scan,
            leading_trivia: Vec::new(),
            trailing_trivia: Vec::new(),
            was_new_line: true,
        }
    }

    // port: tsc/internal/format/scanner.go:formattingScanner.advance
    pub(crate) fn advance(&mut self) {
        self.last_token_info = None;
        let is_started = self.s.token_full_start() != self.start_pos;
        if is_started {
            self.was_new_line = self
                .trailing_trivia
                .last()
                .is_some_and(|trivia| trivia.kind == K::NewLineTrivia);
        } else {
            self.s.scan();
        }
        self.leading_trivia.clear();
        self.trailing_trivia.clear();
        let mut pos = self.s.token_full_start();
        // Read the leading trivia up to the token.
        while pos < self.end_pos {
            let token = self.s.token();
            if !is_trivia(token) {
                break;
            }
            self.s.scan();
            let item = TextRangeWithKind::new(pos, self.s.token_full_start(), token);
            pos = self.s.token_full_start();
            self.leading_trivia.push(item);
        }
        self.saved_pos = self.s.token_full_start();
    }

    // port: tsc/internal/format/scanner.go:formattingScanner.shouldRescanJsxText
    fn should_rescan_jsx_text(&self, node: &NodeRead<'_>) -> bool {
        if node.kind() == K::JsxText {
            return true;
        }
        if node.kind() != K::JsxElement {
            return false;
        }
        self.last_token_info
            .as_ref()
            .is_some_and(|info| info.token.kind == K::JsxText)
    }

    // port: tsc/internal/format/scanner.go:formattingScanner.readTokenInfo
    pub(crate) fn read_token_info(
        &mut self,
        view: AstView<'_>,
        n: NodeId,
    ) -> Result<TokenInfo, Error> {
        debug_assert(self.is_on_token())?;
        let node = view.node(n)?;
        // The kind of the context node decides whether the scanner should be
        // greedier and consume more text.
        let expected = if should_rescan_greater_than_token(&node) {
            ScanAction::RescanGreaterThanToken
        } else if should_rescan_slash_token(&node) {
            ScanAction::RescanSlashToken
        } else if should_rescan_template_token(&node) {
            ScanAction::RescanTemplateToken
        } else if should_rescan_jsx_identifier(view, &node)? {
            ScanAction::RescanJsxIdentifier
        } else if self.should_rescan_jsx_text(&node) {
            ScanAction::RescanJsxText
        } else if should_rescan_jsx_attribute_value(view, &node)? {
            ScanAction::RescanJsxAttributeValue
        } else {
            ScanAction::Scan
        };

        if expected == self.last_scan_action {
            if let Some(info) = self.last_token_info.take() {
                // Asked before with the same scan action: the text does not
                // need rescanning. Fixing the kind is fine here because it does
                // not change how much text is consumed, which rescanning can.
                let info = fix_token_kind(info, &node);
                self.last_token_info = Some(info.clone());
                return Ok(info);
            }
        }

        if self.s.token_full_start() != self.saved_pos {
            // Asked before with another scan action: scan the text again.
            self.s.reset_token_state(self.saved_pos);
            self.s.scan();
        }

        let mut current = self.next_token(&node, expected)?;
        let token = TextRangeWithKind::new(self.s.token_full_start(), self.s.token_end(), current);

        // Consume the trailing trivia.
        self.trailing_trivia.clear();
        while self.s.token_full_start() < self.end_pos {
            current = self.s.scan();
            if !is_trivia(current) {
                break;
            }
            self.trailing_trivia.push(TextRangeWithKind::new(
                self.s.token_full_start(),
                self.s.token_end(),
                current,
            ));
            if current == K::NewLineTrivia {
                // Move past the new line.
                self.s.scan();
                break;
            }
        }

        let info = fix_token_kind(
            TokenInfo {
                leading_trivia: self.leading_trivia.clone(),
                token,
                trailing_trivia: self.trailing_trivia.clone(),
            },
            &node,
        );
        self.last_token_info = Some(info.clone());
        Ok(info)
    }

    // port: tsc/internal/format/scanner.go:formattingScanner.getNextToken
    fn next_token(&mut self, n: &NodeRead<'_>, expected: ScanAction) -> Result<K, Error> {
        let token = self.s.token();
        self.last_scan_action = ScanAction::Scan;
        match expected {
            ScanAction::RescanGreaterThanToken => {
                if token == K::GreaterThanToken {
                    self.last_scan_action = ScanAction::RescanGreaterThanToken;
                    let rescanned = self.s.rescan_greater_than_token();
                    debug_assert(n.kind() == rescanned)?;
                    return Ok(rescanned);
                }
            }
            ScanAction::RescanSlashToken => {
                if starts_with_slash_token(token) {
                    self.last_scan_action = ScanAction::RescanSlashToken;
                    let rescanned = self.s.rescan_slash_token(false);
                    debug_assert(n.kind() == rescanned)?;
                    return Ok(rescanned);
                }
            }
            ScanAction::RescanTemplateToken => {
                if token == K::CloseBraceToken {
                    self.last_scan_action = ScanAction::RescanTemplateToken;
                    return Ok(self.s.rescan_template_token(false));
                }
            }
            ScanAction::RescanJsxIdentifier => {
                self.last_scan_action = ScanAction::RescanJsxIdentifier;
                return Ok(self.s.scan_jsx_identifier());
            }
            ScanAction::RescanJsxText => {
                self.last_scan_action = ScanAction::RescanJsxText;
                return Ok(self.s.rescan_jsx_token(false));
            }
            ScanAction::RescanJsxAttributeValue => {
                self.last_scan_action = ScanAction::RescanJsxAttributeValue;
                return Ok(self.s.rescan_jsx_attribute_value());
            }
            ScanAction::Scan => {}
        }
        Ok(token)
    }

    // port: tsc/internal/format/scanner.go:formattingScanner.readEOFTokenRange
    pub(crate) fn read_eof_token_range(&self) -> Result<TextRangeWithKind, Error> {
        debug_assert(self.is_on_eof())?;
        Ok(TextRangeWithKind::new(
            self.s.token_full_start(),
            self.s.token_end(),
            K::EndOfFile,
        ))
    }

    fn current_kind(&self) -> K {
        self.last_token_info
            .as_ref()
            .map_or_else(|| self.s.token(), |info| info.token.kind)
    }

    // port: tsc/internal/format/scanner.go:formattingScanner.isOnToken
    pub(crate) fn is_on_token(&self) -> bool {
        let current = self.current_kind();
        current != K::EndOfFile && !is_trivia(current)
    }

    // port: tsc/internal/format/scanner.go:formattingScanner.isOnEOF
    pub(crate) fn is_on_eof(&self) -> bool {
        self.current_kind() == K::EndOfFile
    }

    fn skip_to(&mut self, position: i64) {
        self.s.reset_token_state(position);
        self.saved_pos = self.s.token_full_start();
        self.last_scan_action = ScanAction::Scan;
        self.last_token_info = None;
        self.was_new_line = false;
        self.leading_trivia.clear();
        self.trailing_trivia.clear();
    }

    // port: tsc/internal/format/scanner.go:formattingScanner.skipToEndOf
    pub(crate) fn skip_to_end_of(&mut self, range: TextRange) {
        self.skip_to(range.end());
    }

    // port: tsc/internal/format/scanner.go:formattingScanner.skipToStartOf
    pub(crate) fn skip_to_start_of(&mut self, range: TextRange) {
        self.skip_to(range.pos());
    }

    // port: tsc/internal/format/scanner.go:formattingScanner.getCurrentLeadingTrivia
    pub(crate) fn current_leading_trivia(&self) -> &[TextRangeWithKind] {
        &self.leading_trivia
    }

    // port: tsc/internal/format/scanner.go:formattingScanner.lastTrailingTriviaWasNewLine
    pub(crate) fn last_trailing_trivia_was_new_line(&self) -> bool {
        self.was_new_line
    }

    // port: tsc/internal/format/scanner.go:formattingScanner.getTokenFullStart
    pub(crate) fn token_full_start(&self) -> i64 {
        self.last_token_info
            .as_ref()
            .map_or_else(|| self.s.token_full_start(), |info| info.token.loc.pos())
    }

    /// The start of the token the underlying scanner is on, past its trivia.
    pub(crate) fn scanner_token_start(&self) -> i64 {
        self.s.token_start()
    }
}
