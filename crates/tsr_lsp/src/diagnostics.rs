use crate::{
    client::{self, Client},
    error,
};
use std::collections::BTreeSet;
use tsr_ipc::Context;
use tsr_ls::converters::{Converters, DiagnosticOptions};
use tsr_lsproto as lsp;
use tsr_project::{project::ProjectKind, Project, Snapshot};

pub fn options(
    caps: &lsp::ClientCapabilities,
    locale: tsr_locale::Locale,
    pull: bool,
    style: bool,
) -> DiagnosticOptions {
    let doc = caps.text_document.as_deref();
    let (related, tags) = if pull {
        let caps = doc.and_then(|d| d.diagnostic.as_deref());
        (
            caps.and_then(|c| c.related_information.as_deref())
                .copied()
                .unwrap_or(false),
            caps.and_then(|c| c.tag_support.as_deref())
                .map(|tags| tags.value_set.clone())
                .unwrap_or_default(),
        )
    } else {
        let caps = doc.and_then(|d| d.publish_diagnostics.as_deref());
        (
            caps.and_then(|c| c.related_information.as_deref())
                .copied()
                .unwrap_or(false),
            caps.and_then(|c| c.tag_support.as_deref())
                .map(|tags| tags.value_set.clone())
                .unwrap_or_default(),
        )
    };
    DiagnosticOptions {
        locale,
        related_information: related,
        tags,
        visual_studio: caps
            .vs_supports_visual_studio_extensions
            .as_deref()
            .copied()
            .unwrap_or(false),
        style_checks_as_warnings: pull && style,
    }
}

// port: tsc/internal/ls/diagnostics.go:LanguageService.ProvideDiagnostics
pub fn document(
    context: &Context,
    request: &str,
    project: &Project,
    uri: &lsp::DocumentUri,
    encoding: tsr_jsstring::PositionEncoding,
    options: &DiagnosticOptions,
    enabled: bool,
) -> Result<lsp::RelatedFullDocumentDiagnosticReport, lsp::ResponseError> {
    if context.err().is_some() {
        return Err(crate::canceled());
    }
    let mut result = lsp::RelatedFullDocumentDiagnosticReport::default();
    if !enabled {
        return Ok(result);
    }
    let program = project
        .program()
        .ok_or_else(|| error(-32603, "project has no program"))?;
    let file = program
        .source_file(uri.file_name().as_bytes())
        .ok_or_else(|| error(-32603, "file is not in the project"))?;
    let checker = project
        .scheduler()
        .unwrap()
        .acquire(
            tsr_checker::CheckerLifetime::Diagnostics,
            Some(file.source()),
            context,
            request,
        )
        .map_err(|e| error(-32603, e.to_string()))?;
    let cancellation = tsr_core::CancellationToken::new();
    let cancel = cancellation.clone();
    let stop = context.after_func(move || cancel.cancel());
    struct Stop(tsr_ipc::AfterFuncStop);
    impl Drop for Stop {
        fn drop(&mut self) {
            self.0.stop();
        }
    }
    let _stop = Stop(stop);
    let mut operation = checker
        .operation()
        .map_err(|e| error(-32603, e.to_string()))?;
    let run = || -> Result<Vec<tsr_ast::Diagnostic>, tsr_compiler::Error> {
        let mut diagnostics = Vec::new();
        let source = file.bound().view().source_file()?;
        let ids: Vec<_> = std::iter::once(file.source())
            .chain(
                source
                    .supplemental_source_files()?
                    .iter()
                    .flatten()
                    .copied(),
            )
            .collect();
        for id in ids {
            let file = program.files().iter().find(|f| f.source() == id).ok_or(
                tsr_compiler::Error::Unsupported("missing supplemental source"),
            )?;
            diagnostics.extend(program.syntactic_diagnostics(Some(file))?);
            diagnostics.extend(program.semantic_diagnostics_in(
                &mut operation,
                file,
                Some(&cancellation),
            )?);
            diagnostics.extend(program.suggestion_diagnostics_in(
                &mut operation,
                file,
                Some(&cancellation),
            )?);
            if program.options().emit_declarations() && !cancellation.is_canceled() {
                diagnostics
                    .extend(program.declaration_diagnostics_with_checker(&mut operation, file)?);
            }
        }
        Ok(diagnostics)
    };
    let mut run = run;
    let diagnostics = run();
    if context.err().is_some() {
        return Err(crate::canceled());
    }
    let diagnostics = diagnostics.map_err(|e| error(-32603, e.to_string()))?;
    let mut converters = Converters::new(encoding);
    for diagnostic in with_synthesized_aggregates(program.as_ref(), diagnostics)
        .map_err(|e| error(-32603, e.to_string()))?
    {
        result.items.push(Some(Box::new(
            converters
                .diagnostic(program.as_ref(), &diagnostic, options)
                .map_err(|e| error(-32603, e.to_string()))?,
        )));
    }
    Ok(result)
}

pub(crate) fn open_projects(snapshot: &Snapshot) -> BTreeSet<Vec<u8>> {
    let mut names = BTreeSet::new();
    if let Some(fs) = snapshot.filesystem() {
        for path in fs.overlays().keys() {
            for project in snapshot.projects() {
                if project.contains_file(path.as_bytes()) {
                    names.insert(project.data().unwrap().path.as_bytes().to_vec());
                }
            }
        }
    }
    names
}
pub fn publish_project(
    client: &dyn Client,
    project: &Project,
    encoding: tsr_jsstring::PositionEncoding,
    options: &DiagnosticOptions,
    enabled: bool,
) -> Result<(), lsp::ResponseError> {
    let data = project.data().unwrap();
    let mut items = Vec::new();
    if enabled {
        items.extend_from_slice(&data.command_line.errors);
        items.extend_from_slice(
            data.program
                .program_diagnostics()
                .map_err(|e| error(-32603, e.to_string()))?,
        );
        items.extend(
            project
                .scheduler()
                .unwrap()
                .global_diagnostics()
                .map_err(|e| error(-32603, e.to_string()))?,
        );
    }
    let items = data
        .program
        .sort_and_deduplicate_diagnostics(&items)
        .map_err(|e| error(-32603, e.to_string()))?;
    let mut converter = Converters::new(encoding);
    let diagnostics = items
        .iter()
        .map(|d| {
            converter
                .diagnostic(data.program.as_ref(), d, options)
                .map(|d| Some(Box::new(d)))
        })
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| error(-32603, e.to_string()))?;
    client::notify(
        client,
        "textDocument/publishDiagnostics",
        &lsp::PublishDiagnosticsParams {
            uri: lsp::DocumentUri::from_file_name(data.name.as_bytes()),
            diagnostics,
            ..Default::default()
        },
    )
}
// port: tsc/internal/project/session.go:Session.publishProgramDiagnostics
pub fn published(
    client: &dyn Client,
    previous: &Snapshot,
    current: &Snapshot,
    encoding: tsr_jsstring::PositionEncoding,
    options: &DiagnosticOptions,
    enabled: bool,
) -> Result<(), lsp::ResponseError> {
    let old_open = open_projects(previous);
    let new_open = open_projects(current);
    for project in current.projects() {
        let data = project.data().unwrap();
        if data.kind != ProjectKind::Configured {
            continue;
        }
        let path = data.path.as_bytes();
        let was_open = old_open.contains(path);
        let is_open = new_open.contains(path);
        if is_open
            && (!was_open
                || Some(data.last_update) == current.id()
                    && data.update_kind != tsr_project::project::ProgramUpdateKind::Cloned)
        {
            publish_project(client, project, encoding, options, enabled)?;
        } else if was_open && !is_open {
            publish_project(client, project, encoding, options, false)?;
        }
    }
    for project in previous.projects() {
        let data = project.data().unwrap();
        if data.kind == ProjectKind::Configured
            && current.project_by_path(data.path.as_bytes()).is_none()
        {
            publish_project(client, project, encoding, options, false)?;
        }
    }
    Ok(())
}

// port: tsc/internal/ls/diagnostics.go:LanguageService.toLSPDiagnostics
// port: tsc/internal/ls/diagnostics.go:isSynthesizedContentMappedDiagnostic
// port: tsc/internal/ls/diagnostics.go:aggregateSynthesizedDiagnostics
fn with_synthesized_aggregates(
    program: &dyn tsr_compiler::diagnostic_writer::DiagnosticSources,
    diagnostics: Vec<tsr_ast::Diagnostic>,
) -> Result<Vec<tsr_ast::Diagnostic>, tsr_compiler::Error> {
    let mut out = Vec::new();
    let mut groups: Vec<(tsr_ast::NodeId, Vec<tsr_ast::Diagnostic>)> = Vec::new();
    for diagnostic in diagnostics {
        let synthesized = if let Some(id) = diagnostic.file.filter(|_| diagnostic.source.is_empty())
        {
            program
                .diagnostic_source(id)?
                .span_map()
                .is_some_and(|map| map.virtual_to_original_span(diagnostic.loc).1.is_none())
        } else {
            false
        };
        if synthesized {
            let id = diagnostic.file.unwrap();
            if let Some((_, group)) = groups.iter_mut().find(|(key, _)| *key == id) {
                group.push(diagnostic);
            } else {
                groups.push((id, vec![diagnostic]));
            }
        } else {
            out.push(diagnostic);
        }
    }
    for (id, group) in groups {
        let file = program.diagnostic_source(id)?;
        let mut aggregate = tsr_ast::Diagnostic::new(Some(id), tsr_core::TextRange::new(0,0), tsr_diagnostics::Virtual_code_produced_by_the_content_mapper_0_has_problems_with_no_corresponding_location_in_this_file, vec![tsr_jsstring::JsString::from_bytes(file.content_mapper())]);
        aggregate.category = worst_category(&group);
        aggregate.related_information = group.into_iter().map(std::sync::Arc::new).collect();
        out.push(aggregate);
    }
    Ok(out)
}
// port: tsc/internal/ls/diagnostics.go:worstCategory
fn worst_category(group: &[tsr_ast::Diagnostic]) -> i32 {
    let mut worst = group[0].category;
    for d in group {
        match d.category {
            1 => return 1,
            0 => worst = 0,
            _ => {}
        }
    }
    worst
}

#[cfg(test)]
mod tests {
    use super::*;
    use tsr_ast::{
        AstBuilder, AstFile, ContentMapperSourceFileInfo, Diagnostic, SourceFileParseOptions,
        SpanSegment,
    };
    use tsr_compiler::diagnostic_writer::DiagnosticSources;
    use tsr_core::TextRange;
    use tsr_jsstring::{JsString, SourceText};
    struct Sources(AstFile);
    impl DiagnosticSources for Sources {
        fn diagnostic_source(
            &self,
            id: tsr_ast::NodeId,
        ) -> Result<tsr_ast::SourceFileRead<'_>, tsr_compiler::Error> {
            Ok(self.0.view().source_file(id)?)
        }
    }
    #[test]
    fn synthesized_errors_aggregate_but_mapper_reports_keep_original_ranges() {
        let mut builder = AstBuilder::new(SourceText::default(), &tsr_arena::Counters::new());
        let id = builder.new_source_file(
            SourceFileParseOptions {
                file_name: JsString::from_bytes(b"/component.vue".as_slice()),
                ..Default::default()
            },
            SourceText::from_bytes(b"xxxfoo".as_slice()),
            None,
            None,
        );
        builder
            .source_file_mut(id)
            .unwrap()
            .set_content_mapper_info(ContentMapperSourceFileInfo {
                content_mapper: JsString::from_bytes(b"mapper".as_slice()),
                original_text: SourceText::from_bytes(b"\nfoo".as_slice()),
                span_map: Some(std::sync::Arc::new(tsr_ast::span_map::SpanMap::new(&[
                    SpanSegment {
                        virtual_start: 3,
                        virtual_end: 6,
                        original_start: 1,
                        original_end: 4,
                        features: tsr_ast::span_map::FEATURE_ALL,
                        ..Default::default()
                    },
                ]))),
                ..Default::default()
            });
        let sources = Sources(builder.complete(id).unwrap().publish_unbound());
        let make = |start, end, source: &[u8], category| {
            Diagnostic::external(
                Some(id),
                TextRange::new(start, end),
                JsString::from_bytes(source),
                category,
                999,
                JsString::from_bytes(b"detail".as_slice()),
            )
        };
        let diagnostics = vec![
            make(0, 1, b"", 0),
            make(3, 6, b"", 1),
            make(1, 2, b"", 1),
            make(1, 4, b"mapper", 0),
        ];
        let result = with_synthesized_aggregates(&sources, diagnostics).unwrap();
        assert_eq!(result.len(), 3);
        assert_eq!(result[0].loc, TextRange::new(3, 6));
        assert_eq!(result[1].source.as_bytes(), b"mapper");
        let aggregate = &result[2];
        assert_eq!(aggregate.category, 1);
        assert_eq!(aggregate.related_information.len(), 2);
        let mut converter = Converters::new(tsr_jsstring::PositionEncoding::Utf16);
        let options = DiagnosticOptions {
            related_information: true,
            ..Default::default()
        };
        let converted = converter.diagnostic(&sources, aggregate, &options).unwrap();
        assert_eq!(converted.range, lsp::Range::default());
        assert!(converted
            .related_information
            .unwrap()
            .iter()
            .flatten()
            .all(|r| r.location.range == lsp::Range::default()
                && r.location.uri.0 == "file:///component.vue"));
        for diagnostic in &result[..2] {
            let converted = converter
                .diagnostic(&sources, diagnostic, &options)
                .unwrap();
            assert_eq!(
                converted.range,
                lsp::Range {
                    start: lsp::Position {
                        line: 1,
                        character: 0
                    },
                    end: lsp::Position {
                        line: 1,
                        character: 3
                    }
                }
            );
        }
    }
}
