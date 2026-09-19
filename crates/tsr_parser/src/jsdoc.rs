//! JSDoc parsing preserves scanner checkpoints and source-byte provenance.
use crate::tokens::token_is_identifier_or_keyword;
use crate::{JSDocInfo, Parser, ParserFactory, ParsingContext};
use std::sync::Arc;
use ts_ast::SyntaxKind as K;
use ts_ast::{node_flags, Diagnostic, FactoryMethods, JsString, NodeDataRead, NodeId, NodeListId};
use ts_core::TextRange;
use ts_diagnostics::{self as diagnostics, Message};
use ts_scanner::CommentRange;

#[derive(Clone, Copy, PartialEq)]
enum CommentState {
    BeginningOfLine,
    SawAsterisk,
    SavingComments,
    SavingBackticks,
}
const PROPERTY: u8 = 1;
const PARAMETER: u8 = 2;
const CALLBACK_PARAMETER: u8 = 4;

/// port: tsc/internal/parser/utilities.go:isJSDocLikeText
fn is_jsdoc_like_text(text: &[u8]) -> bool {
    text.len() >= 4 && text[1] == b'*' && text[2] == b'*' && text[3] != b'/'
}
/// port: tsc/internal/parser/utilities.go:GetJSDocCommentRanges
pub(crate) fn get_jsdoc_comment_ranges(
    factory: &impl ParserFactory,
    mut ranges: Vec<CommentRange>,
    node: NodeId,
    text: &[u8],
) -> Vec<CommentRange> {
    let node = factory.node(node);
    if matches!(
        node.kind().known(),
        Some(
            K::Parameter
                | K::TypeParameter
                | K::FunctionExpression
                | K::ArrowFunction
                | K::ParenthesizedExpression
                | K::VariableDeclaration
                | K::ExportSpecifier
        )
    ) {
        ranges.extend(ts_scanner::get_trailing_comment_ranges(
            text,
            node.range().pos(),
        ));
    }
    ranges.extend(ts_scanner::get_leading_comment_ranges(
        text,
        node.range().pos(),
    ));
    ranges.retain(|comment| {
        let start = comment.loc.pos() as usize;
        comment.loc.end() <= node.range().end()
            && comment.loc.len() >= 4
            && text[start + 1] == b'*'
            && text[start + 2] == b'*'
            && text[start + 3] != b'/'
    });
    ranges
}

impl<F: ParserFactory> Parser<'_, F> {
    /// port: tsc/internal/parser/jsdoc.go:Parser.withJSDoc
    pub(crate) fn with_js_doc(&mut self, node: NodeId, info: u8) -> Vec<NodeId> {
        if info & 1 == 0 {
            return Vec::new();
        }
        if !self.is_java_script() {
            self.factory.add_node_flags(
                node,
                node_flags::HAS_JS_DOC
                    | if info & 2 != 0 {
                        node_flags::POSSIBLY_CONTAINS_DEPRECATED_TAG
                    } else {
                        0
                    },
            );
            if info & 4 == 0 {
                return Vec::new();
            }
        }
        let scratch = std::mem::take(&mut self.jsdoc_comment_ranges_space);
        let mut ranges = get_jsdoc_comment_ranges(&self.factory, scratch, node, self.source_text);
        self.has_deprecated_tag = false;
        let mut docs = Vec::with_capacity(ranges.len());
        let mut pos = self.factory.node(node).range().pos();
        for comment in &ranges {
            if let Some(parsed) =
                self.parse_js_doc_comment(node, comment.loc.pos(), comment.loc.end(), pos)
            {
                self.factory.set_node_parent(parsed, Some(node));
                docs.push(parsed);
                pos = self.factory.node(parsed).range().end();
            }
        }
        ranges.clear();
        self.jsdoc_comment_ranges_space = ranges;
        if !docs.is_empty() {
            self.factory.add_node_flags(
                node,
                node_flags::HAS_JS_DOC
                    | if self.has_deprecated_tag {
                        node_flags::POSSIBLY_CONTAINS_DEPRECATED_TAG
                    } else {
                        0
                    },
            );
            self.has_deprecated_tag = false;
            if self.is_java_script() {
                self.reparse_tags(node, &docs);
            }
            self.jsdoc_infos.push(JSDocInfo {
                parent: node,
                js_docs: docs.clone(),
            });
        }
        docs
    }
    /// port: tsc/internal/parser/jsdoc.go:Parser.parseJSDocTypeExpression
    pub(crate) fn parse_js_doc_type_expression(&mut self, may_omit_braces: bool) -> NodeId {
        let pos = self.node_pos();
        let has_brace = if may_omit_braces {
            self.parse_optional(K::OpenBraceToken)
        } else {
            self.parse_expected(K::OpenBraceToken)
        };
        let saved = self.context_flags;
        self.set_context_flags(node_flags::JS_DOC, true);
        let ty = self.parse_js_doc_type();
        self.context_flags = saved;
        if has_brace {
            self.parse_expected_jsdoc(K::CloseBraceToken);
        }
        let node = self.factory.new_js_doc_type_expression(Some(ty));
        self.finish_node(node, pos)
    }
    /// port: tsc/internal/parser/jsdoc.go:Parser.parseJSDocNameReference
    fn parse_js_doc_name_reference(&mut self) -> NodeId {
        let pos = self.node_pos();
        let brace = self.parse_optional(K::OpenBraceToken);
        let name = self.parse_js_doc_link_name();
        if brace {
            self.parse_expected_jsdoc(K::CloseBraceToken);
        }
        self.scanner.reset_pos(self.scanner.token_full_start());
        self.next_token_jsdoc();
        let node = self.factory.new_js_doc_name_reference(name);
        self.finish_node(node, pos)
    }
    /// The saved scanner uses the same source owner; truncation never manufactures a new owner.
    /// port: tsc/internal/parser/jsdoc.go:Parser.parseJSDocComment
    pub(crate) fn parse_js_doc_comment(
        &mut self,
        _parent: NodeId,
        start: i64,
        mut end: i64,
        full_start: i64,
    ) -> Option<NodeId> {
        crate::recursion::guarded(|| {
            if end == -1 {
                end = self.source_text.len() as i64;
            }
            if !is_jsdoc_like_text(&self.source_text[start as usize..]) {
                return None;
            }
            let source = self.source_text;
            let token = self.token;
            let context = self.context_flags;
            let parsing = self.parsing_contexts;
            let scanner = self.scanner.mark();
            let errors = self.diagnostics.len();
            let has_error = self.has_parse_error;
            let has_await = self.statement_has_await_identifier;
            let line_start = source[..start as usize]
                .iter()
                .rposition(|&b| b == b'\n')
                .map_or(0, |p| p + 1);
            let indent = start + 4 - line_start as i64;
            self.source_text = &source[..end as usize - 2];
            self.scanner.set_text(self.source_text);
            self.scanner.reset_pos(start + 3);
            self.set_context_flags(node_flags::JS_DOC, true);
            self.parsing_contexts |= 1 << ParsingContext::JSDocComment as u32;
            let node = self.parse_js_doc_comment_worker(start, end, full_start, indent);
            if self.is_java_script() {
                self.jsdoc_diagnostics
                    .extend(self.diagnostics.drain(errors..));
            } else {
                self.diagnostics.truncate(errors);
            }
            self.source_text = source;
            self.scanner.set_text(source);
            self.context_flags = context;
            self.parsing_contexts = parsing;
            self.scanner.rewind(scanner);
            self.token = token;
            self.has_parse_error = has_error;
            self.statement_has_await_identifier = has_await;
            Some(node)
        })
    }
    fn jsdoc_token_text(&self) -> JsString {
        self.source_owner
            .slice(self.scanner.token_start() as usize..self.scanner.token_end() as usize)
            .expect("JSDoc token range belongs to source")
    }
    fn jsdoc_text_node(&mut self, comments: &[JsString], pos: i64, end: i64) -> NodeId {
        // stringSliceArena.Clone returns nil for an empty input, including the
        // text node emitted immediately before a leading JSDoc link.
        let text = if comments.is_empty() {
            ts_ast::TextSlice::empty()
        } else {
            self.factory.alloc_text(comments.to_vec())
        };
        let node = self.factory.new_js_doc_text(text);
        self.finish_node_with_end(node, pos, end)
    }
    /// port: tsc/internal/parser/jsdoc.go:Parser.parseJSDocCommentWorker
    fn parse_js_doc_comment_worker(
        &mut self,
        start: i64,
        end: i64,
        full_start: i64,
        mut indent: i64,
    ) -> NodeId {
        use CommentState::{BeginningOfLine, SavingBackticks, SavingComments, SawAsterisk};
        let mut tags = Vec::with_capacity(1);
        let (mut tags_pos, mut tags_end) = (-1, -1);
        let mut state = SawAsterisk;
        let mut backticks = 0;
        let mut fenced = false;
        let mut parts = Vec::with_capacity(1);
        let mut comments = std::mem::take(&mut self.jsdoc_comments_space);
        let mut comments_pos = -1;
        let mut link_end = start;
        let mut margin = -1;
        self.next_token_jsdoc();
        while self.parse_optional_jsdoc(K::WhitespaceTrivia) {}
        if self.parse_optional_jsdoc(K::NewLineTrivia) {
            state = BeginningOfLine;
            indent = 0;
        }
        loop {
            if self.token != K::BacktickToken && backticks > 0 {
                if backticks >= 3 {
                    fenced = !fenced;
                }
                backticks = 0;
            }
            match self.token {
                K::AtToken if !fenced && self.scanner.can_follow_jsdoc_at() => {
                    remove_trailing_whitespace(&mut comments);
                    if comments_pos == -1 {
                        comments_pos = self.node_pos();
                    }
                    let tag = self.parse_tag(&tags, indent);
                    if tags_pos == -1 {
                        tags_pos = self.factory.node(tag).range().pos();
                    }
                    tags.push(tag);
                    tags_end = self.factory.node(tag).range().end();
                    state = BeginningOfLine;
                    margin = -1;
                }
                K::NewLineTrivia => {
                    comments.push(self.jsdoc_token_text());
                    state = BeginningOfLine;
                    indent = 0;
                }
                K::AsteriskToken => {
                    let text = self.jsdoc_token_text();
                    if state == SawAsterisk {
                        state = SavingComments;
                        push_comment(&mut comments, &mut indent, &mut margin, text);
                    } else {
                        assert!(state == BeginningOfLine, "state must be BeginningOfLine");
                        state = SawAsterisk;
                        indent += text.len() as i64;
                    }
                }
                K::WhitespaceTrivia => {
                    assert!(
                        state != SavingComments && state != SavingBackticks,
                        "whitespace shouldn't come from the scanner while saving top-level comment text"
                    );
                    let text = self.jsdoc_token_text();
                    if margin > -1 && indent + text.len() as i64 > margin {
                        let mut existing = margin - indent;
                        if existing < 0 {
                            existing += text.len() as i64;
                        }
                        comments.push(
                            text.slice(existing.max(0) as usize..text.len())
                                .expect("indent within whitespace"),
                        );
                    }
                    indent += text.len() as i64;
                }
                K::EndOfFile => break,
                K::BacktickToken => {
                    backticks += 1;
                    state = if state == SavingBackticks {
                        SavingComments
                    } else {
                        SavingBackticks
                    };
                    push_comment(
                        &mut comments,
                        &mut indent,
                        &mut margin,
                        self.jsdoc_token_text(),
                    );
                }
                K::OpenBraceToken if !fenced => {
                    state = SavingComments;
                    let comment_end = self.scanner.token_full_start();
                    if let Some(link) = self.parse_js_doc_link(self.scanner.token_end() - 1) {
                        if link_end == start {
                            remove_leading_newlines(&mut comments);
                        }
                        parts.push(self.jsdoc_text_node(&comments, link_end, comment_end));
                        parts.push(link);
                        comments.clear();
                        link_end = self.scanner.token_end();
                    } else {
                        push_comment(
                            &mut comments,
                            &mut indent,
                            &mut margin,
                            self.jsdoc_token_text(),
                        );
                    }
                }
                _ => {
                    if self.token == K::AtToken || state != SavingBackticks {
                        state = if fenced {
                            SavingBackticks
                        } else {
                            SavingComments
                        };
                    }
                    let text = if self.token == K::JSDocCommentTextToken {
                        self.token_value()
                    } else {
                        self.jsdoc_token_text()
                    };
                    push_comment(&mut comments, &mut indent, &mut margin, text);
                }
            }
            if state == SavingComments || state == SavingBackticks {
                self.next_jsdoc_comment_text_token(state == SavingBackticks);
            } else {
                self.next_token_jsdoc();
            }
        }
        if comments_pos == -1 {
            comments_pos = self.scanner.token_full_start();
        }
        if let Some(last) = comments.last_mut() {
            *last = trim_space_end(last, unicode_is_space);
            parts.push(self.jsdoc_text_node(&comments, link_end, comments_pos));
        }
        comments.clear();
        self.jsdoc_comments_space = comments;
        assert!(
            parts.is_empty() || tags.is_empty() || comments_pos != -1,
            "having parsed tags implies that the end of the comment span should be set"
        );
        let tags =
            (tags_pos != -1).then(|| self.new_node_list(TextRange::new(tags_pos, tags_end), tags));
        // Go passes NewSlice(1)[:0] directly, without Arena.Clone's empty-to-nil
        // conversion used by ordinary parser lists. Keep that visible backing.
        let parts = self
            .factory
            .alloc_nodes(parts.into_iter().map(Some).collect());
        let comment = self
            .factory
            .alloc_list(TextRange::new(start, comments_pos), parts);
        let node = self.factory.new_js_doc(Some(comment), tags);
        self.finish_node_with_end(node, full_start, end)
    }
    /// port: tsc/internal/parser/jsdoc.go:Parser.isNextNonwhitespaceTokenEndOfFile
    fn is_next_nonwhitespace_token_end_of_file(&mut self) -> bool {
        loop {
            match self.next_token_jsdoc() {
                K::EndOfFile => return true,
                K::WhitespaceTrivia | K::NewLineTrivia => {}
                _ => return false,
            }
        }
    }
    /// port: tsc/internal/parser/jsdoc.go:Parser.skipWhitespace
    pub(crate) fn skip_whitespace(&mut self) {
        if matches!(self.token, K::WhitespaceTrivia | K::NewLineTrivia)
            && self.look_ahead(Self::is_next_nonwhitespace_token_end_of_file)
        {
            return;
        }
        while matches!(self.token, K::WhitespaceTrivia | K::NewLineTrivia) {
            self.next_token_jsdoc();
        }
    }
    /// port: tsc/internal/parser/jsdoc.go:Parser.skipWhitespaceOrAsterisk
    pub(crate) fn skip_whitespace_or_asterisk(&mut self) -> JsString {
        if matches!(self.token, K::WhitespaceTrivia | K::NewLineTrivia)
            && self.look_ahead(Self::is_next_nonwhitespace_token_end_of_file)
        {
            return JsString::default();
        }
        let mut preceding = self.scanner.has_preceding_line_break();
        let mut seen = false;
        let mut indent = Vec::new();
        while preceding && self.token == K::AsteriskToken
            || matches!(self.token, K::WhitespaceTrivia | K::NewLineTrivia)
        {
            indent.extend_from_slice(self.scanner.token_text());
            if self.token == K::NewLineTrivia {
                preceding = true;
                seen = true;
                indent.clear();
            } else if self.token == K::AsteriskToken {
                preceding = false;
            }
            self.next_token_jsdoc();
        }
        if seen {
            JsString::from_bytes(indent)
        } else {
            JsString::default()
        }
    }
    /// port: tsc/internal/parser/jsdoc.go:Parser.parseTrailingTagComments
    fn parse_trailing_tag_comments(
        &mut self,
        pos: i64,
        end: i64,
        mut margin: i64,
        indent: JsString,
    ) -> Option<NodeListId> {
        if indent.is_empty() {
            margin += end - pos;
        }
        let initial = if margin == 0 {
            indent
        } else if margin < indent.len() as i64 {
            indent
                .slice(margin as usize..indent.len())
                .expect("tag margin within indentation")
        } else {
            JsString::default()
        };
        self.parse_tag_comments(margin, Some(initial))
    }
    /// port: tsc/internal/parser/jsdoc.go:Parser.parseTagComments
    fn parse_tag_comments(
        &mut self,
        mut indent: i64,
        initial: Option<JsString>,
    ) -> Option<NodeListId> {
        use CommentState::{BeginningOfLine, SavingBackticks, SavingComments, SawAsterisk};
        let pos = self.node_pos();
        let mut comments = std::mem::take(&mut self.jsdoc_tag_comments_space);
        let mut parts = std::mem::take(&mut self.jsdoc_tag_comments_parts_space);
        let mut link_end = -1;
        let mut state = BeginningOfLine;
        let mut backticks = 0;
        let mut fenced = false;
        assert!(indent >= 0, "indent must be a natural number");
        let mut margin = -1;
        if let Some(initial) = initial {
            if !initial.is_empty() {
                push_comment(&mut comments, &mut indent, &mut margin, initial);
            }
            state = SawAsterisk;
        }
        loop {
            if self.token != K::BacktickToken && backticks > 0 {
                if backticks >= 3 {
                    fenced = !fenced;
                }
                backticks = 0;
            }
            match self.token {
                K::NewLineTrivia => {
                    state = BeginningOfLine;
                    comments.push(self.jsdoc_token_text());
                    indent = 0;
                }
                K::AtToken if !fenced && self.scanner.can_follow_jsdoc_at() => {
                    self.scanner.reset_pos(self.scanner.token_end() - 1);
                    break;
                }
                K::EndOfFile => break,
                K::WhitespaceTrivia => {
                    assert!(
                        state != SavingComments && state != SavingBackticks,
                        "whitespace shouldn't come from the scanner while saving comment text"
                    );
                    let text = self.jsdoc_token_text();
                    if margin > -1 && indent + text.len() as i64 > margin {
                        comments.push(
                            text.slice((margin - indent).max(0) as usize..text.len())
                                .expect("tag indentation within whitespace"),
                        );
                        state = if fenced {
                            SavingBackticks
                        } else {
                            SavingComments
                        };
                    }
                    indent += text.len() as i64;
                }
                K::OpenBraceToken if !fenced => {
                    state = SavingComments;
                    let end = self.scanner.token_full_start();
                    if let Some(link) = self.parse_js_doc_link(self.scanner.token_end() - 1) {
                        parts.push(self.jsdoc_text_node(
                            &comments,
                            if link_end > -1 { link_end } else { pos },
                            end,
                        ));
                        parts.push(link);
                        comments.clear();
                        link_end = self.scanner.token_end();
                    } else {
                        push_comment(
                            &mut comments,
                            &mut indent,
                            &mut margin,
                            self.jsdoc_token_text(),
                        );
                    }
                }
                K::BacktickToken => {
                    backticks += 1;
                    state = if state == SavingBackticks {
                        SavingComments
                    } else {
                        SavingBackticks
                    };
                    push_comment(
                        &mut comments,
                        &mut indent,
                        &mut margin,
                        self.jsdoc_token_text(),
                    );
                }
                K::AsteriskToken if state == BeginningOfLine => {
                    state = SawAsterisk;
                    indent += 1;
                }
                _ => {
                    if self.token == K::AtToken || state != SavingBackticks {
                        state = if fenced {
                            SavingBackticks
                        } else {
                            SavingComments
                        };
                    }
                    let text = if self.token == K::JSDocCommentTextToken {
                        self.token_value()
                    } else {
                        self.jsdoc_token_text()
                    };
                    push_comment(&mut comments, &mut indent, &mut margin, text);
                }
            }
            if state == SavingComments || state == SavingBackticks {
                self.next_jsdoc_comment_text_token(state == SavingBackticks);
            } else {
                self.next_token_jsdoc();
            }
        }
        remove_leading_newlines(&mut comments);
        remove_trailing_whitespace(&mut comments);
        if !comments.is_empty() {
            parts.push(self.jsdoc_text_node(
                &comments,
                if link_end > -1 { link_end } else { pos },
                self.node_pos(),
            ));
        }
        comments.clear();
        self.jsdoc_tag_comments_space = comments;
        let result = if parts.is_empty() {
            None
        } else {
            Some(self.new_node_list(TextRange::new(pos, self.scanner.token_end()), parts.clone()))
        };
        parts.clear();
        self.jsdoc_tag_comments_parts_space = parts;
        result
    }
    /// port: tsc/internal/parser/jsdoc.go:Parser.parseJSDocLink
    fn parse_js_doc_link(&mut self, start: i64) -> Option<NodeId> {
        let state = self.mark();
        let Some(kind) = self.parse_js_doc_link_prefix() else {
            self.rewind(state);
            return None;
        };
        self.commit(state);
        self.next_token_jsdoc();
        self.skip_whitespace();
        let name = self.parse_js_doc_link_name();
        let mut text = Vec::new();
        while !matches!(
            self.token,
            K::CloseBraceToken | K::NewLineTrivia | K::EndOfFile
        ) {
            text.push(self.jsdoc_token_text());
            self.next_token_jsdoc();
        }
        // Go starts with a nil slice and only allocates after a text token.
        let text = if text.is_empty() {
            ts_ast::TextSlice::empty()
        } else {
            self.factory.alloc_text(text)
        };
        let node = match kind.as_bytes() {
            b"link" => self.factory.new_js_doc_link(name, text),
            b"linkcode" => self.factory.new_js_doc_link_code(name, text),
            _ => self.factory.new_js_doc_link_plain(name, text),
        };
        Some(self.finish_node_with_end(node, start, self.scanner.token_end()))
    }
    /// port: tsc/internal/parser/jsdoc.go:Parser.parseJSDocLinkName
    fn parse_js_doc_link_name(&mut self) -> Option<NodeId> {
        if !token_is_identifier_or_keyword(self.token) {
            return None;
        }
        let pos = self.node_pos();
        let mut name = self.parse_identifier_name();
        while self.parse_optional(K::DotToken) {
            let right = if self.token == K::PrivateIdentifier {
                self.create_missing_identifier()
            } else {
                self.parse_identifier_name()
            };
            let node = self.factory.new_qualified_name(Some(name), Some(right));
            name = self.finish_node(node, pos);
        }
        while self.token == K::PrivateIdentifier {
            self.scan_operation(ts_scanner::Scanner::rescan_hash_token);
            self.next_token_jsdoc();
            let right = self.parse_identifier();
            let node = self.factory.new_qualified_name(Some(name), Some(right));
            name = self.finish_node(node, pos);
        }
        Some(name)
    }
    /// port: tsc/internal/parser/jsdoc.go:Parser.parseJSDocLinkPrefix
    fn parse_js_doc_link_prefix(&mut self) -> Option<JsString> {
        self.skip_whitespace_or_asterisk();
        if self.token == K::OpenBraceToken
            && self.next_token_jsdoc() == K::AtToken
            && token_is_identifier_or_keyword(self.next_token_jsdoc())
        {
            let value = self.token_value();
            if is_js_doc_link_tag(value.as_bytes()) {
                return Some(value);
            }
        }
        None
    }
    /// port: tsc/internal/parser/jsdoc.go:Parser.parseOptionalJsdoc
    fn parse_optional_jsdoc(&mut self, kind: K) -> bool {
        if self.token == kind {
            self.next_token_jsdoc();
            true
        } else {
            false
        }
    }
    /// port: tsc/internal/parser/jsdoc.go:Parser.parseJSDocIdentifierName
    fn parse_js_doc_identifier_name(&mut self, diagnostic: Option<&'static Message>) -> NodeId {
        if !token_is_identifier_or_keyword(self.token) {
            if let Some(message) = diagnostic {
                self.parse_error_at_current_token(message, vec![]);
            } else if crate::identifiers::is_reserved_word(self.token) {
                self.parse_error_at_current_token(
                    diagnostics::Identifier_expected_0_is_a_reserved_word_that_cannot_be_used_here,
                    vec![self.jsdoc_token_text()],
                );
            }
            let node = self.new_identifier(JsString::default());
            return self.finish_node(node, self.node_pos());
        }
        let pos = self.scanner.token_start();
        let end = self.scanner.token_end();
        let text = self.token_value();
        self.next_token_jsdoc();
        let node = self.new_identifier(text);
        self.finish_node_with_end(node, pos, end)
    }
}
fn push_comment(comments: &mut Vec<JsString>, indent: &mut i64, margin: &mut i64, text: JsString) {
    if *margin == -1 {
        *margin = *indent;
    }
    *indent += text.len() as i64;
    comments.push(text);
}
/// port: tsc/internal/parser/jsdoc.go:isJSDocLinkTag
fn is_js_doc_link_tag(kind: &[u8]) -> bool {
    matches!(kind, b"link" | b"linkcode" | b"linkplain")
}
/// port: tsc/internal/parser/jsdoc.go:removeLeadingNewlines
fn remove_leading_newlines(comments: &mut Vec<JsString>) {
    let count = comments
        .iter()
        .take_while(|s| s.as_bytes().iter().all(|&b| b == b'\r' || b == b'\n'))
        .count();
    comments.drain(..count);
}
fn unicode_is_space(ch: i32) -> bool {
    matches!(ch, 0x09..=0x0d | 0x20 | 0x85 | 0xa0 | 0x1680 | 0x2000..=0x200a | 0x2028 | 0x2029 | 0x202f | 0x205f | 0x3000)
}
fn trim_space_end(text: &JsString, space: fn(i32) -> bool) -> JsString {
    let mut end = text.len();
    while end > 0 {
        let bytes = &text.as_bytes()[..end];
        let mut start = end - 1;
        if bytes[start] >= 0x80 {
            let limit = end.saturating_sub(4);
            while start > limit && bytes[start] & 0xc0 == 0x80 {
                start -= 1;
            }
        }
        let (rune, width) = ts_jsstring::wtf8::decode_utf8(&bytes[start..]);
        if start + width != end || !space(rune) {
            break;
        }
        end = start;
    }
    text.slice(0..end).expect("trim is a source prefix")
}

/// port: tsc/internal/parser/jsdoc.go:trimEnd
fn trim_end(text: &JsString) -> JsString {
    trim_space_end(text, ts_scanner::is_white_space_like)
}
/// port: tsc/internal/parser/jsdoc.go:removeTrailingWhitespace
fn remove_trailing_whitespace(comments: &mut Vec<JsString>) {
    while let Some(last) = comments.last_mut() {
        *last = trim_end(last);
        if !last.is_empty() {
            break;
        }
        comments.pop();
    }
}

impl<F: ParserFactory> Parser<'_, F> {
    /// port: tsc/internal/parser/jsdoc.go:Parser.parseTag
    fn parse_tag(&mut self, tags: &[NodeId], margin: i64) -> NodeId {
        crate::recursion::guarded(|| {
            assert!(
                self.token == K::AtToken,
                "should be called only at the start of a tag"
            );
            let start = self.scanner.token_start();
            self.next_token_jsdoc();
            let name = self.parse_js_doc_identifier_name(Some(diagnostics::Identifier_expected));
            let indent = self.skip_whitespace_or_asterisk();
            match self.jsdoc_text(name).as_bytes() {
                b"implements" => self.parse_implements_tag(start, name, margin, indent),
                b"augments" | b"extends" => self.parse_augments_tag(start, name, margin, indent),
                b"public" => {
                    self.parse_simple_tag(start, F::new_js_doc_public_tag, name, margin, indent)
                }
                b"private" => {
                    self.parse_simple_tag(start, F::new_js_doc_private_tag, name, margin, indent)
                }
                b"protected" => {
                    self.parse_simple_tag(start, F::new_js_doc_protected_tag, name, margin, indent)
                }
                b"readonly" => {
                    self.parse_simple_tag(start, F::new_js_doc_readonly_tag, name, margin, indent)
                }
                b"override" => {
                    self.parse_simple_tag(start, F::new_js_doc_override_tag, name, margin, indent)
                }
                b"deprecated" => {
                    self.has_deprecated_tag = true;
                    self.parse_simple_tag(start, F::new_js_doc_deprecated_tag, name, margin, indent)
                }
                b"this" => self.parse_this_tag(start, name, margin, indent),
                b"arg" | b"argument" | b"param" => {
                    self.parse_parameter_or_property_tag(start, name, PARAMETER, margin)
                }
                b"return" | b"returns" => self.parse_return_tag(tags, start, name, margin, indent),
                b"template" => self.parse_template_tag(start, name, margin, indent),
                b"type" => self.parse_type_tag(tags, start, name, margin, indent),
                b"typedef" => self.parse_typedef_tag(start, name, margin, indent),
                b"callback" => self.parse_callback_tag(start, name, margin, indent),
                b"overload" => self.parse_overload_tag(start, name, margin, indent),
                b"satisfies" => self.parse_satisfies_tag(start, name, margin, indent),
                b"see" => self.parse_see_tag(start, name, margin, indent),
                b"exception" | b"throws" => self.parse_throws_tag(start, name, margin, indent),
                b"import" => self.parse_import_tag(start, name, margin, indent),
                _ => self.parse_unknown_tag(start, name, margin, indent),
            }
        })
    }
    /// port: tsc/internal/parser/jsdoc.go:Parser.parseUnknownTag
    fn parse_unknown_tag(
        &mut self,
        start: i64,
        name: NodeId,
        indent: i64,
        indent_text: JsString,
    ) -> NodeId {
        let comments =
            self.parse_trailing_tag_comments(start, self.node_pos(), indent, indent_text);
        let node = self.factory.new_js_doc_unknown_tag(Some(name), comments);
        self.finish_node(node, start)
    }
    /// port: tsc/internal/parser/jsdoc.go:Parser.tryParseTypeExpression
    fn try_parse_type_expression(&mut self) -> Option<NodeId> {
        self.skip_whitespace_or_asterisk();
        (self.token == K::OpenBraceToken).then(|| self.parse_js_doc_type_expression(false))
    }
    /// port: tsc/internal/parser/jsdoc.go:Parser.parseBracketNameInPropertyAndParamTag
    fn parse_bracket_name_in_property_and_param_tag(&mut self, target: u8) -> (NodeId, bool) {
        let bracketed = self.parse_optional_jsdoc(K::OpenBracketToken);
        if bracketed {
            self.skip_whitespace();
        }
        let backquoted = self.parse_optional_jsdoc(K::BacktickToken);
        let name = self.parse_js_doc_entity_name(
            (target != PARAMETER).then_some(diagnostics::Identifier_expected),
        );
        if backquoted {
            self.parse_expected_token_jsdoc(K::BacktickToken);
        }
        if bracketed {
            self.skip_whitespace();
            if self.parse_optional_token(K::EqualsToken).is_some() {
                self.parse_expression();
            }
            self.parse_expected(K::CloseBracketToken);
        }
        (name, bracketed)
    }
    /// port: tsc/internal/parser/jsdoc.go:isObjectOrObjectArrayTypeReference
    pub(crate) fn is_object_or_object_array_type_reference(&self, mut node: NodeId) -> bool {
        loop {
            let view = self.factory.node(node);
            match view.kind().known() {
                Some(K::ObjectKeyword) => return true,
                Some(K::ArrayType) => {
                    node = view
                        .data_source()
                        .as_array_type_node()
                        .expect("array type")
                        .element_type()
                        .expect("array element type");
                }
                Some(K::TypeReference) => {
                    let data = view
                        .data_source()
                        .as_type_reference_node()
                        .expect("type reference");
                    return data.type_arguments().is_none()
                        && data.type_name().is_some_and(|name| {
                            self.factory.node(name).kind() == K::Identifier
                                && self.jsdoc_text(name).as_bytes() == b"Object"
                        });
                }
                _ => return false,
            }
        }
    }
    /// port: tsc/internal/parser/jsdoc.go:Parser.parseParameterOrPropertyTag
    fn parse_parameter_or_property_tag(
        &mut self,
        start: i64,
        tag_name: NodeId,
        target: u8,
        indent: i64,
    ) -> NodeId {
        crate::recursion::guarded(|| {
            let mut ty = self.try_parse_type_expression();
            let mut name_first = ty.is_none();
            self.skip_whitespace_or_asterisk();
            let (name, bracketed) = self.parse_bracket_name_in_property_and_param_tag(target);
            let indent_text = self.skip_whitespace_or_asterisk();
            if name_first && self.look_ahead(|p| p.parse_js_doc_link_prefix().is_none()) {
                ty = self.try_parse_type_expression();
            }
            let comment =
                self.parse_trailing_tag_comments(start, self.node_pos(), indent, indent_text);
            if let Some(nested) = self.parse_nested_type_literal(ty, name, target, indent) {
                ty = Some(nested);
                name_first = true;
            }
            let kind = if target == PROPERTY {
                K::JSDocPropertyTag
            } else {
                K::JSDocParameterTag
            };
            let node = self.factory.new_js_doc_parameter_or_property_tag(
                kind.into(),
                Some(tag_name),
                Some(name),
                bracketed,
                ty,
                name_first,
                comment,
            );
            self.finish_node(node, start)
        })
    }
    /// port: tsc/internal/parser/jsdoc.go:Parser.parseNestedTypeLiteral
    fn parse_nested_type_literal(
        &mut self,
        ty: Option<NodeId>,
        name: NodeId,
        target: u8,
        indent: i64,
    ) -> Option<NodeId> {
        let ty = ty?;
        let inner = self.jsdoc_type(ty).expect("JSDoc type expression type");
        if !self.is_object_or_object_array_type_reference(inner) {
            return None;
        }
        let pos = self.node_pos();
        let mut children = Vec::new();
        loop {
            let state = self.mark();
            let Some(child) =
                self.parse_child_parameter_or_property_tag(target, indent, Some(name))
            else {
                self.rewind(state);
                break;
            };
            self.commit(state);
            match self.factory.node(child).kind().known() {
                Some(K::JSDocParameterTag | K::JSDocPropertyTag) => children.push(child),
                Some(K::JSDocTemplateTag) => {
                    let loc = self.factory.node(self.jsdoc_tag_name(child)).range();
                    self.parse_error_at_range(loc, diagnostics::A_JSDoc_template_tag_may_not_follow_a_typedef_callback_or_overload_tag, vec![]);
                }
                _ => {}
            }
        }
        if children.is_empty() {
            return None;
        }
        let nodes = self
            .factory
            .alloc_nodes(children.into_iter().map(Some).collect());
        let literal = self
            .factory
            .new_js_doc_type_literal(nodes, self.factory.node(inner).kind() == K::ArrayType);
        self.finish_node(literal, pos);
        let node = self.factory.new_js_doc_type_expression(Some(literal));
        Some(self.finish_node(node, pos))
    }
    /// port: tsc/internal/parser/jsdoc.go:Parser.parseReturnTag
    fn parse_return_tag(
        &mut self,
        tags: &[NodeId],
        start: i64,
        name: NodeId,
        indent: i64,
        text: JsString,
    ) -> NodeId {
        if tags
            .iter()
            .any(|&tag| self.factory.node(tag).kind() == K::JSDocReturnTag)
        {
            self.duplicate_jsdoc_tag(name);
        }
        let ty = self.try_parse_type_expression();
        let comments = self.parse_trailing_tag_comments(start, self.node_pos(), indent, text);
        let node = self.factory.new_js_doc_return_tag(Some(name), ty, comments);
        self.finish_node(node, start)
    }
    /// port: tsc/internal/parser/jsdoc.go:Parser.parseTypeTag
    fn parse_type_tag(
        &mut self,
        tags: &[NodeId],
        start: i64,
        name: NodeId,
        indent: i64,
        text: JsString,
    ) -> NodeId {
        if tags
            .iter()
            .any(|&tag| self.factory.node(tag).kind() == K::JSDocTypeTag)
        {
            self.duplicate_jsdoc_tag(name);
        }
        let ty = self.parse_js_doc_type_expression(true);
        let comments = (indent != -1)
            .then(|| self.parse_trailing_tag_comments(start, self.node_pos(), indent, text))
            .flatten();
        let node = self
            .factory
            .new_js_doc_type_tag(Some(name), Some(ty), comments);
        self.finish_node(node, start)
    }
    fn duplicate_jsdoc_tag(&mut self, name: NodeId) {
        self.parse_error_at(
            self.factory.node(name).range().pos(),
            self.scanner.token_start(),
            diagnostics::X_0_tag_already_specified,
            vec![self.jsdoc_text(name)],
        );
    }
    /// port: tsc/internal/parser/jsdoc.go:Parser.parseSeeTag
    fn parse_see_tag(&mut self, start: i64, name: NodeId, indent: i64, text: JsString) -> NodeId {
        let has_name = self.is_identifier()
            && !self.source_text[self.scanner.token_end() as usize..].starts_with(b"://")
            || self.token == K::OpenBraceToken
                && self.look_ahead(|p| token_is_identifier_or_keyword(p.next_token()));
        let reference = has_name.then(|| self.parse_js_doc_name_reference());
        let comments = self.parse_trailing_tag_comments(start, self.node_pos(), indent, text);
        let node = self
            .factory
            .new_js_doc_see_tag(Some(name), reference, comments);
        self.finish_node(node, start)
    }
    /// port: tsc/internal/parser/jsdoc.go:Parser.parseImplementsTag
    fn parse_implements_tag(
        &mut self,
        start: i64,
        name: NodeId,
        margin: i64,
        text: JsString,
    ) -> NodeId {
        let class = self.parse_expression_with_type_arguments_for_augments();
        let comments = self.parse_trailing_tag_comments(start, self.node_pos(), margin, text);
        let node = self
            .factory
            .new_js_doc_implements_tag(Some(name), Some(class), comments);
        self.finish_node(node, start)
    }
    /// port: tsc/internal/parser/jsdoc.go:Parser.parseAugmentsTag
    fn parse_augments_tag(
        &mut self,
        start: i64,
        name: NodeId,
        margin: i64,
        text: JsString,
    ) -> NodeId {
        let class = self.parse_expression_with_type_arguments_for_augments();
        let comments = self.parse_trailing_tag_comments(start, self.node_pos(), margin, text);
        let node = self
            .factory
            .new_js_doc_augments_tag(Some(name), Some(class), comments);
        self.finish_node(node, start)
    }
    /// port: tsc/internal/parser/jsdoc.go:Parser.parseSatisfiesTag
    fn parse_satisfies_tag(
        &mut self,
        start: i64,
        name: NodeId,
        margin: i64,
        text: JsString,
    ) -> NodeId {
        let ty = self.parse_js_doc_type_expression(false);
        let comments = self.parse_trailing_tag_comments(start, self.node_pos(), margin, text);
        let node = self
            .factory
            .new_js_doc_satisfies_tag(Some(name), Some(ty), comments);
        self.finish_node(node, start)
    }
    /// port: tsc/internal/parser/jsdoc.go:Parser.parseThrowsTag
    fn parse_throws_tag(
        &mut self,
        start: i64,
        name: NodeId,
        margin: i64,
        text: JsString,
    ) -> NodeId {
        let ty = self.try_parse_type_expression();
        let comments = self.parse_trailing_tag_comments(start, self.node_pos(), margin, text);
        let node = self.factory.new_js_doc_throws_tag(Some(name), ty, comments);
        self.finish_node(node, start)
    }
    /// port: tsc/internal/parser/jsdoc.go:Parser.parseImportTag
    fn parse_import_tag(
        &mut self,
        start: i64,
        name: NodeId,
        margin: i64,
        text: JsString,
    ) -> NodeId {
        let pos = self.scanner.token_full_start();
        let identifier = self.is_identifier().then(|| self.parse_identifier());
        let clause = self.try_parse_import_clause(identifier, pos, K::TypeKeyword, true);
        let specifier = self.parse_module_specifier();
        let attributes = self.try_parse_import_attributes();
        let comments = self.parse_trailing_tag_comments(start, self.node_pos(), margin, text);
        let node = self.factory.new_js_doc_import_tag(
            Some(name),
            clause,
            Some(specifier),
            attributes,
            comments,
        );
        self.finish_node(node, start)
    }
    /// port: tsc/internal/parser/jsdoc.go:Parser.parseExpressionWithTypeArgumentsForAugments
    fn parse_expression_with_type_arguments_for_augments(&mut self) -> NodeId {
        let brace = self.parse_optional(K::OpenBraceToken);
        let pos = self.node_pos();
        let expression = self.parse_property_access_entity_name_expression();
        self.scanner.set_skip_jsdoc_leading_asterisks(true);
        let arguments = self.parse_type_arguments();
        self.scanner.set_skip_jsdoc_leading_asterisks(false);
        let node = self
            .factory
            .new_expression_with_type_arguments(Some(expression), arguments);
        self.finish_node(node, pos);
        if brace {
            self.skip_whitespace();
            self.parse_expected(K::CloseBraceToken);
        }
        node
    }
    /// port: tsc/internal/parser/jsdoc.go:Parser.parsePropertyAccessEntityNameExpression
    fn parse_property_access_entity_name_expression(&mut self) -> NodeId {
        let pos = self.node_pos();
        let mut node = self.parse_js_doc_identifier_name(Some(diagnostics::Identifier_expected));
        while self.parse_optional(K::DotToken) {
            let name = self.parse_js_doc_identifier_name(Some(diagnostics::Identifier_expected));
            node = self
                .factory
                .new_property_access_expression(Some(node), None, Some(name), 0);
            self.finish_node(node, pos);
        }
        node
    }
    /// port: tsc/internal/parser/jsdoc.go:Parser.parseSimpleTag
    fn parse_simple_tag(
        &mut self,
        start: i64,
        create: fn(&mut F, Option<NodeId>, Option<NodeListId>) -> NodeId,
        name: NodeId,
        margin: i64,
        text: JsString,
    ) -> NodeId {
        let comments = self.parse_trailing_tag_comments(start, self.node_pos(), margin, text);
        let node = create(&mut self.factory, Some(name), comments);
        self.finish_node(node, start)
    }
    /// port: tsc/internal/parser/jsdoc.go:Parser.parseThisTag
    fn parse_this_tag(&mut self, start: i64, name: NodeId, margin: i64, text: JsString) -> NodeId {
        let ty = self.parse_js_doc_type_expression(true);
        self.skip_whitespace();
        let comments = self.parse_trailing_tag_comments(start, self.node_pos(), margin, text);
        let node = self
            .factory
            .new_js_doc_this_tag(Some(name), Some(ty), comments);
        self.finish_node(node, start)
    }
    /// port: tsc/internal/parser/jsdoc.go:Parser.parseJSDocTypeNameWithNamespace
    fn parse_js_doc_type_name_with_namespace(&mut self, nested: bool) -> Option<NodeId> {
        crate::recursion::guarded(|| {
            let start = self.scanner.token_start();
            if !token_is_identifier_or_keyword(self.token) {
                return None;
            }
            let name = self.parse_js_doc_identifier_name(None);
            if self.parse_optional_jsdoc(K::DotToken) {
                let body = self.parse_js_doc_type_name_with_namespace(true);
                let node = self.factory.new_module_declaration(
                    None,
                    K::NamespaceKeyword.into(),
                    Some(name),
                    None,
                    body,
                );
                if nested {
                    self.factory
                        .add_node_flags(node, node_flags::NESTED_NAMESPACE);
                }
                return Some(self.finish_node(node, start));
            }
            if nested {
                self.factory
                    .add_node_flags(name, node_flags::IDENTIFIER_IS_IN_JS_DOC_NAMESPACE);
            }
            Some(name)
        })
    }
    /// port: tsc/internal/parser/jsdoc.go:Parser.parseTypedefTag
    fn parse_typedef_tag(
        &mut self,
        start: i64,
        name: NodeId,
        indent: i64,
        indent_text: JsString,
    ) -> NodeId {
        let mut ty = self.try_parse_type_expression();
        self.skip_whitespace_or_asterisk();
        let full_name = self
            .parse_js_doc_type_name_with_namespace(false)
            .unwrap_or_else(|| {
                self.parse_js_doc_identifier_name(Some(diagnostics::Identifier_expected))
            });
        self.skip_whitespace();
        let mut comment = self.parse_tag_comments(indent, None);
        let mut end = -1;
        let mut has_children = false;
        if ty.is_none_or(|id| {
            self.is_object_or_object_array_type_reference(
                self.jsdoc_type(id).expect("typedef expression type"),
            )
        }) {
            let mut type_tag = None;
            let mut properties = Vec::new();
            loop {
                let state = self.mark();
                let Some(child) = self.parse_child_property_tag(indent) else {
                    self.rewind(state);
                    break;
                };
                self.commit(state);
                has_children = true;
                match self.factory.node(child).kind().known() {
                    Some(K::JSDocTemplateTag) => {
                        let loc = self.factory.node(self.jsdoc_tag_name(child)).range();
                        self.parse_error_at_range(loc, diagnostics::A_JSDoc_template_tag_may_not_follow_a_typedef_callback_or_overload_tag, vec![]);
                    }
                    Some(K::JSDocTypeTag) => {
                        if type_tag.is_none() {
                            type_tag = Some(child);
                        } else if let Some(index) = self.parse_error_at_current_token(
                            diagnostics::A_JSDoc_typedef_comment_may_not_contain_multiple_type_tags,
                            vec![],
                        ) {
                            self.diagnostics[index].related_information.push(Arc::new(
                                Diagnostic::new(
                                    None,
                                    TextRange::new(0, 0),
                                    diagnostics::The_tag_was_first_specified_here,
                                    vec![],
                                ),
                            ));
                        }
                    }
                    _ => properties.push(child),
                }
            }
            if has_children {
                let is_array = ty.is_some_and(|id| {
                    self.factory
                        .node(self.jsdoc_type(id).expect("typedef expression type"))
                        .kind()
                        == K::ArrayType
                });
                let pos = properties
                    .first()
                    .map_or(start, |&node| self.factory.node(node).range().pos());
                let nodes = self
                    .factory
                    .alloc_nodes(properties.into_iter().map(Some).collect());
                let literal = self.factory.new_js_doc_type_literal(nodes, is_array);
                let child_ty = type_tag.and_then(|tag| self.jsdoc_type(tag));
                ty = if child_ty.is_some_and(|node| {
                    !self.is_object_or_object_array_type_reference(
                        self.jsdoc_type(node).expect("child type expression"),
                    )
                }) {
                    child_ty
                } else {
                    Some(self.finish_node(literal, pos))
                };
                end = self
                    .factory
                    .node(ty.expect("typedef synthesized type"))
                    .range()
                    .end();
            }
        }
        if end == -1 {
            end = if let Some(ty) = ty.filter(|_| has_children) {
                self.factory.node(ty).range().end()
            } else if comment.is_some() {
                self.node_pos()
            } else {
                self.factory.node(full_name).range().end()
            };
        }
        if comment.is_none() {
            comment = self.parse_trailing_tag_comments(start, end, indent, indent_text);
        }
        let node = self
            .factory
            .new_js_doc_typedef_tag(Some(name), ty, Some(full_name), comment);
        self.finish_node_with_end(node, start, end);
        if let Some(ty) = ty {
            self.factory.set_node_parent(ty, Some(node));
        }
        node
    }
    /// port: tsc/internal/parser/jsdoc.go:Parser.parseCallbackTagParameters
    fn parse_callback_tag_parameters(&mut self, indent: i64) -> NodeListId {
        let pos = self.node_pos();
        let mut parameters = Vec::new();
        loop {
            let state = self.mark();
            let Some(child) =
                self.parse_child_parameter_or_property_tag(CALLBACK_PARAMETER, indent, None)
            else {
                self.rewind(state);
                break;
            };
            self.commit(state);
            if self.factory.node(child).kind() == K::JSDocTemplateTag {
                let loc = self.factory.node(self.jsdoc_tag_name(child)).range();
                self.parse_error_at_range(loc, diagnostics::A_JSDoc_template_tag_may_not_follow_a_typedef_callback_or_overload_tag, vec![]);
            } else {
                parameters.push(child);
            }
        }
        self.new_node_list(TextRange::new(pos, self.node_pos()), parameters)
    }
    /// port: tsc/internal/parser/jsdoc.go:Parser.parseJSDocSignature
    fn parse_js_doc_signature(&mut self, start: i64, indent: i64) -> NodeId {
        let parameters = self.parse_callback_tag_parameters(indent);
        let mut return_tag = None;
        let state = self.mark();
        if self.parse_optional_jsdoc(K::AtToken) {
            let tag = self.parse_tag(&[], indent);
            if self.factory.node(tag).kind() == K::JSDocReturnTag {
                return_tag = Some(tag);
            }
        }
        if return_tag.is_none() {
            self.rewind(state);
        } else {
            self.commit(state);
        }
        let node = self
            .factory
            .new_js_doc_signature(None, Some(parameters), return_tag);
        self.finish_node(node, start)
    }
    /// port: tsc/internal/parser/jsdoc.go:Parser.parseCallbackTag
    fn parse_callback_tag(
        &mut self,
        start: i64,
        name: NodeId,
        indent: i64,
        text: JsString,
    ) -> NodeId {
        let full_name = self
            .parse_js_doc_type_name_with_namespace(false)
            .unwrap_or_else(|| {
                self.parse_js_doc_identifier_name(Some(diagnostics::Identifier_expected))
            });
        self.skip_whitespace();
        let mut comment = self.parse_tag_comments(indent, None);
        let ty = self.parse_js_doc_signature(self.node_pos(), indent);
        if comment.is_none() {
            comment = self.parse_trailing_tag_comments(start, self.node_pos(), indent, text);
        }
        let end = if comment.is_some() {
            self.node_pos()
        } else {
            self.factory.node(ty).range().end()
        };
        let node =
            self.factory
                .new_js_doc_callback_tag(Some(name), Some(ty), Some(full_name), comment);
        self.finish_node_with_end(node, start, end)
    }
    /// port: tsc/internal/parser/jsdoc.go:Parser.parseOverloadTag
    fn parse_overload_tag(
        &mut self,
        start: i64,
        name: NodeId,
        indent: i64,
        text: JsString,
    ) -> NodeId {
        self.skip_whitespace();
        let mut comment = self.parse_tag_comments(indent, None);
        let ty = self.parse_js_doc_signature(start, indent);
        if comment.is_none() {
            comment = self.parse_trailing_tag_comments(start, self.node_pos(), indent, text);
        }
        let end = if comment.is_some() {
            self.node_pos()
        } else {
            self.factory.node(ty).range().end()
        };
        let node = self
            .factory
            .new_js_doc_overload_tag(Some(name), Some(ty), comment);
        self.finish_node_with_end(node, start, end)
    }
    /// port: tsc/internal/parser/jsdoc.go:textsEqual
    fn texts_equal(&self, mut a: NodeId, mut b: NodeId) -> bool {
        while self.factory.node(a).kind() != K::Identifier
            || self.factory.node(b).kind() != K::Identifier
        {
            if self.factory.node(a).kind() == K::Identifier
                || self.factory.node(b).kind() == K::Identifier
            {
                return false;
            }
            let (left_a, right_a) = self.jsdoc_qualified_name(a);
            let (left_b, right_b) = self.jsdoc_qualified_name(b);
            if self.jsdoc_text(right_a) != self.jsdoc_text(right_b) {
                return false;
            }
            a = left_a;
            b = left_b;
        }
        self.jsdoc_text(a) == self.jsdoc_text(b)
    }
    /// port: tsc/internal/parser/jsdoc.go:Parser.parseChildPropertyTag
    fn parse_child_property_tag(&mut self, indent: i64) -> Option<NodeId> {
        self.parse_child_parameter_or_property_tag(PROPERTY, indent, None)
    }
    /// port: tsc/internal/parser/jsdoc.go:Parser.parseChildParameterOrPropertyTag
    fn parse_child_parameter_or_property_tag(
        &mut self,
        target: u8,
        indent: i64,
        name: Option<NodeId>,
    ) -> Option<NodeId> {
        let mut can_parse = true;
        let mut seen_asterisk = false;
        loop {
            match self.next_token_jsdoc() {
                K::AtToken => {
                    if can_parse && self.scanner.can_follow_jsdoc_at() {
                        let child = self.try_parse_child_tag(target, indent)?;
                        if let Some(name) = name {
                            if matches!(
                                self.factory.node(child).kind().known(),
                                Some(K::JSDocParameterTag | K::JSDocPropertyTag)
                            ) {
                                let child_name = self.jsdoc_name(child).expect("property name");
                                if self.factory.node(child_name).kind() == K::Identifier
                                    || !self
                                        .texts_equal(name, self.jsdoc_qualified_name(child_name).0)
                                {
                                    return None;
                                }
                            }
                        }
                        return Some(child);
                    }
                    seen_asterisk = false;
                }
                K::NewLineTrivia => {
                    can_parse = true;
                    seen_asterisk = false;
                }
                K::AsteriskToken => {
                    if seen_asterisk {
                        can_parse = false;
                    }
                    seen_asterisk = true;
                }
                K::Identifier => can_parse = false,
                K::EndOfFile => return None,
                _ => {}
            }
        }
    }
    /// port: tsc/internal/parser/jsdoc.go:Parser.tryParseChildTag
    fn try_parse_child_tag(&mut self, target: u8, indent: i64) -> Option<NodeId> {
        assert!(self.token == K::AtToken, "should only be called when at @");
        let start = self.scanner.token_full_start();
        self.next_token_jsdoc();
        let name = self.parse_js_doc_identifier_name(Some(diagnostics::Identifier_expected));
        let text = self.skip_whitespace_or_asterisk();
        let mask = match self.jsdoc_text(name).as_bytes() {
            b"type" => {
                if target == PROPERTY {
                    return Some(self.parse_type_tag(&[], start, name, -1, JsString::default()));
                }
                0
            }
            b"prop" | b"property" => PROPERTY,
            b"arg" | b"argument" | b"param" => PARAMETER | CALLBACK_PARAMETER,
            b"template" => return Some(self.parse_template_tag(start, name, indent, text)),
            b"this" => return Some(self.parse_this_tag(start, name, indent, text)),
            _ => return None,
        };
        (target & mask != 0)
            .then(|| self.parse_parameter_or_property_tag(start, name, target, indent))
    }
    /// port: tsc/internal/parser/jsdoc.go:Parser.parseTemplateTagTypeParameter
    fn parse_template_tag_type_parameter(&mut self) -> Option<NodeId> {
        let pos = self.node_pos();
        let bracketed = self.parse_optional_jsdoc(K::OpenBracketToken);
        if bracketed {
            self.skip_whitespace();
        }
        let modifiers = self.parse_modifiers_ex(false, true, false);
        let name = self.parse_js_doc_identifier_name(Some(
            diagnostics::Unexpected_token_A_type_parameter_name_was_expected_without_curly_braces,
        ));
        let default = if bracketed {
            self.skip_whitespace();
            self.parse_expected(K::EqualsToken);
            let flags = self.context_flags;
            self.set_context_flags(node_flags::JS_DOC, true);
            let ty = self.parse_js_doc_type();
            self.context_flags = flags;
            self.parse_expected(K::CloseBracketToken);
            Some(ty)
        } else {
            None
        };
        if self.node_is_missing(Some(name)) {
            return None;
        }
        let node =
            self.factory
                .new_type_parameter_declaration(modifiers, Some(name), None, None, default);
        Some(self.finish_node(node, pos))
    }
    /// port: tsc/internal/parser/jsdoc.go:Parser.parseTemplateTagTypeParameters
    fn parse_template_tag_type_parameters(&mut self) -> NodeListId {
        let mut nodes = Vec::new();
        loop {
            self.skip_whitespace();
            if let Some(node) = self.parse_template_tag_type_parameter() {
                nodes.push(node);
            }
            self.skip_whitespace_or_asterisk();
            if !self.parse_optional_jsdoc(K::CommaToken) {
                break;
            }
        }
        self.new_node_list(TextRange::new(0, 0), nodes)
    }
    /// port: tsc/internal/parser/jsdoc.go:Parser.parseTemplateTag
    fn parse_template_tag(
        &mut self,
        start: i64,
        name: NodeId,
        indent: i64,
        text: JsString,
    ) -> NodeId {
        let constraint =
            (self.token == K::OpenBraceToken).then(|| self.parse_js_doc_type_expression(false));
        let parameters = self.parse_template_tag_type_parameters();
        let comments = self.parse_trailing_tag_comments(start, self.node_pos(), indent, text);
        let node = self.factory.new_js_doc_template_tag(
            Some(name),
            constraint,
            Some(parameters),
            comments,
        );
        self.finish_node(node, start)
    }
    /// port: tsc/internal/parser/jsdoc.go:Parser.parseJSDocEntityName
    fn parse_js_doc_entity_name(&mut self, message: Option<&'static Message>) -> NodeId {
        let mut entity = self.parse_js_doc_identifier_name(message);
        if self.parse_optional(K::OpenBracketToken) {
            self.parse_expected(K::CloseBracketToken);
        }
        while self.parse_optional(K::DotToken) {
            let name = self.parse_js_doc_identifier_name(Some(diagnostics::Identifier_expected));
            if self.parse_optional(K::OpenBracketToken) {
                self.parse_expected(K::CloseBracketToken);
            }
            let pos = self.factory.node(entity).range().pos();
            entity = self.factory.new_qualified_name(Some(entity), Some(name));
            self.finish_node(entity, pos);
        }
        entity
    }
    pub(crate) fn jsdoc_qualified_name(&self, id: NodeId) -> (NodeId, NodeId) {
        let n = self.factory.node(id);
        let d = n.data_source().as_qualified_name().expect("qualified name");
        (
            d.left().expect("qualified left"),
            d.right().expect("qualified right"),
        )
    }
    pub(crate) fn jsdoc_text(&self, id: NodeId) -> JsString {
        match self.factory.node(id).data() {
            NodeDataRead::NoSubstitutionTemplateLiteral(d) => d.text_owned(),
            NodeDataRead::Identifier(d) => d.text_owned(),
            NodeDataRead::StringLiteral(d) => d.text_owned(),
            NodeDataRead::NumericLiteral(d) => d.text_owned(),
            NodeDataRead::BigIntLiteral(d) => d.text_owned(),
            _ => panic!("node has no text"),
        }
    }
    pub(crate) fn jsdoc_name(&self, id: NodeId) -> Option<NodeId> {
        self.factory.node(id).data().declaration_name_generated()
    }
    pub(crate) fn jsdoc_type(&self, id: NodeId) -> Option<NodeId> {
        match self.factory.node(id).data() {
            NodeDataRead::JSDocTypeExpression(d) => d.r#type(),
            NodeDataRead::JSDocVariadicType(d) => d.r#type(),
            NodeDataRead::JSDocOptionalType(d) => d.r#type(),
            NodeDataRead::JSDocNullableType(d) => d.r#type(),
            NodeDataRead::JSDocNonNullableType(d) => d.r#type(),
            NodeDataRead::JSDocSignature(d) => d.r#type(),
            NodeDataRead::JSDocTypeTag(d) => d.type_expression(),
            NodeDataRead::JSDocReturnTag(d) => d.type_expression(),
            NodeDataRead::JSDocThisTag(d) => d.type_expression(),
            NodeDataRead::JSDocParameterOrPropertyTag(d) => d.type_expression(),
            NodeDataRead::JSDocTypedefTag(d) => d.type_expression(),
            NodeDataRead::JSDocCallbackTag(d) => d.type_expression(),
            NodeDataRead::JSDocOverloadTag(d) => d.type_expression(),
            NodeDataRead::JSDocSatisfiesTag(d) => d.type_expression(),
            NodeDataRead::FunctionDeclaration(d) => d.r#type(),
            NodeDataRead::MethodDeclaration(d) => d.r#type(),
            NodeDataRead::FunctionExpression(d) => d.r#type(),
            NodeDataRead::ArrowFunction(d) => d.r#type(),
            NodeDataRead::ParameterDeclaration(d) => d.r#type(),
            NodeDataRead::PropertyDeclaration(d) => d.r#type(),
            NodeDataRead::PropertySignatureDeclaration(d) => d.r#type(),
            NodeDataRead::TypeAliasDeclaration(d) => d.r#type(),
            NodeDataRead::VariableDeclaration(d) => d.r#type(),
            NodeDataRead::FunctionTypeNode(d) => d.r#type(),
            NodeDataRead::ParenthesizedTypeNode(d) => d.r#type(),
            NodeDataRead::ExportAssignment(d) => d.r#type(),
            NodeDataRead::PropertyAssignment(d) => d.r#type(),
            NodeDataRead::ShorthandPropertyAssignment(d) => d.r#type(),
            NodeDataRead::BinaryExpression(d) => d.r#type(),
            NodeDataRead::GetAccessorDeclaration(d) => d.r#type(),
            NodeDataRead::SetAccessorDeclaration(d) => d.r#type(),
            NodeDataRead::ConstructorDeclaration(d) => d.r#type(),
            NodeDataRead::ConstructorTypeNode(d) => d.r#type(),
            NodeDataRead::CallSignatureDeclaration(d) => d.r#type(),
            NodeDataRead::ConstructSignatureDeclaration(d) => d.r#type(),
            NodeDataRead::IndexSignatureDeclaration(d) => d.r#type(),
            NodeDataRead::MethodSignatureDeclaration(d) => d.r#type(),
            _ => None,
        }
    }
    fn jsdoc_tag_name(&self, id: NodeId) -> NodeId {
        match self.factory.node(id).data() {
            NodeDataRead::JSDocTemplateTag(d) => d.tag_name(),
            _ => panic!("expected JSDoc template tag"),
        }
        .expect("JSDoc tag name")
    }
}

#[cfg(test)]
#[path = "jsdoc_tests.rs"]
mod tests;
