//! Checker handles are used only while the completion or editing request holds its lease.
use crate::{syntax::Syntax, CompletionOptions, LanguageService, Result};
use tsr_ast::{span_map::FEATURE_COMPLETION, NodeId};
use tsr_autoimport::ImportAdder;
use tsr_checker::{GeneratedTypeNodes, Operation, SymbolRef};
use tsr_lsproto as lsp;

impl LanguageService<'_> {
    pub(crate) fn import_generated_types(
        &self,
        checker: &mut Operation<'_>,
        syntax: &Syntax<'_>,
        nodes: &mut GeneratedTypeNodes,
        roots: &mut [NodeId],
        options: &CompletionOptions,
        adder: &mut ImportAdder,
    ) -> Result<()> {
        let symbols =
            tsr_autoimport::type_nodes::importable_references(nodes, roots, checker, self.program)?;
        if symbols.is_empty() {
            return Ok(());
        }
        let registry = self.prepare_auto_imports(checker, syntax, &options.auto_import)?;
        let mut targets = std::collections::HashSet::<SymbolRef>::new();
        for symbol in symbols {
            let symbol = checker.skip_alias(symbol)?;
            let target = checker.get_merged_symbol(symbol)?;
            if !targets.insert(target) {
                continue;
            }
            let Some(id) = tsr_autoimport::export_id_for_symbol(self.program, checker, target)?
            else {
                continue;
            };
            let mut fixes = Vec::new();
            for export in registry
                .index
                .entries()
                .iter()
                .filter(|e| e.id == id && e.id.module.as_bytes() != syntax.file.path())
            {
                self.check_canceled()?;
                fixes.extend(tsr_autoimport::fix::fixes(
                    self.program,
                    checker,
                    syntax.source,
                    export,
                    tsr_autoimport::fix::Usage {
                        type_only: true,
                        ..Default::default()
                    },
                    &options.auto_import,
                )?);
            }
            fixes.sort_by(crate::auto_imports::rank);
            if let Some(fix) = fixes.into_iter().next() {
                adder.add(fix, self.program.options().verbatim_module_syntax.is_true());
            }
        }
        Ok(())
    }
    pub(crate) fn import_adder_edits(
        &mut self,
        syntax: &Syntax<'_>,
        options: &CompletionOptions,
        adder: &ImportAdder,
    ) -> Result<Vec<Option<Box<lsp::TextEdit>>>> {
        self.map_import_adder_edits(syntax, options, adder, Some(FEATURE_COMPLETION))
    }
    pub(crate) fn import_adder_action_edits(
        &mut self,
        syntax: &Syntax<'_>,
        options: &CompletionOptions,
        adder: &ImportAdder,
    ) -> Result<Vec<Option<Box<lsp::TextEdit>>>> {
        self.map_import_adder_edits(syntax, options, adder, None)
    }
    fn map_import_adder_edits(
        &mut self,
        syntax: &Syntax<'_>,
        options: &CompletionOptions,
        adder: &ImportAdder,
        feature: Option<i32>,
    ) -> Result<Vec<Option<Box<lsp::TextEdit>>>> {
        if !adder.has_fixes() {
            return Ok(Vec::new());
        }
        let settings = crate::completion_snippets::settings(options, syntax)?;
        let edits = adder.edits(
            syntax.view,
            syntax.source,
            &tsr_autoimport::edits::Options {
                format: &options.format,
                locale: &options.locale,
                single_quote: crate::inlay_hints::single_quote(syntax, options.quote)?,
                semicolons: settings.semicolons != tsr_format::SemicolonPreference::Remove,
                prefer_type_only: options.prefer_type_only,
                verbatim: self.program.options().verbatim_module_syntax.is_true(),
                newline: self.program.options().new_line.as_str(),
                usage: None,
            },
        )?;
        let mut result = Vec::new();
        for edit in edits {
            let range = tsr_core::TextRange::new(edit.start, edit.end);
            let text = crate::change_nodes::reindent(
                &syntax.file,
                range,
                &crate::change_nodes::NodeOptions::default(),
                self.program.options().new_line.as_str(),
                edit.text,
            );
            let (range, fidelity) = match feature {
                Some(feature) => self.range(syntax.source, range, feature)?,
                None => self.unrestricted_range(syntax.source, range)?,
            };
            if !fidelity.is_exact() {
                return Ok(Vec::new());
            }
            result.push(Some(Box::new(lsp::TextEdit {
                range,
                new_text: text,
            })));
        }
        Ok(result)
    }
}
