//! Source-map positions (the "Source Maps" section of
//! `tsc/internal/printer/printer.go` and `lineCharacterCache` of
//! `utilities.go`).
//!
//! A write given a [`Generator`] maps the start and end of each node and of
//! each brace token, and the ends of each comment, to their source positions;
//! source columns are UTF-16 code units. Generator errors are upstream panics.

use super::{position_is_synthesized, Session};
use crate::{emit_flags as ef, EmitFlags, Error};
use std::rc::Rc;
use tsr_ast::{NodeId, NodeKind, SourceFileRead, SyntaxKind as K};
use tsr_core::TextRange;
use tsr_jsstring::JsString;
use tsr_sourcemap::{Source, Utf16Offset};

const NIL: &str = "runtime error: invalid memory address or nil pointer dereference";

/// `sourcemap.Source` as the printer holds it: a source file of the printed
/// view, or a source a [`MapSourcePosition`] handler supplies. Upstream
/// compares sources by identity: the file node, or the handler's allocation.
#[derive(Clone)]
pub enum SourceMapSource {
    File(NodeId),
    Mapped(Rc<dyn Source>),
}

impl PartialEq for SourceMapSource {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::File(left), Self::File(right)) => left == right,
            (Self::Mapped(left), Self::Mapped(right)) => Rc::ptr_eq(left, right),
            _ => false,
        }
    }
}

impl std::fmt::Debug for SourceMapSource {
    fn fmt(&self, output: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::File(file) => output.debug_tuple("File").field(file).finish(),
            Self::Mapped(source) => output
                .debug_tuple("Mapped")
                .field(&String::from_utf8_lossy(source.file_name()))
                .finish(),
        }
    }
}

/// `PrintHandlers.MapSourcePosition`: composes a source-map position before
/// it reaches the generator. It receives the source, its text and line map,
/// and the position; `None` emits a generated-only mapping.
pub type MapSourcePosition<'c> =
    Box<dyn Fn(&SourceMapSource, &dyn Source, i64) -> Option<(SourceMapSource, i64)> + 'c>;

/// The text, name and line starts of a source-map source.
pub(crate) enum SourceData<'a> {
    File(SourceFileRead<'a>),
    Mapped(Rc<dyn Source>),
}

impl Source for SourceData<'_> {
    fn text(&self) -> &[u8] {
        match self {
            Self::File(file) => file.text().as_bytes(),
            Self::Mapped(source) => source.text(),
        }
    }
    fn file_name(&self) -> &[u8] {
        match self {
            Self::File(file) => file.file_name(),
            Self::Mapped(source) => source.file_name(),
        }
    }
    fn ecma_line_map(&self) -> &[i32] {
        match self {
            Self::File(file) => file.ecma_line_map(),
            Self::Mapped(source) => source.ecma_line_map(),
        }
    }
}

/// `lineCharacterCache`: cached line/character lookups for a source,
/// optimized for monotonically increasing positions. Within one line only the
/// bytes since the last position are counted, so a long line costs O(n)
/// rather than O(n²). Characters are UTF-16 code units.
pub(crate) struct LineCharacterCache<'a> {
    source: SourceData<'a>,
    cached_line: isize,
    cached_pos: usize,
    cached_char: Utf16Offset,
    has_cached: bool,
}

impl<'a> LineCharacterCache<'a> {
    // port: tsc/internal/printer/utilities.go:newLineCharacterCache
    fn new(source: SourceData<'a>) -> Self {
        Self {
            source,
            cached_line: 0,
            cached_pos: 0,
            cached_char: 0,
            has_cached: false,
        }
    }

    /// The 0-based line and the UTF-16 offset in it of a byte position.
    // port: tsc/internal/printer/utilities.go:lineCharacterCache.getLineAndCharacter
    fn get_line_and_character(&mut self, pos: i64) -> (isize, Utf16Offset) {
        let text = self.source.text();
        let line_map = self.source.ecma_line_map();
        let line =
            tsr_jsstring::scanner_positions::compute_line_of_position(line_map, pos as isize);
        let line_start = line_map[usize::try_from(line).expect("a line of a position")] as usize;
        // When pos is beyond the source text (e.g., for error-recovery tokens like
        // missing closing braces), we can't slice past the text end. Compute the
        // UTF-16 length up to EOF and add the remaining byte offset arithmetically,
        // matching TypeScript's computeLineAndCharacterOfPosition which uses
        // arithmetic (position - lineStarts[lineNumber]) and handles this implicitly.
        let end_pos = usize::try_from(pos)
            .expect("a mapped position is not synthesized")
            .min(text.len());
        let mut character =
            if self.has_cached && line == self.cached_line && end_pos >= self.cached_pos {
                // Incremental: only count UTF-16 code units from the last cached position.
                self.cached_char
                    + tsr_jsstring::line_map::utf16_len(&text[self.cached_pos..end_pos])
            } else {
                // Full computation from line start.
                tsr_jsstring::line_map::utf16_len(&text[line_start..end_pos])
            };
        let cached_char = character;
        character += (pos - end_pos as i64) as Utf16Offset;
        self.cached_line = line;
        self.cached_pos = end_pos;
        self.cached_char = cached_char;
        self.has_cached = true;
        (line, character)
    }
}

/// `sourceMapState`: what a node's or token's leading position changed,
/// for its trailing position.
#[derive(Clone, Copy, Debug)]
pub(crate) struct SourceMapState {
    emit_flags: EmitFlags,
    source_map_range: TextRange,
    has_token_source_map_range: bool,
}

impl<'a> Session<'a, '_> {
    fn source_data(&self, source: &SourceMapSource) -> SourceData<'a> {
        match source {
            SourceMapSource::File(file) => SourceData::File(
                self.view
                    .source_file(*file)
                    .expect("a source-map source file belongs to the printed view"),
            ),
            SourceMapSource::Mapped(source) => SourceData::Mapped(Rc::clone(source)),
        }
    }

    // port: tsc/internal/printer/printer.go:Printer.setSourceMapSource
    pub(super) fn set_source_map_source(&mut self, source: SourceMapSource) {
        if self.source_maps_disabled {
            return;
        }

        let data = self.source_data(&source);
        self.source_map_source = Some(source.clone());
        let cache = self
            .source_map_line_char_cache
            .insert(LineCharacterCache::new(data));
        {
            let most_recent = self.printer.most_recent_source_map_source.borrow();
            if most_recent.0.as_ref() == Some(&source) {
                self.source_map_source_index = most_recent.1;
                return;
            }
        }

        let file_name = cache.source.file_name();
        self.source_map_source_is_json =
            tsr_tspath::file_extension_is(file_name, tsr_tspath::EXTENSION_JSON);
        if self.source_map_source_is_json {
            return;
        }

        let generator = self.source_map_generator.as_deref_mut().expect(NIL);
        self.source_map_source_index = generator.add_source(JsString::from_bytes(file_name));
        if self.printer.options.inline_sources {
            let text = JsString::from_bytes(cache.source.text());
            if let Err(error) = generator.set_source_content(self.source_map_source_index, text) {
                panic!("{error}");
            }
        }

        *self.printer.most_recent_source_map_source.borrow_mut() =
            (Some(source), self.source_map_source_index);
    }

    // port: tsc/internal/printer/printer.go:Printer.emitPos
    pub(super) fn emit_pos(&mut self, mut pos: i64) {
        if self.source_maps_disabled
            || self.source_map_generator.is_none()
            || self.source_map_source_is_json
            || position_is_synthesized(pos)
        {
            return;
        }
        let Some(source) = self.source_map_source.clone() else {
            return;
        };

        let mut source_index = self.source_map_source_index;
        let mut mapped_cache = None;
        if let Some(map_source_position) = &self.printer.map_source_position {
            let cache = self.source_map_line_char_cache.as_ref().expect(NIL);
            let Some((mapped_source, mapped_pos)) =
                map_source_position(&source, &cache.source, pos)
            else {
                let (line, column) = (self.writer.get_line(), self.writer.get_column());
                let generator = self.source_map_generator.as_deref_mut().expect(NIL);
                if let Err(error) = generator.add_generated_mapping(line, column) {
                    panic!("{error}");
                }
                return;
            };
            pos = mapped_pos;
            if mapped_source != source {
                let saved_source = self.source_map_source.take();
                let saved_source_index = self.source_map_source_index;
                let saved_source_is_json = self.source_map_source_is_json;
                let saved_line_char_cache = self.source_map_line_char_cache.take();
                self.set_source_map_source(mapped_source);
                source_index = self.source_map_source_index;
                mapped_cache = self.source_map_line_char_cache.take();
                self.source_map_source = saved_source;
                self.source_map_source_index = saved_source_index;
                self.source_map_source_is_json = saved_source_is_json;
                self.source_map_line_char_cache = saved_line_char_cache;
            }
        }

        let line_char_cache = match &mut mapped_cache {
            Some(cache) => cache,
            None => self.source_map_line_char_cache.as_mut().expect(NIL),
        };
        let (source_line, source_character) = line_char_cache.get_line_and_character(pos);
        let (line, column) = (self.writer.get_line(), self.writer.get_column());
        let generator = self.source_map_generator.as_deref_mut().expect(NIL);
        if let Err(error) =
            generator.add_source_mapping(line, column, source_index, source_line, source_character)
        {
            panic!("{error}");
        }
    }

    // port: tsc/internal/printer/printer.go:Printer.emitSourcePos
    fn emit_source_pos(&mut self, source: Option<SourceMapSource>, pos: i64) {
        if source == self.source_map_source {
            self.emit_pos(pos);
        } else {
            let saved_source_map_source = self.source_map_source.take();
            let saved_source_map_source_index = self.source_map_source_index;
            let saved_source_map_line_char_cache = self.source_map_line_char_cache.take();
            // A nil source dereferences in setSourceMapSource unless maps are disabled.
            match source {
                Some(source) => self.set_source_map_source(source),
                None => assert!(self.source_maps_disabled, "{NIL}"),
            }
            self.emit_pos(pos);
            self.source_map_source = saved_source_map_source;
            self.source_map_source_index = saved_source_map_source_index;
            self.source_map_line_char_cache = saved_source_map_line_char_cache;
        }
    }

    /// `emitSourceMapsBeforeNode` of a node with `emit_flags` and source-map
    /// range `loc`, once `shouldEmitSourceMaps` held.
    fn emit_source_maps_before(
        &mut self,
        kind: NodeKind,
        emit_flags: EmitFlags,
        loc: TextRange,
    ) -> SourceMapState {
        if kind != K::NotEmittedStatement
            && emit_flags & ef::NO_LEADING_SOURCE_MAP == 0
            && !position_is_synthesized(loc.pos())
        {
            if let Some(text) = self.source_text() {
                let pos = tsr_scanner::skip_trivia(text, loc.pos());
                self.emit_source_pos(self.source_map_source.clone(), pos);
            }
        }

        if emit_flags & ef::NO_NESTED_SOURCE_MAPS != 0 {
            self.source_maps_disabled = true;
        }

        SourceMapState {
            emit_flags,
            source_map_range: loc,
            has_token_source_map_range: false,
        }
    }

    // port: tsc/internal/printer/printer.go:Printer.emitSourceMapsBeforeNode
    pub(super) fn emit_source_maps_before_node(
        &mut self,
        node: NodeId,
    ) -> Result<Option<SourceMapState>, Error> {
        if !self.should_emit_source_maps(Some(node))? {
            return Ok(None);
        }
        let emit_flags = self.emit_flags(node);
        let loc = self
            .printer
            .emit_context
            .source_map_range(&super::ViewFactory(self.view), node);
        let kind = self.node(node)?.kind();
        Ok(Some(self.emit_source_maps_before(kind, emit_flags, loc)))
    }

    /// `emitSourceMapsBeforeNode` of a node the printer creates, with the emit
    /// flags and source-map range upstream gives it.
    pub(super) fn emit_source_maps_before_created_node(
        &mut self,
        kind: NodeKind,
        emit_flags: EmitFlags,
        loc: TextRange,
    ) -> Result<Option<SourceMapState>, Error> {
        if !self.should_emit_source_maps(None)? {
            return Ok(None);
        }
        Ok(Some(self.emit_source_maps_before(kind, emit_flags, loc)))
    }

    // port: tsc/internal/printer/printer.go:Printer.emitSourceMapsAfterNode
    pub(super) fn emit_source_maps_after_node(
        &mut self,
        kind: NodeKind,
        previous_state: Option<SourceMapState>,
    ) {
        let Some(previous_state) = previous_state else {
            return;
        };

        let emit_flags = previous_state.emit_flags;
        let loc = previous_state.source_map_range;

        if emit_flags & ef::NO_NESTED_SOURCE_MAPS != 0 {
            self.source_maps_disabled = false;
        }

        if kind != K::NotEmittedStatement
            && emit_flags & ef::NO_TRAILING_SOURCE_MAP == 0
            && !position_is_synthesized(loc.end())
        {
            self.emit_source_pos(self.source_map_source.clone(), loc.end());
        }
    }

    /// A token of a node the printer creates reads no emit flags or token
    /// ranges: none is a brace, so none reaches them.
    // port: tsc/internal/printer/printer.go:Printer.emitSourceMapsBeforeToken
    pub(super) fn emit_source_maps_before_token(
        &mut self,
        token: K,
        mut pos: i64,
        context: Option<NodeId>,
        flags: super::tef::TokenEmitFlags,
    ) -> Result<Option<SourceMapState>, Error> {
        if !self.should_emit_token_source_maps(token, pos, context, flags)? {
            return Ok(None);
        }

        let (emit_flags, token_range) = match context {
            Some(context) => (
                self.emit_flags(context),
                self.printer
                    .emit_context
                    .token_source_map_range(context, token.into()),
            ),
            None => (ef::NONE, None),
        };
        let loc = token_range.unwrap_or_default();
        if let Some(token_range) = token_range {
            pos = token_range.pos();
        }
        if pos >= 0 {
            if let Some(text) = self.source_text() {
                pos = tsr_scanner::skip_trivia(text, pos);
            }
        }
        if emit_flags & ef::NO_TOKEN_LEADING_SOURCE_MAPS == 0 && pos >= 0 {
            self.emit_source_pos(self.source_map_source.clone(), pos);
        }

        Ok(Some(SourceMapState {
            emit_flags,
            source_map_range: loc,
            has_token_source_map_range: token_range.is_some(),
        }))
    }

    // port: tsc/internal/printer/printer.go:Printer.emitSourceMapsAfterToken
    pub(super) fn emit_source_maps_after_token(
        &mut self,
        mut pos: i64,
        previous_state: Option<SourceMapState>,
    ) {
        let Some(previous_state) = previous_state else {
            return;
        };

        let emit_flags = previous_state.emit_flags;
        let loc = previous_state.source_map_range;
        let has_loc = previous_state.has_token_source_map_range;
        if emit_flags & ef::NO_TOKEN_TRAILING_SOURCE_MAPS == 0 {
            if has_loc {
                pos = loc.end();
            }
            if pos >= 0 {
                self.emit_source_pos(self.source_map_source.clone(), pos);
            }
        }
    }
}
