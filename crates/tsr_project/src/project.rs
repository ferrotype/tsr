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
    // port: tsc/internal/project/project.go:Project.containsFile
    pub fn contains_file(&self, path: &[u8]) -> bool {
        self.program()
            .is_some_and(|program| program.source_file(path).is_some())
    }
}
