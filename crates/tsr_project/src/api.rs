//! Snapshot requests shared by language-service orchestration and the public API.
//! API opens are reference counted independently of editor overlays.
use std::collections::{BTreeMap, BTreeSet};
use tsr_jsstring::JsString;
use tsr_lsproto::DocumentUri;

#[derive(Clone, Debug, Default)]
pub struct ApiSnapshotRequest {
    pub open_projects: Option<BTreeSet<JsString>>,
    pub close_projects: Option<BTreeSet<JsString>>,
    pub open_files: Option<BTreeSet<DocumentUri>>,
    pub close_files: Option<BTreeSet<JsString>>,
}

/// `All` loads every reachable tree; `Referencing` follows only branches that
/// contain a requested project. An empty referenced set therefore loads none.
#[derive(Clone, Debug)]
pub enum ProjectTreeRequest {
    All,
    Referencing(BTreeSet<JsString>),
}

#[derive(Clone, Debug, Default)]
pub struct ResourceRequest {
    pub documents: Vec<DocumentUri>,
    pub configured_documents: Vec<DocumentUri>,
    pub projects: BTreeSet<JsString>,
    pub project_tree: Option<ProjectTreeRequest>,
    pub api: Option<ApiSnapshotRequest>,
    pub watch_changes: crate::file_change::FileChangeSummary,
}
impl ResourceRequest {
    pub fn document(uri: DocumentUri) -> Self {
        Self {
            documents: vec![uri],
            ..Self::default()
        }
    }
    pub(crate) fn needs_update(&self) -> bool {
        !self.configured_documents.is_empty()
            || !self.projects.is_empty()
            || self.project_tree.is_some()
            || self.api.is_some()
            || !self.watch_changes.is_empty()
    }
}

#[derive(Clone, Debug, Default)]
pub(crate) struct ApiState {
    pub projects: BTreeMap<JsString, usize>,
    pub files: BTreeMap<JsString, ApiFile>,
}
#[derive(Clone, Debug)]
pub(crate) struct ApiFile {
    pub name: JsString,
    pub references: usize,
}

/// The pin returns the retained snapshot even when one API open fails. Host and
/// compiler failures still use `Result`; the request error belongs to the snapshot.
pub struct ApiUpdate {
    pub snapshot: crate::Snapshot,
    pub error: Option<JsString>,
}
