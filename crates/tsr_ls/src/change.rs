//! Request-local editing transaction. All projection edits are mapped before
//! ordering; one unmappable or conflicting projection drops the entire file.
use crate::{LanguageService, Result};
use std::collections::{BTreeMap, BTreeSet};
use tsr_ast::NodeId;
use tsr_core::TextRange;
use tsr_lsproto as lsp;

#[derive(Default)]
pub(crate) struct Tracker {
    changes: Vec<(NodeId, TextRange, String)>,
}
pub(crate) struct Changes {
    pub edits: BTreeMap<lsp::DocumentUri, Vec<lsp::TextEdit>>,
    pub unmappable: Vec<lsp::DocumentUri>,
}
impl Tracker {
    // port: tsc/internal/ls/change/tracker.go:Tracker.ReplaceTextRangeWithText
    pub fn replace_text(&mut self, source: NodeId, range: TextRange, text: String) {
        self.changes.push((source, range, text));
    }

    pub fn reserve(&mut self, source: NodeId, range: TextRange) -> usize {
        let index = self.changes.len();
        self.replace_text(source, range, String::new());
        index
    }
    pub fn fill(&mut self, index: usize, text: String) {
        self.changes[index].2 = text;
    }

    // port: tsc/internal/ls/change/tracker.go:Tracker.GetChanges
    pub fn finish(self, service: &mut LanguageService<'_>) -> Result<Changes> {
        let mut edits: BTreeMap<lsp::DocumentUri, Vec<lsp::TextEdit>> = BTreeMap::new();
        let mut projections: BTreeMap<lsp::DocumentUri, BTreeSet<NodeId>> = BTreeMap::new();
        let mut unmappable = BTreeSet::new();
        for (source, range, new_text) in self.changes {
            service.check_canceled()?;
            let uri = lsp::DocumentUri::from_file_name(
                service.source(source)?.original_file_name()?.as_bytes(),
            );
            let (range, fidelity) = service.unrestricted_range(source, range)?;
            if !fidelity.is_exact() {
                unmappable.insert(uri.clone());
            }
            projections.entry(uri.clone()).or_default().insert(source);
            edits
                .entry(uri)
                .or_default()
                .push(lsp::TextEdit { range, new_text });
        }
        for (uri, items) in &mut edits {
            if unmappable.contains(uri) {
                continue;
            }
            items.sort_by_key(|e| range_key(&e.range));
            let multiple = projections[uri].len() > 1;
            if multiple {
                items.dedup_by(|b, a| a.range == b.range && a.new_text == b.new_text);
            }
            for pair in items.windows(2) {
                if text_edits_conflict(&pair[0], &pair[1], multiple) {
                    if multiple {
                        unmappable.insert(uri.clone());
                        break;
                    }
                    // A same-projection overlap is a bug in the provider,
                    // rather than ambiguous mapper output (the pin panics).
                    panic!(
                        "changes overlap: {:?} and {:?}",
                        pair[0].range, pair[1].range
                    );
                }
            }
        }
        edits.retain(|uri, _| !unmappable.contains(uri));
        Ok(Changes {
            edits,
            unmappable: unmappable.into_iter().collect(),
        })
    }
}
pub(crate) fn range_key(r: &lsp::Range) -> (u32, u32, u32, u32) {
    (r.start.line, r.start.character, r.end.line, r.end.character)
}
// port: tsc/internal/ls/change/trackerimpl.go:textEditsConflict
fn text_edits_conflict(a: &lsp::TextEdit, b: &lsp::TextEdit, multiple: bool) -> bool {
    (a.range.end.line, a.range.end.character) > (b.range.start.line, b.range.start.character)
        || multiple
            && a.range.start == a.range.end
            && a.range == b.range
            && a.new_text != b.new_text
}
pub(crate) fn document_edits(
    changes: Changes,
) -> Vec<lsp::TextDocumentEditOrCreateFileOrRenameFileOrDeleteFile> {
    debug_assert!(changes
        .unmappable
        .iter()
        .all(|uri| !changes.edits.contains_key(uri)));
    changes
        .edits
        .into_iter()
        .map(
            |(uri, edits)| lsp::TextDocumentEditOrCreateFileOrRenameFileOrDeleteFile {
                text_document_edit: Some(Box::new(lsp::TextDocumentEdit {
                    text_document: lsp::OptionalVersionedTextDocumentIdentifier {
                        uri,
                        ..Default::default()
                    },
                    edits: edits
                        .into_iter()
                        .map(|edit| lsp::TextEditOrAnnotatedTextEditOrSnippetTextEdit {
                            text_edit: Some(Box::new(edit)),
                            ..Default::default()
                        })
                        .collect(),
                })),
                ..Default::default()
            },
        )
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    // source: tsc/internal/ls/change/trackerimpl_test.go:TestTextEditsConflictAtSameInsertionPointAcrossProjections
    #[test]
    fn insertions_conflict_only_between_projections() {
        let a = lsp::TextEdit {
            range: lsp::Range {
                start: lsp::Position {
                    line: 1,
                    character: 2,
                },
                end: lsp::Position {
                    line: 1,
                    character: 2,
                },
            },
            new_text: "a".into(),
        };
        let b = lsp::TextEdit {
            new_text: "b".into(),
            ..a.clone()
        };
        assert!(text_edits_conflict(&a, &b, true));
        assert!(!text_edits_conflict(&a, &b, false));
        assert!(!text_edits_conflict(&a, &a, true));
    }
}
