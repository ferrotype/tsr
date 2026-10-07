use super::{BTreeMap, BTreeSet, Error, JsString, Project, ProjectBuilder, ProjectKind, VecDeque};
use crate::api::{ApiFile, ApiSnapshotRequest, ProjectTreeRequest, ResourceRequest};

impl ProjectBuilder<'_> {
    // port: tsc/internal/project/projectcollectionbuilder.go:ProjectCollectionBuilder.createAncestorTree
    pub(super) fn discover_ancestor_projects(
        &mut self,
        file: &JsString,
        path: &JsString,
        selected: &JsString,
    ) -> Result<(), Error> {
        if !self.overlays.contains_key(path) {
            return Ok(());
        }
        let mut current = selected.clone();
        let mut seen = BTreeSet::new();
        loop {
            if !seen.insert(current.clone()) {
                break;
            }
            if self
                .projects
                .get(&current)
                .and_then(Project::data)
                .is_some_and(|project| {
                    !project.command_line.options.composite.is_true()
                        || project
                            .command_line
                            .options
                            .disable_solution_searching
                            .is_true()
                })
            {
                break;
            }
            let name = self
                .projects
                .get(&current)
                .and_then(Project::data)
                .map_or_else(|| current.clone(), |project| project.name.clone());
            let ancestor = self
                .configs
                .ancestor_config_file_name(file.as_bytes(), &name)?;
            if ancestor.is_empty() {
                break;
            }
            let key = self.configs.path(ancestor.as_bytes());
            if !self.projects.contains_key(&key) {
                self.delayed_projects
                    .entry(key.clone())
                    .or_insert_with(|| crate::DelayedProject {
                        name: ancestor,
                        path: key.clone(),
                        potential_project_references: BTreeSet::new(),
                    })
                    .potential_project_references
                    .insert(current.clone());
            }
            self.keep.insert(key.clone());
            current = key;
        }
        Ok(())
    }

    fn ensure_project(&mut self, name: &JsString) -> Result<Option<JsString>, Error> {
        let key = self.configs.path(name.as_bytes());
        let Some(command) = self.configs.acquire_for_project(name, &key)? else {
            self.projects.remove(&key);
            self.delayed_projects.remove(&key);
            self.keep.remove(&key);
            self.configs.release_project(&key);
            return Ok(None);
        };
        self.update_project(&key, name, ProjectKind::Configured, command)?;
        self.keep.insert(key.clone());
        Ok(Some(key))
    }

    // port: tsc/internal/project/projectcollectionbuilder.go:ProjectCollectionBuilder.HandleAPIRequest
    pub(super) fn handle_api_request(
        &mut self,
        request: Option<&ApiSnapshotRequest>,
    ) -> Result<Option<JsString>, Error> {
        let empty = ApiSnapshotRequest::default();
        let request = request.unwrap_or(&empty);
        let mut close = BTreeSet::new();
        for key in request.close_projects.iter().flatten() {
            let key = self.configs.path(key.as_bytes());
            if let Some(count) = self.api_state.projects.get_mut(&key) {
                *count -= 1;
                if *count == 0 {
                    self.api_state.projects.remove(&key);
                    close.insert(key);
                }
            }
        }
        for name in request.open_projects.iter().flatten() {
            let Some(key) = self.ensure_project(name)? else {
                return Ok(Some(JsString::from_bytes(
                    [b"project not found for open: ".as_slice(), name.as_bytes()].concat(),
                )));
            };
            *self.api_state.projects.entry(key.clone()).or_default() += 1;
            close.remove(&key);
        }
        for path in request.close_files.iter().flatten() {
            let path = self.configs.path(path.as_bytes());
            if let Some(file) = self.api_state.files.get_mut(&path) {
                file.references -= 1;
                if file.references == 0 {
                    self.api_state.files.remove(&path);
                }
            }
        }
        for uri in request.open_files.iter().flatten() {
            let name = uri.file_name();
            let path = self.configs.path(name.as_bytes());
            let file = self.api_state.files.entry(path).or_insert(ApiFile {
                name: name.clone(),
                references: 0,
            });
            file.name = name;
            file.references += 1;
        }
        for key in self.api_state.projects.keys().cloned().collect::<Vec<_>>() {
            let Some(name) = self
                .projects
                .get(&key)
                .and_then(Project::data)
                .map(|d| d.name.clone())
            else {
                return Ok(Some(JsString::from_bytes(
                    [b"project not found for update: ".as_slice(), key.as_bytes()].concat(),
                )));
            };
            self.ensure_project(&name)?;
        }
        for key in close {
            let retained = self
                .overlays
                .keys()
                .any(|path| self.old.defaults.get(path) == Some(&key));
            if !retained {
                self.projects.remove(&key);
                self.keep.remove(&key);
                self.configs.release_project(&key);
            }
        }
        Ok(None)
    }

    pub(super) fn load_resources(&mut self, request: &ResourceRequest) -> Result<(), Error> {
        for uri in &request.configured_documents {
            let name = uri.file_name();
            let path = self.configs.path(name.as_bytes());
            self.select_configured(&name, &path)?;
        }
        for key in &request.projects {
            // The inferred project is built from its roots above. Its name
            // is an identity, not a config file that can be acquired.
            if self
                .projects
                .get(key)
                .and_then(Project::data)
                .is_some_and(|data| data.kind == ProjectKind::Inferred)
            {
                self.keep.insert(key.clone());
                continue;
            }
            if let Some(name) = self
                .projects
                .get(key)
                .and_then(Project::data)
                .map(|d| d.name.clone())
                .or_else(|| {
                    self.delayed_projects
                        .get(key)
                        .map(|project| project.name.clone())
                })
            {
                self.ensure_project(&name)?;
            }
        }
        if let Some(tree) = &request.project_tree {
            self.load_project_trees(tree)?;
        }
        Ok(())
    }

    // port: tsc/internal/project/projectcollectionbuilder.go:ProjectCollectionBuilder.DidRequestProjectTrees
    // port: tsc/internal/project/projectcollectionbuilder.go:ProjectCollectionBuilder.ensureProjectTree
    fn load_project_trees(&mut self, request: &ProjectTreeRequest) -> Result<(), Error> {
        // Loaded projects and delayed ancestors are roots. Configs visited only
        // through `extends` do not become projects.
        let mut roots: BTreeMap<_, _> = self
            .projects
            .iter()
            .filter_map(|(key, project)| {
                project
                    .data()
                    .filter(|data| data.kind == ProjectKind::Configured)
                    .map(|data| (key.clone(), data.name.clone()))
            })
            .collect();
        roots.extend(
            self.delayed_projects
                .iter()
                .map(|(key, project)| (key.clone(), project.name.clone())),
        );
        let mut queue = VecDeque::new();
        for (key, name) in roots {
            let update =
                match request {
                    ProjectTreeRequest::All => true,
                    ProjectTreeRequest::Referencing(targets) => {
                        self.delayed_projects.get(&key).is_some_and(|project| {
                            project
                                .potential_project_references
                                .iter()
                                .any(|reference| targets.contains(reference))
                        }) || self.projects.get(&key).and_then(Project::data).is_some_and(
                            |project| {
                                project
                                    .command_line
                                    .resolved_project_reference_paths()
                                    .iter()
                                    .any(|reference| {
                                        targets.contains(&self.configs.path(reference.as_bytes()))
                                    })
                            },
                        )
                    }
                };
            queue.push_back((name, update));
        }
        let mut seen = BTreeSet::new();
        while let Some((name, update)) = queue.pop_front() {
            let key = self.configs.path(name.as_bytes());
            // Updating the project precedes the traversal guard at the pin.
            // A delayed root seen earlier may later qualify as a child.
            if update && self.ensure_project(&name)?.is_none() {
                continue;
            }
            if !seen.insert(key.clone()) {
                continue;
            }
            let Some(program) = self.projects.get(&key).and_then(Project::program).cloned() else {
                continue;
            };
            if program.options().disable_referenced_project_load.is_true() {
                continue;
            }
            for child in program.resolved_project_references().flatten() {
                let load = match request {
                    ProjectTreeRequest::All => true,
                    ProjectTreeRequest::Referencing(targets) => !program
                        .range_resolved_project_reference_in_child_config(
                            child,
                            |reference, _, _, _| !targets.contains(reference),
                        ),
                };
                if load {
                    queue.push_back((child.config_name(), true));
                }
            }
        }
        Ok(())
    }
}
