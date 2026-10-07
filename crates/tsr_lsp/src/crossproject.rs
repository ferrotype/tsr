//! Cross-project request orchestration retains each snapshot until its checker
//! work ends. No checker lease is held while loading another project tree.
use crate::{
    client,
    language_features::{self, Options, Request},
};
use std::{
    collections::{HashMap, HashSet, VecDeque},
    sync::Arc,
};
use tsr_ipc::Context;
use tsr_json::RawValue;
use tsr_jsstring::{JsString, PositionEncoding};
use tsr_lsproto as lsp;
use tsr_project::{
    api::{ProjectTreeRequest, ResourceRequest},
    session::Session,
    Snapshot,
};
use tsr_vfs::FileSystem;

type Result<T> = std::result::Result<T, lsp::ResponseError>;

impl Request {
    pub(crate) fn crosses_projects(&self) -> bool {
        matches!(
            self,
            Self::References(_)
                | Self::VSReferences(_)
                | Self::Implementation(_)
                | Self::Rename(_)
                | Self::CallIncoming(_)
                | Self::ResolveLens(_)
        )
    }
    fn position(&self) -> &lsp::Position {
        match self {
            Self::References(p) | Self::VSReferences(p) => &p.position,
            Self::Implementation(p) | Self::LensImplementations(p) => &p.position,
            Self::Rename(p) => &p.position,
            Self::IncomingAt(p) => &p.position,
            Self::LensLocations(p) => &p.range.start,
            _ => unreachable!("cross-project request"),
        }
    }
    fn at(&self, uri: lsp::DocumentUri, position: lsp::Position) -> Self {
        let mut request = self.clone();
        match &mut request {
            Self::References(p) | Self::VSReferences(p) => {
                p.text_document.uri = uri;
                p.position = position;
            }
            Self::Implementation(p) | Self::LensImplementations(p) => {
                p.text_document.uri = uri;
                p.position = position;
            }
            Self::Rename(p) => {
                p.text_document.uri = uri;
                p.position = position;
            }
            Self::IncomingAt(p) => {
                p.text_document.uri = uri;
                p.position = position;
            }
            Self::LensLocations(lens) => {
                return if lens.data.as_ref().expect("validated lens").kind.0
                    == lsp::CodeLensKind::IMPLEMENTATIONS
                {
                    Self::LensImplementations(lsp::ImplementationParams {
                        text_document: lsp::TextDocumentIdentifier { uri },
                        position,
                        ..Default::default()
                    })
                } else {
                    Self::References(lsp::ReferenceParams {
                        text_document: lsp::TextDocumentIdentifier { uri },
                        position,
                        context: Some(Box::new(lsp::ReferenceContext {
                            include_declaration: false,
                        })),
                        ..Default::default()
                    })
                };
            }
            _ => unreachable!("cross-project request"),
        }
        request
    }
}

struct Work {
    project: JsString,
    uri: lsp::DocumentUri,
    position: lsp::Position,
    original: bool,
}

/// Same routing and result preference as the pin's `handleCrossProject`.
// port: tsc/internal/ls/crossproject.go:handleCrossProject
#[allow(
    clippy::too_many_arguments,
    reason = "request execution context shared with single-project dispatch"
)]
pub(crate) fn execute(
    context: &Context,
    request_id: &str,
    session: &Session,
    host: &Arc<dyn tsr_vfs::FileSystem>,
    initial: &Snapshot,
    request: &Request,
    encoding: PositionEncoding,
    capabilities: &lsp::ClientCapabilities,
    options: &Options,
) -> Result<RawValue> {
    let path = request.uri().path(
        initial
            .filesystem()
            .unwrap()
            .use_case_sensitive_file_names(),
    );
    let default = initial
        .project_for_file(path.as_bytes())
        .ok_or_else(|| crate::error(-32603, "no default project"))?;
    if let Request::CallIncoming(item) = request {
        let positions = language_features::execute(
            context,
            request_id,
            Some(default),
            Request::IncomingPositions(item.clone()),
            encoding,
            capabilities,
            options,
        )?;
        let mut results = Vec::new();
        for position in decode::<Vec<lsp::TextDocumentPositionParams>>(&positions)? {
            let result = execute_search(
                context,
                request_id,
                session,
                host,
                initial,
                default,
                path.as_bytes(),
                &Request::IncomingAt(position),
                encoding,
                capabilities,
                options,
            )?;
            results.push(decode::<lsp::CallHierarchyIncomingCallsOrNull>(&result)?);
        }
        return client::raw(&merge_incoming_declarations(results));
    }
    if let Request::ResolveLens(lens) = request {
        let result = execute_search(
            context,
            request_id,
            session,
            host,
            initial,
            default,
            path.as_bytes(),
            &Request::LensLocations(lens.clone()),
            encoding,
            capabilities,
            options,
        )?;
        let locations = decode::<lsp::LocationsOrNull>(&result)?
            .locations
            .map(|v| *v)
            .unwrap_or_default();
        return client::raw(
            &tsr_ls::LanguageService::code_lens_result(
                lens.clone(),
                &locations,
                options.lens_command.as_deref(),
                &options.locale,
            )
            .map_err(language_features::service_error)?,
        );
    }
    execute_search(
        context,
        request_id,
        session,
        host,
        initial,
        default,
        path.as_bytes(),
        request,
        encoding,
        capabilities,
        options,
    )
}

#[allow(
    clippy::too_many_arguments,
    reason = "retain the initial project and routing context through declaration searches"
)]
fn execute_search(
    context: &Context,
    request_id: &str,
    session: &Session,
    host: &Arc<dyn FileSystem>,
    initial: &Snapshot,
    default: &tsr_project::Project,
    initial_path: &[u8],
    request: &Request,
    encoding: PositionEncoding,
    capabilities: &lsp::ClientCapabilities,
    options: &Options,
) -> Result<RawValue> {
    let mut default_result = if matches!(request, Request::Rename(_) | Request::LensLocations(_)) {
        Some(language_features::execute_with_targets(
            context,
            request_id,
            Some(default),
            request.clone(),
            encoding,
            capabilities,
            options,
        )?)
    } else {
        None
    };
    if let Some((result, _)) = default_result
        .as_ref()
        .filter(|_| matches!(request, Request::Rename(_)))
    {
        let edit = decode::<lsp::WorkspaceEditOrNull>(result)?;
        let moved = edit
            .workspace_edit
            .as_ref()
            .and_then(|e| e.document_changes.as_deref())
            .and_then(|changes| {
                changes
                    .iter()
                    // Single-project dispatch appends the requested move
                    // after associated resource moves (for example CSS).
                    // Recompute across projects for that original file.
                    .rev()
                    .find_map(|change| change.rename_file.as_deref())
            });
        if let Some(moved) = moved {
            let will_rename = capabilities
                .workspace
                .as_deref()
                .and_then(|w| w.file_operations.as_deref())
                .and_then(|w| w.will_rename.as_deref())
                .copied()
                .unwrap_or(false);
            if will_rename {
                return Ok(result.clone());
            }
            let snapshot = session
                .flush_resources(
                    &ResourceRequest {
                        documents: vec![moved.old_uri.clone()],
                        project_tree: Some(ProjectTreeRequest::All),
                        ..Default::default()
                    },
                    host.clone(),
                )
                .map_err(crate::project_error)?;
            let all = language_features::file_renames(
                context,
                request_id,
                &snapshot,
                &lsp::RenameFilesParams {
                    files: vec![Some(Box::new(lsp::FileRename {
                        old_uri: moved.old_uri.0.clone(),
                        new_uri: moved.new_uri.0.clone(),
                    }))],
                },
                encoding,
                capabilities,
                &options.completion,
            )?;
            let resource = client::raw(&lsp::WorkspaceEditOrNull {
                workspace_edit: Some(Box::new(lsp::WorkspaceEdit {
                    document_changes: Some(Box::new(vec![
                        lsp::TextDocumentEditOrCreateFileOrRenameFileOrDeleteFile {
                            rename_file: Some(Box::new(moved.clone())),
                            ..Default::default()
                        },
                    ])),
                    ..Default::default()
                })),
            })?;
            if context.err().is_some() {
                return Err(crate::canceled());
            }
            return combine_rename(&[all, resource]);
        }
    }
    let default_id = default.data().unwrap().path.clone();
    let mut queue = VecDeque::new();
    let mut enqueued = HashSet::new();
    for project in std::iter::once(default).chain(
        initial
            .projects()
            .into_iter()
            .filter(|p| p.contains_file(initial_path)),
    ) {
        let key = project.data().unwrap().path.clone();
        if enqueued.insert(key.clone()) {
            queue.push_back(Work {
                project: key,
                uri: request.uri().clone(),
                position: request.position().clone(),
                original: false,
            });
        }
    }
    let initial_order: Vec<_> = queue.iter().map(|work| work.project.clone()).collect();
    let mut results = Vec::new();
    let mut definition = None;
    loop {
        while let Some(work) = queue.pop_front() {
            if context.err().is_some() {
                return Err(crate::canceled());
            }
            // The initial default service is retained by request preparation.
            // Go refreshes other projects, but passes this service directly as
            // initialItem.ls even if a later notification has queued an edit.
            let snapshot = if work.project == default_id {
                initial.clone()
            } else {
                session
                    .flush_resources(
                        &ResourceRequest {
                            projects: [work.project.clone()].into(),
                            ..Default::default()
                        },
                        host.clone(),
                    )
                    .map_err(crate::project_error)?
            };
            let Some(project) = snapshot
                .project_by_path(work.project.as_bytes())
                .filter(|p| p.contains_file(work.uri.file_name().as_bytes()))
            else {
                continue;
            };
            let (result, targets) = if work.project == default_id {
                if let Some(result) = default_result.take() {
                    result
                } else {
                    language_features::execute_with_targets(
                        context,
                        request_id,
                        Some(project),
                        request.at(work.uri, work.position),
                        encoding,
                        capabilities,
                        options,
                    )?
                }
            } else {
                language_features::execute_with_targets(
                    context,
                    request_id,
                    Some(project),
                    request.at(work.uri, work.position),
                    encoding,
                    capabilities,
                    options,
                )?
            };
            // execute_with_targets has released its checker lease. Project
            // discovery below may safely load programs and acquire checkers.
            if work.project == default_id && definition.is_none() {
                definition = targets.default_definition;
            }
            for position in targets.original_positions {
                let Ok(other) = session.flush_resources(
                    &ResourceRequest {
                        configured_documents: vec![position.uri.clone()],
                        ..Default::default()
                    },
                    host.clone(),
                ) else {
                    // The pin ignores failed discovery of an original
                    // declaration project and continues the existing search.
                    continue;
                };
                for project in other
                    .projects()
                    .into_iter()
                    .filter(|p| p.contains_file(position.uri.file_name().as_bytes()))
                {
                    let key = project.data().unwrap().path.clone();
                    if enqueued.insert(key.clone()) {
                        queue.push_back(Work {
                            project: key,
                            uri: position.uri.clone(),
                            position: position.position.clone(),
                            original: true,
                        });
                    }
                }
            }
            results.push((work.project, work.original, result));
        }
        if context.err().is_some() {
            return Err(crate::canceled());
        }
        if let Some(definition) = &definition {
            let snapshot = session
                .flush_resources(
                    &ResourceRequest {
                        project_tree: Some(ProjectTreeRequest::Referencing(
                            results.iter().map(|(key, _, _)| key.clone()).collect(),
                        )),
                        ..Default::default()
                    },
                    host.clone(),
                )
                .map_err(crate::project_error)?;
            for project in snapshot.projects() {
                let key = project.data().unwrap().path.clone();
                if enqueued.contains(&key) {
                    continue;
                }
                let position = std::iter::once(&definition.position)
                    .chain(definition.source.iter())
                    .chain(definition.generated.iter())
                    .find(|position| project.contains_file(position.uri.file_name().as_bytes()));
                if let Some(position) = position {
                    enqueued.insert(key.clone());
                    queue.push_back(Work {
                        project: key,
                        uri: position.uri.clone(),
                        position: position.position.clone(),
                        original: false,
                    });
                }
            }
        }
        if context.err().is_some() {
            return Err(crate::canceled());
        }
        if queue.is_empty() {
            break;
        }
    }
    results.sort_by_key(|(key, original, _)| {
        (
            initial_order
                .iter()
                .position(|id| id == key)
                .unwrap_or(initial_order.len() + usize::from(*original)),
            key.clone(),
        )
    });
    let results: Vec<_> = results.into_iter().map(|(_, _, value)| value).collect();
    if results.len() == 1 {
        return Ok(results.into_iter().next().unwrap());
    }
    combine(request, &results)
}

fn decode<T: tsr_json::Decode + Default>(raw: &RawValue) -> Result<T> {
    let mut result = T::default();
    tsr_json::unmarshal(&raw.0, &mut result, tsr_json::Options::default())
        .map_err(|e| crate::error(-32603, e.to_string()))?;
    Ok(result)
}
type RangeKey = (u32, u32, u32, u32);
fn range_key(range: &lsp::Range) -> RangeKey {
    (
        range.start.line,
        range.start.character,
        range.end.line,
        range.end.character,
    )
}
fn location_key(location: &lsp::Location) -> (lsp::DocumentUri, RangeKey) {
    (location.uri.clone(), range_key(&location.range))
}

fn combine(request: &Request, results: &[RawValue]) -> Result<RawValue> {
    match request {
        Request::References(_) | Request::LensLocations(_) => {
            let mut seen = HashSet::new();
            let mut values = Vec::new();
            for result in results {
                for location in decode::<lsp::LocationsOrNull>(result)?
                    .locations
                    .into_iter()
                    .flat_map(|v| *v)
                {
                    if seen.insert(location_key(&location)) {
                        values.push(location);
                    }
                }
            }
            client::raw(&lsp::LocationsOrNull {
                locations: Some(Box::new(values)),
            })
        }
        Request::VSReferences(_) => {
            let mut values = Vec::new();
            let mut next = 0;
            for result in results {
                let mut ids = HashMap::new();
                for item in decode::<lsp::VSReferenceItemsOrNull>(result)?
                    .vs_reference_items
                    .into_iter()
                    .flat_map(|v| *v)
                    .flatten()
                {
                    let mut item = item;
                    ids.insert(item.vs_id, next);
                    item.vs_id = next;
                    next += 1;
                    if let Some(id) = item.vs_definition_id.as_mut() {
                        **id = ids.get(id).copied().unwrap_or_default();
                    }
                    values.push(Some(item));
                }
            }
            client::raw(&lsp::VSReferenceItemsOrNull {
                vs_reference_items: Some(Box::new(values)),
            })
        }
        Request::Implementation(_) | Request::LensImplementations(_) => {
            let values = results
                .iter()
                .map(decode::<lsp::LocationOrLocationsOrDefinitionLinksOrNull>)
                .collect::<Result<Vec<_>>>()?;
            let mut seen = HashSet::new();
            // An empty JSON array decodes as Locations even when the service
            // returned empty DefinitionLinks. It must not discard other links.
            if values.iter().any(|v| {
                v.locations
                    .as_ref()
                    .is_some_and(|locations| !locations.is_empty())
            }) {
                let mut locations = Vec::new();
                for location in values
                    .into_iter()
                    .flat_map(|v| v.locations.into_iter().flat_map(|v| *v))
                {
                    if seen.insert(location_key(&location)) {
                        locations.push(location);
                    }
                }
                client::raw(&lsp::LocationOrLocationsOrDefinitionLinksOrNull {
                    locations: Some(Box::new(locations)),
                    ..Default::default()
                })
            } else {
                let mut links = Vec::new();
                for link in values
                    .into_iter()
                    .flat_map(|v| v.definition_links.into_iter().flat_map(|v| *v))
                    .flatten()
                {
                    if seen.insert((link.target_uri.clone(), range_key(&link.target_range))) {
                        links.push(Some(link));
                    }
                }
                client::raw(&lsp::LocationOrLocationsOrDefinitionLinksOrNull {
                    definition_links: Some(Box::new(links)),
                    ..Default::default()
                })
            }
        }
        Request::Rename(_) => combine_rename(results),
        Request::IncomingAt(_) => {
            let mut seen = HashSet::new();
            let mut values = Vec::new();
            for result in results {
                for call in decode::<lsp::CallHierarchyIncomingCallsOrNull>(result)?
                    .call_hierarchy_incoming_calls
                    .into_iter()
                    .flat_map(|v| *v)
                    .flatten()
                {
                    if let Some(from) = &call.from {
                        if seen.insert((from.uri.clone(), range_key(&from.range))) {
                            values.push(Some(call));
                        }
                    }
                }
            }
            values.sort_by(|a, b| {
                let a = a.as_ref().unwrap();
                let b = b.as_ref().unwrap();
                a.from
                    .as_ref()
                    .unwrap()
                    .uri
                    .0
                    .cmp(&b.from.as_ref().unwrap().uri.0)
                    .then_with(|| match (a.from_ranges.first(), b.from_ranges.first()) {
                        (Some(a), Some(b)) => range_key(a).cmp(&range_key(b)),
                        _ => std::cmp::Ordering::Equal,
                    })
            });
            client::raw(&lsp::CallHierarchyIncomingCallsOrNull {
                call_hierarchy_incoming_calls: (!values.is_empty()).then(|| Box::new(values)),
            })
        }
        _ => unreachable!("cross-project result"),
    }
}

// Go combines projects first (first project wins for a caller), then combines
// declaration responses by selection range, preserving order and unique ranges.
fn merge_incoming_declarations(
    results: Vec<lsp::CallHierarchyIncomingCallsOrNull>,
) -> lsp::CallHierarchyIncomingCallsOrNull {
    let mut values: Vec<Option<Box<lsp::CallHierarchyIncomingCall>>> = Vec::new();
    let mut indices = HashMap::<_, usize>::new();
    for result in results {
        for call in result
            .call_hierarchy_incoming_calls
            .into_iter()
            .flat_map(|v| *v)
            .flatten()
        {
            let Some(from) = &call.from else {
                continue;
            };
            let key = (from.uri.clone(), range_key(&from.selection_range));
            if let Some(&index) = indices.get(&key) {
                let existing = values[index].as_mut().expect("recorded caller");
                for range in call.from_ranges {
                    if !existing.from_ranges.contains(&range) {
                        existing.from_ranges.push(range);
                    }
                }
            } else {
                indices.insert(key, values.len());
                values.push(Some(call));
            }
        }
    }
    lsp::CallHierarchyIncomingCallsOrNull {
        call_hierarchy_incoming_calls: (!values.is_empty()).then(|| Box::new(values)),
    }
}

// port: tsc/internal/ls/crossproject.go:combineRenameResponse
fn combine_rename(results: &[RawValue]) -> Result<RawValue> {
    let mut changes: HashMap<lsp::DocumentUri, Vec<Option<Box<lsp::TextEdit>>>> = HashMap::new();
    let mut ranges: HashMap<lsp::DocumentUri, HashSet<RangeKey>> = HashMap::new();
    let mut documents = Vec::new();
    let mut renames = HashSet::new();
    for result in results {
        let Some(edit) = decode::<lsp::WorkspaceEditOrNull>(result)?.workspace_edit else {
            continue;
        };
        for change in edit.document_changes.into_iter().flat_map(|v| *v) {
            if change
                .rename_file
                .as_ref()
                .is_none_or(|r| renames.insert((r.old_uri.clone(), r.new_uri.clone())))
            {
                documents.push(change);
            }
        }
        for (uri, edits) in edit.changes.into_iter().flat_map(|m| *m) {
            for edit in edits.into_iter().flatten() {
                if ranges
                    .entry(uri.clone())
                    .or_default()
                    .insert(range_key(&edit.range))
                {
                    changes.entry(uri.clone()).or_default().push(Some(edit));
                }
            }
        }
    }
    client::raw(&lsp::WorkspaceEditOrNull {
        workspace_edit: (!changes.is_empty() || !documents.is_empty()).then(|| {
            Box::new(lsp::WorkspaceEdit {
                changes: (!changes.is_empty()).then(|| Box::new(changes)),
                document_changes: (!documents.is_empty()).then(|| Box::new(documents)),
                ..Default::default()
            })
        }),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    fn raw(text: &str) -> RawValue {
        RawValue(text.as_bytes().to_vec())
    }
    #[test]
    fn incoming_declarations_preserve_order_and_merge_unique_ranges() {
        fn call(uri: &str, start: u32, ranges: &[u32]) -> lsp::CallHierarchyIncomingCall {
            lsp::CallHierarchyIncomingCall {
                from: Some(Box::new(lsp::CallHierarchyItem {
                    uri: lsp::DocumentUri(uri.into()),
                    selection_range: lsp::Range {
                        start: lsp::Position {
                            line: start,
                            character: 1,
                        },
                        end: lsp::Position {
                            line: start,
                            character: 2,
                        },
                    },
                    ..Default::default()
                })),
                from_ranges: ranges
                    .iter()
                    .map(|&line| lsp::Range {
                        start: lsp::Position { line, character: 3 },
                        end: lsp::Position { line, character: 4 },
                    })
                    .collect(),
            }
        }
        let result = merge_incoming_declarations(vec![
            lsp::CallHierarchyIncomingCallsOrNull {
                call_hierarchy_incoming_calls: Some(Box::new(vec![Some(Box::new(call(
                    "file:///z.ts",
                    0,
                    &[2, 3],
                )))])),
            },
            lsp::CallHierarchyIncomingCallsOrNull {
                call_hierarchy_incoming_calls: Some(Box::new(vec![
                    Some(Box::new(call("file:///a.ts", 0, &[1]))),
                    Some(Box::new(call("file:///z.ts", 0, &[3, 4]))),
                    Some(Box::new(call("file:///z.ts", 10, &[11]))),
                ])),
            },
        ]);
        let calls = result.call_hierarchy_incoming_calls.unwrap();
        assert_eq!(calls.len(), 3);
        assert_eq!(
            calls[0].as_ref().unwrap().from.as_ref().unwrap().uri.0,
            "file:///z.ts"
        );
        assert_eq!(
            calls[0]
                .as_ref()
                .unwrap()
                .from_ranges
                .iter()
                .map(|r| r.start.line)
                .collect::<Vec<_>>(),
            [2, 3, 4]
        );
        assert_eq!(
            calls[2]
                .as_ref()
                .unwrap()
                .from
                .as_ref()
                .unwrap()
                .selection_range
                .start
                .line,
            10
        );
    }

    #[test]
    fn incoming_calls_without_callers_are_null_across_projects() {
        let result = combine(
            &Request::IncomingAt(lsp::TextDocumentPositionParams::default()),
            &[raw("null"), raw("null")],
        )
        .unwrap();
        assert_eq!(result.0, b"null");
    }

    #[test]
    fn references_deduplicate_locations_and_keep_first_project_order() {
        let first = raw(
            r#"[{"uri":"file:///a.ts","range":{"start":{"line":1,"character":2},"end":{"line":1,"character":3}}}]"#,
        );
        let second = raw(
            r#"[{"uri":"file:///a.ts","range":{"start":{"line":1,"character":2},"end":{"line":1,"character":3}}},{"uri":"file:///b.ts","range":{"start":{"line":0,"character":0},"end":{"line":0,"character":1}}}]"#,
        );
        let result = combine(
            &Request::References(lsp::ReferenceParams::default()),
            &[first, second],
        )
        .unwrap();
        let locations = decode::<lsp::LocationsOrNull>(&result)
            .unwrap()
            .locations
            .unwrap();
        assert_eq!(locations.len(), 2);
        assert_eq!(locations[0].uri.0, "file:///a.ts");
        assert_eq!(locations[1].uri.0, "file:///b.ts");
    }
    #[test]
    fn rename_deduplicates_ranges_and_resource_moves_but_keeps_document_edits() {
        let item = raw(
            r#"{"changes":{"file:///a.ts":[{"range":{"start":{"line":0,"character":0},"end":{"line":0,"character":1}},"newText":"new"}]},"documentChanges":[{"kind":"rename","oldUri":"file:///a.ts","newUri":"file:///b.ts"}]}"#,
        );
        let result = combine_rename(&[item.clone(), item]).unwrap();
        let edit = decode::<lsp::WorkspaceEditOrNull>(&result)
            .unwrap()
            .workspace_edit
            .unwrap();
        assert_eq!(edit.document_changes.unwrap().len(), 1);
        assert_eq!(
            edit.changes.unwrap()[&lsp::DocumentUri("file:///a.ts".into())].len(),
            1
        );
    }

    // Source: TestGetEditsForFileRename_cssImport4.
    #[test]
    fn css_import_rename_keeps_the_declaration_and_associated_resource_moves() {
        let text = "import styles from \"./app.css\";";
        let mut fs = tsr_vfs::MemoryBuilder::new(b"/", true);
        for (path, content) in [
            (
                "/tsconfig.json",
                r#"{"compilerOptions":{"noLib":true,"allowArbitraryExtensions":true}}"#,
            ),
            ("/a.ts", text),
            ("/app.css", ".cookie-banner { display: none; }"),
            (
                "/app.d.css.ts",
                "declare const css: { cookieBanner: string; }; export default css;",
            ),
        ] {
            fs.insert_loaded(path.as_bytes(), content.as_bytes());
        }
        let host: Arc<dyn FileSystem> = Arc::new(fs.finish());
        let session = Session::new(
            tsr_project::session::SessionOptions::default(),
            host.clone(),
            &tsr_arena::Counters::new(),
        );
        let uri = lsp::DocumentUri("file:///a.ts".into());
        let snapshot = session
            .did_open_file(
                uri.clone(),
                1,
                JsString::from_bytes(text.as_bytes()),
                lsp::LanguageKind("typescript".into()),
            )
            .unwrap();
        let request = Request::Rename(lsp::RenameParams {
            text_document: lsp::TextDocumentIdentifier { uri },
            position: lsp::Position {
                line: 0,
                character: 22,
            },
            new_name: "app2.css".into(),
            ..Default::default()
        });
        let options = Options {
            organize: tsr_ls::OrganizeOptions::default(),
            rename: tsr_ls::RenameOptions::default(),
            formatting: false,
            completion: tsr_ls::CompletionOptions::default(),
            auto_closing_tags: false,
            maximum_hover_length: 0,
            prefer_source_definition: false,
            inlay: tsr_ls::InlayHintsOptions::default(),
            code_lens: tsr_ls::CodeLensOptions::default(),
            lens_command: None,
            locale: tsr_locale::Locale::default(),
        };
        for will_rename in [false, true] {
            let capabilities = decode::<lsp::ClientCapabilities>(&raw(&format!(
                r#"{{"workspace":{{"workspaceEdit":{{"documentChanges":true,"resourceOperations":["rename"]}},"fileOperations":{{"willRename":{will_rename}}}}}}}"#
            )))
            .unwrap();
            let result = execute(
                &Context::background(),
                "css-rename",
                &session,
                &host,
                &snapshot,
                &request,
                PositionEncoding::Utf16,
                &capabilities,
                &options,
            )
            .unwrap();
            let changes = decode::<lsp::WorkspaceEditOrNull>(&result)
                .unwrap()
                .workspace_edit
                .unwrap()
                .document_changes
                .unwrap();
            let moves: Vec<_> = changes
                .iter()
                .filter_map(|change| change.rename_file.as_ref())
                .map(|rename| (rename.old_uri.0.as_str(), rename.new_uri.0.as_str()))
                .collect();
            let declaration = ("file:///app.d.css.ts", "file:///app2.d.css.ts");
            if will_rename {
                assert_eq!(moves, [declaration]);
                assert_eq!(changes.len(), 1);
            } else {
                assert_eq!(
                    moves,
                    [("file:///app.css", "file:///app2.css"), declaration]
                );
                let edits: Vec<_> = changes
                    .iter()
                    .filter_map(|change| change.text_document_edit.as_ref())
                    .collect();
                assert_eq!(edits.len(), 1);
                assert_eq!(edits[0].text_document.uri.0, "file:///a.ts");
                assert_eq!(edits[0].edits.len(), 1);
                assert_eq!(
                    edits[0].edits[0].text_edit.as_ref().unwrap().new_text,
                    "./app2.css"
                );
            }
        }
        session.close();
    }

    // Source: TestFindAllReferencesUmdModuleAsGlobalConst, first two opens.
    #[test]
    fn opening_a_umd_package_then_its_consumer_keeps_module_references_local() {
        let index = "export * from \"./three-core\";\nexport as namespace THREE;";
        let global =
            "import * as _THREE from 'three';\ndeclare global { const THREE: typeof _THREE; }";
        let mut fs = tsr_vfs::MemoryBuilder::new(b"/", true);
        for (name, text) in [
            (
                "/node_modules/@types/three/three-core.d.ts",
                "export class Vector3 { x: number; y: number; }",
            ),
            ("/node_modules/@types/three/index.d.ts", index),
            ("/typings/global.d.ts", global),
            (
                "/src/index.ts",
                "export const a = {};\nlet v = new THREE.Vector2();",
            ),
            (
                "/tsconfig.json",
                r#"{"compilerOptions":{"noLib":true,"esModuleInterop":true,"module":"es6","target":"es6","allowJs":true,"skipLibCheck":true,"typeRoots":["node_modules/@types/"],"types":["three"]},"files":["/src/index.ts","typings/global.d.ts"]}"#,
            ),
        ] {
            fs.insert_loaded(name.as_bytes(), text.as_bytes());
        }
        let host: Arc<dyn FileSystem> = Arc::new(fs.finish());
        let session = Session::new(
            tsr_project::session::SessionOptions::default(),
            host.clone(),
            &tsr_arena::Counters::new(),
        );
        let options = Options {
            organize: tsr_ls::OrganizeOptions::default(),
            rename: tsr_ls::RenameOptions::default(),
            formatting: false,
            completion: tsr_ls::CompletionOptions::default(),
            auto_closing_tags: false,
            maximum_hover_length: 0,
            prefer_source_definition: false,
            inlay: tsr_ls::InlayHintsOptions::default(),
            code_lens: tsr_ls::CodeLensOptions::default(),
            lens_command: None,
            locale: tsr_locale::Locale::default(),
        };
        for (name, text, line, character) in [
            ("/node_modules/@types/three/index.d.ts", index, 1, 20),
            ("/typings/global.d.ts", global, 0, 25),
        ] {
            let uri = lsp::DocumentUri::from_file_name(name.as_bytes());
            let snapshot = session
                .did_open_file(
                    uri.clone(),
                    1,
                    JsString::from_bytes(text.as_bytes()),
                    lsp::LanguageKind("typescript".into()),
                )
                .unwrap();
            let request = Request::References(lsp::ReferenceParams {
                text_document: lsp::TextDocumentIdentifier { uri },
                position: lsp::Position { line, character },
                context: Some(Box::new(lsp::ReferenceContext {
                    include_declaration: true,
                })),
                ..Default::default()
            });
            let result = execute(
                &Context::background(),
                "umd-references",
                &session,
                &host,
                &snapshot,
                &request,
                PositionEncoding::Utf16,
                &lsp::ClientCapabilities::default(),
                &options,
            )
            .unwrap();
            let locations = decode::<lsp::LocationsOrNull>(&result)
                .unwrap()
                .locations
                .unwrap();
            assert_eq!(locations.len(), 1, "{name}: {locations:?}");
            assert_eq!(locations[0].uri.file_name().as_bytes(), name.as_bytes());
            assert_eq!(locations[0].range.start, lsp::Position { line, character });
            assert_eq!(
                locations[0].range.end,
                lsp::Position {
                    line,
                    character: character + 5
                }
            );
        }
        session.close();
    }
    #[test]
    fn visual_studio_reference_ids_are_unique_and_repoint_definitions() {
        let item = raw(
            r#"[{"_vs_id":0,"_vs_location":{"uri":"file:///a.ts","range":{"start":{"line":0,"character":0},"end":{"line":0,"character":1}}}},{"_vs_id":1,"_vs_definitionId":0,"_vs_location":{"uri":"file:///a.ts","range":{"start":{"line":1,"character":0},"end":{"line":1,"character":1}}}}]"#,
        );
        let result = combine(
            &Request::VSReferences(lsp::ReferenceParams::default()),
            &[item.clone(), item],
        )
        .unwrap();
        let items = decode::<lsp::VSReferenceItemsOrNull>(&result)
            .unwrap()
            .vs_reference_items
            .unwrap();
        assert_eq!(
            items
                .iter()
                .map(|item| item.as_ref().unwrap().vs_id)
                .collect::<Vec<_>>(),
            [0, 1, 2, 3]
        );
        assert_eq!(
            items[3].as_ref().unwrap().vs_definition_id.as_deref(),
            Some(&2)
        );
    }

    #[test]
    fn implementation_loads_unopened_consumers_from_the_owner_project() {
        let a = "/*😀*/ export function shared() { return 1; }\nexport interface Service { run(): void; }\n";
        let mut fs = tsr_vfs::MemoryBuilder::new(b"/", true);
        for (name, text) in [
            ("/tsconfig.json", r#"{"files":[],"references":[{"path":"./a"},{"path":"./b"}]}"#),
            ("/a/tsconfig.json", r#"{"compilerOptions":{"noLib":true,"composite":true},"files":["a.ts"]}"#),
            ("/b/tsconfig.json", r#"{"compilerOptions":{"noLib":true,"composite":true},"files":["b.ts"],"references":[{"path":"../a"}]}"#),
            ("/a/a.ts", a),
            ("/b/b.ts", "import {shared, Service} from \"../a/a\";\nexport function callerB() { shared(); }\nexport class B implements Service { run() {} }\n"),
        ] { fs.insert_loaded(name.as_bytes(), text.as_bytes()); }
        let host: Arc<dyn tsr_vfs::FileSystem> = Arc::new(fs.finish());
        let session = Session::new(
            tsr_project::session::SessionOptions::default(),
            host.clone(),
            &tsr_arena::Counters::new(),
        );
        let uri = lsp::DocumentUri("file:///a/a.ts".into());
        let snapshot = session
            .did_open_file(
                uri.clone(),
                1,
                JsString::from_bytes(a.as_bytes()),
                lsp::LanguageKind("typescript".into()),
            )
            .unwrap();
        let request = Request::Implementation(lsp::ImplementationParams {
            text_document: lsp::TextDocumentIdentifier { uri },
            position: lsp::Position {
                line: 1,
                character: 17,
            },
            ..Default::default()
        });
        let options = Options {
            organize: tsr_ls::OrganizeOptions::default(),
            rename: tsr_ls::RenameOptions::default(),
            formatting: false,
            completion: tsr_ls::CompletionOptions::default(),
            auto_closing_tags: false,
            maximum_hover_length: 0,
            prefer_source_definition: false,
            inlay: tsr_ls::InlayHintsOptions::default(),
            code_lens: tsr_ls::CodeLensOptions::default(),
            lens_command: None,
            locale: tsr_locale::Locale::default(),
        };
        let capabilities = decode::<lsp::ClientCapabilities>(&raw(
            r#"{"textDocument":{"implementation":{"linkSupport":true}}}"#,
        ))
        .unwrap();
        let response = execute(
            &Context::background(),
            "implementation",
            &session,
            &host,
            &snapshot,
            &request,
            PositionEncoding::Utf16,
            &capabilities,
            &options,
        )
        .unwrap();
        let response =
            decode::<lsp::LocationOrLocationsOrDefinitionLinksOrNull>(&response).unwrap();
        assert_eq!(response.definition_links.unwrap().len(), 1);
        session.close();
    }

    #[test]
    fn default_query_retains_prepared_snapshot_while_tree_loading_flushes_later_edits() {
        let old_text = "export const value = 1; value;";
        let new_text = "\nexport const value = 1; value;";
        let mut fs = tsr_vfs::MemoryBuilder::new(b"/", true);
        fs.insert_loaded(
            b"/tsconfig.json",
            br#"{"compilerOptions":{"noLib":true},"files":["a.ts"]}"#.as_slice(),
        );
        fs.insert_loaded(b"/a.ts", old_text.as_bytes());
        let host: Arc<dyn tsr_vfs::FileSystem> = Arc::new(fs.finish());
        let session = Session::new(
            tsr_project::session::SessionOptions::default(),
            host.clone(),
            &tsr_arena::Counters::new(),
        );
        let uri = lsp::DocumentUri("file:///a.ts".into());
        let snapshot = session
            .did_open_file(
                uri.clone(),
                1,
                JsString::from_bytes(old_text.as_bytes()),
                lsp::LanguageKind("typescript".into()),
            )
            .unwrap();
        session
            .did_change_file(
                uri.clone(),
                2,
                vec![lsp::TextDocumentContentChangePartialOrWholeDocument {
                    whole_document: Some(Box::new(lsp::TextDocumentContentChangeWholeDocument {
                        text: new_text.into(),
                    })),
                    ..Default::default()
                }],
            )
            .unwrap();
        let request = Request::References(lsp::ReferenceParams {
            text_document: lsp::TextDocumentIdentifier { uri },
            position: lsp::Position {
                line: 0,
                character: 13,
            },
            context: Some(Box::new(lsp::ReferenceContext {
                include_declaration: true,
            })),
            ..Default::default()
        });
        let options = Options {
            organize: tsr_ls::OrganizeOptions::default(),
            rename: tsr_ls::RenameOptions::default(),
            formatting: false,
            completion: tsr_ls::CompletionOptions::default(),
            auto_closing_tags: false,
            maximum_hover_length: 0,
            prefer_source_definition: false,
            inlay: tsr_ls::InlayHintsOptions::default(),
            code_lens: tsr_ls::CodeLensOptions::default(),
            lens_command: None,
            locale: tsr_locale::Locale::default(),
        };
        let result = execute(
            &Context::background(),
            "references",
            &session,
            &host,
            &snapshot,
            &request,
            PositionEncoding::Utf16,
            &lsp::ClientCapabilities::default(),
            &options,
        )
        .unwrap();
        let locations = decode::<lsp::LocationsOrNull>(&result)
            .unwrap()
            .locations
            .unwrap();
        assert_eq!(locations.len(), 2);
        assert!(locations
            .iter()
            .all(|location| location.range.start.line == 0));
        let current = session.snapshot().unwrap();
        let source = current
            .project_for_file(b"/a.ts")
            .unwrap()
            .program()
            .unwrap()
            .source_file(b"/a.ts")
            .unwrap()
            .bound()
            .view()
            .source_file()
            .unwrap();
        assert_eq!(source.text().as_bytes(), new_text.as_bytes());
        session.close();
    }
}
