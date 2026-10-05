use crate::{
    config::ConfigFileRegistry, extended_config::ConfigOwnership,
    program_counter::ProgramReference, project::INFERRED_PROJECT_NAME, snapshot_fs::SnapshotFs,
    Project,
};
use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    sync::Arc,
};
use tsr_core::CompilerOptions;
use tsr_jsstring::JsString;

/// A configured project discovered while walking solution ancestors. Its
/// configuration and program are loaded only when a resource request needs it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DelayedProject {
    pub name: JsString,
    pub path: JsString,
    pub potential_project_references: BTreeSet<JsString>,
}

pub(crate) struct SessionSnapshot {
    pub id: u64,
    pub parent: u64,
    pub fs: Arc<SnapshotFs>,
    pub configs: Arc<ConfigFileRegistry>,
    pub projects: BTreeMap<JsString, Project>,
    pub delayed_projects: BTreeMap<JsString, DelayedProject>,
    pub defaults: BTreeMap<JsString, JsString>,
    pub contributions: Arc<crate::content_mappers::Contributions>,
    pub api_state: crate::api::ApiState,
    pub api_error: Option<JsString>,
    pub inferred_options: Option<Arc<CompilerOptions>>,
    pub config_ownership: Arc<ConfigOwnership>,
    pub _programs: Vec<ProgramReference>,
}
impl SessionSnapshot {
    pub(crate) fn mapper_watch_state(
        &self,
    ) -> (Vec<JsString>, std::collections::BTreeSet<JsString>) {
        let mut extensions: Vec<_> = self
            .contributions
            .extensions
            .iter()
            .map(|e| JsString::from_bytes(e.as_bytes()))
            .collect();
        for config in self
            .configs
            .configs
            .values()
            .filter_map(|c| c.command_line.as_ref())
        {
            extensions.extend(config.content_mapper_extensions());
        }
        extensions.sort();
        extensions.dedup();
        let files = self
            .projects
            .values()
            .filter_map(|p| p.data())
            .flat_map(|data| data.content_mapper_watched_files.iter().cloned())
            .collect();
        (extensions, files)
    }

    pub(crate) fn watches(&self) -> crate::watch::WatchSet {
        self.configs
            .configs
            .iter()
            .filter_map(|(path, config)| {
                config.root_files_watch.as_ref().map(|watch| {
                    (
                        JsString::from_bytes([b"config:".as_slice(), path.as_bytes()].concat()),
                        watch.clone(),
                    )
                })
            })
            .chain(self.projects.iter().map(|(path, project)| {
                (
                    JsString::from_bytes([b"project:".as_slice(), path.as_bytes()].concat()),
                    project.data().unwrap().program_files_watch.clone(),
                )
            }))
            .chain(self.projects.iter().map(|(path, project)| {
                (
                    JsString::from_bytes([b"mapper:".as_slice(), path.as_bytes()].concat()),
                    project.data().unwrap().content_mapper_watch.clone(),
                )
            }))
            .chain(self.projects.iter().filter_map(|(path, project)| {
                project.data().unwrap().typings_watch.as_ref().map(|watch| {
                    (
                        JsString::from_bytes([b"typings:".as_slice(), path.as_bytes()].concat()),
                        watch.clone(),
                    )
                })
            }))
            .collect()
    }
}
#[derive(Clone)]
enum Root {
    Standalone(Project),
    Session(Arc<SessionSnapshot>),
}
#[derive(Clone)]
pub struct Snapshot {
    root: Root,
}
impl Snapshot {
    pub fn content_mapper_extensions(&self) -> Vec<JsString> {
        self.state()
            .map_or_else(Vec::new, |state| state.mapper_watch_state().0)
    }
    /// An embedding snapshot can own one project without a language-server session.
    pub fn new(project: Project) -> Self {
        Self {
            root: Root::Standalone(project),
        }
    }
    pub(crate) fn from_session(state: SessionSnapshot) -> Self {
        Self {
            root: Root::Session(Arc::new(state)),
        }
    }
    pub(crate) fn state(&self) -> Option<&Arc<SessionSnapshot>> {
        match &self.root {
            Root::Standalone(_) => None,
            Root::Session(state) => Some(state),
        }
    }
    /// The single-project embedding API; session callers select by file or name.
    pub fn project(&self) -> &Project {
        match &self.root {
            Root::Standalone(project) => project,
            Root::Session(state) => {
                assert_eq!(
                    state.projects.len(),
                    1,
                    "select a project in a multi-project snapshot"
                );
                state.projects.values().next().unwrap()
            }
        }
    }
    pub fn id(&self) -> Option<u64> {
        self.state().map(|s| s.id)
    }
    pub fn parent_id(&self) -> Option<u64> {
        self.state().map(|s| s.parent)
    }
    pub fn filesystem(&self) -> Option<&Arc<SnapshotFs>> {
        self.state().map(|s| &s.fs)
    }
    pub fn configs(&self) -> Option<&Arc<ConfigFileRegistry>> {
        self.state().map(|s| &s.configs)
    }
    // port: tsc/internal/project/projectcollection.go:ProjectCollection.Projects
    pub fn projects(&self) -> Vec<&Project> {
        match &self.root {
            Root::Standalone(project) => vec![project],
            Root::Session(state) => {
                let mut projects: Vec<_> = state
                    .projects
                    .iter()
                    .filter(|(key, _)| key.as_bytes() != INFERRED_PROJECT_NAME)
                    .map(|(_, value)| value)
                    .collect();
                projects.sort_by(|a, b| a.data().unwrap().name.cmp(&b.data().unwrap().name));
                projects.extend(state.projects.get(INFERRED_PROJECT_NAME));
                projects
            }
        }
    }
    /// Ancestor projects whose command lines and programs have not been loaded.
    pub fn delayed_projects(&self) -> Vec<&DelayedProject> {
        self.state().map_or_else(Vec::new, |state| {
            let mut projects: Vec<_> = state.delayed_projects.values().collect();
            projects.sort_by(|a, b| a.name.cmp(&b.name));
            projects
        })
    }
    pub fn project_by_path(&self, path: &[u8]) -> Option<&Project> {
        self.state()?.projects.get(path)
    }
    // port: tsc/internal/project/projectcollection.go:ProjectCollection.GetDefaultProject
    pub fn project_for_file(&self, path: &[u8]) -> Option<&Project> {
        let state = self.state()?;
        if let Some(project) = state.defaults.get(path) {
            return state.projects.get(project);
        }
        let containing: Vec<_> = self
            .projects()
            .into_iter()
            .filter(|project| {
                project.data().unwrap().path.as_bytes() != INFERRED_PROJECT_NAME
                    && project.contains_file(path)
            })
            .collect();
        if containing.is_empty() {
            return state
                .projects
                .get(INFERRED_PROJECT_NAME)
                .filter(|p| p.contains_file(path));
        }
        if containing.len() == 1 {
            return containing.first().copied();
        }
        let direct: Vec<_> = containing
            .iter()
            .copied()
            .filter(|p| !p.program().unwrap().is_source_from_project_reference(path))
            .collect();
        if direct.len() <= 1 {
            return direct.first().or_else(|| containing.first()).copied();
        }
        self.default_configured_project(path)
            .or_else(|| containing.first().copied())
    }

    // port: tsc/internal/project/projectcollection.go:ProjectCollection.findDefaultConfiguredProjectWorker
    fn default_configured_project(&self, path: &[u8]) -> Option<&Project> {
        let state = self.state()?;
        let names = state.configs.file_names.get(path)?;
        let mut config_name = names.nearest.clone();
        let mut visited = BTreeSet::new();
        let mut fallback = None;
        loop {
            let project = state.projects.get(&state.fs.path(config_name.as_bytes()))?;
            let mut queue = VecDeque::from([project]);
            while let Some(project) = queue.pop_front() {
                let data = project.data().unwrap();
                if !visited.insert(data.path.clone()) {
                    continue;
                }
                if project.contains_file(path) {
                    if !data.program.is_source_from_project_reference(path) {
                        return Some(project);
                    }
                    fallback = fallback.or(Some(project));
                }
                queue.extend(
                    data.command_line
                        .resolved_project_reference_paths()
                        .iter()
                        .filter_map(|name| state.projects.get(&state.fs.path(name.as_bytes()))),
                );
            }
            // The pin consults the registry at the requested path here, rather
            // than the last project's command line.
            if state
                .configs
                .config(&JsString::from_bytes(path))
                .is_some_and(|c| c.options.disable_solution_searching.is_true())
            {
                return fallback;
            }
            match names
                .ancestors
                .get(&config_name)
                .filter(|name| !name.is_empty())
            {
                Some(ancestor) => config_name = ancestor.clone(),
                None => return fallback,
            }
        }
    }
}
