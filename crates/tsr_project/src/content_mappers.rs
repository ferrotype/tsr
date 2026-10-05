//! Session-owned mapper processes and program-owned project leases. The shared
//! host performs the protocol; this layer connects its lifecycle to snapshots.
use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex, Weak},
};
use tsr_contentmapper::{Host, Project as MapperProject};
use tsr_jsstring::JsString;
use tsr_tsoptions::{config_mappers::ContentMapper, ParsedCommandLine};

#[derive(Clone, Default, Debug)]
pub struct Contributions {
    pub mappers: Vec<ContentMapper>,
    pub extensions: Vec<String>,
}

pub struct MapperHost {
    host: tsr_contentmapper::HostImpl,
    projects: Mutex<BTreeMap<usize, Weak<ProjectLease>>>,
    locale: Mutex<tsr_locale::Locale>,
}
impl MapperHost {
    // port: tsc/internal/project/session.go:newContentMapperHost
    pub fn new(
        trusted: bool,
        spawner: Option<Arc<dyn tsr_contentmapper::Spawner>>,
        context: &tsr_ipc::Context,
        locale: tsr_locale::Locale,
        logger: Option<tsr_contentmapper::Logger>,
    ) -> Option<Self> {
        if !trusted {
            return None;
        }
        let spawner = spawner?;
        Some(Self {
            host: tsr_contentmapper::new_host_with_options(
                context,
                spawner,
                locale.clone(),
                tsr_contentmapper::HostOptions { logger },
            ),
            projects: Mutex::default(),
            locale: Mutex::new(locale),
        })
    }
    // port: tsc/internal/project/compilerhost.go:compilerHost.ensureContentMapperProject
    pub fn project(&self, command: &Arc<ParsedCommandLine>) -> Option<Arc<dyn MapperProject>> {
        if command.content_mappers.as_ref().is_none_or(Vec::is_empty) {
            return None;
        }
        let key = Arc::as_ptr(command) as usize;
        let mut projects = self.projects.lock().expect("mapper project index");
        if let Some(project) = projects.get(&key).and_then(Weak::upgrade) {
            return Some(project);
        }
        projects.retain(|_, project| project.strong_count() != 0);
        let project = self.host.project(tsr_contentmapper::ProjectSpec {
            config_file_name: command.config_name(),
            mappers: command.content_mappers.clone().unwrap_or_default().into(),
            compiler_options: Arc::new(command.options.clone()),
        })?;
        let lease = Arc::new(ProjectLease {
            project,
            _command: command.clone(),
        });
        projects.insert(key, Arc::downgrade(&lease));
        Some(lease)
    }
    pub fn locale(&self) -> String {
        self.locale.lock().expect("mapper locale").to_string()
    }
    pub fn set_locale(&self, locale: tsr_locale::Locale) {
        self.host.set_locale(locale.clone());
        *self.locale.lock().expect("mapper locale") = locale;
    }
    pub fn timings(&self) -> tsr_contentmapper::Timings {
        self.host.timings()
    }
    pub fn close(&self) {
        let _ = self.host.close();
    }
}
impl Drop for MapperHost {
    fn drop(&mut self) {
        self.close();
    }
}

struct ProjectLease {
    project: Arc<dyn MapperProject>,
    _command: Arc<ParsedCommandLine>,
}
impl Drop for ProjectLease {
    fn drop(&mut self) {
        let _ = self.project.close();
    }
}
impl MapperProject for ProjectLease {
    fn refresh(&self) -> Result<(), tsr_contentmapper::Error> {
        self.project.refresh()
    }
    fn identities(&self) -> Result<Vec<String>, tsr_contentmapper::Error> {
        self.project.identities()
    }
    fn identity(&self, mapper: usize) -> Result<String, tsr_contentmapper::Error> {
        self.project.identity(mapper)
    }
    fn watched_files(&self) -> Result<Vec<String>, tsr_contentmapper::Error> {
        self.project.watched_files()
    }
    fn diagnostics(&self) -> Vec<tsr_contentmapper::OptionDiagnostic> {
        self.project.diagnostics()
    }
    fn transform(
        &self,
        mapper: usize,
        request: &tsr_contentmapper::Request,
    ) -> Result<tsr_contentmapper::TransformResult, tsr_contentmapper::Error> {
        self.project.transform(mapper, request)
    }
    fn close(&self) -> Result<(), tsr_contentmapper::Error> {
        self.project.close()
    }
}

/// Files that invalidate a mapper configuration, including resolved package
/// manifests even when mapper initialization or the manifest itself failed.
pub fn watched_files(
    command: &ParsedCommandLine,
    project: Option<&Arc<dyn MapperProject>>,
) -> Vec<JsString> {
    let mut files: Vec<_> = command
        .content_mappers
        .iter()
        .flatten()
        .filter(|mapper| {
            !mapper.package.is_empty()
                && mapper.contribution_id.is_empty()
                && !mapper.package_directory.is_empty()
        })
        .map(|mapper| {
            JsString::from_bytes(tsr_tspath::combine(
                mapper.package_directory.as_bytes(),
                &[b"package.json"],
            ))
        })
        .collect();
    if let Some(project) = project {
        files.extend(
            project
                .watched_files()
                .unwrap_or_default()
                .into_iter()
                .map(|file| JsString::from_bytes(file.into_bytes())),
        );
    }
    files.sort();
    files.dedup();
    files
}

#[cfg(test)]
mod tests;
