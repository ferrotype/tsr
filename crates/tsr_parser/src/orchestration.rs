use crate::{Parser, ParsingContext};
use std::{collections::HashMap, sync::Arc};
use ts_arena::Counters;
use ts_ast::{
    node_flags, AstBuilder, Diagnostic, Factory, NodeId, ParsedFile, RuntimeFactory,
    SourceFileParseOptions, SyntaxKind,
};
use ts_core::{ScriptKind, TextRange};
use ts_jsstring::SourceText;

/// Parse already loaded source bytes. BOM decoding belongs to SourceText's
/// loader constructor. The returned owner remains mutable for the binder.
/// port: tsc/internal/parser/parser.go:ParseSourceFile
pub fn parse_source_file(
    source: SourceText,
    script_kind: ScriptKind,
    opts: SourceFileParseOptions,
) -> ParsedFile {
    parse_source_file_with_counters(source, script_kind, opts, &Counters::default())
}

pub fn parse_source_file_with_counters(
    source: SourceText,
    script_kind: ScriptKind,
    opts: SourceFileParseOptions,
    counters: &Counters,
) -> ParsedFile {
    crate::on_parser_worker(move || {
        let factory = AstBuilder::new(source.clone(), counters);
        let mut parser = Parser::new(opts, &source, script_kind, factory);
        parser.next_token();
        let root = if script_kind == ScriptKind::JSON {
            parser.parse_json_text()
        } else {
            parser.parse_source_file_worker()
        };
        parser.seed_final_jsdoc_cache(root);
        parser
            .factory
            .complete(root)
            .expect("parser constructs an owner-valid source graph")
    })
}

/// Parse an entity-name fragment while retaining its complete source/AST owner.
/// A malformed fragment returns None and never exposes a dangling node identity.
/// port: tsc/internal/parser/parser.go:ParseIsolatedEntityName
pub fn parse_isolated_entity_name(source: SourceText) -> Option<ParsedFile> {
    crate::on_parser_worker(move || {
        let factory = AstBuilder::new(source.clone(), &Counters::default());
        let mut parser = Parser::new(
            SourceFileParseOptions::default(),
            &source,
            ScriptKind::JS,
            factory,
        );
        parser.next_token();
        let name = parser.parse_entity_name(true, false, None);
        if parser.token != SyntaxKind::EndOfFile || !parser.diagnostics.is_empty() {
            return None;
        }
        Some(
            parser
                .factory
                .complete(name)
                .expect("parser constructs an owner-valid fragment"),
        )
    })
}

impl Parser<'_, AstBuilder> {
    /// port: tsc/internal/parser/parser.go:Parser.parseSourceFileWorker
    pub(crate) fn parse_source_file_worker(&mut self) -> NodeId {
        let declaration = ts_core::path::is_declaration_file_name(self.opts.file_name.as_bytes());
        if declaration {
            self.context_flags |= node_flags::AMBIENT;
        }
        let pos = self.node_pos();
        let mut statements = self.parse_list_index(
            ParsingContext::SourceElements,
            Self::parse_toplevel_statement,
        );
        let end = self.node_pos();
        let jsdoc = self.jsdoc_scanner_info();
        let eof = self.parse_token_node();
        self.with_js_doc(eof, jsdoc);
        assert!(
            self.factory.node(eof).kind() == SyntaxKind::EndOfFile,
            "Expected end of file token from scanner."
        );
        statements.append(&mut self.reparse_list);
        let list = self.new_parsed_node_list(TextRange::new(pos, end), statements);
        let mut root = self.factory.new_source_file(
            self.opts.clone(),
            self.source_owner.clone(),
            Some(list),
            Some(eof),
        );
        self.finish_node(root, pos);
        self.finish_source_file(root, declaration);
        let should_reparse = {
            let file = self
                .factory
                .view()
                .source_file(root)
                .expect("source metadata");
            !file.is_declaration_file
                && file.external_module_indicator.is_some()
                && !self.possible_await_spans.is_empty()
        };
        if should_reparse {
            let reparsed = self.reparse_top_level_await(root);
            self.finish_node(reparsed, pos);
            if root != reparsed {
                root = reparsed;
                self.finish_source_file(root, declaration);
            }
        }
        self.collect_external_module_references(root);
        if self.factory.node(root).flags() & node_flags::JAVA_SCRIPT_FILE != 0 {
            let mut diagnostics = self.js_diagnostics.clone();
            attach_file_to_diagnostics(&mut diagnostics, root);
            self.factory
                .source_file_mut(root)
                .expect("source metadata")
                .js_diagnostics = diagnostics;
        }
        root
    }
    /// port: tsc/internal/parser/parser.go:Parser.finishSourceFile
    pub(crate) fn finish_source_file(&mut self, root: NodeId, declaration: bool) {
        let pragmas = super::pragmas::get_comment_pragmas(self.source_text);
        let pragmas = if pragmas.is_empty() {
            ts_ast::PragmaSlice::empty()
        } else {
            self.factory.source_pragmas(pragmas).expect("owned pragmas")
        };
        let comments = if self.scanner.comment_directives().is_empty() {
            ts_ast::CommentSlice::empty()
        } else {
            self.factory
                .source_comments(self.scanner.comment_directives().to_vec())
                .expect("owned directives")
        };
        {
            let file = self.factory.source_file_mut(root).expect("source metadata");
            file.comment_directives = comments;
            file.pragmas = pragmas;
        }
        self.process_pragmas_into_fields(root);
        let mut diagnostics = self.diagnostics.clone();
        attach_file_to_diagnostics(&mut diagnostics, root);
        let mut jsdoc_diagnostics = self.jsdoc_diagnostics.clone();
        attach_file_to_diagnostics(&mut jsdoc_diagnostics, root);
        let node_count = self.factory.node_count();
        let text_count = self.factory.text_count();
        self.reparsed_clones.sort_by(|&left, &right| {
            let left = self.factory.node(left).range();
            let right = self.factory.node(right).range();
            left.pos()
                .cmp(&right.pos())
                .then_with(|| left.end().cmp(&right.end()))
        });
        {
            let file = self.factory.source_file_mut(root).expect("source metadata");
            file.diagnostics = diagnostics;
            file.jsdoc_diagnostics = jsdoc_diagnostics;
            file.is_declaration_file = declaration;
            file.language_variant = self.language_variant;
            file.script_kind = self.script_kind;
            file.node_count = node_count;
            file.text_count = text_count;
            file.identifier_count = self.identifier_count;
            file.has_lazy_jsdoc = !matches!(self.script_kind, ScriptKind::JS | ScriptKind::JSX);
            file.reparsed_clones.clone_from(&self.reparsed_clones);
        }
        Factory::add_node_flags(&mut self.factory, root, self.source_flags);
        self.set_external_module_indicator(root);
    }
    // The Go finish path creates a new map after both initial parse and optional
    // await reparse. Only the final root escapes, so publish its final map once.
    fn seed_final_jsdoc_cache(&mut self, source: NodeId) {
        let mut entries = HashMap::new();
        for info in std::mem::take(&mut self.jsdoc_infos) {
            entries.insert(info.parent, info.js_docs);
        }
        for (parent, docs) in entries {
            self.factory
                .seed_source_jsdoc(source, parent, docs)
                .expect("eager JSDoc belongs to parsed file");
        }
    }
    /// port: tsc/internal/parser/parser.go:Parser.parseToplevelStatement
    pub(crate) fn parse_toplevel_statement(&mut self, mut index: usize) -> NodeId {
        self.statement_has_await_identifier = false;
        let statement = self.parse_statement();
        index += self.reparse_list.len();
        if self.statement_has_await_identifier
            && self.factory.node(statement).flags() & node_flags::AWAIT_CONTEXT == 0
        {
            if self.possible_await_spans.last() == Some(&index) {
                *self
                    .possible_await_spans
                    .last_mut()
                    .expect("last span observed") = index + 1;
            } else {
                self.possible_await_spans.extend([index, index + 1]);
            }
        }
        statement
    }
    /// port: tsc/internal/parser/parser.go:Parser.reparseTopLevelAwait
    pub(crate) fn reparse_top_level_await(&mut self, root: NodeId) -> NodeId {
        assert!(
            self.possible_await_spans.len().is_multiple_of(2),
            "possibleAwaitSpans malformed: odd number of indices, not paired into spans."
        );
        let (old_statements, eof) = {
            let node = self.factory.node(root);
            let file = node.data_source().as_source_file().expect("source payload");
            (
                file.statements().expect("parsed source statements"),
                file.end_of_file_token(),
            )
        };
        let original_nodes = self.factory.read_list(old_statements).nodes();
        let mut statements = vec![];
        let saved_diagnostics = std::mem::take(&mut self.diagnostics);
        let mut after = 0;
        let mut i = 0;
        while i < self.possible_await_spans.len() {
            let next = self.possible_await_spans[i];
            let previous = self
                .factory
                .read_nodes(original_nodes)
                .at(after)
                .expect("source statement");
            let next_node = self
                .factory
                .read_nodes(original_nodes)
                .at(next)
                .expect("source statement");
            statements.extend(
                self.factory
                    .read_nodes(
                        original_nodes
                            .slice(after..next)
                            .expect("source statement range"),
                    )
                    .iter()
                    .map(|id| id.expect("source statement")),
            );
            let previous_pos = self.factory.node(previous).range().pos();
            let next_pos = self.factory.node(next_node).range().pos();
            if let Some(start) = saved_diagnostics
                .iter()
                .position(|d| d.loc.pos() >= previous_pos)
            {
                let end = saved_diagnostics[start..]
                    .iter()
                    .position(|d| d.loc.pos() >= next_pos)
                    .map_or(saved_diagnostics.len(), |end| start + end);
                self.diagnostics
                    .extend_from_slice(&saved_diagnostics[start..end]);
            }
            let mut state = self.mark();
            self.context_flags |= node_flags::AWAIT_CONTEXT;
            self.scanner.reset_pos(next_pos);
            self.next_token();
            after = self.possible_await_spans[i + 1];
            while self.token != SyntaxKind::EndOfFile {
                let start = self.scanner.token_full_start();
                let statement = self.parse_statement();
                statements.push(statement);
                if start == self.scanner.token_full_start() {
                    self.next_token();
                }
                if after < original_nodes.len() {
                    let last = self
                        .factory
                        .read_nodes(original_nodes)
                        .at(after - 1)
                        .expect("last await statement");
                    let end = self.factory.node(statement).range().end();
                    let previous_end = self.factory.node(last).range().end();
                    if end == previous_end {
                        break;
                    }
                    if end > previous_end {
                        i += 2;
                        after = if i < self.possible_await_spans.len() {
                            self.possible_await_spans[i + 1]
                        } else {
                            original_nodes.len()
                        };
                    }
                }
            }
            state.diagnostics_len = self.diagnostics.len();
            self.rewind(state);
            i += 2;
        }
        if after < original_nodes.len() {
            let previous = self
                .factory
                .read_nodes(original_nodes)
                .at(after)
                .expect("source statement");
            statements.extend(
                self.factory
                    .read_nodes(
                        original_nodes
                            .slice(after..original_nodes.len())
                            .expect("source statement range"),
                    )
                    .iter()
                    .map(|id| id.expect("source statement")),
            );
            let previous_pos = self.factory.node(previous).range().pos();
            if let Some(start) = saved_diagnostics
                .iter()
                .position(|d| d.loc.pos() >= previous_pos)
            {
                self.diagnostics
                    .extend_from_slice(&saved_diagnostics[start..]);
            }
        }
        let loc = self.factory.read_list(old_statements).loc();
        let list = self.new_node_list(loc, statements);
        let root = self.factory.new_source_file(
            self.opts.clone(),
            self.source_owner.clone(),
            Some(list),
            eof,
        );
        let nodes = self.factory.read_list(list).nodes();
        for i in 0..nodes.len() {
            let node = self
                .factory
                .read_nodes(nodes)
                .at(i)
                .expect("source statement");
            Factory::set_node_parent(&mut self.factory, node, Some(root));
        }
        root
    }
}

/// port: tsc/internal/parser/parser.go:attachFileToDiagnostics
fn attach_file_to_diagnostics(diagnostics: &mut [Diagnostic], file: NodeId) {
    for diagnostic in diagnostics {
        diagnostic.file = Some(file);
        for related in &mut diagnostic.related_information {
            Arc::make_mut(related).file = Some(file);
        }
    }
}
