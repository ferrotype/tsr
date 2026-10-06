//! Generated-node edits share one private factory. Each formatted fragment is
//! cloned into its own owner before assigning positions and adopting text.
use crate::{
    change::{Changes, Tracker},
    syntax::Syntax,
    LanguageService, Result,
};
use tsr_ast::{AstBuilder, AstView, NodeId};
use tsr_compiler::{diagnostic_writer::DiagnosticSources, Program};
use tsr_core::TextRange;
use tsr_format::{FormatCodeSettings, FormatContext, FormatFile, SemicolonPreference};
use tsr_printer::EmitContext;

#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub(crate) enum Leading {
    #[default]
    None,
    Exclude,
    IncludeAll,
    JsDoc,
    StartLine,
}
#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub(crate) enum Trailing {
    #[default]
    None,
    Exclude,
    ExcludeWhitespace,
    Include,
}
#[derive(Clone, Default)]
pub(crate) struct NodeOptions {
    pub prefix: String,
    pub suffix: String,
    pub indentation: Option<i64>,
    pub delta: Option<i64>,
    pub leading: Leading,
    pub trailing: Trailing,
    pub joiner: String,
}
struct NodeEdit {
    index: usize,
    source: NodeId,
    range: TextRange,
    nodes: Vec<NodeId>,
    options: NodeOptions,
}
pub(crate) struct NodeTracker<'a> {
    pub ast: AstBuilder,
    pub emit: EmitContext,
    pub program: &'a Program,
    pub settings: FormatCodeSettings,
    pub newline: String,
    pub raw: Tracker,
    nodes: Vec<NodeEdit>,
    pub deleted: Vec<(NodeId, NodeId)>,
    pub insertions_at_start: Vec<(NodeId, NodeId)>,
}
impl<'a> NodeTracker<'a> {
    pub fn new(program: &'a Program, settings: &FormatCodeSettings) -> Self {
        let emit = EmitContext::default();
        let mut ast = AstBuilder::with_hooks(
            tsr_jsstring::SourceText::from_loaded_bytes(Vec::new()),
            &tsr_arena::Counters::new(),
            emit.factory_hooks(),
        );
        for file in program.files() {
            ast.retain_completed(file.bound());
        }
        if let Some(config) = &program.config().config_file {
            ast.retain_file(config.file.clone());
        }
        Self {
            ast,
            emit,
            program,
            settings: settings.clone(),
            newline: program.options().new_line.as_str().to_owned(),
            raw: Tracker::default(),
            nodes: Vec::new(),
            deleted: Vec::new(),
            insertions_at_start: Vec::new(),
        }
    }
    pub fn source_view(&self, source: NodeId) -> Result<AstView<'a>> {
        if let Some(file) = self.program.file_of_node(source) {
            return Ok(file.bound().view().ast());
        }
        self.program
            .config_source(source)
            .map(|config| config.file.view())
            .ok_or_else(|| tsr_arena::Error::WrongOwner.into())
    }
    pub fn syntax(&self, source: NodeId) -> Result<Syntax<'a>> {
        Syntax::new(self.source_view(source)?, source)
    }
    pub fn retain_generated(
        &mut self,
        nodes: tsr_checker::GeneratedTypeNodes,
        roots: &[NodeId],
    ) -> Result<()> {
        if let Some(&root) = roots.first() {
            self.ast
                .retain_file(nodes.ast.complete(root)?.publish_unbound());
        }
        Ok(())
    }
    // port: tsc/internal/ls/change/tracker.go:Tracker.ReplaceRangeWithNodes
    pub fn replace_nodes(
        &mut self,
        source: NodeId,
        range: TextRange,
        nodes: Vec<NodeId>,
        options: NodeOptions,
    ) {
        let index = self.raw.reserve(source, range);
        self.nodes.push(NodeEdit {
            index,
            source,
            range,
            nodes,
            options,
        });
    }
    pub fn replace_node(
        &mut self,
        source: NodeId,
        old: NodeId,
        new: NodeId,
        options: Option<NodeOptions>,
    ) -> Result<()> {
        self.replace_node_with_nodes(source, old, vec![new], options)
    }
    // port: tsc/internal/ls/change/tracker.go:Tracker.ReplaceNodeWithNodes
    pub fn replace_node_with_nodes(
        &mut self,
        source: NodeId,
        old: NodeId,
        new: Vec<NodeId>,
        options: Option<NodeOptions>,
    ) -> Result<()> {
        let options = options.unwrap_or_else(|| NodeOptions {
            leading: Leading::Exclude,
            trailing: Trailing::Exclude,
            ..Default::default()
        });
        let range = self.adjusted_range(source, old, old, options.leading, options.trailing)?;
        self.replace_nodes(source, range, new, options);
        Ok(())
    }
    pub fn insert_node(&mut self, source: NodeId, pos: i64, new: NodeId, options: NodeOptions) {
        self.replace_nodes(source, TextRange::new(pos, pos), vec![new], options);
    }
    pub fn delete_node(
        &mut self,
        source: NodeId,
        node: NodeId,
        leading: Leading,
        trailing: Trailing,
    ) -> Result<()> {
        self.delete_range(source, node, node, leading, trailing)
    }
    pub fn delete_range(
        &mut self,
        source: NodeId,
        first: NodeId,
        last: NodeId,
        leading: Leading,
        trailing: Trailing,
    ) -> Result<()> {
        let range = self.adjusted_range(source, first, last, leading, trailing)?;
        self.raw.replace_text(source, range, String::new());
        Ok(())
    }
    // port: tsc/internal/ls/change/tracker.go:Tracker.Delete
    pub fn delete(&mut self, source: NodeId, node: NodeId) {
        self.deleted.push((source, node));
    }

    pub fn finish(mut self, service: &mut LanguageService<'_>) -> Result<Changes> {
        self.finish_deletions()?;
        self.finish_insertions_at_start()?;
        let first = self.nodes.iter().find_map(|e| e.nodes.first().copied());
        let Some(first) = first else {
            return self.raw.finish(service);
        };
        let generated = self.ast.complete(first)?.publish_unbound();
        for edit in self.nodes {
            service.check_canceled()?;
            let source = self.program.diagnostic_source(edit.source)?;
            let mut printed = Vec::new();
            for node in edit.nodes {
                let mut ast = AstBuilder::with_hooks(
                    tsr_jsstring::SourceText::from_loaded_bytes(Vec::new()),
                    &tsr_arena::Counters::new(),
                    self.emit.factory_hooks(),
                );
                ast.retain_file(generated.clone());
                let node =
                    tsr_ast::deep_clone_preserving_ranges(&mut ast, Some(node)).expect("edit root");
                let text = tsr_printer::print_and_position_node_in_source(
                    &mut ast,
                    node,
                    Some(edit.source),
                    self.newline.as_bytes(),
                    self.settings.editor.indent_size as isize,
                    &self.emit,
                )?;
                let file = tsr_printer::create_synthetic_source_file(
                    &mut ast,
                    node,
                    &text,
                    source.parse_options().clone(),
                )?;
                let mut docs = tsr_parser::ParserJsDocProvider::default();
                let view = self
                    .program
                    .file_of_node(edit.source)
                    .map(|f| f.bound().view().ast())
                    .or_else(|| {
                        self.program
                            .config_source(edit.source)
                            .map(|f| f.file.view())
                    })
                    .ok_or(tsr_arena::Error::WrongOwner)?;
                let mut original = FormatFile {
                    view,
                    source: edit.source,
                    jsdoc: &mut docs,
                };
                let mut settings = self.settings.clone();
                if settings.semicolons == SemicolonPreference::Ignore
                    && !original.probably_uses_semicolons()?
                {
                    settings.semicolons = SemicolonPreference::Remove;
                }
                let pos = edit.range.pos();
                let initial = if let Some(indentation) = edit.options.indentation {
                    indentation
                } else {
                    tsr_format::get_indentation(
                        &mut original,
                        pos,
                        &settings,
                        edit.options.prefix == self.newline || line_start(&source, pos) == pos,
                    )?
                };
                let mut synthetic = FormatFile {
                    view: ast.view(),
                    source: file,
                    jsdoc: &mut docs,
                };
                let delta = if let Some(delta) = edit.options.delta {
                    delta
                } else if settings.editor.indent_size != 0
                    && tsr_format::should_indent_child_node(
                        &synthetic, &settings, node, None, false, false,
                    )?
                {
                    settings.editor.indent_size
                } else {
                    0
                };
                // Go's formatter context retains the original settings; the
                // writing settings above govern smart indentation only.
                let edits = tsr_format::format_node_given_indentation(
                    &mut synthetic,
                    &FormatContext::new(self.settings.clone(), self.newline.as_bytes()),
                    file,
                    source.language_variant,
                    initial,
                    delta,
                )?;
                let text = tsr_core::apply_bulk_edits(&text, &edits)?;
                let text = String::from_utf8_lossy(&text).into_owned();
                printed.push(text.strip_suffix(&self.newline).unwrap_or(&text).to_owned());
            }
            let joiner = if edit.options.joiner.is_empty() {
                &self.newline
            } else {
                &edit.options.joiner
            };
            let text = printed.join(joiner);
            let no_indent = if edit.options.indentation.is_some()
                || line_start(&source, edit.range.pos()) == edit.range.pos()
            {
                &text
            } else {
                text.trim_start_matches(char::is_whitespace)
            };
            let suffix = if no_indent.ends_with(&edit.options.suffix) {
                ""
            } else {
                &edit.options.suffix
            };
            let text = format!("{}{no_indent}{suffix}", edit.options.prefix);
            let text = reindent(&source, edit.range, &edit.options, &self.newline, text);
            self.raw.fill(edit.index, text);
        }
        self.raw.finish(service)
    }
}
pub(crate) fn line_start(source: &tsr_ast::SourceFileRead<'_>, pos: i64) -> i64 {
    // ECMA line maps include Unicode separators, unlike byte-only CR/LF scans.
    let lines = source.ecma_line_map();
    i64::from(
        lines[lines
            .partition_point(|&p| i64::from(p) <= pos)
            .saturating_sub(1)],
    )
}
// port: tsc/internal/ls/change/trackerimpl.go:Tracker.reindentInsertedLines
pub(crate) fn reindent(
    source: &tsr_ast::SourceFileRead<'_>,
    range: TextRange,
    options: &NodeOptions,
    newline: &str,
    mut text: String,
) -> String {
    if text.is_empty()
        || range.pos() != range.end()
        || options.indentation.is_some()
        || !text.ends_with(newline)
    {
        return text;
    }
    let mut pos = range.pos();
    if let Some(spans) = source.span_map() {
        let (mapped, fidelity) = spans.virtual_to_original_position(pos as i32);
        if !fidelity.is_exact() {
            return text;
        }
        pos = i64::from(mapped);
    }
    let original = source.original_text();
    let Ok(pos) = usize::try_from(pos) else {
        return text;
    };
    if pos > original.len() {
        return text;
    }
    let start = original[..pos]
        .iter()
        .rposition(|&c| c == b'\r' || c == b'\n')
        .map_or(0, |p| p + 1);
    let indent = |t: &[u8]| t.iter().take_while(|&&b| b == b' ' || b == b'\t').count();
    if start == pos {
        if indent(text.as_bytes()) == 0 {
            text.insert_str(
                0,
                &String::from_utf8_lossy(&original[start..start + indent(&original[start..])]),
            );
        }
    } else if indent(&original[start..pos]) == pos - start {
        text.push_str(&String::from_utf8_lossy(&original[start..pos]));
    }
    text
}
