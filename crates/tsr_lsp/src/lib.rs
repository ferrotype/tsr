//! Production document/project dispatch. Transports own admission and pump
//! callbacks independently; this worker boundary may perform synchronous I/O.
mod capabilities;
pub mod client;
pub mod connection;
pub mod content_mappers;
mod diagnostics;
pub mod dynamic_queue;
pub mod logger;
pub mod progress;
mod recovery;
pub mod rpc_client;
pub mod runtime;
pub mod stack_sanitizer;
#[cfg(test)]
mod tests;
pub mod watcher;
use std::sync::Arc;
use tsr_json::{Decode, RawValue};
use tsr_jsstring::JsString;
use tsr_lsproto::{self as lsp, ResponseError};
use tsr_project::{
    file_change::{FileChange, FileChangeKind as K},
    session::Session,
    Snapshot,
};
use tsr_vfs::FileSystem;

pub struct Server {
    session: Arc<Session>,
}
impl Server {
    pub fn new(session: Arc<Session>) -> Self {
        Self { session }
    }
    pub fn session(&self) -> &Arc<Session> {
        &self.session
    }

    /// Apply a document notification in connection order. Its snapshot barrier
    /// uses the caller's I/O cancellation scope without changing the session's
    /// default host, which background updates and future requests still use.
    // port: tsc/internal/lsp/server.go:Server.handleDidOpen
    // port: tsc/internal/lsp/server.go:Server.handleDidChange
    // port: tsc/internal/lsp/server.go:Server.handleDidClose
    // port: tsc/internal/lsp/server.go:Server.handleDidSave
    // port: tsc/internal/lsp/server.go:Server.handleDidChangeWatchedFiles
    pub fn notification(
        &self,
        method: &str,
        params: Option<&RawValue>,
        host: Arc<dyn FileSystem>,
    ) -> Result<(), ResponseError> {
        let change = match method {
            "textDocument/didOpen" => {
                let params: lsp::DidOpenTextDocumentParams = decode(params)?;
                let doc = params
                    .text_document
                    .ok_or_else(|| invalid("missing textDocument"))?;
                let mut change = FileChange::new(K::Open, doc.uri);
                change.version = doc.version;
                change.content = JsString::from_bytes(doc.text.as_bytes());
                change.language = doc.language_id;
                self.session.enqueue(change).map_err(project_error)?;
                self.session
                    .flush_with_host(None, host)
                    .map_err(project_error)?;
                return Ok(());
            }
            "textDocument/didChange" => {
                let params: lsp::DidChangeTextDocumentParams = decode(params)?;
                let mut change = FileChange::new(K::Change, params.text_document.uri);
                change.version = params.text_document.version;
                change.changes = params.content_changes;
                change
            }
            "textDocument/didClose" => {
                let params: lsp::DidCloseTextDocumentParams = decode(params)?;
                FileChange::new(K::Close, params.text_document.uri)
            }
            "textDocument/didSave" => {
                let params: lsp::DidSaveTextDocumentParams = decode(params)?;
                FileChange::new(K::Save, params.text_document.uri)
            }
            "workspace/didChangeWatchedFiles" => {
                let params: lsp::DidChangeWatchedFilesParams = decode(params)?;
                for event in params.changes.into_iter().flatten() {
                    let kind = match event.r#type.0 {
                        1 => K::WatchCreate,
                        2 => K::WatchChange,
                        3 => K::WatchDelete,
                        _ => continue,
                    };
                    self.session
                        .enqueue(FileChange::new(kind, event.uri))
                        .map_err(project_error)?;
                }
                return Ok(());
            }
            _ => {
                return Err(ResponseError {
                    code: -32601,
                    message: format!("method not implemented: {method}"),
                    data: None,
                })
            }
        };
        self.session.enqueue(change).map_err(project_error)
    }
    pub fn snapshot(&self, host: Arc<dyn FileSystem>) -> Result<Snapshot, ResponseError> {
        self.session
            .flush_with_host(None, host)
            .map_err(project_error)
    }
    pub fn close(&self) {
        self.session.close();
    }
}
fn decode<T: Decode + Default + 'static>(params: Option<&RawValue>) -> Result<T, ResponseError> {
    lsp::unmarshal_params(params)
}
fn invalid(message: &str) -> ResponseError {
    ResponseError {
        code: -32602,
        message: message.into(),
        data: None,
    }
}
fn canceled() -> ResponseError {
    error(-32800, "request cancelled")
}
fn error(code: i32, message: impl Into<String>) -> ResponseError {
    ResponseError {
        code,
        message: message.into(),
        data: None,
    }
}
#[allow(
    clippy::needless_pass_by_value,
    reason = "map_err transfers the failed operation's error"
)]
fn project_error(error: tsr_project::session::Error) -> ResponseError {
    ResponseError {
        code: -32603,
        message: error.to_string(),
        data: None,
    }
}
