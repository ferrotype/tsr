//! The printer (`tsc/internal/printer/printer.go`). Every function that emits a
//! node kind is a port of the upstream function of the same name and keeps its
//! order of writes, comments included.
//!
//! A `Printer` holds options and the emit context. Each `write` opens a
//! [`Session`] over one AST view and one writer; nothing about a node is cached
//! across sessions. Upstream keeps the per-write state (the current source
//! file, the comment containers, the writer) on the printer itself and saves
//! and restores it around `Write`.
//!
//! Named boundaries, each returning [`Error::Unsupported`] where it is reached:
//! source-map emission (`Write` takes no generator here), the helper table
//! behind `emitHelpers` and `getUniqueHelperName`, generated names
//! (`NameGenerator`), and the nodes upstream creates while printing in
//! `parenthesizeExpressionForNoAsi`.

#[path = "printer_comments.rs"]
mod comments;
#[path = "printer_expressions.rs"]
mod expressions;
#[path = "printer_statements.rs"]
mod statements;

use comments::{token_emit_flags as tef, CommentSeparator, DetachedCommentsInfo, SourceMapSource};

use crate::emit_flags as ef;
use crate::list_format as lf;
use crate::literal_text::{with_flag, LiteralTextFlags};
use crate::{
    get_type_node_precedence, EmitContext, EmitTextWriter, Error, ListFormat, TextWriter,
    TrailingSemicolonDeferringWriter, TypePrecedence,
};
use std::collections::HashMap;
use tsr_arena::SymbolId;
use tsr_ast::operator_precedence as op;
use tsr_ast::{
    node_flags, token_flags, AstView, JsString, NodeId, NodeKind, NodeListId, NodeRead,
    SourceFileRead, SyntaxKind as K,
};
use tsr_core::{NewLineKind, ScriptTarget, TextRange};
use tsr_jsstring::LiteralEscapeFlags;
use tsr_scanner::token_to_string;

/// `PrinterOptions`. The source-map options are not taken: `Write` has no
/// source-map generator here.
#[derive(Clone, Debug, Default)]
pub struct PrinterOptions {
    pub remove_comments: bool,
    pub new_line: NewLineKind,
    pub omit_trailing_semicolon: bool,
    /// Read by `emitHelpers`, which is a boundary until the helper table lands.
    pub no_emit_helpers: bool,
    pub target: ScriptTarget,
    pub omit_brace_source_map_positions: bool,
    pub only_print_js_doc_style: bool,
    pub never_ascii_escape: bool,
    pub preserve_source_newlines: bool,
    pub terminate_unterminated_literals: bool,
}

/// `WriteKind`: which writer method a piece of text goes through.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WriteKind {
    None,
    Keyword,
    Operator,
    Punctuation,
    StringLiteral,
    Parameter,
    Property,
    Comment,
    Literal,
}

/// `SnippetKind`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SnippetKind {
    #[allow(
        dead_code,
        reason = "set through the emit context's snippet table, which is not recorded yet"
    )]
    TabStop,
}

/// `SnippetElement`: a language-service placeholder an empty statement stands
/// for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct SnippetElement {
    pub(crate) kind: SnippetKind,
    pub(crate) order: i64,
}

pub struct Printer<'c> {
    pub options: PrinterOptions,
    pub(crate) emit_context: &'c EmitContext,
    /// Identifiers to report through `write_symbol` (`Printer.IdToSymbol`).
    pub id_to_symbol: Option<HashMap<NodeId, SymbolId>>,
    own_writer: Option<TextWriter>,
}

fn new_line_character(kind: NewLineKind) -> &'static [u8] {
    if kind == NewLineKind::CRLF {
        b"\r\n"
    } else {
        b"\n"
    }
}

impl<'c> Printer<'c> {
    /// Handlers (name generation, emit notifications) are not taken: no caller in
    /// the supported set installs any.
    // port: tsc/internal/printer/printer.go:NewPrinter
    pub fn new(options: PrinterOptions, emit_context: &'c EmitContext) -> Self {
        Self {
            options,
            emit_context,
            id_to_symbol: None,
            own_writer: None,
        }
    }

    /// Prints into a reusable writer of the printer's own and returns the bytes.
    // port: tsc/internal/printer/printer.go:Printer.Emit
    pub fn emit(
        &mut self,
        view: AstView<'_>,
        node: NodeId,
        source_file: Option<NodeId>,
    ) -> Result<Vec<u8>, Error> {
        let new_line = new_line_character(self.options.new_line);
        let mut writer = self
            .own_writer
            .take()
            .unwrap_or_else(|| TextWriter::new(new_line, 0));
        let result = self.write(view, node, source_file, &mut writer);
        let text = writer.text().to_vec();
        writer.clear();
        self.own_writer = Some(writer);
        result.map(|()| text)
    }

    /// Prints a whole file, with its comments unless the options remove them.
    // port: tsc/internal/printer/printer.go:Printer.EmitSourceFile
    pub fn emit_source_file(
        &mut self,
        view: AstView<'_>,
        source_file: NodeId,
    ) -> Result<Vec<u8>, Error> {
        self.emit(view, source_file, Some(source_file))
    }

    /// Prints one node through `writer`. `source_file` is the file whose text
    /// parsed literals, identifiers and comments are read from, as upstream's
    /// `currentSourceFile`. No source-map generator is taken, so source maps
    /// are disabled, as upstream's `Write` with a nil generator.
    // port: tsc/internal/printer/printer.go:Printer.Write
    pub fn write(
        &self,
        view: AstView<'_>,
        node: NodeId,
        source_file: Option<NodeId>,
        writer: &mut dyn EmitTextWriter,
    ) -> Result<(), Error> {
        let mut deferring;
        let writer: &mut dyn EmitTextWriter = if self.options.omit_trailing_semicolon {
            deferring = TrailingSemicolonDeferringWriter::new(writer);
            &mut deferring
        } else {
            writer
        };
        let mut session = Session {
            printer: self,
            view,
            writer,
            current_source: None,
            unique_helper_names: None,
            external_helpers_module_name: None,
            next_list_element_pos: 0,
            write_kind: WriteKind::None,
            source_maps_disabled: true,
            source_map_source: None,
            container_pos: -1,
            container_end: -1,
            declaration_list_container_end: -1,
            detached_comments_info: Vec::new(),
            comments_disabled: self.options.remove_comments,
            in_extends: false,
        };
        session.set_source_file(source_file)?;
        session.writer.clear();
        session.write_root(node)
    }
}

/// One `Printer.Write` in progress: the printer state upstream saves and
/// restores around `Write`, and the per-write comment state.
pub(crate) struct Session<'a, 'c> {
    pub(crate) printer: &'a Printer<'c>,
    pub(crate) view: AstView<'a>,
    writer: &'a mut dyn EmitTextWriter,
    pub(crate) current_source: Option<(NodeId, SourceFileRead<'a>)>,
    /// `uniqueHelperNames`; present for a file flagged `ExternalHelpers`.
    unique_helper_names: Option<HashMap<JsString, NodeId>>,
    /// `externalHelpersModuleName`. The emit context records no helper module
    /// names yet (`GetExternalHelpersModuleName`), so it is never set.
    external_helpers_module_name: Option<NodeId>,
    next_list_element_pos: i64,
    write_kind: WriteKind,
    source_maps_disabled: bool,
    source_map_source: Option<SourceMapSource>,
    container_pos: i64,
    container_end: i64,
    declaration_list_container_end: i64,
    detached_comments_info: Vec<DetachedCommentsInfo>,
    comments_disabled: bool,
    in_extends: bool,
}

/// A node position for line comparisons; synthesized tokens and parents
/// upstream creates on the fly are represented by their range without a node.
#[derive(Clone, Copy)]
pub(crate) struct Span {
    node: Option<NodeId>,
    pos: i64,
    end: i64,
}

impl Span {
    fn of(node: &NodeRead<'_>) -> Self {
        Self {
            node: Some(node.id()),
            pos: i64::from(node.pos()),
            end: i64::from(node.end()),
        }
    }
    /// `ast.NodeIsSynthesized`: the node's position is synthesized.
    fn node_is_synthesized(self) -> bool {
        position_is_synthesized(self.pos)
    }
    fn range(self) -> TextRange {
        TextRange::new(self.pos, self.end)
    }
}

fn position_is_synthesized(position: i64) -> bool {
    position < 0
}

fn is_punctuation_kind(kind: K) -> bool {
    (K::FirstPunctuation as u16..=K::LastPunctuation as u16).contains(&(kind as u16))
}

fn is_keyword_kind(kind: K) -> bool {
    (K::FirstKeyword as u16..=K::LastKeyword as u16).contains(&(kind as u16))
}

fn is_type_node_kind(kind: K) -> bool {
    (K::FirstTypeNode as u16..=K::LastTypeNode as u16).contains(&(kind as u16))
        || matches!(
            kind,
            K::AnyKeyword
                | K::UnknownKeyword
                | K::NumberKeyword
                | K::BigIntKeyword
                | K::ObjectKeyword
                | K::BooleanKeyword
                | K::StringKeyword
                | K::SymbolKeyword
                | K::VoidKeyword
                | K::UndefinedKeyword
                | K::NeverKeyword
                | K::IntrinsicKeyword
                | K::ExpressionWithTypeArguments
                | K::JSDocAllType
                | K::JSDocNullableType
                | K::JSDocNonNullableType
                | K::JSDocOptionalType
                | K::JSDocVariadicType
        )
}

// port: tsc/internal/printer/utilities.go:greatestEnd
fn greatest_end(end: i64, ends: &[Option<i64>]) -> i64 {
    ends.iter()
        .rev()
        .flatten()
        .fold(end, |acc, value| acc.max(*value))
}

/// Recursion guard for the emitters that recurse once per nested node.
fn guard<R>(work: impl FnOnce() -> R) -> R {
    stacker::maybe_grow(128 * 1024, 2 * 1024 * 1024, work)
}

impl<'a> Session<'a, '_> {
    pub(crate) fn node(&self, id: NodeId) -> Result<NodeRead<'a>, Error> {
        Ok(self.view.node(id)?)
    }

    fn known_kind(&self, id: NodeId) -> Result<K, Error> {
        let read = self.node(id)?;
        read.kind().known().ok_or(Error::UnexpectedKind {
            context: "node kind",
            kind: read.kind(),
        })
    }

    fn list_nodes(&self, list: NodeListId) -> Result<Vec<NodeId>, Error> {
        let slice = self.view.list(list)?.nodes();
        let read = self.view.node_slice(slice)?;
        read.iter()
            .map(|entry| entry.ok_or(Error::MissingNode("list entry")))
            .collect()
    }

    fn list_nodes_of(&self, list: Option<NodeListId>) -> Result<Vec<NodeId>, Error> {
        match list {
            Some(list) => self.list_nodes(list),
            None => Ok(Vec::new()),
        }
    }

    fn list_len(&self, list: Option<NodeListId>) -> Result<usize, Error> {
        match list {
            Some(list) => Ok(self.view.list(list)?.nodes().len()),
            None => Ok(0),
        }
    }

    fn list_range(&self, list: Option<NodeListId>) -> Result<Option<TextRange>, Error> {
        match list {
            Some(list) => Ok(Some(self.view.list(list)?.loc())),
            None => Ok(None),
        }
    }

    fn emit_flags(&self, node: NodeId) -> u32 {
        self.printer.emit_context.emit_flags(node)
    }

    //
    // Top-level setup
    //

    /// The emit context records no helper module names yet, so
    /// `externalHelpersModuleName` stays nil; source maps are disabled, so
    /// `setSourceMapSource` returns at its guard.
    // port: tsc/internal/printer/printer.go:Printer.setSourceFile
    fn set_source_file(&mut self, source_file: Option<NodeId>) -> Result<(), Error> {
        self.current_source = match source_file {
            Some(file) => Some((file, self.view.source_file(file)?)),
            None => None,
        };
        self.unique_helper_names = None;
        self.external_helpers_module_name = None;
        if let Some(file) = source_file {
            let original = self.printer.emit_context.most_original(file);
            if self.emit_flags(original) & ef::EXTERNAL_HELPERS != 0 {
                self.unique_helper_names = Some(HashMap::new());
            }
            self.set_source_map_source(file)?;
        }
        Ok(())
    }

    /// Boundary: past its guard this registers the file with the source-map
    /// generator.
    fn set_source_map_source(&mut self, _source: NodeId) -> Result<(), Error> {
        if self.source_maps_disabled {
            return Ok(());
        }
        Err(Error::Unsupported("source-map emission"))
    }

    //
    // Low-level writing
    //

    // port: tsc/internal/printer/printer.go:Printer.writeAs
    fn write_as(&mut self, text: &[u8], write_kind: WriteKind) {
        match write_kind {
            WriteKind::None => self.writer.write(text),
            WriteKind::Parameter => self.write_parameter(text),
            WriteKind::Keyword => self.write_keyword(text),
            WriteKind::Operator => self.write_operator(text),
            WriteKind::Property => self.write_property(text),
            WriteKind::Punctuation => self.write_punctuation(text),
            WriteKind::StringLiteral => self.writer.write_string_literal(text),
            WriteKind::Comment => self.write_comment(text),
            WriteKind::Literal => self.write_literal(text),
        }
    }

    // port: tsc/internal/printer/printer.go:Printer.write
    fn write(&mut self, text: &[u8]) {
        self.write_as(text, self.write_kind);
    }

    /// Unused upstream at the pin; the emitters save and restore the field.
    // port: tsc/internal/printer/printer.go:Printer.setWriteKind
    #[allow(dead_code, reason = "unused upstream at the pin")]
    fn set_write_kind(&mut self, kind: WriteKind) -> WriteKind {
        let previous = self.write_kind;
        self.write_kind = kind;
        previous
    }

    // port: tsc/internal/printer/printer.go:Printer.writeSymbol
    fn write_symbol(&mut self, text: &[u8], symbol: Option<SymbolId>) {
        match symbol {
            None => self.write(text),
            Some(symbol) => self.writer.write_symbol(text, Some(symbol)),
        }
    }

    // port: tsc/internal/printer/printer.go:Printer.writeLiteral
    fn write_literal(&mut self, text: &[u8]) {
        self.writer.write_literal(text);
    }

    // port: tsc/internal/printer/printer.go:Printer.writePunctuation
    fn write_punctuation(&mut self, text: &[u8]) {
        self.writer.write_punctuation(text);
    }

    // port: tsc/internal/printer/printer.go:Printer.writeOperator
    fn write_operator(&mut self, text: &[u8]) {
        self.writer.write_operator(text);
    }

    // port: tsc/internal/printer/printer.go:Printer.writeKeyword
    fn write_keyword(&mut self, text: &[u8]) {
        self.writer.write_keyword(text);
    }

    // port: tsc/internal/printer/printer.go:Printer.writeProperty
    fn write_property(&mut self, text: &[u8]) {
        self.writer.write_property(text);
    }

    // port: tsc/internal/printer/printer.go:Printer.writeParameter
    fn write_parameter(&mut self, text: &[u8]) {
        self.writer.write_parameter(text);
    }

    // port: tsc/internal/printer/printer.go:Printer.writeComment
    fn write_comment(&mut self, text: &[u8]) {
        self.writer.write_comment(text);
    }

    // port: tsc/internal/printer/printer.go:Printer.writeSpace
    fn write_space(&mut self) {
        self.writer.write_space(b" ");
    }

    // port: tsc/internal/printer/printer.go:Printer.writeLine
    fn write_line(&mut self) {
        self.writer.write_line();
    }

    // port: tsc/internal/printer/printer.go:Printer.writeLineRepeat
    fn write_line_repeat(&mut self, count: i64) {
        for _ in 0..count {
            self.write_line();
        }
    }

    /// Writes the lines of a helper's text without their common indentation,
    /// each on a new line; blank lines are dropped.
    // port: tsc/internal/printer/printer.go:Printer.writeLines
    #[allow(dead_code, reason = "its caller, emitHelpers, is a boundary")]
    fn write_lines(&mut self, text: &[u8]) {
        let lines = tsr_jsstring::text::split_lines(text);
        let indentation = tsr_jsstring::text::guess_indentation(&lines);
        for line in lines {
            let line = if indentation > 0 {
                &line[indentation..]
            } else {
                line
            };
            if !line.is_empty() {
                self.write_line();
                self.write(line);
            }
        }
    }

    // port: tsc/internal/printer/printer.go:Printer.writeTrailingSemicolon
    fn write_trailing_semicolon(&mut self) {
        self.writer.write_trailing_semicolon(b";");
    }

    // port: tsc/internal/printer/printer.go:Printer.increaseIndent
    fn increase_indent(&mut self) {
        self.writer.increase_indent();
    }

    // port: tsc/internal/printer/printer.go:Printer.decreaseIndent
    fn decrease_indent(&mut self) {
        self.writer.decrease_indent();
    }

    // port: tsc/internal/printer/printer.go:Printer.increaseIndentIf
    fn increase_indent_if(&mut self, requested: bool) {
        if requested {
            self.increase_indent();
        }
    }

    // port: tsc/internal/printer/printer.go:Printer.decreaseIndentIf
    fn decrease_indent_if(&mut self, requested: bool) {
        if requested {
            self.decrease_indent();
        }
    }

    /// Writes the lines between `node` and `parent`'s start when source lines
    /// are preserved, indenting; returns whether it indented.
    // port: tsc/internal/printer/printer.go:Printer.writeLineSeparatorsAndIndentBefore
    fn write_line_separators_and_indent_before(
        &mut self,
        node: NodeId,
        parent: Span,
    ) -> Result<bool, Error> {
        if self.printer.options.preserve_source_newlines {
            let leading_newlines =
                self.get_leading_line_terminator_count(Some(parent), Some(node), lf::NONE)?;
            if leading_newlines > 0 {
                self.write_lines_and_indent(leading_newlines, false);
                return Ok(true);
            }
        }
        Ok(false)
    }

    // port: tsc/internal/printer/printer.go:Printer.writeLineSeparatorsAfter
    fn write_line_separators_after(&mut self, node: NodeId, parent: Span) -> Result<(), Error> {
        if self.printer.options.preserve_source_newlines {
            let trailing_newlines = self.get_closing_line_terminator_count(
                Some(parent),
                Some(node),
                lf::NONE,
                TextRange::new(-1, -1),
            )?;
            if trailing_newlines > 0 {
                self.write_line_repeat(trailing_newlines);
            }
        }
        Ok(())
    }

    // port: tsc/internal/printer/printer.go:Printer.shouldEmitIndented
    fn should_emit_indented(&self, node: NodeId) -> bool {
        self.emit_flags(node) & ef::INDENTED != 0
    }

    // port: tsc/internal/printer/printer.go:Printer.shouldElideIndentation
    fn should_elide_indentation(&self, node: NodeId) -> bool {
        self.emit_flags(node) & ef::NO_INDENTATION != 0
    }

    // port: tsc/internal/printer/printer.go:Printer.shouldEmitOnSingleLine
    fn should_emit_on_single_line(&self, node: NodeId) -> bool {
        self.emit_flags(node) & ef::SINGLE_LINE != 0
    }

    // port: tsc/internal/printer/printer.go:Printer.shouldEmitOnMultipleLines
    fn should_emit_on_multiple_lines(&self, node: NodeId) -> bool {
        self.emit_flags(node) & ef::MULTI_LINE != 0
    }

    // port: tsc/internal/printer/printer.go:Printer.shouldEmitOnNewLine
    fn should_emit_on_new_line(&self, node: Option<NodeId>, format: ListFormat) -> bool {
        if node.is_some_and(|node| self.emit_flags(node) & ef::START_ON_NEW_LINE != 0) {
            return true;
        }
        format & lf::PREFER_NEW_LINE != 0
    }

    //
    // Line positions
    //

    pub(crate) fn source_text(&self) -> Option<&[u8]> {
        self.current_source
            .as_ref()
            .map(|(_, source)| source.text().as_bytes())
    }

    fn line_starts(&self) -> Option<&[i32]> {
        self.current_source
            .as_ref()
            .map(|(_, source)| source.ecma_line_map())
    }

    // port: tsc/internal/printer/utilities.go:getStartPositionOfRange
    fn get_start_position_of_range(&self, pos: i64, include_comments: bool) -> i64 {
        if position_is_synthesized(pos) {
            return -1;
        }
        let text = self
            .source_text()
            .expect("line comparisons need a source file");
        tsr_scanner::skip_trivia_ex(
            text,
            pos,
            Some(&tsr_scanner::SkipTriviaOptions {
                stop_at_comments: include_comments,
                ..Default::default()
            }),
        )
    }

    // port: tsc/internal/printer/utilities.go:GetLinesBetweenPositions
    fn get_lines_between_positions(&self, pos1: i64, pos2: i64) -> i64 {
        if pos1 == pos2 {
            return 0;
        }
        let line_starts = self
            .line_starts()
            .expect("line comparisons need a source file");
        let (lower, upper, negative) = if pos1 < pos2 {
            (pos1, pos2, false)
        } else {
            (pos2, pos1, true)
        };
        let lower_line =
            tsr_jsstring::scanner_positions::compute_line_of_position(line_starts, lower as isize);
        let upper_line = lower_line
            + tsr_jsstring::scanner_positions::compute_line_of_position(
                &line_starts[lower_line as usize..],
                upper as isize,
            );
        let lines = (upper_line - lower_line) as i64;
        if negative {
            -lines
        } else {
            lines
        }
    }

    // port: tsc/internal/printer/utilities.go:getLinesBetweenRangeEndAndRangeStart
    fn get_lines_between_range_end_and_range_start(
        &self,
        range1: Span,
        range2: Span,
        include_second_range_comments: bool,
    ) -> i64 {
        let range2_start =
            self.get_start_position_of_range(range2.pos, include_second_range_comments);
        self.get_lines_between_positions(range1.end, range2_start)
    }

    // port: tsc/internal/printer/utilities.go:getLinesBetweenPositionAndPrecedingNonWhitespaceCharacter
    fn get_lines_between_position_and_preceding_non_whitespace_character(
        &self,
        pos: i64,
        stop_pos: i64,
        include_comments: bool,
    ) -> i64 {
        let text = self.source_text().expect("needs a source file");
        let start_pos = tsr_scanner::skip_trivia_ex(
            text,
            pos,
            Some(&tsr_scanner::SkipTriviaOptions {
                stop_at_comments: include_comments,
                ..Default::default()
            }),
        );
        let prev_pos = self.get_previous_non_whitespace_position(start_pos, stop_pos);
        self.get_lines_between_positions(if prev_pos >= 0 { prev_pos } else { stop_pos }, start_pos)
    }

    // port: tsc/internal/printer/utilities.go:getLinesBetweenPositionAndNextNonWhitespaceCharacter
    fn get_lines_between_position_and_next_non_whitespace_character(
        &self,
        pos: i64,
        stop_pos: i64,
        include_comments: bool,
    ) -> i64 {
        let text = self.source_text().expect("needs a source file");
        let next_pos = tsr_scanner::skip_trivia_ex(
            text,
            pos,
            Some(&tsr_scanner::SkipTriviaOptions {
                stop_at_comments: include_comments,
                ..Default::default()
            }),
        );
        self.get_lines_between_positions(pos, stop_pos.min(next_pos))
    }

    /// Reads single bytes as runes, as upstream does; an index past the text
    /// panics as upstream's does.
    // port: tsc/internal/printer/utilities.go:getPreviousNonWhitespacePosition
    fn get_previous_non_whitespace_position(&self, mut pos: i64, stop_pos: i64) -> i64 {
        let text = self.source_text().expect("needs a source file");
        while pos >= stop_pos {
            let byte = text[usize::try_from(pos).expect("index out of range")];
            if !tsr_jsstring::classify::is_white_space_like(i32::from(byte)) {
                return pos;
            }
            pos -= 1;
        }
        -1
    }

    // port: tsc/internal/printer/utilities.go:PositionsAreOnSameLine
    fn positions_are_on_same_line(&self, pos1: i64, pos2: i64) -> bool {
        self.get_lines_between_positions(pos1, pos2) == 0
    }

    // port: tsc/internal/printer/utilities.go:RangeIsOnSingleLine
    fn range_is_on_single_line(&self, range: Span) -> bool {
        self.range_start_is_on_same_line_as_range_end(range, range)
    }

    // port: tsc/internal/printer/utilities.go:RangeStartPositionsAreOnSameLine
    fn range_start_positions_are_on_same_line(&self, range1: Span, range2: Span) -> bool {
        self.positions_are_on_same_line(
            self.get_start_position_of_range(range1.pos, false),
            self.get_start_position_of_range(range2.pos, false),
        )
    }

    // port: tsc/internal/printer/utilities.go:rangeEndPositionsAreOnSameLine
    fn range_end_positions_are_on_same_line(&self, range1: Span, range2: Span) -> bool {
        self.positions_are_on_same_line(range1.end, range2.end)
    }

    // port: tsc/internal/printer/utilities.go:rangeStartIsOnSameLineAsRangeEnd
    fn range_start_is_on_same_line_as_range_end(&self, range1: Span, range2: Span) -> bool {
        self.positions_are_on_same_line(
            self.get_start_position_of_range(range1.pos, false),
            range2.end,
        )
    }

    // port: tsc/internal/printer/utilities.go:rangeEndIsOnSameLineAsRangeStart
    fn range_end_is_on_same_line_as_range_start(&self, range1: Span, range2: Span) -> bool {
        self.positions_are_on_same_line(
            range1.end,
            self.get_start_position_of_range(range2.pos, false),
        )
    }

    // port: tsc/internal/printer/utilities.go:originalNodesHaveSameParent
    fn original_nodes_have_same_parent(
        &self,
        node_a: NodeId,
        node_b: NodeId,
    ) -> Result<bool, Error> {
        let node_a = self.printer.emit_context.most_original(node_a);
        let parent_a = self.node(node_a)?.parent();
        if parent_a.is_some() {
            // For performance, do not call `MostOriginal` for `nodeB` if `nodeA` doesn't even
            // have a parent node.
            let node_b = self.printer.emit_context.most_original(node_b);
            return Ok(parent_a == self.node(node_b)?.parent());
        }
        Ok(false)
    }

    // port: tsc/internal/printer/utilities.go:siblingNodePositionsAreComparable
    fn sibling_node_positions_are_comparable(
        &self,
        previous_node: NodeId,
        next_node: NodeId,
    ) -> Result<bool, Error> {
        if self.node(next_node)?.pos() < self.node(previous_node)?.end() {
            return Ok(false);
        }
        let previous_node = self.printer.emit_context.most_original(previous_node);
        let next_node = self.printer.emit_context.most_original(next_node);
        let parent = self.node(previous_node)?.parent();
        if parent.is_none() || parent != self.node(next_node)?.parent() {
            return Ok(false);
        }
        if let Some(parent_node_array) = self.get_containing_node_array(previous_node)? {
            let nodes = self
                .view
                .node_slice(self.view.list(parent_node_array)?.nodes())?;
            let index_of = |node: NodeId| nodes.iter().position(|entry| entry == Some(node));
            let prev_node_index = index_of(previous_node);
            return Ok(prev_node_index.is_some()
                && index_of(next_node) == prev_node_index.map(|index| index + 1));
        }
        Ok(false)
    }

    // port: tsc/internal/printer/utilities.go:getContainingNodeArray
    fn get_containing_node_array(&self, node: NodeId) -> Result<Option<NodeListId>, Error> {
        let read = self.node(node)?;
        let Some(parent) = read.parent() else {
            return Ok(None);
        };
        let parent_read = self.node(parent)?;
        let parent_kind = parent_read.kind();
        match read.kind().known() {
            Some(K::TypeParameter) => {
                if tsr_ast::utilities::is_function_like(Some(&parent_read))
                    || tsr_ast::utilities::is_class_like(&parent_read)
                    || parent_kind == K::InterfaceDeclaration
                    || tsr_ast::utilities_positions::is_type_or_js_type_alias_declaration(
                        &parent_read,
                    )
                {
                    return Ok(parent_read.type_parameter_list());
                } else if parent_kind != K::InferType {
                    return Err(Error::UnexpectedKind {
                        context: "Unexpected TypeParameter parent",
                        kind: parent_kind,
                    });
                }
            }
            Some(K::Parameter) => return Ok(parent_read.parameter_list()),
            Some(K::TemplateLiteralTypeSpan) => {
                return Ok(parent_read
                    .data_source()
                    .as_template_literal_type_node()
                    .ok_or(Error::InterfaceConversion {
                        found: parent_kind,
                        expected: "TemplateLiteralTypeNode",
                    })?
                    .template_spans())
            }
            Some(K::TemplateSpan) => {
                return Ok(parent_read
                    .data_source()
                    .as_template_expression()
                    .ok_or(Error::InterfaceConversion {
                        found: parent_kind,
                        expected: "TemplateExpression",
                    })?
                    .template_spans())
            }
            Some(K::Decorator) => {
                if crate::utilities::can_have_decorators(parent_kind) {
                    if let Some(modifiers) = parent_read.modifiers() {
                        return Ok(Some(modifiers));
                    }
                }
                return Ok(None);
            }
            Some(K::HeritageClause) => {
                if tsr_ast::utilities::is_class_like(&parent_read) {
                    let data = parent_read.data_source();
                    return Ok(match parent_kind.known() {
                        Some(K::ClassExpression) => data
                            .as_class_expression()
                            .and_then(|class| class.heritage_clauses()),
                        _ => data
                            .as_class_declaration()
                            .and_then(|class| class.heritage_clauses()),
                    });
                }
                return Ok(parent_read
                    .data_source()
                    .as_interface_declaration()
                    .ok_or(Error::InterfaceConversion {
                        found: parent_kind,
                        expected: "InterfaceDeclaration",
                    })?
                    .heritage_clauses());
            }
            _ => {}
        }

        match parent_kind.known() {
            Some(K::TypeLiteral | K::InterfaceDeclaration) => {
                if tsr_ast::utilities::is_type_element(&read) {
                    return Ok(parent_read.member_list());
                }
            }
            Some(K::UnionType) => {
                return Ok(parent_read
                    .data_source()
                    .as_union_type_node()
                    .and_then(|union| union.types()))
            }
            Some(K::IntersectionType) => {
                return Ok(parent_read
                    .data_source()
                    .as_intersection_type_node()
                    .and_then(|intersection| intersection.types()))
            }
            Some(K::ArrayLiteralExpression | K::TupleType | K::NamedImports | K::NamedExports) => {
                return Ok(parent_read.element_list())
            }
            Some(K::ObjectLiteralExpression | K::JsxAttributes) => {
                return Ok(parent_read.property_list())
            }
            Some(K::CallExpression | K::NewExpression) => {
                if tsr_ast::utilities::is_type_node(&read) {
                    return Ok(parent_read.type_argument_list());
                }
                if Some(node) != parent_read.expression() {
                    return Ok(parent_read.argument_list());
                }
            }
            Some(K::JsxElement | K::JsxFragment) => {
                if tsr_ast::utilities::is_jsx_child(&read) {
                    return Ok(parent_read.children_list());
                }
            }
            Some(K::JsxOpeningElement | K::JsxSelfClosingElement) => {
                if tsr_ast::utilities::is_type_node(&read) {
                    return Ok(parent_read.type_argument_list());
                }
            }
            Some(K::Block | K::ModuleBlock | K::CaseClause | K::DefaultClause) => {
                return Ok(parent_read.statement_list())
            }
            Some(K::CaseBlock) => {
                return Ok(parent_read
                    .data_source()
                    .as_case_block()
                    .and_then(|block| block.clauses()))
            }
            Some(K::ClassDeclaration | K::ClassExpression) => {
                if tsr_ast::utilities::is_class_element(&read) {
                    return Ok(parent_read.member_list());
                }
            }
            Some(K::EnumDeclaration) => {
                if read.kind() == K::EnumMember {
                    return Ok(parent_read.member_list());
                }
            }
            Some(K::SourceFile) if tsr_ast::utilities::is_statement(self.view, node)? => {
                return Ok(parent_read.statement_list());
            }
            _ => {}
        }

        if tsr_ast::utilities::is_modifier(&read) {
            if let Some(modifiers) = parent_read.modifiers() {
                return Ok(Some(modifiers));
            }
        }
        Ok(None)
    }

    // port: tsc/internal/printer/utilities.go:skipSynthesizedParentheses
    fn skip_synthesized_parentheses(&self, span: Span) -> Result<Span, Error> {
        let mut span = span;
        while let Some(id) = span.node {
            let read = self.node(id)?;
            if read.kind() != K::ParenthesizedExpression
                || !tsr_ast::utilities::node_is_synthesized(&read)
            {
                break;
            }
            let inner = read
                .data_source()
                .as_parenthesized_expression()
                .and_then(|node| node.expression())
                .ok_or(Error::MissingNode("parenthesized expression"))?;
            span = Span::of(&self.node(inner)?);
        }
        Ok(span)
    }

    // port: tsc/internal/printer/printer.go:Printer.getLinesBetweenNodes
    fn get_lines_between_nodes(
        &self,
        parent: NodeId,
        node1: Span,
        node2: Span,
    ) -> Result<i64, Error> {
        if self.should_elide_indentation(parent) {
            return Ok(0);
        }
        let parent = self.skip_synthesized_parentheses(Span::of(&self.node(parent)?))?;
        let node1 = self.skip_synthesized_parentheses(node1)?;
        let node2 = self.skip_synthesized_parentheses(node2)?;
        // Always use a newline for synthesized code if the synthesizer desires it.
        if self.should_emit_on_new_line(node2.node, lf::NONE) {
            return Ok(1);
        }
        if self.current_source.is_some()
            && !parent.node_is_synthesized()
            && !node1.node_is_synthesized()
            && !node2.node_is_synthesized()
        {
            if self.printer.options.preserve_source_newlines {
                return Ok(self.get_effective_lines(|include_comments| {
                    self.get_lines_between_range_end_and_range_start(node1, node2, include_comments)
                }));
            }
            return Ok(i64::from(
                !self.range_end_is_on_same_line_as_range_start(node1, node2),
            ));
        }
        Ok(0)
    }

    /// The line difference to a position's adjacent comments counts as one
    /// line, not two; a comment on the same line counts as none.
    // port: tsc/internal/printer/printer.go:Printer.getEffectiveLines
    fn get_effective_lines(&self, get_line_difference: impl Fn(bool) -> i64) -> i64 {
        // If 'preserveSourceNewlines' is disabled, we should never call this function
        // because it could be more expensive than alternative approximations.
        assert!(
            self.printer.options.preserve_source_newlines,
            "Should not be called when preserveSourceNewlines is false"
        );
        let lines = get_line_difference(true);
        if lines == 0 {
            return get_line_difference(false);
        }
        lines
    }

    // port: tsc/internal/printer/printer.go:Printer.getLeadingLineTerminatorCount
    fn get_leading_line_terminator_count(
        &self,
        parent: Option<Span>,
        first_child: Option<NodeId>,
        format: ListFormat,
    ) -> Result<i64, Error> {
        if format & lf::PRESERVE_LINES != 0 || self.printer.options.preserve_source_newlines {
            if format & lf::PREFER_NEW_LINE != 0 {
                return Ok(1);
            }
            let Some(first_child) = first_child else {
                return Ok(i64::from(!parent.is_none_or(|parent| {
                    self.current_source.is_some() && self.range_is_on_single_line(parent)
                })));
            };
            let first = self.node(first_child)?;
            if self.next_list_element_pos > 0
                && i64::from(first.pos()) == self.next_list_element_pos
            {
                // The parent list already wrote this child's leading line terminators
                // as its separating line terminators: in a class, the newline
                // between a constructor and `public foo() {}` is the class
                // member list's, not the modifier list's.
                return Ok(0);
            }
            if first.kind() == K::JsxText {
                // JsxText will be written with its leading whitespace, so don't add more manually.
                return Ok(0);
            }
            if let Some(parent) = parent {
                if self.current_source.is_some()
                    && !position_is_synthesized(parent.pos)
                    && !tsr_ast::utilities::node_is_synthesized(&first)
                    && first.parent().is_none()
                {
                    if self.printer.options.preserve_source_newlines {
                        let first_pos = i64::from(first.pos());
                        return Ok(self.get_effective_lines(|include_comments| {
                            self.get_lines_between_position_and_preceding_non_whitespace_character(
                                first_pos,
                                parent.pos,
                                include_comments,
                            )
                        }));
                    }
                    return Ok(i64::from(
                        !self.range_start_positions_are_on_same_line(parent, Span::of(&first)),
                    ));
                }
            }
            if self.should_emit_on_new_line(Some(first_child), format) {
                return Ok(1);
            }
        }
        Ok(i64::from(format & lf::MULTI_LINE != 0))
    }

    // port: tsc/internal/printer/printer.go:Printer.getSeparatingLineTerminatorCount
    fn get_separating_line_terminator_count(
        &self,
        previous: Option<NodeId>,
        next: Option<NodeId>,
        format: ListFormat,
    ) -> Result<i64, Error> {
        if format & lf::PRESERVE_LINES != 0 || self.printer.options.preserve_source_newlines {
            let (Some(previous), Some(next)) = (previous, next) else {
                return Ok(0);
            };
            let previous_read = self.node(previous)?;
            let next_read = self.node(next)?;
            if next_read.kind() == K::JsxText {
                // JsxText will be written with its leading whitespace, so don't add more manually.
                return Ok(0);
            } else if self.current_source.is_some()
                && !tsr_ast::utilities::node_is_synthesized(&previous_read)
                && !tsr_ast::utilities::node_is_synthesized(&next_read)
            {
                if self.printer.options.preserve_source_newlines
                    && self.sibling_node_positions_are_comparable(previous, next)?
                {
                    return Ok(self.get_effective_lines(|include_comments| {
                        self.get_lines_between_range_end_and_range_start(
                            Span::of(&previous_read),
                            Span::of(&next_read),
                            include_comments,
                        )
                    }));
                } else if !self.printer.options.preserve_source_newlines
                    && self.original_nodes_have_same_parent(previous, next)?
                {
                    // Without preserveSourceNewlines, nodes of one parent on
                    // separate lines keep a single line terminator.
                    return Ok(i64::from(!self.range_end_is_on_same_line_as_range_start(
                        Span::of(&previous_read),
                        Span::of(&next_read),
                    )));
                }
                // Otherwise the format says whether new lines are preferred.
                return Ok(i64::from(format & lf::PREFER_NEW_LINE != 0));
            } else if self.should_emit_on_new_line(Some(previous), format)
                || self.should_emit_on_new_line(Some(next), format)
            {
                return Ok(1);
            }
        } else if self.should_emit_on_new_line(next, lf::NONE) {
            return Ok(1);
        }
        Ok(i64::from(format & lf::MULTI_LINE != 0))
    }

    // port: tsc/internal/printer/printer.go:Printer.getClosingLineTerminatorCount
    fn get_closing_line_terminator_count(
        &self,
        parent: Option<Span>,
        last_child: Option<NodeId>,
        format: ListFormat,
        children_text_range: TextRange,
    ) -> Result<i64, Error> {
        if format & lf::PRESERVE_LINES != 0 || self.printer.options.preserve_source_newlines {
            if format & lf::PREFER_NEW_LINE != 0 {
                return Ok(1);
            }
            let Some(last_child) = last_child else {
                return Ok(i64::from(!parent.is_none_or(|parent| {
                    self.current_source.is_some() && self.range_is_on_single_line(parent)
                })));
            };
            let last = self.node(last_child)?;
            if let Some(parent) = parent {
                if self.current_source.is_some()
                    && !position_is_synthesized(parent.pos)
                    && !tsr_ast::utilities::node_is_synthesized(&last)
                    && (last.parent().is_none()
                        || parent.node.is_some() && last.parent() == parent.node)
                {
                    if self.printer.options.preserve_source_newlines {
                        let end =
                            greatest_end(i64::from(last.end()), &[Some(children_text_range.end())]);
                        return Ok(self.get_effective_lines(|include_comments| {
                            self.get_lines_between_position_and_next_non_whitespace_character(
                                end,
                                parent.end,
                                include_comments,
                            )
                        }));
                    }
                    return Ok(i64::from(
                        !self.range_end_positions_are_on_same_line(parent, Span::of(&last)),
                    ));
                }
            }
            if self.should_emit_on_new_line(Some(last_child), format) {
                return Ok(1);
            }
        }
        if format & lf::MULTI_LINE != 0 && format & lf::NO_TRAILING_NEW_LINE == 0 {
            return Ok(1);
        }
        Ok(0)
    }

    //
    // Tokens/Keywords
    //

    // port: tsc/internal/printer/printer.go:Printer.writeTokenText
    fn write_token_text(&mut self, token: K, write_kind: WriteKind, pos: i64) -> i64 {
        let token_string = token_to_string(token).as_bytes();
        self.write_as(token_string, write_kind);
        if position_is_synthesized(pos) {
            pos
        } else {
            pos + token_string.len() as i64
        }
    }

    // port: tsc/internal/printer/printer.go:Printer.emitToken
    fn emit_token(
        &mut self,
        token: K,
        pos: i64,
        write_kind: WriteKind,
        context: NodeId,
    ) -> Result<i64, Error> {
        self.emit_token_ex(token, pos, write_kind, Some(context), tef::NONE)
    }

    /// `context` is `None` for a node the printer creates itself.
    // port: tsc/internal/printer/printer.go:Printer.emitTokenEx
    fn emit_token_ex(
        &mut self,
        token: K,
        pos: i64,
        write_kind: WriteKind,
        context: Option<NodeId>,
        flags: tef::TokenEmitFlags,
    ) -> Result<i64, Error> {
        let (state, pos) = self.enter_token(token, pos, context, flags)?;
        let pos = self.write_token_text(token, write_kind, pos);
        self.exit_token(token, pos, context, state)?;
        Ok(pos)
    }

    // port: tsc/internal/printer/printer.go:Printer.emitKeywordNode
    fn emit_keyword_node(&mut self, node: Option<NodeId>) -> Result<(), Error> {
        self.emit_keyword_node_ex(node, tef::NONE)
    }

    // port: tsc/internal/printer/printer.go:Printer.emitKeywordNodeEx
    fn emit_keyword_node_ex(
        &mut self,
        node: Option<NodeId>,
        flags: tef::TokenEmitFlags,
    ) -> Result<(), Error> {
        let Some(node) = node else {
            return Ok(());
        };
        let read = self.node(node)?;
        let kind = self.known_kind(node)?;
        let state = self.enter_token_node(node, flags)?;
        self.write_token_text(kind, WriteKind::Keyword, i64::from(read.pos()));
        self.exit_token_node(node, state)
    }

    // port: tsc/internal/printer/printer.go:Printer.emitPunctuationNode
    fn emit_punctuation_node(&mut self, node: Option<NodeId>) -> Result<(), Error> {
        self.emit_punctuation_node_ex(node, tef::NONE)
    }

    // port: tsc/internal/printer/printer.go:Printer.emitPunctuationNodeEx
    fn emit_punctuation_node_ex(
        &mut self,
        node: Option<NodeId>,
        flags: tef::TokenEmitFlags,
    ) -> Result<(), Error> {
        let Some(node) = node else {
            return Ok(());
        };
        let read = self.node(node)?;
        let kind = self.known_kind(node)?;
        let state = self.enter_token_node(node, flags)?;
        self.write_token_text(kind, WriteKind::Punctuation, i64::from(read.pos()));
        self.exit_token_node(node, state)
    }

    // port: tsc/internal/printer/printer.go:Printer.emitTokenNode
    fn emit_token_node(&mut self, node: Option<NodeId>) -> Result<(), Error> {
        self.emit_token_node_ex(node, tef::NONE)
    }

    // port: tsc/internal/printer/printer.go:Printer.emitTokenNodeEx
    fn emit_token_node_ex(
        &mut self,
        node: Option<NodeId>,
        flags: tef::TokenEmitFlags,
    ) -> Result<(), Error> {
        let Some(node) = node else {
            return Ok(());
        };
        let kind = self.known_kind(node)?;
        if is_keyword_kind(kind) {
            self.emit_keyword_node_ex(Some(node), flags)
        } else if is_punctuation_kind(kind) {
            self.emit_punctuation_node_ex(Some(node), flags)
        } else {
            Err(Error::UnexpectedKind {
                context: "unexpected TokenNode",
                kind: kind.into(),
            })
        }
    }

    //
    // Snippet elements
    //

    /// `EmitContext.SnippetElement`. The emit context has no snippet table yet
    /// (the language service sets it), so no node carries one.
    #[allow(clippy::unused_self, reason = "the emit context's snippet table")]
    fn snippet_element(&self, _node: NodeId) -> Option<SnippetElement> {
        None
    }

    // port: tsc/internal/printer/printer.go:Printer.emitSnippetNode
    fn emit_snippet_node(&mut self, node: NodeId, snippet: SnippetElement) -> Result<(), Error> {
        match snippet.kind {
            SnippetKind::TabStop => self.emit_tab_stop(node, snippet),
        }
    }

    // port: tsc/internal/printer/printer.go:Printer.emitTabStop
    fn emit_tab_stop(&mut self, node: NodeId, snippet: SnippetElement) -> Result<(), Error> {
        let kind = self.node(node)?.kind();
        if kind != K::EmptyStatement {
            return Err(Error::UnexpectedKind {
                context: "Snippet tab stops can only be emitted on empty statements",
                kind,
            });
        }
        self.writer
            .raw_write(format!("${}", snippet.order).as_bytes());
        Ok(())
    }

    //
    // Literals
    //

    // port: tsc/internal/printer/printer.go:Printer.emitLiteral
    fn emit_literal(&mut self, node: NodeId, mut flags: LiteralTextFlags) -> Result<(), Error> {
        if self.printer.options.never_ascii_escape {
            flags = with_flag(flags, LiteralEscapeFlags::NEVER_ASCII_ESCAPE);
        }
        if self.printer.options.terminate_unterminated_literals {
            flags = with_flag(flags, LiteralEscapeFlags::TERMINATE_UNTERMINATED_LITERALS);
        }
        let text = self.get_literal_text_of_node(node, flags)?;
        // Quick info expects every literal through WriteStringLiteral.
        self.writer.write_string_literal(&text);
        Ok(())
    }

    // port: tsc/internal/printer/printer.go:Printer.emitNumericLiteral
    fn emit_numeric_literal(&mut self, node: NodeId) -> Result<(), Error> {
        let state = self.enter_node(node)?;
        self.emit_literal(node, LiteralEscapeFlags::NONE)?;
        self.exit_node(node, state)?;
        Ok(())
    }

    // port: tsc/internal/printer/printer.go:Printer.emitBigIntLiteral
    fn emit_big_int_literal(&mut self, node: NodeId) -> Result<(), Error> {
        let state = self.enter_node(node)?;
        self.emit_literal(node, LiteralEscapeFlags::NONE)?;
        self.exit_node(node, state)?;
        Ok(())
    }

    // port: tsc/internal/printer/printer.go:Printer.emitStringLiteral
    fn emit_string_literal(&mut self, node: NodeId) -> Result<(), Error> {
        let state = self.enter_node(node)?;
        self.emit_literal(node, LiteralEscapeFlags::NONE)?;
        self.exit_node(node, state)?;
        Ok(())
    }

    // port: tsc/internal/printer/printer.go:Printer.emitNoSubstitutionTemplateLiteral
    fn emit_no_substitution_template_literal(&mut self, node: NodeId) -> Result<(), Error> {
        let state = self.enter_node(node)?;
        self.emit_literal(node, LiteralEscapeFlags::NONE)?;
        self.exit_node(node, state)?;
        Ok(())
    }

    // port: tsc/internal/printer/printer.go:Printer.emitRegularExpressionLiteral
    fn emit_regular_expression_literal(&mut self, node: NodeId) -> Result<(), Error> {
        let state = self.enter_node(node)?;
        self.emit_literal(node, LiteralEscapeFlags::NONE)?;
        self.exit_node(node, state)?;
        Ok(())
    }

    // port: tsc/internal/printer/printer.go:Printer.emitTemplateHead
    fn emit_template_head(&mut self, node: NodeId) -> Result<(), Error> {
        let state = self.enter_node(node)?;
        self.emit_literal(node, LiteralEscapeFlags::NONE)?;
        self.exit_node(node, state)?;
        Ok(())
    }

    // port: tsc/internal/printer/printer.go:Printer.emitTemplateMiddle
    fn emit_template_middle(&mut self, node: NodeId) -> Result<(), Error> {
        let state = self.enter_node(node)?;
        self.emit_literal(node, LiteralEscapeFlags::NONE)?;
        self.exit_node(node, state)?;
        Ok(())
    }

    // port: tsc/internal/printer/printer.go:Printer.emitTemplateTail
    fn emit_template_tail(&mut self, node: NodeId) -> Result<(), Error> {
        let state = self.enter_node(node)?;
        self.emit_literal(node, LiteralEscapeFlags::NONE)?;
        self.exit_node(node, state)?;
        Ok(())
    }

    // port: tsc/internal/printer/printer.go:Printer.emitTemplateMiddleTail
    fn emit_template_middle_tail(&mut self, node: NodeId) -> Result<(), Error> {
        match self.known_kind(node)? {
            K::TemplateMiddle => self.emit_template_middle(node),
            K::TemplateTail => self.emit_template_tail(node),
            _ => Ok(()),
        }
    }

    //
    // Names
    //

    /// Identifier and literal text. A generated name's text comes from the
    /// `NameGenerator` (boundary); the emit context records no string-literal
    /// text sources yet.
    // port: tsc/internal/printer/printer.go:Printer.getTextOfNode
    fn get_text_of_node(&self, node: NodeId, include_trivia: bool) -> Result<Vec<u8>, Error> {
        let read = self.node(node)?;
        if tsr_ast::utilities::is_member_name(&read)
            && self.printer.emit_context.has_auto_generate_info(node)
        {
            return Err(Error::Unsupported("NameGenerator.GenerateName"));
        }
        let can_use_source_file = self.current_source.is_some()
            && read.parent().is_some()
            && !tsr_ast::utilities::node_is_synthesized(&read);
        match read.kind().known() {
            Some(K::Identifier | K::PrivateIdentifier) => {
                if !can_use_source_file || !self.node_belongs_to_current_source(node)? {
                    return Ok(match read.kind().known() {
                        Some(K::Identifier) => read
                            .data_source()
                            .as_identifier()
                            .ok_or(Error::MissingNode("identifier payload"))?
                            .text()
                            .to_vec(),
                        _ => read
                            .data_source()
                            .as_private_identifier()
                            .ok_or(Error::MissingNode("private identifier payload"))?
                            .text()
                            .to_vec(),
                    });
                }
                let text = self.source_text().expect("checked above");
                Ok(tsr_scanner::get_text_of_node_from_source_text(
                    self.view,
                    text,
                    Some(node),
                    include_trivia,
                )?
                .as_bytes()
                .to_vec())
            }
            Some(K::JsxNamespacedName) => Err(Error::Unsupported("JsxNamespacedName text")),
            Some(
                K::StringLiteral
                | K::NumericLiteral
                | K::BigIntLiteral
                | K::NoSubstitutionTemplateLiteral
                | K::TemplateHead
                | K::TemplateMiddle
                | K::TemplateTail,
            ) => self.get_literal_text_of_node(node, LiteralEscapeFlags::NONE),
            _ => Err(Error::UnexpectedKind {
                context: "getTextOfNode",
                kind: read.kind(),
            }),
        }
    }

    /// `GetSourceFileOfNode(node) == MostOriginal(currentSourceFile)`.
    fn node_belongs_to_current_source(&self, node: NodeId) -> Result<bool, Error> {
        let Some((file, _)) = &self.current_source else {
            return Ok(false);
        };
        let original = self.printer.emit_context.most_original(*file);
        Ok(tsr_ast::utilities::get_source_file_of_node(self.view, Some(node))? == Some(original))
    }

    // port: tsc/internal/printer/printer.go:Printer.emitIdentifierText
    fn emit_identifier_text(&mut self, node: NodeId) -> Result<(), Error> {
        if self.printer.emit_context.has_auto_generate_info(node) {
            return Err(Error::Unsupported("NameGenerator.GenerateName"));
        }
        let text = self.get_text_of_node(node, false)?;
        let symbol = self
            .printer
            .id_to_symbol
            .as_ref()
            .and_then(|map| map.get(&node).copied());
        if symbol.is_some() {
            self.write_symbol(&text, symbol);
            return Ok(());
        }
        self.write(&text);
        Ok(())
    }

    // port: tsc/internal/printer/printer.go:Printer.emitIdentifierName
    fn emit_identifier_name(&mut self, node: NodeId) -> Result<(), Error> {
        let state = self.enter_node(node)?;
        self.emit_identifier_text(node)?;
        self.exit_node(node, state)?;
        Ok(())
    }

    /// A helper name (`__helper`) is substituted with `tslib_1.__helper` when the
    /// file imports its helpers, or with a unique name in a module whose own
    /// declarations may conflict. Both substitutions are boundaries: the emit
    /// context records no helper module name yet, and unique helper names come
    /// from the helper table and the name generator.
    // port: tsc/internal/printer/printer.go:Printer.emitIdentifierReference
    fn emit_identifier_reference(&mut self, node: NodeId) -> Result<(), Error> {
        if (self.external_helpers_module_name.is_some() || self.unique_helper_names.is_some())
            && self.emit_flags(node) & ef::HELPER_NAME != 0
        {
            if self.external_helpers_module_name.is_some() {
                return Err(Error::Unsupported(
                    "external helpers module name substitution",
                ));
            }
            if self.unique_helper_names.is_some() {
                return Err(Error::Unsupported("getUniqueHelperName"));
            }
        }
        let state = self.enter_node(node)?;
        self.emit_identifier_text(node)?;
        self.exit_node(node, state)?;
        Ok(())
    }

    // port: tsc/internal/printer/printer.go:Printer.emitBindingIdentifier
    fn emit_binding_identifier(&mut self, node: NodeId) -> Result<(), Error> {
        if self.unique_helper_names.is_some() && self.emit_flags(node) & ef::HELPER_NAME != 0 {
            // Substituting `__helper` with `__helper_1` in an ES module needs the
            // helper table's unique names.
            return Err(Error::Unsupported("getUniqueHelperName"));
        }
        let state = self.enter_node(node)?;
        self.emit_identifier_text(node)?;
        self.exit_node(node, state)?;
        Ok(())
    }

    // port: tsc/internal/printer/printer.go:Printer.emitPrivateIdentifier
    fn emit_private_identifier(&mut self, node: NodeId) -> Result<(), Error> {
        let state = self.enter_node(node)?;
        let text = self.get_text_of_node(node, false)?;
        self.write(&text);
        self.exit_node(node, state)?;
        Ok(())
    }

    // port: tsc/internal/printer/printer.go:Printer.emitQualifiedName
    fn emit_qualified_name(&mut self, node: NodeId) -> Result<(), Error> {
        let state = self.enter_node(node)?;
        let read = self.node(node)?;
        let name = read
            .data_source()
            .as_qualified_name()
            .ok_or(Error::MissingNode("qualified name payload"))?;
        let left = name
            .left()
            .ok_or(Error::MissingNode("qualified name left"))?;
        let right = name
            .right()
            .ok_or(Error::MissingNode("qualified name right"))?;
        self.emit_entity_name(left)?;
        self.write_punctuation(b".");
        self.emit_member_name(Some(right))?;
        self.exit_node(node, state)?;
        Ok(())
    }

    // port: tsc/internal/printer/printer.go:Printer.emitComputedPropertyName
    fn emit_computed_property_name(&mut self, node: NodeId) -> Result<(), Error> {
        let state = self.enter_node(node)?;
        let expression = self
            .node(node)?
            .data_source()
            .as_computed_property_name()
            .and_then(|name| name.expression())
            .ok_or(Error::MissingNode("computed property name expression"))?;
        self.write_punctuation(b"[");
        self.emit_expression(expression, op::DISALLOW_COMMA)?;
        self.write_punctuation(b"]");
        self.exit_node(node, state)?;
        Ok(())
    }

    // port: tsc/internal/printer/printer.go:Printer.emitEntityName
    fn emit_entity_name(&mut self, node: NodeId) -> Result<(), Error> {
        stacker::maybe_grow(128 * 1024, 2 * 1024 * 1024, || {
            self.emit_entity_name_worker(node)
        })
    }

    fn emit_entity_name_worker(&mut self, node: NodeId) -> Result<(), Error> {
        match self.known_kind(node)? {
            K::Identifier => self.emit_identifier_reference(node),
            K::QualifiedName => self.emit_qualified_name(node),
            // TypeQuery nodes may carry a property access as their name.
            K::PropertyAccessExpression => self.emit_expression(node, op::DISALLOW_COMMA),
            kind => Err(Error::UnexpectedKind {
                context: "unexpected EntityName",
                kind: kind.into(),
            }),
        }
    }

    // port: tsc/internal/printer/printer.go:Printer.emitBindingName
    fn emit_binding_name(&mut self, node: Option<NodeId>) -> Result<(), Error> {
        stacker::maybe_grow(128 * 1024, 2 * 1024 * 1024, || {
            self.emit_binding_name_worker(node)
        })
    }

    fn emit_binding_name_worker(&mut self, node: Option<NodeId>) -> Result<(), Error> {
        let Some(node) = node else {
            return Ok(());
        };
        match self.known_kind(node)? {
            K::Identifier => self.emit_binding_identifier(node),
            K::ObjectBindingPattern | K::ArrayBindingPattern => self.emit_binding_pattern(node),
            kind => Err(Error::UnexpectedKind {
                context: "unexpected BindingName",
                kind: kind.into(),
            }),
        }
    }

    // port: tsc/internal/printer/printer.go:Printer.emitObjectBindingPattern
    // port: tsc/internal/printer/printer.go:Printer.emitArrayBindingPattern
    fn emit_binding_pattern(&mut self, node: NodeId) -> Result<(), Error> {
        let state = self.enter_node(node)?;
        let read = self.node(node)?;
        let elements = read.element_list();
        let object = read.kind() == K::ObjectBindingPattern;
        self.write_punctuation(if object { b"{" } else { b"[" });
        self.emit_list(
            Self::emit_binding_element_node,
            node,
            elements,
            if object {
                lf::OBJECT_BINDING_PATTERN_ELEMENTS
            } else {
                lf::ARRAY_BINDING_PATTERN_ELEMENTS
            },
        )?;
        self.write_punctuation(if object { b"}" } else { b"]" });
        self.exit_node(node, state)?;
        Ok(())
    }

    // port: tsc/internal/printer/printer.go:Printer.emitBindingElement
    fn emit_binding_element(&mut self, node: NodeId) -> Result<(), Error> {
        let state = self.enter_node(node)?;
        let read = self.node(node)?;
        let data = read
            .data_source()
            .as_binding_element()
            .ok_or(Error::MissingNode("binding element"))?;
        let (rest, property, name, initializer) = (
            data.dot_dot_dot_token(),
            data.property_name(),
            data.name(),
            data.initializer(),
        );
        self.emit_token_node(rest)?;
        if property.is_some() {
            self.emit_property_name(property)?;
            self.write_punctuation(b":");
            self.write_space();
        }
        if let Some(name) = name {
            self.emit_binding_name(Some(name))?;
            self.emit_initializer(initializer, i64::from(self.node(name)?.end()), node)?;
        }
        self.exit_node(node, state)?;
        Ok(())
    }

    // port: tsc/internal/printer/printer.go:Printer.emitBindingElementNode
    fn emit_binding_element_node(&mut self, node: NodeId) -> Result<(), Error> {
        self.emit_binding_element(node)
    }

    // port: tsc/internal/printer/printer.go:Printer.emitPropertyName
    fn emit_property_name(&mut self, node: Option<NodeId>) -> Result<(), Error> {
        let Some(node) = node else {
            return Ok(());
        };
        let saved = self.write_kind;
        self.write_kind = WriteKind::Property;
        let result = match self.known_kind(node)? {
            K::Identifier => self.emit_identifier_name(node),
            K::PrivateIdentifier => self.emit_private_identifier(node),
            K::StringLiteral => self.emit_string_literal(node),
            K::NoSubstitutionTemplateLiteral => self.emit_no_substitution_template_literal(node),
            K::NumericLiteral => self.emit_numeric_literal(node),
            K::BigIntLiteral => self.emit_big_int_literal(node),
            K::ComputedPropertyName => self.emit_computed_property_name(node),
            kind => Err(Error::UnexpectedKind {
                context: "unexpected PropertyName",
                kind: kind.into(),
            }),
        };
        self.write_kind = saved;
        result
    }

    // port: tsc/internal/printer/printer.go:Printer.emitMemberName
    fn emit_member_name(&mut self, node: Option<NodeId>) -> Result<(), Error> {
        let Some(node) = node else {
            return Ok(());
        };
        match self.known_kind(node)? {
            K::Identifier => self.emit_identifier_name(node),
            K::PrivateIdentifier => self.emit_private_identifier(node),
            kind => Err(Error::UnexpectedKind {
                context: "unexpected MemberName",
                kind: kind.into(),
            }),
        }
    }

    //
    // Signature elements
    //

    /// Returns the position after the modifiers, for the token emitters that
    /// follow.
    // port: tsc/internal/printer/printer.go:Printer.emitModifierList
    fn emit_modifier_list(
        &mut self,
        parent: NodeId,
        modifiers: Option<NodeListId>,
        allow_decorators: bool,
    ) -> Result<i64, Error> {
        let parent_pos = i64::from(self.node(parent)?.pos());
        let Some(list) = modifiers else {
            return Ok(parent_pos);
        };
        let nodes = self.list_nodes(list)?;
        if nodes.is_empty() {
            return Ok(parent_pos);
        }
        let mut decorators = Vec::with_capacity(nodes.len());
        for &node in &nodes {
            decorators.push(self.node(node)?.kind() == K::Decorator);
        }
        let last_end = i64::from(self.node(*nodes.last().expect("nonempty"))?.end());
        let result = greatest_end(parent_pos, &[Some(last_end)]);
        if decorators.iter().all(|&decorator| !decorator) {
            // Every modifier-like is a modifier: emit the list as modifiers.
            self.emit_list(
                Self::emit_keyword_node_item,
                parent,
                Some(list),
                lf::MODIFIERS,
            )?;
            return Ok(result);
        }
        if decorators.iter().all(|&decorator| decorator) {
            if !allow_decorators {
                return Ok(parent_pos);
            }
            self.emit_list(Self::emit_modifier_like, parent, Some(list), lf::DECORATORS)?;
            return Ok(result);
        }

        // Contiguous chunks of modifiers or of decorators, so that each chunk
        // is formatted consistently.
        self.writer.on_before_emit_node_list(list);
        let loc = self.view.list(list)?.loc();
        let (mut start, mut pos) = (0usize, 0usize);
        let mut last_mode: Option<bool> = None;
        let mut mode: Option<bool> = None;
        while start < nodes.len() {
            while pos < nodes.len() {
                mode = Some(decorators[pos]);
                match last_mode {
                    None => last_mode = mode,
                    Some(_) if mode != last_mode => break,
                    Some(_) => {}
                }
                pos += 1;
            }
            // Upstream compares the chunk's end with the index of the last
            // modifier rather than with the length of the list.
            let mut range = TextRange::new(-1, -1);
            if start == 0 {
                range = TextRange::new(loc.pos(), range.end());
            }
            if pos + 1 == nodes.len() {
                range = TextRange::new(range.pos(), loc.end());
            }
            let chunk_is_modifiers = last_mode == Some(false);
            if allow_decorators || chunk_is_modifiers {
                self.emit_list_items(
                    Self::emit_modifier_like,
                    parent,
                    &nodes[start..pos],
                    if chunk_is_modifiers {
                        lf::MODIFIERS
                    } else {
                        lf::DECORATORS
                    },
                    false,
                    range,
                )?;
            }
            start = pos;
            last_mode = mode;
            pos += 1;
        }
        self.writer.on_after_emit_node_list(list);
        Ok(result)
    }

    fn emit_keyword_node_item(&mut self, node: NodeId) -> Result<(), Error> {
        self.emit_keyword_node(Some(node))
    }

    // port: tsc/internal/printer/printer.go:Printer.emitTypeParameter
    fn emit_type_parameter(&mut self, node: NodeId) -> Result<(), Error> {
        let state = self.enter_node(node)?;
        let read = self.node(node)?;
        let declaration = read
            .data_source()
            .as_type_parameter_declaration()
            .ok_or(Error::MissingNode("type parameter payload"))?;
        let (modifiers, name, constraint, default_type) = (
            declaration.modifiers(),
            declaration.name(),
            declaration.constraint(),
            declaration.default_type(),
        );
        self.emit_modifier_list(node, modifiers, false)?;
        self.emit_binding_identifier(name.ok_or(Error::MissingNode("type parameter name"))?)?;
        if let Some(constraint) = constraint {
            self.write_space();
            self.write_keyword(b"extends");
            self.write_space();
            self.emit_type_node_outside_extends(constraint)?;
        }
        if let Some(default_type) = default_type {
            self.write_space();
            self.write_operator(b"=");
            self.write_space();
            self.emit_type_node_outside_extends(default_type)?;
        }
        self.exit_node(node, state)?;
        Ok(())
    }

    /// Quick info stores type arguments in place of type parameters on
    /// instantiated signatures; both print here.
    // port: tsc/internal/printer/printer.go:Printer.emitTypeParameterDeclarationNode
    fn emit_type_parameter_declaration_node(&mut self, node: NodeId) -> Result<(), Error> {
        if self.known_kind(node)? == K::TypeParameter {
            self.emit_type_parameter(node)
        } else {
            self.emit_type_argument(node)
        }
    }

    // port: tsc/internal/printer/printer.go:Printer.emitParameterName
    fn emit_parameter_name(&mut self, node: Option<NodeId>) -> Result<(), Error> {
        let saved = self.write_kind;
        self.write_kind = WriteKind::Parameter;
        let result = self.emit_binding_name(node);
        self.write_kind = saved;
        result
    }

    // port: tsc/internal/printer/printer.go:Printer.emitParameter
    fn emit_parameter(&mut self, node: NodeId) -> Result<(), Error> {
        let state = self.enter_node(node)?;
        let read = self.node(node)?;
        let parameter = read
            .data_source()
            .as_parameter_declaration()
            .ok_or(Error::MissingNode("parameter payload"))?;
        let (modifiers, dot_dot_dot, name, question, type_node, initializer) = (
            parameter.modifiers(),
            parameter.dot_dot_dot_token(),
            parameter.name(),
            parameter.question_token(),
            parameter.r#type(),
            parameter.initializer(),
        );
        self.emit_modifier_list(node, modifiers, true)?;
        self.emit_token_node(dot_dot_dot)?;
        self.emit_parameter_name(name)?;
        self.emit_token_node(question)?;
        self.emit_type_annotation(type_node)?;
        // The parser can make a parameter declaration with just an
        // initializer, so the position falls back to any present child.
        let mut ends = Vec::new();
        for child in [type_node, question, name].into_iter().flatten() {
            ends.push(Some(i64::from(self.node(child)?.end())));
        }
        ends.push(self.list_range(modifiers)?.map(TextRange::end));
        let equals_pos = greatest_end(i64::from(read.pos()), &ends);
        self.emit_initializer(initializer, equals_pos, node)?;
        self.exit_node(node, state)?;
        Ok(())
    }

    // port: tsc/internal/printer/printer.go:Printer.emitParameterDeclarationNode
    fn emit_parameter_declaration_node(&mut self, node: NodeId) -> Result<(), Error> {
        self.emit_parameter(node)
    }

    // port: tsc/internal/printer/printer.go:Printer.emitTypeParameters
    fn emit_type_parameters(
        &mut self,
        parent: NodeId,
        nodes: Option<NodeListId>,
    ) -> Result<(), Error> {
        if nodes.is_none() {
            return Ok(());
        }
        // Upstream allows the trailing comma for arrow functions only, until it
        // preserves it everywhere `shouldAllowTrailingComma` says.
        let trailing = if self.node(parent)?.kind() == K::ArrowFunction {
            lf::ALLOW_TRAILING_COMMA
        } else {
            lf::NONE
        };
        self.emit_list(
            Self::emit_type_parameter_declaration_node,
            parent,
            nodes,
            lf::TYPE_PARAMETERS | trailing,
        )
    }

    // port: tsc/internal/printer/printer.go:Printer.emitTypeAnnotation
    fn emit_type_annotation(&mut self, node: Option<NodeId>) -> Result<(), Error> {
        let Some(node) = node else {
            return Ok(());
        };
        self.write_punctuation(b":");
        self.write_space();
        self.emit_type_node_outside_extends(node)
    }

    // port: tsc/internal/printer/printer.go:Printer.emitInitializer
    fn emit_initializer(
        &mut self,
        node: Option<NodeId>,
        equal_token_pos: i64,
        context: NodeId,
    ) -> Result<(), Error> {
        let Some(node) = node else {
            return Ok(());
        };
        self.write_space();
        self.emit_token(
            K::EqualsToken,
            equal_token_pos,
            WriteKind::Operator,
            context,
        )?;
        self.write_space();
        self.emit_expression(node, op::DISALLOW_COMMA)
    }

    // port: tsc/internal/printer/printer.go:Printer.emitParameters
    fn emit_parameters(
        &mut self,
        parent: NodeId,
        parameters: Option<NodeListId>,
    ) -> Result<(), Error> {
        self.generate_all_names(parameters)?;
        self.emit_list(
            Self::emit_parameter_declaration_node,
            parent,
            parameters,
            lf::PARAMETERS,
        )
    }

    // port: tsc/internal/printer/printer.go:Printer.emitParametersForIndexSignature
    fn emit_parameters_for_index_signature(
        &mut self,
        parent: NodeId,
        parameters: Option<NodeListId>,
    ) -> Result<(), Error> {
        self.generate_all_names(parameters)?;
        self.emit_list(
            Self::emit_parameter_declaration_node,
            parent,
            parameters,
            lf::INDEX_SIGNATURE_PARAMETERS,
        )
    }

    /// Type parameters, parameters and return type of a function-like node.
    // port: tsc/internal/printer/printer.go:Printer.emitSignature
    fn emit_signature(&mut self, node: NodeId) -> Result<(), Error> {
        let read = self.node(node)?;
        let data = read.data_source();
        let (type_parameters, parameters, type_node) = match read.kind().known() {
            Some(K::MethodSignature) => {
                let method = data
                    .as_method_signature_declaration()
                    .ok_or(Error::MissingNode("method signature"))?;
                (
                    method.type_parameters(),
                    method.parameters(),
                    method.r#type(),
                )
            }
            Some(K::CallSignature) => {
                let signature = data
                    .as_call_signature_declaration()
                    .ok_or(Error::MissingNode("call signature"))?;
                (
                    signature.type_parameters(),
                    signature.parameters(),
                    signature.r#type(),
                )
            }
            Some(K::ConstructSignature) => {
                let signature = data
                    .as_construct_signature_declaration()
                    .ok_or(Error::MissingNode("construct signature"))?;
                (
                    signature.type_parameters(),
                    signature.parameters(),
                    signature.r#type(),
                )
            }
            Some(K::FunctionType) => {
                let function = data
                    .as_function_type_node()
                    .ok_or(Error::MissingNode("function type"))?;
                (
                    function.type_parameters(),
                    function.parameters(),
                    function.r#type(),
                )
            }
            Some(K::ConstructorType) => {
                let constructor = data
                    .as_constructor_type_node()
                    .ok_or(Error::MissingNode("constructor type"))?;
                (
                    constructor.type_parameters(),
                    constructor.parameters(),
                    constructor.r#type(),
                )
            }
            // `FunctionLikeData`: the declarations and expressions with a body.
            Some(
                K::GetAccessor
                | K::SetAccessor
                | K::FunctionDeclaration
                | K::FunctionExpression
                | K::ArrowFunction
                | K::MethodDeclaration
                | K::Constructor,
            ) => (
                read.type_parameter_list(),
                read.parameter_list(),
                read.type_node(),
            ),
            _ => {
                return Err(Error::UnexpectedKind {
                    context: "FunctionLikeData",
                    kind: read.kind(),
                })
            }
        };
        self.emit_type_parameters(node, type_parameters)?;
        self.emit_parameters(node, parameters)?;
        self.emit_type_annotation(type_node)
    }

    //
    // Type members
    //

    // port: tsc/internal/printer/printer.go:Printer.emitPropertySignature
    fn emit_property_signature(&mut self, node: NodeId) -> Result<(), Error> {
        let state = self.enter_node(node)?;
        let read = self.node(node)?;
        let property = read
            .data_source()
            .as_property_signature_declaration()
            .ok_or(Error::MissingNode("property signature payload"))?;
        let (modifiers, name, postfix, type_node) = (
            property.modifiers(),
            property.name(),
            property.postfix_token(),
            property.r#type(),
        );
        self.emit_modifier_list(node, modifiers, false)?;
        self.emit_property_name(name)?;
        self.emit_token_node(postfix)?;
        self.emit_type_annotation(type_node)?;
        self.write_trailing_semicolon();
        self.exit_node(node, state)?;
        Ok(())
    }

    // port: tsc/internal/printer/printer.go:Printer.emitMethodSignature
    fn emit_method_signature(&mut self, node: NodeId) -> Result<(), Error> {
        let state = self.enter_node(node)?;
        let read = self.node(node)?;
        let method = read
            .data_source()
            .as_method_signature_declaration()
            .ok_or(Error::MissingNode("method signature payload"))?;
        let (modifiers, name, postfix) =
            (method.modifiers(), method.name(), method.postfix_token());
        self.emit_modifier_list(node, modifiers, false)?;
        self.emit_property_name(name)?;
        self.emit_token_node(postfix)?;
        let indented = self.should_emit_indented(node);
        self.increase_indent_if(indented);
        self.push_name_generation_scope(Some(node));
        self.emit_signature(node)?;
        self.write_trailing_semicolon();
        self.pop_name_generation_scope(Some(node));
        self.decrease_indent_if(indented);
        self.exit_node(node, state)?;
        Ok(())
    }

    // port: tsc/internal/printer/printer.go:Printer.emitCallSignature
    fn emit_call_signature(&mut self, node: NodeId) -> Result<(), Error> {
        let state = self.enter_node(node)?;
        let indented = self.should_emit_indented(node);
        self.increase_indent_if(indented);
        self.push_name_generation_scope(Some(node));
        self.emit_signature(node)?;
        self.write_trailing_semicolon();
        self.pop_name_generation_scope(Some(node));
        self.decrease_indent_if(indented);
        self.exit_node(node, state)?;
        Ok(())
    }

    // port: tsc/internal/printer/printer.go:Printer.emitConstructSignature
    fn emit_construct_signature(&mut self, node: NodeId) -> Result<(), Error> {
        let state = self.enter_node(node)?;
        self.write_keyword(b"new");
        self.write_space();
        let indented = self.should_emit_indented(node);
        self.increase_indent_if(indented);
        self.push_name_generation_scope(Some(node));
        self.emit_signature(node)?;
        self.write_trailing_semicolon();
        self.pop_name_generation_scope(Some(node));
        self.decrease_indent_if(indented);
        self.exit_node(node, state)?;
        Ok(())
    }

    // port: tsc/internal/printer/printer.go:Printer.emitIndexSignature
    fn emit_index_signature(&mut self, node: NodeId) -> Result<(), Error> {
        let state = self.enter_node(node)?;
        let read = self.node(node)?;
        let index = read
            .data_source()
            .as_index_signature_declaration()
            .ok_or(Error::MissingNode("index signature payload"))?;
        let (modifiers, parameters, type_node) =
            (index.modifiers(), index.parameters(), index.r#type());
        self.emit_modifier_list(node, modifiers, false)?;
        let indented = self.should_emit_indented(node);
        self.increase_indent_if(indented);
        self.push_name_generation_scope(Some(node));
        self.emit_parameters_for_index_signature(node, parameters)?;
        self.emit_type_annotation(type_node)?;
        self.write_trailing_semicolon();
        self.pop_name_generation_scope(Some(node));
        self.decrease_indent_if(indented);
        self.exit_node(node, state)?;
        Ok(())
    }

    // port: tsc/internal/printer/printer.go:Printer.emitAccessorDeclaration
    fn emit_accessor_declaration(&mut self, token: K, node: NodeId) -> Result<(), Error> {
        let state = self.enter_node(node)?;
        let read = self.node(node)?;
        let (modifiers, name, body) = (read.modifiers(), read.name(), read.body());
        let pos = self.emit_modifier_list(node, modifiers, true)?;
        self.emit_token(token, pos, WriteKind::Keyword, node)?;
        self.write_space();
        self.emit_property_name(name)?;
        let indented = self.should_emit_indented(node);
        self.increase_indent_if(indented);
        self.push_name_generation_scope(Some(node));
        self.emit_signature(node)?;
        self.emit_function_body_node(body)?;
        self.pop_name_generation_scope(Some(node));
        self.decrease_indent_if(indented);
        self.exit_node(node, state)?;
        Ok(())
    }

    // port: tsc/internal/printer/printer.go:Printer.emitGetAccessorDeclaration
    fn emit_get_accessor_declaration(&mut self, node: NodeId) -> Result<(), Error> {
        self.emit_accessor_declaration(K::GetKeyword, node)
    }

    // port: tsc/internal/printer/printer.go:Printer.emitSetAccessorDeclaration
    fn emit_set_accessor_declaration(&mut self, node: NodeId) -> Result<(), Error> {
        self.emit_accessor_declaration(K::SetKeyword, node)
    }

    // port: tsc/internal/printer/printer.go:Printer.emitTypeElement
    fn emit_type_element(&mut self, node: NodeId) -> Result<(), Error> {
        match self.known_kind(node)? {
            K::PropertySignature => self.emit_property_signature(node),
            K::MethodSignature => self.emit_method_signature(node),
            K::CallSignature => self.emit_call_signature(node),
            K::ConstructSignature => self.emit_construct_signature(node),
            K::GetAccessor => self.emit_get_accessor_declaration(node),
            K::SetAccessor => self.emit_set_accessor_declaration(node),
            K::IndexSignature => self.emit_index_signature(node),
            K::NotEmittedTypeElement => self.emit_nothing(node),
            kind => Err(Error::UnexpectedKind {
                context: "unexpected TypeElement",
                kind: kind.into(),
            }),
        }
    }

    //
    // Types
    //

    // port: tsc/internal/printer/printer.go:Printer.emitKeywordTypeNode
    fn emit_keyword_type_node(&mut self, node: NodeId) -> Result<(), Error> {
        self.emit_keyword_node(Some(node))
    }

    // port: tsc/internal/printer/printer.go:Printer.emitTypePredicateParameterName
    fn emit_type_predicate_parameter_name(&mut self, node: NodeId) -> Result<(), Error> {
        match self.known_kind(node)? {
            K::Identifier => self.emit_identifier_reference(node),
            K::ThisType => self.emit_this_type(node),
            kind => Err(Error::UnexpectedKind {
                context: "unexpected TypePredicateParameterName",
                kind: kind.into(),
            }),
        }
    }

    // port: tsc/internal/printer/printer.go:Printer.emitTypePredicate
    fn emit_type_predicate(&mut self, node: NodeId) -> Result<(), Error> {
        let state = self.enter_node(node)?;
        let read = self.node(node)?;
        let predicate = read
            .data_source()
            .as_type_predicate_node()
            .ok_or(Error::MissingNode("type predicate payload"))?;
        let (asserts, parameter_name, type_node) = (
            predicate.asserts_modifier(),
            predicate.parameter_name(),
            predicate.r#type(),
        );
        if asserts.is_some() {
            self.emit_token_node(asserts)?;
            self.write_space();
        }
        self.emit_type_predicate_parameter_name(
            parameter_name.ok_or(Error::MissingNode("predicate parameter name"))?,
        )?;
        if let Some(type_node) = type_node {
            self.write_space();
            self.write_keyword(b"is");
            self.write_space();
            self.emit_type_node_outside_extends(type_node)?;
        }
        self.exit_node(node, state)?;
        Ok(())
    }

    // port: tsc/internal/printer/printer.go:Printer.emitTypeArgument
    fn emit_type_argument(&mut self, node: NodeId) -> Result<(), Error> {
        self.emit_type_node_outside_extends(node)
    }

    // port: tsc/internal/printer/printer.go:Printer.emitTypeArguments
    fn emit_type_arguments(
        &mut self,
        parent: NodeId,
        nodes: Option<NodeListId>,
    ) -> Result<(), Error> {
        if nodes.is_none() {
            return Ok(());
        }
        self.emit_list(
            Self::emit_type_parameter_declaration_node,
            parent,
            nodes,
            lf::TYPE_ARGUMENTS,
        )
    }

    // port: tsc/internal/printer/printer.go:Printer.emitTypeReference
    fn emit_type_reference(&mut self, node: NodeId) -> Result<(), Error> {
        let state = self.enter_node(node)?;
        let read = self.node(node)?;
        let reference = read
            .data_source()
            .as_type_reference_node()
            .ok_or(Error::MissingNode("type reference payload"))?;
        let (type_name, type_arguments) = (reference.type_name(), reference.type_arguments());
        self.emit_entity_name(type_name.ok_or(Error::MissingNode("type reference name"))?)?;
        self.emit_type_arguments(node, type_arguments)?;
        self.exit_node(node, state)?;
        Ok(())
    }

    /// The return type of a function or constructor type, including the arrow.
    // port: tsc/internal/printer/printer.go:Printer.emitReturnType
    fn emit_return_type(&mut self, node: Option<NodeId>) -> Result<(), Error> {
        let Some(node) = node else {
            return Ok(());
        };
        self.write_punctuation(b"=>");
        self.write_space();
        let constrained_infer = self.in_extends && self.known_kind(node)? == K::InferType && {
            let parameter = self
                .node(node)?
                .data_source()
                .as_infer_type_node()
                .and_then(|infer| infer.type_parameter())
                .ok_or(Error::MissingNode("infer type parameter"))?;
            self.node(parameter)?
                .data_source()
                .as_type_parameter_declaration()
                .is_some_and(|declaration| declaration.constraint().is_some())
        };
        if constrained_infer {
            // In the `extends` clause of a conditional type, `infer U extends V`
            // in a return position must be parenthesized to avoid an ambiguous parse.
            self.emit_type_node_preserving_extends(node, TypePrecedence::HIGHEST)
        } else {
            self.emit_type_node_preserving_extends(node, TypePrecedence::LOWEST)
        }
    }

    // port: tsc/internal/printer/printer.go:Printer.emitFunctionType
    fn emit_function_type(&mut self, node: NodeId) -> Result<(), Error> {
        let state = self.enter_node(node)?;
        let read = self.node(node)?;
        let function = read
            .data_source()
            .as_function_type_node()
            .ok_or(Error::MissingNode("function type payload"))?;
        let (type_parameters, parameters, type_node) = (
            function.type_parameters(),
            function.parameters(),
            function.r#type(),
        );
        let indented = self.should_emit_indented(node);
        self.increase_indent_if(indented);
        self.push_name_generation_scope(Some(node));
        self.emit_type_parameters(node, type_parameters)?;
        self.emit_parameters(node, parameters)?;
        self.write_space();
        self.emit_return_type(type_node)?;
        self.pop_name_generation_scope(Some(node));
        self.decrease_indent_if(indented);
        self.exit_node(node, state)?;
        Ok(())
    }

    // port: tsc/internal/printer/printer.go:Printer.emitConstructorType
    fn emit_constructor_type(&mut self, node: NodeId) -> Result<(), Error> {
        let state = self.enter_node(node)?;
        let read = self.node(node)?;
        let constructor = read
            .data_source()
            .as_constructor_type_node()
            .ok_or(Error::MissingNode("constructor type payload"))?;
        let (modifiers, type_parameters, parameters, type_node) = (
            constructor.modifiers(),
            constructor.type_parameters(),
            constructor.parameters(),
            constructor.r#type(),
        );
        self.emit_modifier_list(node, modifiers, false)?;
        self.write_keyword(b"new");
        self.write_space();
        let indented = self.should_emit_indented(node);
        self.increase_indent_if(indented);
        self.push_name_generation_scope(Some(node));
        self.emit_type_parameters(node, type_parameters)?;
        self.emit_parameters(node, parameters)?;
        self.write_space();
        self.emit_return_type(type_node)?;
        self.pop_name_generation_scope(Some(node));
        self.decrease_indent_if(indented);
        self.exit_node(node, state)?;
        Ok(())
    }

    // port: tsc/internal/printer/printer.go:Printer.emitTypeQuery
    fn emit_type_query(&mut self, node: NodeId) -> Result<(), Error> {
        let state = self.enter_node(node)?;
        let read = self.node(node)?;
        let query = read
            .data_source()
            .as_type_query_node()
            .ok_or(Error::MissingNode("type query payload"))?;
        let (expr_name, type_arguments) = (query.expr_name(), query.type_arguments());
        self.write_keyword(b"typeof");
        self.write_space();
        self.emit_entity_name(expr_name.ok_or(Error::MissingNode("type query name"))?)?;
        self.emit_type_arguments(node, type_arguments)?;
        self.exit_node(node, state)?;
        Ok(())
    }

    // port: tsc/internal/printer/printer.go:Printer.emitTypeLiteral
    fn emit_type_literal(&mut self, node: NodeId) -> Result<(), Error> {
        let state = self.enter_node(node)?;
        let members = self
            .node(node)?
            .data_source()
            .as_type_literal_node()
            .ok_or(Error::MissingNode("type literal payload"))?
            .members();
        self.push_name_generation_scope(Some(node));
        self.generate_all_member_names(members)?;
        self.write_punctuation(b"{");
        let flags = if self.should_emit_on_single_line(node) {
            lf::SINGLE_LINE_TYPE_LITERAL_MEMBERS
        } else {
            lf::MULTI_LINE_TYPE_LITERAL_MEMBERS
        };
        self.emit_list(
            Self::emit_type_element,
            node,
            members,
            flags | lf::NO_SPACE_IF_EMPTY,
        )?;
        self.write_punctuation(b"}");
        self.pop_name_generation_scope(Some(node));
        self.exit_node(node, state)?;
        Ok(())
    }

    // port: tsc/internal/printer/printer.go:Printer.emitArrayType
    fn emit_array_type(&mut self, node: NodeId) -> Result<(), Error> {
        let state = self.enter_node(node)?;
        let element = self
            .node(node)?
            .data_source()
            .as_array_type_node()
            .and_then(|array| array.element_type())
            .ok_or(Error::MissingNode("array element type"))?;
        self.emit_postfix_type_operand(element, node)?;
        self.write_punctuation(b"[");
        self.write_punctuation(b"]");
        self.exit_node(node, state)?;
        Ok(())
    }

    /// A parsed `typeof X` operand keeps its parse-tree form; a synthesized one is
    /// parenthesized like any other type-operator-precedence operand.
    // port: tsc/internal/printer/printer.go:Printer.emitPostfixTypeOperand
    fn emit_postfix_type_operand(&mut self, operand: NodeId, parent: NodeId) -> Result<(), Error> {
        let parent_is_parse_tree = self.node(parent)?.flags() & node_flags::SYNTHESIZED == 0;
        if parent_is_parse_tree && self.known_kind(operand)? == K::TypeQuery {
            return self.emit_type_node(operand, TypePrecedence::TypeOperator);
        }
        self.emit_type_node(operand, TypePrecedence::Postfix)
    }

    // port: tsc/internal/printer/printer.go:Printer.emitTupleElementType
    fn emit_tuple_element_type(&mut self, node: NodeId) -> Result<(), Error> {
        self.emit_type_node_outside_extends(node)
    }

    // port: tsc/internal/printer/printer.go:Printer.emitTupleType
    fn emit_tuple_type(&mut self, node: NodeId) -> Result<(), Error> {
        let state = self.enter_node(node)?;
        let read = self.node(node)?;
        let elements = read
            .data_source()
            .as_tuple_type_node()
            .ok_or(Error::MissingNode("tuple type payload"))?
            .elements();
        self.emit_token(
            K::OpenBracketToken,
            i64::from(read.pos()),
            WriteKind::Punctuation,
            node,
        )?;
        let flags = if self.should_emit_on_single_line(node) {
            lf::SINGLE_LINE_TUPLE_TYPE_ELEMENTS
        } else {
            lf::MULTI_LINE_TUPLE_TYPE_ELEMENTS
        };
        self.emit_list(
            Self::emit_tuple_element_type,
            node,
            elements,
            flags | lf::NO_SPACE_IF_EMPTY,
        )?;
        let elements_end = match elements {
            Some(list) => self.view.list(list)?.loc().end(),
            None => -1,
        };
        self.emit_token(
            K::CloseBracketToken,
            elements_end,
            WriteKind::Punctuation,
            node,
        )?;
        self.exit_node(node, state)?;
        Ok(())
    }

    // port: tsc/internal/printer/printer.go:Printer.emitRestType
    fn emit_rest_type(&mut self, node: NodeId) -> Result<(), Error> {
        let state = self.enter_node(node)?;
        let inner = self
            .node(node)?
            .data_source()
            .as_rest_type_node()
            .and_then(|rest| rest.r#type())
            .ok_or(Error::MissingNode("rest type"))?;
        self.write_punctuation(b"...");
        self.emit_type_node_outside_extends(inner)?;
        self.exit_node(node, state)?;
        Ok(())
    }

    // port: tsc/internal/printer/printer.go:Printer.emitOptionalType
    fn emit_optional_type(&mut self, node: NodeId) -> Result<(), Error> {
        let state = self.enter_node(node)?;
        let inner = self
            .node(node)?
            .data_source()
            .as_optional_type_node()
            .and_then(|optional| optional.r#type())
            .ok_or(Error::MissingNode("optional type"))?;
        self.emit_postfix_type_operand(inner, node)?;
        self.write_punctuation(b"?");
        self.exit_node(node, state)?;
        Ok(())
    }

    // port: tsc/internal/printer/printer.go:Printer.emitNamedTupleMember
    fn emit_named_tuple_member(&mut self, node: NodeId) -> Result<(), Error> {
        let state = self.enter_node(node)?;
        let read = self.node(node)?;
        let member = read
            .data_source()
            .as_named_tuple_member()
            .ok_or(Error::MissingNode("named tuple member payload"))?;
        let (dot_dot_dot, name, question, type_node) = (
            member.dot_dot_dot_token(),
            member.name(),
            member.question_token(),
            member.r#type(),
        );
        let name = name.ok_or(Error::MissingNode("named tuple member name"))?;
        self.emit_punctuation_node(dot_dot_dot)?;
        self.emit_identifier_name(name)?;
        self.emit_punctuation_node(question)?;
        let name_end = i64::from(self.node(name)?.end());
        let question_end = match question {
            Some(question) => Some(i64::from(self.node(question)?.end())),
            None => None,
        };
        self.emit_token(
            K::ColonToken,
            greatest_end(name_end, &[question_end]),
            WriteKind::Punctuation,
            node,
        )?;
        self.write_space();
        self.emit_type_node_outside_extends(
            type_node.ok_or(Error::MissingNode("named tuple member type"))?,
        )?;
        self.exit_node(node, state)?;
        Ok(())
    }

    // port: tsc/internal/printer/printer.go:Printer.emitUnionTypeConstituent
    fn emit_union_type_constituent(&mut self, node: NodeId) -> Result<(), Error> {
        self.emit_type_node(node, TypePrecedence::TypeOperator)
    }

    // port: tsc/internal/printer/printer.go:Printer.emitUnionType
    fn emit_union_type(&mut self, node: NodeId) -> Result<(), Error> {
        let state = self.enter_node(node)?;
        let types = self
            .node(node)?
            .data_source()
            .as_union_type_node()
            .ok_or(Error::MissingNode("union type payload"))?
            .types();
        self.emit_list(
            Self::emit_union_type_constituent,
            node,
            types,
            lf::UNION_TYPE_CONSTITUENTS,
        )?;
        self.exit_node(node, state)?;
        Ok(())
    }

    // port: tsc/internal/printer/printer.go:Printer.emitIntersectionTypeConstituent
    fn emit_intersection_type_constituent(&mut self, node: NodeId) -> Result<(), Error> {
        self.emit_type_node(node, TypePrecedence::TypeOperator)
    }

    // port: tsc/internal/printer/printer.go:Printer.emitIntersectionType
    fn emit_intersection_type(&mut self, node: NodeId) -> Result<(), Error> {
        let state = self.enter_node(node)?;
        let types = self
            .node(node)?
            .data_source()
            .as_intersection_type_node()
            .ok_or(Error::MissingNode("intersection type payload"))?
            .types();
        self.emit_list(
            Self::emit_intersection_type_constituent,
            node,
            types,
            lf::INTERSECTION_TYPE_CONSTITUENTS,
        )?;
        self.exit_node(node, state)?;
        Ok(())
    }

    // port: tsc/internal/printer/printer.go:Printer.emitConditionalType
    fn emit_conditional_type(&mut self, node: NodeId) -> Result<(), Error> {
        let state = self.enter_node(node)?;
        let read = self.node(node)?;
        let conditional = read
            .data_source()
            .as_conditional_type_node()
            .ok_or(Error::MissingNode("conditional type payload"))?;
        let (check, extends, true_type, false_type) = (
            conditional
                .check_type()
                .ok_or(Error::MissingNode("check type"))?,
            conditional
                .extends_type()
                .ok_or(Error::MissingNode("extends type"))?,
            conditional
                .true_type()
                .ok_or(Error::MissingNode("true type"))?,
            conditional
                .false_type()
                .ok_or(Error::MissingNode("false type"))?,
        );
        self.emit_type_node(check, TypePrecedence::Union)?;
        self.write_space();
        self.write_keyword(b"extends");
        self.write_space();
        self.emit_type_node_in_extends(extends)?;
        self.write_space();
        self.write_punctuation(b"?");
        self.write_space();
        self.emit_type_node_outside_extends(true_type)?;
        self.write_space();
        self.write_punctuation(b":");
        self.write_space();
        self.emit_type_node_outside_extends(false_type)?;
        self.exit_node(node, state)?;
        Ok(())
    }

    // port: tsc/internal/printer/printer.go:Printer.emitInferTypeParameter
    fn emit_infer_type_parameter(&mut self, node: NodeId) -> Result<(), Error> {
        let state = self.enter_node(node)?;
        let read = self.node(node)?;
        let declaration = read
            .data_source()
            .as_type_parameter_declaration()
            .ok_or(Error::MissingNode("infer type parameter payload"))?;
        let (name, constraint) = (declaration.name(), declaration.constraint());
        self.emit_binding_identifier(name.ok_or(Error::MissingNode("infer type parameter name"))?)?;
        if let Some(constraint) = constraint {
            self.write_space();
            self.write_keyword(b"extends");
            self.write_space();
            self.emit_type_node_in_extends(constraint)?;
        }
        self.exit_node(node, state)?;
        Ok(())
    }

    // port: tsc/internal/printer/printer.go:Printer.emitInferType
    fn emit_infer_type(&mut self, node: NodeId) -> Result<(), Error> {
        let state = self.enter_node(node)?;
        let parameter = self
            .node(node)?
            .data_source()
            .as_infer_type_node()
            .and_then(|infer| infer.type_parameter())
            .ok_or(Error::MissingNode("infer type parameter"))?;
        self.write_keyword(b"infer");
        self.write_space();
        self.emit_infer_type_parameter(parameter)?;
        self.exit_node(node, state)?;
        Ok(())
    }

    // port: tsc/internal/printer/printer.go:Printer.emitParenthesizedType
    fn emit_parenthesized_type(&mut self, node: NodeId) -> Result<(), Error> {
        let state = self.enter_node(node)?;
        let inner = self
            .node(node)?
            .data_source()
            .as_parenthesized_type_node()
            .and_then(|parenthesized| parenthesized.r#type())
            .ok_or(Error::MissingNode("parenthesized type"))?;
        self.write_punctuation(b"(");
        self.emit_type_node_outside_extends(inner)?;
        self.write_punctuation(b")");
        self.exit_node(node, state)?;
        Ok(())
    }

    // port: tsc/internal/printer/printer.go:Printer.emitThisType
    fn emit_this_type(&mut self, node: NodeId) -> Result<(), Error> {
        let state = self.enter_node(node)?;
        self.write_keyword(b"this");
        self.exit_node(node, state)
    }

    // port: tsc/internal/printer/printer.go:Printer.emitTypeOperator
    fn emit_type_operator(&mut self, node: NodeId) -> Result<(), Error> {
        let state = self.enter_node(node)?;
        let read = self.node(node)?;
        let operator_node = read
            .data_source()
            .as_type_operator_node()
            .ok_or(Error::MissingNode("type operator payload"))?;
        let (operator, inner) = (operator_node.operator(), operator_node.r#type());
        let operator = operator.known().ok_or(Error::UnexpectedKind {
            context: "TypeOperator",
            kind: operator,
        })?;
        self.emit_token(operator, i64::from(read.pos()), WriteKind::Keyword, node)?;
        self.write_space();
        let precedence = if operator == K::ReadonlyKeyword {
            TypePrecedence::Postfix
        } else {
            TypePrecedence::TypeOperator
        };
        self.emit_type_node(
            inner.ok_or(Error::MissingNode("type operator operand"))?,
            precedence,
        )?;
        self.exit_node(node, state)?;
        Ok(())
    }

    // port: tsc/internal/printer/printer.go:Printer.emitIndexedAccessType
    fn emit_indexed_access_type(&mut self, node: NodeId) -> Result<(), Error> {
        let state = self.enter_node(node)?;
        let read = self.node(node)?;
        let access = read
            .data_source()
            .as_indexed_access_type_node()
            .ok_or(Error::MissingNode("indexed access payload"))?;
        let (object, index) = (
            access
                .object_type()
                .ok_or(Error::MissingNode("indexed access object"))?,
            access
                .index_type()
                .ok_or(Error::MissingNode("indexed access index"))?,
        );
        self.emit_postfix_type_operand(object, node)?;
        self.write_punctuation(b"[");
        self.emit_type_node_outside_extends(index)?;
        self.write_punctuation(b"]");
        self.exit_node(node, state)?;
        Ok(())
    }

    // port: tsc/internal/printer/printer.go:Printer.emitMappedTypeParameter
    fn emit_mapped_type_parameter(&mut self, node: NodeId) -> Result<(), Error> {
        let state = self.enter_node(node)?;
        let read = self.node(node)?;
        let declaration = read
            .data_source()
            .as_type_parameter_declaration()
            .ok_or(Error::MissingNode("mapped type parameter payload"))?;
        let (name, constraint) = (declaration.name(), declaration.constraint());
        self.emit_binding_identifier(
            name.ok_or(Error::MissingNode("mapped type parameter name"))?,
        )?;
        self.write_space();
        self.write_keyword(b"in");
        self.write_space();
        self.emit_type_node_outside_extends(
            constraint.ok_or(Error::MissingNode("mapped type constraint"))?,
        )?;
        self.exit_node(node, state)?;
        Ok(())
    }

    // port: tsc/internal/printer/printer.go:Printer.emitMappedType
    fn emit_mapped_type(&mut self, node: NodeId) -> Result<(), Error> {
        let state = self.enter_node(node)?;
        let read = self.node(node)?;
        let mapped = read
            .data_source()
            .as_mapped_type_node()
            .ok_or(Error::MissingNode("mapped type payload"))?;
        let (readonly, type_parameter, name_type, question, type_node, members) = (
            mapped.readonly_token(),
            mapped
                .type_parameter()
                .ok_or(Error::MissingNode("mapped type parameter"))?,
            mapped.name_type(),
            mapped.question_token(),
            mapped.r#type(),
            mapped.members(),
        );
        let single_line = self.should_emit_on_single_line(node);
        self.write_punctuation(b"{");
        if single_line {
            self.write_space();
        } else {
            self.write_line();
            self.increase_indent();
        }
        if let Some(readonly) = readonly {
            self.emit_token_node(Some(readonly))?;
            if self.known_kind(readonly)? != K::ReadonlyKeyword {
                self.write_keyword(b"readonly");
            }
            self.write_space();
        }
        self.write_punctuation(b"[");
        self.emit_mapped_type_parameter(type_parameter)?;
        if let Some(name_type) = name_type {
            self.write_space();
            self.write_keyword(b"as");
            self.write_space();
            self.emit_type_node_outside_extends(name_type)?;
        }
        self.write_punctuation(b"]");
        if let Some(question) = question {
            self.emit_punctuation_node(Some(question))?;
            if self.known_kind(question)? != K::QuestionToken {
                self.write_punctuation(b"?");
            }
        }
        if let Some(type_node) = type_node {
            self.write_punctuation(b":");
            self.write_space();
            self.emit_type_node_outside_extends(type_node)?;
        }
        self.write_trailing_semicolon();
        if self.list_len(members)? > 0 {
            if single_line {
                self.write_space();
            } else {
                self.write_line();
            }
            self.emit_list(Self::emit_type_element, node, members, lf::PRESERVE_LINES)?;
        }
        if single_line {
            self.write_space();
        } else {
            self.write_line();
            self.decrease_indent();
        }
        self.write_punctuation(b"}");
        self.exit_node(node, state)?;
        Ok(())
    }

    // port: tsc/internal/printer/printer.go:Printer.emitLiteralType
    fn emit_literal_type(&mut self, node: NodeId) -> Result<(), Error> {
        let state = self.enter_node(node)?;
        let literal = self
            .node(node)?
            .data_source()
            .as_literal_type_node()
            .and_then(|literal| literal.literal())
            .ok_or(Error::MissingNode("literal type literal"))?;
        self.emit_expression(literal, op::COMMA)?;
        self.exit_node(node, state)?;
        Ok(())
    }

    // port: tsc/internal/printer/printer.go:Printer.emitTemplateTypeSpan
    fn emit_template_type_span(&mut self, node: NodeId) -> Result<(), Error> {
        let state = self.enter_node(node)?;
        let read = self.node(node)?;
        let span = read
            .data_source()
            .as_template_literal_type_span()
            .ok_or(Error::MissingNode("template span payload"))?;
        let (type_node, literal) = (
            span.r#type()
                .ok_or(Error::MissingNode("template span type"))?,
            span.literal()
                .ok_or(Error::MissingNode("template span literal"))?,
        );
        self.emit_type_node_outside_extends(type_node)?;
        self.emit_template_middle_tail(literal)?;
        self.exit_node(node, state)?;
        Ok(())
    }

    // port: tsc/internal/printer/printer.go:Printer.emitTemplateTypeSpanNode
    fn emit_template_type_span_node(&mut self, node: NodeId) -> Result<(), Error> {
        self.emit_template_type_span(node)
    }

    // port: tsc/internal/printer/printer.go:Printer.emitTemplateType
    fn emit_template_type(&mut self, node: NodeId) -> Result<(), Error> {
        let state = self.enter_node(node)?;
        let read = self.node(node)?;
        let template = read
            .data_source()
            .as_template_literal_type_node()
            .ok_or(Error::MissingNode("template literal type payload"))?;
        let (head, spans) = (
            template.head().ok_or(Error::MissingNode("template head"))?,
            template.template_spans(),
        );
        self.emit_template_head(head)?;
        self.emit_list(
            Self::emit_template_type_span_node,
            node,
            spans,
            lf::TEMPLATE_EXPRESSION_SPANS,
        )?;
        self.exit_node(node, state)?;
        Ok(())
    }

    // port: tsc/internal/printer/printer.go:Printer.emitImportTypeNode
    fn emit_import_type_node(&mut self, node: NodeId) -> Result<(), Error> {
        let state = self.enter_node(node)?;
        let read = self.node(node)?;
        let import = read
            .data_source()
            .as_import_type_node()
            .ok_or(Error::MissingNode("import type payload"))?;
        let (is_type_of, argument, attributes, qualifier, type_arguments) = (
            import.is_type_of(),
            import
                .argument()
                .ok_or(Error::MissingNode("import type argument"))?,
            import.attributes(),
            import.qualifier(),
            import.type_arguments(),
        );
        if is_type_of {
            self.write_keyword(b"typeof");
            self.write_space();
        }
        self.write_keyword(b"import");
        self.write_punctuation(b"(");
        self.emit_type_node_outside_extends(argument)?;
        if let Some(attributes) = attributes {
            self.write_punctuation(b",");
            self.write_space();
            self.emit_import_type_attributes(attributes)?;
        }
        self.write_punctuation(b")");
        if let Some(qualifier) = qualifier {
            self.write_punctuation(b".");
            self.emit_entity_name(qualifier)?;
        }
        self.emit_type_arguments(node, type_arguments)?;
        self.exit_node(node, state)?;
        Ok(())
    }

    // port: tsc/internal/printer/printer.go:Printer.emitImportTypeNodeAttributes
    fn emit_import_type_attributes(&mut self, node: NodeId) -> Result<(), Error> {
        let state = self.enter_node(node)?;
        let read = self.node(node)?;
        let data = read
            .data_source()
            .as_import_attributes()
            .ok_or(Error::MissingNode("import attributes payload"))?;
        let (token, attributes) = (data.token(), data.attributes());
        self.write_punctuation(b"{");
        self.write_space();
        self.write_keyword(if token == K::AssertKeyword {
            b"assert"
        } else {
            b"with"
        });
        self.write_punctuation(b":");
        self.write_space();
        self.emit_list(
            Self::emit_import_attribute_node,
            node,
            attributes,
            lf::IMPORT_ATTRIBUTES,
        )?;
        self.write_space();
        self.write_punctuation(b"}");
        self.exit_node(node, state)?;
        Ok(())
    }

    // port: tsc/internal/printer/printer.go:Printer.emitImportAttribute
    fn emit_import_attribute(&mut self, node: NodeId) -> Result<(), Error> {
        let state = self.enter_node(node)?;
        let read = self.node(node)?;
        let data = read
            .data_source()
            .as_import_attribute()
            .ok_or(Error::MissingNode("import attribute payload"))?;
        let (name, value) = (data.name(), data.value());
        self.emit_import_attribute_name(name.ok_or(Error::MissingNode("import attribute name"))?)?;
        self.write_punctuation(b":");
        self.write_space();
        let value = value.ok_or(Error::MissingNode("import attribute value"))?;
        if self.emit_flags(value) & ef::NO_LEADING_COMMENTS == 0 {
            let comment_range = self.comment_target(value)?.comment_range;
            self.emit_trailing_comments(comment_range.pos(), CommentSeparator::After);
        }
        self.emit_expression(value, op::DISALLOW_COMMA)?;
        self.exit_node(node, state)?;
        Ok(())
    }

    // port: tsc/internal/printer/printer.go:Printer.emitImportAttributeNode
    fn emit_import_attribute_node(&mut self, node: NodeId) -> Result<(), Error> {
        self.emit_import_attribute(node)
    }

    // port: tsc/internal/printer/printer.go:Printer.emitImportAttributeName
    fn emit_import_attribute_name(&mut self, node: NodeId) -> Result<(), Error> {
        match self.known_kind(node)? {
            K::Identifier => self.emit_identifier_name(node),
            K::StringLiteral => self.emit_string_literal(node),
            kind => Err(Error::UnexpectedKind {
                context: "unexpected ImportAttributeName",
                kind: kind.into(),
            }),
        }
    }

    // port: tsc/internal/printer/printer.go:Printer.emitTypeNodeInExtends
    fn emit_type_node_in_extends(&mut self, node: NodeId) -> Result<(), Error> {
        let saved = self.in_extends;
        self.in_extends = true;
        let result = self.emit_type_node_preserving_extends(node, TypePrecedence::LOWEST);
        self.in_extends = saved;
        result
    }

    // port: tsc/internal/printer/printer.go:Printer.emitTypeNodeOutsideExtends
    pub(crate) fn emit_type_node_outside_extends(&mut self, node: NodeId) -> Result<(), Error> {
        let saved = self.in_extends;
        self.in_extends = false;
        let result = self.emit_type_node_preserving_extends(node, TypePrecedence::LOWEST);
        self.in_extends = saved;
        result
    }

    // port: tsc/internal/printer/printer.go:Printer.emitTypeNodePreservingExtends
    fn emit_type_node_preserving_extends(
        &mut self,
        node: NodeId,
        precedence: TypePrecedence,
    ) -> Result<(), Error> {
        self.emit_type_node(node, precedence)
    }

    // port: tsc/internal/printer/printer.go:Printer.emitTypeNode
    fn emit_type_node(&mut self, node: NodeId, precedence: TypePrecedence) -> Result<(), Error> {
        stacker::maybe_grow(128 * 1024, 2 * 1024 * 1024, || {
            self.emit_type_node_worker(node, precedence)
        })
    }

    fn emit_type_node_worker(
        &mut self,
        node: NodeId,
        precedence: TypePrecedence,
    ) -> Result<(), Error> {
        let mut precedence = precedence;
        if self.in_extends && precedence <= TypePrecedence::Conditional {
            // In the `extends` clause of a conditional or infer type a conditional
            // type must be parenthesized.
            precedence = TypePrecedence::Function;
        }
        let saved_in_extends = self.in_extends;
        let kind = self.known_kind(node)?;
        let node_precedence =
            get_type_node_precedence(self.view, node)?.ok_or(Error::UnexpectedKind {
                context: "unhandled TypeNode",
                kind: kind.into(),
            })?;
        let parens = node_precedence < precedence;
        if parens {
            self.in_extends = false;
            self.write_punctuation(b"(");
        }
        let result = match kind {
            K::AnyKeyword
            | K::UnknownKeyword
            | K::NumberKeyword
            | K::BigIntKeyword
            | K::ObjectKeyword
            | K::BooleanKeyword
            | K::StringKeyword
            | K::SymbolKeyword
            | K::VoidKeyword
            | K::UndefinedKeyword
            | K::NeverKeyword
            | K::IntrinsicKeyword => self.emit_keyword_type_node(node),
            K::TypePredicate => self.emit_type_predicate(node),
            K::TypeReference => self.emit_type_reference(node),
            K::FunctionType => self.emit_function_type(node),
            K::ConstructorType => self.emit_constructor_type(node),
            K::TypeQuery => self.emit_type_query(node),
            K::TypeLiteral => self.emit_type_literal(node),
            K::ArrayType => self.emit_array_type(node),
            K::TupleType => self.emit_tuple_type(node),
            K::OptionalType => self.emit_optional_type(node),
            K::RestType => self.emit_rest_type(node),
            K::UnionType => self.emit_union_type(node),
            K::IntersectionType => self.emit_intersection_type(node),
            K::ConditionalType => self.emit_conditional_type(node),
            K::InferType => self.emit_infer_type(node),
            K::ParenthesizedType => self.emit_parenthesized_type(node),
            K::ThisType => self.emit_this_type(node),
            K::TypeOperator => self.emit_type_operator(node),
            K::IndexedAccessType => self.emit_indexed_access_type(node),
            K::MappedType => self.emit_mapped_type(node),
            K::LiteralType => self.emit_literal_type(node),
            K::NamedTupleMember => self.emit_named_tuple_member(node),
            K::TemplateLiteralType => self.emit_template_type(node),
            K::TemplateLiteralTypeSpan => self.emit_template_type_span(node),
            K::ImportType => self.emit_import_type_node(node),
            // Pseudo-types such as `f<T>.C`, where `f` is a generic function.
            K::PropertyAccessExpression => self.emit_property_access_expression(node),
            K::ExpressionWithTypeArguments => self.emit_expression_with_type_arguments(node),
            K::JSDocAllType => self.emit_jsdoc_all_type(node),
            K::JSDocNonNullableType => self.emit_jsdoc_non_nullable_type(node),
            K::JSDocNullableType => self.emit_jsdoc_nullable_type(node),
            K::JSDocOptionalType => self.emit_jsdoc_optional_type(node),
            K::JSDocVariadicType => self.emit_jsdoc_variadic_type(node),
            kind => Err(Error::UnexpectedKind {
                context: "unhandled TypeNode",
                kind: kind.into(),
            }),
        };
        result?;
        if parens {
            self.write_punctuation(b")");
        }
        self.in_extends = saved_in_extends;
        Ok(())
    }

    fn jsdoc_type_operand(&self, node: NodeId, what: &'static str) -> Result<NodeId, Error> {
        let read = self.node(node)?;
        let data = read.data_source();
        let inner = match read.kind().known() {
            Some(K::JSDocNonNullableType) => data
                .as_js_doc_non_nullable_type()
                .and_then(|node| node.r#type()),
            Some(K::JSDocNullableType) => data
                .as_js_doc_nullable_type()
                .and_then(|node| node.r#type()),
            Some(K::JSDocOptionalType) => data
                .as_js_doc_optional_type()
                .and_then(|node| node.r#type()),
            Some(K::JSDocVariadicType) => data
                .as_js_doc_variadic_type()
                .and_then(|node| node.r#type()),
            _ => None,
        };
        inner.ok_or(Error::MissingNode(what))
    }

    // port: tsc/internal/printer/printer.go:Printer.emitJSDocAllType
    fn emit_jsdoc_all_type(&mut self, node: NodeId) -> Result<(), Error> {
        self.emit_keyword_node(Some(node))
    }

    // port: tsc/internal/printer/printer.go:Printer.emitJSDocNonNullableType
    fn emit_jsdoc_non_nullable_type(&mut self, node: NodeId) -> Result<(), Error> {
        let state = self.enter_node(node)?;
        let inner = self.jsdoc_type_operand(node, "JSDoc non-nullable type")?;
        self.write_punctuation(b"!");
        self.emit_type_node(inner, TypePrecedence::NonArray)?;
        self.exit_node(node, state)?;
        Ok(())
    }

    // port: tsc/internal/printer/printer.go:Printer.emitJSDocNullableType
    fn emit_jsdoc_nullable_type(&mut self, node: NodeId) -> Result<(), Error> {
        let state = self.enter_node(node)?;
        let inner = self.jsdoc_type_operand(node, "JSDoc nullable type")?;
        self.write_punctuation(b"?");
        self.emit_type_node(inner, TypePrecedence::NonArray)?;
        self.exit_node(node, state)?;
        Ok(())
    }

    // port: tsc/internal/printer/printer.go:Printer.emitJSDocOptionalType
    fn emit_jsdoc_optional_type(&mut self, node: NodeId) -> Result<(), Error> {
        let state = self.enter_node(node)?;
        let inner = self.jsdoc_type_operand(node, "JSDoc optional type")?;
        self.emit_type_node(inner, TypePrecedence::JsDoc)?;
        self.write_punctuation(b"=");
        self.exit_node(node, state)?;
        Ok(())
    }

    // port: tsc/internal/printer/printer.go:Printer.emitJSDocVariadicType
    fn emit_jsdoc_variadic_type(&mut self, node: NodeId) -> Result<(), Error> {
        let state = self.enter_node(node)?;
        let inner = self.jsdoc_type_operand(node, "JSDoc variadic type")?;
        self.write_punctuation(b"...");
        self.emit_type_node(inner, TypePrecedence::JsDoc)?;
        self.exit_node(node, state)?;
        Ok(())
    }

    //
    // Expressions (the subset literal types and entity names can hold)
    //

    /// `ast.GetExpressionPrecedence` of the expression under any partially
    /// emitted wrappers.
    fn expression_precedence(&self, node: NodeId) -> Result<i32, Error> {
        let skipped = tsr_ast::skip_partially_emitted_expressions(self.view, node)?;
        Ok(tsr_ast::get_expression_precedence(
            self.view,
            &self.node(skipped)?,
        )?)
    }

    // port: tsc/internal/printer/printer.go:Printer.emitExpression
    fn emit_expression(&mut self, node: NodeId, precedence: i32) -> Result<(), Error> {
        stacker::maybe_grow(128 * 1024, 2 * 1024 * 1024, || {
            self.emit_expression_worker(node, precedence)
        })
    }

    fn emit_expression_worker(&mut self, node: NodeId, precedence: i32) -> Result<(), Error> {
        let kind = self.known_kind(node)?;
        let parens = self.expression_precedence(node)? < precedence;
        if parens {
            self.write_punctuation(b"(");
        }
        match kind {
            K::TrueKeyword | K::FalseKeyword | K::NullKeyword => {
                self.emit_token_node(Some(node))?;
            }
            K::ThisKeyword | K::SuperKeyword | K::ImportKeyword => {
                self.emit_keyword_expression(node)?;
            }
            K::NumericLiteral => self.emit_numeric_literal(node)?,
            K::BigIntLiteral => self.emit_big_int_literal(node)?,
            K::StringLiteral => self.emit_string_literal(node)?,
            K::RegularExpressionLiteral => self.emit_regular_expression_literal(node)?,
            K::NoSubstitutionTemplateLiteral => self.emit_no_substitution_template_literal(node)?,
            K::Identifier => self.emit_identifier_reference(node)?,
            K::PrivateIdentifier => self.emit_private_identifier(node)?,
            K::ArrayLiteralExpression => self.emit_array_literal_expression(node)?,
            K::ObjectLiteralExpression => self.emit_object_literal_expression(node)?,
            K::PropertyAccessExpression => self.emit_property_access_expression(node)?,
            K::ElementAccessExpression => self.emit_element_access_expression(node)?,
            K::CallExpression => self.emit_call_expression(node)?,
            K::NewExpression => self.emit_new_expression(node)?,
            K::TaggedTemplateExpression => self.emit_tagged_template_expression(node)?,
            K::TypeAssertionExpression => self.emit_type_assertion_expression(node)?,
            K::ParenthesizedExpression => self.emit_parenthesized_expression(node)?,
            K::FunctionExpression => self.emit_function_expression(node)?,
            K::ArrowFunction => self.emit_arrow_function(node)?,
            K::DeleteExpression => self.emit_keyword_unary_expression(node, K::DeleteKeyword)?,
            K::TypeOfExpression => self.emit_keyword_unary_expression(node, K::TypeOfKeyword)?,
            K::VoidExpression => self.emit_keyword_unary_expression(node, K::VoidKeyword)?,
            K::AwaitExpression => self.emit_keyword_unary_expression(node, K::AwaitKeyword)?,
            K::PrefixUnaryExpression => self.emit_prefix_unary_expression(node)?,
            K::PostfixUnaryExpression => self.emit_postfix_unary_expression(node)?,
            K::BinaryExpression => self.emit_binary_expression(node)?,
            K::ConditionalExpression => self.emit_conditional_expression(node)?,
            K::TemplateExpression => self.emit_template_expression(node)?,
            K::YieldExpression => self.emit_yield_expression(node)?,
            K::SpreadElement => self.emit_spread_element(node)?,
            K::ClassExpression => self.emit_class(node)?,
            K::OmittedExpression => self.emit_nothing(node)?,
            K::AsExpression => self.emit_as_or_satisfies_expression(node, b"as")?,
            K::NonNullExpression => self.emit_non_null_expression(node)?,
            K::ExpressionWithTypeArguments => self.emit_expression_with_type_arguments(node)?,
            K::SatisfiesExpression => self.emit_as_or_satisfies_expression(node, b"satisfies")?,
            K::MetaProperty => self.emit_meta_property(node)?,
            K::MissingDeclaration => {}
            K::JsxElement | K::JsxFragment => self.emit_jsx_element_or_fragment(node)?,
            K::JsxSelfClosingElement => self.emit_jsx_self_closing_element(node)?,
            // Upstream returns here, before the closing parenthesis.
            K::NotEmittedStatement => return Ok(()),
            K::PartiallyEmittedExpression => self.emit_partially_emitted_expression(node)?,
            kind => {
                return Err(Error::UnexpectedKind {
                    context: "unexpected Expression",
                    kind: kind.into(),
                })
            }
        }
        if parens {
            self.write_punctuation(b")");
        }
        Ok(())
    }

    // port: tsc/internal/printer/printer.go:Printer.emitKeywordExpression
    fn emit_keyword_expression(&mut self, node: NodeId) -> Result<(), Error> {
        self.emit_keyword_node(Some(node))
    }

    // port: tsc/internal/printer/printer.go:Printer.emitPrefixUnaryExpression
    fn emit_prefix_unary_expression(&mut self, node: NodeId) -> Result<(), Error> {
        let state = self.enter_node(node)?;
        let read = self.node(node)?;
        let unary = read
            .data_source()
            .as_prefix_unary_expression()
            .ok_or(Error::MissingNode("prefix unary payload"))?;
        let operator = unary.operator().known().ok_or(Error::UnexpectedKind {
            context: "PrefixUnaryExpression",
            kind: unary.operator(),
        })?;
        let operand = unary
            .operand()
            .ok_or(Error::MissingNode("prefix unary operand"))?;
        self.emit_token(operator, i64::from(read.pos()), WriteKind::Operator, node)?;
        // `+ +x` and `- -x` need a space so they do not read as increments.
        let operand_read = self.node(operand)?;
        if operand_read.kind() == K::PrefixUnaryExpression {
            let inner = operand_read
                .data_source()
                .as_prefix_unary_expression()
                .map(|inner| inner.operator())
                .and_then(NodeKind::known);
            if (operator == K::PlusToken && matches!(inner, Some(K::PlusToken | K::PlusPlusToken)))
                || (operator == K::MinusToken
                    && matches!(inner, Some(K::MinusToken | K::MinusMinusToken)))
            {
                self.write_space();
            }
        }
        self.emit_expression(operand, op::UNARY)?;
        self.exit_node(node, state)?;
        Ok(())
    }

    /// A numeric literal written without a dot or exponent needs `..` before a
    /// member name, as in `1..toString`.
    // port: tsc/internal/printer/printer.go:Printer.mayNeedDotDotForPropertyAccess
    fn may_need_dot_dot_for_property_access(&self, expression: NodeId) -> Result<bool, Error> {
        let read = self.node(expression)?;
        if read.kind() != K::NumericLiteral {
            return Ok(false);
        }
        let text =
            self.get_literal_text_of_node(expression, LiteralEscapeFlags::NEVER_ASCII_ESCAPE)?;
        let flags = read
            .data_source()
            .as_numeric_literal()
            .map_or(0, |literal| literal.token_flags());
        let with_specifier = token_flags::BINARY_SPECIFIER
            | token_flags::OCTAL_SPECIFIER
            | token_flags::HEX_SPECIFIER;
        Ok(flags & with_specifier == 0
            && !text.contains(&b'.')
            && !text.contains(&b'E')
            && !text.contains(&b'e'))
    }

    // port: tsc/internal/printer/printer.go:Printer.emitPropertyAccessExpression
    fn emit_property_access_expression(&mut self, node: NodeId) -> Result<(), Error> {
        let state = self.enter_node(node)?;
        let read = self.node(node)?;
        let access = read
            .data_source()
            .as_property_access_expression()
            .ok_or(Error::MissingNode("property access payload"))?;
        let (expression, question_dot, name) = (
            access
                .expression()
                .ok_or(Error::MissingNode("property access expression"))?,
            access.question_dot_token(),
            access
                .name()
                .ok_or(Error::MissingNode("property access name"))?,
        );
        let precedence = if tsr_ast::utilities::is_optional_chain(&read) {
            op::OPTIONAL_CHAIN
        } else {
            op::MEMBER
        };
        self.emit_expression(expression, precedence)?;
        let expression_read = self.node(expression)?;
        let name_read = self.node(name)?;
        // Upstream synthesizes a dot token spanning the gap when none was parsed.
        let token = match question_dot {
            Some(token) => Span::of(&self.node(token)?),
            None => Span {
                node: None,
                pos: i64::from(expression_read.end()),
                end: i64::from(name_read.pos()),
            },
        };
        let token_kind = match question_dot {
            Some(token) => self.known_kind(token)?,
            None => K::DotToken,
        };
        let lines_before_dot =
            self.get_lines_between_nodes(node, Span::of(&expression_read), token)?;
        self.write_line_repeat(lines_before_dot);
        self.increase_indent_if(lines_before_dot > 0);
        let should_emit_dot_dot = token_kind != K::QuestionDotToken
            && self.may_need_dot_dot_for_property_access(expression)?
            && !self.writer.has_trailing_comment()
            && !self.writer.has_trailing_whitespace();
        if should_emit_dot_dot {
            self.write_punctuation(b".");
        }
        if question_dot.is_some() {
            self.emit_token_node(question_dot)?;
        } else {
            self.emit_token(
                K::DotToken,
                i64::from(expression_read.end()),
                WriteKind::Punctuation,
                node,
            )?;
        }
        let lines_after_dot = self.get_lines_between_nodes(node, token, Span::of(&name_read))?;
        self.write_line_repeat(lines_after_dot);
        self.increase_indent_if(lines_after_dot > 0);
        self.emit_member_name(Some(name))?;
        self.decrease_indent_if(lines_after_dot > 0);
        self.decrease_indent_if(lines_before_dot > 0);
        self.exit_node(node, state)?;
        Ok(())
    }

    // port: tsc/internal/printer/printer.go:Printer.emitElementAccessExpression
    fn emit_element_access_expression(&mut self, node: NodeId) -> Result<(), Error> {
        let state = self.enter_node(node)?;
        let read = self.node(node)?;
        let access = read
            .data_source()
            .as_element_access_expression()
            .ok_or(Error::MissingNode("element access payload"))?;
        let expression = access
            .expression()
            .ok_or(Error::MissingNode("element access expression"))?;
        let argument = access
            .argument_expression()
            .ok_or(Error::MissingNode("element access argument"))?;
        let question_dot = access.question_dot_token();
        let precedence = if tsr_ast::utilities::is_optional_chain(&read) {
            op::OPTIONAL_CHAIN
        } else {
            op::MEMBER
        };
        self.emit_expression(expression, precedence)?;
        self.emit_token_node(question_dot)?;
        let question_end = question_dot
            .map(|id| self.node(id).map(|read| i64::from(read.end())))
            .transpose()?;
        self.emit_token(
            K::OpenBracketToken,
            greatest_end(
                -1,
                &[Some(i64::from(self.node(expression)?.end())), question_end],
            ),
            WriteKind::Punctuation,
            node,
        )?;
        self.emit_expression(argument, op::COMMA)?;
        self.emit_token(
            K::CloseBracketToken,
            i64::from(self.node(argument)?.end()),
            WriteKind::Punctuation,
            node,
        )?;
        self.exit_node(node, state)?;
        Ok(())
    }

    // port: tsc/internal/printer/printer.go:Printer.emitExpressionWithTypeArguments
    fn emit_expression_with_type_arguments(&mut self, node: NodeId) -> Result<(), Error> {
        let state = self.enter_node(node)?;
        let read = self.node(node)?;
        let with_arguments = read
            .data_source()
            .as_expression_with_type_arguments()
            .ok_or(Error::MissingNode("expression with type arguments payload"))?;
        let (expression, type_arguments) = (
            with_arguments
                .expression()
                .ok_or(Error::MissingNode("expression"))?,
            with_arguments.type_arguments(),
        );
        self.emit_expression(expression, op::MEMBER)?;
        self.emit_type_arguments(node, type_arguments)?;
        self.exit_node(node, state)?;
        Ok(())
    }

    //
    // Lists
    //

    // port: tsc/internal/printer/printer.go:Printer.emitList
    fn emit_list(
        &mut self,
        emit: fn(&mut Self, NodeId) -> Result<(), Error>,
        parent: NodeId,
        children: Option<NodeListId>,
        mut format: ListFormat,
    ) -> Result<(), Error> {
        if self.should_emit_on_multiple_lines(parent) {
            format |= lf::PREFER_NEW_LINE | lf::INDENTED;
        }
        self.emit_list_range(emit, parent, children, format, -1, -1)
    }

    // port: tsc/internal/printer/printer.go:Printer.emitListRange
    fn emit_list_range(
        &mut self,
        emit: fn(&mut Self, NodeId) -> Result<(), Error>,
        parent: NodeId,
        children: Option<NodeListId>,
        format: ListFormat,
        start: i64,
        count: i64,
    ) -> Result<(), Error> {
        let is_nil = children.is_none();
        let nodes = self.list_nodes_of(children)?;
        let length = nodes.len() as i64;
        let start = start.max(0);
        let count = if count < 0 { length - start } else { count };
        if is_nil && format & lf::OPTIONAL_IF_NIL != 0 {
            return Ok(());
        }
        let is_empty = is_nil || start >= length || count <= 0;
        if is_empty && format & lf::OPTIONAL_IF_EMPTY != 0 {
            if let Some(list) = children {
                self.writer.on_before_emit_node_list(list);
                self.writer.on_after_emit_node_list(list);
            }
            return Ok(());
        }
        let children_range = self.list_range(children)?;
        if format & lf::BRACKETS_MASK != 0 {
            self.write_punctuation(get_opening_bracket(format)?);
            if let (true, Some(range)) = (is_empty, children_range) {
                // Emit comments within empty lists
                self.emit_trailing_comments(range.pos(), CommentSeparator::Before);
            }
        }
        if let Some(list) = children {
            self.writer.on_before_emit_node_list(list);
        }
        if is_empty {
            // Write a line terminator if the parent node was multi-line
            let parent_on_single_line = self.printer.options.preserve_source_newlines
                && self.current_source.is_some()
                && self.range_is_on_single_line(Span::of(&self.node(parent)?));
            if format & lf::MULTI_LINE != 0 && !parent_on_single_line {
                self.write_line();
            } else if format & lf::SPACE_BETWEEN_BRACES != 0 && format & lf::NO_SPACE_IF_EMPTY == 0
            {
                self.write_space();
            }
        } else {
            let end = (start + count).min(length) as usize;
            let has_trailing_comma = match children {
                Some(list) => self.has_trailing_comma(parent, list)?,
                None => false,
            };
            self.emit_list_items(
                emit,
                parent,
                &nodes[start as usize..end],
                format,
                has_trailing_comma,
                children_range.expect("a nonempty list exists"),
            )?;
        }
        if let Some(list) = children {
            self.writer.on_after_emit_node_list(list);
        }
        if format & lf::BRACKETS_MASK != 0 {
            if let (true, Some(range)) = (is_empty, children_range) {
                // Emit comments within empty lists
                self.emit_leading_comments(range.end(), false);
            }
            self.write_punctuation(get_closing_bracket(format)?);
        }
        Ok(())
    }

    // port: tsc/internal/printer/printer.go:Printer.writeDelimiter
    fn write_delimiter(&mut self, format: ListFormat) {
        match format & lf::DELIMITERS_MASK {
            lf::COMMA_DELIMITED => self.write_punctuation(b","),
            lf::BAR_DELIMITED => {
                self.write_space();
                self.write_punctuation(b"|");
            }
            lf::ASTERISK_DELIMITED => {
                self.write_space();
                self.write_punctuation(b"*");
                self.write_space();
            }
            lf::AMPERSAND_DELIMITED => {
                self.write_space();
                self.write_punctuation(b"&");
            }
            _ => {}
        }
    }

    /// NodeList.HasTrailingComma is unreliable on transformed nodes as some nodes
    /// may have been removed; the original node's list decides when it has the
    /// same kind as the printed parent.
    // port: tsc/internal/printer/printer.go:Printer.hasTrailingComma
    fn has_trailing_comma(&self, parent: NodeId, children: NodeListId) -> Result<bool, Error> {
        if !self.view.list_has_trailing_comma(children)? {
            return Ok(false);
        }
        let original = self.printer.emit_context.most_original(parent);
        if original == parent {
            return Ok(true);
        }
        let parent_read = self.node(parent)?;
        let original_read = self.node(original)?;
        if original_read.kind() != parent_read.kind() {
            return Ok(false);
        }
        let original_list = match parent_read.kind().known() {
            Some(K::ObjectLiteralExpression) => original_read.property_list(),
            Some(K::ArrayLiteralExpression | K::NamedImports | K::NamedExports) => {
                original_read.element_list()
            }
            Some(K::CallExpression | K::NewExpression) => {
                if parent_read.type_argument_list() == Some(children) {
                    original_read.type_argument_list()
                } else if parent_read.argument_list() == Some(children) {
                    original_read.argument_list()
                } else {
                    Some(children)
                }
            }
            Some(
                K::Constructor
                | K::MethodDeclaration
                | K::GetAccessor
                | K::SetAccessor
                | K::FunctionDeclaration
                | K::FunctionExpression
                | K::ArrowFunction
                | K::FunctionType
                | K::ConstructorType
                | K::CallSignature
                | K::ConstructSignature,
            ) => {
                if parent_read.type_parameter_list() == Some(children) {
                    original_read.type_parameter_list()
                } else if parent_read.parameter_list() == Some(children) {
                    original_read.parameter_list()
                } else {
                    Some(children)
                }
            }
            Some(
                K::ClassDeclaration
                | K::ClassExpression
                | K::InterfaceDeclaration
                | K::TypeAliasDeclaration
                | K::JSTypeAliasDeclaration,
            ) => {
                if parent_read.type_parameter_list() == Some(children) {
                    original_read.type_parameter_list()
                } else {
                    Some(children)
                }
            }
            Some(K::ObjectBindingPattern | K::ArrayBindingPattern) => {
                if parent_read.element_list() == Some(children) {
                    original_read.element_list()
                } else {
                    Some(children)
                }
            }
            Some(K::ImportAttributes) => original_read
                .data_source()
                .as_import_attributes()
                .and_then(|attributes| attributes.attributes()),
            _ => Some(children),
        };
        match original_list {
            Some(list) => Ok(self.view.list_has_trailing_comma(list)?),
            None => Ok(false),
        }
    }

    /// `emitTrailingCommentsOfPosition` returns at once without comments, so
    /// checking first spares its callers' emit-flag lookups; the intervening
    /// comment state ends the same either way.
    fn comments_possible(&self) -> bool {
        !self.comments_disabled && self.current_source.is_some()
    }

    /// Emits a list without brackets or raising events.
    // port: tsc/internal/printer/printer.go:Printer.emitListItems
    fn emit_list_items(
        &mut self,
        emit: fn(&mut Self, NodeId) -> Result<(), Error>,
        parent: NodeId,
        children: &[NodeId],
        format: ListFormat,
        has_trailing_comma: bool,
        children_text_range: TextRange,
    ) -> Result<(), Error> {
        // Write the opening line terminator or leading whitespace.
        let may_emit_intervening_comments = format & lf::NO_INTERVENING_COMMENTS == 0;
        let mut should_emit_intervening_comments = may_emit_intervening_comments;

        let parent_span = Span::of(&self.node(parent)?);
        let leading_line_terminator_count = match children.first() {
            Some(&first) => {
                self.get_leading_line_terminator_count(Some(parent_span), Some(first), format)?
            }
            None => 0,
        };
        if leading_line_terminator_count > 0 {
            self.write_line_repeat(leading_line_terminator_count);
            should_emit_intervening_comments = false;
        } else if format & lf::SPACE_BETWEEN_BRACES != 0 {
            self.write_space();
        }

        // Increase the indent, if requested.
        if format & lf::INDENTED != 0 {
            self.increase_indent();
        }

        let parent_end = greatest_end(-1, &[Some(parent_span.end)]);

        // Emit each child.
        let mut previous_sibling: Option<NodeId> = None;
        let mut should_decrease_indent_after_emit = false;
        for &child in children {
            // Write the delimiter if this is not the first node.
            if format & lf::ASTERISK_DELIMITED != 0 {
                // always write JSDoc in the format "\n *"
                self.write_line();
                self.write_delimiter(format);
            } else if let Some(previous) = previous_sibling {
                // A comment on its own line after an element, before the
                // delimiter, is not that element's trailing comment.
                if format & lf::DELIMITERS_MASK != 0 {
                    let previous_end = i64::from(self.node(previous)?.end());
                    if previous_end != parent_end
                        && !self.comments_disabled
                        && self.should_emit_trailing_comments(Some(previous))
                    {
                        self.emit_leading_comments(previous_end, false);
                    }
                }

                self.write_delimiter(format);

                // Write either a line terminator or whitespace to separate the elements.
                let separating_line_terminator_count =
                    self.get_separating_line_terminator_count(Some(previous), Some(child), format)?;
                if separating_line_terminator_count > 0 {
                    // If a synthesized node in a single-line list starts on a new
                    // line, we should increase the indent.
                    if format & (lf::LINES_MASK | lf::INDENTED) == lf::SINGLE_LINE {
                        self.increase_indent();
                        should_decrease_indent_after_emit = true;
                    }

                    if should_emit_intervening_comments
                        && format & lf::DELIMITERS_MASK != 0
                        && self.comments_possible()
                        && !position_is_synthesized(i64::from(self.node(child)?.pos()))
                        && self.should_emit_leading_comments(child)
                    {
                        let comment_range = self.comment_target(child)?.comment_range;
                        self.emit_trailing_comments_of_position(
                            comment_range.pos(),
                            format & lf::SPACE_BETWEEN_SIBLINGS != 0,
                            true,
                        );
                    }

                    self.write_line_repeat(separating_line_terminator_count);
                    should_emit_intervening_comments = false;
                } else if format & lf::SPACE_BETWEEN_SIBLINGS != 0 {
                    self.write_space();
                }
            }

            // Emit this child.
            if should_emit_intervening_comments
                && self.comments_possible()
                && self.should_emit_leading_comments(child)
            {
                let comment_range = self.comment_target(child)?.comment_range;
                self.emit_trailing_comments_of_position(comment_range.pos(), false, false);
            } else {
                should_emit_intervening_comments = may_emit_intervening_comments;
            }

            self.next_list_element_pos = i64::from(self.node(child)?.pos());
            emit(self, child)?;

            if should_decrease_indent_after_emit {
                self.decrease_indent();
                should_decrease_indent_after_emit = false;
            }

            previous_sibling = Some(child);
        }

        // Write a trailing comma, if requested.
        let skip_trailing_comments =
            self.comments_disabled || !self.should_emit_trailing_comments(previous_sibling);
        let emit_trailing_comma = has_trailing_comma
            && format & lf::ALLOW_TRAILING_COMMA != 0
            && format & lf::COMMA_DELIMITED != 0;
        if emit_trailing_comma {
            match previous_sibling {
                Some(previous) if !skip_trailing_comments => {
                    let previous_end = i64::from(self.node(previous)?.end());
                    self.emit_token(
                        K::CommaToken,
                        previous_end,
                        WriteKind::Punctuation,
                        previous,
                    )?;
                }
                _ => self.write_punctuation(b","),
            }
        }

        // Emit any trailing comment of the last element in the list, as in
        // `[...\n 2\n /* end of element 2 */\n]`.
        if let Some(previous) = previous_sibling
            .filter(|_| format & lf::DELIMITERS_MASK != 0 && !skip_trailing_comments)
        {
            let previous_end = i64::from(self.node(previous)?.end());
            if parent_end != previous_end {
                let comments_pos = if emit_trailing_comma && children_text_range.end() > 0 {
                    children_text_range.end()
                } else {
                    previous_end
                };
                self.emit_leading_comments(comments_pos, false);
            }
        }

        // Decrease the indent, if requested.
        if format & lf::INDENTED != 0 {
            self.decrease_indent();
        }

        // Write the closing line terminator or closing whitespace.
        let closing_line_terminator_count = self.get_closing_line_terminator_count(
            Some(parent_span),
            children.last().copied(),
            format,
            children_text_range,
        )?;
        if closing_line_terminator_count > 0 {
            self.write_line_repeat(closing_line_terminator_count);
        } else if format & (lf::SPACE_AFTER_LIST | lf::SPACE_BETWEEN_BRACES) != 0 {
            self.write_space();
        }
        Ok(())
    }

    //
    // Name generation
    //

    // port: tsc/internal/printer/printer.go:Printer.shouldReuseTempVariableScope
    fn should_reuse_temp_variable_scope(&self, node: Option<NodeId>) -> bool {
        node.is_some_and(|node| self.emit_flags(node) & ef::REUSE_TEMP_VARIABLE_SCOPE != 0)
    }

    /// Boundary: the scope stack is the `NameGenerator`'s, ported separately.
    /// Until the printer owns one there is no scope to push; every generated
    /// name is refused where it is generated or printed.
    // port: tsc/internal/printer/printer.go:Printer.pushNameGenerationScope
    fn push_name_generation_scope(&mut self, node: Option<NodeId>) {
        let _reuse_temp_variable_scope = self.should_reuse_temp_variable_scope(node);
    }

    /// Boundary: see `push_name_generation_scope`.
    // port: tsc/internal/printer/printer.go:Printer.popNameGenerationScope
    fn pop_name_generation_scope(&mut self, node: Option<NodeId>) {
        let _reuse_temp_variable_scope = self.should_reuse_temp_variable_scope(node);
    }

    // port: tsc/internal/printer/printer.go:Printer.generateAllNames
    fn generate_all_names(&mut self, nodes: Option<NodeListId>) -> Result<(), Error> {
        let Some(nodes) = nodes else {
            return Ok(());
        };
        for node in self.list_nodes(nodes)? {
            self.generate_names(Some(node))?;
        }
        Ok(())
    }

    // port: tsc/internal/printer/printer.go:Printer.generateNames
    fn generate_names(&mut self, node: Option<NodeId>) -> Result<(), Error> {
        guard(|| self.generate_names_worker(node))
    }

    #[allow(clippy::match_same_arms, reason = "upstream's case order")]
    fn generate_names_worker(&mut self, node: Option<NodeId>) -> Result<(), Error> {
        let Some(node) = node else {
            return Ok(());
        };
        let read = self.node(node)?;
        let data = read.data_source();
        match read.kind().known() {
            Some(K::Block | K::CaseClause | K::DefaultClause) => {
                self.generate_all_names(read.statement_list())?;
            }
            Some(K::LabeledStatement | K::WithStatement | K::DoStatement | K::WhileStatement) => {
                self.generate_names(read.statement())?;
            }
            Some(K::IfStatement) => {
                let statement = data
                    .as_if_statement()
                    .ok_or(Error::MissingNode("if payload"))?;
                let (then_statement, else_statement) =
                    (statement.then_statement(), statement.else_statement());
                self.generate_names(then_statement)?;
                self.generate_names(else_statement)?;
            }
            Some(K::ForStatement | K::ForOfStatement | K::ForInStatement) => {
                self.generate_names(read.initializer())?;
                self.generate_names(read.statement())?;
            }
            Some(K::SwitchStatement) => {
                let case_block = data
                    .as_switch_statement()
                    .ok_or(Error::MissingNode("switch payload"))?
                    .case_block();
                self.generate_names(case_block)?;
            }
            Some(K::CaseBlock) => {
                let clauses = data
                    .as_case_block()
                    .ok_or(Error::MissingNode("case block payload"))?
                    .clauses();
                self.generate_all_names(clauses)?;
            }
            Some(K::TryStatement) => {
                let statement = data
                    .as_try_statement()
                    .ok_or(Error::MissingNode("try payload"))?;
                let (try_block, catch_clause, finally_block) = (
                    statement.try_block(),
                    statement.catch_clause(),
                    statement.finally_block(),
                );
                self.generate_names(try_block)?;
                self.generate_names(catch_clause)?;
                self.generate_names(finally_block)?;
            }
            Some(K::CatchClause) => {
                let clause = data
                    .as_catch_clause()
                    .ok_or(Error::MissingNode("catch payload"))?;
                let (declaration, block) = (clause.variable_declaration(), clause.block());
                self.generate_names(declaration)?;
                self.generate_names(block)?;
            }
            Some(K::VariableStatement) => {
                let list = data
                    .as_variable_statement()
                    .ok_or(Error::MissingNode("variable statement payload"))?
                    .declaration_list();
                self.generate_names(list)?;
            }
            Some(K::VariableDeclarationList) => {
                let declarations = data
                    .as_variable_declaration_list()
                    .ok_or(Error::MissingNode("declaration list payload"))?
                    .declarations();
                self.generate_all_names(declarations)?;
            }
            Some(
                K::VariableDeclaration | K::Parameter | K::BindingElement | K::ClassDeclaration,
            ) => {
                self.generate_name_if_needed(read.name())?;
            }
            Some(K::FunctionDeclaration) => {
                self.generate_name_if_needed(read.name())?;
                if self.should_reuse_temp_variable_scope(Some(node)) {
                    self.generate_all_names(read.parameter_list())?;
                    self.generate_names(read.body())?;
                }
            }
            Some(K::ObjectBindingPattern | K::ArrayBindingPattern) => {
                self.generate_all_names(read.element_list())?;
            }
            Some(K::ImportDeclaration | K::JSImportDeclaration) => {
                self.generate_names(read.import_clause())?;
            }
            Some(K::ImportClause) => {
                let clause = data
                    .as_import_clause()
                    .ok_or(Error::MissingNode("import clause payload"))?;
                let (name, named_bindings) = (clause.name(), clause.named_bindings());
                self.generate_name_if_needed(name)?;
                self.generate_names(named_bindings)?;
            }
            Some(K::NamespaceImport | K::NamespaceExport) => {
                self.generate_name_if_needed(read.name())?;
            }
            Some(K::NamedImports) => {
                self.generate_all_names(read.element_list())?;
            }
            Some(K::ImportSpecifier) => {
                if let Some(property_name) = read.property_name() {
                    self.generate_name_if_needed(Some(property_name))?;
                } else {
                    self.generate_name_if_needed(read.name())?;
                }
            }
            _ => {}
        }
        Ok(())
    }

    // port: tsc/internal/printer/printer.go:Printer.generateAllMemberNames
    fn generate_all_member_names(&mut self, nodes: Option<NodeListId>) -> Result<(), Error> {
        let Some(nodes) = nodes else {
            return Ok(());
        };
        for node in self.list_nodes(nodes)? {
            self.generate_member_names(Some(node))?;
        }
        Ok(())
    }

    // port: tsc/internal/printer/printer.go:Printer.generateMemberNames
    fn generate_member_names(&mut self, node: Option<NodeId>) -> Result<(), Error> {
        let Some(node) = node else {
            return Ok(());
        };
        let read = self.node(node)?;
        if matches!(
            read.kind().known(),
            Some(
                K::PropertyAssignment
                    | K::ShorthandPropertyAssignment
                    | K::PropertyDeclaration
                    | K::PropertySignature
                    | K::MethodDeclaration
                    | K::MethodSignature
                    | K::GetAccessor
                    | K::SetAccessor
            )
        ) {
            self.generate_name_if_needed(read.name())?;
        }
        Ok(())
    }

    // port: tsc/internal/printer/printer.go:Printer.generateNameIfNeeded
    fn generate_name_if_needed(&mut self, name: Option<NodeId>) -> Result<(), Error> {
        let Some(name) = name else {
            return Ok(());
        };
        let read = self.node(name)?;
        if tsr_ast::utilities::is_member_name(&read) {
            self.generate_name(name)?;
        } else if tsr_ast::utilities::is_binding_pattern(&read) {
            self.generate_names(Some(name))?;
        }
        Ok(())
    }

    /// Generates the text for a generated identifier or private identifier.
    /// A name that is not generated produces its own text, which is discarded;
    /// a generated one needs the `NameGenerator` (boundary).
    // port: tsc/internal/printer/printer.go:Printer.generateName
    fn generate_name(&mut self, name: NodeId) -> Result<(), Error> {
        if self.printer.emit_context.has_auto_generate_info(name) {
            return Err(Error::Unsupported("NameGenerator.GenerateName"));
        }
        self.get_text_of_node(name, false)?;
        Ok(())
    }

    //
    // General
    //

    /// The `Write` dispatch.
    #[allow(clippy::match_same_arms, reason = "upstream's case order")]
    fn write_root(&mut self, node: NodeId) -> Result<(), Error> {
        let kind = self.known_kind(node)?;
        match kind {
            // Pseudo-literals
            K::TemplateHead => self.emit_template_head(node),
            K::TemplateMiddle => self.emit_template_middle(node),
            K::TemplateTail => self.emit_template_tail(node),
            // Identifiers
            K::Identifier => self.emit_identifier_name(node),
            K::PrivateIdentifier => self.emit_private_identifier(node),
            // Names
            K::QualifiedName => self.emit_qualified_name(node),
            K::ComputedPropertyName => self.emit_computed_property_name(node),
            // Signature elements
            K::TypeParameter => self.emit_type_parameter(node),
            K::Parameter => self.emit_parameter(node),
            K::Decorator => self.emit_decorator(node),
            // Type members
            K::PropertySignature => self.emit_property_signature(node),
            K::MethodSignature => self.emit_method_signature(node),
            K::CallSignature => self.emit_call_signature(node),
            K::ConstructSignature => self.emit_construct_signature(node),
            K::IndexSignature => self.emit_index_signature(node),
            K::PropertyDeclaration
            | K::MethodDeclaration
            | K::ClassStaticBlockDeclaration
            | K::Constructor => self.emit_class_element(node),
            K::GetAccessor => self.emit_get_accessor_declaration(node),
            K::SetAccessor => self.emit_set_accessor_declaration(node),
            // Binding patterns
            K::ObjectBindingPattern | K::ArrayBindingPattern => self.emit_binding_pattern(node),
            K::BindingElement => self.emit_binding_element(node),
            // Misc
            K::TemplateSpan => self.emit_template_span(node),
            K::SemicolonClassElement => self.emit_class_element(node),
            // Declarations (non-statement)
            K::VariableDeclaration => self.emit_variable_declaration(node),
            K::VariableDeclarationList => self.emit_variable_declaration_list(node),
            K::ModuleBlock => self.emit_block(node),
            K::CaseBlock => self.emit_case_block(node),
            K::ImportClause => self.emit_import_clause(node),
            K::NamespaceImport | K::NamespaceExport => self.emit_namespace_import(node),
            K::NamedImports | K::NamedExports => self.emit_named_imports_or_exports(node),
            K::ImportSpecifier | K::ExportSpecifier => self.emit_import_or_export_specifier(node),
            K::ImportAttributes => self.emit_import_attributes(node),
            K::ImportAttribute => self.emit_import_attribute(node),
            // Module references
            K::ExternalModuleReference => self.emit_external_module_reference(node),
            // JSX (non-expression)
            K::JsxText => self.emit_jsx_text(node),
            K::JsxOpeningElement => self.emit_jsx_opening_element(node),
            K::JsxOpeningFragment => self.emit_jsx_fragment_tag(node, b"<"),
            K::JsxClosingElement => self.emit_jsx_closing_element(node),
            K::JsxClosingFragment => self.emit_jsx_fragment_tag(node, b"</"),
            K::JsxAttribute => self.emit_jsx_attribute(node),
            K::JsxAttributes => self.emit_jsx_attributes(node),
            K::JsxSpreadAttribute => self.emit_jsx_spread_attribute(node),
            K::JsxExpression => self.emit_jsx_expression(node),
            K::JsxNamespacedName => self.emit_jsx_namespaced_name(node),
            // Clauses
            K::CaseClause | K::DefaultClause => self.emit_case_or_default_clause(node),
            K::HeritageClause => self.emit_heritage_clause(node),
            K::CatchClause => self.emit_catch_clause(node),
            // Property assignments
            K::PropertyAssignment => self.emit_property_assignment(node),
            K::ShorthandPropertyAssignment => self.emit_shorthand_property_assignment(node),
            K::SpreadAssignment => self.emit_spread_assignment(node),
            // Enum
            K::EnumMember => self.emit_enum_member(node),
            // Top-level nodes
            K::SourceFile => self.emit_source_file(node),
            // Transformation nodes
            K::NotEmittedTypeElement => self.emit_nothing(node),
            _ if is_type_node_kind(kind) => self.emit_type_node_outside_extends(node),
            _ if tsr_ast::utilities::is_statement(self.view, node)? => self.emit_statement(node),
            _ if tsr_ast::utilities::is_expression_kind(kind.into()) => {
                self.emit_expression(node, op::LOWEST)
            }
            _ if is_keyword_kind(kind) => self.emit_keyword_node(Some(node)),
            _ if is_punctuation_kind(kind) => self.emit_punctuation_node(Some(node)),
            _ if tsr_ast::utilities::is_js_doc_kind(kind.into()) => self.emit_jsdoc_node(node),
            _ => Err(Error::UnexpectedKind {
                context: "unhandled Node",
                kind: kind.into(),
            }),
        }
    }

    /// Upstream's own emitter is unimplemented at the pin and panics.
    // port: tsc/internal/printer/printer.go:Printer.emitJSDocNode
    #[allow(clippy::unused_self, reason = "upstream panics without reading state")]
    fn emit_jsdoc_node(&mut self, _node: NodeId) -> Result<(), Error> {
        Err(Error::Unsupported("emitJSDocNode"))
    }
}

// port: tsc/internal/printer/printer.go:getOpeningBracket
fn get_opening_bracket(format: ListFormat) -> Result<&'static [u8], Error> {
    Ok(match format & lf::BRACKETS_MASK {
        lf::BRACES => b"{",
        lf::PARENTHESIS => b"(",
        lf::ANGLE_BRACKETS => b"<",
        lf::SQUARE_BRACKETS => b"[",
        _ => return Err(Error::Unsupported("unexpected bracket format")),
    })
}

// port: tsc/internal/printer/printer.go:getClosingBracket
fn get_closing_bracket(format: ListFormat) -> Result<&'static [u8], Error> {
    Ok(match format & lf::BRACKETS_MASK {
        lf::BRACES => b"}",
        lf::PARENTHESIS => b")",
        lf::ANGLE_BRACKETS => b">",
        lf::SQUARE_BRACKETS => b"]",
        _ => return Err(Error::Unsupported("unexpected bracket format")),
    })
}
