//! Serialized replacement of editor registrations from immutable snapshot state.
use crate::client::{self, Client};
use std::sync::Mutex;
use tsr_lsproto::{self as lsp, Any};
#[derive(Default)]
struct State {
    snapshot: u64,
    extensions: Vec<String>,
    registered: bool,
}
#[derive(Default)]
pub(crate) struct MapperRegistrations {
    state: Mutex<State>,
}
fn object(fields: impl IntoIterator<Item = (&'static str, Any)>) -> Any {
    Any::Object(
        fields
            .into_iter()
            .map(|(key, value)| (key.into(), value))
            .collect(),
    )
}
fn string(value: &str) -> Any {
    Any::String(value.into())
}
fn get<'a>(value: &'a Any, path: &[&str]) -> Option<&'a Any> {
    let mut value = value;
    for key in path {
        let Any::Object(fields) = value else {
            return None;
        };
        value = fields.get(*key)?;
    }
    Some(value)
}
fn enabled(value: &Any, path: &[&str]) -> bool {
    get(value, path) == Some(&Any::Boolean(true))
}
fn value(value: &impl tsr_json::Encode) -> Result<Any, lsp::ResponseError> {
    let mut output = Any::Null;
    tsr_json::unmarshal(
        &client::raw(value)?.0,
        &mut output,
        tsr_json::Options::default(),
    )
    .map_err(|e| crate::invalid(&e.to_string()))?;
    Ok(output)
}
// (registration ID suffix, client capability, LSP method, advertised provider)
const REGISTRATIONS: &[(&str, &str, &str, &str)] = &[
    ("did-open", "synchronization", "textDocument/didOpen", ""),
    (
        "did-change",
        "synchronization",
        "textDocument/didChange",
        "",
    ),
    ("did-close", "synchronization", "textDocument/didClose", ""),
    (
        "diagnostic",
        "diagnostic",
        "textDocument/diagnostic",
        "diagnosticProvider",
    ),
    ("hover", "hover", "textDocument/hover", "hoverProvider"),
    (
        "signature-help",
        "signatureHelp",
        "textDocument/signatureHelp",
        "signatureHelpProvider",
    ),
    (
        "definition",
        "definition",
        "textDocument/definition",
        "definitionProvider",
    ),
    (
        "type-definition",
        "typeDefinition",
        "textDocument/typeDefinition",
        "typeDefinitionProvider",
    ),
    (
        "implementation",
        "implementation",
        "textDocument/implementation",
        "implementationProvider",
    ),
    (
        "references",
        "references",
        "textDocument/references",
        "referencesProvider",
    ),
    (
        "document-highlight",
        "documentHighlight",
        "textDocument/documentHighlight",
        "documentHighlightProvider",
    ),
    (
        "completion",
        "completion",
        "textDocument/completion",
        "completionProvider",
    ),
    ("rename", "rename", "textDocument/rename", "renameProvider"),
    (
        "semantic-tokens",
        "semanticTokens",
        "textDocument/semanticTokens",
        "semanticTokensProvider",
    ),
    (
        "document-symbol",
        "documentSymbol",
        "textDocument/documentSymbol",
        "documentSymbolProvider",
    ),
    (
        "folding-range",
        "foldingRange",
        "textDocument/foldingRange",
        "foldingRangeProvider",
    ),
    (
        "selection-range",
        "selectionRange",
        "textDocument/selectionRange",
        "selectionRangeProvider",
    ),
    (
        "inlay-hint",
        "inlayHint",
        "textDocument/inlayHint",
        "inlayHintProvider",
    ),
    (
        "code-lens",
        "codeLens",
        "textDocument/codeLens",
        "codeLensProvider",
    ),
    (
        "code-action",
        "codeAction",
        "textDocument/codeAction",
        "codeActionProvider",
    ),
    (
        "formatting",
        "formatting",
        "textDocument/formatting",
        "documentFormattingProvider",
    ),
    (
        "range-formatting",
        "rangeFormatting",
        "textDocument/rangeFormatting",
        "documentRangeFormattingProvider",
    ),
    (
        "on-type-formatting",
        "onTypeFormatting",
        "textDocument/onTypeFormatting",
        "documentOnTypeFormattingProvider",
    ),
    (
        "linked-editing",
        "linkedEditingRange",
        "textDocument/linkedEditingRange",
        "linkedEditingRangeProvider",
    ),
    (
        "call-hierarchy",
        "callHierarchy",
        "textDocument/prepareCallHierarchy",
        "callHierarchyProvider",
    ),
    ("will-rename-files", "", "workspace/willRenameFiles", ""),
];
impl MapperRegistrations {
    // port: tsc/internal/lsp/server.go:Server.RegisterContentMapperExtensions
    // port: tsc/internal/project/session.go:Session.updateContentMapperRegistrations
    pub(crate) fn update(
        &self,
        client: &dyn Client,
        context: &tsr_ipc::Context,
        caps: &lsp::ClientCapabilities,
        snapshot: &tsr_project::Snapshot,
    ) -> Result<(), lsp::ResponseError> {
        let Some(snapshot_id) = snapshot.id() else {
            return Ok(());
        };
        let caps_value = value(caps)?;
        if !enabled(
            &caps_value,
            &["textDocument", "synchronization", "dynamicRegistration"],
        ) {
            return Ok(());
        }
        let extensions = snapshot
            .content_mapper_extensions()
            .into_iter()
            .map(|extension| {
                String::from_utf8(extension.as_bytes().to_vec())
                    .map_err(|e| crate::invalid(&e.to_string()))
            })
            .collect::<Result<Vec<_>, _>>()?;
        let mut state = self.state.lock().expect("mapper registration state");
        if snapshot_id <= state.snapshot {
            return Ok(());
        }
        if state.extensions == extensions {
            state.snapshot = snapshot_id;
            return Ok(());
        }
        let supported: Vec<_> = REGISTRATIONS
            .iter()
            .filter(|(_, cap, _, _)| {
                if cap.is_empty() {
                    enabled(
                        &caps_value,
                        &["workspace", "fileOperations", "dynamicRegistration"],
                    ) && enabled(&caps_value, &["workspace", "fileOperations", "willRename"])
                } else {
                    enabled(&caps_value, &["textDocument", cap, "dynamicRegistration"])
                }
            })
            .collect();
        if state.registered {
            let registrations = Any::Array(
                supported
                    .iter()
                    .map(|(id, _, method, _)| {
                        object([
                            ("id", string(&format!("content-mapper-{id}"))),
                            ("method", string(method)),
                        ])
                    })
                    .collect(),
            );
            client.request(
                context,
                "client/unregisterCapability",
                client::raw(&object([("unregisterations", registrations)]))?,
            )?;
            state.registered = false;
        }
        if !extensions.is_empty() {
            let server = value(&crate::capabilities::initialize(
                caps,
                tsr_jsstring::PositionEncoding::Utf16,
            ))?;
            let selector = Any::Array(
                extensions
                    .iter()
                    .map(|extension| object([("pattern", string(&format!("**/*{extension}")))]))
                    .collect(),
            );
            let registrations = Any::Array(
                supported
                    .iter()
                    .map(|(id, _, method, provider)| {
                        let mut options = match get(&server, &["capabilities", provider]) {
                            Some(Any::Object(fields)) => fields.clone(),
                            _ => std::collections::HashMap::default(),
                        };
                        if *id == "will-rename-files" {
                            options.insert(
                                "filters".into(),
                                Any::Array(
                                    extensions
                                        .iter()
                                        .map(|extension| {
                                            object([
                                                ("scheme", string("file")),
                                                (
                                                    "pattern",
                                                    object([(
                                                        "glob",
                                                        string(&format!("**/*{extension}")),
                                                    )]),
                                                ),
                                            ])
                                        })
                                        .collect(),
                                ),
                            );
                        } else {
                            options.insert("documentSelector".into(), selector.clone());
                        }
                        if *id == "did-change" {
                            options.insert("syncKind".into(), Any::Number(2.0));
                        }
                        object([
                            ("id", string(&format!("content-mapper-{id}"))),
                            ("method", string(method)),
                            ("registerOptions", Any::Object(options)),
                        ])
                    })
                    .collect(),
            );
            client.request(
                context,
                "client/registerCapability",
                client::raw(&object([("registrations", registrations)]))?,
            )?;
            state.registered = true;
        }
        state.extensions = extensions;
        state.snapshot = snapshot_id;
        Ok(())
    }
}
