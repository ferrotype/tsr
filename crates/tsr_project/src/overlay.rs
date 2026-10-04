//! Immutable file handles and copy-on-write editor overlays. A batch publishes
//! a new map only after every edit and disk check has succeeded.
use crate::file_change::{FileChange, FileChangeKind as K, FileChangeSummary};
use std::{
    collections::BTreeMap,
    sync::{Arc, OnceLock},
};
use tsr_core::ScriptKind;
use tsr_jsstring::lsp::lsp_line_and_character_to_position;
use tsr_jsstring::{JsString, LspLineMap, LspPosition, PositionEncoding};
use tsr_lsproto::{DocumentUri, LanguageKind};
use tsr_vfs::{Error, FileSystem};

#[derive(Debug)]
pub struct FileHandle {
    name: JsString,
    content: JsString,
    hash: u128,
    version: i32,
    kind: ScriptKind,
    overlay: bool,
    matches_disk: bool,
    line_map: OnceLock<LspLineMap>,
}
impl FileHandle {
    // port: tsc/internal/project/overlayfs.go:newOverlay
    pub fn overlay(name: JsString, content: JsString, version: i32, kind: ScriptKind) -> Self {
        Self {
            hash: xxhash_rust::xxh3::xxh3_128(content.as_bytes()),
            name,
            content,
            version,
            kind,
            overlay: true,
            matches_disk: false,
            line_map: OnceLock::new(),
        }
    }
    // port: tsc/internal/project/overlayfs.go:newDiskFile
    pub fn disk(name: JsString, content: JsString) -> Self {
        let kind = ScriptKind::from_file_name(name.as_bytes());
        Self {
            overlay: false,
            matches_disk: true,
            ..Self::overlay(name, content, 0, kind)
        }
    }
    pub fn file_name(&self) -> &JsString {
        &self.name
    }
    pub fn content(&self) -> &JsString {
        &self.content
    }
    pub fn hash(&self) -> u128 {
        self.hash
    }
    pub fn version(&self) -> i32 {
        self.version
    }
    pub fn kind(&self) -> ScriptKind {
        self.kind
    }
    pub fn is_overlay(&self) -> bool {
        self.overlay
    }
    pub fn matches_disk_text(&self) -> bool {
        self.matches_disk
    }
    // port: tsc/internal/project/overlayfs.go:fileBase.LSPLineMap
    pub fn line_map(&self) -> &LspLineMap {
        self.line_map
            .get_or_init(|| LspLineMap::new(self.content.as_bytes()))
    }
    fn with_disk_match(&self, matches_disk: bool) -> Self {
        Self {
            matches_disk,
            ..Self::overlay(
                self.name.clone(),
                self.content.clone(),
                self.version,
                self.kind,
            )
        }
    }
    // port: tsc/internal/project/overlayfs.go:Overlay.computeMatchesDiskText
    fn compute_disk_match(&self, fs: &dyn FileSystem) -> Result<(bool, bool), Error> {
        if tsr_tspath::is_dynamic_file_name(self.name.as_bytes()) {
            return Ok((false, false));
        }
        let Some(content) = fs.read_file(self.name.as_bytes())? else {
            return Ok((false, false));
        };
        Ok((
            xxhash_rust::xxh3::xxh3_128(content.text.as_bytes()) == self.hash,
            true,
        ))
    }
}
pub type Overlays = Arc<BTreeMap<JsString, Arc<FileHandle>>>;
pub struct OverlayFs {
    fs: Arc<dyn FileSystem>,
    cwd: JsString,
    encoding: PositionEncoding,
    overlays: Overlays,
}
impl OverlayFs {
    // port: tsc/internal/project/overlayfs.go:newOverlayFS
    pub fn new(fs: Arc<dyn FileSystem>, cwd: JsString, encoding: PositionEncoding) -> Self {
        Self {
            fs,
            cwd,
            encoding,
            overlays: Arc::default(),
        }
    }
    pub fn overlays(&self) -> &Overlays {
        &self.overlays
    }
    pub fn file_system(&self) -> &Arc<dyn FileSystem> {
        &self.fs
    }
    fn to_path(&self, name: &[u8]) -> JsString {
        tsr_tspath::to_path(
            name,
            self.cwd.as_bytes(),
            self.fs.use_case_sensitive_file_names(),
        )
    }
    // port: tsc/internal/project/overlayfs.go:overlayFS.getFile
    pub fn get_file(&self, name: &[u8]) -> Result<Option<Arc<FileHandle>>, Error> {
        if let Some(file) = self.overlays.get(&self.to_path(name)) {
            return Ok(Some(file.clone()));
        }
        Ok(self.fs.read_file(name)?.map(|file| {
            Arc::new(FileHandle::disk(
                JsString::from_bytes(name),
                file.text
                    .slice(0..file.text.len())
                    .expect("whole file text"),
            ))
        }))
    }
    // port: tsc/internal/project/overlayfs.go:overlayFS.processChanges
    pub fn process_changes(&mut self, changes: &[FileChange]) -> Result<FileChangeSummary, Error> {
        #[derive(Default)]
        struct Events<'a> {
            open: Option<&'a FileChange>,
            close: bool,
            watched: bool,
            changes: Vec<&'a FileChange>,
            saved: bool,
            created: bool,
            deleted: bool,
        }
        let mut grouped: BTreeMap<&DocumentUri, Events<'_>> = BTreeMap::new();
        let mut result = FileChangeSummary::default();
        for change in changes {
            let events = grouped.entry(&change.uri).or_default();
            assert!(events.open.is_none(), "should see no changes after open");
            if change.kind.is_watch() && !change.uri.0.contains("/node_modules/") {
                result.includes_watch_change_outside_node_modules = true;
            }
            match change.kind {
                K::Open => {
                    *events = Events {
                        open: Some(change),
                        ..Default::default()
                    };
                }
                K::Close => {
                    events.close = true;
                    events.changes.clear();
                    events.saved = false;
                    events.watched = false;
                }
                K::Change => {
                    assert!(!events.close, "should see no changes after close");
                    events.changes.push(change);
                    events.saved = false;
                    events.watched = false;
                }
                K::Save => {
                    events.saved = true;
                }
                K::WatchCreate => {
                    if events.deleted {
                        events.deleted = false;
                        events.watched = true;
                    } else {
                        events.created = true;
                    }
                }
                K::WatchChange => {
                    if !events.created {
                        events.watched = true;
                        events.saved = false;
                    }
                }
                K::WatchDelete => {
                    events.watched = false;
                    events.saved = false;
                    if events.created {
                        events.created = false;
                    } else {
                        events.deleted = true;
                    }
                }
            }
        }
        let mut overlays = (*self.overlays).clone();
        for (uri, events) in grouped {
            let path = uri.path(self.fs.use_case_sensitive_file_names());
            let mut overlay = overlays.get(&path).cloned();
            if let Some(open) = events.open {
                assert!(
                    result.opened.is_none() && result.reopened.is_none(),
                    "can only process one file open event at a time"
                );
                match &overlay {
                    Some(old) if old.content != open.content => {
                        result.changed.insert(uri.clone());
                    }
                    None => {
                        result.opened = Some(uri.clone());
                    }
                    _ => {
                        result.reopened = Some(uri.clone());
                    }
                }
                let name = uri.file_name();
                let kind = language_kind_to_script_kind(&open.language);
                let kind = if kind == ScriptKind::UNKNOWN {
                    ScriptKind::from_file_name(name.as_bytes())
                } else {
                    kind
                };
                overlays.insert(
                    path,
                    Arc::new(FileHandle::overlay(
                        name,
                        open.content.clone(),
                        open.version,
                        kind,
                    )),
                );
                continue;
            }
            if events.close && overlay.is_some() {
                result.closed.insert(uri.clone());
                overlays.remove(&path);
                overlay = None;
            }
            if events.watched {
                if let Some(old) = &overlay {
                    if !events.saved {
                        let (matches, _) = old.compute_disk_match(&*self.fs)?;
                        if matches != old.matches_disk {
                            let next = Arc::new(old.with_disk_match(matches));
                            overlays.insert(path.clone(), next.clone());
                            overlay = Some(next);
                        }
                    }
                } else {
                    result.changed.insert(uri.clone());
                }
            }
            if !events.changes.is_empty() && overlay.is_some() {
                result.changed.insert(uri.clone());
                for change in events.changes {
                    for edit in &change.changes {
                        let old = overlay.as_ref().expect("changes have an overlay");
                        let content = if let Some(partial) = &edit.partial {
                            let position = |p: tsr_lsproto::Position| {
                                lsp_line_and_character_to_position(
                                    old.content.as_bytes(),
                                    old.line_map(),
                                    LspPosition {
                                        line: p.line,
                                        character: p.character,
                                    },
                                    self.encoding,
                                ) as usize
                            };
                            let start = position(partial.range.start.clone());
                            let end = position(partial.range.end.clone());
                            let mut bytes = old.content.as_bytes()[..start].to_vec();
                            bytes.extend(partial.text.as_bytes());
                            bytes.extend(&old.content.as_bytes()[end..]);
                            JsString::from_bytes(bytes)
                        } else if let Some(whole) = &edit.whole_document {
                            JsString::from_bytes(whole.text.as_bytes())
                        } else {
                            continue;
                        };
                        overlay = Some(Arc::new(FileHandle::overlay(
                            old.name.clone(),
                            content,
                            change.version,
                            old.kind,
                        )));
                    }
                    if !change.changes.is_empty() {
                        let old = overlay.as_ref().expect("changed overlay");
                        // A union with neither arm still advances the version in Go.
                        let next = if old.version == change.version && !old.matches_disk {
                            old.clone()
                        } else {
                            Arc::new(FileHandle::overlay(
                                old.name.clone(),
                                old.content.clone(),
                                change.version,
                                old.kind,
                            ))
                        };
                        overlays.insert(path.clone(), next.clone());
                        overlay = Some(next);
                    }
                }
            }
            if events.saved {
                if let Some(old) = &overlay {
                    overlays.insert(path.clone(), Arc::new(old.with_disk_match(true)));
                } else if !events.watched {
                    result.changed.insert(uri.clone());
                }
            }
            if overlay.is_none() {
                if events.created {
                    result.created.insert(uri.clone());
                }
                if events.deleted {
                    result.deleted.insert(uri.clone());
                }
            }
        }
        self.overlays = Arc::new(overlays);
        Ok(result)
    }
}
// port: tsc/internal/ls/lsconv/converters.go:LanguageKindToScriptKind
pub fn language_kind_to_script_kind(language: &LanguageKind) -> ScriptKind {
    match language.0.as_str() {
        "typescript" => ScriptKind::TS,
        "typescriptreact" => ScriptKind::TSX,
        "javascript" => ScriptKind::JS,
        "javascriptreact" => ScriptKind::JSX,
        "json" => ScriptKind::JSON,
        _ => ScriptKind::UNKNOWN,
    }
}

#[cfg(test)]
mod tests;
