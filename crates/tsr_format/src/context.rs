//! What a rule sees: two adjacent tokens, their parents and the node that
//! contains both (`format/context.go`). The line facts are computed on first
//! use and dropped when the context moves on.

use crate::{
    scanner::TextRangeWithKind, settings::FormatCodeSettings, util::range_is_on_one_line, Error,
    FormatFile,
};
use ts_arena::NodeId;
use ts_ast::SyntaxKind as K;
use ts_core::{TextRange, Tristate};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FormatRequestKind {
    FormatDocument,
    FormatSelection,
    FormatOnEnter,
    FormatOnSemicolon,
    FormatOnOpeningCurlyBrace,
    FormatOnClosingCurlyBrace,
}

pub struct FormattingContext<'a, 'p, 'f> {
    pub(crate) current_token_span: TextRangeWithKind,
    pub(crate) next_token_span: TextRangeWithKind,
    pub(crate) context_node: Option<NodeId>,
    pub(crate) current_token_parent: Option<NodeId>,
    pub(crate) next_token_parent: Option<NodeId>,

    context_node_all_on_same_line: Tristate,
    next_node_all_on_same_line: Tristate,
    tokens_are_on_same_line: Tristate,
    context_node_block_is_on_one_line: Tristate,
    next_node_block_is_on_one_line: Tristate,

    pub(crate) file: &'f mut FormatFile<'a, 'p>,
    pub(crate) formatting_request_kind: FormatRequestKind,
    pub(crate) options: FormatCodeSettings,
}

fn tristate(value: bool) -> Tristate {
    if value {
        Tristate::TRUE
    } else {
        Tristate::FALSE
    }
}

impl<'a, 'p, 'f> FormattingContext<'a, 'p, 'f> {
    // port: tsc/internal/format/context.go:NewFormattingContext
    pub fn new(
        file: &'f mut FormatFile<'a, 'p>,
        kind: FormatRequestKind,
        options: FormatCodeSettings,
    ) -> Self {
        let empty = TextRangeWithKind::new(0, 0, K::Unknown);
        Self {
            current_token_span: empty,
            next_token_span: empty,
            context_node: None,
            current_token_parent: None,
            next_token_parent: None,
            context_node_all_on_same_line: Tristate::UNKNOWN,
            next_node_all_on_same_line: Tristate::UNKNOWN,
            tokens_are_on_same_line: Tristate::UNKNOWN,
            context_node_block_is_on_one_line: Tristate::UNKNOWN,
            next_node_block_is_on_one_line: Tristate::UNKNOWN,
            file,
            formatting_request_kind: kind,
            options,
        }
    }

    // port: tsc/internal/format/context.go:FormattingContext.UpdateContext
    pub(crate) fn update_context(
        &mut self,
        current: TextRangeWithKind,
        current_parent: NodeId,
        next: TextRangeWithKind,
        next_parent: NodeId,
        common_parent: NodeId,
    ) {
        self.current_token_span = current;
        self.current_token_parent = Some(current_parent);
        self.next_token_span = next;
        self.next_token_parent = Some(next_parent);
        self.context_node = Some(common_parent);
        // Drop the cached results.
        self.context_node_all_on_same_line = Tristate::UNKNOWN;
        self.next_node_all_on_same_line = Tristate::UNKNOWN;
        self.tokens_are_on_same_line = Tristate::UNKNOWN;
        self.context_node_block_is_on_one_line = Tristate::UNKNOWN;
        self.next_node_block_is_on_one_line = Tristate::UNKNOWN;
    }

    /// Upstream's context nodes are never nil once `UpdateContext` has run.
    fn required(node: Option<NodeId>) -> Result<NodeId, Error> {
        node.ok_or_else(|| Error::Assertion("the formatting context has not been updated".into()))
    }

    // port: tsc/internal/format/context.go:FormattingContext.nodeIsOnOneLine
    // port: tsc/internal/format/context.go:withTokenStart
    fn node_is_on_one_line(&mut self, node: NodeId) -> Result<Tristate, Error> {
        let start = self.file.token_pos(node)?;
        let end = i64::from(self.file.node(node)?.end());
        Ok(tristate(range_is_on_one_line(
            TextRange::new(start, end),
            self.file,
        )?))
    }

    // port: tsc/internal/format/context.go:FormattingContext.blockIsOnOneLine
    fn block_is_on_one_line(&mut self, node: NodeId) -> Result<Tristate, Error> {
        let open = self
            .file
            .navigator()
            .find_child_of_kind(node, K::OpenBraceToken)?;
        let close = self
            .file
            .navigator()
            .find_child_of_kind(node, K::CloseBraceToken)?;
        let (Some(open), Some(close)) = (open, close) else {
            return Ok(Tristate::FALSE);
        };
        let open_end = i64::from(self.file.node(open)?.end());
        let close_start = self.file.token_pos(close)?;
        Ok(tristate(range_is_on_one_line(
            TextRange::new(open_end, close_start),
            self.file,
        )?))
    }

    // port: tsc/internal/format/context.go:FormattingContext.ContextNodeAllOnSameLine
    pub(crate) fn context_node_all_on_same_line(&mut self) -> Result<bool, Error> {
        if self.context_node_all_on_same_line == Tristate::UNKNOWN {
            let node = Self::required(self.context_node)?;
            self.context_node_all_on_same_line = self.node_is_on_one_line(node)?;
        }
        Ok(self.context_node_all_on_same_line == Tristate::TRUE)
    }

    // port: tsc/internal/format/context.go:FormattingContext.NextNodeAllOnSameLine
    pub(crate) fn next_node_all_on_same_line(&mut self) -> Result<bool, Error> {
        if self.next_node_all_on_same_line == Tristate::UNKNOWN {
            let node = Self::required(self.next_token_parent)?;
            self.next_node_all_on_same_line = self.node_is_on_one_line(node)?;
        }
        Ok(self.next_node_all_on_same_line == Tristate::TRUE)
    }

    // port: tsc/internal/format/context.go:FormattingContext.TokensAreOnSameLine
    pub(crate) fn tokens_are_on_same_line(&mut self) -> Result<bool, Error> {
        if self.tokens_are_on_same_line == Tristate::UNKNOWN {
            let range = TextRange::new(
                self.current_token_span.loc.pos(),
                self.next_token_span.loc.end(),
            );
            self.tokens_are_on_same_line = tristate(range_is_on_one_line(range, self.file)?);
        }
        Ok(self.tokens_are_on_same_line == Tristate::TRUE)
    }

    // port: tsc/internal/format/context.go:FormattingContext.ContextNodeBlockIsOnOneLine
    pub(crate) fn context_node_block_is_on_one_line(&mut self) -> Result<bool, Error> {
        if self.context_node_block_is_on_one_line == Tristate::UNKNOWN {
            let node = Self::required(self.context_node)?;
            self.context_node_block_is_on_one_line = self.block_is_on_one_line(node)?;
        }
        Ok(self.context_node_block_is_on_one_line == Tristate::TRUE)
    }

    // port: tsc/internal/format/context.go:FormattingContext.NextNodeBlockIsOnOneLine
    pub(crate) fn next_node_block_is_on_one_line(&mut self) -> Result<bool, Error> {
        if self.next_node_block_is_on_one_line == Tristate::UNKNOWN {
            let node = Self::required(self.next_token_parent)?;
            self.next_node_block_is_on_one_line = self.block_is_on_one_line(node)?;
        }
        Ok(self.next_node_block_is_on_one_line == Tristate::TRUE)
    }
}
