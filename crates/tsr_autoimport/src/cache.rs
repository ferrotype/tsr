use crate::Registry;
use std::{
    collections::{HashMap, HashSet},
    sync::{Arc, Mutex},
};
use tsr_ast::NodeId;
use tsr_checker::Error;
use tsr_compiler::Program;
use tsr_jsstring::JsString;

/// Snapshot-local publication. Building never holds the mutex; published
/// indexes contain no checker handles and are immutable through every alias.
pub struct Cache {
    sources: HashMap<JsString, NodeId>,
    state: Mutex<State>,
}
struct State {
    index: Option<Arc<Registry>>,
    dirty: HashSet<JsString>,
}
pub(crate) fn sources(program: &Program) -> HashMap<JsString, NodeId> {
    program
        .files()
        .iter()
        .map(|f| {
            let source = f.bound().view().source_file().expect("bound source");
            (JsString::from_bytes(source.path()), f.source())
        })
        .collect()
}
impl Cache {
    pub fn new(program: &Program) -> Self {
        Self {
            sources: sources(program),
            state: Mutex::new(State {
                index: None,
                dirty: HashSet::new(),
            }),
        }
    }
    /// Like the pin's single-dirty-file fast path, editing the requested file
    /// need not discard exports from unchanged modules it previously imported.
    /// Other source changes are checked against the index before reuse.
    pub fn for_update(program: &Program, previous: &Self, dirty: &JsString) -> Self {
        Self::with_changes(program, previous, Some(dirty))
    }
    /// Go's project bucket survives a root-list change when no source or
    /// dependency resolution changed. `get` still rejects any newly added file.
    /// The caller must rule out option, package and filesystem changes.
    pub fn for_root_change(program: &Program, previous: &Self) -> Self {
        Self::with_changes(program, previous, None)
    }
    fn with_changes(program: &Program, previous: &Self, dirty: Option<&JsString>) -> Self {
        let state = previous.state.lock().unwrap();
        let mut changed = state.dirty.clone();
        changed.extend(dirty.cloned());
        Self {
            sources: sources(program),
            state: Mutex::new(State {
                index: state.index.clone(),
                dirty: changed,
            }),
        }
    }
    pub fn get(
        &self,
        program: &Program,
        requested: &[u8],
        preferences: &crate::Preferences,
    ) -> Result<Option<Arc<Registry>>, Error> {
        if self.sources != sources(program) {
            return Err(tsr_arena::Error::WrongOwner.into());
        }
        let state = self.state.lock().unwrap();
        if state.dirty.iter().any(|p| p.as_bytes() != requested) {
            return Ok(None);
        }
        Ok(state
            .index
            .as_ref()
            .filter(|index| {
                index.build_key == preferences.build_key()
                    && index
                        .requested_file
                        .as_ref()
                        .is_none_or(|file| file.as_bytes() == requested)
                    && self.sources.iter().all(|(path, id)| {
                        // New files always require extraction, even the requested file.
                        index
                            .sources
                            .get(path)
                            .is_some_and(|old| old == id || path.as_bytes() == requested)
                    })
            })
            .cloned())
    }
    pub fn is_prepared(&self) -> bool {
        self.state.lock().unwrap().index.is_some()
    }
    pub fn dependencies(&self) -> crate::Dependencies {
        self.state
            .lock()
            .unwrap()
            .index
            .as_ref()
            .map_or_else(crate::Dependencies::default, |index| {
                index.dependencies.clone()
            })
    }
    pub fn publish(&self, index: Registry) -> Arc<Registry> {
        let index = Arc::new(index);
        let mut state = self.state.lock().unwrap();
        state.index = Some(index.clone());
        state.dirty.clear();
        index
    }
}
