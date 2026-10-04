use crate::{source_fs::SourceFs, CheckerPool, Project};
use std::sync::Arc;
use tsr_arena::Counters;
use tsr_compiler::{Program, ProgramCheckerHost};
use tsr_jsstring::JsString;
use tsr_tsoptions::ParsedCommandLine;

pub const INFERRED_PROJECT_NAME: &[u8] = b"/dev/null/inferred";
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProjectKind {
    Inferred,
    Configured,
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ProgramUpdateKind {
    #[default]
    None,
    Cloned,
    SameFileNames,
    NewFiles,
}

#[derive(Clone)]
pub struct ProjectData {
    pub program_files_watch: Arc<crate::watch::WatchedFiles>,
    pub name: JsString,
    pub path: JsString,
    pub kind: ProjectKind,
    pub current_directory: JsString,
    pub command_line: Arc<ParsedCommandLine>,
    pub program: Arc<Program>,
    pub update_kind: ProgramUpdateKind,
    pub last_update: u64,
    pub host: Arc<SourceFs>,
    // Like Go's dirty/dirtyFilePath, these belong to this snapshot, not the
    // shared program. A dirty project with no single file needs a full rebuild.
    pub(crate) dirty: bool,
    pub(crate) dirty_file: Option<JsString>,
    // Config-discovery preference used when this project was last selected.
    // Closed projects may be retained across a preference change; program
    // inclusion alone must not resurrect their old default selection.
    pub(crate) config_search: JsString,
}
impl Project {
    pub(crate) fn from_program(data: ProjectData, counters: &Counters, queries: usize) -> Self {
        let pool = CheckerPool::for_program(
            Arc::new(ProgramCheckerHost::new(data.program.clone())),
            counters,
            queries,
        );
        let scheduler = crate::scheduler::CheckerScheduler::new(
            pool.clone(),
            data.program.clone(),
            std::time::Duration::ZERO,
        );
        Self {
            completion_host: None,
            auto_imports: Some(Arc::new(tsr_autoimport::Cache::new(&data.program))),
            pool,
            scheduler: Some(scheduler),
            data: Some(Arc::new(data)),
        }
    }
    pub fn data(&self) -> Option<&Arc<ProjectData>> {
        self.data.as_ref()
    }
    pub fn program(&self) -> Option<&Arc<Program>> {
        self.data.as_ref().map(|d| &d.program)
    }
    // port: tsc/internal/project/project.go:Project.DisplayName
    pub fn display_name(&self, cwd: &[u8]) -> Option<JsString> {
        let data = self.data.as_ref()?;
        Some(JsString::from_bytes(
            if data.kind == ProjectKind::Inferred {
                tsr_tspath::base_name(data.current_directory.as_bytes()).to_vec()
            } else {
                tsr_tspath::convert_to_relative_path(data.name.as_bytes(), cwd, true)
            },
        ))
    }
    // port: tsc/internal/project/project.go:Project.containsFile
    pub fn contains_file(&self, path: &[u8]) -> bool {
        self.program()
            .is_some_and(|program| program.source_file(path).is_some())
    }
}
