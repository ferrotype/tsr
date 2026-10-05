use crate::{
    completion_context::{Context, ALL},
    completion_items,
    syntax::Syntax,
    CompletionOptions, LanguageService, Result,
};
use std::collections::HashSet;
use tsr_ast::{utilities as ast, NodeId, SyntaxKind as K};
use tsr_lsproto as lsp;

impl LanguageService<'_> {
    // port: tsc/internal/ls/completions.go:LanguageService.getLabelStatementCompletions
    pub(crate) fn label_completions(
        &mut self,
        syntax: &mut Syntax<'_>,
        context: &Context,
        position: &lsp::Position,
        options: &CompletionOptions,
    ) -> Result<Option<lsp::CompletionList>> {
        let Some(previous) = context.previous else {
            return Ok(None);
        };
        let read = syntax.view.node(previous)?;
        let Some(parent) = read.parent() else {
            return Ok(None);
        };
        if !matches!(
            read.kind().known(),
            Some(K::BreakKeyword | K::ContinueKeyword | K::Identifier)
        ) || !matches!(
            syntax.view.node(parent)?.kind().known(),
            Some(K::BreakStatement | K::ContinueStatement)
        ) {
            return Ok(None);
        }
        let mut current: Option<NodeId> = Some(parent);
        let mut names = HashSet::new();
        let mut list = lsp::CompletionList::default();
        while let Some(id) = current {
            let read = syntax.view.node(id)?;
            if ast::is_function_like(Some(&read)) {
                break;
            }
            if read.kind() == K::LabeledStatement {
                if let Some(label) = read.label() {
                    let name = String::from_utf8_lossy(syntax.view.node_text(label)?.as_bytes())
                        .into_owned();
                    if names.insert(name.clone()) {
                        list.items.push(Some(Box::new(lsp::CompletionItem {
                            label: name,
                            kind: Some(Box::new(lsp::CompletionItemKind::PROPERTY)),
                            sort_text: Some(Box::new("11".into())),
                            ..Default::default()
                        })));
                    }
                }
            }
            current = read.parent();
        }
        if list.items.is_empty() {
            return Ok(Some(list));
        }
        let replacement = self.completion_replacement(syntax, context.location)?;
        completion_items::defaults(&mut list, options, position, replacement, ALL);
        Ok(Some(list))
    }
}
