//! Config discovery and copy-on-write registry updates for a session snapshot.
use crate::{
    dirty, extended_config::ConfigOwnership, file_change::FileChangeSummary, overlay::Overlays,
    source_fs::SourceFs,
};
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
};
use tsr_compiler::CompilerConfigHost;
use tsr_core::{CompilerOptions, Tristate};
use tsr_jsstring::JsString;
use tsr_tsoptions::{ConfigValue, ParseConfigHost, ParsedCommandLine};
use tsr_vfs::{Error, FileSystem};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
pub enum PendingReload {
    #[default]
    None,
    FileNames,
    Full,
}

#[derive(Clone)]
pub struct ConfigEntry {
    pub root_files_watch: Option<Arc<crate::watch::WatchedFiles>>,
    pub file_name: JsString,
    pub pending_reload: PendingReload,
    pub command_line: Option<Arc<ParsedCommandLine>>,
    pub retaining_projects: BTreeSet<JsString>,
    pub retaining_open_files: BTreeSet<JsString>,
    pub retaining_configs: BTreeSet<JsString>,
}
impl ConfigEntry {
    fn new(name: JsString, relative: bool) -> Self {
        Self {
            root_files_watch: Some(crate::watch::WatchedFiles::new(
                JsString::from_bytes([b"root files for ".as_slice(), name.as_bytes()].concat()),
                crate::watch::ALL_CHANGES,
                relative,
            )),
            file_name: name,
            pending_reload: PendingReload::Full,
            command_line: None,
            retaining_projects: BTreeSet::new(),
            retaining_open_files: BTreeSet::new(),
            retaining_configs: BTreeSet::new(),
        }
    }
}
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ConfigFileNames {
    pub nearest: JsString,
    pub ancestors: BTreeMap<JsString, JsString>,
}
#[derive(Default)]
pub struct ConfigFileRegistry {
    pub configs: Arc<BTreeMap<JsString, Arc<ConfigEntry>>>,
    pub file_names: Arc<BTreeMap<JsString, Arc<ConfigFileNames>>>,
    pub custom_config_file_name: JsString,
}
impl ConfigFileRegistry {
    // port: tsc/internal/project/configfileregistry.go:ConfigFileRegistry.GetConfig
    pub fn config(&self, path: &JsString) -> Option<&Arc<ParsedCommandLine>> {
        self.configs.get(path)?.command_line.as_ref()
    }
}

#[derive(Default)]
pub struct AffectedConfigs {
    pub projects: BTreeSet<JsString>,
    pub files: BTreeSet<JsString>,
}

/// Config parsing is coordinated by the snapshot builder. The filesystem and
/// extended cache remain shareable; no registry lock is held across callbacks.
pub struct ConfigRegistryBuilder {
    base: Arc<ConfigFileRegistry>,
    configs: dirty::Map<JsString, Arc<ConfigEntry>>,
    names: dirty::Map<JsString, Arc<ConfigFileNames>>,
    host: CompilerConfigHost,
    fs: Arc<SourceFs>,
    overlays: Overlays,
    custom_name: JsString,
    external_code: bool,
    ownership: Arc<ConfigOwnership>,
    relative_patterns: bool,
}
impl ConfigRegistryBuilder {
    pub fn new(
        base: Arc<ConfigFileRegistry>,
        fs: Arc<dyn FileSystem>,
        cwd: JsString,
        overlays: Overlays,
        custom_name: JsString,
        external_code: bool,
        ownership: Arc<ConfigOwnership>,
    ) -> Self {
        let fs = Arc::new(SourceFs::new(fs, cwd.clone(), false));
        let mut names = dirty::Map::new(base.file_names.clone());
        if custom_name != base.custom_config_file_name {
            for key in names.keys() {
                names.remove(&key);
            }
        }
        Self {
            configs: dirty::Map::new(base.configs.clone()),
            relative_patterns: false,
            names,
            base,
            host: CompilerConfigHost::new_live(fs.clone(), cwd),
            fs,
            overlays,
            custom_name,
            external_code,
            ownership,
        }
    }
    pub fn path(&self, name: &[u8]) -> JsString {
        tsr_tspath::to_path(
            name,
            self.host.current_directory(),
            self.fs.use_case_sensitive_file_names(),
        )
    }
    #[must_use]
    pub fn with_relative_patterns(mut self, relative: bool) -> Self {
        self.relative_patterns = relative;
        self
    }
    pub fn configs(&self) -> impl Iterator<Item = (JsString, Arc<ConfigEntry>)> + '_ {
        self.configs.keys().into_iter().map(|key| {
            let value = self.configs.get(&key).unwrap().clone();
            (key, value)
        })
    }
    // port: tsc/internal/project/configfileregistrybuilder.go:configFileRegistryBuilder.computeConfigFileName
    pub fn compute_config_file_name(&self, file: &[u8], ancestor: bool) -> Result<JsString, Error> {
        let start = tsr_tspath::directory(file);
        if !self.custom_name.is_empty() {
            let mut directory = start.clone();
            let mut skip = ancestor;
            loop {
                let candidate = tsr_tspath::combine(&directory, &[self.custom_name.as_bytes()]);
                if !skip && self.fs.file_exists(&candidate)? {
                    return Ok(JsString::from_bytes(candidate));
                }
                if directory.ends_with(b"/node_modules") {
                    break;
                }
                let parent = tsr_tspath::directory(&directory);
                if parent == directory {
                    break;
                }
                directory = parent;
                skip = false;
            }
        }
        let mut directory = start;
        let mut skip_ts = ancestor;
        let mut skip_js = ancestor && !file.ends_with(b"/tsconfig.json");
        loop {
            for (skip, base) in [
                (skip_ts, b"tsconfig.json".as_slice()),
                (skip_js, b"jsconfig.json".as_slice()),
            ] {
                let candidate = tsr_tspath::combine(&directory, &[base]);
                if !skip && self.fs.file_exists(&candidate)? {
                    return Ok(JsString::from_bytes(candidate));
                }
            }
            if directory.ends_with(b"/node_modules") {
                break;
            }
            let parent = tsr_tspath::directory(&directory);
            if parent == directory {
                break;
            }
            directory = parent;
            skip_ts = false;
            skip_js = false;
        }
        Ok(JsString::default())
    }
    // port: tsc/internal/project/configfileregistrybuilder.go:configFileRegistryBuilder.getConfigFileNameForFile
    pub fn config_file_name(&mut self, name: &[u8]) -> Result<JsString, Error> {
        if tsr_tspath::is_dynamic_file_name(name) {
            return Ok(JsString::default());
        }
        let path = self.path(name);
        if let Some(entry) = self.names.get(&path) {
            return Ok(entry.nearest.clone());
        }
        let nearest = self.compute_config_file_name(name, false)?;
        if self.overlays.contains_key(&path) {
            self.names.insert(
                path,
                Arc::new(ConfigFileNames {
                    nearest: nearest.clone(),
                    ..Default::default()
                }),
            );
        }
        Ok(nearest)
    }
    // port: tsc/internal/project/configfileregistrybuilder.go:configFileRegistryBuilder.getAncestorConfigFileName
    pub fn ancestor_config_file_name(
        &mut self,
        file: &[u8],
        config: &JsString,
    ) -> Result<JsString, Error> {
        if tsr_tspath::is_dynamic_file_name(file) {
            return Ok(JsString::default());
        }
        let path = self.path(file);
        let Some(entry) = self.names.get(&path) else {
            // API-opened and request-only files have no editor overlay and do
            // not populate this cache, but still search ancestor configurations.
            return self.compute_config_file_name(config.as_bytes(), true);
        };
        if let Some(result) = entry.ancestors.get(config) {
            return Ok(result.clone());
        }
        let result = self.compute_config_file_name(config.as_bytes(), true)?;
        if self.overlays.contains_key(&path) {
            let mut next = (**entry).clone();
            next.ancestors.insert(config.clone(), result.clone());
            self.names.insert(path, Arc::new(next));
        }
        Ok(result)
    }
    fn change(&mut self, path: &JsString, update: impl FnOnce(&mut ConfigEntry) -> bool) {
        let Some(old) = self.configs.get(path) else {
            return;
        };
        let mut entry = (**old).clone();
        if update(&mut entry) {
            self.configs.insert(path.clone(), Arc::new(entry));
        }
    }
    // port: tsc/internal/project/configfileregistrybuilder.go:configFileRegistryBuilder.reloadIfNeeded
    fn acquire(
        &mut self,
        name: &JsString,
        retainer: &JsString,
        project: bool,
    ) -> Result<Option<Arc<ParsedCommandLine>>, Error> {
        let path = self.path(name.as_bytes());
        let old =
            self.configs.get(&path).cloned().unwrap_or_else(|| {
                Arc::new(ConfigEntry::new(name.clone(), self.relative_patterns))
            });
        let retain = project || self.overlays.contains_key(retainer);
        let retained = if project {
            &old.retaining_projects
        } else {
            &old.retaining_open_files
        };
        if old.pending_reload == PendingReload::None && (!retain || retained.contains(retainer)) {
            return Ok(old.command_line.clone());
        }
        let mut entry = (*old).clone();
        if retain {
            if project {
                entry.retaining_projects.insert(retainer.clone());
            } else {
                entry.retaining_open_files.insert(retainer.clone());
            }
        }
        match entry.pending_reload {
            PendingReload::None => {}
            PendingReload::FileNames => {
                if let Some(command) = &entry.command_line {
                    entry.command_line = Some(Arc::new(
                        command.reload_file_names_of_parsed_command_line(&*self.fs)?,
                    ));
                }
            }
            PendingReload::Full => {
                let options = CompilerOptions {
                    run_external_code: if self.external_code {
                        Tristate::TRUE
                    } else {
                        Tristate::UNKNOWN
                    },
                    ..Default::default()
                };
                entry.command_line = tsr_tsoptions::read_config_with_cache(
                    name.as_bytes(),
                    path.clone(),
                    &options,
                    &ConfigValue::Null,
                    &self.host,
                    Some(&*self.ownership),
                )?
                .command_line
                .map(Arc::new);
                self.update_extending_configs(
                    &path,
                    entry.command_line.as_deref(),
                    old.command_line.as_deref(),
                );
                if let (Some(command), Some(watch)) = (&entry.command_line, &entry.root_files_watch)
                {
                    entry.root_files_watch = Some(watch.with_input(crate::watch::config_patterns(
                        command,
                        name.as_bytes(),
                        self.host.current_directory(),
                        self.fs.use_case_sensitive_file_names(),
                    )));
                }
            }
        }
        entry.pending_reload = PendingReload::None;
        let result = entry.command_line.clone();
        self.configs.insert(path, Arc::new(entry));
        Ok(result)
    }
    // port: tsc/internal/project/configfileregistrybuilder.go:configFileRegistryBuilder.acquireConfigForProject
    pub fn acquire_for_project(
        &mut self,
        name: &JsString,
        project: &JsString,
    ) -> Result<Option<Arc<ParsedCommandLine>>, Error> {
        self.acquire(name, project, true)
    }
    // port: tsc/internal/project/configfileregistrybuilder.go:configFileRegistryBuilder.acquireConfigForFile
    pub fn acquire_for_file(
        &mut self,
        name: &JsString,
        file: &JsString,
    ) -> Result<Option<Arc<ParsedCommandLine>>, Error> {
        self.acquire(name, file, false)
    }
    // port: tsc/internal/project/configfileregistrybuilder.go:configFileRegistryBuilder.updateExtendingConfigs
    fn update_extending_configs(
        &mut self,
        path: &JsString,
        new: Option<&ParsedCommandLine>,
        old: Option<&ParsedCommandLine>,
    ) {
        let mut next = BTreeSet::new();
        for name in new
            .into_iter()
            .flat_map(ParsedCommandLine::extended_source_files)
        {
            let key = self.path(name.as_bytes());
            if !self.configs.contains_key(&key) {
                let mut entry = ConfigEntry::new(name.clone(), self.relative_patterns);
                entry.root_files_watch = None;
                self.configs.insert(key.clone(), Arc::new(entry));
            }
            self.change(&key, |entry| entry.retaining_configs.insert(path.clone()));
            next.insert(key);
        }
        for name in old
            .into_iter()
            .flat_map(ParsedCommandLine::extended_source_files)
        {
            let key = self.path(name.as_bytes());
            if !next.contains(&key) {
                self.change(&key, |entry| entry.retaining_configs.remove(path));
            }
        }
    }
    // port: tsc/internal/project/configfileregistrybuilder.go:configFileRegistryBuilder.releaseConfigForProject
    pub fn release_project(&mut self, project: &JsString) {
        for key in self.configs.keys() {
            self.change(&key, |entry| entry.retaining_projects.remove(project));
        }
    }
    pub(crate) fn retain_project_configs(&mut self, project: &JsString, keep: &BTreeSet<JsString>) {
        for key in self.configs.keys() {
            if !keep.contains(&key) {
                self.change(&key, |entry| entry.retaining_projects.remove(project));
            }
        }
    }
    // port: tsc/internal/project/configfileregistrybuilder.go:configFileRegistryBuilder.didCloseFile
    pub fn close_file(&mut self, path: &JsString) {
        self.names.remove(path);
        for key in self.configs.keys() {
            self.change(&key, |entry| entry.retaining_open_files.remove(path));
        }
    }
    pub(crate) fn existing_config(&self, path: &JsString) -> Option<Arc<ParsedCommandLine>> {
        self.configs.get(path)?.command_line.clone()
    }

    pub(crate) fn custom_name_changed(&self) -> bool {
        self.custom_name != self.base.custom_config_file_name
    }

    pub(crate) fn searched_config_names(&self, file: &JsString) -> Vec<JsString> {
        let Some(names) = self.names.get(file) else {
            return Vec::new();
        };
        let mut result = Vec::new();
        let mut current = &names.nearest;
        while !current.is_empty() {
            result.push(current.clone());
            let Some(ancestor) = names.ancestors.get(current) else {
                break;
            };
            current = ancestor;
        }
        result
    }
    fn mark_config_changed(&mut self, path: &JsString, affected: &mut AffectedConfigs) {
        self.change(path, |entry| {
            if entry.pending_reload == PendingReload::Full {
                return false;
            }
            entry.pending_reload = PendingReload::Full;
            affected
                .projects
                .extend(entry.retaining_projects.iter().cloned());
            true
        });
    }
    // port: tsc/internal/project/configfileregistrybuilder.go:configFileRegistryBuilder.invalidateCache
    fn invalidate(&mut self) -> Result<AffectedConfigs, Error> {
        let mut affected = AffectedConfigs::default();
        for key in self.names.keys() {
            affected.files.insert(key.clone());
            self.names.remove(&key);
        }
        for key in self.configs.keys() {
            let old = self.configs.get(&key).unwrap();
            let mut next = (**old).clone();
            affected
                .projects
                .extend(next.retaining_projects.iter().cloned());
            if next.pending_reload != PendingReload::Full {
                let text = self.fs.read_file(next.file_name.as_bytes())?;
                let previous = next
                    .command_line
                    .as_ref()
                    .and_then(|c| c.config_file.as_ref());
                let equal = text.zip(previous).is_some_and(|(text, previous)| {
                    text.text.as_bytes()
                        == previous
                            .file
                            .view()
                            .source_file(previous.root)
                            .expect("config owner")
                            .text()
                            .as_bytes()
                });
                next.pending_reload = if equal {
                    PendingReload::FileNames
                } else {
                    PendingReload::Full
                };
            }
            self.configs.insert(key, Arc::new(next));
        }
        Ok(affected)
    }
    // port: tsc/internal/project/configfileregistrybuilder.go:configFileRegistryBuilder.DidChangeFiles
    pub fn did_change_files(
        &mut self,
        summary: &FileChangeSummary,
    ) -> Result<AffectedConfigs, Error> {
        let mut affected = AffectedConfigs::default();
        for uri in &summary.closed {
            self.close_file(&self.path(uri.file_name().as_bytes()));
        }
        let excessive = summary.has_excessive_watch_events()
            && summary.includes_watch_change_outside_node_modules;
        let names = |uris: &BTreeSet<tsr_lsproto::DocumentUri>| -> BTreeMap<_, _> {
            uris.iter()
                .filter(|uri| !tsr_tspath::contains_ignored_path(uri.0.as_bytes()))
                .map(|uri| {
                    let name = uri.file_name();
                    (self.path(name.as_bytes()), name)
                })
                .collect()
        };
        let mut created = names(&summary.created);
        let deleted = names(&summary.deleted);
        let mut all = names(&summary.changed);
        all.extend(created.clone());
        all.extend(deleted.clone());
        for (path, name) in &all {
            if let Some(entry) = self.configs.get(path) {
                if excessive {
                    return self.invalidate();
                }
                let extending = entry.retaining_configs.clone();
                self.mark_config_changed(path, &mut affected);
                for key in extending {
                    self.mark_config_changed(&key, &mut affected);
                }
                created.remove(path);
            } else if tsr_tspath::base_name(path.as_bytes()) == b"package.json" {
                let matching: Vec<_> = self
                    .configs()
                    .filter_map(|(key, entry)| {
                        entry
                            .command_line
                            .as_ref()
                            .filter(|command| {
                                command.content_mappers.iter().flatten().any(|mapper| {
                                    !mapper.package.is_empty()
                                        && !mapper.package_directory.is_empty()
                                        && self.path(&tsr_tspath::combine(
                                            mapper.package_directory.as_bytes(),
                                            &[b"package.json"],
                                        )) == *path
                                })
                            })
                            .map(|_| key)
                    })
                    .collect();
                for key in matching {
                    self.mark_config_changed(&key, &mut affected);
                }
            }
            let base = tsr_tspath::base_name(name.as_bytes());
            if base == b"tsconfig.json"
                || base == b"jsconfig.json"
                || !self.custom_name.is_empty() && base == self.custom_name.as_bytes()
            {
                if excessive {
                    return self.invalidate();
                }
                let directory = tsr_tspath::directory(path.as_bytes());
                for key in self.names.keys() {
                    if tsr_tspath::contains_path(&directory, key.as_bytes(), b"", true) {
                        affected.files.insert(key.clone());
                        self.names.remove(&key);
                    }
                }
            }
        }
        for key in self.configs.keys() {
            let entry = self.configs.get(&key).unwrap();
            if entry.pending_reload != PendingReload::None {
                continue;
            }
            let Some(command) = &entry.command_line else {
                continue;
            };
            let removed = deleted.iter().any(|(path, name)| {
                command.file_names_by_path().contains_key(path)
                    && command.matched_file_spec(name.as_bytes()).is_empty()
            });
            let mut added = false;
            for (path, name) in &created {
                if command.possibly_matches_file_name(name.as_bytes())
                    || command.possibly_matches_directory_name(&tsr_tspath::Path::from_bytes(
                        path.as_bytes().to_vec(),
                    )) && self.fs.directory_exists(name.as_bytes())?
                {
                    added = true;
                    break;
                }
            }
            if removed || added {
                if excessive {
                    return self.invalidate();
                }
                self.change(&key, |entry| {
                    entry.pending_reload = PendingReload::FileNames;
                    affected
                        .projects
                        .extend(entry.retaining_projects.iter().cloned());
                    true
                });
            }
        }
        Ok(affected)
    }
    // port: tsc/internal/project/configfileregistrybuilder.go:configFileRegistryBuilder.Cleanup
    pub fn cleanup(&mut self) {
        for key in self.configs.keys() {
            let entry = self.configs.get(&key).unwrap();
            if entry.retaining_projects.is_empty()
                && entry.retaining_open_files.is_empty()
                && entry.retaining_configs.is_empty()
            {
                self.configs.remove(&key);
            }
        }
    }
    // port: tsc/internal/project/configfileregistrybuilder.go:configFileRegistryBuilder.Finalize
    pub fn finalize(self) -> Arc<ConfigFileRegistry> {
        let (configs, changed_configs) = self.configs.finalize();
        let (file_names, changed_names) = self.names.finalize();
        let retained: BTreeSet<_> = configs
            .values()
            .filter_map(|entry| entry.command_line.as_ref())
            .flat_map(|command| command.extended_source_files())
            .map(|name| {
                tsr_tspath::to_path(
                    name.as_bytes(),
                    self.host.current_directory(),
                    self.fs.use_case_sensitive_file_names(),
                )
            })
            .collect();
        self.ownership.retain(|key| retained.contains(key));
        if !changed_configs
            && !changed_names
            && self.custom_name == self.base.custom_config_file_name
        {
            return self.base;
        }
        Arc::new(ConfigFileRegistry {
            configs,
            file_names,
            custom_config_file_name: self.custom_name,
        })
    }
}

#[cfg(test)]
mod tests;
