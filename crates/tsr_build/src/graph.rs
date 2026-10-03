use crate::{
    d, lock, Arc, BuildTask, Diagnostic, Error, HashMap, JsString, Mutex, Node, Orchestrator,
    Ordering, ParsedCommandLine, WorkGroup,
};
use std::collections::HashSet;
use tsr_core::collections::OrderedMap;
use tsr_tsoptions::{ConfigValue, ExtendedConfigCache};

impl Orchestrator {
    // port: tsc/internal/execute/build/orchestrator.go:Orchestrator.GenerateGraph
    pub fn generate_graph(&mut self) -> Result<(), Error> {
        for node in &self.tasks {
            node.task.close_project();
        }
        self.generate_graph_with_tasks(HashMap::new(), true)
    }

    // port: tsc/internal/execute/build/orchestrator.go:Orchestrator.GenerateGraphReusingOldTasks
    pub fn generate_graph_reusing_old_tasks(&mut self) -> Result<(), Error> {
        let tasks = self
            .tasks
            .iter()
            .map(|node| (self.path(node.task.config.as_bytes()), node.task.clone()))
            .collect();
        self.generate_graph_with_tasks(tasks, false)
    }

    fn generate_graph_with_tasks(
        &mut self,
        old: HashMap<JsString, Arc<BuildTask>>,
        initial: bool,
    ) -> Result<(), Error> {
        self.tasks.clear();
        self.by_path.clear();
        self.order.clear();
        self.order_indices.clear();
        self.errors.clear();
        let roots = self.opts.command.resolved_project_paths().to_vec();
        let host =
            tsr_compiler::CompilerConfigHost::new_live(self.host.fs.clone(), self.host.cwd.clone());
        let cache = ExtendedConfigCache::new(&host);
        let mut raw = OrderedMap::default();
        if self.opts.command.raw.as_object().is_some() {
            raw.insert(
                JsString::from_bytes(b"compilerOptions".as_slice()),
                self.opts.command.raw.clone(),
            );
        }
        let raw = ConfigValue::Object(raw);
        let mut scheduled = HashSet::new();
        let mut pending = roots.clone();
        // Configs discovered by one batch become the next batch. This keeps
        // dynamically discovered graph work bounded without recursive worker waits.
        while !pending.is_empty() {
            pending.retain(|name| scheduled.insert(self.path(name.as_bytes())));
            let parsed = Mutex::new(Vec::new());
            let group =
                WorkGroup::new(self.opts.command.compiler_options.single_threaded.is_true());
            for (position, name) in pending.iter().enumerate() {
                let (cache, raw, parsed, opts, old, host) =
                    (&cache, &raw, &parsed, &self.opts, &old, &self.host);
                group.queue(move || {
                    let previous = old.get(&host.path(name.as_bytes()));
                    if let Some(task) = previous.filter(|task| !task.dirty.load(Ordering::Acquire))
                    {
                        lock(parsed).push((position, Ok::<_, Error>(task.clone())));
                        return;
                    }
                    let start = opts.sys.now();
                    let result = cache.read_config_file(
                        name.as_bytes(),
                        &opts.command.compiler_options,
                        raw,
                    );
                    let time = tsr_tsc::elapsed(opts.sys.now(), start);
                    let task = result
                        .map(|result| {
                            let task = BuildTask::new(name.clone(), result.command_line, time);
                            task.initial_cycle.store(initial, Ordering::Release);
                            if let Some(previous) = previous {
                                lock(&task.state)
                                    .info
                                    .clone_from(&lock(&previous.state).info);
                            }
                            Arc::new(task)
                        })
                        .map_err(Error::from);
                    lock(parsed).push((position, task));
                });
            }
            group.run_and_wait();
            drop(group);
            let mut parsed = parsed
                .into_inner()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            parsed.sort_by_key(|item| item.0);
            pending.clear();
            for (_, task) in parsed {
                let task = task?;
                if let Some(config) = &task.resolved {
                    pending.extend_from_slice(config.resolved_project_reference_paths());
                }
                self.by_path
                    .insert(self.path(task.config.as_bytes()), self.tasks.len());
                self.tasks.push(Node {
                    task,
                    upstream: Vec::new(),
                    downstream: Vec::new(),
                });
            }
        }
        let mut completed = HashSet::new();
        let mut analyzing = HashSet::new();
        for root in roots {
            self.setup_task(
                self.index(root.as_bytes()),
                None,
                false,
                &mut completed,
                &mut analyzing,
                &mut Vec::new(),
            );
        }
        for (path, task) in old {
            if !self
                .by_path
                .get(&path)
                .is_some_and(|&index| Arc::ptr_eq(&self.tasks[index].task, &task))
            {
                task.close_project();
            }
        }
        Ok(())
    }

    // port: tsc/internal/execute/build/orchestrator.go:Orchestrator.setupBuildTask
    fn setup_task(
        &mut self,
        index: usize,
        downstream: Option<usize>,
        circular: bool,
        completed: &mut HashSet<usize>,
        analyzing: &mut HashSet<usize>,
        stack: &mut Vec<JsString>,
    ) -> Option<usize> {
        if !completed.contains(&index) {
            if !analyzing.insert(index) {
                if !circular {
                    self.errors.push(Diagnostic::compiler(
                        d::Project_references_may_not_form_a_circular_graph_Cycle_detected_Colon_0,
                        vec![JsString::from_bytes(
                            stack
                                .iter()
                                .map(JsString::as_bytes)
                                .collect::<Vec<_>>()
                                .join(&b'\n'),
                        )],
                    ));
                }
                return None;
            }
            let config_name = self.tasks[index].task.config.clone();
            stack.push(config_name.clone());
            let references = self.tasks[index]
                .task
                .resolved
                .as_ref()
                .map(|config| {
                    config
                        .resolved_project_reference_paths()
                        .iter()
                        .zip(config.project_references.iter().flatten())
                        .map(|(name, reference)| (name.clone(), reference.circular))
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();
            for (ref_index, (name, ref_circular)) in references.into_iter().enumerate() {
                if let Some(upstream) = self.setup_task(
                    self.index(name.as_bytes()),
                    Some(index),
                    circular || ref_circular,
                    completed,
                    analyzing,
                    stack,
                ) {
                    self.tasks[index].upstream.push((upstream, ref_index));
                }
            }
            stack.pop();
            completed.insert(index);
            self.order.push(config_name);
            self.order_indices.push(index);
        }
        if self.opts.command.compiler_options.watch.is_true() {
            if let Some(downstream) = downstream {
                self.tasks[index].downstream.push(downstream);
            }
        }
        Some(index)
    }
}

impl tsr_compiler::ResolvedProjectReferenceProvider for Orchestrator {
    fn get_resolved_project_reference(
        &self,
        _file_name: &[u8],
        path: &[u8],
    ) -> Option<Arc<ParsedCommandLine>> {
        self.tasks[*self
            .by_path
            .get(path)
            .expect("reference belongs to build graph")]
        .task
        .project_reference()
    }
}
