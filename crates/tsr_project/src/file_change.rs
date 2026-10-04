use std::collections::BTreeSet;
use tsr_jsstring::JsString;
use tsr_lsproto::{DocumentUri, LanguageKind, TextDocumentContentChangePartialOrWholeDocument};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FileChangeKind {
    Open,
    Close,
    Change,
    Save,
    WatchCreate,
    WatchChange,
    WatchDelete,
}
impl FileChangeKind {
    // port: tsc/internal/project/filechange.go:FileChangeKind.IsWatchKind
    pub fn is_watch(self) -> bool {
        matches!(
            self,
            Self::WatchCreate | Self::WatchChange | Self::WatchDelete
        )
    }
}
#[derive(Clone, Debug)]
pub struct FileChange {
    pub kind: FileChangeKind,
    pub uri: DocumentUri,
    pub version: i32,
    pub content: JsString,
    pub language: LanguageKind,
    pub changes: Vec<TextDocumentContentChangePartialOrWholeDocument>,
}
impl FileChange {
    pub fn new(kind: FileChangeKind, uri: DocumentUri) -> Self {
        Self {
            kind,
            uri,
            version: 0,
            content: JsString::default(),
            language: LanguageKind::default(),
            changes: Vec::new(),
        }
    }
}
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FileChangeSummary {
    pub opened: Option<DocumentUri>,
    pub reopened: Option<DocumentUri>,
    pub closed: BTreeSet<DocumentUri>,
    pub changed: BTreeSet<DocumentUri>,
    pub created: BTreeSet<DocumentUri>,
    pub deleted: BTreeSet<DocumentUri>,
    pub includes_watch_change_outside_node_modules: bool,
    pub invalidate_all: bool,
}
impl FileChangeSummary {
    // port: tsc/internal/project/filechange.go:FileChangeSummary.IsEmpty
    pub fn is_empty(&self) -> bool {
        !self.invalidate_all
            && self.opened.is_none()
            && self.reopened.is_none()
            && self.closed.is_empty()
            && self.changed.is_empty()
            && self.created.is_empty()
            && self.deleted.is_empty()
    }
    // port: tsc/internal/project/filechange.go:FileChangeSummary.HasExcessiveWatchEvents
    pub fn has_excessive_watch_events(&self) -> bool {
        self.invalidate_all || self.created.len() + self.deleted.len() + self.changed.len() > 1000
    }
    // port: tsc/internal/project/filechange.go:FileChangeSummary.HasExcessiveNonCreateWatchEvents
    pub fn has_excessive_non_create_watch_events(&self) -> bool {
        self.invalidate_all || self.deleted.len() + self.changed.len() > 1000
    }
    /// The pin merges only watch summaries, not editor open/close events.
    // port: tsc/internal/project/filechange.go:mergeFileChangeSummary
    pub fn merge_watch_changes(&mut self, source: &Self) {
        if source.is_empty() {
            return;
        }
        self.invalidate_all |= source.invalidate_all;
        self.changed.extend(source.changed.iter().cloned());
        self.created.extend(source.created.iter().cloned());
        self.deleted.extend(source.deleted.iter().cloned());
        self.includes_watch_change_outside_node_modules |=
            source.includes_watch_change_outside_node_modules;
    }
}
